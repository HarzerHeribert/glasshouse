//! Several live harness sessions at once.
//!
//! [`crate::session::attach`] runs exactly one harness and gives it the user's
//! terminal for the whole of its life. That is the right shape for
//! `glasshouse launch` and the wrong shape for an interface that shows several
//! sessions: its input pump cannot be cancelled, so it relies on the process
//! exiting out from under it, and nothing else can have the keyboard meanwhile.
//!
//! [`SessionRuntime`] is the other shape. It owns any number of live
//! [`LiveSession`]s, each with its own reader thread draining the pseudo-
//! terminal into its own bounded [`Scrollback`]. Every session keeps running
//! whether or not anyone is looking at it, and focus is *only* a statement
//! about which one the keyboard reaches — changing it never touches a process.
//!
//! Two consequences worth being explicit about, because they are the whole
//! point:
//!
//! - **Output is never lost while a session is unfocused.** Each session's
//!   reader thread runs continuously and independently; the buffer is what the
//!   viewport reads from when the session is brought forward.
//! - **A session's exit is detected from the process, not from its output.** A
//!   harness that dies silently is noticed exactly as fast as one that prints a
//!   farewell, because [`SessionRuntime::poll_exits`] asks the process.

use std::collections::VecDeque;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};

use crate::launch::HarnessLaunch;
use crate::pty::{ExitStatus, PtyOutput, PtyProcess, TerminalSize};
use crate::session::{SessionId, SessionPresentation};

/// How much of each session's output is kept by default.
///
/// Bounded on purpose: a harness left running for a day would otherwise grow
/// this without limit, and a session runtime that leaks memory in proportion to
/// how useful it has been is not one anyone can leave open.
pub const DEFAULT_SCROLLBACK_BYTES: usize = 256 * 1024;

/// Size of one read from a pseudo-terminal.
const READ_CHUNK: usize = 8 * 1024;

/// A bounded record of what a session has printed.
///
/// Bytes rather than lines or parsed cells: Glasshouse does not emulate a
/// terminal, and a byte buffer keeps escape sequences intact for whatever
/// eventually renders them. When the cap is reached the oldest bytes go, which
/// can leave a partial UTF-8 sequence at the front — [`Scrollback::text`]
/// handles that rather than pretending it cannot happen.
#[derive(Debug)]
pub struct Scrollback {
    bytes: VecDeque<u8>,
    capacity: usize,
    dropped: u64,
}

impl Scrollback {
    pub fn new(capacity: usize) -> Self {
        Self {
            bytes: VecDeque::new(),
            capacity,
            dropped: 0,
        }
    }

    /// Append output, discarding the oldest bytes if that exceeds the cap.
    pub fn push(&mut self, chunk: &[u8]) {
        // A chunk larger than the whole buffer keeps only its tail: the most
        // recent output is the part worth having.
        let chunk = if chunk.len() > self.capacity {
            let skipped = chunk.len() - self.capacity;
            self.dropped += skipped as u64;
            &chunk[skipped..]
        } else {
            chunk
        };

        let overflow = (self.bytes.len() + chunk.len()).saturating_sub(self.capacity);
        for _ in 0..overflow {
            self.bytes.pop_front();
        }
        self.dropped += overflow as u64;
        self.bytes.extend(chunk.iter().copied());
    }

    /// Everything currently held, as text.
    ///
    /// Dropping the oldest bytes can sever a multi-byte character, so the front
    /// is advanced to the next UTF-8 boundary before decoding. Anything still
    /// invalid — a harness emitting genuine binary — is replaced rather than
    /// refused, because a scrollback that returns an error instead of the
    /// session's output would be useless exactly when it is most needed.
    pub fn text(&self) -> String {
        let (front, back) = self.bytes.as_slices();
        let mut all = Vec::with_capacity(self.bytes.len());
        all.extend_from_slice(front);
        all.extend_from_slice(back);

        let start = all
            .iter()
            .position(|byte| !is_utf8_continuation(*byte))
            .unwrap_or(all.len());
        String::from_utf8_lossy(&all[start..]).into_owned()
    }

