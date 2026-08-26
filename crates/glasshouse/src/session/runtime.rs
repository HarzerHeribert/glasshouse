//! Several live harness sessions at once.
//!
//! [`fn@crate::session::attach`] runs exactly one harness and gives it the user's
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
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};

use crate::events::{EventBus, LifecycleEvent, MessageOrigin, ProcessExit, RecordedEvent};
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

/// How long [`SessionRuntime::crash_report`] will wait for a dead session's
/// reader thread to finish before reporting what it has.
///
/// A ceiling, not a delay: the wait ends the instant the reader says it is
/// done, which is the ordinary case and costs nothing measurable — the gap
/// it closes was measured at 1.1ms to 2.2ms under the full workspace suite
/// on Linux.
///
/// Deliberately the same 250ms `session::attach` allows its own output pump
/// after a harness exits, and for the same two reasons. Waiting is necessary
/// because a process's death outruns its last words; the *bound* is
/// necessary because on Windows no end-of-file ever arrives while the pty is
/// open, so there is nothing else to end the wait — a session whose output
/// simply never ends would otherwise hang whoever asked about it. See
/// [`SessionRuntime::crash_report`].
const OUTPUT_DRAIN_WAIT: Duration = Duration::from_millis(250);

/// "The pseudo-terminal has nothing more to give", and a way to wait for it.
///
/// A plain flag would be enough to *ask*; this exists to be *waited on*. The
/// only thread that knows a session's output has finished is its reader, and
/// the only way another thread can learn it promptly — rather than by
/// sleeping and looking again, which is a guess wearing a number — is to be
/// woken. See [`SessionRuntime::crash_report`], the one caller that waits.
#[derive(Default)]
struct OutputEnd {
    ended: Mutex<bool>,
    changed: Condvar,
}

impl OutputEnd {
    fn ended(&self) -> bool {
        match self.ended.lock() {
            Ok(ended) => *ended,
            // A panic elsewhere must not make this unanswerable: the flag is
            // a `bool` and cannot be left half-written, so the poisoned value
            // is the real one.
            Err(poisoned) => *poisoned.into_inner(),
        }
    }

    /// Called by the reader thread, once, when the pseudo-terminal is done.
    fn finish(&self) {
        match self.ended.lock() {
            Ok(mut ended) => *ended = true,
            Err(poisoned) => *poisoned.into_inner() = true,
        }
        self.changed.notify_all();
    }

    /// Block until the reader is done or `timeout` elapses; report which.
    ///
    /// A reader that panicked will never notify, so the timeout is what makes
    /// this safe to call at all rather than a nicety.
    fn wait_until_ended(&self, timeout: Duration) -> bool {
        let Ok(ended) = self.ended.lock() else {
            // The reader thread is gone. Nothing will ever notify, so waiting
            // would only spend the whole timeout to learn that.
            return false;
        };
        match self
            .changed
            .wait_timeout_while(ended, timeout, |ended| !*ended)
        {
            Ok((ended, _)) => *ended,
            Err(poisoned) => *poisoned.into_inner().0,
        }
    }
}

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

/// A question a harness asks its terminal and waits for an answer to.
///
/// These were not chosen from a specification. A real Claude Code 2.1.245
/// startup was captured in a pseudo-terminal and every escape sequence it
/// wrote before drawing was examined; these are the ones that are questions
/// rather than instructions. The rest — bracketed paste, focus reporting,
/// synchronised output, keyboard-protocol pushes — are commands, and a
/// terminal that silently accepts them is behaving correctly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalQuery {
    /// `ESC[6n` — Device Status Report, "where is the cursor?".
    CursorPosition,
    /// `ESC[c` — Primary Device Attributes, "what kind of terminal are you?".
    DeviceAttributes,
    /// `ESC[>0q` — XTVERSION, "what program are you?".
    Version,
}

