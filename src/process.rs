//! Supervision for compact mode; direct modes still replace the process.
use crate::cli::{CliError, command_error};
use crate::output::Renderer;
use nix::fcntl::{FcntlArg, OFlag, fcntl};
use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, kill, killpg, sigaction};
use nix::unistd::{Pid, getpid};
use signal_hook::consts::{SIGCONT, SIGHUP, SIGINT, SIGQUIT, SIGTERM, SIGTSTP};
use signal_hook::iterator::Signals;
use std::io::{self, IsTerminal, Read, Write};
use std::os::fd::AsFd;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

const MAX_EVENT: u64 = 16 * 1024 * 1024;

pub(crate) fn run(mut command: Command) -> Result<(), CliError> {
    let mut signals = Signals::new([SIGINT, SIGTERM, SIGHUP, SIGQUIT, SIGTSTP, SIGCONT])
        .map_err(|e| CliError::new(1, format!("could not install signal handlers: {e}")))?;
    let handle = signals.handle();
    if io::stdin().is_terminal() {
        command.stdin(Stdio::null());
    }
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|e| command_error("codex", e))?;
    let group = Pid::from_raw(child.id() as i32);
    let target = Arc::new(Mutex::new(Some(group)));
    let forwarding = Arc::clone(&target);
    let cancellation = Arc::new(AtomicI32::new(0));
    let forwarded_cancellation = Arc::clone(&cancellation);
    let signal_thread = thread::spawn(move || {
        for signal in signals.forever() {
            let target = forwarding.lock().expect("signal target lock");
            if let Some(group) = *target {
                if let Ok(signal) = Signal::try_from(signal) {
                    if matches!(
                        signal,
                        Signal::SIGINT | Signal::SIGTERM | Signal::SIGHUP | Signal::SIGQUIT
                    ) {
                        forwarded_cancellation.store(signal as i32, Ordering::Release);
                    }
                    let _ = killpg(group, signal);
                    if signal == Signal::SIGTSTP {
                        // Stop the supervisor only after its owned group has
                        // received suspension. SIGCONT will resume both.
                        let _ = kill(getpid(), Signal::SIGSTOP);
                    }
                }
            }
        }
    });
    let shutdown = Arc::new(AtomicBool::new(false));
    let (diagnostic_send, diagnostic_receive) = std::sync::mpsc::channel();
    let stderr = child.stderr.take().expect("piped stderr");
    let stderr_shutdown = Arc::clone(&shutdown);
    let stderr_thread = thread::spawn(move || -> io::Result<()> {
        let result = read_lines(stderr, &stderr_shutdown, |line| {
            let text = String::from_utf8_lossy(line);
            if !matches!(
                text.trim_end_matches(['\r', '\n']),
                "hook: PreToolUse"
                    | "hook: PreToolUse Completed"
                    | "hook: PostToolUse"
                    | "hook: PostToolUse Completed"
            ) {
                io::stderr().write_all(line)?;
            }
            Ok(())
        });
        if let Err(ref error) = result {
            let _ = diagnostic_send.send(error.to_string());
        }
        result
    });
    let stdout = child.stdout.take().expect("piped stdout");
    let stdout_shutdown = Arc::clone(&shutdown);
    let (send, receive) = std::sync::mpsc::sync_channel(8);
    let stdout_thread = thread::spawn(move || {
        let result = read_lines(stdout, &stdout_shutdown, |line| {
            let line = std::str::from_utf8(line)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            send.send(Ok(line.to_owned()))
                .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "renderer closed"))
        });
        if let Err(error) = result {
            let _ = send.send(Err(error));
        }
    });
    // Reap independently of presentation: terminal/pipe backpressure must never
    // prevent cancellation or leave the agent running after output fails.
    let reaper_target = Arc::clone(&target);
    let reaper_shutdown = Arc::clone(&shutdown);
    let reaper_cancellation = Arc::clone(&cancellation);
    let reaper = thread::spawn(move || -> io::Result<std::process::ExitStatus> {
        loop {
            {
                let mut target = reaper_target.lock().expect("signal target lock");
                if let Some(status) = child.try_wait()? {
                    let _ = killpg(group, Signal::SIGKILL);
                    *target = None;
                    reaper_shutdown.store(true, Ordering::Release);
                    restore_signals()?;
                    let cancelled = reaper_cancellation.load(Ordering::Acquire);
                    if cancelled != 0 {
                        // The owned agent has been reaped. Honor cancellation
                        // even when presentation is blocked on an unread pipe.
                        if let Ok(signal) = Signal::try_from(cancelled) {
                            let _ = kill(getpid(), signal);
                        }
                    }
                    return Ok(status);
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
    });
    let mut renderer = Renderer::new(io::stdout(), io::stderr());
    let result = (|| -> io::Result<()> {
        loop {
            if let Ok(error) = diagnostic_receive.try_recv() {
                return Err(io::Error::other(error));
            }
            match receive.recv_timeout(Duration::from_millis(25)) {
                Ok(line) => renderer.event(&line?)?,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
        Ok(())
    })();
    drop(receive);
    if result.is_err() {
        shutdown.store(true, Ordering::Release);
        if cancellation.load(Ordering::Acquire) == 0 {
            let target = target.lock().expect("signal target lock");
            if let Some(group) = *target {
                let _ = killpg(group, Signal::SIGKILL);
            }
        }
    }
    let status = reaper
        .join()
        .map_err(|_| CliError::new(1, "Codex reaper panicked"))?
        .map_err(|e| CliError::new(1, format!("could not reap Codex: {e}")))?;
    handle.close();
    let signal_result = signal_thread.join();
    // A descendant retaining stderr must not keep the wrapper alive after Codex exits.
    stdout_thread
        .join()
        .map_err(|_| CliError::new(1, "stdout reader panicked"))?;
    let stderr_result = stderr_thread
        .join()
        .map_err(|_| CliError::new(1, "stderr reader panicked"))?;
    signal_result.map_err(|_| CliError::new(1, "signal forwarding thread panicked"))?;
    let code = status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(1));
    if cancellation.load(Ordering::Acquire) != 0 && code != 0 {
        let _ = renderer.finish(false);
        return Err(CliError::new(
            code,
            format!("Codex exited with status {code}"),
        ));
    }
    result.map_err(|e| {
        CliError::new(
            1,
            format!("could not render Codex output: {e}; use --verbose for direct output"),
        )
    })?;
    stderr_result
        .map_err(|e| CliError::new(1, format!("could not read Codex diagnostics: {e}")))?;
    let rendered = renderer.finish(code == 0);
    if code != 0 {
        return Err(CliError::new(
            code,
            format!("Codex exited with status {code}"),
        ));
    }
    match rendered {
        Ok(true) => Ok(()),
        Ok(false) => Err(CliError::new(
            1,
            "Codex event stream did not complete successfully",
        )),
        Err(e) => Err(CliError::new(
            1,
            format!("could not render Codex output: {e}"),
        )),
    }
}

/// Pipes may be inherited by detached processes outside the owned process group.
/// After Codex exits, drain available data without waiting for those processes.
fn read_lines(
    mut reader: impl Read + AsFd,
    shutdown: &AtomicBool,
    mut emit: impl FnMut(&[u8]) -> io::Result<()>,
) -> io::Result<()> {
    let flags = OFlag::from_bits_truncate(fcntl(reader.as_fd(), FcntlArg::F_GETFL)?);
    fcntl(reader.as_fd(), FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK))?;
    let mut buffer = [0; 8192];
    let mut pending = Vec::new();
    let mut draining = false;
    let mut drained = 0;
    loop {
        if shutdown.load(Ordering::Acquire) {
            draining = true;
            if drained > MAX_EVENT {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Codex pipe exceeded post-exit drain limit",
                ));
            }
        }
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(length) => {
                if draining {
                    drained += length as u64;
                }
                for part in buffer[..length].split_inclusive(|byte| *byte == b'\n') {
                    if pending.len() + part.len() > MAX_EVENT as usize {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Codex output line exceeds 16 MiB",
                        ));
                    }
                    pending.extend_from_slice(part);
                    if part.last() == Some(&b'\n') {
                        emit(&pending)?;
                        pending.clear();
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if shutdown.load(Ordering::Acquire) {
                    break;
                }
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => return Err(error),
        }
    }
    if !pending.is_empty() {
        emit(&pending)?;
    }
    Ok(())
}

