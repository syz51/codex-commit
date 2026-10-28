use serde::Deserialize;
use std::env;
use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Clone, Copy, Debug, PartialEq, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
enum Sandbox {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

impl Sandbox {
    fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
            Self::DangerFullAccess => "danger-full-access",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
enum ApprovalPolicy {
    OnRequest,
    Never,
}

impl ApprovalPolicy {
    fn as_str(self) -> &'static str {
        match self {
            Self::OnRequest => "on-request",
            Self::Never => "never",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(try_from = "String")]
struct Model(String);

impl TryFrom<String> for Model {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.trim().is_empty() {
            return Err("model must not be blank".to_owned());
        }
        Ok(Self(value))
    }
}

impl std::str::FromStr for Model {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::try_from(value.to_owned())
    }
}

impl Default for Model {
    fn default() -> Self {
        Self("gpt-6-luna".to_owned())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
enum ReasoningEffort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    #[default]
    Max,
}

impl ReasoningEffort {
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    model: Option<Model>,
    model_reasoning_effort: Option<ReasoningEffort>,
    profile: Option<String>,
    sandbox: Option<Sandbox>,
    approval_policy: Option<ApprovalPolicy>,
}

fn default_config_path() -> Option<PathBuf> {
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            env::var_os("HOME")
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .map(|home| home.join(".config"))
        })
        .map(|base| base.join("codex-commit/config.toml"))
}

fn load_config(path: Option<&Path>, explicit: bool) -> Result<(Config, Option<PathBuf>), CliError> {
    let Some(path) = path else {
        return Ok((Config::default(), None));
    };
    let source = match std::fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) if !explicit && error.kind() == io::ErrorKind::NotFound => {
            return Ok((Config::default(), None));
        }
        Err(error) => {
            return Err(CliError::new(
                2,
                format!("could not read config {}: {error}", path.display()),
            ));
        }
    };
    let config = toml::from_str(&source)
        .map_err(|error| CliError::new(2, format!("invalid config {}: {error}", path.display())))?;
    Ok((config, Some(path.to_owned())))
}