    /// Bytes currently held.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// How many bytes have been discarded to stay within the cap.
    ///
    /// Exposed so a viewport can tell the user its history is incomplete rather
    /// than silently showing a truncated session.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

/// True for a byte that can only appear in the middle of a UTF-8 character.
fn is_utf8_continuation(byte: u8) -> bool {
    byte & 0b1100_0000 == 0b1000_0000
}

/// One running harness.
pub struct LiveSession {
    id: SessionId,
    process: PtyProcess,
    scrollback: Arc<Mutex<Scrollback>>,
    /// Set by the reader thread when the pseudo-terminal reports end-of-file.
    /// Distinct from the process having exited: output can end first, and a
    /// process can exit while output is still buffered.
    output_ended: Arc<AtomicBool>,
    presentation: SessionPresentation,
    exit: Option<ExitStatus>,
}

impl std::fmt::Debug for LiveSession {
    /// Hand-written to keep the session's output out of it. A `Debug` that
    /// printed the scrollback would put whatever the harness said — including
    /// anything a user pasted into it — into logs and panic messages.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveSession")
            .field("id", &self.id)
            .field("presentation", &self.presentation)
            .field("exited", &self.exit.is_some())
            .finish_non_exhaustive()
    }
}

impl LiveSession {
    pub fn id(&self) -> &SessionId {
        &self.id
    }

    pub fn presentation(&self) -> SessionPresentation {
        self.presentation
    }

    /// The exit status, once the process has been observed to end.
    pub fn exit(&self) -> Option<&ExitStatus> {
        self.exit.as_ref()
    }

    pub fn is_running(&self) -> bool {
        self.exit.is_none()
    }

    /// Everything the session has printed, within the scrollback bound.
    pub fn scrollback(&self) -> String {
        self.with_scrollback(Scrollback::text)
    }

    /// Read something from the scrollback without copying all of it out.
    pub fn with_scrollback<T>(&self, read: impl FnOnce(&Scrollback) -> T) -> T {
        match self.scrollback.lock() {
            Ok(scrollback) => read(&scrollback),
            // The reader thread panicking must not take the session with it:
            // the process is still running and still steerable, and an empty
            // view of its history is better than an unusable runtime.
            Err(poisoned) => read(&poisoned.into_inner()),
        }
    }

    /// True once the pseudo-terminal has no more output to give.
    pub fn output_ended(&self) -> bool {
        self.output_ended.load(Ordering::SeqCst)
    }

    pub fn process_id(&self) -> Option<u32> {
        self.process.process_id()
    }
}

/// Every live session in one project, and which of them has the keyboard.
pub struct SessionRuntime {
    sessions: Vec<LiveSession>,
    focused: Option<SessionId>,
    scrollback_bytes: usize,
}

impl std::fmt::Debug for SessionRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionRuntime")
            .field("sessions", &self.sessions.len())
            .field("focused", &self.focused)
            .finish_non_exhaustive()
    }
}

impl Default for SessionRuntime {
    fn default() -> Self {
        Self::new()
    }
}

/// Why an operation on a session could not happen.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("session `{id}` is not running in this Glasshouse")]
    NotLive { id: SessionId },
    #[error("session `{id}` has already exited")]
    Exited { id: SessionId },
    #[error(
        "session `{id}` is headless and has no viewport to bring forward; \
         change its presentation first"
    )]
    Headless { id: SessionId },
    #[error("could not {action} session `{id}`")]
    Io {
        id: SessionId,
        action: &'static str,
        #[source]
        source: std::io::Error,
    },
}

impl SessionRuntime {
    pub fn new() -> Self {
        Self::with_scrollback_bytes(DEFAULT_SCROLLBACK_BYTES)
    }

    /// A runtime whose sessions keep `scrollback_bytes` of output each.
    pub fn with_scrollback_bytes(scrollback_bytes: usize) -> Self {
        Self {
            sessions: Vec::new(),
            focused: None,
            scrollback_bytes,
        }
    }

