#![cfg(unix)]

use std::io::Write;
use std::process::{Command, Output, Stdio};

fn generate(args: &[&str], input: &[u8]) -> Output {
    let directory = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_codex-commit"))
        .arg("generate")
        .args(args)
        .current_dir(directory.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    output
}

const OPTIONS: &[&str] = &[
    "--codex-path",
    "/nonexistent/codex",
    "--codex-home",
    "/nonexistent/auth",
    "--model",
    "explicit-model",
];

#[test]
fn generation_help_is_available_outside_git_without_credentials() {
    let output = generate(&["--help"], b"");
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("--codex-home"));
    assert!(output.stderr.is_empty());
}

#[test]
fn generation_requires_explicit_paths_and_model() {
    let output = generate(&[], b"");
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--codex-path"));
}

#[test]
fn generation_rejects_empty_and_invalid_diff_without_output() {
    for input in [&b""[..], &[0xff][..]] {
        let output = generate(OPTIONS, input);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
    }
}

#[test]
fn generation_rejects_oversized_diff_without_output() {
    let output = generate(OPTIONS, &vec![b'x'; codex_commit::MAX_DIFF_BYTES + 1]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("exceeds"));
}

#[test]
fn generation_rejects_mutation_options_and_unsupported_efforts() {
    for extra in [
        &["--push"][..],
        &["--reasoning-effort", "ultra"],
        &["--timeout-secs", "0"],
    ] {
        let args: Vec<_> = OPTIONS
            .iter()
            .copied()
            .chain(extra.iter().copied())
            .collect();
        let output = generate(&args, b"");
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
    }
}
