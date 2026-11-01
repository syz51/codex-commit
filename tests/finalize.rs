//! Real-Git coverage for the native verified-commit boundary; no model calls.
#![cfg(unix)]

use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use tempfile::{Builder, TempDir};

struct Fixture {
    _temporary: Option<TempDir>,
    root: PathBuf,
    repo: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let supplied = env::var_os("CODEX_COMMIT_TEST_TMPDIR").map(PathBuf::from);
        let temporary = match &supplied {
            Some(base) => {
                fs::create_dir_all(base).unwrap();
                Builder::new().prefix("finalize-").tempdir_in(base)
            }
            None => Builder::new().prefix("codex-commit-finalize-").tempdir(),
        }
        .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let temporary = if supplied.is_some() {
            let _ = temporary.keep();
            None
        } else {
            Some(temporary)
        };
        let repo = root.join("repository with spaces");
        fs::create_dir(&repo).unwrap();
        let fixture = Self {
            _temporary: temporary,
            root,
            repo,
        };
        fixture.git(&["init", "-b", "main"]);
        fs::write(fixture.repo.join("tracked"), "original\n").unwrap();
        fixture.git(&["add", "tracked"]);
        fixture.git(&["commit", "-m", "Initial"]);
        fixture
    }

    fn command(&self, program: impl AsRef<OsStr>) -> Command {
        let mut command = Command::new(program);
        command
            .env_clear()
            .current_dir(&self.repo)
            .stdin(Stdio::null())
            .env("PATH", env::var_os("PATH").unwrap_or_default())
            .env("HOME", &self.root)
            .env("TMPDIR", &self.root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "Fixture")
            .env("GIT_COMMITTER_NAME", "Fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
            .env("GIT_TERMINAL_PROMPT", "0");
        command
    }

    fn git(&self, args: &[&str]) -> Vec<u8> {
        let output = self.command("git").args(args).output().unwrap();
        assert!(output.status.success(), "{output:?}");
        output.stdout
    }

    fn finalize(&self, paths: &[OsString]) -> Command {
        let binary = env::var_os("CODEX_COMMIT_BIN")
            .unwrap_or_else(|| OsString::from(env!("CARGO_BIN_EXE_codex-commit")));
        let mut command = self.command(binary);
        command
            .args(["finalize", "--message", "Selected update", "--"])
            .args(paths);
        command
    }

    fn stage(&self, names: &[OsString]) {
        let output = self
            .command("git")
            .args(["add", "--"])
            .args(names)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }

    fn hook(&self, source: &str) -> PathBuf {
        let path = self.repo.join(".git/hooks/pre-commit");
        fs::write(&path, source).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn fake_git(&self, failure_command: &str, code: i32) -> PathBuf {
        let real = env::split_paths(&env::var_os("PATH").unwrap_or_default())
            .map(|directory| directory.join("git"))
            .find(|path| path.is_file())
            .unwrap();
        let bin = self.root.join("fake-bin");
        fs::create_dir(&bin).unwrap();
        let script = format!(
            "#!/bin/sh\nif [ \"$1\" = '{failure_command}' ]; then exit {code}; fi\nexec '{}' \"$@\"\n",
            real.to_str().unwrap().replace('\'', "'\\''")
        );
        let fake = bin.join("git");
        fs::write(&fake, script).unwrap();
        fs::set_permissions(fake, fs::Permissions::from_mode(0o755)).unwrap();
        bin
    }
}

fn success(output: Output) {
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn three_paths_and_binary_filenames_commit_without_shell_transport() {
    let fixture = Fixture::new();
    let paths = vec![
        OsString::from("-first"),
        OsString::from("quoted'\nsecond"),
        if cfg!(target_os = "linux") {
            OsString::from_vec(b"third-\xff".to_vec())
        } else {
            OsString::from("third") // macOS filesystems reject non-UTF-8 names.
        },
    ];
    for path in &paths {
        fs::write(fixture.repo.join(path), "new content\n").unwrap();
    }
    fixture.stage(&paths);
    success(fixture.finalize(&paths).output().unwrap());
    assert_eq!(fixture.git(&["status", "--porcelain=v1"]), b"");
    let changed = fixture.git(&[
        "diff-tree",
        "--no-commit-id",
        "--name-only",
        "-r",
        "-z",
        "HEAD",
    ]);
    assert_eq!(changed.iter().filter(|byte| **byte == 0).count(), 3);
}

#[test]
fn scope_mismatches_preserve_index_and_head_without_running_hooks() {
    let fixture = Fixture::new();
    for name in ["first", "second", "third"] {
        fs::write(fixture.repo.join(name), "new\n").unwrap();
    }
    fixture.stage(&["first".into(), "second".into(), "third".into()]);
    fixture.hook("#!/bin/sh\nprintf ran > hook-ran\n");
    let head = fixture.git(&["rev-parse", "HEAD"]);
    let index = fs::read(fixture.repo.join(".git/index")).unwrap();
    for paths in [
        vec!["first".into()],
        vec![
            "first".into(),
            "second".into(),
            "third".into(),
            "extra".into(),
        ],
    ] {
        let output = fixture.finalize(&paths).output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&output.stderr).contains("do not exactly match"));
        assert_eq!(fixture.git(&["rev-parse", "HEAD"]), head);
        assert_eq!(fs::read(fixture.repo.join(".git/index")).unwrap(), index);
        assert!(!fixture.repo.join("hook-ran").exists());
    }
}

