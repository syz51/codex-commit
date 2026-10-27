# Changelog

## 0.2.2

- Find unpublished GitHub releases through authenticated release listings and bind asset validation and publication to the release ID, avoiding draft-by-tag API lookup failures.
- Preserve retry safety, immutable assets, and exact release asset checks throughout automated publication.

## 0.2.1

- Publish native Mac downloads and the Cargo package automatically from a version tag.
- Authenticate Cargo publishing through short-lived GitHub OIDC credentials.
- Update Homebrew automatically through formula pull requests that merge after both Mac architecture checks pass.
- Reuse Git already available on PATH without requiring Homebrew's Git package.

## 0.2.0

- Replace the shell launcher with a dependency-free Rust executable while preserving the CLI, Luna/Max settings, streamed output, and Unix process replacement.
- Publish a Cargo package and native macOS downloads for Apple Silicon and Intel.
- Add Homebrew distribution through `syz51/tap/codex-commit`.
- Test Linux and both Mac architectures, Cargo packaging, and staged installation in CI.
- Generate release checksums and Homebrew formula metadata from the native archives.

## 0.1.0

- Initial shell utility with commit-only defaults, optional `--push`, dry-run, and interactive mode.
