# codex-commit

[![CI](https://github.com/syz51/codex-commit/actions/workflows/ci.yml/badge.svg)](https://github.com/syz51/codex-commit/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codex-commit.svg)](https://crates.io/crates/codex-commit)

One command to inspect and commit your existing Git changes with **GPT-6 Luna at Max reasoning by default**. Add `--push` to push afterward. A small Rust executable that launches Codex CLI with concise progress, a final summary, and normal signal handling.

```sh
codex-commit
```

Each commit task creates a saved Codex CLI session. Generation-only calls use isolated ephemeral sessions. The Rust executable supports macOS and Linux and uses clap for argument parsing and shell completions.

## Install

All installation methods require Git and [Codex CLI](https://developers.openai.com/codex/cli/) with access to your selected model (`gpt-6-luna` by default). Authenticate with `codex login` and configure your Git identity first. A remote and push credentials are needed only with `--push`. Tested with Codex CLI 0.165.0.

### Homebrew on macOS

```sh
brew tap syz51/tap
brew trust --formula syz51/tap/codex-commit
brew install syz51/tap/codex-commit
```

Homebrew 7 requires explicit trust for third-party formulae. Omit the trust line on older versions that do not provide `brew trust`; this command trusts only `codex-commit`.

The [tap](https://github.com/syz51/homebrew-tap) selects the prebuilt Apple Silicon or Intel executable for your Mac. It uses the Git already available on `PATH`, including macOS Command Line Tools Git, without requiring Homebrew's Git package. The command checks for Git when it runs and reports an error if it is missing. Install Codex CLI separately. Upgrade with `brew upgrade syz51/tap/codex-commit`.

### Cargo

With Rust 1.85 or newer installed:

```sh
cargo install codex-commit --locked
```

Cargo compiles the published source for your machine and normally installs into `~/.cargo/bin`. Run the command again to install a newer published version. No Rust installation is needed for the Homebrew or prebuilt downloads.

### Prebuilt macOS downloads

Download the matching archive and `SHA256SUMS` from [v0.6.3](https://github.com/syz51/codex-commit/releases/tag/v0.6.3):

| Mac | Archive |
| --- | --- |
| Apple Silicon (M-series) | [`codex-commit-v0.6.3-aarch64-apple-darwin.tar.gz`](https://github.com/syz51/codex-commit/releases/download/v0.6.3/codex-commit-v0.6.3-aarch64-apple-darwin.tar.gz) |
| Intel | [`codex-commit-v0.6.3-x86_64-apple-darwin.tar.gz`](https://github.com/syz51/codex-commit/releases/download/v0.6.3/codex-commit-v0.6.3-x86_64-apple-darwin.tar.gz) |

For example, on Apple Silicon:

```sh
release=https://github.com/syz51/codex-commit/releases/download/v0.6.3
archive=codex-commit-v0.6.3-aarch64-apple-darwin.tar.gz
curl -fLO "$release/$archive"
curl -fLO "$release/SHA256SUMS"
grep "  $archive\$" SHA256SUMS | shasum -a 256 -c -
tar -xzf "$archive"
mkdir -p ~/.local/bin
install -m 755 codex-commit ~/.local/bin/codex-commit
```

Use the Intel archive name instead on an Intel Mac. Make sure your installation directory is on `PATH`. Releases include checksums, and published release assets are immutable.

### From source

```sh
git clone https://github.com/syz51/codex-commit.git
cd codex-commit
cargo install --path . --locked
```

Alternatively, `make install` builds a release executable and installs into `~/.local/bin`, with zsh, bash, and fish completions under `~/.local/share`. `PREFIX` and `DESTDIR` are supported for custom locations and packaging. Shell startup files and Codex configuration are not changed.

### Shell completions

Homebrew installs zsh, bash, and fish completions automatically. Zsh must have Homebrew's `share/zsh/site-functions` on its `fpath` before `compinit`; see [Homebrew's shell setup](https://docs.brew.sh/Shell-Completion#configuring-completions-in-zsh).

For Cargo or prebuilt installs, generate zsh completions into a directory on your `fpath`:

```sh
mkdir -p ~/.local/share/zsh/site-functions
codex-commit --completions zsh > ~/.local/share/zsh/site-functions/_codex-commit
```

Add the following to `~/.zshrc` before your existing `compinit` call or shell framework initialization. If neither initializes completions, also add `autoload -Uz compinit && compinit` afterward:

```zsh
fpath=(~/.local/share/zsh/site-functions $fpath)
```

Start a new shell after setup. Regenerate the file after upgrading a Cargo or prebuilt install; `make install` regenerates it automatically. Flags include descriptions, and `-C` completes directories.

`codex-commit --completions SHELL` also supports `bash`, `fish`, `powershell`, and `elvish`. It prints the script to stdout without requiring Git, a repository, or Codex. Install generated files using your shell's completion setup.

## Use

```sh
codex-commit                              # commit in the current repository
codex-commit --push                       # commit, then push
codex-commit -C ~/project                 # another repository
codex-commit 'Commit only the README change'
codex-commit --dry-run                    # inspect the prompt without running Codex
codex-commit --verbose                    # stream Codex's full terminal output
codex-commit --interactive                # interactive Codex for approvals and follow-up
codex-commit -p personal                  # use a Codex configuration profile
codex-commit -m gpt-6.1-sol --reasoning-effort high # override model and effort
codex-commit --sandbox workspace-write    # override the sandbox for this run
codex-commit --interactive -a on-request  # request human approvals interactively
codex-commit --sandbox danger-full-access -a never # opt into full local access
codex-commit --verbose 2>&1 | tee codex-commit.log # capture detailed terminal output
```

Options precede optional instructions. Use `--` before instructions beginning with a dash. The command runs from the repository root, including when invoked in a subdirectory. Bare repositories and detached HEADs are rejected.

The default mode runs unattended and exits when Codex finishes. Both modes leave sandbox settings to your Codex configuration, including trusted project settings. `-p` / `--profile` selects a Codex profile, and `--sandbox` overrides the sandbox for this run. The wrapper does not read or change your Codex configuration or automatically widen access when a commit is blocked. Configured sandbox or network restrictions may block staging, committing protected Git metadata, or pushing. Codex reports the failure; use `--interactive` for required human approval or explicitly override permissions.

Unattended `codex exec` cannot prompt for human approval; configured automatic review may handle eligible approval requests. Use `--interactive` when human approval is needed; its terminal stays open for follow-up messages and its approval policy follows Codex configuration unless overridden. `-a` / `--ask-for-approval` accepts `on-request` (requires `--interactive`) or `never`. To explicitly restore the former unattended full-access behavior, use `--sandbox danger-full-access -a never`; run it only in repositories you trust. Both modes default to GPT-6 Luna and Max reasoning. `-m` / `--model` and `--reasoning-effort` override these selections. Reasoning levels are `none`, `minimal`, `low`, `medium`, `high`, `xhigh`, and `max`; actual model availability and supported effort levels are checked by Codex. The normal service tier is always selected, without enabling Fast mode.

### Global configuration

On macOS and Linux, `codex-commit` optionally reads `$XDG_CONFIG_HOME/codex-commit/config.toml`. When `XDG_CONFIG_HOME` is unset, empty, or relative, it falls back to `$HOME/.config/codex-commit/config.toml`. Only absolute environment paths are used; without a valid base, no default file is loaded. This follows the [XDG Base Directory specification](https://specifications.freedesktop.org/basedir/0.8/). No project configuration is loaded by the wrapper.

Create the file yourself if you want wrapper defaults; all fields are optional:

```toml
model = "gpt-6-luna"
model_reasoning_effort = "max"
profile = "personal"
sandbox = "workspace-write"
# approval_policy = "on-request" # requires --interactive when set
```

Explicit CLI options override this file, which overrides the wrapper defaults independently for each field. Model and reasoning default to `gpt-6-luna` and `max`; unset profile, sandbox, and approval settings are left to Codex. `--config PATH` selects another file (relative to the invoking directory), and `--no-config` bypasses wrapper configuration. `--dry-run` shows the loaded path and merged overrides without resolving Codex's effective settings. Missing default files are ignored; missing explicit files, unreadable files, invalid TOML, unknown fields, and unsupported values fail before Git or Codex runs. Help, version, and shell completion generation do not read configuration. The wrapper never creates or edits configuration files.

The task asks Codex to inspect staged, unstaged, and untracked changes; follow repository instructions; run relevant checks; and commit existing intended changes without rewriting history. Memory settings are inherited from Codex. The prompt respects applicable remembered preferences and ongoing restrictions, distinguishes them from past task instructions, and gives current explicit instructions precedence over conflicting older preferences. Historical restrictions must be checked for applicability: a rule against editing a file does not itself exclude the user's existing changes from a commit. Explicit restrictions on committing or publishing and confidentiality rules still apply. Unresolved consequential scope conflicts stop the task; excluded changes must be explained in the final summary. These are prompt policies, so review the reported scope and resulting commit.

Before launching Codex for a dirty working tree, the wrapper asks Git to resolve both author and committer identities with `user.useConfigOnly=true`. If either identity cannot be resolved without inference, it prints an advisory warning. Git configuration and explicit author/committer environment overrides are respected; no identity or configuration is changed. The warning does not block the task, and clean working trees and `--dry-run` skip this check.

By default the final push policy forbids pushing, publishing, fetching, or pulling, even when repository or additional instructions mention pushing. Rerun with `--push` to enable pushing.

With `--push`, Codex can push existing local commits from a clean worktree. With no upstream, it may set one only when `origin` is the sole remote. Both push modes are prompt policies, not OS-enforced network or Git restrictions; review the streamed output.

Default unattended output immediately reports when a turn starts, shows concise progress on stderr, and keeps the final response on stdout. Codex JSON messages do not identify commentary separately, so the latest message is held until another message or tool activity establishes it as progress, or turn completion establishes it as the final response. Ctrl-C and other cancellation signals are forwarded immediately. If Codex has not exited after a 250 ms grace period, the supervisor force-stops its process group; a second cancellation forces shutdown immediately. Cancellation preserves the original signal exit status even when output is blocked.

The prompt prefers inspection commands that succeed when differences are expected; documented comparison status 1 must be handled at the command boundary without masking real comparison, pipeline, validation, or test errors. Temporary files, when needed, belong in one unique task-owned directory under `TMPDIR` (fallback `/tmp`), cleaned up at the end, including failure paths. These are prompt policies; the renderer continues to show nonzero command results as failures.

It renders Codex's JSON events, hiding the startup banner, echoed prompt, reasoning, routine hook notifications, and successful raw tool output. Failed command output and tool errors remain visible. For routine runs, the prompt requests one combined progress update after inspection, with further updates for material discoveries, failures, or long-running work. The final summary includes the observed commit and push results, checks, and remaining changes with exclusion reasons. The renderer preserves messages rather than enforcing an update count.

Use `--verbose` for Codex's full terminal output. Both `--verbose` and `--interactive` replace the Rust process with Codex on Unix, preserving its terminal streams and process identity. Compact mode supervises Codex instead, forwards interrupt and termination signals, and preserves its exit code; invalid or incomplete event streams fail explicitly. Piped or file input is passed to Codex as context; compact mode does not read from an interactive terminal. **Ctrl-C interrupts Codex** in all modes. Cancellation cannot undo an operation that already completed; inspect local and remote state before retrying.

Use the printed session ID to continue a run interactively:

```sh
codex resume --include-non-interactive SESSION_ID
```

For the underlying contracts, see the official [Luna model documentation](https://developers.openai.com/api/docs/models/gpt-6-luna) and [Codex non-interactive mode documentation](https://developers.openai.com/codex/noninteractive/).

## Development

```sh
make check  # rustfmt and Clippy
make test   # Rust workspace unit, integration, terminal, and packaging tests
```

Development and release tooling use the pinned Rust 1.99.1 toolchain. Runtime code uses `clap` and `clap_complete`; all dependencies, including tests and the private `xtask` release helper, are recorded in `Cargo.lock`. Run `cargo test --workspace --locked` directly if you do not use Make.

CI tests native Linux, Apple Silicon macOS, and Intel macOS, including installation. Automated Rust tests use a mock Codex executable, real disposable Git repositories, and real terminal signals; they do not call a model or require credentials. Release builds execute and check each native Mac binary before packaging with `cargo xtask`.

See [CONTRIBUTING.md](CONTRIBUTING.md), [release procedures](docs/RELEASING.md), [CHANGELOG.md](CHANGELOG.md), and [SECURITY.md](SECURITY.md).

[MIT licensed](LICENSE). Not affiliated with OpenAI.

## Generate a message from a supplied public diff

Generation does not inspect a repository or commit or push anything. Supply only
public diff text on stdin, with explicit Codex executable, authentication home,
and model:

```sh
codex-commit generate --codex-path /absolute/path/to/codex \
  --codex-home /absolute/path/to/auth-home --model gpt-6.1-sol < public.diff
```

Success writes one JSON object with `subject` and `body` strings. Errors return a
nonzero status without a fallback message. Input is limited to 1 MiB and results
to 16 KiB. The default reasoning level is high and the timeout is 120 seconds;
`--reasoning-effort` and `--timeout-secs` override them. Service tier is ordinary.

The authentication home supplies credentials only. Generation copies the current
access token into an isolated home and removes OAuth refresh credentials; it
never writes back to the source authentication home. An expired token fails with
an authentication error: refresh the source credentials with Codex CLI and retry.
Generation ignores user
configuration and rules and runs in an empty, ephemeral context. Before inference,
a loopback compatibility check verifies that the exact CLI configuration exposes
no tools. Unsupported CLI controls, tool exposure, incomplete responses, generic
messages, and unsupported validation claims fail closed. Diff text is untrusted
data; changes inside it are never instructions to execute. Callers remain
responsible for reviewing message accuracy and their public-data boundary.

### Rust library

```toml
[dependencies]
codex-commit = "=0.6.3"
```

```rust,no_run
use codex_commit::{generate_message, GenerationOptions, GenerationRequest};

let options = GenerationOptions::new(
    "/absolute/path/to/codex".into(),
    "/absolute/path/to/auth-home".into(),
    "gpt-6.1-sol".into(),
);
let message = generate_message(GenerationRequest { diff: "public diff text" }, &options)?;
println!("{}", message.subject);
# Ok::<(), codex_commit::Error>(())
```

The library exposes typed messages and errors, explicit timeout and cancellation,
and bounded input/output. CLI parsing, terminal rendering, and the Codex event
protocol are private implementation details. The existing commit CLI retains its
configuration, repository inspection, and optional push behavior.
