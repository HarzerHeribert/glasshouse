//! Input and application events.
//!
//! Glasshouse's interface has to react to two unrelated sources: the user's
//! keyboard and terminal, and things that happen on their own — a harness
//! session producing output, a session exiting, a lifecycle event arriving.
//! Both land in one [`Event`] stream so the interface has a single loop and a
//! single place where redraws are decided.

use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::event::{self, KeyEvent, MouseEvent};

/// Something the interface must react to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Key(KeyEvent),
    Mouse(MouseEvent),
    /// Text pasted into the terminal, when bracketed paste is enabled.
    Paste(String),
    /// The terminal window changed size, in columns and rows.
    Resize(u16, u16),
    /// No input arrived within the tick interval.
    ///
    /// A tick is not a redraw request. It exists so the loop regains control
    /// regularly enough to notice a shutdown request, poll child processes,
    /// and repaint anything that became dirty on another thread.
    Tick,
    /// A signal asked Glasshouse to wind down.
    Shutdown,
    /// Something outside the terminal happened. Producers on other threads
    /// send these through [`EventSource::sender`].
    App(AppEvent),
}

/// An event raised by Glasshouse itself rather than by the terminal.
///
/// Deliberately small for now. Session and lifecycle variants arrive with the
/// session runtime; adding them speculatively would mean guessing at their
/// payloads before there is a session to carry one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppEvent {
    /// Something changed and the interface should repaint.
    Redraw,
}

/// Pulls events from the terminal and from other threads.
///
/// The terminal is polled with a timeout rather than read on a dedicated
/// thread. A thread blocked in `event::read()` cannot be cancelled — it would
/// sit there holding stdin until the user happened to press a key — so polling
/// keeps shutdown immediate and costs nothing but one syscall per tick.
///
/// That polling is done by this module's own `wait_for_terminal` rather than
/// by crossterm, because a terminal that goes away is not something
/// crossterm's own poll can report or even survive. See that function.
pub struct EventSource {
    tick: Duration,
    sender: Sender<AppEvent>,
    receiver: Receiver<AppEvent>,
}

impl EventSource {
    /// Create a source that yields [`Event::Tick`] when idle for `tick`.
    pub fn new(tick: Duration) -> Self {
        let (sender, receiver) = channel();
        Self {
            tick,
            sender,
            receiver,
        }
    }

    /// A handle other threads can use to wake the interface.
    ///
    /// Cloneable, so every session reader can hold one.
    pub fn sender(&self) -> Sender<AppEvent> {
        self.sender.clone()
    }

    /// Wait for the next event.
    ///
    /// Returns [`Event::Shutdown`] as soon as a signal has asked Glasshouse to
    /// stop, so callers never need to check that separately — and equally when
    /// the terminal itself has gone away, which is the same instruction
    /// arriving without a signal to carry it.
    pub fn next(&self) -> Result<Event> {
        if crate::shutdown::shutdown_requested() {
            return Ok(Event::Shutdown);
        }

        // Events queued by other threads are already here; deliver them before
        // spending the tick waiting on the terminal.
        match self.receiver.try_recv() {
            Ok(app) => return Ok(Event::App(app)),
            Err(TryRecvError::Empty) => {}
            // The source owns a sender, so the channel cannot be disconnected
            // while `self` is alive. Treat it as empty rather than failing.
            Err(TryRecvError::Disconnected) => {}
        }

        let deadline = Instant::now() + self.tick;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(Event::Tick);
            }
            // The wait happens here rather than inside crossterm, so that a
            // terminal which has gone away is recognised as one. See
            // `wait_for_terminal` for why that distinction cannot be left to
            // the library.
            let waited = wait_for_terminal(remaining)?;
            match waited {
                Wait::HangUp => {
                    // Nothing will ever arrive on this terminal again, and
                    // there is no longer anyone to show an interface to. Set
                    // the process-wide flag as well as answering this caller:
                    // a second loop started afterwards (the wizard handing
                    // over to the shell) must not sit down at the same dead
                    // terminal, and an attached session supervising a harness
                    // watches the same flag.
                    crate::shutdown::request_shutdown();
                    return Ok(Event::Shutdown);
                }
                // A signal cut the wait short, so it answered nothing about
                // the terminal. Look again rather than handing crossterm a
                // terminal nobody has just examined — the hangup and the
                // `SIGHUP` announcing it arrive together, and taking the
                // signal as an answer walked straight back into the spin.
                Wait::Interrupted => continue,
                Wait::Ready | Wait::Idle | Wait::Unavailable => {}
            }
            // Already waited above, so crossterm is asked only for what it
            // has *now* — except where the wait could not be taken over, in
            // which case it does the waiting exactly as it always did.
            let poll_for = if waited == Wait::Unavailable {
                remaining
            } else {
                Duration::ZERO
            };
            if !event::poll(poll_for).context("could not poll for terminal input")? {
                continue;
            }
            let raw = event::read().context("could not read terminal input")?;
            match translate(raw) {
                // Events Glasshouse does not act on (key releases, focus
                // changes) must not be reported as input, but must also not
                // consume the whole tick — keep waiting out the remainder.
                None => continue,
                Some(ev) => return Ok(ev),
            }
        }
    }
}

