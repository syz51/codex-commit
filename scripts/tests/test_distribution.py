"""Focused validation for release metadata that becomes executable Ruby."""

import importlib.util
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest


SPEC = importlib.util.spec_from_file_location(
    "generate_homebrew_formula", Path(__file__).parents[1] / "generate_homebrew_formula.py"
)
GENERATOR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GENERATOR)


class FormulaTests(unittest.TestCase):
    def checksums(self):
        return "\n".join(
            f"{'a' * 64}  codex-commit-v0.2.0-{target}.tar.gz"
            for target in GENERATOR.TARGETS
        )

    def test_both_native_archives_and_digests_are_bound_to_version(self):
        formula = GENERATOR.render("0.2.0", self.checksums())
        self.assertEqual(formula.count('sha256 "' + "a" * 64 + '"'), 2)
        for target in GENERATOR.TARGETS:
            self.assertIn(f"/v0.2.0/codex-commit-v0.2.0-{target}.tar.gz", formula)
        self.assertIn('assert_match "Push: disabled"', formula)

    def test_rejects_missing_duplicate_or_unexpected_assets(self):
        lines = self.checksums().splitlines()
        for checksum in (lines[0], self.checksums() + "\n" + lines[0],
                         self.checksums().replace("v0.2.0", "v0.3.0")):
            with self.subTest(checksum=checksum):
                with self.assertRaises(ValueError):
                    GENERATOR.render("0.2.0", checksum)

    def test_rejects_injection_or_malformed_digest(self):
        for version in ('0.2.0"; system "false', "../0.2.0", "v0.2.0"):
            with self.subTest(version=version):
                with self.assertRaises(ValueError):
                    GENERATOR.render(version, self.checksums())
        with self.assertRaises(ValueError):
            GENERATOR.render("0.2.0", self.checksums().replace("a" * 64, "z" * 64))


class PackageTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="codex-commit-package-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.target = "aarch64-apple-darwin"
        binary = self.root / "target" / self.target / "release" / "codex-commit"
        binary.parent.mkdir(parents=True)
        binary.write_text("#!/bin/sh\nprintf 'codex-commit 0.2.0\\n'\n")
        binary.chmod(0o755)
        self.env = dict(os.environ, CARGO_TARGET_DIR=str(self.root / "target"), TMPDIR=str(self.root))

    def package(self, target=None, version="0.2.0"):
        return subprocess.run(
            [str(Path(__file__).parents[1] / "package-release.sh"), target or self.target,
             version, str(self.root / "output with spaces")],
            text=True, capture_output=True, env=self.env, timeout=10,
        )

    def test_archive_contains_only_executable_and_public_docs(self):
        result = self.package()
        self.assertEqual(result.returncode, 0, result.stderr)
        with tarfile.open(result.stdout.strip()) as archive:
            self.assertEqual(set(archive.getnames()), {"codex-commit", "LICENSE", "README.md"})
            self.assertTrue(archive.getmember("codex-commit").mode & 0o111)

    def test_rejects_unsupported_target_and_version_mismatch(self):
        for target, version in (("x86_64-pc-windows-msvc", "0.2.0"), (self.target, "0.3.0")):
            with self.subTest(target=target, version=version):
                self.assertNotEqual(self.package(target, version).returncode, 0)


if __name__ == "__main__":
    unittest.main()
