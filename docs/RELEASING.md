# Releasing

Release versions must match `Cargo.toml`, `Cargo.lock`, and the `vX.Y.Z` Git tag. Published crates and immutable GitHub releases cannot be overwritten; fix a bad release in a new version rather than moving its tag.

## Prepare and validate

1. Update the Cargo package version and lockfile, `CHANGELOG.md`, and versioned download examples in `README.md`.
2. Run `make check` and `make test`. Both use the pinned Rust toolchain; tests cover the full Cargo workspace, including the private release helper.
3. Run `cargo package -p codex-commit --list` and `cargo publish -p codex-commit --dry-run --locked`; inspect the packaged file list. It must not contain credentials, test transcripts, or local configuration. The private `xtask` package is not published.
4. Merge the pull request only after all required Linux, Apple Silicon, Intel, and lint checks pass. Check the CI run for the resulting `main` commit too.
5. Create an annotated `vX.Y.Z` tag at that checked commit and push that exact tag. Never rewrite a published tag.

## Build, verify, and publish

The tag-triggered `Release` workflow uses `cargo xtask verify-release-tag --tag vX.Y.Z` to check that the tag version matches Cargo metadata and that its commit is on `main`. It builds natively on Apple Silicon and Intel runners, checks each Mach-O architecture, executes help/version, packages the two archives with the Rust helper, and creates a **draft** GitHub release with `SHA256SUMS`.

The same packaging steps can be run locally on each native Mac:

```sh
cargo build -p codex-commit --release --locked --target TARGET
cargo xtask package --target TARGET --version X.Y.Z --output <scratch>/dist
```

After collecting both archives, run `cargo xtask checksums --version X.Y.Z --dir <scratch>/dist` to create the release's `SHA256SUMS`.

The archives contain `codex-commit`, `LICENSE`, and `README.md` at their root:

```text
codex-commit-vX.Y.Z-aarch64-apple-darwin.tar.gz
codex-commit-vX.Y.Z-x86_64-apple-darwin.tar.gz
SHA256SUMS
```

1. Verify that both release jobs succeeded at the tagged commit. Download draft assets with `gh release download vX.Y.Z --repo syz51/codex-commit --dir <scratch>` and check `SHA256SUMS` and both archive contents.
2. Publish the exact tagged source using `cargo publish -p codex-commit --locked`. Authenticate through your own crates.io credential provider or a short-lived token. Do not add a token to the repository, workflow, shell history, or release assets. This workflow does not store a Cargo publishing credential or claim that trusted publishing is configured.
3. Check the public crates.io version and install it into an isolated root with `cargo install codex-commit --version X.Y.Z --locked --root <scratch>`. Verify help, version, dry-run, and terminal cancellation using the installed binary.
4. Review the draft release notes, then publish with `gh release edit vX.Y.Z --repo syz51/codex-commit --draft=false --latest`. Repository release immutability locks the assets and tag when it is published.
5. Verify the release with `gh release verify vX.Y.Z --repo syz51/codex-commit`. Verify downloaded archives with `gh release verify-asset vX.Y.Z <archive> --repo syz51/codex-commit`. Install and execute each archive on its native architecture.

## Homebrew

Generate formula metadata only from the verified release checksums:

```sh
cargo xtask homebrew \
  --version X.Y.Z \
  --checksums <scratch>/SHA256SUMS \
  --output <homebrew-tap>/Formula/codex-commit.rb
```

The generator requires exactly the Apple Silicon and Intel archive names and validates both SHA-256 digests. Open a pull request in [syz51/homebrew-tap](https://github.com/syz51/homebrew-tap). Run the tap's native Mac CI and formula tests before merging. Then verify the public path with `brew install syz51/tap/codex-commit` and `brew test syz51/tap/codex-commit`.

The release workflow needs only the repository's built-in GitHub token, with write permission scoped to its draft-creation job. Cargo publication and the separate Homebrew tap update remain explicit maintainer steps. Trusted publishing may replace manual Cargo authentication after it is configured and tested separately.
