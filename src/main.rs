#[cfg(not(unix))]
compile_error!("codex-commit currently supports macOS and Linux (Unix) only");

#[cfg(unix)]
mod cli;
#[cfg(unix)]
mod finalize;
#[cfg(unix)]
mod generate_cli;
#[cfg(unix)]
mod inspection;
#[cfg(unix)]
mod output;
#[cfg(unix)]
mod process;
#[cfg(unix)]
mod scratch;

#[cfg(unix)]
fn main() {
    let mut args = std::env::args_os().skip(1).peekable();
    let result = if args.peek().is_some_and(|arg| arg == "finalize") {
        args.next();
        finalize::run(args)
    } else if args.peek().is_some_and(|arg| arg == "generate") {
        args.next();
        generate_cli::run(args).map(|()| 0)
    } else {
        cli::run(args)
    };
    match result {
        Ok(0) => {}
        Ok(code) => std::process::exit(code),
        Err(error) => {
            use std::io::Write;
            let _ = writeln!(std::io::stderr(), "codex-commit: {}", error.message);
            std::process::exit(error.code);
        }
    }
}