    /// Start a harness and keep it.
    ///
    /// The launch carries the project-derived working directory, so a session
    /// started here is bound to the active project exactly as one started by
    /// `glasshouse launch` is. The first session that can hold the keyboard
    /// takes focus; a headless one never does.
    pub fn start(
        &mut self,
        id: SessionId,
        presentation: SessionPresentation,
        launch: &HarnessLaunch<'_>,
    ) -> Result<&LiveSession> {
        let (process, output) = launch.spawn()?;
        let scrollback = Arc::new(Mutex::new(Scrollback::new(self.scrollback_bytes)));
        let output_ended = Arc::new(AtomicBool::new(false));

        {
            let scrollback = Arc::clone(&scrollback);
            let output_ended = Arc::clone(&output_ended);
            let name = format!("glasshouse-session-{}", short(&id));
            std::thread::Builder::new()
                .name(name)
                .spawn(move || pump(output, &scrollback, &output_ended))
                .context("could not start the session output reader")?;
        }

        let focusable = presentation != SessionPresentation::Headless;
        self.sessions.push(LiveSession {
            id: id.clone(),
            process,
            scrollback,
            output_ended,
            presentation,
            exit: None,
        });
        if self.focused.is_none() && focusable {
            self.focused = Some(id.clone());
        }

        Ok(self.sessions.last().expect("the session was just pushed"))
    }

    pub fn sessions(&self) -> &[LiveSession] {
        &self.sessions
    }

    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    pub fn get(&self, id: &SessionId) -> Option<&LiveSession> {
        self.sessions.iter().find(|session| &session.id == id)
    }

    fn get_mut(&mut self, id: &SessionId) -> Result<&mut LiveSession, RuntimeError> {
        self.sessions
            .iter_mut()
            .find(|session| &session.id == id)
            .ok_or_else(|| RuntimeError::NotLive { id: id.clone() })
    }

    /// Which session the keyboard reaches, if any.
    pub fn focused(&self) -> Option<&SessionId> {
        self.focused.as_ref()
    }

    /// Bring a session forward.
    ///
    /// This changes **only** which session receives keystrokes. Every other
    /// session keeps running, keeps producing output, and keeps filling its own
    /// scrollback; nothing is suspended, restarted, or signalled. That is the
    /// property the whole multi-session model rests on, so it is worth stating
    /// where the code does it.
    pub fn focus(&mut self, id: &SessionId) -> Result<(), RuntimeError> {
        let session = self.get_mut(id)?;
        if session.presentation == SessionPresentation::Headless {
            return Err(RuntimeError::Headless { id: id.clone() });
        }
        self.focused = Some(id.clone());
        Ok(())
    }

    /// Forward raw keystrokes to the focused session.
    ///
    /// Returns `Ok(false)` when nothing has focus, which is not an error: an
    /// interface with no session in the viewport still receives key events, and
    /// they simply have nowhere to go.
    pub fn write_to_focused(&mut self, bytes: &[u8]) -> Result<bool, RuntimeError> {
        let Some(id) = self.focused.clone() else {
            return Ok(false);
        };
        self.write_input(&id, bytes)?;
        Ok(true)
    }

    /// Send raw bytes to a session, focused or not.
    pub fn write_input(&mut self, id: &SessionId, bytes: &[u8]) -> Result<(), RuntimeError> {
        let session = self.get_mut(id)?;
        if session.exit.is_some() {
            return Err(RuntimeError::Exited { id: id.clone() });
        }
        session
            .process
            .write_input(bytes)
            .map_err(|source| RuntimeError::Io {
                id: id.clone(),
                action: "write to",
                source,
            })
    }

    /// Send text to a session without needing it in the viewport.
    ///
    /// This is what lets an orchestrator drive a worker the user is not looking
    /// at, and it deliberately does not change focus: a message arriving in a
    /// background session must not yank the user out of the one they are in.
    pub fn send_text(&mut self, id: &SessionId, text: &str) -> Result<(), RuntimeError> {
        self.write_input(id, text.as_bytes())
    }

    /// Interrupt a session, focused or not.
    pub fn interrupt(&mut self, id: &SessionId) -> Result<(), RuntimeError> {
        let session = self.get_mut(id)?;
        if session.exit.is_some() {
            return Err(RuntimeError::Exited { id: id.clone() });
        }
        session
            .process
            .interrupt()
            .map_err(|source| RuntimeError::Io {
                id: id.clone(),
                action: "interrupt",
                source,
            })
    }