fn restore_signals() -> io::Result<()> {
    // No owned child remains. Restore cancellation before any further output
    // or cleanup can block; signal-hook otherwise leaves no-op dispositions.
    let default = SigAction::new(SigHandler::SigDfl, SaFlags::empty(), SigSet::empty());
    for signal in [
        Signal::SIGINT,
        Signal::SIGTERM,
        Signal::SIGHUP,
        Signal::SIGQUIT,
        Signal::SIGTSTP,
        Signal::SIGCONT,
    ] {
        // SIG_DFL installs no Rust handler or borrowed/shared data.
        unsafe { sigaction(signal, &default) }?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    #[test]
    fn nonblocking_reader_preserves_fragmented_lines_and_final_partial() {
        let (reader, mut writer) = UnixStream::pair().unwrap();
        let thread = thread::spawn(move || {
            writer.write_all(b"{\"type\":").unwrap();
            thread::sleep(Duration::from_millis(25));
            writer.write_all(b"\"turn.completed\"}\npartial").unwrap();
        });
        let mut lines = Vec::new();
        read_lines(reader, &AtomicBool::new(false), |line| {
            lines.push(line.to_vec());
            Ok(())
        })
        .unwrap();
        thread.join().unwrap();
        assert_eq!(
            lines,
            [
                b"{\"type\":\"turn.completed\"}\n".to_vec(),
                b"partial".to_vec()
            ]
        );
    }

    #[test]
    fn shutdown_drains_available_data_without_waiting_for_open_writer() {
        let (reader, mut writer) = UnixStream::pair().unwrap();
        writer.write_all(b"complete\npartial").unwrap();
        let mut lines = Vec::new();
        read_lines(reader, &AtomicBool::new(true), |line| {
            lines.push(line.to_vec());
            Ok(())
        })
        .unwrap();
        assert_eq!(lines, [b"complete\n".to_vec(), b"partial".to_vec()]);
        drop(writer);
    }
}