#[test]
fn unresolved_index_and_git_exit_one_stop_before_commit() {
    let fixture = Fixture::new();
    fixture.git(&["checkout", "-b", "other"]);
    fs::write(fixture.repo.join("tracked"), "other\n").unwrap();
    fixture.git(&["commit", "-am", "Other"]);
    fixture.git(&["checkout", "main"]);
    fs::write(fixture.repo.join("tracked"), "main\n").unwrap();
    fixture.git(&["commit", "-am", "Main"]);
    let merge = fixture
        .command("git")
        .args(["merge", "other"])
        .output()
        .unwrap();
    assert!(!merge.status.success());
    let head = fixture.git(&["rev-parse", "HEAD"]);
    let output = fixture.finalize(&["tracked".into()]).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("unmerged entries"));
    assert_eq!(fixture.git(&["rev-parse", "HEAD"]), head);

    let fixture = Fixture::new();
    fs::write(fixture.repo.join("tracked"), "changed\n").unwrap();
    fixture.stage(&["tracked".into()]);
    fixture.hook("#!/bin/sh\nprintf ran > hook-ran\n");
    let head = fixture.git(&["rev-parse", "HEAD"]);
    let bin = fixture.fake_git("diff", 1);
    let path = env::join_paths(
        std::iter::once(bin).chain(env::split_paths(&env::var_os("PATH").unwrap_or_default())),
    )
    .unwrap();
    let output = fixture
        .finalize(&["tracked".into()])
        .env("PATH", path)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("git diff failed"));
    assert_eq!(fixture.git(&["rev-parse", "HEAD"]), head);
    assert!(!fixture.repo.join("hook-ran").exists());
}

#[test]
fn moves_list_both_sides_and_partial_staging_stays_partial() {
    for git_mv in [false, true] {
        let fixture = Fixture::new();
        fixture.git(&["config", "diff.renames", "copies"]);
        if git_mv {
            fixture.git(&["mv", "tracked", "moved"]);
        } else {
            fs::rename(fixture.repo.join("tracked"), fixture.repo.join("moved")).unwrap();
            fixture.git(&["add", "-A", "--", "tracked", "moved"]);
        }
        success(
            fixture
                .finalize(&["tracked".into(), "moved".into()])
                .output()
                .unwrap(),
        );
        assert_eq!(fixture.git(&["ls-tree", "--name-only", "HEAD"]), b"moved\n");
        assert!(fixture.git(&["status", "--porcelain=v1"]).is_empty());
    }
    let fixture = Fixture::new();
    fs::write(fixture.repo.join("tracked"), "selected\n").unwrap();
    fixture.stage(&["tracked".into()]);
    fs::write(fixture.repo.join("tracked"), "remaining\n").unwrap();
    success(fixture.finalize(&["tracked".into()]).output().unwrap());
    assert_eq!(fixture.git(&["show", "HEAD:tracked"]), b"selected\n");
    assert_eq!(
        fs::read(fixture.repo.join("tracked")).unwrap(),
        b"remaining\n"
    );
    assert_eq!(fixture.git(&["status", "--porcelain=v1"]), b" M tracked\n");
}

