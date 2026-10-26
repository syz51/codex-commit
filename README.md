# codex-commit

[![CI](https://github.com/syz51/codex-commit/actions/workflows/ci.yml/badge.svg)](https://github.com/syz51/codex-commit/actions/workflows/ci.yml)

One command to inspect and commit your existing Git changes with **GPT-6 Luna at Max reasoning**. Add `--push` to push afterward. A small, dependency-free shell wrapper around Codex CLI.

```sh
codex-commit
```

Codex shows status, diff summaries, commands, and progress as it works. Each run creates a saved Codex CLI session.

## Install

Requires a POSIX shell, Git, and [Codex CLI](https://developers.openai.com/codex/cli/) with access to `gpt-6-luna`. Authenticate with `codex login` and configure your normal Git identity first. A remote and Git push credentials are needed only with `--push`; commit-only mode does not require them. Tested with Codex CLI 0.164.0.

```sh
git clone https://github.com/syz51/codex-commit.git
cd codex-commit
make install
```

This installs `codex-commit` in `~/.local/bin`; add that directory to your `PATH` if needed. To choose another location, use `make install PREFIX=/usr/local`. `DESTDIR` is supported for packaging. No Codex or shell configuration is changed.

## Use

```sh
codex-commit                              # commit in the current repository
codex-commit --push                       # commit, then push
codex-commit -C ~/project                 # another repository
codex-commit 'Commit only the README change'
codex-commit --dry-run                    # inspect the prompt without running Codex
codex-commit --interactive                # interactive Codex with approval prompts
codex-commit 2>&1 | tee codex-commit.log   # capture progress and final output
```

Options precede the optional instructions. Use `--` before instructions that begin with a dash. The command runs from the repository root, including when invoked in a subdirectory. Bare repositories and detached HEADs are rejected.

The default mode runs unattended and exits when Codex finishes. It grants Codex full local command access so it can update Git metadata. Run it only in repositories you trust. `--interactive` instead uses the workspace-write sandbox and Codex permission prompts; its terminal stays open for follow-up messages. Both modes explicitly select the normal service tier, without enabling Fast mode.

The task asks Codex to inspect staged, unstaged, and untracked changes; follow repository instructions; run relevant checks; and commit existing intended changes without rewriting history. By default its final policy forbids pushing, publishing, fetching, or pulling, even when repository or additional instructions mention pushing. Rerun with `--push` to enable pushing.

With `--push`, the task also asks Codex to push the current branch without force. It can push existing local commits from a clean worktree. With no upstream, it may set one only when `origin` is the sole remote. Both push modes are prompt policies, not OS-enforced network or Git restrictions; review the streamed output.

Progress streams directly to stderr, and the final response goes to stdout in unattended mode. **Ctrl-C interrupts Codex**; normal signals and exit status pass through because the wrapper replaces itself with Codex. Cancellation cannot undo a commit or push that already completed. Inspect `git status` and `git log` before retrying an interrupted run.

Use the session ID printed by Codex to continue an unattended run interactively:

```sh
codex resume --include-non-interactive SESSION_ID
```

For the underlying contracts, see the official [Luna model documentation](https://developers.openai.com/api/docs/models/gpt-6-luna) and [Codex non-interactive mode documentation](https://developers.openai.com/codex/noninteractive/).

## Development

```sh
make check
make lint  # requires ShellCheck
make test  # requires uv and Python
```

CI runs the integration tests and installation checks on Linux and macOS, with shell linting on Linux. The automated tests use a mock Codex executable and real disposable Git repositories; they do not call a model or require credentials.

See [CONTRIBUTING.md](CONTRIBUTING.md) for pull requests and [SECURITY.md](SECURITY.md) for private vulnerability reports.

[MIT licensed](LICENSE). Not affiliated with OpenAI.
