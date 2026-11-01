//! Shared process supervision with compact rendering or inherited terminal streams.
use crate::cli::{CliError, command_error};
use crate::output::Renderer;
use crate::scratch::TaskScratch;
use nix::fcntl::{FcntlArg, OFlag, fcntl};
use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, kill, killpg, sigaction};
use nix::unistd::{Pid, getpid};
use signal_hook::consts::{SIGCONT, SIGHUP, SIGINT, SIGQUIT, SIGTERM, SIGTSTP, SIGWINCH};
use signal_hook::iterator::SignalsInfo;
use signal_hook::iterator::exfiltrator::WithOrigin;
use signal_hook::low_level::siginfo::Cause;
use std::io::{self, IsTerminal, Read, Write};
use std::os::fd::AsFd;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const CANCELLATION_GRACE: Duration = Duration::from_millis(250);

const MAX_EVENT: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum OutputMode {
    Compact,
    Direct,
}

pub(crate) struct PreparedSignals {
    events: Option<SignalsInfo<WithOrigin>>,
    original: Vec<(i32, nix::libc::sigaction)>,
    startup_active: Arc<AtomicBool>,
    early_cancellation: Arc<AtomicI32>,
    startup_handlers: Vec<signal_hook::SigId>,
}

impl PreparedSignals {
    fn finish_startup(&mut self) -> i32 {
        self.startup_active.store(false, Ordering::Release);
        for handler in self.startup_handlers.drain(..) {
            signal_hook::low_level::unregister(handler);
        }
        self.early_cancellation.load(Ordering::Acquire)
    }
}

impl Drop for PreparedSignals {
    fn drop(&mut self) {
        self.finish_startup();
        self.events.take();
        if let Err(error) = restore_signals(&self.original) {
            report_cancelled_cleanup(&CliError::new(
                1,
                format!("could not restore signal dispositions: {error}"),
            ));
        }
    }
}

pub(crate) fn prepare_signals() -> Result<PreparedSignals, CliError> {
    let mut original = Vec::new();
    let mut active = Vec::new();
    for signal in [SIGINT, SIGTERM, SIGHUP, SIGQUIT, SIGTSTP, SIGCONT, SIGWINCH] {
        let mut action: nix::libc::sigaction = unsafe { std::mem::zeroed() };
        if unsafe { nix::libc::sigaction(signal, std::ptr::null(), &mut action) } < 0 {
            return Err(CliError::new(
                1,
                format!(
                    "could not inspect signal disposition: {}",
                    io::Error::last_os_error()
                ),
            ));
        }
        // Replacing SIG_IGN with a caught handler would make exec reset it to
        // default in Codex, breaking inherited nohup/background semantics.
        if action.sa_sigaction != nix::libc::SIG_IGN {
            active.push(signal);
        }
        original.push((signal, action));
    }
    let events = SignalsInfo::<WithOrigin>::new(&active)
        .map_err(|error| CliError::new(1, format!("could not install signal handlers: {error}")))?;
    let mut prepared = PreparedSignals {
        events: Some(events),
        original,
        startup_active: Arc::new(AtomicBool::new(true)),
        early_cancellation: Arc::new(AtomicI32::new(0)),
        startup_handlers: Vec::new(),
    };
    for signal in active
        .into_iter()
        .filter(|signal| matches!(*signal, SIGINT | SIGTERM | SIGHUP | SIGQUIT))
    {
        let active = Arc::clone(&prepared.startup_active);
        let cancellation = Arc::clone(&prepared.early_cancellation);
        // Registration allocates here, but the handler only performs lock-free
        // atomic operations while temporary files are being created/configured.
        let handler = unsafe {
            signal_hook::low_level::register(signal, move || {
                if active.load(Ordering::Acquire) {
                    let _ = cancellation.compare_exchange(
                        0,
                        signal,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    );
                }
            })
        }
        .map_err(|error| {
            CliError::new(
                1,
                format!("could not install startup signal handler: {error}"),
            )
        })?;
        prepared.startup_handlers.push(handler);
    }
    Ok(prepared)
}

