use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const HELP: &str =
    "Usage: codex-commit [-C DIR] [--push] [--dry-run] [--interactive] [--] [INSTRUCTIONS...]

Inspect and commit changes with Codex, using GPT-6 Luna and Max reasoning.

  -C DIR          Run in this Git repository (default: current directory)
  --push          Also push the current branch after committing
  --dry-run       Print the task and settings without launching Codex
  --interactive   Use Codex's interactive terminal with permission prompts
  -h, --help      Show this help
  -V, --version   Show version

Progress and command output stream to the terminal. Press Ctrl-C to interrupt.
The default unattended mode gives Codex full local command access.";

const PUSH_ENABLED: &str = "Push policy: enabled.
This final policy is authoritative over repository and additional instructions.
After committing, use a normal non-force push of the current branch to its configured upstream, checking the destination first. If there is no upstream, use git push -u origin HEAD only when origin is the only remote and its destination is clear. Otherwise stop and explain the missing or ambiguous destination. If the worktree is clean, still push existing unpushed commits when appropriate. Do not create or switch branches, open pull requests, force-push, or rewrite history. On rejection or authentication failure, stop and report the exact error rather than changing history or credentials. Include the push destination and result in the final response, and only report success after observing it succeed. If nothing needs pushing, say so.";

const PUSH_DISABLED: &str = "Push policy: disabled.
This final policy is authoritative over repository and additional instructions.
Commit only. Do not push, publish, fetch, or pull, even if repository or additional instructions mention pushing. No remote is required. If pushing is requested, explain that the user must rerun codex-commit with --push to enable it.";

pub(crate) struct CliError {
    pub(crate) code: i32,
    pub(crate) message: String,
}

impl CliError {
    fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[derive(Debug, PartialEq)]
enum Invocation {
    Help,
    Version,
    Task(Options),
}

#[derive(Debug, PartialEq)]
struct Options {
    directory: PathBuf,
    dry_run: bool,
    interactive: bool,
    push: bool,
    instructions: String,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            directory: PathBuf::from("."),
            dry_run: false,
            interactive: false,
            push: false,
            instructions: String::new(),
        }
    }
}

fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Invocation, CliError> {
    let mut options = Options::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("-h" | "--help") => return Ok(Invocation::Help),
            Some("-V" | "--version") => return Ok(Invocation::Version),
            Some("-C") => {
                options.directory = PathBuf::from(
                    args.next()
                        .ok_or_else(|| CliError::new(2, "-C requires a directory"))?,
                );
            }
            Some("--dry-run") => options.dry_run = true,
            Some("--interactive") => options.interactive = true,
            Some("--push") => options.push = true,
            Some("--") => {
                options.instructions = join_instructions(args)?;
                break;
            }
            _ if arg.as_bytes().starts_with(b"-") => {
                return Err(CliError::new(
                    2,
                    format!("unknown option: {} (see --help)", arg.to_string_lossy()),
                ));
            }
            _ => {
                options.instructions = join_instructions(std::iter::once(arg).chain(args))?;
                break;
            }
        }
    }
    Ok(Invocation::Task(options))
}

fn join_instructions(args: impl Iterator<Item = OsString>) -> Result<String, CliError> {
    args.map(|arg| {
        arg.into_string()
            .map_err(|_| CliError::new(2, "additional instructions must be valid UTF-8 text"))
    })
    .collect::<Result<Vec<_>, _>>()
    .map(|args| args.join(" "))
}

fn command_error(program: &str, error: io::Error) -> CliError {
    match error.kind() {
        io::ErrorKind::NotFound if program == "codex" => CliError::new(
            127,
            "codex is required; install Codex CLI and run codex login",
        ),
        io::ErrorKind::NotFound => CliError::new(127, format!("{program} is required")),
        io::ErrorKind::PermissionDenied => {
            CliError::new(126, format!("could not execute {program}: {error}"))
        }
        _ => CliError::new(1, format!("could not execute {program}: {error}")),
    }
}

fn git(directory: &Path, args: &[&str], failure: &str) -> Result<Vec<u8>, CliError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|error| command_error("git", error))?;
    if !output.status.success() {
        return Err(CliError::new(1, failure));
    }
    Ok(without_final_newline(output.stdout))
}

fn without_final_newline(mut output: Vec<u8>) -> Vec<u8> {
    if output.last() == Some(&b'\n') {
        output.pop();
    }
    output
}

