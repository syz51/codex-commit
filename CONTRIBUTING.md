# Contributing

Open an issue for a bug or proposal, or send a focused pull request. For vulnerabilities, use the private reporting instructions in [SECURITY.md](SECURITY.md).

Keep the Rust executable small and preserve signal handling and the commit-only default. Compact unattended output must retain failures and keep the final response on stdout. Supervise Codex in every mode so Rust owns task cleanup. For `--verbose` and `--interactive`, inherit streams and the caller's job group, leave terminal settings to Codex and the shell, and preserve ignored signals and child exit status. Add focused tests for behavior changes. Avoid dependencies unless they solve a concrete maintenance problem.

## Checks

Rust development and release tooling use only the pinned Rust toolchain in `rust-toolchain.toml`: Rust 1.99.1 with rustfmt and Clippy. The published executable retains Rust 1.85 as its tested minimum supported version. Tests and the private release helper may require the current development toolchain.

```sh
make check
make test
cargo publish -p codex-commit --dry-run --locked --allow-dirty
```

The package dry run verifies the current working tree without uploading it; `--allow-dirty` permits checking intended changes before committing. Report each required check accurately, including any failure or incomplete run.

`make test` runs `cargo test --workspace --locked`, including the Rust mock Codex executable, structured event rendering and failure handling, disposable local Git remotes, real terminal cancellation and child cleanup, and release-helper validation. Exercise compact rendering and direct terminal supervision when changing process handling. Tests do not need Codex login or network push credentials. CI also checks installation on Linux, Apple Silicon macOS, and Intel macOS. Put manual-test artifacts in one scratch directory and remove them after testing.

The Workflow lint workflow checks changes under `.github/` with actionlint 1.7.12, ShellCheck 0.11.0 and zizmor 1.32.0. To run it locally, put these versions on `PATH` and run `actionlint` and `zizmor .github/workflows`; without ShellCheck, actionlint silently skips the `run:` scripts. `.github/actionlint.yaml` lists the accepted exceptions.

The CLI uses `clap` and `clap_complete` to keep parsing, help, and shell completions in one command definition. Runtime, test, and private `xtask` dependencies are locked in `Cargo.lock`; use Cargo to update and verify them rather than adding a second language or package manager to the tooling.

Create a branch and open a pull request against `main`. The protected branch requires passing CI before merging. Describe the user-visible change and how you verified it; include only sanitized output. Do not commit credentials or private repository content.

Add a user-facing entry under `## Unreleased` in `CHANGELOG.md` with each shipped change; a daily workflow releases `main` as the next patch version when shipped files changed. See [docs/RELEASING.md](docs/RELEASING.md) for the daily release, manual versions, and Cargo, GitHub Release, and Homebrew publication.