pub(crate) fn run(
    mut command: Command,
    scratch: TaskScratch,
    mode: OutputMode,
    mut signals: PreparedSignals,
) -> Result<i32, CliError> {
    // Darwin does not reliably distinguish tty-generated signals from kill in
    // siginfo. Keep a stable attachment policy instead of consulting a changing
    // foreground group after the queued signal was generated.
    let direct_terminal_attached = cfg!(target_os = "macos")
        && mode == OutputMode::Direct
        && std::fs::File::open("/dev/tty").is_ok();
    if let Ok(signal) = Signal::try_from(signals.early_cancellation.load(Ordering::Acquire)) {
        if let Err(error) = scratch.close() {
            report_cancelled_cleanup(&error);
        }
        drop(signals);
        reraise(signal).map_err(|error| {
            CliError::new(1, format!("could not honor startup cancellation: {error}"))
        })?;
        return Ok(128 + signal as i32);
    }
    let handle = signals
        .events
        .as_ref()
        .expect("prepared signal intake")
        .handle();
    if mode == OutputMode::Compact {
        if io::stdin().is_terminal() {
            command.stdin(Stdio::null());
        }
        command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            handle.close();
            return finish_with_cleanup(Err(command_error("codex", error)), scratch.close());
        }
    };
    let early_cancellation = signals.finish_startup();
    let mut events = signals.events.take().expect("prepared signal intake");
    let original = std::mem::take(&mut signals.original);
    drop(signals);
    let child_pid = Pid::from_raw(child.id() as i32);
    // Compact mode uses the child PID as its isolated process-group ID.
    let target = Arc::new(Mutex::new(Some(child_pid)));
    let forwarding = Arc::clone(&target);
    let cancellation = Arc::new(AtomicI32::new(early_cancellation));
    let forwarded_cancellation = Arc::clone(&cancellation);
    let cancellation_deadline = Arc::new(Mutex::new(if early_cancellation == 0 {
        None
    } else {
        Some(Instant::now() + CANCELLATION_GRACE)
    }));
    let forwarded_deadline = Arc::clone(&cancellation_deadline);
    let resume = Arc::new(AtomicBool::new(false));
    let forwarded_resume = Arc::clone(&resume);
    if let Ok(signal) = Signal::try_from(early_cancellation) {
        let _ = send_to_child(child_pid, signal, mode);
        let _ = send_to_child(child_pid, Signal::SIGCONT, mode);
    }
    let signal_thread = thread::spawn(move || {
        let mut startup_event_pending = early_cancellation != 0;
        for origin in events.forever() {
            if startup_event_pending && origin.signal == early_cancellation {
                startup_event_pending = false;
                continue;
            }
            let Ok(signal) = Signal::try_from(origin.signal) else {
                continue;
            };
            // The direct child and pipeline peers already receive tty group
            // signals. Do not duplicate them or impose a cancellation deadline
            // on a TUI that intentionally handles terminal Ctrl-C itself.
            // Attached Darwin Unknown origins are ambiguous even for kill;
            // intentionally reserve these controls for the native job/app and
            // use supervisor TERM/HUP for explicit cancellation in that case.
            if mode == OutputMode::Direct
                && (origin.process.is_none()
                    || (cfg!(target_os = "macos")
                        && direct_terminal_attached
                        && matches!(origin.cause, Cause::Unknown)))
                && matches!(
                    signal,
                    Signal::SIGINT | Signal::SIGQUIT | Signal::SIGTSTP | Signal::SIGWINCH
                )
            {
                continue;
            }
            let target = forwarding.lock().expect("signal target lock");
            if matches!(
                signal,
                Signal::SIGINT | Signal::SIGTERM | Signal::SIGHUP | Signal::SIGQUIT
            ) {
                if forwarded_cancellation
                    .compare_exchange(0, signal as i32, Ordering::AcqRel, Ordering::Acquire)
                    .is_err()
                {
                    // A second cancellation means stop the owned child now,
                    // preserving the first signal as the wrapper's status.
                    if let Some(child_pid) = *target {
                        let _ = send_to_child(child_pid, Signal::SIGKILL, mode);
                    }
                    continue;
                }
                *forwarded_deadline
                    .lock()
                    .expect("cancellation deadline lock") =
                    Some(Instant::now() + CANCELLATION_GRACE);
                if let Some(child_pid) = *target {
                    let _ = send_to_child(child_pid, signal, mode);
                    // Pending cancellation must reach a stopped agent too.
                    let _ = send_to_child(child_pid, Signal::SIGCONT, mode);
                }
                // Continue remembering cancellation while the child is gone
                // but its scratch directory is still being removed.
                continue;
            }
            if signal == Signal::SIGCONT && mode == OutputMode::Direct {
                forwarded_resume.store(true, Ordering::Release);
                continue;
            }
            if let Some(child_pid) = *target {
                let _ = send_to_child(child_pid, signal, mode);
            }
            if signal == Signal::SIGTSTP
                && mode == OutputMode::Compact
                && forwarded_cancellation.load(Ordering::Acquire) == 0
            {
                // Stop the supervisor only after its owned group has received
                // suspension. SIGCONT will resume both.
                let _ = kill(getpid(), Signal::SIGSTOP);
            }
        }
    });
    let shutdown = Arc::new(AtomicBool::new(false));
    let presentation = if mode == OutputMode::Compact {
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
        Some((diagnostic_receive, receive, stdout_thread, stderr_thread))
    } else {
        None
    };
    // Reap independently of presentation: terminal/pipe backpressure must never
    // prevent cancellation or leave the agent running after output fails.
    let reaper_target = Arc::clone(&target);
    let reaper_shutdown = Arc::clone(&shutdown);
    let reaper_cancellation = Arc::clone(&cancellation);
    let reaper = thread::spawn(move || {
        let waited = (|| -> io::Result<std::process::ExitStatus> {
            loop {
                if resume.swap(false, Ordering::AcqRel) {
                    let _ = send_to_child(child_pid, Signal::SIGCONT, mode);
                }
                let mut stopped = false;
                {
                    let mut target = reaper_target.lock().expect("signal target lock");
                    if cancellation_deadline
                        .lock()
                        .expect("cancellation deadline lock")
                        .is_some_and(|deadline| Instant::now() >= deadline)
                    {
                        let _ = send_to_child(child_pid, Signal::SIGKILL, mode);
                    }
                    let status = if mode == OutputMode::Compact {
                        child.try_wait()?
                    } else {
                        loop {
                            let mut status = 0;
                            // A single waiter drains stop/continue transitions.
                            // A group fg may have resumed both wrapper and child
                            // before an old stopped notification was consumed.
                            let waited = unsafe {
                                nix::libc::waitpid(
                                    child_pid.as_raw(),
                                    &mut status,
                                    nix::libc::WNOHANG
                                        | nix::libc::WUNTRACED
                                        | nix::libc::WCONTINUED,
                                )
                            };
                            if waited < 0 {
                                let error = io::Error::last_os_error();
                                if error.kind() == io::ErrorKind::Interrupted {
                                    continue;
                                }
                                return Err(error);
                            }
                            if waited == 0 {
                                break None;
                            }
                            if nix::libc::WIFSTOPPED(status) {
                                stopped = true;
                            } else if nix::libc::WIFCONTINUED(status) {
                                stopped = false;
                            } else {
                                break Some(std::process::ExitStatus::from_raw(status));
                            }
                        }
                    };
                    if let Some(status) = status {
                        if mode == OutputMode::Compact {
                            let _ = killpg(child_pid, Signal::SIGKILL);
                        }
                        *target = None;
                        reaper_shutdown.store(true, Ordering::Release);
                        return Ok(status);
                    }
                }
                if stopped && reaper_cancellation.load(Ordering::Acquire) == 0 {
                    // The shell owns foreground groups and terminal modes;
                    // it receives this wrapper stop alongside its stopped child.
                    let _ = kill(getpid(), Signal::SIGSTOP);
                }
                thread::sleep(Duration::from_millis(10));
            }
        })();
        if waited.is_err() {
            let _ = send_to_child(child_pid, Signal::SIGKILL, mode);
            let _ = child.wait();
            *reaper_target.lock().expect("signal target lock") = None;
            reaper_shutdown.store(true, Ordering::Release);
        }
        // Closing here, before cancellation terminates this process, also
        // cleans nonempty scratch when presentation is blocked on an unread
        // pipe. Dropping a TempDir only when run returns cannot cover that path.
        let cleanup = scratch.close();
        if reaper_cancellation.load(Ordering::Acquire) != 0 {
            if let Err(error) = &cleanup {
                report_cancelled_cleanup(error);
            }
        }
        restore_signals(&original)?;
        let cancelled = reaper_cancellation.load(Ordering::Acquire);
        if cancelled != 0 {
            // The owned agent has been reaped and its temporary files removed.
            // Honor cancellation independently of output backpressure.
            if let Ok(signal) = Signal::try_from(cancelled) {
                reraise(signal)?;
            }
        } else if mode == OutputMode::Direct {
            if let Ok(status) = &waited {
                if let Some(signal) = status
                    .signal()
                    .and_then(|signal| Signal::try_from(signal).ok())
                {
                    if let Err(error) = &cleanup {
                        report_cancelled_cleanup(error);
                    }
                    reraise(signal)?;
                }
            }
        }
        match waited {
            Ok(status) => Ok((status, cleanup)),
            Err(error) => match cleanup {
                Ok(()) => Err(error),
                Err(cleanup) => Err(io::Error::new(
                    error.kind(),
                    format!("{error}; {}", cleanup.message),
                )),
            },
        }
    });
    let Some((diagnostic_receive, receive, stdout_thread, stderr_thread)) = presentation else {
        let reaped = reaper.join();
        handle.close();
        signal_thread
            .join()
            .map_err(|_| CliError::new(1, "signal forwarding thread panicked"))?;
        let (status, cleanup) = reaped
            .map_err(|_| CliError::new(1, "Codex reaper panicked"))?
            .map_err(|error| CliError::new(1, format!("could not reap Codex: {error}")))?;
        let code = status
            .code()
            .unwrap_or_else(|| 128 + status.signal().unwrap_or(1));
        return match cleanup {
            Ok(()) => Ok(code),
            Err(mut error) => {
                if code != 0 {
                    error.code = code;
                }
                Err(error)
            }
        };
    };
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
            if let Some(child_pid) = *target {
                let _ = send_to_child(child_pid, Signal::SIGKILL, mode);
            }
        }
    }
    let reaped = reaper.join();
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
    let (status, cleanup) = reaped
        .map_err(|_| CliError::new(1, "Codex reaper panicked"))?
        .map_err(|e| CliError::new(1, format!("could not reap Codex: {e}")))?;
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
    let cleanup_failed = cleanup.is_err();
    let outcome = (|| {
        result.map_err(|e| {
            CliError::new(
                1,
                format!("could not render Codex output: {e}; use --verbose for direct output"),
            )
        })?;
        stderr_result
            .map_err(|e| CliError::new(1, format!("could not read Codex diagnostics: {e}")))?;
        let rendered = renderer.finish(code == 0 && !cleanup_failed);
        if code != 0 {
            return Err(CliError::new(
                code,
                format!("Codex exited with status {code}"),
            ));
        }
        match rendered {
            Ok(true) => Ok(()),
            Ok(false) if cleanup_failed => Ok(()),
            Ok(false) => Err(CliError::new(
                1,
                "Codex event stream did not complete successfully",
            )),
            Err(e) => Err(CliError::new(
                1,
                format!("could not render Codex output: {e}"),
            )),
        }
    })();
    finish_with_cleanup(outcome, cleanup).map(|()| 0)
}

