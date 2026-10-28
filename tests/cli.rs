//! Native CLI integration tests. Codex is mocked; Git and terminal signals are real.

#![cfg(unix)]

use nix::errno::Errno;
use nix::fcntl::{FcntlArg, OFlag, fcntl};
use nix::libc;
use nix::pty::openpty;
use nix::sys::signal::{Signal, kill, killpg};
use nix::sys::termios::{LocalFlags, SetArg, SpecialCharacterIndices, tcgetattr, tcsetattr};
use nix::unistd::Pid;
use serde_json::Value;
use std::collections::BTreeMap;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tempfile::{Builder, TempDir};

nix::ioctl_write_int_bad!(set_controlling_terminal, libc::TIOCSCTTY);

struct CapturedChild {
    child: Child,
    reaped: bool,
}

impl Drop for CapturedChild {
    fn drop(&mut self) {
        if self.reaped {
            return;
        }
        // bounded_output starts an isolated group containing only this test's
        // command and descendants. Clean it even when a deadline/assertion fails.
        let _ = killpg(Pid::from_raw(self.child.id() as i32), Signal::SIGKILL);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

trait BoundedOutput {
    fn bounded_output(&mut self) -> io::Result<Output>;
}

impl BoundedOutput for Command {
    fn bounded_output(&mut self) -> io::Result<Output> {
        // Anonymous files avoid pipe backpressure while retaining separate byte
        // streams. Live streaming itself is exercised through a real PTY below.
        let directory = self.get_current_dir().expect("fixture working directory");
        let mut stdout = tempfile::tempfile_in(directory)?;
        let mut stderr = tempfile::tempfile_in(directory)?;
        self.stdin(Stdio::null())
            .stdout(Stdio::from(stdout.try_clone()?))
            .stderr(Stdio::from(stderr.try_clone()?))
            .process_group(0);
        let mut child = CapturedChild {
            child: self.spawn()?,
            reaped: false,
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = child.child.try_wait()? {
                child.reaped = true;
                break status;
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "fixture command exceeded ten seconds",
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        stdout.seek(SeekFrom::Start(0))?;
        stderr.seek(SeekFrom::Start(0))?;
        let mut output = Output {
            status,
            stdout: Vec::new(),
            stderr: Vec::new(),
        };
        stdout.read_to_end(&mut output.stdout)?;
        stderr.read_to_end(&mut output.stderr)?;
        Ok(output)
    }
}

fn cli() -> PathBuf {
    env::var_os("CODEX_COMMIT_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_codex-commit")))
        .canonicalize()
        .expect("compiled or explicitly selected CLI")
}

fn mock_codex() -> &'static Path {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        let path = Path::new(env!("CARGO_BIN_EXE_codex-commit"))
            .parent()
            .expect("binary directory")
            .join("examples/mock-codex");
        assert!(
            path.is_file(),
            "Cargo must build the Rust mock example: {}",
            path.display()
        );
        path
    })
}

fn real_git() -> &'static Path {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        env::split_paths(&env::var_os("PATH").unwrap_or_default())
            .map(|directory| directory.join("git"))
            .find(|path| {
                fs::metadata(path).is_ok_and(|metadata| {
                    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
                })
            })
            .expect("Git is required for integration tests")
            .canonicalize()
            .expect("Git path")
    })
}

fn text(output: &[u8]) -> String {
    String::from_utf8_lossy(output).into_owned()
}

fn assert_code(output: &Output, code: i32) {
    assert_eq!(
        output.status.code(),
        Some(code),
        "stdout: {}\nstderr: {}",
        text(&output.stdout),
        text(&output.stderr)
    );
}

fn decode_os(value: &Value) -> OsString {
    OsString::from_vec(
        value
            .as_array()
            .expect("raw byte array")
            .iter()
            .map(|byte| u8::try_from(byte.as_u64().expect("byte")).expect("u8"))
            .collect(),
    )
}

struct Invocation {
    args: Vec<OsString>,
    cwd: PathBuf,
    pid: u32,
}

impl Invocation {
    fn option(&self, name: &str) -> &OsStr {
        let position = self
            .args
            .iter()
            .position(|arg| arg == name)
            .expect("option");
        &self.args[position + 1]
    }

    fn has(&self, arg: &str) -> bool {
        self.args.iter().any(|item| item == arg)
    }

    fn prompt(&self) -> &str {
        self.args
            .last()
            .and_then(|arg| arg.to_str())
            .expect("UTF-8 prompt")
    }
}

struct Fixture {
    _temporary: Option<TempDir>,
    root: PathBuf,
    repo: PathBuf,
    bin: PathBuf,
    log: PathBuf,
    child_log: PathBuf,
    environment: BTreeMap<OsString, OsString>,
}

