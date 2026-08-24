//! Attaching the user's real terminal to a harness session.
//!
//! This is Glasshouse's first production consumer of
//! [`crate::launch::HarnessLaunch`]: a direct-attach session that hands the
//! terminal to a real native harness and gets out of the way. It is
//! deliberately a *transparent bridge*, not a renderer — bytes from the
//! harness go straight to the terminal and bytes from the terminal go
//! straight to the harness — which is what "Glasshouse orchestrates agents
//! without hiding them" means at this layer.
//!
//! # Why there is no terminal emulation here
//!
//! The pty module's documentation records that ConPTY opens every Windows
//! session by asking "where is the cursor?" (`ESC[6n`) and blocks until
//! something answers, and that answering it needs a component that
//! understands terminal control sequences. A direct attach already has one:
//! the user's own terminal emulator. The query travels out of the pty, into
//! this process's standard output, and to the real terminal, which replies on
//! standard input exactly as it would for any other program — and that reply
//! is forwarded straight back into the pty. Nothing here parses it, and
//! nothing here may answer it *for* the terminal: two replies to one query
//! would be delivered to the harness as spurious input.
//!
//! This is why [`attach`] insists on a terminal at both ends rather than
//! treating that as a nicety. Attached to a pipe there would be no emulator
//! to answer, and a Windows session would hang before its first byte with
//! nothing in the output to explain why.
//!
//! # Lifetime of the pumps
//!
//! Two threads move bytes. The output pump ends by itself when the pty
//! closes. The input pump cannot: a thread blocked in a read on standard
//! input is not cancellable, and there is no portable way to interrupt one
//! without stealing the keystroke that unblocks it. It is therefore left
//! running and the process exits out from under it, which is sound only
//! because `attach` owns the terminal for the whole life of the process.
//! Nothing here is reusable from inside a longer-lived interface; the
//! session runtime that multiplexes several harnesses needs a different
//! input path, not this one.