impl TerminalQuery {
    /// The byte sequence that asks it.
    const PATTERNS: [(&'static [u8], TerminalQuery); 3] = [
        (b"\x1b[6n", TerminalQuery::CursorPosition),
        (b"\x1b[c", TerminalQuery::DeviceAttributes),
        (b"\x1b[>0q", TerminalQuery::Version),
    ];
}

/// Questions Glasshouse deliberately does **not** answer.
///
/// `ESC[?u` asks whether the terminal speaks the kitty keyboard protocol.
/// Codex 0.149.0 asks it at startup, and the temptation is to reply — an
/// unanswered question is what this whole mechanism exists to prevent.
///
/// Replying would be a lie with consequences. The reply means "supported",
/// the harness would then enable the protocol and expect *key events encoded
/// that way*, and `crate::tui::event` sends ordinary bytes. The harness would
/// come up looking fine and then mis-read every keystroke.
///
/// Silence is the correct answer, and it is not a timeout: the established
/// idiom is to send `ESC[?u` and `ESC[c` together, and a device-attributes
/// reply arriving with no keyboard reply before it *is* the negative answer.
/// Codex sends exactly that pair, in that order. Answering device attributes —
/// which Glasshouse does — is therefore what lets a harness conclude "no kitty
/// protocol here" immediately rather than waiting.
///
/// Do not add a reply here without also teaching `tui::event` to encode keys
/// in the protocol being claimed.
#[cfg(test)]
const DELIBERATELY_UNANSWERED: &[&[u8]] = &[b"\x1b[?u"];

/// The longest query pattern, which is how much history the scanner keeps.
const LONGEST_QUERY: usize = 5;

/// Finds terminal queries in a byte stream, across chunk boundaries.
///
/// A query can be split by any read, so a search over each chunk in isolation
/// would miss one straddling a boundary. This keeps a rolling window of the
/// last few bytes instead, which handles every pattern at once and needs no
/// per-pattern match state.
#[derive(Debug, Default)]
struct TerminalQueryScanner {
    /// The last `LONGEST_QUERY` bytes seen, oldest first.
    window: Vec<u8>,
}

impl TerminalQueryScanner {
    /// Feed a chunk; returns the queries it completed, in order.
    fn scan(&mut self, chunk: &[u8]) -> Vec<TerminalQuery> {
        let mut found = Vec::new();
        for byte in chunk {
            if self.window.len() == LONGEST_QUERY {
                self.window.remove(0);
            }
            self.window.push(*byte);
            // Longest pattern first, so `ESC[>0q` is never mistaken for a
            // shorter suffix of itself.
            for (pattern, query) in TerminalQuery::PATTERNS {
                if self.window.ends_with(pattern) {
                    found.push(query);
                    break;
                }
            }
        }
        found
    }
}

/// One running harness.
pub struct LiveSession {
    id: SessionId,
    process: PtyProcess,
    scrollback: Arc<Mutex<Scrollback>>,
    /// The session's screen, as a terminal would have drawn it.
    ///
    /// Fed by the same reader thread that fills `scrollback`: the raw bytes
    /// remain the record of what was said, this is what it looks like.
    screen: Arc<Mutex<vt100::Parser>>,
    /// Cursor-position queries the harness has asked and nobody has answered.
    ///
    /// Counted by the reader thread and answered by whichever thread owns the
    /// process, because writing to the child needs `&mut PtyProcess` and the
    /// reader does not have it. See `SessionRuntime::answer_terminal_queries`.
    pending_queries: Arc<Mutex<Vec<TerminalQuery>>>,
    /// Set by the reader thread when the pseudo-terminal reports end-of-file.
    /// Distinct from the process having exited: output can end first, and a
    /// process can exit while output is still buffered — which is not a
    /// remark, it is the race [`SessionRuntime::crash_report`] exists to
    /// close.
    output_ended: Arc<OutputEnd>,
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

    /// Read the session's screen as a terminal would have drawn it.
    ///
    /// Borrowed rather than copied out: a screen is a grid of cells and the
    /// viewport wants to walk it, not own it.
    pub fn with_screen<T>(&self, read: impl FnOnce(&vt100::Screen) -> T) -> T {
        match self.screen.lock() {
            Ok(parser) => read(parser.screen()),
            Err(poisoned) => read(poisoned.into_inner().screen()),
        }
    }