impl Fixture {
    fn new() -> Self {
        let supplied = env::var_os("CODEX_COMMIT_TEST_TMPDIR").map(PathBuf::from);
        let temporary = if let Some(root) = &supplied {
            fs::create_dir_all(root).expect("scratch root");
            Builder::new().prefix("rust-cli-").tempdir_in(root)
        } else {
            Builder::new().prefix("codex-commit-tests-").tempdir()
        }
        .expect("fixture temporary directory");
        let root = temporary
            .path()
            .canonicalize()
            .expect("canonical scratch root");
        let temporary = if supplied.is_some() {
            let _ = temporary.keep();
            None
        } else {
            Some(temporary)
        };
        let repo = root.join("repository with spaces");
        let bin = root.join("bin");
        fs::create_dir(&repo).expect("repository directory");
        fs::create_dir(&bin).expect("mock executable directory");
        fs::copy(mock_codex(), bin.join("codex")).expect("copy native Codex mock");
        fs::set_permissions(bin.join("codex"), fs::Permissions::from_mode(0o755))
            .expect("executable mock");
        let path = env::join_paths(
            std::iter::once(bin.clone())
                .chain(env::split_paths(&env::var_os("PATH").unwrap_or_default())),
        )
        .expect("fixture PATH");
        let log = root.join("codex.json");
        let child_log = root.join("child.json");
        let environment = [
            (OsString::from("PATH"), path),
            (OsString::from("HOME"), root.as_os_str().to_owned()),
            (OsString::from("TMPDIR"), root.as_os_str().to_owned()),
            (OsString::from("FAKE_CODEX_LOG"), log.as_os_str().to_owned()),
            (
                OsString::from("FAKE_CODEX_CHILD_LOG"),
                child_log.as_os_str().to_owned(),
            ),
            (OsString::from("GIT_CONFIG_NOSYSTEM"), OsString::from("1")),
            (
                OsString::from("GIT_CONFIG_GLOBAL"),
                OsString::from("/dev/null"),
            ),
            (
                OsString::from("GIT_AUTHOR_NAME"),
                OsString::from("Test Author"),
            ),
            (
                OsString::from("GIT_AUTHOR_EMAIL"),
                OsString::from("test@example.invalid"),
            ),
            (
                OsString::from("GIT_COMMITTER_NAME"),
                OsString::from("Test Author"),
            ),
            (
                OsString::from("GIT_COMMITTER_EMAIL"),
                OsString::from("test@example.invalid"),
            ),
            (OsString::from("GIT_TERMINAL_PROMPT"), OsString::from("0")),
        ]
        .into_iter()
        .collect();
        let fixture = Self {
            _temporary: temporary,
            root,
            repo,
            bin,
            log,
            child_log,
            environment,
        };
        fixture.git(&["init", "-b", "main"]);
        fs::write(fixture.repo.join("tracked.txt"), "original\n").expect("fixture content");
        fixture.git(&["add", "tracked.txt"]);
        fixture.git(&["commit", "-m", "Initial fixture"]);
        fixture
    }

    fn command(&self, program: impl AsRef<OsStr>) -> Command {
        let mut command = Command::new(program);
        // Explicit per-process environments keep parallel tests independent and
        // prevent credentials or user Git configuration from entering fixtures.
        command
            .env_clear()
            .envs(&self.environment)
            .current_dir(&self.repo);
        command
    }

    fn git(&self, args: &[&str]) -> String {
        self.git_at(&self.repo, args)
    }

    fn git_at(&self, directory: &Path, args: &[&str]) -> String {
        let output = self
            .command(real_git())
            .current_dir(directory)
            .args(args)
            .bounded_output()
            .expect("run Git");
        assert_code(&output, 0);
        text(&output.stdout).trim().to_owned()
    }

    fn invoke(&self, args: &[&str]) -> Output {
        self.command(cli())
            .args(args)
            .bounded_output()
            .expect("run CLI")
    }

    fn invocation(&self) -> Invocation {
        let value: Value =
            serde_json::from_slice(&fs::read(&self.log).expect("mock log")).expect("JSON log");
        Invocation {
            args: value["argv"]
                .as_array()
                .expect("argv")
                .iter()
                .map(decode_os)
                .collect(),
            cwd: PathBuf::from(decode_os(&value["cwd"])),
            pid: u32::try_from(value["pid"].as_u64().expect("PID")).expect("u32 PID"),
        }
    }

    fn empty_bin(&self, name: &str) -> PathBuf {
        let directory = self.root.join(name);
        fs::create_dir(&directory).expect("new bin directory");
        directory
    }

    fn git_only(&self) -> PathBuf {
        let directory = self.empty_bin("git-only");
        symlink(real_git(), directory.join("git")).expect("Git symlink");
        directory
    }
}

#[test]
fn default_launch_from_nested_directory_uses_luna_max_and_saved_thread() {
    let fixture = Fixture::new();
    let nested = fixture.repo.join("nested");
    fs::create_dir(&nested).unwrap();
    let output = fixture
        .command(cli())
        .current_dir(nested)
        .bounded_output()
        .unwrap();
    assert_code(&output, 0);
    let call = fixture.invocation();
    assert_eq!(call.cwd, fixture.repo);
    assert!(call.has("exec"));
    assert_eq!(call.option("--model"), "gpt-6-luna");
    assert!(call.has("model_reasoning_effort=\"max\""));
    assert!(call.has("service_tier=\"default\""));
    assert_eq!(call.option("-C"), fixture.repo);
    assert!(!call.has("--sandbox"));
    assert!(!call.has("-a"));
    assert!(
        !call
            .args
            .iter()
            .any(|arg| arg.to_string_lossy().starts_with("approval_policy="))
    );
    assert!(!call.has("--ephemeral"));
    assert!(!call.has("--json"));
    assert!(call.prompt().contains("Push policy: disabled."));
    assert!(text(&output.stdout).contains("finished"));
}