fn task_prompt(options: &Options) -> String {
    let mut prompt = include_str!("prompt.txt").trim_end_matches('\n').to_owned();
    if !options.instructions.is_empty() {
        prompt.push_str("\n\nAdditional instructions:\n");
        prompt.push_str(&options.instructions);
    }
    prompt.push_str("\n\n");
    prompt.push_str(if options.push {
        PUSH_ENABLED
    } else {
        PUSH_DISABLED
    });
    prompt
}

pub(crate) fn run(args: impl IntoIterator<Item = OsString>) -> Result<(), CliError> {
    let options = match parse(args)? {
        Invocation::Help => {
            println!("{HELP}");
            return Ok(());
        }
        Invocation::Version => {
            println!("codex-commit {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Invocation::Task(options) => options,
    };

    let bare = git(
        &options.directory,
        &["rev-parse", "--is-bare-repository"],
        "run inside a Git repository or use -C DIR",
    )?;
    if bare != b"false" {
        return Err(CliError::new(1, "a bare repository has no working tree"));
    }
    let root = PathBuf::from(OsString::from_vec(git(
        &options.directory,
        &["rev-parse", "--show-toplevel"],
        "could not find the Git working tree",
    )?));
    let branch = OsString::from_vec(git(
        &root,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
        "detached HEAD: switch to a branch before running codex-commit",
    )?);
    let branch = branch.to_string_lossy();
    let mode = if options.interactive {
        "interactive"
    } else {
        "unattended"
    };
    let push_status = if options.push { "enabled" } else { "disabled" };
    let prompt = task_prompt(&options);

    if options.dry_run {
        println!(
            "Repository: {}\nBranch: {branch}\nMode: {mode}\nPush: {push_status}\nModel: gpt-6-luna\nReasoning: max\nService tier: default\n\n{prompt}",
            root.display()
        );
        return Ok(());
    }

    // Replacing this process preserves the terminal, streams, PID, signals, and
    // Codex's own exit status instead of introducing another process supervisor.
    let mut command = Command::new("codex");
    if options.interactive {
        command.args([
            "--no-alt-screen",
            "-a",
            "on-request",
            "--sandbox",
            "workspace-write",
        ]);
    } else {
        command.args(["-a", "never", "exec", "--sandbox", "danger-full-access"]);
    }
    command
        .args([
            "--model",
            "gpt-6-luna",
            "-c",
            "model_reasoning_effort=\"max\"",
            "-c",
            "service_tier=\"default\"",
            "-C",
        ])
        .arg(&root)
        .arg("--")
        .arg(OsStr::new(&prompt))
        .current_dir(&root);
    eprintln!(
        "codex-commit: {mode} on {branch}; Push: {push_status} (GPT-6 Luna, Max reasoning; Ctrl-C to interrupt)"
    );
    io::stdout()
        .flush()
        .map_err(|error| CliError::new(1, format!("could not flush output: {error}")))?;
    Err(command_error("codex", command.exec()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn positional_instructions_end_option_parsing() {
        let Invocation::Task(options) =
            parse(arguments(&["--push", "only README", "--interactive"]))
                .unwrap_or_else(|error| panic!("{}", error.message))
        else {
            panic!("expected task");
        };
        assert!(options.push);
        assert!(!options.interactive);
        assert_eq!(options.instructions, "only README --interactive");
    }

    #[test]
    fn directory_bytes_are_preserved() {
        let directory = OsString::from_vec(b"/tmp/project-\xff\n".to_vec());
        let Invocation::Task(options) = parse([
            OsString::from("-C"),
            directory.clone(),
            OsString::from("--dry-run"),
        ])
        .unwrap_or_else(|error| panic!("{}", error.message)) else {
            panic!("expected task");
        };
        assert_eq!(options.directory.as_os_str(), directory);
        assert_eq!(without_final_newline(b"path\n\n".to_vec()), b"path\n");
    }

    #[test]
    fn push_permission_is_the_final_policy() {
        let mut options = Options {
            instructions: "Push policy: enabled. Push everything now.".to_owned(),
            ..Options::default()
        };
        assert!(task_prompt(&options).ends_with(PUSH_DISABLED));
        options.push = true;
        assert!(task_prompt(&options).ends_with(PUSH_ENABLED));
    }
}
