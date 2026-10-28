#[cfg(not(unix))]
compile_error!("codex-commit currently supports macOS and Linux (Unix) only");

#[cfg(unix)]
mod cli;
#[cfg(unix)]
mod generate_cli;
#[cfg(unix)]
mod output;
#[cfg(unix)]
mod process;

#[cfg(unix)]
fn main() {
    let mut args = std::env::args_os().skip(1).peekable();
    let result = if args.peek().is_some_and(|arg| arg == "generate") {
        args.next();
        generate_cli::run(args)
    } else {
        cli::run(args)
    };
    if let Err(error) = result {
        use std::io::Write;
        let _ = writeln!(std::io::stderr(), "codex-commit: {}", error.message);
        std::process::exit(error.code);
    }
}