#[test]
fn directory_option_and_interactive_permissions() {
    let fixture = Fixture::new();
    let output = fixture
        .command(cli())
        .current_dir(&fixture.root)
        .arg("-C")
        .arg(&fixture.repo)
        .args(["--interactive", "--push"])
        .bounded_output()
        .unwrap();
    assert_code(&output, 0);
    let call = fixture.invocation();
    assert!(!call.has("exec"));
    assert!(call.has("--no-alt-screen"));
    assert!(!call.has("-a"));
    assert!(!call.has("--sandbox"));
    assert!(
        !call
            .args
            .iter()
            .any(|arg| arg.to_string_lossy().starts_with("approval_policy="))
    );
    assert_eq!(call.option("--model"), "gpt-6-luna");
    assert!(call.has("model_reasoning_effort=\"max\""));
    assert!(call.prompt().contains("Push policy: enabled."));
}

fn write_config(path: &Path, source: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, source).unwrap();
}

#[test]
fn global_config_uses_absolute_xdg_or_home_fallback() {
    let fixture = Fixture::new();
    let home_config = fixture.root.join(".config/codex-commit/config.toml");
    write_config(&home_config, "sandbox = 'read-only'\nprofile = 'home'\n");
    for xdg in [None, Some(""), Some("relative-config")] {
        let mut command = fixture.command(cli());
        if let Some(value) = xdg {
            command.env("XDG_CONFIG_HOME", value);
        }
        let output = command.arg("--dry-run").bounded_output().unwrap();
        assert_code(&output, 0);
        let stdout = text(&output.stdout);
        assert!(stdout.contains(&format!("Config: {}", home_config.display())));
        assert!(stdout.contains("Sandbox override: read-only"));
        assert!(stdout.contains("Profile: home"));
    }
    let xdg = fixture.root.join("xdg");
    let xdg_config = xdg.join("codex-commit/config.toml");
    write_config(&xdg_config, "profile = 'xdg'");
    let output = fixture
        .command(cli())
        .env("XDG_CONFIG_HOME", xdg)
        .arg("--dry-run")
        .bounded_output()
        .unwrap();
    assert_code(&output, 0);
    assert!(text(&output.stdout).contains("Profile: xdg"));
    assert!(text(&output.stdout).contains(&format!("Config: {}", xdg_config.display())));
    assert!(text(&output.stdout).contains("Sandbox override: none"));
    for home in [None, Some("relative-home")] {
        let mut command = fixture.command(cli());
        command
            .env_remove("HOME")
            .env("XDG_CONFIG_HOME", "relative-config");
        if let Some(value) = home {
            command.env("HOME", value);
        }
        let output = command.arg("--dry-run").bounded_output().unwrap();
        assert_code(&output, 0);
        assert!(text(&output.stdout).contains("Config: none"));
    }
    assert!(!fixture.log.exists());
}

#[test]
fn config_precedence_and_explicit_relative_path_are_forwarded() {
    let fixture = Fixture::new();
    let default = fixture.root.join(".config/codex-commit/config.toml");
    write_config(
        &default,
        "sandbox = 'read-only'\nprofile = 'global'\napproval_policy = 'on-request'",
    );
    // Configured human approvals must be validated after merging CLI overrides.
    let output = fixture.invoke(&["-a", "never"]);
    assert_code(&output, 0);
    let call = fixture.invocation();
    assert_eq!(call.option("--profile"), "global");
    assert_eq!(call.option("--sandbox"), "read-only");
    assert!(call.has("approval_policy=\"never\""));
    let output = fixture.invoke(&["--interactive", "--sandbox", "workspace-write", "-p", "cli"]);
    assert_code(&output, 0);
    let call = fixture.invocation();
    assert_eq!(call.option("--sandbox"), "workspace-write");
    assert_eq!(call.option("--profile"), "cli");
    assert!(call.has("approval_policy=\"on-request\""));
    write_config(&fixture.repo.join("custom.toml"), "profile = 'explicit'");
    assert_code(&fixture.invoke(&["--config", "custom.toml"]), 0);
    let call = fixture.invocation();
    assert_eq!(call.option("--profile"), "explicit");
    assert!(!call.has("--sandbox"));
    assert!(!call.has("approval_policy=\"on-request\""));
    assert_code(&fixture.invoke(&["--no-config"]), 0);
    assert!(!fixture.invocation().has("--profile"));
}

#[test]
fn config_failures_are_reported_before_git_or_codex() {
    let fixture = Fixture::new();
    let config = fixture.root.join(".config/codex-commit/config.toml");
    let empty = fixture.empty_bin("empty-config-bin");
    for source in [
        "[broken",
        "unknown = true",
        "sandbox = 'invalid'",
        "approval_policy = 'untrusted'",
        "profile = 42",
        "sandbox = 1",
        "approval_policy = 'on-request'",
    ] {
        write_config(&config, source);
        let output = fixture
            .command(cli())
            .env("PATH", &empty)
            .bounded_output()
            .unwrap();
        assert_code(&output, 2);
        let stderr = text(&output.stderr);
        if source.contains("on-request") {
            assert!(stderr.contains("requires --interactive"));
        } else {
            assert!(stderr.contains(&config.display().to_string()), "{stderr}");
        }
        assert!(!fixture.log.exists());
    }
    fs::remove_file(&config).unwrap();
    fs::create_dir(&config).unwrap();
    let output = fixture
        .command(cli())
        .env("PATH", &empty)
        .bounded_output()
        .unwrap();
    assert_code(&output, 2);
    assert!(text(&output.stderr).contains("could not read config"));
    assert!(!fixture.log.exists());
}

