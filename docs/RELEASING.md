# Releasing

A release is a stable `vX.Y.Z` tag on a checked `main` commit. The tag publishes the source to crates.io and GitHub, and the Homebrew tap updates itself without a cross-repository credential. Published crates and immutable GitHub releases cannot be overwritten; fix a bad release in a new version rather than moving its tag.

## Daily release

The `Daily release` workflow runs at 22:03 JST (13:03 UTC) every day; GitHub may start scheduled runs late. Dispatch it manually to release sooner:

```sh
gh workflow run daily-release.yml --repo syz51/codex-commit --ref main
```

It runs `cargo xtask prepare-release` on `main`, which compares `main` with the latest stable tag reachable from it:

- **Nothing shipped changed:** no release. Shipped files are those `cargo package --list` reports at `main` or at the tag: `src/`, `tests/`, `Cargo.toml`, `Cargo.lock`, `README.md`, and `LICENSE`. Changes to CI, `docs/`, `xtask/`, or `CHANGELOG.md` alone wait for the next release.
- **Shipped files changed:** the next patch version is prepared. `Cargo.toml`, `Cargo.lock`, and the versioned examples in `README.md` move to it, and the `## Unreleased` entries in `CHANGELOG.md` move under its heading. When Unreleased is empty, the entries are the subjects of the commits that changed shipped files, which are the squash-merged PR titles.
- **`main` already carries a newer, untagged version:** nothing is prepared; that release commit is tagged once its CI passes.

A prepared release is pushed to the `automation/daily-release` branch as `Release vX.Y.Z` and proposed as a pull request with squash auto-merge. The release GitHub App authenticates the push, so the four required checks run on it, and the PR merges only after they pass. Each daily run replaces an unmerged release PR with one prepared from current `main`, and closes an open release PR when `main` no longer needs it. To skip a day, close the PR; the next daily run proposes the release again while unreleased shipped changes remain.

Write user-facing `## Unreleased` entries in `CHANGELOG.md` with each change, as before. They become the release notes; the commit-subject fallback is only a safety net.

## Tagging

The `Tag release` workflow runs whenever CI succeeds on a `main` push. When that commit changes the root package version from its first parent, `cargo xtask release-tag` checks that the new version is newer than every reachable stable tag, has no tag yet, and has a `## X.Y.Z` section in `CHANGELOG.md`. The release App then pushes an annotated `vX.Y.Z` tag at exactly that commit, which starts the `Release` workflow. Commits that keep the version are ignored. CI runs on `main` are never replaced by later pushes, so every release commit gets its own result.

If CI fails on a release commit, fix the cause and rerun the failed CI jobs on that commit; success tags it. Until then, daily runs report the version as pending and prepare nothing new.

## Prepare a version manually

Use this for minor or major versions, or to release before the daily run. Tagging and publication stay automatic.

1. Update `Cargo.toml`, `Cargo.lock`, `CHANGELOG.md` (move the Unreleased entries under `## X.Y.Z`), and versioned download examples in `README.md` together.
2. Run `make check`, `make test`, and `cargo publish -p codex-commit --dry-run --locked`. Inspect `cargo package -p codex-commit --list` for credentials, transcripts, or local configuration. The private `xtask` package is not published.
3. Merge the version change. After all four CI checks pass on the resulting `main` commit (lint, Linux, Apple Silicon, and Intel), `Tag release` tags it.

The tag must match the Cargo version and point to a commit reachable from `main`. Only stable versions with three numeric components are accepted. Never move a published tag. To tag by hand instead, for example while the App is unavailable, create an annotated `vX.Y.Z` tag at the checked commit and push it:

```sh
git tag -a vX.Y.Z -m 'vX.Y.Z'
git push origin vX.Y.Z
```

## Automatic publication

The `Release` workflow checks the exact tagged commit's latest required GitHub Actions check results before any publication. It then:

1. Builds on native Apple Silicon and Intel runners, checks Mach-O architecture, and executes each binary's help and version commands.
2. Uses the Rust `xtask` helper to package each executable, `LICENSE`, and `README.md`, create `SHA256SUMS`, and upload a GitHub release draft. Each executable generates its own completion scripts with `--completions SHELL`; no separate completion download is required.
3. Compiles and checks the Cargo package before requesting short-lived crates.io credentials through GitHub OIDC. It publishes the exact tagged source.
4. Downloads the public crate, checks its registry checksum and embedded Git commit, compiles a downstream consumer of the exact public library, installs the executable from crates.io, and runs the Rust CLI integration tests against that installed executable, including terminal cancellation.
5. Publishes the GitHub release only after Cargo verification succeeds, then verifies release immutability and all asset attestations. GitHub creates release attestations asynchronously, so verification retries for up to 30 attempts with five-second delays and fails if verification still cannot succeed. Publication is serialized across versions, and an older version cannot replace a newer version as Latest.

The public archives are:

```text
codex-commit-vX.Y.Z-aarch64-apple-darwin.tar.gz
codex-commit-vX.Y.Z-x86_64-apple-darwin.tar.gz
SHA256SUMS
```

There is no manual tagging, Cargo publication, release-draft approval, or Homebrew formula edit in this flow. Release failures remain visible as failed workflow jobs.

## Homebrew synchronization

The [Homebrew tap](https://github.com/syz51/homebrew-tap) checks for a new published stable release every 15 minutes; GitHub may delay scheduled runs. Its synchronization workflow can also be dispatched manually to start immediately.

The tap verifies the immutable upstream release and its checksums, generates the formula with the released Rust helper, and opens an update PR using a short-lived GitHub App installation token. Normal PR events run native Mac CI. Auto-merge waits for both Apple Silicon and Intel installation, system-Git, style, and audit checks. Branch protection remains enforced; [manually dispatched checks do not qualify as required PR checks](https://docs.github.com/en/pull-requests/how-tos/merge-and-close-pull-requests/troubleshooting-required-status-checks#checks-from-some-workflow-jobs-are-not-evaluated).

After that PR merges, users receive the update through `brew update` and `brew upgrade syz51/tap/codex-commit`. The formula generates and installs zsh, bash, and fish completions from the installed executable and tests the installed files. A source push without a new version tag runs CI but does not publish a new package.

## One-time configuration

The source repository needs:

- A crates.io trusted publisher for crate `codex-commit`, repository owner `syz51`, repository `codex-commit`, workflow filename `release.yml`, and GitHub environment `release`.
- A GitHub environment named `release`, restricted to release tag deployments. It has no manual approval gate.
- Permission to run the SHA-pinned `rust-lang/crates-io-auth-action` in the selected-actions allowlist, alongside the existing GitHub-owned actions.
- Release immutability enabled. Only the release-draft and final-publication jobs receive repository content-write permission; only the Cargo job receives `id-token: write`.

- A GitHub environment named `release-automation`, restricted to the `main` branch, with variable `RELEASE_APP_CLIENT_ID` and secret `RELEASE_APP_PRIVATE_KEY` for a dedicated GitHub App installed only on `syz51/codex-commit` with Contents and Pull requests write permissions. Only the jobs that push the release branch, enable auto-merge, or push a tag mint a short-lived installation token, and none of them run Cargo; the jobs that build and run `xtask` never receive the App key or token. Pushes, pull requests, and merges made with `GITHUB_TOKEN` start no workflows, so they would never get CI results or start `Release`; the App is required.
- Auto-merge enabled, squash merging allowed, and the four strict required checks on `main`.

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
