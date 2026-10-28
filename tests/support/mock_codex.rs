//! A native test fixture. Cargo does not install examples with the application.

use nix::libc;
use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, sigaction};
use serde_json::json;
use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
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

fn json_mode() -> bool {
    env::args_os().any(|arg| arg == "--json")
}

fn message(text: &str) {
    if json_mode() {
        println!(
            "{}",
            json!({"type":"item.completed","item":{"type":"agent_message","text":text}})
        );
    } else {
        println!("{text}");
    }
}

fn checked_git(args: &[&str]) {
    if json_mode() {
        let output = Command::new("git")
            .args(args)
            .output()
            .expect("fixture Git");
        println!(
            "{}",
            json!({"type":"item.completed","item":{"type":"command_execution","command":format!("git {}", args.join(" ")),"status":if output.status.success() {"completed"} else {"failed"},"exit_code":output.status.code(),"aggregated_output":format!("{}{}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr))}})
        );
        if !output.status.success() {
            process::exit(output.status.code().unwrap_or(1));
        }
        return;
    }
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

    if json_mode() {
        println!(
            "{}",
            json!({"type":"thread.started","thread_id":"mock-session"})
        );
        println!("{}", json!({"type":"turn.started"}));
    }
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
                message("child ready");
            }
            eprintln!("live progress");
            message("live output");
            if env::var_os("FAKE_CODEX_LARGE_PROGRESS").is_some() {
                message(&format!("BLOCKED_PROGRESS{}", "x".repeat(200000)));
            }
            if json_mode() {
                println!(
                    "{}",
                    json!({"type":"item.started","item":{"type":"command_execution","command":"wait","status":"in_progress"}})
                );
            }
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
        "large-final" => {
            message(&"final output".repeat(20000));
            println!("{}", json!({"type":"turn.completed"}));
            io::stdout().flush()?;
            return Ok(());
        }
        "partial-wait" => {
            print!("{{\"type\":\"item.completed\",\"item\":");
            io::stdout().flush()?;
            eprintln!("partial ready");
            loop {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        "escaped" => {
            let mut command = Command::new("/bin/sleep");
            command.arg("60");
            // Test-only owned descendant deliberately escapes the supervised group.
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            let child = command.spawn()?;
            fs::write(
                env::var_os("FAKE_CODEX_CHILD_LOG").expect("child log"),
                child.id().to_string(),
            )?;
            drop(child);
        }
        "stdin" => {
            let mut text = String::new();
            io::stdin().read_to_string(&mut text)?;
            message(&text);
        }
        "large" => {
            eprintln!("{}", "x".repeat(131072));
            println!(
                "{}",
                json!({"type":"item.completed","item":{"type":"command_execution","command":"large diff","exit_code":0,"status":"completed","aggregated_output":"large successful diff".repeat(131072)}})
            );
        }
        "malformed" => {
            println!("not-json");
            return Ok(());
        }
        "truncated" => {
            return Ok(());
        }
        "diagnostic" => {
            eprintln!("hook: PreToolUse");
            eprintln!("hook: PostToolUse Completed");
            eprintln!("warning: keep this warning");
            println!(
                "{}",
                json!({"type":"item.completed","item":{"type":"command_execution","command":"git push","exit_code":128,"status":"failed","aggregated_output":"fatal: exact push error"}})
            );
        }
        _ => {}
    }
    message("finished");
    if json_mode() {
        println!("{}", json!({"type":"turn.completed"}));
    }
    io::stdout().flush()?;
    process::exit(
        env::var("FAKE_CODEX_EXIT")
            .unwrap_or_default()
            .parse()
            .unwrap_or(0),
    );
}
