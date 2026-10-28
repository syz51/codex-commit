//! The explicit, repository-independent generation entry point.
use crate::cli::CliError;
use clap::{Arg, Command, value_parser};
use codex_commit::{GenerationOptions, GenerationRequest, MAX_DIFF_BYTES, ReasoningEffort};
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

fn cli() -> Command {
    Command::new("codex-commit generate")
        .about("Generate a commit message as JSON from a public diff on stdin; never access Git.")
        .after_help("Supply authentication paths and model explicitly. User configuration and instructions are ignored.\nOnly subject/body JSON is written to stdout. Empty, oversized or invalid input fails without a fallback.")
        .arg(Arg::new("codex-path").long("codex-path").required(true)
            .value_parser(value_parser!(PathBuf)).value_name("PATH")
            .help("Codex CLI executable"))
        .arg(Arg::new("codex-home").long("codex-home").required(true)
            .value_parser(value_parser!(PathBuf)).value_name("PATH")
            .help("Codex authentication home (authentication only)"))
        .arg(Arg::new("model").long("model").required(true).value_name("MODEL"))
        .arg(Arg::new("reasoning-effort").long("reasoning-effort")
            .value_parser(["low", "medium", "high", "xhigh"]).default_value("high"))
        .arg(Arg::new("timeout-secs").long("timeout-secs")
            .value_parser(value_parser!(u64).range(1..=3600)).default_value("120"))
}

pub(crate) fn run(args: impl IntoIterator<Item = OsString>) -> Result<(), CliError> {
    let matches = match cli()
        .try_get_matches_from(std::iter::once(OsString::from("codex-commit generate")).chain(args))
    {
        Ok(matches) => matches,
        Err(error) if error.kind() == clap::error::ErrorKind::DisplayHelp => {
            print!("{error}");
            return Ok(());
        }
        Err(error) => return Err(CliError::new(error.exit_code(), error.to_string())),
    };
    let mut bytes = Vec::new();
    io::stdin()
        .take(MAX_DIFF_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| CliError::new(1, format!("could not read public diff: {error}")))?;
    if bytes.len() > MAX_DIFF_BYTES {
        return Err(CliError::new(
            2,
            format!("public diff exceeds {MAX_DIFF_BYTES} bytes"),
        ));
    }
    let diff = String::from_utf8(bytes)
        .map_err(|_| CliError::new(2, "public diff must be valid UTF-8"))?;
    let mut options = GenerationOptions::new(
        matches
            .get_one::<PathBuf>("codex-path")
            .expect("required path")
            .clone(),
        matches
            .get_one::<PathBuf>("codex-home")
            .expect("required home")
            .clone(),
        matches
            .get_one::<String>("model")
            .expect("required model")
            .clone(),
    );
    options.effort = match matches
        .get_one::<String>("reasoning-effort")
        .map(String::as_str)
    {
        Some("low") => ReasoningEffort::Low,
        Some("medium") => ReasoningEffort::Medium,
        Some("xhigh") => ReasoningEffort::XHigh,
        _ => ReasoningEffort::High,
    };
    options.timeout = Duration::from_secs(
        *matches
            .get_one::<u64>("timeout-secs")
            .expect("default timeout"),
    );
    let cancellation = Arc::new(AtomicBool::new(false));
    let mut handlers = Vec::new();
    for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        match signal_hook::flag::register(signal, Arc::clone(&cancellation)) {
            Ok(handler) => handlers.push(handler),
            Err(error) => {
                for handler in handlers {
                    signal_hook::low_level::unregister(handler);
                }
                return Err(CliError::new(
                    1,
                    format!("could not register cancellation: {error}"),
                ));
            }
        }
    }
    options.cancellation = Some(cancellation);
    let result = codex_commit::generate_message(GenerationRequest { diff: &diff }, &options);
    for handler in handlers {
        signal_hook::low_level::unregister(handler);
    }
    let message = result.map_err(|error| CliError::new(1, error.to_string()))?;
    let output = serde_json::to_vec(&message)
        .map_err(|error| CliError::new(1, format!("could not encode commit message: {error}")))?;
    let mut stdout = io::stdout().lock();
    stdout
        .write_all(&output)
        .and_then(|()| stdout.write_all(b"\n"))
        .map_err(|error| CliError::new(1, format!("could not write commit message: {error}")))
}
