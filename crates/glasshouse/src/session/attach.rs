//! Attaching the user's real terminal to a harness session.
//!
//! Glasshouse's first production consumer of
//! [`crate::launch::HarnessLaunch`]: a direct-attach session that hands the
//! terminal to a real native harness and gets out of the way. It is a
//! *transparent bridge*, not a renderer — bytes pass straight through in
//! both directions, and nothing here may answer a query from the pty (such
//! as ConPTY's cursor-position probe) itself: that is the user's terminal
//! emulator's job, and a second answer would reach the harness as spurious
//! input. [`attach`] therefore refuses to run without a terminal at both
//! ends.
//!
//! Two threads move bytes. The output pump ends when the pty closes. The
//! input pump cannot be cancelled — a blocked read on standard input has no
//! portable interrupt — so it is left running when the process exits, which
//! is sound only because `attach` owns the terminal for the process's whole
//! life. It must not own anything whose destructor matters; see
//! `spawn_input_pump`.
// History: design-decisions.md, "Trims: session module docs, second packet", session/attach.rs module doc.

use std::io::{IsTerminal, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::launch::HarnessLaunch;
use crate::pty::{ExitStatus, ProcessSignal, PtyOutput, PtyProcess, TerminalSize, next_chunk};
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
    spawn_input_pump(&process, std::io::stdin())?;

    // Plain `?`. Nothing after `launch.spawn()` above holds a strong
    // reference to this `Arc` except this stack frame and the forced-exit
    // guard, which unregisters before it — so every return from here,
    // including this one, drops the last reference and runs
    // `PtyProcess::drop`, which kills the harness and reaps it. That is the
    // property `spawn_input_pump` exists to keep; see its doc comment for why
    // the pump must not own the process.
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
    // Both `Ok(0)` and an error mean the same thing here: nothing more is
    // coming. A pty reports the end of a session as end-of-file on some
    // platforms and as a read error on others, and neither is a fault to
    // report — the supervising loop already knows the exit status from
    // the process itself. A read a signal interrupted is neither, and
    // [`next_chunk`] keeps it from ending this relay.
    while let Some(read) = next_chunk(&mut output, &mut buffer) {
        if stdout.write_all(&buffer[..read]).is_err() || stdout.flush().is_err() {
            break;
        }
    }
    drained.store(true, Ordering::SeqCst);
}

/// Start the thread that forwards the terminal's input to the harness,
/// **without giving it ownership of the process**.
///
/// Holds a [`std::sync::Weak`], never an `Arc`: this thread cannot be
/// cancelled (module doc), so an `Arc` clone would never let the strong
/// count reach zero, and `PtyProcess::drop` — the only thing that kills and
/// reaps the harness on an unhappy path — would never run. A `Weak` costs
/// one upgrade per keystroke and makes the guarantee structural: the pump
/// *cannot* keep the harness alive, whatever `attach` later adds.
///
/// `input` is the terminal's real standard input in production and a
/// parameter only so a test can hold this pump open without one.
// History: design-decisions.md, "Trims: session module docs, second packet", session/attach.rs `spawn_input_pump`.
fn spawn_input_pump(
    process: &Arc<Mutex<PtyProcess>>,
    input: impl Read + Send + 'static,
) -> Result<()> {
    let process = Arc::downgrade(process);
    std::thread::Builder::new()
        .name("glasshouse-session-input".into())
        .spawn(move || pump_input(input, &process))
        .context("could not start the session input forwarder")?;
    Ok(())
}

/// Forward this process's standard input to the harness, byte for byte, with
/// no interpretation: everything the harness's own interface depends on
/// (arrow keys, bracketed paste, Ctrl-C's `0x03`, a cursor-position reply) is
/// carried by exactly these bytes.
///
/// This thread cannot be cancelled (module doc), so when its read ends there
/// is no second chance to notice why: a lost terminal must call
/// `crate::shutdown::request_shutdown` here, not just fall silent. This is
/// belt-and-suspenders, not the only path — `supervise` already reacts to
/// `shutdown_requested` on the signal path (`SIGHUP`) — but it is what still
/// notices if that path ever stops applying. `stdin_hung_up` confirms an
/// actual hangup (via `POLLHUP`) before requesting shutdown, so a stray
/// `Err` unrelated to one cannot trigger a shutdown it does not warrant.
// History: design-decisions.md, "Trims: session module docs, second packet", session/attach.rs `pump_input`.
fn pump_input(mut input: impl Read, process: &Weak<Mutex<PtyProcess>>) {
    let mut buffer = [0u8; 4096];
    loop {
        // A read a signal interrupted is not a lost terminal, and treating it
        // as one here has the sharpest version of the consequence: this
        // thread is the only thing carrying the keyboard and it is never
        // restarted, so the harness would go deaf for the rest of the session
        // while `stdin_hung_up` below correctly reported no hangup, and
        // nothing would notice. See [`next_chunk`].
        let Some(read) = next_chunk(&mut input, &mut buffer) else {
            // Confirmed hangup, not merely inferred: only this fires
            // shutdown. `attach` owns the terminal for the whole life of
            // the process (module doc), so there is nothing else in it
            // for a process-wide shutdown to wrongly take down.
            if stdin_hung_up() {
                crate::shutdown::request_shutdown();
            }
            break;
        };
        // Upgraded per write and dropped again before the next read, so this
        // thread never holds the process across the blocking read above — see
        // [`spawn_input_pump`]. A `None` means `attach` has returned and the
        // harness is already being killed and reaped by the drop that made
        // this upgrade fail; there is nothing left to forward input to.
        let Some(process) = process.upgrade() else {
            break;
        };
        // Held only for the write; the blocking read above deliberately
        // happens outside the lock so a quiet session never keeps the
        // supervising loop from polling. A failure here means the harness
        // itself is gone, not the terminal — `supervise` already learns that
        // from the child's own exit status, so this does not request
        // shutdown too.
        if lock(&process).write_input(&buffer[..read]).is_err() {
            break;
        }
    }
}