    /// True once the pseudo-terminal has no more output to give.
    pub fn output_ended(&self) -> bool {
        self.output_ended.ended()
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
    /// The one normalized lifecycle-event stream every session here feeds.
    ///
    /// Owned by the runtime rather than passed to each call, because the
    /// thread that has to publish is not always the caller: a session's
    /// reader thread reports its own output ending, and it can only do that
    /// if it was handed a bus when it started.
    events: EventBus,
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

/// What is left of a session after its process died.
///
/// Phase 45 requires that a crash costs neither the terminal output nor the
/// event history. This is that promise as a value: everything in it was
/// already held outside the process, so producing it involves no recovery and
/// cannot itself fail.
#[derive(Debug, Clone)]
pub struct CrashReport {
    pub session: SessionId,
    /// How the process died, as the operating system reported it. Says
    /// nothing about whether the work was finished — see [`ProcessExit`].
    pub exit: ProcessExit,
    /// The session's terminal output, within the scrollback bound.
    pub output: String,
    /// Every lifecycle event recorded for this session, oldest first.
    pub history: Vec<RecordedEvent>,
}

impl SessionRuntime {
    pub fn new() -> Self {
        Self::with_scrollback_bytes(DEFAULT_SCROLLBACK_BYTES)
    }

    /// A runtime whose sessions keep `scrollback_bytes` of output each.
    pub fn with_scrollback_bytes(scrollback_bytes: usize) -> Self {
        Self::with_event_bus(scrollback_bytes, EventBus::new())
    }

    /// A runtime publishing onto an event bus somebody else already owns.
    ///
    /// [`EventBus`] is cheap to clone and shares one stream, so this is how a
    /// caller that already has consumers attached — a TUI, an orchestrator —
    /// gets this runtime's events without having to subscribe twice.
    pub fn with_event_bus(scrollback_bytes: usize, events: EventBus) -> Self {
        Self {
            sessions: Vec::new(),
            focused: None,
            scrollback_bytes,
            events,
        }
    }

    /// The lifecycle events this runtime's sessions produce.
    pub fn events(&self) -> &EventBus {
        &self.events
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
        let output_ended = Arc::new(OutputEnd::default());
        // Read back from the process rather than from the launch: this is the
        // size the pseudo-terminal actually got, which is what the harness will
        // be drawing for.
        let size = process.size();
        let screen = Arc::new(Mutex::new(vt100::Parser::new(size.rows, size.cols, 0)));
        let pending_queries = Arc::new(Mutex::new(Vec::new()));

        {
            let scrollback = Arc::clone(&scrollback);
            let screen = Arc::clone(&screen);
            let pending_queries = Arc::clone(&pending_queries);
            let output_ended = Arc::clone(&output_ended);
            let name = format!("glasshouse-session-{}", short(&id));
            // The bus travels into the reader thread rather than being
            // consulted from the outside, because this thread is the only one
            // that knows when the pseudo-terminal stopped giving output. It is
            // also the thread that must never wait: a reader that blocks stops
            // draining the terminal, whose buffer then fills, and the harness
            // itself blocks on `write`. `EventBus::publish` is bounded work
            // with no wait on any consumer, which is what makes putting it
            // here safe — see `crate::events::bus`.
            let events = self.events.clone();
            let session = id.clone();
            std::thread::Builder::new()
                .name(name)
                .spawn(move || {
                    pump(
                        output,
                        &scrollback,
                        &screen,
                        &pending_queries,
                        &output_ended,
                        &events,
                        &session,
                    )
                })
                .context("could not start the session output reader")?;
        }

        let focusable = presentation != SessionPresentation::Headless;
        self.sessions.push(LiveSession {
            id: id.clone(),
            process,
            scrollback,
            screen,
            pending_queries,
            output_ended,
            presentation,
            exit: None,
        });
        if self.focused.is_none() && focusable {
            self.focused = Some(id.clone());
        }

        self.events.publish(&id, LifecycleEvent::SessionStarted);

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
        // The keyboard reached this session, so a person is at the other end
        // of it. Recorded as such, and never merged with a machine-sent line:
        // the harness cannot tell them apart, which is exactly why Glasshouse
        // has to.
        self.events.publish(
            &id,
            LifecycleEvent::TextDelivered {
                origin: MessageOrigin::UserKeystroke,
                bytes: bytes.len(),
            },
        );
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
        self.send_text_from(id, text, MessageOrigin::UserKeystroke)
    }

    /// Send text to a session and record who sent it.
    ///
    /// [`SessionRuntime::send_text`] is the same call with
    /// [`MessageOrigin::UserKeystroke`], because its callers are places a
    /// person typed something — the shell's send-a-line prompt, and the
    /// keyboard. Anything Glasshouse originates goes through here with
    /// [`MessageOrigin::Machine`] instead, and the two are separate records
    /// for the whole life of the event log.
    ///
    /// The origin is a parameter rather than something inferred from the
    /// text or the caller's stack, because there is no way to infer it. A
    /// machine-sent line and a typed one are identical bytes.
    pub fn send_text_from(
        &mut self,
        id: &SessionId,
        text: &str,
        origin: MessageOrigin,
    ) -> Result<(), RuntimeError> {
        self.write_input(id, text.as_bytes())?;
        self.events.publish(
            id,
            LifecycleEvent::TextDelivered {
                origin,
                bytes: text.len(),
            },
        );
        Ok(())
    }

    /// Interrupt a session, focused or not.
    pub fn interrupt(&mut self, id: &SessionId) -> Result<(), RuntimeError> {
        self.interrupt_from(id, MessageOrigin::UserKeystroke)
    }

    /// Interrupt a session and record who asked for it.
    ///
    /// See [`SessionRuntime::send_text_from`] for why the origin is passed
    /// rather than inferred.
    pub fn interrupt_from(
        &mut self,
        id: &SessionId,
        origin: MessageOrigin,
    ) -> Result<(), RuntimeError> {
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
            })?;
        self.events
            .publish(id, LifecycleEvent::InterruptDelivered { origin });
        Ok(())
    }