fn cli() -> clap::Command {
    use clap::{Arg, ArgAction, ValueHint, value_parser};
    clap::Command::new("codex-commit")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Inspect and commit changes with Codex (default: GPT-6 Luna, Max reasoning).")
        .after_help("Use `codex-commit generate --help` for generation-only JSON output from a supplied public diff.\nOptions precede optional instructions. Use -- before instructions beginning with a dash.\nCompact progress is the default; use --verbose for full command output. Press Ctrl-C to interrupt.\nDefault config: $XDG_CONFIG_HOME/codex-commit/config.toml (fallback: ~/.config/codex-commit/config.toml).\nUnset, empty, or relative XDG_CONFIG_HOME values use the fallback.\nSandbox settings follow your Codex configuration unless overridden.\nUnattended mode cannot prompt for human approval; use --interactive when needed.")
        .args_override_self(true)
        .arg(Arg::new("directory").short('C').value_name("DIR").default_value(".")
            .value_parser(value_parser!(PathBuf)).value_hint(ValueHint::DirPath)
            .allow_hyphen_values(true).help("Run in this Git repository"))
        .arg(Arg::new("push").long("push").action(ArgAction::SetTrue)
            .help("Also push the current branch after committing"))
        .arg(Arg::new("dry-run").long("dry-run").action(ArgAction::SetTrue)
            .help("Print the task and settings without launching Codex"))
        .arg(Arg::new("verbose").long("verbose").action(ArgAction::SetTrue)
            .help("Stream the full Codex terminal transcript"))
        .arg(Arg::new("interactive").long("interactive").action(ArgAction::SetTrue)
            .help("Use Codex's interactive terminal for approvals and follow-up messages"))
        .arg(Arg::new("model").short('m').long("model").value_name("MODEL")
            .value_parser(value_parser!(Model))
            .help("Choose a Codex model (default: gpt-6-luna)"))
        .arg(Arg::new("reasoning-effort").long("reasoning-effort").value_name("LEVEL")
            .value_parser(value_parser!(ReasoningEffort))
            .help("Choose reasoning effort (default: max; support depends on model)"))
        .arg(Arg::new("sandbox").long("sandbox").value_name("MODE")
            .value_parser(value_parser!(Sandbox))
            .help("Override Codex's configured sandbox for this run"))
        .arg(Arg::new("approval").short('a').long("ask-for-approval").value_name("POLICY")
            .value_parser(value_parser!(ApprovalPolicy))
            .help("Override Codex's configured approval policy for this run"))
        .arg(Arg::new("profile").short('p').long("profile").value_name("NAME")
            .help("Select a Codex configuration profile"))
        .arg(Arg::new("config").long("config").value_name("PATH")
            .value_parser(value_parser!(PathBuf)).value_hint(ValueHint::FilePath)
            .conflicts_with("no-config").help("Read this configuration file instead of the global default"))
        .arg(Arg::new("no-config").long("no-config").action(ArgAction::SetTrue)
            .help("Ignore codex-commit configuration files"))
        .arg(Arg::new("completions").long("completions").value_name("SHELL")
            .value_parser(value_parser!(clap_complete::Shell))
            .conflicts_with_all(["directory", "push", "dry-run", "interactive", "verbose", "sandbox", "approval", "profile", "config", "no-config", "model", "reasoning-effort", "instructions"])
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
    pub(crate) fn new(code: i32, message: impl Into<String>) -> Self {
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
    model: Model,
    reasoning_effort: ReasoningEffort,
    directory: PathBuf,
    dry_run: bool,
    interactive: bool,
    verbose: bool,
    push: bool,
    sandbox: Option<Sandbox>,
    approval: Option<ApprovalPolicy>,
    profile: Option<String>,
    config_path: Option<PathBuf>,
    instructions: String,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            model: Model::default(),
            reasoning_effort: ReasoningEffort::default(),
            directory: PathBuf::from("."),
            dry_run: false,
            interactive: false,
            verbose: false,
            push: false,
            sandbox: None,
            approval: None,
            profile: None,
            config_path: None,
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
    let explicit_path = matches.get_one::<PathBuf>("config");
    let default_path = default_config_path();
    let path = if matches.get_flag("no-config") {
        None
    } else {
        explicit_path.or(default_path.as_ref())
    };
    let (config, config_path) = load_config(path.map(PathBuf::as_path), explicit_path.is_some())?;
    let approval = matches
        .get_one::<ApprovalPolicy>("approval")
        .copied()
        .or(config.approval_policy);
    if approval == Some(ApprovalPolicy::OnRequest) && !matches.get_flag("interactive") {
        return Err(CliError::new(
            2,
            "approval policy on-request requires --interactive; unattended Codex cannot prompt for human approval",
        ));
    }
    Ok(Invocation::Task(Options {
        model: matches
            .get_one::<Model>("model")
            .cloned()
            .or(config.model)
            .unwrap_or_default(),
        reasoning_effort: matches
            .get_one::<ReasoningEffort>("reasoning-effort")
            .copied()
            .or(config.model_reasoning_effort)
            .unwrap_or_default(),
        directory: matches
            .get_one::<PathBuf>("directory")
            .expect("default directory")
            .clone(),
        dry_run: matches.get_flag("dry-run"),
        interactive: matches.get_flag("interactive"),
        verbose: matches.get_flag("verbose"),
        push: matches.get_flag("push"),
        sandbox: matches
            .get_one::<Sandbox>("sandbox")
            .copied()
            .or(config.sandbox),
        approval,
        profile: matches
            .get_one::<String>("profile")
            .cloned()
            .or(config.profile),
        config_path,
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

pub(crate) fn command_error(program: &str, error: io::Error) -> CliError {
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
    let model = &options.model.0;
    let reasoning_effort = options.reasoning_effort.as_str();

    if options.dry_run {
        println!(
            "Repository: {}\nBranch: {branch}\nMode: {mode}\nOutput: {}\nPush: {push_status}\nModel: {model}\nReasoning: {reasoning_effort}\nService tier: default\nConfig: {}\nProfile: {}\nSandbox override: {}\nApproval: {}\n\n{prompt}",
            root.display(),
            if options.interactive || options.verbose {
                "direct"
            } else {
                "compact"
            },
            options
                .config_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "none".to_owned()),
            options.profile.as_deref().unwrap_or("Codex default"),
            options
                .sandbox
                .map(Sandbox::as_str)
                .unwrap_or("none (inherited from Codex configuration)"),
            options
                .approval
                .map(ApprovalPolicy::as_str)
                .unwrap_or(if options.interactive {
                    "inherited from Codex configuration"
                } else {
                    "Codex exec (no human approval prompts)"
                }),
        );
        return Ok(());
    }

    // Replacing this process preserves the terminal, streams, PID, signals, and
    // Codex's own exit status instead of introducing another process supervisor.
    let mut command = Command::new("codex");
    if let Some(profile) = &options.profile {
        command.arg("--profile").arg(profile);
    }
    if let Some(sandbox) = &options.sandbox {
        command.arg("--sandbox").arg(sandbox.as_str());
    }
    if let Some(approval) = &options.approval {
        command
            .arg("-c")
            .arg(format!("approval_policy=\"{}\"", approval.as_str()));
    }
    if options.interactive {
        command.arg("--no-alt-screen");
    } else {
        command.arg("exec");
        if !options.verbose {
            command.arg("--json");
        }
    }
    command
        .arg("--model")
        .arg(model)
        .arg("-c")
        .arg(format!("model_reasoning_effort=\"{reasoning_effort}\""))
        .args(["-c", "service_tier=\"default\"", "-C"])
        .arg(&root)
        .arg("--")
        .arg(OsStr::new(&prompt))
        .current_dir(&root);
    eprintln!(
        "codex-commit: {mode} on {branch}; Push: {push_status} ({model}, {reasoning_effort} reasoning; Ctrl-C to interrupt)"
    );
    io::stdout()
        .flush()
        .map_err(|error| CliError::new(1, format!("could not flush output: {error}")))?;
    if options.interactive || options.verbose {
        Err(command_error("codex", command.exec()))
    } else {
        crate::process::run(command)
    }
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