#[test]
fn config_missing_explicit_and_opt_out_behavior() {
    let fixture = Fixture::new();
    let output = fixture.invoke(&["--dry-run"]);
    assert_code(&output, 0);
    assert!(text(&output.stdout).contains("Config: none"));
    let missing = fixture.root.join("missing.toml");
    let output = fixture
        .command(cli())
        .arg("--config")
        .arg(&missing)
        .bounded_output()
        .unwrap();
    assert_code(&output, 2);
    assert!(text(&output.stderr).contains(&missing.display().to_string()));
    write_config(
        &fixture.root.join(".config/codex-commit/config.toml"),
        "invalid ???",
    );
    assert_code(&fixture.invoke(&["--no-config", "--dry-run"]), 0);
    for args in [&["--help"][..], &["--version"], &["--completions", "zsh"]] {
        assert_code(&fixture.invoke(args), 0);
    }
    for args in [
        &["--config"][..],
        &["--config", "missing.toml", "--no-config"],
        &["--completions", "zsh", "--no-config"],
        &["--completions", "zsh", "--config", "missing.toml"],
    ] {
        assert_code(&fixture.invoke(args), 2);
    }
    assert!(!fixture.log.exists());
}

#[test]
fn model_and_reasoning_config_merge_independently_with_cli() {
    let fixture = Fixture::new();
    let config = fixture.root.join(".config/codex-commit/config.toml");
    write_config(
        &config,
        "model = 'custom-model'\nmodel_reasoning_effort = 'medium'",
    );
    for (flags, model, effort) in [
        (vec![], "custom-model", "medium"),
        (
            vec!["--interactive", "-m", "cli-model"],
            "cli-model",
            "medium",
        ),
        (vec!["--reasoning-effort", "low"], "custom-model", "low"),
        (
            vec![
                "--interactive",
                "--model",
                "both",
                "--reasoning-effort",
                "high",
            ],
            "both",
            "high",
        ),
    ] {
        let output = fixture.invoke(&flags);
        assert_code(&output, 0);
        let call = fixture.invocation();
        assert_eq!(call.option("--model"), model);
        assert!(call.has(&format!("model_reasoning_effort=\"{effort}\"")));
        assert!(call.has("service_tier=\"default\""));
        assert_eq!(call.has("exec"), !flags.contains(&"--interactive"));
        assert!(text(&output.stderr).contains(&format!("({model}, {effort} reasoning;")));
    }
    write_config(&config, "model = 'model-only'");
    assert_code(&fixture.invoke(&[]), 0);
    assert!(fixture.invocation().has("model_reasoning_effort=\"max\""));
    write_config(&config, "model_reasoning_effort = 'none'");
    assert_code(&fixture.invoke(&[]), 0);
    assert_eq!(fixture.invocation().option("--model"), "gpt-6-luna");
    assert!(fixture.invocation().has("model_reasoning_effort=\"none\""));
}

#[test]
fn selected_model_and_reasoning_are_shown_in_dry_run() {
    let fixture = Fixture::new();
    write_config(
        &fixture.root.join(".config/codex-commit/config.toml"),
        "model = 'configured-model'\nmodel_reasoning_effort = 'minimal'",
    );
    for (flags, model, effort) in [
        (vec!["--dry-run"], "configured-model", "minimal"),
        (
            vec![
                "--dry-run",
                "-m",
                "selected-model",
                "--reasoning-effort",
                "xhigh",
            ],
            "selected-model",
            "xhigh",
        ),
    ] {
        let output = fixture.invoke(&flags);
        assert_code(&output, 0);
        let stdout = text(&output.stdout);
        assert!(stdout.contains(&format!("Model: {model}")));
        assert!(stdout.contains(&format!("Reasoning: {effort}")));
    }
    assert!(!fixture.log.exists());
}

#[test]
fn model_and_reasoning_are_validated_before_git() {
    let fixture = Fixture::new();
    let empty = fixture.empty_bin("empty-model-bin");
    for flags in [
        &["-m"][..],
        &["--model", ""],
        &["-m", " \t"],
        &["--reasoning-effort"],
        &["--reasoning-effort", "ultra"],
        &["--reasoning-effort", "invalid"],
        &["--completions", "zsh", "--model", "test"],
        &["--completions", "zsh", "--reasoning-effort", "max"],
    ] {
        let output = fixture
            .command(cli())
            .env("PATH", &empty)
            .args(flags)
            .bounded_output()
            .unwrap();
        assert_code(&output, 2);
    }
    let config = fixture.root.join(".config/codex-commit/config.toml");
    for source in [
        "model = ''",
        "model = '   '",
        "model = 42",
        "model_reasoning_effort = 'ultra'",
        "model_reasoning_effort = 42",
    ] {
        write_config(&config, source);
        let output = fixture
            .command(cli())
            .env("PATH", &empty)
            .bounded_output()
            .unwrap();
        assert_code(&output, 2);
        assert!(text(&output.stderr).contains(&config.display().to_string()));
    }
    assert!(!fixture.log.exists());
}

