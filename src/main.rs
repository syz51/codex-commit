#[cfg(not(unix))]
compile_error!("codex-commit currently supports macOS and Linux (Unix) only");

#[cfg(unix)]
mod cli;
#[cfg(unix)]
mod output;
#[cfg(unix)]
mod process;

#[cfg(unix)]
fn main() {
    if let Err(error) = cli::run(std::env::args_os().skip(1)) {
        use std::io::Write;
        let _ = writeln!(std::io::stderr(), "codex-commit: {}", error.message);
        std::process::exit(error.code);
    }
}
