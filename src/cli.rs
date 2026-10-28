use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn cli() -> clap::Command {
    use clap::{Arg, ArgAction, ValueHint, value_parser};
    clap::Command::new("codex-commit")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Inspect and commit changes with Codex, using GPT-6 Luna and Max reasoning.")
        .after_help("Options precede optional instructions. Use -- before instructions beginning with a dash.\nProgress and command output stream to the terminal. Press Ctrl-C to interrupt.\nThe default unattended mode gives Codex full local command access.")
        .args_override_self(true)
        .arg(Arg::new("directory").short('C').value_name("DIR").default_value(".")
            .value_parser(value_parser!(PathBuf)).value_hint(ValueHint::DirPath)
            .allow_hyphen_values(true).help("Run in this Git repository"))
        .arg(Arg::new("push").long("push").action(ArgAction::SetTrue)
            .help("Also push the current branch after committing"))
        .arg(Arg::new("dry-run").long("dry-run").action(ArgAction::SetTrue)
            .help("Print the task and settings without launching Codex"))
        .arg(Arg::new("interactive").long("interactive").action(ArgAction::SetTrue)
            .help("Use Codex's interactive terminal with permission prompts"))
        .arg(Arg::new("completions").long("completions").value_name("SHELL")
            .value_parser(value_parser!(clap_complete::Shell))
            .conflicts_with_all(["directory", "push", "dry-run", "interactive", "instructions"])
            .help("Print a shell completion script without launching Codex"))
        .arg(Arg::new("instructions").value_name("INSTRUCTIONS").num_args(0..)
            .trailing_var_arg(true).value_parser(value_parser!(OsString))
            .help("Additional instructions for Codex"))
}

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
    Display(String),
    Completions(clap_complete::Shell),
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
    let matches = match cli()
        .try_get_matches_from(std::iter::once(OsString::from("codex-commit")).chain(args))
    {
        Ok(matches) => matches,
        Err(error)
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) =>
        {
            return Ok(Invocation::Display(error.to_string()));
        }
        Err(error) => return Err(CliError::new(error.exit_code(), error.to_string())),
    };
    if let Some(shell) = matches.get_one::<clap_complete::Shell>("completions") {
        return Ok(Invocation::Completions(*shell));
    }
    Ok(Invocation::Task(Options {
        directory: matches
            .get_one::<PathBuf>("directory")
            .expect("default directory")
            .clone(),
        dry_run: matches.get_flag("dry-run"),
        interactive: matches.get_flag("interactive"),
        push: matches.get_flag("push"),
        instructions: join_instructions(
            matches
                .get_many::<OsString>("instructions")
                .into_iter()
                .flatten()
                .cloned(),
        )?,
    }))
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
        Invocation::Display(output) => {
            print!("{output}");
            return Ok(());
        }
        Invocation::Completions(shell) => {
            clap_complete::generate(shell, &mut cli(), "codex-commit", &mut io::stdout());
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
    fn clap_definition_is_consistent() {
        cli().debug_assert();
    }

    #[test]
    fn delimiter_preserves_dash_leading_instructions() {
        let Invocation::Task(options) = parse(arguments(&["--", "--push", "only README"]))
            .unwrap_or_else(|error| panic!("{}", error.message))
        else {
            panic!("expected task");
        };
        assert!(!options.push);
        assert_eq!(options.instructions, "--push only README");
    }

    #[test]
    fn invalid_instruction_bytes_are_rejected_at_boundary() {
        let error = parse([OsString::from_vec(vec![0xff])]).expect_err("invalid UTF-8");
        assert_eq!(error.code, 2);
        assert_eq!(
            error.message,
            "additional instructions must be valid UTF-8 text"
        );
    }

    #[test]
    fn completions_require_a_shell_and_exclude_task_options() {
        for args in [
            &["--completions"][..],
            &["--completions", "invalid"],
            &["--completions", "zsh", "--push"],
            &["-C", ".", "--completions", "zsh"],
            &["--completions", "zsh", "instructions"],
        ] {
            assert_eq!(
                parse(arguments(args)).expect_err("invalid invocation").code,
                2
            );
        }
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