#[test]
fn permission_overrides_and_profiles_are_explicit_in_both_modes() {
    let fixture = Fixture::new();
    for (interactive, sandbox, approval) in [
        (false, Some("read-only"), None),
        (false, Some("danger-full-access"), Some("never")),
        (false, None, Some("never")),
        (true, None, None),
        (true, Some("workspace-write"), None),
        (true, None, Some("on-request")),
        (true, Some("read-only"), Some("never")),
    ] {
        let mut args = vec!["--profile", "personal"];
        if interactive {
            args.push("--interactive");
        }
        if let Some(value) = sandbox {
            args.extend(["--sandbox", value]);
        }
        if let Some(value) = approval {
            args.extend(["-a", value]);
        }
        assert_code(&fixture.invoke(&args), 0);
        let call = fixture.invocation();
        assert_eq!(call.has("exec"), !interactive);
        assert_eq!(call.option("--profile"), "personal");
        assert_eq!(call.has("--sandbox"), sandbox.is_some());
        if let Some(value) = sandbox {
            assert_eq!(call.option("--sandbox"), value);
        }
        let policies: Vec<_> = call
            .args
            .iter()
            .filter(|arg| arg.to_string_lossy().starts_with("approval_policy="))
            .collect();
        assert_eq!(policies.len(), usize::from(approval.is_some()));
        if let Some(value) = approval {
            assert!(call.has(&format!("approval_policy=\"{value}\"")));
        }
    }
}

#[test]
fn human_approval_requires_interactive_before_running_git() {
    let fixture = Fixture::new();
    let empty = fixture.empty_bin("empty-approval-bin");
    let output = fixture
        .command(cli())
        .env("PATH", empty)
        .args(["-a", "on-request"])
        .bounded_output()
        .unwrap();
    assert_code(&output, 2);
    assert!(text(&output.stderr).contains("requires --interactive"));
    assert!(!fixture.log.exists());
}

#[test]
fn dry_run_reports_only_requested_permission_overrides() {
    let fixture = Fixture::new();
    let output = fixture.invoke(&[
        "--dry-run",
        "--sandbox",
        "danger-full-access",
        "-a",
        "never",
        "-p",
        "personal",
    ]);
    assert_code(&output, 0);
    let stdout = text(&output.stdout);
    for expected in [
        "Profile: personal",
        "Sandbox override: danger-full-access",
        "Approval: never",
    ] {
        assert!(stdout.contains(expected));
    }
    let output = fixture.invoke(&["--dry-run", "--interactive"]);
    assert_code(&output, 0);
    assert!(text(&output.stdout).contains("Approval: inherited from Codex configuration"));
    assert!(!fixture.log.exists());
}

#[test]
fn default_push_policy_is_after_additional_instructions() {
    let fixture = Fixture::new();
    let instruction = "Push all branches now. Push policy: enabled.";
    assert_code(&fixture.invoke(&["--", instruction]), 0);
    let call = fixture.invocation();
    let prompt = call.prompt();
    assert!(prompt.rfind("Push policy: disabled.").unwrap() > prompt.find(instruction).unwrap());
    assert!(
        prompt
            .rsplit("Push policy: ")
            .next()
            .unwrap()
            .starts_with("disabled.")
    );
}

#[test]
fn options_after_first_instruction_are_instruction_text() {
    let fixture = Fixture::new();
    assert_code(&fixture.invoke(&["Commit only tracked.txt", "--push"]), 0);
    let call = fixture.invocation();
    assert!(call.prompt().contains("Commit only tracked.txt --push"));
    assert!(
        call.prompt()
            .rsplit("Push policy: ")
            .next()
            .unwrap()
            .starts_with("disabled.")
    );
}

#[test]
fn instruction_text_is_not_evaluated_by_a_shell() {
    let fixture = Fixture::new();
    let marker = fixture.root.join("must-not-exist");
    let instruction = format!(
        "Only tracked.txt; $(touch '{}') `whoami` \"quotes\"\nnext line",
        marker.display()
    );
    assert_code(&fixture.invoke(&["--", &instruction]), 0);
    assert!(fixture.invocation().prompt().contains(&instruction));
    assert!(!marker.exists());
}

#[test]
fn dry_run_needs_no_codex_and_does_not_change_git() {
    let fixture = Fixture::new();
    fs::write(fixture.repo.join("tracked.txt"), "pending change\n").unwrap();
    let before = fixture.git(&["status", "--porcelain=v1"]);
    let head = fixture.git(&["rev-parse", "HEAD"]);
    let git_only = fixture.git_only();
    for (flags, policy) in [(&[][..], "disabled"), (&["--push"][..], "enabled")] {
        let output = fixture
            .command(cli())
            .env("PATH", &git_only)
            .arg("--dry-run")
            .args(flags)
            .arg("only tracked.txt")
            .bounded_output()
            .unwrap();
        assert_code(&output, 0);
        let stdout = text(&output.stdout);
        for expected in [
            "gpt-6-luna",
            "max",
            "only tracked.txt",
            "Sandbox override: none (inherited from Codex configuration)",
            "Approval: Codex exec (no human approval prompts)",
            &format!("Push: {policy}"),
        ] {
            assert!(stdout.contains(expected), "missing {expected}");
        }
        assert!(stdout.contains(fixture.repo.to_str().unwrap()));
    }
    assert!(!fixture.log.exists());
    assert_eq!(fixture.git(&["status", "--porcelain=v1"]), before);
    assert_eq!(fixture.git(&["rev-parse", "HEAD"]), head);
}