/// What one wait on the terminal produced.
///
/// A platform with no hangup answer never constructs [`Wait::HangUp`] or
/// [`Wait::Interrupted`], and an unconstructed variant is a `-D warnings`
/// build failure rather than a warning. The variants stay — they are the
/// vocabulary the loop above is written in, and deleting them per platform
/// would mean two loops — so the platform that cannot produce them says so
/// here. Found by compiling the non-Unix path locally with the cfg flipped
/// (practice §18), which is the only Windows evidence this machine can offer.
#[cfg_attr(not(unix), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wait {
    /// Input is there to be read.
    Ready,
    /// Nothing arrived before the deadline.
    Idle,
    /// A signal arrived instead, so the wait says nothing either way.
    Interrupted,
    /// The far end of the terminal is gone.
    HangUp,
    /// The wait could not be taken over on this platform, so nothing was
    /// waited on and crossterm must do it — exactly as it did before any of
    /// this existed.
    Unavailable,
}

/// Wait up to `timeout` for the terminal, distinguishing input from a hangup.
///
/// # Why this is not left to crossterm
///
/// `crossterm::event::poll` cannot report a hangup, and worse, it cannot
/// survive one. Its Unix source reacts to a readable terminal by looping on
/// `read` until the read yields an event or fails; a descriptor whose far end
/// has gone away is *permanently readable and returns zero bytes*, which is
/// neither. `try_read` therefore never returns, so `poll` never returns, so
/// [`EventSource::next`] never returns, and the shutdown check at the top of
/// it is never reached again.
///
/// That is not a theory. Three orphaned `glasshouse` processes were found
/// nineteen hours old at 99% CPU, and a 1622-sample profile put every single
/// sample in that `read`. A signal had already asked one of them to stop and
/// it never noticed, because noticing happens between calls to `next` and
/// there was never going to be another one.
///
/// So the wait is taken over here, and crossterm is only ever handed a
/// terminal that has bytes waiting. `poll(2)` reports `POLLHUP` the moment
/// the far end closes, and it is the right instrument rather than a
/// speculative `read`: it cannot consume a keystroke, and this loop is the
/// only thing reading the user's input. Measured on macOS against a pty whose
/// master was closed: a live terminal with input pending reports `POLLIN`
/// alone (`0x1`) and a hung-up one reports `POLLIN | POLLHUP` (`0x11`), so a
/// keystroke can never be mistaken for a hangup.
///
/// One race remains and is not closable from here: a terminal that dies in the
/// window between this call returning [`Wait::Ready`] and crossterm's `read`
/// leaves crossterm in the same unbounded loop. That window is microseconds
/// wide against the 16ms one it replaces, and closing it would mean parsing
/// terminal input here instead of in the library.
///
/// # Windows
///
/// **Deliberately unhandled.** Windows has no `poll` on a console handle, its
/// console input is read through `ReadConsoleInput` rather than a descriptor,
/// and a console that goes away there produces a `CTRL_CLOSE_EVENT` and a
/// failing handle rather than an endless run of zero-byte reads — a different
/// mechanism, needing a different answer. This project has no way to run a
/// native Windows terminal, so a Windows branch here could not be tested by
/// anyone who wrote it. [`Wait::Unavailable`] keeps the old behaviour there
/// exactly, and this comment is the record of what is missing: a Windows
/// hangup path, and the native terminal needed to prove it.
#[cfg(unix)]
fn wait_for_terminal(timeout: Duration) -> Result<Wait> {
    let Some(fd) = terminal_fd() else {
        return Ok(Wait::Unavailable);
    };

    let mut watched = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // `poll` counts whole milliseconds. Round a sub-millisecond remainder up
    // rather than down: a zero timeout here would turn the last fraction of
    // every tick into a spin, which is the defect this exists to remove.
    let millis = i32::try_from(timeout.as_millis().max(1)).unwrap_or(i32::MAX);

    // SAFETY: `watched` is one initialised `pollfd` and the count says so.
    // `poll` reads `fd`/`events` and writes only `revents`.
    let ready = unsafe { libc::poll(&mut watched, 1, millis) };

    if ready < 0 {
        let error = std::io::Error::last_os_error();
        // A signal arriving mid-wait is not a terminal problem, and it is not
        // an answer about the terminal either.
        if error.kind() == std::io::ErrorKind::Interrupted {
            return Ok(Wait::Interrupted);
        }
        return Err(anyhow::Error::new(error).context("could not wait on the terminal"));
    }
    if ready == 0 {
        return Ok(Wait::Idle);
    }

    // Checked before `POLLIN`, and that order is the whole point: a hung-up
    // terminal reports both at once, and reading the `POLLIN` half of that is
    // what spins forever. `POLLERR` and `POLLNVAL` mean the descriptor is
    // unusable, which is the same outcome by a different route.
    if watched.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
        return Ok(Wait::HangUp);
    }
    Ok(Wait::Ready)
}