    /// Tell a session's pseudo-terminal its window changed size.
    pub fn resize(&mut self, id: &SessionId, size: TerminalSize) -> Result<()> {
        let session = self
            .sessions
            .iter_mut()
            .find(|session| &session.id == id)
            .ok_or_else(|| anyhow::anyhow!("session `{id}` is not running in this Glasshouse"))?;
        session.process.resize(size)
    }

    /// Notice any session whose process has ended since the last call.
    ///
    /// Asked of the process, never inferred from its output going quiet — a
    /// harness can be silent for minutes while thinking, and treating that as
    /// death is the classic way a session manager kills work in progress.
    /// Each exit is reported exactly once; the session stays in the runtime
    /// afterwards so its final output remains readable.
    pub fn poll_exits(&mut self) -> Vec<(SessionId, ExitStatus)> {
        let mut ended = Vec::new();
        for session in &mut self.sessions {
            if session.exit.is_some() {
                continue;
            }
            match session.process.try_wait() {
                Ok(Some(status)) => {
                    session.exit = Some(status.clone());
                    ended.push((session.id.clone(), status));
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(session = %session.id, %error, "could not check on a session");
                }
            }
        }

        // Focus must not stay on something that is over, or the keyboard would
        // vanish into a dead session.
        if let Some(focused) = self.focused.clone()
            && ended.iter().any(|(id, _)| id == &focused)
        {
            self.focused = self
                .sessions
                .iter()
                .find(|session| {
                    session.is_running() && session.presentation != SessionPresentation::Headless
                })
                .map(|session| session.id.clone());
        }

        ended
    }

    /// Stop a session and forget it.
    ///
    /// Best effort on the signal: a process that has already gone is not a
    /// failure to close.
    pub fn close(&mut self, id: &SessionId) -> Result<(), RuntimeError> {
        let index = self
            .sessions
            .iter()
            .position(|session| &session.id == id)
            .ok_or_else(|| RuntimeError::NotLive { id: id.clone() })?;

        let mut session = self.sessions.remove(index);
        if session.exit.is_none() {
            let _ = session.process.signal(crate::pty::ProcessSignal::Kill);
        }

        if self.focused.as_ref() == Some(id) {
            self.focused = self
                .sessions
                .iter()
                .find(|session| {
                    session.is_running() && session.presentation != SessionPresentation::Headless
                })
                .map(|session| session.id.clone());
        }
        Ok(())
    }
}

/// Drain a pseudo-terminal into a scrollback until it has nothing left.
fn pump(mut output: PtyOutput, scrollback: &Mutex<Scrollback>, ended: &AtomicBool) {
    let mut buffer = [0u8; READ_CHUNK];
    loop {
        // `Ok(0)` and an error mean the same thing: nothing more is coming. A
        // pseudo-terminal reports the end of a session as end-of-file on some
        // platforms and as a read error on others, and neither is a fault —
        // the exit status comes from the process, not from here.
        let read = match output.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        match scrollback.lock() {
            Ok(mut scrollback) => scrollback.push(&buffer[..read]),
            // Another thread panicked holding the lock. Keep draining anyway:
            // stopping would block the pseudo-terminal and eventually the
            // harness itself, which is far worse than a gap in the history.
            Err(poisoned) => poisoned.into_inner().push(&buffer[..read]),
        }
    }
    ended.store(true, Ordering::SeqCst);
}