    /// Tell a session's pseudo-terminal its window changed size.
    pub fn resize(&mut self, id: &SessionId, size: TerminalSize) -> Result<()> {
        let session = self
            .sessions
            .iter_mut()
            .find(|session| &session.id == id)
            .ok_or_else(|| anyhow::anyhow!("session `{id}` is not running in this Glasshouse"))?;
        // Both, or the harness draws for one shape while Glasshouse renders
        // another: the child is told through its pseudo-terminal, the emulator
        // is told directly, and they must agree.
        match session.screen.lock() {
            Ok(mut parser) => parser.screen_mut().set_size(size.rows, size.cols),
            Err(poisoned) => poisoned
                .into_inner()
                .screen_mut()
                .set_size(size.rows, size.cols),
        }
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
                    // One session Glasshouse cannot ask about must not cost
                    // the others their poll. This loop has no `?` in it for
                    // that reason: a failed worker cannot take unrelated
                    // sessions, or the Glasshouse instance, down with it.
                    tracing::warn!(session = %session.id, %error, "could not check on a session");
                }
            }
        }

        // Published after the loop so that every session has been asked
        // before any consumer is told about any of them. An event a
        // subscriber acts on must not describe a half-polled runtime.
        for (id, status) in &ended {
            self.events.publish(
                id,
                LifecycleEvent::ProcessExited {
                    exit: ProcessExit::from_status(status),
                },
            );
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

    /// Answer the terminal questions the sessions have asked.
    ///
    /// **An embedded session inverts `session::attach`'s rule.** `attach` is a
    /// pass-through and must never answer, because the user's real terminal is
    /// on the other end and will; a second reply would reach the harness as
    /// input. Here Glasshouse *is* the terminal — the output goes into a buffer
    /// it owns and is redrawn into a viewport, and no real terminal ever sees
    /// the question. Nothing else can answer, so a harness that waits for a
    /// reply waits forever.
    ///
    /// **Waiting forever is not the only way this hurts.** A harness that
    /// gives up on an unanswered question may not merely degrade for that
    /// session: Claude Code counts the failures and, after two, disables its
    /// fullscreen renderer *globally*, writing that decision into the user's
    /// own configuration where it outlives Glasshouse entirely. Answering is
    /// therefore not a nicety, it is the difference between embedding a
    /// harness and quietly damaging it.
    ///
    /// Called from the interface's tick. Best effort per session: one harness
    /// that cannot be written to must not stop the others being answered.
    pub fn answer_terminal_queries(&mut self) {
        for session in &mut self.sessions {
            if session.exit.is_some() {
                continue;
            }
            let pending: Vec<TerminalQuery> = match session.pending_queries.lock() {
                Ok(mut queue) => std::mem::take(&mut *queue),
                Err(poisoned) => std::mem::take(&mut *poisoned.into_inner()),
            };
            if pending.is_empty() {
                continue;
            }

            for query in pending {
                let reply = match query {
                    TerminalQuery::CursorPosition => {
                        // vt100 reports zero-based; a Device Status Report is
                        // one-based. The position is the *emulated* screen's,
                        // which is the screen the harness actually has —
                        // reporting the outer terminal's would answer a
                        // question it did not ask.
                        let (row, col) = match session.screen.lock() {
                            Ok(parser) => parser.screen().cursor_position(),
                            Err(poisoned) => poisoned.into_inner().screen().cursor_position(),
                        };
                        format!("\x1b[{};{}R", row + 1, col + 1)
                    }
                    // "VT100 with Advanced Video Option", which is what the
                    // emulator behind the viewport actually is. Claiming a
                    // richer terminal would invite a harness to use sequences
                    // the viewport cannot draw.
                    TerminalQuery::DeviceAttributes => "\x1b[?1;2c".to_owned(),
                    // XTVERSION. Glasshouse answers with its own name rather
                    // than impersonating a terminal it is not: an application
                    // that recognises the name can decide for itself, and one
                    // that does not falls back to conservative defaults, which
                    // is the correct outcome either way.
                    TerminalQuery::Version => {
                        format!("\x1bP>|Glasshouse({})\x1b\\", crate::VERSION)
                    }
                };

                if let Err(error) = session.process.write_input(reply.as_bytes()) {
                    tracing::debug!(
                        session = %session.id,
                        ?query,
                        %error,
                        "could not answer a terminal query"
                    );
                    break;
                }
            }
        }
    }

    /// Everything that survived a crash, or `None` if this session did not
    /// crash.
    ///
    /// A crashed worker's terminal output and event history outlive it,
    /// because neither belongs to the process: the scrollback is Glasshouse's
    /// buffer and the history is the project's bus. The session stays in the
    /// runtime after it exits for exactly this reason — removing it would be
    /// the only way to lose the output, and `poll_exits` deliberately does
    /// not.
    ///
    /// `None` for a session that is running, that exited on its own terms, or
    /// that Glasshouse closed itself: [`SessionRuntime::close`] removes the
    /// session before it signals, so a deliberate kill is never reported as a
    /// crash.
    ///
    /// # Why this waits
    ///
    /// **A process's exit becomes observable before its last output does.**
    /// The exit comes from `waitpid`; the output has to travel through the
    /// pseudo-terminal and be copied into the scrollback by this session's
    /// reader thread, which is a *different* thread that may not have run
    /// yet. Asking `poll_exits` and then reading the scrollback in the same
    /// breath therefore reports a crashed worker as having said nothing —
    /// which is what the Linux gate had been failing on at random for weeks,
    /// and which reproduced at 8 runs in 17 beside the full workspace suite —
    /// see `docs/product/design-decisions.md`, "A pseudo-terminal child's exit
    /// is observable before its output is".
    ///
    /// `session::attach` — the other shape a harness runs in — has always
    /// done this, and says so in `OUTPUT_DRAIN_GRACE`. This path is the one
    /// that had not learned it.
    ///
    /// Nothing is ever lost when that happens: the bytes are in the kernel's
    /// pty buffer and arrive about two milliseconds later. Linux hands a
    /// reader everything that was written before it reports `EIO`, and a
    /// probe of 200 trials per timing confirmed it never drops a byte, even
    /// when the child is reaped before the first read. So this is not data
    /// loss — it is a post-mortem written before the body stopped talking,
    /// and the fix is to let it finish.
    ///
    /// # Why the wait is bounded
    ///
    /// Output is not guaranteed to end at all. A harness that crashed after
    /// starting something of its own leaves that grandchild holding the pty
    /// slave open, and the reader will sit there for as long as it lives —
    /// see [`crate::pty::PtyOutput`], which records the same lifetime rule
    /// from the other side. An unbounded wait here would hang a caller on
    /// exactly the crash it most needs reporting, so the report is produced
    /// either way and the ceiling is 250ms — the same grace `session::attach`
    /// allows its own pump.
    pub fn crash_report(&self, id: &SessionId) -> Option<CrashReport> {
        let session = self.get(id)?;
        let exit = ProcessExit::from_status(session.exit()?);
        if !exit.is_crash() {
            return None;
        }
        if !session.output_ended.wait_until_ended(OUTPUT_DRAIN_WAIT) {
            tracing::debug!(
                session = %id,
                "reporting a crash before its output ended; something still holds the terminal"
            );
        }
        Some(CrashReport {
            session: id.clone(),
            exit,
            output: session.scrollback(),
            history: self.events.history_for(id),
        })
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
fn pump(
    mut output: PtyOutput,
    scrollback: &Mutex<Scrollback>,
    screen: &Mutex<vt100::Parser>,
    pending_queries: &Mutex<Vec<TerminalQuery>>,
    ended: &OutputEnd,
    events: &EventBus,
    session: &SessionId,
) {
    let mut buffer = [0u8; READ_CHUNK];
    let mut scanner = TerminalQueryScanner::default();
    loop {
        // `Ok(0)` and an error mean the same thing: nothing more is coming. A
        // pseudo-terminal reports the end of a session as end-of-file on some
        // platforms and as a read error on others, and neither is a fault —
        // the exit status comes from the process, not from here.
        let read = match output.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let chunk = &buffer[..read];

        match scrollback.lock() {
            Ok(mut scrollback) => scrollback.push(chunk),
            // Another thread panicked holding the lock. Keep draining anyway:
            // stopping would block the pseudo-terminal and eventually the
            // harness itself, which is far worse than a gap in the history.
            Err(poisoned) => poisoned.into_inner().push(chunk),
        }

        match screen.lock() {
            Ok(mut screen) => screen.process(chunk),
            Err(poisoned) => poisoned.into_inner().process(chunk),
        }

        // Counted here, answered elsewhere: this thread cannot write to the
        // child. An unanswered query is a harness that waits forever, so the
        // count must not be lost even if the owner is slow to notice.
        let found = scanner.scan(chunk);
        if !found.is_empty() {
            match pending_queries.lock() {
                Ok(mut queue) => queue.extend(found),
                Err(poisoned) => poisoned.into_inner().extend(found),
            }
        }
    }
    ended.finish();
    // A statement about a file descriptor, and nothing more. Publishing it as
    // its own event — rather than folding it into an exit, or letting a
    // consumer time the silence — is what keeps "the output stopped" from
    // ever being read as "the work finished".
    events.publish(session, LifecycleEvent::OutputEnded);
}

fn short(id: &SessionId) -> String {
    id.as_str().chars().take(8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wait `crash_report` depends on costs nothing when there is
    /// nothing to wait for, which is the ordinary case and the one that must
    /// not slow a caller down.
    #[test]
    fn waiting_on_output_that_has_already_ended_returns_at_once() {
        let end = OutputEnd::default();
        end.finish();

        let started = std::time::Instant::now();
        assert!(end.wait_until_ended(Duration::from_secs(30)));
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "an already-finished reader must not be waited for: {:?}",
            started.elapsed()
        );
    }

    /// Output is not guaranteed to end: a crashed harness can leave something
    /// of its own holding the pseudo-terminal open. The bound is what keeps
    /// that from hanging whoever asked for the crash report.
    #[test]
    fn waiting_on_output_that_never_ends_gives_up_rather_than_hanging() {
        let end = OutputEnd::default();

        let started = std::time::Instant::now();
        assert!(!end.wait_until_ended(Duration::from_millis(50)));
        assert!(
            started.elapsed() >= Duration::from_millis(50),
            "it gave up before the bound: {:?}",
            started.elapsed()
        );
        assert!(!end.ended());
    }

    /// And the wait really is a wait, not a sleep of the full bound: a
    /// reader finishing wakes it. Asserting only the returned `true` would
    /// pass just as well against a timeout, which is the whole thing this
    /// distinguishes.
    #[test]
    fn a_waiter_is_woken_by_the_reader_finishing_rather_than_by_the_bound() {
        const BOUND: Duration = Duration::from_secs(30);
        let end = Arc::new(OutputEnd::default());

        let reader = Arc::clone(&end);
        let finishing = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            reader.finish();
        });

        let started = std::time::Instant::now();
        assert!(end.wait_until_ended(BOUND));
        let waited = started.elapsed();
        finishing.join().expect("the finishing thread");

        assert!(
            waited < BOUND / 2,
            "the bound elapsed instead of the reader waking it: {waited:?}"
        );
    }

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

    /// The whole reason the scanner keeps state: a read can split a query
    /// anywhere, and a per-chunk search would miss one straddling the seam.
    #[test]
    fn a_query_is_found_however_a_read_splits_it() {
        for (pattern, expected) in TerminalQuery::PATTERNS {
            for split in 0..=pattern.len() {
                let mut scanner = TerminalQueryScanner::default();
                let (head, tail) = pattern.split_at(split);
                let mut found = scanner.scan(head);
                found.extend(scanner.scan(tail));
                assert_eq!(
                    found,
                    vec![expected],
                    "{expected:?} split after {split} byte(s) was missed"
                );
            }
        }
    }

    #[test]
    fn one_byte_at_a_time_still_finds_every_query() {
        for (pattern, expected) in TerminalQuery::PATTERNS {
            let mut scanner = TerminalQueryScanner::default();
            let found: Vec<TerminalQuery> =
                pattern.iter().flat_map(|b| scanner.scan(&[*b])).collect();
            assert_eq!(found, vec![expected]);
        }
    }

    #[test]
    fn several_queries_in_one_chunk_are_all_found() {
        let mut scanner = TerminalQueryScanner::default();
        let mut chunk = Vec::new();
        chunk.extend_from_slice(b"before");
        chunk.extend_from_slice(b"\x1b[6n");
        chunk.extend_from_slice(b"between");
        chunk.extend_from_slice(b"\x1b[c");
        chunk.extend_from_slice(b"and");
        chunk.extend_from_slice(b"\x1b[>0q");
        assert_eq!(
            scanner.scan(&chunk),
            vec![
                TerminalQuery::CursorPosition,
                TerminalQuery::DeviceAttributes,
                TerminalQuery::Version,
            ]
        );
    }

    /// A near miss must not leave the scanner primed, or the next stray byte
    /// would complete a query nobody asked.
    #[test]
    fn a_near_miss_does_not_count_and_does_not_poison_the_next_match() {
        let mut scanner = TerminalQueryScanner::default();
        assert!(
            scanner.scan(b"\x1b[7n").is_empty(),
            "ESC[7n is a different query"
        );
        assert!(
            scanner.scan(b"n").is_empty(),
            "a stray byte must not complete it"
        );
        assert_eq!(
            scanner.scan(b"\x1b[6n"),
            vec![TerminalQuery::CursorPosition],
            "a real query still counts"
        );
    }

    /// A question Glasshouse must stay silent on, and why silence is an
    /// answer rather than a hang.
    #[test]
    fn the_keyboard_protocol_query_is_deliberately_unanswered() {
        for query in DELIBERATELY_UNANSWERED {
            let mut scanner = TerminalQueryScanner::default();
            assert!(
                scanner.scan(query).is_empty(),
                "Glasshouse must not claim a keyboard protocol it does not encode for"
            );
        }
    }

    /// The idiom the silence relies on: a harness sends the keyboard query and
    /// device attributes together, and the device-attributes reply arriving
    /// with nothing before it is what tells the harness the protocol is
    /// absent. Codex 0.149.0 sends exactly this pair, in this order.
    #[test]
    fn device_attributes_still_answer_after_an_unanswered_question() {
        let mut scanner = TerminalQueryScanner::default();
        assert_eq!(
            scanner.scan(b"\x1b[?u\x1b[c"),
            vec![TerminalQuery::DeviceAttributes],
            "the pair must yield exactly one answer: the negative signal"
        );
    }

    /// `ESC[>0q` ends in `q` and contains no shorter query, but a scanner that
    /// tested the shortest pattern first could mistake part of one sequence
    /// for another. The device-attributes query is the trap: `ESC[c` is a
    /// suffix of nothing here, but `ESC[?1;2c` — a *reply* echoed back — must
    /// not be read as a fresh question.
    #[test]
    fn a_reply_flowing_back_is_not_mistaken_for_a_question() {
        let mut scanner = TerminalQueryScanner::default();
        assert!(
            scanner.scan(b"\x1b[?1;2c").is_empty(),
            "a device-attributes reply is not a device-attributes query"
        );
        assert!(
            scanner.scan(b"\x1b[?25h\x1b[2004h").is_empty(),
            "ordinary mode-setting is not a query"
        );
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
