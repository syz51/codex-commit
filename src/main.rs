#[cfg(not(unix))]
compile_error!("codex-commit currently supports macOS and Linux (Unix) only");

#[cfg(unix)]
mod cli;

#[cfg(unix)]
fn main() {
    if let Err(error) = cli::run(std::env::args_os().skip(1)) {
        eprintln!("codex-commit: {}", error.message);
        std::process::exit(error.code);
    }
}