fn send_to_child(child: Pid, signal: Signal, mode: OutputMode) -> nix::Result<()> {
    match mode {
        OutputMode::Compact => killpg(child, signal),
        // Direct mode shares the caller's job group. Never kill pipeline peers
        // or unrelated callers to cancel a single Codex invocation.
        OutputMode::Direct => kill(child, signal),
    }
}

fn report_cancelled_cleanup(error: &CliError) {
    // Cancellation must not wait for a stderr lock or an unread output pipe.
    // Make one best-effort write; it may be partial or unavailable. Restore the
    // shared descriptor flags before the caller re-raises its original signal.
    let stderr = io::stderr();
    let Ok(flags) = fcntl(stderr.as_fd(), FcntlArg::F_GETFL) else {
        return;
    };
    let flags = OFlag::from_bits_retain(flags);
    if fcntl(stderr.as_fd(), FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK)).is_err() {
        return;
    }
    let diagnostic = format!("codex-commit: {}\n", error.message);
    let _ = nix::unistd::write(stderr.as_fd(), diagnostic.as_bytes());
    let _ = fcntl(stderr.as_fd(), FcntlArg::F_SETFL(flags));
}

fn finish_with_cleanup<T>(
    outcome: Result<T, CliError>,
    cleanup: Result<(), CliError>,
) -> Result<T, CliError> {
    match (outcome, cleanup) {
        (outcome, Ok(())) => outcome,
        (Ok(_), Err(cleanup)) => Err(cleanup),
        (Err(mut error), Err(cleanup)) => {
            error.message.push_str("; ");
            error.message.push_str(&cleanup.message);
            Err(error)
        }
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

fn restore_signals(original: &[(i32, nix::libc::sigaction)]) -> io::Result<()> {
    // No owned child remains. Restore cancellation before any further output
    // or cleanup can block; signal-hook otherwise leaves no-op dispositions.
    for (signal, action) in original {
        if unsafe { nix::libc::sigaction(*signal, action, std::ptr::null_mut()) } < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

fn reraise(signal: Signal) -> io::Result<()> {
    // Rust normally ignores SIGPIPE. A directly attached child's signal death
    // must still stay a signal death rather than becoming numeric exit 141.
    if !matches!(signal, Signal::SIGKILL | Signal::SIGSTOP) {
        let default = SigAction::new(SigHandler::SigDfl, SaFlags::empty(), SigSet::empty());
        unsafe { sigaction(signal, &default) }?;
    }
    let mut set = SigSet::empty();
    set.add(signal);
    set.thread_unblock()?;
    kill(getpid(), signal)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    #[test]
    fn startup_cancellation_is_recorded_and_original_dispositions_are_restored() {
        const CHILD: &str = "CODEX_COMMIT_SIGNAL_TEST_CHILD";
        fn wait_for_signal(signals: &PreparedSignals) {
            let deadline = Instant::now() + Duration::from_secs(2);
            while signals.early_cancellation.load(Ordering::Acquire) == 0 {
                assert!(
                    Instant::now() < deadline,
                    "startup handler must observe cancellation"
                );
                thread::sleep(Duration::from_millis(1));
            }
        }
        if let Some(phase) = std::env::var_os(CHILD) {
            let mut before: nix::libc::sigaction = unsafe { std::mem::zeroed() };
            assert_eq!(
                unsafe { nix::libc::sigaction(SIGTERM, std::ptr::null(), &mut before) },
                0
            );
            let signals = prepare_signals().unwrap_or_else(|error| panic!("{}", error.message));
            if phase == "drop" {
                kill(getpid(), Signal::SIGTERM).unwrap();
                wait_for_signal(&signals);
                assert_eq!(signals.early_cancellation.load(Ordering::Acquire), SIGTERM);
                drop(signals);
                let mut after: nix::libc::sigaction = unsafe { std::mem::zeroed() };
                assert_eq!(
                    unsafe { nix::libc::sigaction(SIGTERM, std::ptr::null(), &mut after) },
                    0
                );
                assert_eq!(after.sa_sigaction, before.sa_sigaction);
                return;
            }
            let scratch = TaskScratch::new(&std::env::current_dir().unwrap())
                .unwrap_or_else(|error| panic!("{}", error.message));
            let base = std::path::PathBuf::from(std::env::var_os("TMPDIR").unwrap());
            std::fs::write(
                base.join("owned-root"),
                scratch.path().as_os_str().as_encoded_bytes(),
            )
            .unwrap();
            let mut command = Command::new("/bin/sh");
            command.args(["-c", "touch \"$TMPDIR/must-not-launch\""]);
            kill(getpid(), Signal::SIGTERM).unwrap();
            wait_for_signal(&signals);
            let _ = run(command, scratch, OutputMode::Direct, signals);
            panic!("startup cancellation must terminate before launching Codex");
        }

        // Signal dispositions are process-global: run each scenario in a
        // dedicated test subprocess, never in the parallel unit-test process.
        let base = std::env::var_os("CODEX_COMMIT_TEST_TMPDIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        std::fs::create_dir_all(&base).unwrap();
        let fixture = tempfile::Builder::new()
            .prefix("startup-signals-")
            .tempdir_in(base)
            .unwrap();
        for phase in ["drop", "cancel"] {
            let output = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "process::tests::startup_cancellation_is_recorded_and_original_dispositions_are_restored", "--nocapture"])
                .env(CHILD, phase).env("TMPDIR", fixture.path()).output().unwrap();
            if phase == "drop" {
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
            } else {
                assert_eq!(output.status.signal(), Some(SIGTERM));
                let path = std::fs::read_to_string(fixture.path().join("owned-root")).unwrap();
                assert!(!std::path::Path::new(&path).exists());
                assert!(!fixture.path().join("must-not-launch").exists());
            }
        }
    }

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
