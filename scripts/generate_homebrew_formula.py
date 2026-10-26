#!/usr/bin/env python3
"""Generate a macOS Homebrew formula from the release's checked SHA256SUMS."""

import argparse
from pathlib import Path
import re


TARGETS = ("aarch64-apple-darwin", "x86_64-apple-darwin")


def render(version: str, checksums: str) -> str:
    if re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version) is None:
        raise ValueError("version must be X.Y.Z")
    expected = {f"codex-commit-v{version}-{target}.tar.gz" for target in TARGETS}
    parsed = {}
    for line in checksums.splitlines():
        if not line.strip():
            continue
        match = re.fullmatch(r"([0-9a-fA-F]{64}) [ *](\S+)", line)
        if match is None:
            raise ValueError("malformed SHA256SUMS entry")
        digest, filename = match.groups()
        if filename not in expected:
            raise ValueError(f"unexpected release asset: {filename}")
        if filename in parsed:
            raise ValueError(f"duplicate release asset: {filename}")
        parsed[filename] = digest.lower()
    if set(parsed) != expected:
        raise ValueError("SHA256SUMS must contain both macOS release archives")
    arm, intel = (f"codex-commit-v{version}-{target}.tar.gz" for target in TARGETS)
    base = f"https://github.com/syz51/codex-commit/releases/download/v{version}"
    return f'''class CodexCommit < Formula
  desc "Commit Git changes with Codex Luna at Max reasoning, with optional push"
  homepage "https://github.com/syz51/codex-commit"
  version "{version}"
  license "MIT"

  on_macos do
    on_arm do
      url "{base}/{arm}"
      sha256 "{parsed[arm]}"
    end
    on_intel do
      url "{base}/{intel}"
      sha256 "{parsed[intel]}"
    end
  end

  depends_on :macos
  depends_on "git"

  def install
    bin.install "codex-commit"
  end

  def caveats
    <<~EOS
      Install Codex CLI separately and run `codex login` before using codex-commit.
      The default mode grants Codex full local command access. Use --push to enable pushing.
    EOS
  end

  test do
    assert_match "codex-commit {version}", shell_output("#{{bin}}/codex-commit --version")
    assert_match "Usage:", shell_output("#{{bin}}/codex-commit --help")
    system "git", "init", "-b", "main"
    output = shell_output("#{{bin}}/codex-commit --dry-run")
    assert_match "Push: disabled", output
    assert_match "Model: gpt-6-luna", output
    assert_match "Reasoning: max", output
  end
end
'''


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True)
    parser.add_argument("--checksums", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    try:
        formula = render(args.version, args.checksums.read_text())
    except (OSError, ValueError) as error:
        parser.error(str(error))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(formula)


if __name__ == "__main__":
    main()