use std::io::{IsTerminal, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::launch::HarnessLaunch;
use crate::pty::{ExitStatus, ProcessSignal, PtyOutput, PtyProcess, TerminalSize};
use crate::shutdown::{RawModeGuard, shutdown_requested};

/// How often the supervising loop wakes to poll the child and the terminal.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// How often the terminal is measured to notice a window resize.
///
/// Polling rather than handling `SIGWINCH` is deliberate: it is one cheap
/// call, it behaves identically on Windows (which has no such signal), and it
/// keeps resize handling out of a signal handler, where almost nothing is
/// safe to do. The cost is that a resize is noticed up to this long after it
/// happens, which is imperceptible next to the redraw the harness then does.
const RESIZE_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How long to keep relaying output after the harness has exited.
///
/// A harness usually prints something on its way out, and that write can
/// still be in flight when the process is already gone. On Unix the output
/// pump sees end-of-file promptly and this bound is never reached; on Windows
/// no end-of-file arrives at all while the pty is open (see [`crate::pty`]),
/// so a bound is the only thing that ends the wait.
const OUTPUT_DRAIN_GRACE: Duration = Duration::from_millis(250);

/// How long a harness is given to shut down cleanly after Glasshouse is
/// asked to terminate, before it is killed outright.
const TERMINATION_GRACE: Duration = Duration::from_secs(5);

/// Run a harness session attached to this process's terminal, and return how
/// it finished.
///
/// The terminal is put into raw mode for the duration, which is what routes
/// Ctrl-C to the *harness* rather than to Glasshouse: raw mode disables the
/// line discipline's signal generation, so the keystroke arrives as an
/// ordinary `0x03` byte and is forwarded like any other input. A harness that
/// cancels its current turn on Ctrl-C therefore behaves exactly as it does
/// when run directly, which is the whole point of not hiding it.
///
/// Raw mode is entered *before* the harness starts so that no output can be
/// mangled by the line discipline in the window between spawn and setup, and
/// it is restored on every exit path — normal return, error, panic, or signal
/// — because [`RawModeGuard`] registers with the same restoration machinery
/// the panic hook and signal handler use.
pub fn attach(launch: HarnessLaunch<'_>) -> Result<ExitStatus> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!(
            "a harness session needs a terminal on both standard input and standard \
             output; run this from an interactive terminal rather than through a pipe \
             or a redirect"
        );
    }

    // Measured before raw mode so a failure here leaves the terminal
    // untouched. A harness lays its interface out from the size it sees at
    // startup, so this has to reach the child as its initial size rather
    // than arrive as a resize after the first frame.
    let size = terminal_size();

    let guard = RawModeGuard::acquire()?;
    let (process, output) = launch.size(size).spawn()?;

    let process = Arc::new(Mutex::new(process));
    let output_drained = Arc::new(AtomicBool::new(false));

    // A second interrupt forces Glasshouse down through `process::exit`, which
    // runs no destructor — so without this the harness would be left running
    // in its own session with nothing left to hang it up. Best effort by
    // design: `try_lock` gives up rather than risk blocking the one path whose
    // whole purpose is to always work. The guard unregisters on the way out,
    // so the callback never outlives the session it refers to.
    let _forced_exit = {
        let process = Arc::clone(&process);
        crate::shutdown::on_forced_exit(move || {
            if let Ok(mut process) = process.try_lock() {
                let _ = process.signal(ProcessSignal::Kill);
            }
        })
    };

    {
        let output_drained = Arc::clone(&output_drained);
        std::thread::Builder::new()
            .name("glasshouse-session-output".into())
            .spawn(move || pump_output(output, &output_drained))
            .context("could not start the session output reader")?;
    }
    {
        let process = Arc::clone(&process);
        std::thread::Builder::new()
            .name("glasshouse-session-input".into())
            .spawn(move || pump_input(&process))
            .context("could not start the session input forwarder")?;
    }

    let status = supervise(&process, size)?;

    // Let whatever the harness printed on its way out actually reach the
    // terminal before the guard below puts it back into cooked mode.
    let deadline = Instant::now() + OUTPUT_DRAIN_GRACE;
    while !output_drained.load(Ordering::SeqCst) && Instant::now() < deadline {
        std::thread::sleep(POLL_INTERVAL);
    }

    // Explicit rather than implicit at end of scope: everything after this
    // point — a caller's diagnostics, the shell prompt — belongs to a normal
    // terminal, not a raw one.
    drop(guard);

    Ok(status)
}

