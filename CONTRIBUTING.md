# Contributing

Open an issue for a bug or proposal, or send a focused pull request. For vulnerabilities, use the private reporting instructions in [SECURITY.md](SECURITY.md).

Keep the runtime dependency-free beyond a POSIX shell, Git, and Codex CLI. Preserve streamed terminal output, signal handling, and the commit-only default. Add a focused test for behavior changes.

## Checks

Development requires [uv](https://docs.astral.sh/uv/) and [ShellCheck](https://www.shellcheck.net/):

```sh
make check
make lint
make test
```

Tests use Python's standard library, a mock Codex executable, disposable local Git remotes, and real terminal signal handling. They do not need Codex login, a model, or network push credentials. CI also checks installation on Linux and macOS.

Create a branch and open a pull request against `main`. The protected branch requires passing CI before merging. Describe the user-visible change and how you verified it; include only sanitized output. Do not commit credentials or private repository content.