/// Whether standard input's far end has gone away.
///
/// Mirrors [`crate::tui::event::wait_for_terminal`]'s `POLLHUP` check rather
/// than duplicating its reasoning by inference: a hung-up descriptor reports
/// `POLLHUP` (and `POLLERR`/`POLLNVAL`, the same unusable-descriptor outcome
/// by a different route) independently of whatever a prior read on it
/// returned. The poll has a zero timeout — the read that got us here already
/// did the waiting, so this only asks what state the descriptor is in now.
#[cfg(unix)]
fn stdin_hung_up() -> bool {
    use std::os::fd::AsRawFd;

    let mut watched = libc::pollfd {
        fd: std::io::stdin().as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: `watched` is one initialised `pollfd` and the count says so.
    // `poll` reads `fd`/`events` and writes only `revents`.
    let ready = unsafe { libc::poll(&mut watched, 1, 0) };
    ready > 0 && watched.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0
}

/// This platform has no hangup answer yet — see
/// [`crate::tui::event::wait_for_terminal`]'s own Windows note. Reporting
/// "not hung up" here keeps the old behaviour exactly: the pump still ends,
/// it just does not request a shutdown nobody has confirmed is warranted.
#[cfg(not(unix))]
fn stdin_hung_up() -> bool {
    false
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

    /// The input pump must not keep the harness alive, because it cannot be
    /// ended and `PtyProcess::drop` is the only thing that kills and reaps on
    /// an unhappy path.
    ///
    /// Two assertions, which together are the whole property `attach`'s error
    /// returns depend on. That the strong count is still one with the pump
    /// running is the structural half: nothing but this frame owns the
    /// process, so every return from `attach` — including the `?` on
    /// `supervise`, whose `try_wait` surfaces a raw `waitpid` failure — drops
    /// the last reference. That the pid is unaddressable after that drop is
    /// the half that matters to the user: `kill(pid, 0)` fails with `ESRCH`
    /// only once the harness has been both killed and reaped, so neither an
    /// orphaned session leader nor a zombie survives.
    ///
    /// Dropping the `Arc` stands in for `attach` returning; there is no way
    /// to call `attach` itself here, because it requires a real terminal on
    /// both standard input and standard output and refuses before it spawns
    /// anything otherwise.
    ///
    /// Non-vacuity: make `spawn_input_pump` clone the `Arc` instead of
    /// downgrading it — which is what it did before — and both assertions
    /// fail: the count is two, and the harness is still running afterwards.
    #[cfg(unix)]
    #[test]
    fn the_input_pump_does_not_keep_the_harness_alive() {
        use std::sync::atomic::AtomicBool;

        use crate::pty::{PtyProcess, TerminalCommand};

        /// A terminal nobody is typing at, until the test says otherwise.
        ///
        /// Releasing it produces a byte rather than an end-of-file on
        /// purpose: an ending would send the pump through its hangup check,
        /// which asks about this *process's* real standard input and could
        /// request a shutdown that has nothing to do with this test. A byte
        /// sends it to the upgrade instead, which is the path under test.
        struct QuietTerminal {
            reading: Arc<AtomicBool>,
            release: Arc<AtomicBool>,
        }

        impl Read for QuietTerminal {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                self.reading.store(true, Ordering::SeqCst);
                while !self.release.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                buf[0] = b'x';
                Ok(1)
            }
        }

        // A harness that would outlive this test by minutes if nothing ended
        // it, so "gone" cannot be mistaken for "finished on its own".
        let (process, _output) =
            PtyProcess::spawn(TerminalCommand::new("/bin/sh", "/").args(["-c", "sleep 300"]))
                .expect("spawn a long-running harness");
        let pid = process.process_id().expect("a live child has a pid");

        let process = Arc::new(Mutex::new(process));
        let reading = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        spawn_input_pump(
            &process,
            QuietTerminal {
                reading: Arc::clone(&reading),
                release: Arc::clone(&release),
            },
        )
        .expect("start the input pump");

        let deadline = Instant::now() + Duration::from_secs(10);
        while !reading.load(Ordering::SeqCst) {
            assert!(Instant::now() < deadline, "the input pump never started");
            std::thread::sleep(Duration::from_millis(5));
        }

        assert_eq!(
            Arc::strong_count(&process),
            1,
            "the input pump must not own the harness; with a strong reference \
             held by a thread that cannot be cancelled, no return from `attach` \
             can ever run `PtyProcess::drop`"
        );

        // What every return from `attach` does.
        drop(process);

        // SAFETY: signal 0 sends nothing; it only asks whether the pid is
        // still addressable by this process. A zombie would answer `Ok`, so
        // `ESRCH` is the reaped-and-gone answer specifically.
        let alive = unsafe { libc::kill(pid as libc::pid_t, 0) };
        let errno = std::io::Error::last_os_error().raw_os_error();
        assert_eq!(
            (alive, errno),
            (-1, Some(libc::ESRCH)),
            "dropping the last reference must leave no harness (pid {pid}) running \
             and no zombie behind"
        );

        // Let the pump's read return so the thread ends on the failed
        // upgrade rather than outliving the test blocked in a read.
        release.store(true, Ordering::SeqCst);
    }
}
