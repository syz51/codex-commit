//! Verify the selected index scope and commit without a shell verification chain.

use crate::cli::CliError;
use clap::{Arg, Command as Cli, value_parser};
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::io::{self, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

fn cli() -> Cli {
    Cli::new("codex-commit finalize")
        .about("Commit an already staged index only when its paths exactly match the selected paths.")
        .after_help("Paths are literal repository-root-relative names, including both sides of a rename. This command does not stage, unstage, run checks, or launch Codex. Git hooks remain enabled.")
        .arg(Arg::new("directory").short('C').default_value(".").value_name("DIR")
            .value_parser(value_parser!(PathBuf)))
        .arg(Arg::new("message").long("message").required(true).allow_hyphen_values(true).value_name("TEXT"))
        .arg(Arg::new("paths").required(true).num_args(1..).last(true).value_name("PATH")
            .value_parser(value_parser!(OsString)))
}

fn status_code(status: ExitStatus) -> i32 {
    status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(1))
}

fn git_output(directory: &Path, args: &[&str]) -> Result<Vec<u8>, CliError> {
    let output = Command::new("git")
        .current_dir(directory)
        .args(args)
        .output()
        .map_err(|error| CliError::new(1, format!("could not run git {}: {error}", args[0])))?;
    if !output.status.success() {
        io::stderr()
            .write_all(&output.stderr)
            .map_err(|error| CliError::new(1, format!("could not report Git failure: {error}")))?;
        return Err(CliError::new(
            status_code(output.status),
            format!("git {} failed", args[0]),
        ));
    }
    Ok(output.stdout)
}

pub(crate) fn run(args: impl IntoIterator<Item = OsString>) -> Result<i32, CliError> {
    let matches = match cli()
        .try_get_matches_from(std::iter::once(OsString::from("codex-commit finalize")).chain(args))
    {
        Ok(matches) => matches,
        Err(error) if error.kind() == clap::error::ErrorKind::DisplayHelp => {
            print!("{error}");
            return Ok(0);
        }
        Err(error) => return Err(CliError::new(error.exit_code(), error.to_string())),
    };
    let message = matches
        .get_one::<String>("message")
        .expect("required message");
    if message.trim().is_empty() {
        return Err(CliError::new(2, "commit message must not be blank"));
    }
    let mut selected = BTreeSet::new();
    for path in matches
        .get_many::<OsString>("paths")
        .expect("required paths")
    {
        let bytes = path.as_bytes();
        if bytes
            .split(|byte| *byte == b'/')
            .any(|part| part.is_empty() || part == b"." || part == b"..")
        {
            return Err(CliError::new(
                2,
                "selected paths must be exact repository-root-relative names",
            ));
        }
        if !selected.insert(bytes.to_vec()) {
            return Err(CliError::new(
                2,
                "selected paths must not contain duplicates",
            ));
        }
    }

    let directory = matches
        .get_one::<PathBuf>("directory")
        .expect("default directory");
    if git_output(directory, &["rev-parse", "--is-bare-repository"])? != b"false\n" {
        return Err(CliError::new(1, "a bare repository has no working tree"));
    }
    let mut root = git_output(directory, &["rev-parse", "--show-toplevel"])?;
    if root.pop() != Some(b'\n') || root.is_empty() {
        return Err(CliError::new(
            1,
            "Git returned an invalid working-tree root",
        ));
    }
    let root = PathBuf::from(OsString::from_vec(root));
    git_output(&root, &["symbolic-ref", "--quiet", "HEAD"])?;
    if !git_output(&root, &["ls-files", "--unmerged", "-z"])?.is_empty() {
        return Err(CliError::new(
            1,
            "index has unmerged entries; commit not attempted",
        ));
    }
    let staged = git_output(
        &root,
        &[
            "diff",
            "--cached",
            "--name-only",
            "-z",
            "--no-renames",
            "--no-ext-diff",
            "--no-textconv",
            "--ignore-submodules=none",
            "--no-color",
        ],
    )?;
    if staged.last() != Some(&0) {
        return Err(CliError::new(
            1,
            "index has no staged paths or Git returned an invalid path list; commit not attempted",
        ));
    }
    let staged: BTreeSet<_> = staged[..staged.len() - 1]
        .split(|byte| *byte == 0)
        .map(<[u8]>::to_vec)
        .collect();
    if staged != selected {
        return Err(CliError::new(
            1,
            "staged paths do not exactly match selected paths; index preserved, commit not attempted",
        ));
    }

    let status = Command::new("git")
        .current_dir(&root)
        .args(["commit", "-m", message])
        .status()
        .map_err(|error| CliError::new(1, format!("could not run git commit: {error}")))?;
    if !status.success() {
        return Ok(status_code(status));
    }
    let report_failure = |error: CliError| {
        CliError::new(
            error.code,
            format!(
                "commit succeeded, but result verification failed: {}",
                error.message
            ),
        )
    };
    let head = git_output(&root, &["rev-parse", "--verify", "HEAD"]).map_err(report_failure)?;
    let status = git_output(&root, &["status", "--porcelain=v1"]).map_err(report_failure)?;
    let mut stdout = io::stdout().lock();
    stdout
        .write_all(b"Commit: ")
        .and_then(|()| stdout.write_all(&head))
        .and_then(|()| stdout.write_all(&status))
        .map_err(|error| {
            CliError::new(
                1,
                format!("commit succeeded, but could not report its result: {error}"),
            )
        })?;
    Ok(0)
}
