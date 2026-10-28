# Contributing

Open an issue for a bug or proposal, or send a focused pull request. For vulnerabilities, use the private reporting instructions in [SECURITY.md](SECURITY.md).

Keep the Rust executable small and preserve direct terminal output, process replacement, signal handling, and the commit-only default. Add focused tests for behavior changes. Avoid dependencies unless they solve a concrete maintenance problem.

## Checks

Development and release tooling use only the pinned Rust toolchain in `rust-toolchain.toml`: Rust 1.99.1 with rustfmt and Clippy. The published executable retains Rust 1.85 as its tested minimum supported version. Tests and the private release helper may require the current development toolchain.

```sh
make check
make test
cargo publish -p codex-commit --dry-run --locked
```

`make test` runs `cargo test --workspace --locked`, including the Rust mock Codex executable, disposable local Git remotes, real terminal signal handling, and release-helper validation. Tests do not need Codex login or network push credentials. CI also checks installation on Linux, Apple Silicon macOS, and Intel macOS. Put manual-test artifacts in one scratch directory and remove them after testing.

The CLI uses `clap` and `clap_complete` to keep parsing, help, and shell completions in one command definition. Runtime, test, and private `xtask` dependencies are locked in `Cargo.lock`; use Cargo to update and verify them rather than adding a second language or package manager to the tooling.

Create a branch and open a pull request against `main`. The protected branch requires passing CI before merging. Describe the user-visible change and how you verified it; include only sanitized output. Do not commit credentials or private repository content.

See [docs/RELEASING.md](docs/RELEASING.md) for Cargo, GitHub Release, and Homebrew publication.
