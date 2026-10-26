//! A native test fixture. Cargo does not install examples with the application.

use nix::libc;
use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, sigaction};
use serde_json::json;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::process::{self, Command};
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;

static CHILD: AtomicI32 = AtomicI32::new(0);

extern "C" fn cancelled(signal: i32) {
    // Only async-signal-safe syscalls and a lock-free integer atomic are used.
    // The fixture owns this child; forwarding/reaping models Codex cleanup.
    unsafe {
        libc::write(libc::STDERR_FILENO, b"cancelled\n".as_ptr().cast(), 10);
        let child = CHILD.swap(0, Ordering::SeqCst);
        if child > 0 {
            libc::kill(child, signal);
            while libc::waitpid(child, std::ptr::null_mut(), 0) < 0 {
                // A different signal can interrupt waitpid. No other thread
                // reaps the child, so retrying EINTR is safe.
                if io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
                    break;
                }
            }
        }
        libc::_exit(128 + signal);
    }
}

fn checked_git(args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .status()
        .expect("run fixture Git");
    if !status.success() {
        process::exit(status.code().unwrap_or(1));
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let action = SigAction::new(
        SigHandler::Handler(cancelled),
        SaFlags::empty(),
        SigSet::empty(),
    );
    // The handler is restricted to signal-safe operations above.
    unsafe {
        sigaction(Signal::SIGINT, &action)?;
        sigaction(Signal::SIGTERM, &action)?;
    }

    let args: Vec<_> = env::args_os().skip(1).collect();
    let argument_bytes: Vec<_> = args.iter().map(|arg| arg.as_bytes()).collect();
    fs::write(
        env::var_os("FAKE_CODEX_LOG").expect("fixture log path"),
        serde_json::to_vec(&json!({
            "argv": argument_bytes,
            "cwd": env::current_dir()?.as_os_str().as_bytes(),
            "pid": process::id(),
        }))?,
    )?;

    let mode = env::var("FAKE_CODEX_MODE").unwrap_or_default();
    match mode.as_str() {
        "stream" | "wait" | "wait-child" => {
            if mode == "wait-child" {
                let child = Command::new("/bin/sleep").arg("60").spawn()?;
                let pid = i32::try_from(child.id())?;
                CHILD.store(pid, Ordering::SeqCst);
                // The signal handler owns reaping after this point. Dropping a
                // Child handle neither kills nor waits for the process.
                drop(child);
                let pgid = nix::unistd::getpgid(Some(nix::unistd::Pid::from_raw(pid)))?;
                fs::write(
                    env::var_os("FAKE_CODEX_CHILD_LOG").expect("fixture child log path"),
                    serde_json::to_vec(&json!({"pid": pid, "pgid": pgid.as_raw()}))?,
                )?;
                println!("child ready");
            }
            eprintln!("live progress");
            println!("live output");
            io::stdout().flush()?;
            if mode == "stream" {
                std::thread::sleep(Duration::from_millis(800));
            } else {
                loop {
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
        "git" => {
            checked_git(&["add", "--all"]);
            match Command::new("git")
                .args(["diff", "--cached", "--quiet"])
                .status()?
                .code()
            {
                Some(0) => {}
                Some(1) => checked_git(&["commit", "-m", "Commit fixture change"]),
                Some(code) => process::exit(code),
                None => process::exit(1),
            }
            // This mock follows the prompt policy. It does not demonstrate
            // model compliance or provide a security boundary.
            let prompt = args.last().and_then(|arg| arg.to_str()).unwrap_or("");
            let policy = prompt.rsplit("Push policy: ").next().unwrap_or("");
            match policy.lines().next() {
                Some("enabled.") => checked_git(&["push", "--set-upstream", "origin", "HEAD"]),
                Some("disabled.") => {}
                _ => return Err("missing final push policy".into()),
            }
        }
        _ => {}
    }
    println!("finished");
    io::stdout().flush()?;
    process::exit(
        env::var("FAKE_CODEX_EXIT")
            .unwrap_or_default()
            .parse()
            .unwrap_or(0),
    );
}