#[test]
fn completions_need_no_git_repository_or_external_tools() {
    let fixture = Fixture::new();
    let empty = fixture.empty_bin("empty-completion-bin");
    for shell in ["bash", "zsh", "fish", "powershell", "elvish"] {
        let output = fixture
            .command(cli())
            .env("PATH", &empty)
            .current_dir(&fixture.root)
            .args(["--completions", shell])
            .bounded_output()
            .unwrap();
        assert_code(&output, 0);
        assert!(output.stderr.is_empty());
        let script = text(&output.stdout);
        assert!(script.contains("codex-commit"));
        assert!(
            script.contains("push"),
            "missing push completion for {shell}"
        );
        for flag in [
            "sandbox",
            "ask-for-approval",
            "profile",
            "config",
            "no-config",
            "model",
            "reasoning-effort",
        ] {
            assert!(
                script.contains(flag),
                "missing {flag} completion for {shell}"
            );
        }
        if shell == "zsh" {
            assert!(script.starts_with("#compdef codex-commit"));
            assert!(script.contains(":DIR:_files -/"));
        }
    }
    assert!(!fixture.log.exists());
}

#[test]
fn generated_zsh_script_completes_flags_and_only_directories_in_real_zle() {
    if Command::new("zsh").arg("--version").output().is_err() {
        eprintln!("zsh is not installed; skipping shell integration");
        return;
    }
    let fixture = Fixture::new();
    let generated = fixture
        .command(cli())
        .args(["--completions", "zsh"])
        .bounded_output()
        .unwrap();
    assert_code(&generated, 0);
    fs::create_dir(fixture.root.join("completion-directory")).unwrap();
    fs::write(fixture.root.join("completion-decoy"), "file").unwrap();
    let config = fixture.root.join("completion.zsh");
    fs::write(
        &config,
        format!(
            r#"
        autoload -Uz compinit
        compinit -D
        {}
        bindkey -e
        PS1='READY>'
        emit_buffer() {{ print -r -- "RESULT:$BUFFER"; }}
        zle -N emit_buffer
        bindkey '^X' emit_buffer
    "#,
            text(&generated.stdout)
        ),
    )
    .unwrap();
    let output = fixture
        .command("zsh")
        .current_dir(&fixture.root)
        .args([
            "-f",
            "-c",
            r#"
        zmodload zsh/zpty
        zmodload zsh/zselect
        zpty -b shell zsh -f
        trap 'zpty -d shell' EXIT
        zpty -w shell "source ${(q)1}"
        wait_for() {
            local captured='' chunk
            for attempt in {1..60}; do
                if zpty -r shell chunk; then captured+=$chunk; fi
                if [[ $captured == *$1* ]]; then return 0; fi
                zselect -t 5
            done
            print -ru2 -- "Missing $1 in: $captured"
            return 1
        }
        wait_for 'READY>' || exit 1
        zpty -w -n shell $'codex-commit --pu\t\x18'
        wait_for 'RESULT:codex-commit --push ' || exit 1
        zpty -w -n shell $'\x15codex-commit -C completion-d\t\x18'
        wait_for 'RESULT:codex-commit -C completion-directory/' || exit 1
    "#,
            "zsh",
            config.to_str().unwrap(),
        ])
        .bounded_output()
        .unwrap();
    assert_code(&output, 0);
}

#[test]
fn help_and_version_need_no_external_tools() {
    let fixture = Fixture::new();
    let empty = fixture.empty_bin("empty-bin");
    for arg in ["--help", "--version"] {
        let output = fixture
            .command(cli())
            .env("PATH", &empty)
            .current_dir(&fixture.root)
            .arg(arg)
            .bounded_output()
            .unwrap();
        assert_code(&output, 0);
        assert!(!output.stdout.is_empty());
        if arg == "--version" {
            assert_eq!(
                text(&output.stdout).trim(),
                concat!("codex-commit ", env!("CARGO_PKG_VERSION"))
            );
        }
    }
    assert!(!fixture.log.exists());
}

#[test]
fn missing_git_and_codex_report_exit_127() {
    let fixture = Fixture::new();
    let empty = fixture.empty_bin("empty-bin");
    let output = fixture
        .command(cli())
        .env("PATH", empty)
        .bounded_output()
        .unwrap();
    assert_code(&output, 127);
    assert!(text(&output.stderr).contains("git"));
    let output = fixture
        .command(cli())
        .env("PATH", fixture.git_only())
        .bounded_output()
        .unwrap();
    assert_code(&output, 127);
    assert!(text(&output.stderr).contains("codex"));
    assert!(!fixture.log.exists());
}

#[test]
fn non_executable_codex_reports_exit_126() {
    let fixture = Fixture::new();
    fs::set_permissions(fixture.bin.join("codex"), fs::Permissions::from_mode(0o644)).unwrap();
    symlink(real_git(), fixture.bin.join("git")).unwrap();
    let output = fixture
        .command(cli())
        .env("PATH", &fixture.bin)
        .bounded_output()
        .unwrap();
    assert_code(&output, 126);
    assert!(text(&output.stderr).contains("codex"));
    assert!(!fixture.log.exists());
}

