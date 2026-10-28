# Changelog

## 0.5.0

- Show compact progress and a final summary by default in unattended mode, hiding raw successful tool output, repeated hook notifications, the startup banner, and echoed prompt.
- Preserve failed command output and tool errors; reject invalid or incomplete structured event streams.
- Add `--verbose` for direct Codex terminal output, while retaining direct execution in interactive mode.
- Forward cancellation signals and preserve Codex's exit code in compact mode; cover structured events and process cleanup with focused tests.

## 0.4.0

- Inherit Codex sandbox settings by default instead of granting full local access; interactive mode also inherits the configured approval policy. Unattended runs cannot prompt for human approval.
- Add explicit sandbox, approval-policy, and Codex profile overrides.
- Read optional global XDG TOML configuration, with `--config` and `--no-config` controls and CLI precedence.
- Support model and reasoning selection through configuration and `--model` / `--reasoning-effort`, retaining GPT-6 Luna and Max reasoning as defaults.
- Show configuration paths in help and selected settings in dry-run output; include the new options in generated shell completions.

## 0.3.0

- Use clap for argument parsing and generate shell completions from the same command definition.
- Add `--completions` for zsh, bash, fish, PowerShell, and Elvish without requiring Git or Codex.
- Install zsh, bash, and fish completions automatically through Homebrew and `make install`; document setup for Cargo and prebuilt downloads.

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
