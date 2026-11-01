//! A native test fixture. Cargo does not install examples with the application.

use nix::libc;
use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, sigaction};
use serde_json::json;
use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::os::unix::process::CommandExt;
use std::process::{self, Command};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::Duration;

static CHILD: AtomicI32 = AtomicI32::new(0);
static RESUMED: AtomicBool = AtomicBool::new(false);

extern "C" fn resumed(_: i32) {
    RESUMED.store(true, Ordering::Release);
}

fn enter_raw_mode() -> Result<(), Box<dyn std::error::Error>> {
    let stdin = io::stdin();
    let mut attributes = nix::sys::termios::tcgetattr(&stdin)?;
    nix::sys::termios::cfmakeraw(&mut attributes);
    attributes
        .local_flags
        .insert(nix::sys::termios::LocalFlags::ISIG);
    nix::sys::termios::tcsetattr(&stdin, nix::sys::termios::SetArg::TCSANOW, &attributes)?;
    Ok(())
}

extern "C" fn resized(_: i32) {
    unsafe {
        libc::write(
            libc::STDERR_FILENO,
            b"resize received\n".as_ptr().cast(),
            16,
        );
    }
}

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
    if env::var_os("FAKE_CODEX_TRACK_RESIZE").is_some() {
        let action = SigAction::new(
            SigHandler::Handler(resized),
            SaFlags::empty(),
            SigSet::empty(),
        );
        unsafe {
            sigaction(Signal::SIGWINCH, &action)?;
        }
    }

    if env::var_os("FAKE_CODEX_IGNORE_CANCELLATION").is_some() {
        let ignore = SigAction::new(SigHandler::SigIgn, SaFlags::empty(), SigSet::empty());
        // Deliberately model an agent that cannot gracefully shut down.
        for signal in [
            Signal::SIGINT,
            Signal::SIGTERM,
            Signal::SIGHUP,
            Signal::SIGQUIT,
        ] {
            unsafe {
                sigaction(signal, &ignore)?;
            }
        }
    }

    let args: Vec<_> = env::args_os().skip(1).collect();
    let argument_bytes: Vec<_> = args.iter().map(|arg| arg.as_bytes()).collect();
    let temporary = env::var_os("TMPDIR");
    let uv_cache = env::var_os("UV_CACHE_DIR");
    let scratch = temporary.as_ref().filter(|root| {
        uv_cache.as_ref().is_some_and(|cache| {
            std::path::Path::new(cache) == std::path::Path::new(root).join("uv-cache")
        })
    });
    let scratch_mode = scratch
        .map(|root| fs::metadata(root).map(|metadata| metadata.permissions().mode() & 0o777))
        .transpose()?;
    if let Some(root) = scratch {
        let root = std::path::Path::new(root);
        fs::create_dir_all(root.join("nested/check-files"))?;
        fs::write(
            root.join("nested/check-files/payload"),
            "task temporary data",
        )?;
        fs::create_dir_all(root.join("uv-cache/nested"))?;
        fs::write(root.join("uv-cache/nested/payload"), "task cache data")?;
        if let Some(neighbor) = env::var_os("FAKE_CODEX_SCRATCH_NEIGHBOR") {
            symlink(neighbor, root.join("unrelated-symlink"))?;
        }
    }
    let mode = env::var("FAKE_CODEX_MODE").unwrap_or_default();
    if mode == "scratch-shell" {
        let status = Command::new("/bin/sh")
            .args([
                "-eu",
                "-c",
                r#"[ "$UV_CACHE_DIR" = "$TMPDIR/uv-cache" ]
[ "$PYTHONDONTWRITEBYTECODE" = 1 ]
printf cache > "$UV_CACHE_DIR/shell-cache"
printf temporary > "$TMPDIR/shell-temporary"
"#,
            ])
            .status()?;
        if !status.success() {
            process::exit(status.code().unwrap_or(1));
        }
    }
    fs::write(
        env::var_os("FAKE_CODEX_LOG").expect("fixture log path"),
        serde_json::to_vec(&json!({
            "argv": argument_bytes,
            "cwd": env::current_dir()?.as_os_str().as_bytes(),
            "pid": process::id(),
            "supervisor": unsafe { libc::getppid() },
            "caller_group": unsafe { libc::getpgid(libc::getppid()) },
            "pgid": unsafe { libc::getpgrp() },
            "foreground_group": unsafe { libc::tcgetpgrp(libc::STDIN_FILENO) },
            "scratch": scratch.map(|root| root.as_bytes()),
            "scratch_mode": scratch_mode,
            "environment": {
                "TMPDIR": temporary.as_ref().map(|value| value.as_bytes()),
                "UV_CACHE_DIR": uv_cache.as_ref().map(|value| value.as_bytes()),
                "PYTHONDONTWRITEBYTECODE": env::var_os("PYTHONDONTWRITEBYTECODE").map(|value| value.as_bytes().to_owned()),
            },
        }))?,
    )?;
    if env::var_os("FAKE_CODEX_REPLACE_SCRATCH").is_some() {
        let root = scratch.expect("replacement requires managed scratch");
        fs::remove_dir_all(root)?;
        fs::write(root, "replacement must remain on cleanup failure")?;
    }

    if json_mode() {
        println!(
            "{}",
            json!({"type":"thread.started","thread_id":"mock-session"})
        );
        println!("{}", json!({"type":"turn.started"}));
    }
    if env::var_os("FAKE_CODEX_IDLE_WITHOUT_MESSAGE").is_some() {
        io::stdout().flush()?;
        loop {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    match mode.as_str() {
        "peer-raw" => {
            let terminal = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open("/dev/tty")?;
            let mut attributes = nix::sys::termios::tcgetattr(&terminal)?;
            nix::sys::termios::cfmakeraw(&mut attributes);
            attributes
                .local_flags
                .insert(nix::sys::termios::LocalFlags::ISIG);
            nix::sys::termios::tcsetattr(
                &terminal,
                nix::sys::termios::SetArg::TCSANOW,
                &attributes,
            )?;
            println!("live peer raw ready");
            io::stdout().flush()?;
            let gate = env::var_os("FAKE_CODEX_INPUT_GATE").expect("peer input gate");
            while !std::path::Path::new(&gate).is_file() {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        "tty-read" | "tty-native-read" | "background-read" => {
            if mode == "background-read" {
                println!("background job running");
                io::stdout().flush()?;
                let gate = env::var_os("FAKE_CODEX_INPUT_GATE").expect("background input gate");
                while !std::path::Path::new(&gate).is_file() {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            if mode != "tty-native-read"
                && unsafe { libc::tcgetpgrp(libc::STDIN_FILENO) != libc::getpgrp() }
            {
                return Err("mock must own terminal foreground before reading".into());
            }
            println!("terminal input ready");
            io::stdout().flush()?;
            loop {
                let mut input = String::new();
                if io::stdin().read_line(&mut input)? == 0 {
                    break;
                }
                println!("terminal input: {}", input.trim_end());
                io::stdout().flush()?;
            }
        }
        "signal-int" | "signal-pipe" => {
            let default = SigAction::new(SigHandler::SigDfl, SaFlags::empty(), SigSet::empty());
            let signal = if mode == "signal-int" {
                Signal::SIGINT
            } else {
                Signal::SIGPIPE
            };
            unsafe {
                sigaction(signal, &default)?;
                libc::kill(libc::getpid(), signal as i32);
            }
            return Err("signal should terminate mock".into());
        }
        "stream" | "wait" | "wait-child" | "raw-wait" => {
            if mode == "raw-wait" {
                enter_raw_mode()?;
                let action = SigAction::new(
                    SigHandler::Handler(resumed),
                    SaFlags::empty(),
                    SigSet::empty(),
                );
                unsafe {
                    sigaction(Signal::SIGCONT, &action)?;
                }
            }
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
                    if mode == "raw-wait" && RESUMED.swap(false, Ordering::AcqRel) {
                        enter_raw_mode()?;
                        println!("raw mode resumed");
                        io::stdout().flush()?;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
        "comparison" => {
            // Exercise the documented command-boundary policy with real shell
            // statuses. This fixture does not demonstrate model compliance.
            let output = Command::new("/bin/bash")
                .args([
                    "-c",
                    r#"
set -euo pipefail
task_scratch=$(mktemp -d "${TMPDIR:-/tmp}/codex-commit.XXXXXX")
trap 'result=$?; trap - EXIT; rm -r -- "$task_scratch"; exit "$result"' EXIT
printf '%s\n' "$task_scratch" >> "$FAKE_CODEX_SCRATCH_LOG"
printf 'original\n' > "$task_scratch/left"
printf 'original\n' > "$task_scratch/right"
case "$FAKE_CODEX_COMPARISON" in
  different) printf 'changed\n' > "$task_scratch/right" ;;
  missing) rm -- "$task_scratch/right" ;;
  validation) false ;;
  pipeline) false | cat ;;
esac
if diff -u "$task_scratch/left" "$task_scratch/right"; then
  :
else
  comparison_status=$?
  if [ "$comparison_status" -ne 1 ]; then exit "$comparison_status"; fi
fi
"#,
                ])
                .output()?;
            println!(
                "{}",
                json!({"type":"item.completed","item":{"type":"command_execution","command":"fixture comparison with owned scratch cleanup","status":if output.status.success() {"completed"} else {"failed"},"exit_code":output.status.code(),"aggregated_output":format!("{}{}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr))}})
            );
            if !output.status.success() {
                process::exit(output.status.code().unwrap_or(1));
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