/// The descriptor crossterm reads terminal input from.
///
/// It has to be that one and not merely *a* terminal: polling anything else
/// would report the state of something crossterm is not reading. Crossterm
/// picks standard input when it is a terminal and opens `/dev/tty` otherwise
/// (`crossterm::terminal::sys::file_descriptor::tty_fd`), so this makes the
/// same choice. `None` means there is no terminal to watch at all, which
/// leaves the wait where it was.
#[cfg(unix)]
fn terminal_fd() -> Option<std::os::fd::RawFd> {
    use std::os::fd::AsRawFd;

    // SAFETY: `isatty` only inspects the descriptor.
    if unsafe { libc::isatty(libc::STDIN_FILENO) } == 1 {
        return Some(libc::STDIN_FILENO);
    }

    // Opened once and kept for the life of the process. Reopening it every
    // tick would be two syscalls per tick for a case the interactive paths
    // never reach — both of them refuse to start without a terminal on
    // standard input.
    static DEV_TTY: std::sync::OnceLock<Option<std::fs::File>> = std::sync::OnceLock::new();
    DEV_TTY
        .get_or_init(|| {
            std::fs::File::options()
                .read(true)
                .write(true)
                .open("/dev/tty")
                .ok()
        })
        .as_ref()
        .map(AsRawFd::as_raw_fd)
}

/// See the Unix implementation: this platform has no hangup answer yet, so
/// the wait stays where it always was.
#[cfg(not(unix))]
fn wait_for_terminal(_timeout: Duration) -> Result<Wait> {
    Ok(Wait::Unavailable)
}

/// Convert a crossterm event into a Glasshouse event, or `None` for events
/// that carry no meaning here.
fn translate(raw: event::Event) -> Option<Event> {
    match raw {
        event::Event::Key(key) => {
            // Key *release* and *repeat* events only arrive when the terminal
            // supports the Kitty keyboard protocol. Acting on both press and
            // release would double every keystroke.
            if key.kind == event::KeyEventKind::Release {
                None
            } else {
                Some(Event::Key(key))
            }
        }
        event::Event::Mouse(mouse) => Some(Event::Mouse(mouse)),
        event::Event::Paste(text) => Some(Event::Paste(text)),
        event::Event::Resize(cols, rows) => Some(Event::Resize(cols, rows)),
        event::Event::FocusGained | event::Event::FocusLost => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};

    fn key(kind: KeyEventKind) -> event::Event {
        event::Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('a'),
            KeyModifiers::NONE,
            kind,
        ))
    }

    #[test]
    fn key_releases_are_dropped_so_input_is_not_doubled() {
        assert!(translate(key(KeyEventKind::Release)).is_none());
        assert!(translate(key(KeyEventKind::Press)).is_some());
        assert!(translate(key(KeyEventKind::Repeat)).is_some());
    }

    #[test]
    fn focus_changes_are_not_input() {
        assert!(translate(event::Event::FocusGained).is_none());
        assert!(translate(event::Event::FocusLost).is_none());
    }

    #[test]
    fn resize_and_paste_are_forwarded() {
        assert_eq!(
            translate(event::Event::Resize(120, 40)),
            Some(Event::Resize(120, 40))
        );
        assert_eq!(
            translate(event::Event::Paste("hi".into())),
            Some(Event::Paste("hi".into()))
        );
    }

    #[test]
    fn events_sent_from_another_thread_are_delivered() {
        let source = EventSource::new(Duration::from_millis(50));
        let sender = source.sender();

        // Joined before `next` is called, so the event is definitely queued and
        // the assertion cannot depend on thread scheduling. It also means
        // `next` returns from the channel without ever polling the terminal,
        // which is what lets this run under a test harness with no tty.
        std::thread::spawn(move || sender.send(AppEvent::Redraw).expect("send"))
            .join()
            .expect("collector thread");

        assert_eq!(
            source.next().expect("next"),
            Event::App(AppEvent::Redraw),
            "an event already queued by another thread must be delivered \
             before the terminal is polled"
        );
    }

    #[test]
    fn a_pending_shutdown_short_circuits_before_any_terminal_access() {
        // Not calling `request_shutdown` here: it is process-global and would
        // leak into every other test in this binary. Instead assert the
        // ordering that makes the short circuit possible — the shutdown check
        // is the first thing `next` does, so it cannot be blocked by a
        // terminal that is unavailable.
        assert!(!crate::shutdown::shutdown_requested());
    }
}