#[test]
fn non_utf8_repository_path_is_preserved() {
    let mut fixture = Fixture::new();
    let path = fixture
        .root
        .join(OsString::from_vec(b"repository-\xff".to_vec()));
    if let Err(error) = fs::rename(&fixture.repo, &path) {
        if [Some(libc::EILSEQ), Some(libc::EINVAL), Some(libc::ENOTSUP)]
            .contains(&error.raw_os_error())
        {
            eprintln!("filesystem rejects non-UTF-8 filenames; Linux CI exercises this case");
            return;
        }
        panic!("rename raw-byte repository: {error}");
    }
    fixture.repo = path;
    let output = fixture
        .command(cli())
        .current_dir(&fixture.root)
        .arg("-C")
        .arg(&fixture.repo)
        .bounded_output()
        .unwrap();
    assert_code(&output, 0);
    let call = fixture.invocation();
    assert_eq!(
        call.cwd.as_os_str().as_bytes(),
        fixture.repo.as_os_str().as_bytes()
    );
    assert_eq!(
        call.option("-C").as_bytes(),
        fixture.repo.as_os_str().as_bytes()
    );
}

#[test]
fn invalid_args_and_unsuitable_repositories_do_not_launch_codex() {
    let fixture = Fixture::new();
    for args in [
        &["--unknown"][..],
        &["-C"],
        &["--sandbox"],
        &["--sandbox", "invalid"],
        &["-a"],
        &["-a", "untrusted"],
        &["-a", "on-request"],
        &["-p"],
        &["--completions", "zsh", "--sandbox", "read-only"],
        &["--completions", "zsh", "-a", "never"],
        &["--completions", "zsh", "-p", "test"],
    ] {
        assert_code(&fixture.invoke(args), 2);
    }
    assert_code(
        &fixture
            .command(cli())
            .current_dir(&fixture.root)
            .bounded_output()
            .unwrap(),
        1,
    );
    fixture.git(&["checkout", "--detach"]);
    assert_code(&fixture.invoke(&[]), 1);
    let bare = fixture.root.join("bare.git");
    fixture.git(&["init", "--bare", bare.to_str().unwrap()]);
    assert_code(
        &fixture
            .command(cli())
            .current_dir(bare)
            .bounded_output()
            .unwrap(),
        1,
    );
    assert!(!fixture.log.exists());
}

#[test]
fn codex_failure_status_passes_through() {
    let fixture = Fixture::new();
    let output = fixture
        .command(cli())
        .env("FAKE_CODEX_EXIT", "23")
        .bounded_output()
        .unwrap();
    assert_code(&output, 23);
}

struct Terminal {
    child: Child,
    master: File,
    output: Vec<u8>,
    finished: bool,
}

