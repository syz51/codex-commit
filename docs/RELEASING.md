# Releasing

Push a stable version tag to release the checked source to crates.io and GitHub. The Homebrew tap then updates itself without a cross-repository credential. Published crates and immutable GitHub releases cannot be overwritten; fix a bad release in a new version rather than moving its tag.

## Prepare a version

1. Update `Cargo.toml`, `Cargo.lock`, `CHANGELOG.md`, and versioned download examples in `README.md` together.
2. Run `make check`, `make test`, and `cargo publish -p codex-commit --dry-run --locked`. Inspect `cargo package -p codex-commit --list` for credentials, transcripts, or local configuration. The private `xtask` package is not published.
3. Merge the version change and wait for all four CI checks on the resulting `main` commit: lint, Linux, Apple Silicon, and Intel.
4. Create an annotated `vX.Y.Z` tag at that checked commit and push it:

   ```sh
   git tag -a vX.Y.Z -m 'vX.Y.Z'
   git push origin vX.Y.Z
   ```

The tag must match the Cargo version and point to a commit reachable from `main`. Only stable versions with three numeric components are accepted. Never move a published tag.

## Automatic publication

The `Release` workflow checks the exact tagged commit's latest required GitHub Actions check results before any publication. It then:

1. Builds on native Apple Silicon and Intel runners, checks Mach-O architecture, and executes each binary's help and version commands.
2. Uses the Rust `xtask` helper to package each executable, `LICENSE`, and `README.md`, create `SHA256SUMS`, and upload a GitHub release draft.
3. Compiles and checks the Cargo package before requesting short-lived crates.io credentials through GitHub OIDC. It publishes the exact tagged source.
4. Downloads the public crate, checks its registry checksum and embedded Git commit, installs it from crates.io, and runs the Rust CLI integration tests against that installed executable, including terminal cancellation.
5. Publishes the GitHub release only after Cargo verification succeeds, then verifies release immutability and all asset attestations. GitHub creates release attestations asynchronously, so verification retries for up to 30 attempts with five-second delays and fails if verification still cannot succeed. Publication is serialized across versions, and an older version cannot replace a newer version as Latest.

The public archives are:

```text
codex-commit-vX.Y.Z-aarch64-apple-darwin.tar.gz
codex-commit-vX.Y.Z-x86_64-apple-darwin.tar.gz
SHA256SUMS
```

There is no manual Cargo publication, release-draft approval, or Homebrew formula edit in this flow. Release failures remain visible as failed workflow jobs.

## Homebrew synchronization

The [Homebrew tap](https://github.com/syz51/homebrew-tap) checks for a new published stable release every 15 minutes; GitHub may delay scheduled runs. Its synchronization workflow can also be dispatched manually to start immediately.

The tap verifies the immutable upstream release and its checksums, generates the formula with the released Rust helper, and opens an update PR using a short-lived GitHub App installation token. Normal PR events run native Mac CI. Auto-merge waits for both Apple Silicon and Intel installation, system-Git, style, and audit checks. Branch protection remains enforced; [manually dispatched checks do not qualify as required PR checks](https://docs.github.com/en/pull-requests/how-tos/merge-and-close-pull-requests/troubleshooting-required-status-checks#checks-from-some-workflow-jobs-are-not-evaluated).

After that PR merges, users receive the update through `brew update` and `brew upgrade syz51/tap/codex-commit`. A source push without a new version tag runs CI but does not publish a new package.

## One-time configuration

The source repository needs:

- A crates.io trusted publisher for crate `codex-commit`, repository owner `syz51`, repository `codex-commit`, workflow filename `release.yml`, and GitHub environment `release`.
- A GitHub environment named `release`, restricted to release tag deployments. It has no manual approval gate.
- Permission to run the SHA-pinned `rust-lang/crates-io-auth-action` in the selected-actions allowlist, alongside the existing GitHub-owned actions.
- Release immutability enabled. Only the release-draft and final-publication jobs receive repository content-write permission; only the Cargo job receives `id-token: write`.

The tap needs Actions enabled, auto-merge enabled, and its existing strict required Mac checks. Install a dedicated GitHub App only on `syz51/homebrew-tap`, granting Contents and Pull requests write permissions. In the tap's `release-updates` environment, set variable `RELEASE_APP_CLIENT_ID` and secret `RELEASE_APP_PRIVATE_KEY`, and restrict deployment to the `main` branch. Only the update job can mint a tap-scoped installation token; the read-only Rust build never receives the App key or token. The installation token is revoked after the job. Neither repository stores a long-lived crates.io token or a credential granting access to the other repository.

## Retry a failed release

Rerun failed jobs in Actions, or dispatch the workflow against the same existing tag:

```sh
gh workflow run release.yml --repo syz51/codex-commit --ref vX.Y.Z
```

Dispatching against `main` or a branch fails validation. Retries verify an already published crate's embedded source commit rather than uploading it again. An already published immutable GitHub release is verified and reused without modifying its assets. An unpublished draft may be repaired; unexpected draft assets stop publication so they cannot become immutable accidentally. A crate/source mismatch, missing CI check, invalid checksum, or immutable-release verification failure stops the workflow.

For local packaging diagnostics, use the same Rust helper as CI:

```sh
cargo build -p codex-commit --release --locked --target TARGET
cargo xtask package --target TARGET --version X.Y.Z --output <scratch>/dist
cargo xtask checksums --version X.Y.Z --dir <scratch>/dist
```
