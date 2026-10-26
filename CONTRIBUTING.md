# Contributing

Open an issue for a bug or proposal, or send a focused pull request. For vulnerabilities, use the private reporting instructions in [SECURITY.md](SECURITY.md).

Keep the Rust executable small and preserve direct terminal output, process replacement, signal handling, and the commit-only default. Add focused tests for behavior changes. Avoid dependencies unless they solve a concrete maintenance problem.

## Checks

Development uses the pinned Rust toolchain in `rust-toolchain.toml`, [uv](https://docs.astral.sh/uv/), and Python's standard library:

```sh
make check
make test
uv run --no-project python -B -m unittest discover -s scripts/tests -v
cargo publish --dry-run --locked
```

Tests use a mock Codex executable, disposable local Git remotes, and real terminal signal handling. They do not need Codex login or network push credentials. CI also checks installation on Linux, Apple Silicon macOS, and Intel macOS. Put manual-test artifacts in one scratch directory and remove them after testing.

Create a branch and open a pull request against `main`. The protected branch requires passing CI before merging. Describe the user-visible change and how you verified it; include only sanitized output. Do not commit credentials or private repository content.

See [docs/RELEASING.md](docs/RELEASING.md) for Cargo, GitHub Release, and Homebrew publication.
