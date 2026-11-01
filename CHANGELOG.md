# Changelog

## Unreleased

## 0.6.4

- Default routine commits to Low reasoning, disable native delegation, and supply a bounded metadata-only Git overview with exact repository instruction candidates and compact numstat data.
- Reuse instruction discovery, collapse oversized untracked listings, report advisory probe errors, and keep required-check failures visible while treating whitespace as advisory unless required.
- Reuse normal ignored build caches, scan summarized additions and untracked content for credential indicators and conflict markers, stop on unmerged index entries before staging, and gate commits on selected staged paths and unresolved conflicts.
- Discover existing instruction files before reading them rather than guessing nested paths.
- Manage temporary files and uv caches in a private Rust-owned directory in every commit mode, clean it after success, failure, and handled cancellation, and report cleanup errors without masking Codex failures.
- Supervise verbose and interactive Codex processes while inheriting terminal streams, the caller's job group, and ignored signals; the wrapper and Codex now have separate PIDs.

## 0.6.3

- Treat documented comparison differences as successful inspection while preserving genuine command, pipeline, validation, and test failures.
- Keep temporary files in one unique task-owned directory and clean it up on success and failure instead of using fixed shared paths.

## 0.6.2

- Respect applicable remembered preferences and ongoing restrictions while distinguishing past task instructions and giving current explicit instructions precedence over conflicting older preferences. Clarify that historical editing restrictions do not automatically exclude existing user changes from commits; require applicable exclusion reasons and verification of the resulting commit and working tree.
- Request one combined progress update for routine runs while preserving material updates and failures.
- Warn before launching Codex for a dirty working tree when Git cannot resolve explicit author and committer identities, without changing configuration or blocking the task.

## 0.6.1

- Bound compact-mode cancellation to a 250 ms graceful shutdown window, then force-stop the owned process group; a second cancellation forces immediate shutdown while preserving the original signal exit status.
- Report turn-start activity immediately while preserving the final response on stdout.
- Label recoverable Codex diagnostics neutrally instead of presenting experimental-feature warnings as errors.

## 0.6.0

- Add a public Rust library returning typed commit subjects and bodies from supplied public diffs.
- Add generation-only JSON CLI output with explicit backend settings, bounded input/output, timeout and cancellation.
- Isolate authenticated Codex generation from ambient instructions, configuration, tools and repository state; verify tool isolation before inference and fail closed on invalid or uninformative output.
- Preserve existing commit and optional push behavior.

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