fn short(id: &SessionId) -> String {
    id.as_str().chars().take(8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrollback_keeps_everything_within_its_bound() {
        let mut scrollback = Scrollback::new(16);
        scrollback.push(b"hello ");
        scrollback.push(b"world");
        assert_eq!(scrollback.text(), "hello world");
        assert_eq!(scrollback.dropped(), 0);
        assert_eq!(scrollback.len(), 11);
    }

    #[test]
    fn scrollback_discards_the_oldest_output_when_full() {
        let mut scrollback = Scrollback::new(8);
        scrollback.push(b"abcdefgh");
        assert_eq!(scrollback.text(), "abcdefgh");

        scrollback.push(b"ij");
        assert_eq!(scrollback.text(), "cdefghij", "the oldest bytes go first");
        assert_eq!(scrollback.dropped(), 2);
        assert_eq!(scrollback.len(), 8, "never grows past its bound");
    }

    /// A single write larger than the whole buffer must keep its tail, not its
    /// head: the most recent output is the part worth having.
    #[test]
    fn a_chunk_larger_than_the_buffer_keeps_its_most_recent_end() {
        let mut scrollback = Scrollback::new(4);
        scrollback.push(b"abcdefghij");
        assert_eq!(scrollback.text(), "ghij");
        assert_eq!(scrollback.len(), 4);
        assert_eq!(scrollback.dropped(), 6);
    }

    /// Dropping bytes can sever a multi-byte character. The buffer must not
    /// then produce a replacement character at the front where a real one was
    /// cut — it advances to the next boundary instead.
    #[test]
    fn a_severed_multibyte_character_is_dropped_rather_than_mangled() {
        // "ä" is two bytes; a four-byte buffer holds "äx" plus one more byte.
        let mut scrollback = Scrollback::new(4);
        scrollback.push("äbc".as_bytes()); // 2 + 1 + 1 = 4 bytes exactly
        assert_eq!(scrollback.text(), "äbc");

        // One more byte evicts the first half of "ä", leaving an orphan
        // continuation byte at the front.
        scrollback.push(b"d");
        let text = scrollback.text();
        assert_eq!(
            text, "bcd",
            "the half character must be dropped, not mangled"
        );
        assert!(
            !text.contains('\u{FFFD}'),
            "no replacement character: {text:?}"
        );
    }

    #[test]
    fn scrollback_keeps_escape_sequences_intact() {
        let mut scrollback = Scrollback::new(64);
        scrollback.push(b"\x1b[31mred\x1b[0m");
        assert_eq!(
            scrollback.text(),
            "\u{1b}[31mred\u{1b}[0m",
            "Glasshouse does not emulate a terminal, so sequences pass through"
        );
    }

    #[test]
    fn a_zero_capacity_scrollback_keeps_nothing_and_does_not_panic() {
        let mut scrollback = Scrollback::new(0);
        scrollback.push(b"anything");
        assert!(scrollback.is_empty());
        assert_eq!(scrollback.text(), "");
        assert_eq!(scrollback.dropped(), 8);
    }

    #[test]
    fn genuinely_invalid_bytes_are_replaced_rather_than_refused() {
        let mut scrollback = Scrollback::new(16);
        // 0xC3 starts a two-byte character; 0x28 cannot continue it.
        scrollback.push(&[b'a', 0xC3, 0x28, b'b']);
        let text = scrollback.text();
        assert!(text.starts_with('a') && text.ends_with('b'), "got {text:?}");
    }

    #[test]
    fn an_empty_runtime_has_nothing_focused() {
        let runtime = SessionRuntime::new();
        assert!(runtime.is_empty());
        assert_eq!(runtime.focused(), None);
        assert!(runtime.get(&SessionId::new("nope")).is_none());
    }

    #[test]
    fn operations_on_an_unknown_session_are_refused_by_name() {
        let mut runtime = SessionRuntime::new();
        let id = SessionId::new("ghost");
        assert!(matches!(
            runtime.send_text(&id, "hi"),
            Err(RuntimeError::NotLive { .. })
        ));
        assert!(matches!(
            runtime.interrupt(&id),
            Err(RuntimeError::NotLive { .. })
        ));
        assert!(matches!(
            runtime.focus(&id),
            Err(RuntimeError::NotLive { .. })
        ));
        assert!(matches!(
            runtime.close(&id),
            Err(RuntimeError::NotLive { .. })
        ));
    }

    /// Keys arriving with nothing in the viewport are not an error — an
    /// interface still receives them, they simply have nowhere to go.
    #[test]
    fn keystrokes_with_nothing_focused_are_dropped_not_failed() {
        let mut runtime = SessionRuntime::new();
        assert!(!runtime.write_to_focused(b"x").unwrap());
    }
}