#[test]
fn hooks_and_commit_failures_are_not_bypassed_or_retried() {
    let fixture = Fixture::new();
    fs::write(fixture.repo.join("tracked"), "changed\n").unwrap();
    fixture.stage(&["tracked".into()]);
    let head = fixture.git(&["rev-parse", "HEAD"]);
    fixture.hook("#!/bin/sh\nprintf rejected >&2\nexit 23\n");
    let output = fixture.finalize(&["tracked".into()]).output().unwrap();
    assert_eq!(output.status.code(), Some(1)); // Git translates a failed hook to exit 1.
    assert!(String::from_utf8_lossy(&output.stderr).contains("rejected"));
    assert_eq!(fixture.git(&["rev-parse", "HEAD"]), head);
    let bin = fixture.fake_git("commit", 23);
    let path = env::join_paths(
        std::iter::once(bin).chain(env::split_paths(&env::var_os("PATH").unwrap_or_default())),
    )
    .unwrap();
    let output = fixture
        .finalize(&["tracked".into()])
        .env("PATH", path)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(23));
    assert_eq!(fixture.git(&["rev-parse", "HEAD"]), head);
}

#[test]
fn post_commit_reporting_failure_reports_that_head_already_advanced() {
    let fixture = Fixture::new();
    fs::write(fixture.repo.join("tracked"), "changed\n").unwrap();
    fixture.stage(&["tracked".into()]);
    let head = fixture.git(&["rev-parse", "HEAD"]);
    let bin = fixture.fake_git("status", 19);
    let path = env::join_paths(
        std::iter::once(bin).chain(env::split_paths(&env::var_os("PATH").unwrap_or_default())),
    )
    .unwrap();
    let output = fixture
        .finalize(&["tracked".into()])
        .env("PATH", path)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(19));
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    assert!(diagnostic.contains("commit succeeded, but result verification failed"));
    assert!(!diagnostic.contains("commit not attempted"));
    assert_ne!(fixture.git(&["rev-parse", "HEAD"]), head);
    assert_eq!(fixture.git(&["show", "HEAD:tracked"]), b"changed\n");
}

#[test]
fn directory_selection_resolves_root_and_bare_repositories_are_rejected() {
    let fixture = Fixture::new();
    let binary = Path::new(env!("CARGO_BIN_EXE_codex-commit"));
    fs::create_dir(fixture.repo.join("nested")).unwrap();
    fs::write(fixture.repo.join("tracked"), "changed\n").unwrap();
    fixture.stage(&["tracked".into()]);
    success(
        fixture
            .command(binary)
            .args([
                "finalize",
                "-C",
                "nested",
                "--message",
                "Root-relative selection",
                "--",
                "tracked",
            ])
            .output()
            .unwrap(),
    );
    assert_eq!(fixture.git(&["show", "HEAD:tracked"]), b"changed\n");

    fixture.git(&["init", "--bare", "bare.git"]);
    let output = fixture
        .command(binary)
        .args([
            "finalize",
            "-C",
            "bare.git",
            "--message",
            "Never commit",
            "--",
            "tracked",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("bare repository"));
}

#[test]
fn empty_index_invalid_paths_and_detached_head_never_commit() {
    let fixture = Fixture::new();
    let output = fixture.finalize(&["tracked".into()]).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let head = fixture.git(&["rev-parse", "HEAD"]);
    for args in [
        vec!["finalize"],
        vec!["finalize", "--message", " ", "--", "tracked"],
        vec!["finalize", "--message", "message", "--", "../tracked"],
        vec!["finalize", "--message", "message", "--", "./tracked"],
        vec!["finalize", "--message", "message", "--", "/tracked"],
        vec![
            "finalize",
            "--message",
            "message",
            "--",
            "tracked",
            "tracked",
        ],
    ] {
        let binary = Path::new(env!("CARGO_BIN_EXE_codex-commit"));
        let output = fixture.command(binary).args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{output:?}");
    }
    fixture.git(&["checkout", "--detach"]);
    fs::write(fixture.repo.join("tracked"), "changed\n").unwrap();
    fixture.stage(&["tracked".into()]);
    assert_eq!(
        fixture
            .finalize(&["tracked".into()])
            .output()
            .unwrap()
            .status
            .code(),
        Some(1)
    );
    assert_eq!(fixture.git(&["rev-parse", "HEAD"]), head);
}