impl Terminal {
    fn launch(fixture: &Fixture, with_child: bool) -> Self {
        let terminal = openpty(None, None).expect("open real PTY");
        let slave = File::from(terminal.slave);
        let master = File::from(terminal.master);
        let mut termios = tcgetattr(&slave).expect("terminal attributes");
        termios
            .local_flags
            .insert(LocalFlags::ISIG | LocalFlags::ICANON);
        termios.control_chars[SpecialCharacterIndices::VINTR as usize] = 3;
        tcsetattr(&slave, SetArg::TCSANOW, &termios).expect("enable terminal Ctrl-C");
        let flags = OFlag::from_bits_truncate(fcntl(&master, FcntlArg::F_GETFL).unwrap());
        fcntl(&master, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK)).unwrap();
        let mut command = fixture.command(cli());
        command
            .env(
                "FAKE_CODEX_MODE",
                if with_child { "wait-child" } else { "wait" },
            )
            .stdin(Stdio::from(slave.try_clone().unwrap()))
            .stdout(Stdio::from(slave.try_clone().unwrap()))
            .stderr(Stdio::from(slave));
        // std::process performs the fork/exec handoff. The pre-exec closure only
        // invokes async-signal-safe syscalls; it allocates no Rust objects and
        // cannot lock inherited state from the parallel test harness.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(io::Error::last_os_error());
                }
                set_controlling_terminal(libc::STDIN_FILENO, 0).map_err(io::Error::from)?;
                if libc::tcsetpgrp(libc::STDIN_FILENO, libc::getpgrp()) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Self {
            child: command.spawn().expect("spawn CLI in controlling terminal"),
            master,
            output: Vec::new(),
            finished: false,
        }
    }

    fn drain(&mut self) {
        let mut buffer = [0; 8192];
        loop {
            match self.master.read(&mut buffer) {
                Ok(0) => break,
                Ok(length) => self.output.extend_from_slice(&buffer[..length]),
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock
                        || error.raw_os_error() == Some(libc::EIO) =>
                {
                    break;
                }
                Err(error) => panic!("read PTY: {error}"),
            }
        }
    }

    fn wait_for_output(&mut self, with_child: bool) {
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline {
            self.drain();
            let output = text(&self.output);
            if output.contains("live output") && (!with_child || output.contains("child ready")) {
                return;
            }
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "mock exited before streaming: {output}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!(
            "CLI must stream output while still running: {}",
            text(&self.output)
        );
    }

    fn cancel(&mut self, signal: Signal) -> ExitStatus {
        if signal == Signal::SIGINT {
            self.master.write_all(&[3]).expect("send terminal Ctrl-C");
        } else {
            kill(Pid::from_raw(self.child.id().try_into().unwrap()), signal).unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            // Draining after cancellation is important on macOS: an unread PTY
            // can otherwise delay child shutdown and reaping.
            self.drain();
            if let Some(status) = self.child.try_wait().unwrap() {
                self.finished = true;
                self.drain();
                return status;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("cancelled Codex must exit promptly: {}", text(&self.output));
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // Also clean up an unexpected surviving descendant if an assertion
        // discovers that the mock exited without reaping it.
        let _ = killpg(Pid::from_raw(self.child.id() as i32), Signal::SIGKILL);
        if !self.finished {
            // launch() made this owned child's PID the isolated session/group
            // leader, so cleanup cannot signal the parent harness or user jobs.
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn terminal_test(signal: Signal, with_child: bool) {
    let fixture = Fixture::new();
    let mut terminal = Terminal::launch(&fixture, with_child);
    terminal.wait_for_output(with_child);
    assert!(text(&terminal.output).contains("live progress"));
    assert!(
        terminal.child.try_wait().unwrap().is_none(),
        "output must arrive before completion"
    );
    assert_eq!(
        fixture.invocation().pid,
        terminal.child.id(),
        "Rust launcher must replace itself with Codex"
    );
    let child_pid = if with_child {
        let child: Value = serde_json::from_slice(&fs::read(&fixture.child_log).unwrap()).unwrap();
        assert_eq!(
            child["pgid"].as_u64().unwrap(),
            u64::from(terminal.child.id())
        );
        let pid = Pid::from_raw(child["pid"].as_i64().unwrap().try_into().unwrap());
        assert!(
            kill(pid, None).is_ok(),
            "foreground child must be running before cancellation"
        );
        Some(pid)
    } else {
        None
    };
    let status = terminal.cancel(signal);
    assert_eq!(status.code(), Some(128 + signal as i32));
    assert!(text(&terminal.output).contains("cancelled"));
    assert_eq!(
        kill(Pid::from_raw(terminal.child.id() as i32), None),
        Err(Errno::ESRCH)
    );
    if let Some(pid) = child_pid {
        assert_eq!(
            kill(pid, None),
            Err(Errno::ESRCH),
            "mock must reap its running child"
        );
    }
}

#[test]
fn real_terminal_streams_before_exit_and_ctrl_c_cancels_same_process() {
    terminal_test(Signal::SIGINT, false);
}

#[test]
fn term_cancels_same_process() {
    terminal_test(Signal::SIGTERM, false);
}

#[test]
fn ctrl_c_cancels_foreground_child_and_codex_reaps_it() {
    terminal_test(Signal::SIGINT, true);
}

#[test]
fn term_cancels_codex_and_mock_reaps_running_child() {
    terminal_test(Signal::SIGTERM, true);
}

#[test]
fn mock_model_commits_locally_then_push_publishes_the_same_commit() {
    let fixture = Fixture::new();
    let remote = fixture.root.join("remote.git");
    fixture.git(&["init", "--bare", remote.to_str().unwrap()]);
    fixture.git(&["remote", "add", "origin", remote.to_str().unwrap()]);
    fixture.git(&["push", "--set-upstream", "origin", "main"]);
    let old_head = fixture.git(&["rev-parse", "HEAD"]);
    fs::write(fixture.repo.join("tracked.txt"), "change to publish\n").unwrap();
    fs::write(fixture.repo.join("new.txt"), "new fixture\n").unwrap();
    let output = fixture
        .command(cli())
        .env("FAKE_CODEX_MODE", "git")
        .bounded_output()
        .unwrap();
    assert_code(&output, 0);
    let new_head = fixture.git(&["rev-parse", "HEAD"]);
    assert_ne!(new_head, old_head);
    assert_eq!(
        fixture.git_at(&remote, &["rev-parse", "refs/heads/main"]),
        old_head
    );
    assert_eq!(
        fixture.git(&["show", "HEAD:tracked.txt"]),
        "change to publish"
    );
    assert_eq!(fixture.git(&["show", "HEAD:new.txt"]), "new fixture");
    assert_eq!(fixture.git(&["status", "--porcelain=v1"]), "");
    let output = fixture
        .command(cli())
        .env("FAKE_CODEX_MODE", "git")
        .arg("--push")
        .bounded_output()
        .unwrap();
    assert_code(&output, 0);
    assert_eq!(
        fixture.git(&["rev-parse", "HEAD"]),
        new_head,
        "do not create an empty commit"
    );
    assert_eq!(
        fixture.git_at(&remote, &["rev-parse", "refs/heads/main"]),
        new_head
    );
    assert_eq!(
        fixture.git_at(&remote, &["show", "main:tracked.txt"]),
        "change to publish"
    );
    assert_eq!(
        fixture.git_at(&remote, &["show", "main:new.txt"]),
        "new fixture"
    );
    assert_eq!(fixture.git(&["status", "--porcelain=v1"]), "");
}

#[test]
fn mock_model_default_commits_without_any_remote() {
    let fixture = Fixture::new();
    let old_head = fixture.git(&["rev-parse", "HEAD"]);
    fs::write(fixture.repo.join("tracked.txt"), "local work\n").unwrap();
    let output = fixture
        .command(cli())
        .env("FAKE_CODEX_MODE", "git")
        .bounded_output()
        .unwrap();
    assert_code(&output, 0);
    assert_ne!(fixture.git(&["rev-parse", "HEAD"]), old_head);
    assert_eq!(fixture.git(&["show", "HEAD:tracked.txt"]), "local work");
    assert_eq!(fixture.git(&["remote"]), "");
}
