# Changelog

## 0.2.0

- Replace the shell launcher with a dependency-free Rust executable while preserving the CLI, Luna/Max settings, streamed output, and Unix process replacement.
- Publish a Cargo package and native macOS downloads for Apple Silicon and Intel.
- Add Homebrew distribution through `syz51/tap/codex-commit`.
- Test Linux and both Mac architectures, Cargo packaging, and staged installation in CI.
- Generate release checksums and Homebrew formula metadata from the native archives.

## 0.1.0

- Initial shell utility with commit-only defaults, optional `--push`, dry-run, and interactive mode.