/// Watch the session until the harness exits, forwarding window resizes and
/// honouring a shutdown request.
///
/// Exit is detected from the process itself, never from its output going
/// quiet: a harness waiting on the user produces nothing for minutes at a
/// time, and on Windows the pty yields no end-of-file to notice either.
fn supervise(process: &Mutex<PtyProcess>, initial_size: TerminalSize) -> Result<ExitStatus> {
    let mut last_size = initial_size;
    let mut last_size_check = Instant::now();
    let mut termination_deadline: Option<Instant> = None;

    loop {
        {
            let mut process = lock(process);
            if let Some(status) = process
                .try_wait()
                .context("could not check whether the harness had exited")?
            {
                return Ok(status);
            }

            // A signal reached Glasshouse — not a Ctrl-C, which raw mode
            // delivers to the harness as input and never as a signal, but a
            // real termination request from outside. Ask the harness to stop,
            // then insist if it does not.
            if shutdown_requested() {
                match termination_deadline {
                    None => {
                        termination_deadline = Some(Instant::now() + TERMINATION_GRACE);
                        // Best effort throughout: the harness exiting on its
                        // own between the poll above and here is a race, not
                        // a failure, and the next poll reports it properly.
                        let _ = process.signal(ProcessSignal::Terminate);
                    }
                    Some(deadline) if Instant::now() >= deadline => {
                        let _ = process.signal(ProcessSignal::Kill);
                    }
                    Some(_) => {}
                }
            }
        }

        if last_size_check.elapsed() >= RESIZE_POLL_INTERVAL {
            last_size_check = Instant::now();
            let current = terminal_size();
            if current != last_size {
                last_size = current;
                // A resize that fails is not worth ending a working session
                // over; the harness simply keeps its previous geometry.
                if let Err(err) = lock(process).resize(current) {
                    tracing::debug!(error = %err, "could not resize the harness terminal");
                }
            }
        }

        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Relay everything the harness prints to this process's standard output.
///
/// Every chunk is flushed as it arrives. Buffering would be visible as an
/// interface that lags behind the keystrokes producing it, which for an
/// interactive harness is indistinguishable from it having hung.
fn pump_output(mut output: PtyOutput, drained: &AtomicBool) {
    let mut buffer = [0u8; 8192];
    let mut stdout = std::io::stdout();
    loop {
        // Both `Ok(0)` and an error mean the same thing here: nothing more is
        // coming. A pty reports the end of a session as end-of-file on some
        // platforms and as a read error on others, and neither is a fault to
        // report — the supervising loop already knows the exit status from
        // the process itself.
        let read = match output.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        if stdout.write_all(&buffer[..read]).is_err() || stdout.flush().is_err() {
            break;
        }
    }
    drained.store(true, Ordering::SeqCst);
}

/// Forward this process's standard input to the harness, byte for byte.
///
/// Raw and untranslated on purpose. Everything the harness's own interface
/// depends on — escape sequences for arrow keys, bracketed paste, the `0x03`
/// that Ctrl-C becomes in raw mode, and the terminal's reply to a cursor
/// position query — is carried by exactly these bytes, and any interpretation
/// here would break one of them.
fn pump_input(process: &Mutex<PtyProcess>) {
    let mut buffer = [0u8; 4096];
    let mut stdin = std::io::stdin();
    loop {
        let read = match stdin.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        // Held only for the write; the blocking read above deliberately
        // happens outside the lock so a quiet session never keeps the
        // supervising loop from polling.
        if lock(process).write_input(&buffer[..read]).is_err() {
            break;
        }
    }
}

/// Lock the shared process, ignoring poisoning.
///
/// A panicking pump would poison this mutex, and refusing to continue at that
/// point would be the worst possible response: the child would be left
/// running with nothing supervising it. The data behind the lock is a handle
/// to a live process, not an invariant a panic could have corrupted, so
/// taking it regardless is both safe and the only option that still ends the
/// session cleanly.
fn lock(process: &Mutex<PtyProcess>) -> std::sync::MutexGuard<'_, PtyProcess> {
    process
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The terminal's current size, falling back to a usable default.
///
/// A terminal that cannot be measured is not a reason to refuse a session:
/// the harness gets a conventional 80x24 and corrects itself on the first
/// real resize.
fn terminal_size() -> TerminalSize {
    match crossterm::terminal::size() {
        Ok((cols, rows)) => TerminalSize::new(rows, cols),
        Err(err) => {
            tracing::debug!(error = %err, "could not measure the terminal; using the default size");
            TerminalSize::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Attaching without a terminal must fail with a diagnostic rather than
    /// hang. Under `cargo test` standard input is not a terminal, so this
    /// exercises the real guard on the real path.
    ///
    /// The message matters as much as the failure: the reason a session needs
    /// a terminal (something has to answer the pty's own queries) is not
    /// something a user can be expected to infer from a silent hang.
    #[test]
    fn attaching_without_a_terminal_is_refused() {
        // Only meaningful when the test runner really has no terminal, which
        // is the normal case but not guaranteed for a developer running a
        // single test from an interactive shell.
        if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
            return;
        }

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("proj");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let project = crate::Project::discover(&root, None, false).unwrap();

        let program = tmp.path().join("fake-harness");
        std::fs::write(&program, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&program).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&program, perms).unwrap();
        }
        let executable = crate::platform::exec::resolve_explicit(&program).unwrap();

        let err = attach(HarnessLaunch::new(executable, &project)).unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("needs a terminal"), "{message}");
    }

    /// A zero dimension from a terminal that cannot report its size must
    /// never reach the child: every harness interface misbehaves at zero and
    /// some platforms reject it outright.
    #[test]
    fn the_measured_terminal_size_is_always_usable() {
        let size = terminal_size();
        assert!(size.rows >= 1 && size.cols >= 1);
    }
}
