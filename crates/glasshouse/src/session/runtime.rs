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
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::events::{EventBus, LifecycleEvent, MessageOrigin, ProcessExit, RecordedEvent};
use crate::launch::{HarnessLaunch, OwnedHarnessLaunch};
use crate::pty::{
    CanonicalOverflow, ExitStatus, LineDiscipline, PtyOutput, PtyProcess, TerminalSize, next_chunk,
};
use crate::session::supervision;
use crate::session::{SessionId, SessionPresentation};

/// How much of each session's output is kept by default.
///
/// Bounded on purpose: a harness left running for a day would otherwise grow
/// this without limit, and a session runtime that leaks memory in proportion to
/// how useful it has been is not one anyone can leave open.
pub const DEFAULT_SCROLLBACK_BYTES: usize = 256 * 1024;

/// Size of one read from a pseudo-terminal.
const READ_CHUNK: usize = 8 * 1024;

/// How long a person keeps the keyboard after putting something into a
/// session — capability map line 1719.
///
/// For this long after a person's own input reaches a session, machine text
/// aimed at that same session is **refused**, and told why. A person and an
/// orchestrator addressing one harness are two hands on one keyboard, and the
/// harness cannot tell them apart: it sees one stream of bytes into one line
/// editor. This project has already paid for what happens when they collide —
/// see `SessionRuntime::deliver`, where *"a second message into a worker
/// mid-turn ended that turn and stranded it"* is recorded as the reason a
/// concurrent delivery is refused rather than queued.
///
/// # Why ten seconds, and why it is a constant
///
/// It is long enough to cover the gap between a person's line landing and the
/// harness taking the turn it starts — the window in which an orchestrator's
/// message would either be swallowed by the line editor or end that turn —
/// and short enough that an orchestrator blocked by it is blocked for one
/// visible moment rather than for a stretch anyone would have to plan around.
///
/// It is deliberately **not** configuration. A setting here would be a knob
/// for turning off the rule that a person outranks a machine at their own
/// keyboard, and a person who wants an orchestrator's message delivered while
/// they type has a way to say so already: wait, or use a different session.
/// The one control the map does ask for — a person stopping machine messages
/// *entirely*, for a time they name — is line 1717's mute, which is a
/// separate verb with an explicit duration.
pub const USER_INPUT_PRECEDENCE: Duration = Duration::from_secs(10);

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

/// How many times in a row Glasshouse will put a session's harness back before
/// it stops and says why — Phase 10A's tenth line.
///
/// Three, because the failures a restart actually fixes are transient and
/// singular: a harness killed by the machine running out of memory, a
/// provider socket dropped mid-turn, a `.cmd` shim whose parent console went
/// away. Nothing that has failed three times in a row with no healthy interval
/// between them is going to be fixed by a fourth process, and each attempt is
/// a real harness with a real cost.
///
/// The bound is *consecutive*, and what clears it is [`HEALTHY_AFTER`] rather
/// than a restart having been attempted. That is the whole design: a counter
/// reset on "started" turns a crash loop into an infinite one.
pub const MAX_CONSECUTIVE_RESTARTS: u32 = 3;

/// How long a session's process must stay alive, and keep verifying against
/// the identity recorded for it, before it counts as **healthy**.
///
/// # Why a constant here is safe, when one at start time was not
///
/// This phase removed a timing constant from `SessionRuntime::start` because
/// its length decided *whether a session existed at all*, and the two
/// platforms answered differently — see [`SessionRuntime::start`]. This one
/// decides only *when a restart counter may be cleared*, and it is one-sided:
/// too long merely delays the reset, so a healthy session's next crash counts
/// against a bound it has already outgrown. Too short would be the dangerous
/// direction, and two seconds is more than two orders of magnitude above the
/// millisecond crash loop it exists to refuse to reset for.
pub const HEALTHY_AFTER: Duration = Duration::from_secs(2);

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

    /// Mark the output finished, and report whether this call is what
    /// finished it.
    ///
    /// Two threads can reach this for the same session and only one of them
    /// may publish [`LifecycleEvent::OutputEnded`]: the reader thread, when
    /// the pseudo-terminal reports end-of-file, and — on Windows, where that
    /// report never comes — `poll_exits` once the process has been observed
    /// to end and the drain grace has elapsed. Deciding inside the lock is
    /// what makes "exactly once" true rather than likely.
    fn finish(&self) -> bool {
        let first = match self.ended.lock() {
            Ok(mut ended) => !std::mem::replace(&mut *ended, true),
            Err(poisoned) => !std::mem::replace(&mut *poisoned.into_inner(), true),
        };
        self.changed.notify_all();
        first
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
    /// When `poll_exits` first observed this session's process to have ended.
    ///
    /// Windows only, because it exists to answer a question only Windows
    /// asks: *when may output be called finished if end-of-file will never
    /// arrive?* See [`SessionRuntime::poll_exits`].
    #[cfg(windows)]
    exit_seen: Option<Instant>,
    presentation: SessionPresentation,
    exit: Option<ExitStatus>,
    /// Everything needed to put this harness back — Phase 10A's tenth line.
    ///
    /// Owned rather than borrowed because the exit that would need it is
    /// noticed in [`SessionRuntime::poll_exits`], long after `start` returned
    /// and with no project in scope. See [`OwnedHarnessLaunch`].
    launch: OwnedHarnessLaunch,
    /// When the process now under `process` was started.
    ///
    /// Read only by the health rule below, which asks whether it has been
    /// alive long enough to count — see [`HEALTHY_AFTER`].
    started: Instant,
    /// The identity supervision recorded for the process now under `process`,
    /// or `None` if the pseudo-terminal would not name it.
    identity: Option<supervision::ProcessIdentity>,
    /// Whether the process now under `process` has been observed alive and
    /// verified for [`HEALTHY_AFTER`].
    ///
    /// Phase 10A's eleventh line — *"reset the consecutive-restart count only
    /// when a restarted session has been verified healthy, never when it has
    /// merely been started"* — is this field being the only thing that clears
    /// `restarts`.
    verified_healthy: bool,
    /// Whether **any** process of this session has ever been verified healthy.
    ///
    /// The gate on restarting at all. A harness that has never once come up is
    /// not a session that *exited unexpectedly*; it is a start that did not
    /// work, and putting it back three more times would turn a mistyped
    /// executable into four processes instead of one.
    was_ever_healthy: bool,
    /// Consecutive restarts since this session was last verified healthy.
    restarts: u32,
    /// Why this session will not be restarted again, once the bound is
    /// reached — Phase 10A's tenth line's second half, *"stop with a stated
    /// reason"*.
    restart_halted: Option<String>,
    /// Set by [`SessionRuntime::close`] and friends: an ending the user asked
    /// for is not an unexpected exit and must never be restarted.
    ended_deliberately: bool,
    /// When a person last put something into this session — capability map
    /// line 1719.
    ///
    /// Written by [`SessionRuntime::note_user_input`], and read by
    /// [`SessionRuntime::user_input_precedence`] to answer the one question
    /// that line asks: *is a person currently using this session?* `None` is
    /// "nobody has, in this process's lifetime", which is also what a
    /// restarted Glasshouse sees — see that method for why that is the right
    /// answer rather than a gap to be filled from the event log.
    last_user_input: Option<Instant>,
    /// Held for the whole of one delivery — Phase 10A's thirteenth line.
    ///
    /// *"Never deliver two inputs to the same session concurrently."* Today
    /// `&mut self` on every delivery method already excludes a second one,
    /// and the shipped binary wraps this runtime in a `Mutex` besides, so
    /// this lock is never contended in the build it ships in.
    ///
    /// It is here anyway, and on an `Arc` rather than as a plain flag, for
    /// what it makes *impossible to add*: the day a delivery path takes
    /// `&self`, or hands a session to a thread, or drains a queue without the
    /// outer lock, the second delivery is refused rather than interleaved
    /// with the first. A harness reading half of one message and half of
    /// another is not a defect anyone would find by reading a diff.
    ///
    /// The invariant it enforces is guarded structurally as well:
    /// `only_one_path_writes_to_a_session` fails if a second call site
    /// appears.
    delivery: Arc<Mutex<()>>,
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

    /// Whether this session's current process has been verified healthy.
    ///
    /// Alive, and still the process whose identity was recorded when it
    /// started, for at least [`HEALTHY_AFTER`]. Decided in
    /// [`SessionRuntime::poll_exits`] and nowhere else.
    pub fn verified_healthy(&self) -> bool {
        self.verified_healthy
    }

    /// Consecutive restarts since this session was last verified healthy.
    pub fn restarts(&self) -> u32 {
        self.restarts
    }

    /// Why Glasshouse will not put this harness back again, once the bound in
    /// [`MAX_CONSECUTIVE_RESTARTS`] has been reached or a restart itself
    /// failed. `None` while restarting is still on the table.
    pub fn restart_halted(&self) -> Option<&str> {
        self.restart_halted.as_deref()
    }

    /// Whether the process now under this session is still the one whose
    /// identity was recorded for it.
    ///
    /// A session started through a pseudo-terminal that would not name its
    /// child has no identity, and an unanswerable question is answered `false`
    /// here rather than assumed: the consequence of a wrong `true` is a
    /// crash-loop counter cleared by a stranger, and the consequence of a
    /// wrong `false` is a bound that is reached slightly sooner.
    fn identity_still_verifies(&self) -> bool {
        let Some(identity) = self.identity.as_ref() else {
            return false;
        };
        supervision::host_name().is_some_and(|host| {
            supervision::verify(identity, &host) == supervision::Verdict::Verified
        })
    }

    /// Write one line of Glasshouse's own into the session's scrollback.
    ///
    /// The session's terminal is where a user looks to find out what happened
    /// to it, so a restart and the decision to stop restarting belong there
    /// and not only in a log nobody has open. `\r\n` because this buffer holds
    /// what a terminal was sent.
    fn note(&self, text: &str) {
        if let Ok(mut scrollback) = self.scrollback.lock() {
            scrollback.push(format!("\r\nglasshouse: {text}\r\n").as_bytes());
        }
    }

    /// What this session's terminal is doing with input right now.
    ///
    /// Exposed because it is the one thing about a session that a caller
    /// cannot infer and that decides whether a long message can be delivered
    /// at all. Read from the kernel on every call — see
    /// [`PtyProcess::line_discipline`] for why it is never cached — and
    /// enforced for the caller by [`SessionRuntime::send_text_from`], so
    /// reading it here is for reporting rather than for anyone re-deriving
    /// the check.
    pub fn line_discipline(&self) -> LineDiscipline {
        self.process.line_discipline()
    }

    pub fn process_id(&self) -> Option<u32> {
        self.process.process_id()
    }

    /// The one path an input reaches this session by — Phase 10A's thirteenth
    /// line.
    ///
    /// *"Never deliver two inputs to the same session concurrently."* Two
    /// things make that true, and neither on its own would:
    ///
    /// - **One path.** Keystrokes, a line typed at the shell's prompt, a
    ///   machine-sent message, an interrupt, and the runtime's own answer to
    ///   a terminal query all arrive here. A second place that touched
    ///   `self.process` would be a second order nobody arbitrates, which is
    ///   why `only_one_path_writes_to_a_session` fails if one appears. The
    ///   terminal-query reply is in that list deliberately: it is bytes on the
    ///   same terminal, and a `\x1b[24;80R` landing in the middle of a line
    ///   somebody typed corrupts both.
    /// - **A per-session lock, held across the whole delivery.** `try_lock`
    ///   rather than `lock`: a second concurrent delivery is *refused* and the
    ///   caller told, never queued behind the first. Queuing would deliver it
    ///   eventually, out of the order its sender believed, which is the
    ///   failure this project already paid for once in its own process — a
    ///   second message into a worker mid-turn ended that turn and stranded
    ///   it.
    ///
    /// In today's build the lock is never contended, because every delivery
    /// method takes `&mut self` and the shipped binary owns the runtime behind
    /// a `Mutex`. It is here for the shape of the change that would break it,
    /// which is a shape that compiles.
    fn deliver(&mut self, what: Delivery<'_>) -> Result<(), RuntimeError> {
        if self.exit.is_some() {
            return Err(RuntimeError::Exited {
                id: self.id.clone(),
            });
        }
        let lock = Arc::clone(&self.delivery);
        let Ok(_delivering) = lock.try_lock() else {
            // Reported as `Io` with `WouldBlock` rather than as a variant of
            // its own: `RuntimeError` is matched exhaustively in the shell,
            // whose rendering of `Io` is the source's own sentence, and
            // *"a write would block because one is already in flight"* is
            // exactly what `WouldBlock` means. See
            // `docs/product/evidence/phase-10a.md` for the variant this wants
            // to be once that file can be touched.
            return Err(RuntimeError::Io {
                id: self.id.clone(),
                action: "deliver to",
                source: std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "another input is already being delivered to it, and Glasshouse \
                     will not interleave a second one",
                ),
            });
        };
        // Read the terminal's mode here rather than anywhere earlier: inside
        // the delivery lock, one statement before the write it governs, and
        // never cached. See `PtyProcess::line_discipline`.
        if let Delivery::Bytes(input) = what
            && let LineDiscipline::Canonical(line) = self.process.line_discipline()
            && let Some(bytes) = line.would_discard(input)
        {
            return Err(RuntimeError::LineTooLong {
                id: self.id.clone(),
                bytes,
                limit: line.max_bytes(),
                overflow: line.overflow(),
            });
        }
        match what {
            Delivery::Bytes(bytes) => self.process.write_input(bytes),
            Delivery::Interrupt => self.process.interrupt(),
        }
        .map_err(|source| RuntimeError::Io {
            id: self.id.clone(),
            action: match what {
                Delivery::Bytes(_) => "write to",
                Delivery::Interrupt => "interrupt",
            },
            source,
        })
    }
}

/// One input, on its way to a session.
///
/// A closed set rather than a byte slice, because an interrupt is not bytes
/// and must still be ordered against them — see [`SessionRuntime::deliver`].
#[derive(Debug, Clone, Copy)]
enum Delivery<'a> {
    Bytes(&'a [u8]),
    Interrupt,
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
    #[error(
        "session `{id}` is in canonical mode, where one line of input may carry at \
         most {limit} bytes including its terminator; a {bytes}-byte line would {overflow}, \
         so it was refused instead"
    )]
    LineTooLong {
        id: SessionId,
        /// The offending line's length in bytes, terminator included. **Never
        /// the line itself** — a caller's text can be arbitrarily long and may
        /// carry a secret it pasted, and this sentence is logged.
        bytes: usize,
        /// The terminal's own ceiling, from
        /// [`crate::pty::CanonicalLine::max_bytes`].
        limit: usize,
        /// What this terminal would actually have done with the line.
        ///
        /// Carried rather than spelled out in the format string above,
        /// because the sentence that string used to hold — *"discarded along
        /// with every byte written to that terminal afterwards"* — is macOS's
        /// answer and is **false on Linux**, where the line arrives truncated
        /// and the terminal survives. A refusal that misdescribes the hazard
        /// it prevented is the kind of thing a reader checks once and then
        /// trusts.
        overflow: CanonicalOverflow,
    },
    #[error("could not {action} session `{id}`")]
    Io {
        id: SessionId,
        action: &'static str,
        #[source]
        source: std::io::Error,
    },
}

/// Why a session was not started.
///
/// Separate from [`RuntimeError`] because it is a different question: that
/// enum is about acting on a session that exists, and every one of its
/// variants is something the shell renders as *"cannot do that to this
/// session"*. This one is not about an existing session at all — it says the
/// session is already there, so a second must not be started beside it.
///
/// There is deliberately no variant for a start whose process died: that is a
/// session that failed, not a session that was refused. See
/// [`SessionRuntime::start`] for the measurement behind that distinction.
///
/// [`SessionRuntime::start`] returns `anyhow::Result`, so this travels as a
/// source rather than as a new variant of a type somebody else exhaustively
/// matches.
#[derive(Debug, Clone, thiserror::Error)]
pub enum StartRefused {
    #[error(
        "session `{id}` is already running in this Glasshouse; refusing to start a \
         second session beside it"
    )]
    AlreadyLive { id: SessionId },
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
        // Phase 10A, fifth line, in its in-process form: *"refuse to start a
        // session that would duplicate a live, verified session of the same
        // record."* Verification is free here — this runtime holds the process
        // itself — and the duplicate is not hypothetical: two `LiveSession`s
        // under one identifier would give `get`, `focus` and `poll_exits`
        // whichever the vector happened to reach first, and one of the two
        // processes would then be steerable by nobody.
        //
        // The cross-process half of the same line lives in
        // `store::SessionStore::open_for_resume`, where the duplicate is a
        // process this Glasshouse did not start.
        if let Some(existing) = self.get(&id)
            && existing.is_running()
        {
            return Err(StartRefused::AlreadyLive { id }.into());
        }

        let (process, output) = launch.spawn()?;

        // Phase 10A, ninth line — *"require a started session to become
        // verifiably ready within a bounded time, and record a start that never
        // became ready as a failure with a stated reason rather than as a
        // session"* — is deliberately **not** enforced here, and this comment is
        // the reason why.
        //
        // An earlier version of this phase waited `READINESS_SETTLE` for the
        // process to prove itself and refused the start when it died inside the
        // window. It could not be made to mean the same thing on two operating
        // systems, and it cost a capability that was already closed.
        //
        // # What was measured
        //
        // The fixture is `echo STARTED; kill -9 $$` — the harness
        // `tests/events_lifecycle.rs` has used since Phase 45 closed
        // *"preserve terminal output and event history after a worker
        // crashes"*. One tree, one gate run: **macOS 5 passed, Linux 3 passed
        // and 2 failed.** The cause is not the length of the window. It is that
        // the two kernels disagree about a process that has died and not yet
        // been reaped: `/proc/<pid>/stat` still describes a zombie, so Linux
        // keeps looking and the parent handle reports the `SIGKILL` first;
        // `proc_pidinfo` stops answering for one, so macOS concludes it cannot
        // identify the process and keeps the session. Same code, opposite
        // answers, neither of them a coin flip — so no larger settle window
        // fixes it.
        //
        // # And there is no in-start refusal that would have been right
        // {#no-deterministic-refusal}
        //
        // `spawn` returns a live process id before anyone knows whether the
        // `exec` behind it worked, so at start time *"the process is alive"* is
        // always true and *"it died"* is always a later observation. That
        // observation is the same one for a harness whose configuration was
        // unreadable and for a harness that ran and crashed: on Windows the two
        // fixtures this repository uses for those cases are the same three
        // lines. Nothing separates them — not the exit status, and not whether
        // output arrived, which under Linux container load is the flake §34
        // already records.
        //
        // # So the line is answered where the difference is real
        //
        // In the record. A start that never became ready is one whose record
        // never left `starting` and whose process is gone;
        // `supervision::reconcile` concludes exactly that, durably,
        // identically on every platform, and says so in `supervision_reason` —
        // and a session whose harness died is recorded as `failed` by
        // [`SessionRuntime::poll_exits`], with the harness's own last words
        // still in its scrollback. Both are failures with a stated reason, and
        // neither throws away the output the user needs to see why.
        //
        // Refusing the start is what discarded that output, and it is what a
        // capability that was already closed was closed *against*.

        let scrollback = Arc::new(Mutex::new(Scrollback::new(self.scrollback_bytes)));
        let output_ended = Arc::new(OutputEnd::default());
        // Read back from the process rather than from the launch: this is the
        // size the pseudo-terminal actually got, which is what the harness will
        // be drawing for.
        let size = process.size();
        let screen = Arc::new(Mutex::new(vt100::Parser::new(size.rows, size.cols, 0)));
        let pending_queries = Arc::new(Mutex::new(Vec::new()));
        // Recorded, never judged. What the kernel says about the process now is
        // what `supervision::verify` will be given later to decide whether the
        // thing still running under this pid is still this session's harness —
        // which is the question the health rule in `poll_exits` asks, and the
        // only question asked of it at start time.
        let pid = process.process_id();

        spawn_reader(
            &id,
            output,
            &scrollback,
            &screen,
            &pending_queries,
            &output_ended,
            &self.events,
        )?;

        // Structural, not remembered. An *exited* entry under this id is kept
        // deliberately by `poll_exits`, so that a crashed worker's output and
        // its crash report survive it; pushing beside that entry is precisely
        // what the comment on the duplicate guard above describes, because
        // `get`, `get_mut`, `focus`, `close` and `crash_report` all resolve
        // the **first** match and the corpse is the one already in the vector.
        // The live session would then be steerable by nobody, and a send to it
        // would return `RuntimeError::Exited` for a harness the user can watch
        // running.
        //
        // `shell::resume_session` has been calling `close` by hand to avoid
        // exactly this, with nine lines of comment explaining why. Doing it
        // here means the invariant holds for every caller that reuses an id —
        // `api`, `main`, a future resume path — rather than only for the
        // callers that happened to know.
        //
        // Removing rather than refusing, because refusing would make a
        // restart-under-the-same-id impossible without the caller first
        // knowing to close: the same remembered obligation, moved one step.
        //
        // **Here, and not beside the guard above**, because everything between
        // there and this line can fail — `launch.spawn()` and `spawn_reader`
        // both carry a `?`. Removing earlier would mean a failed restart threw
        // away the crash report of the run that prompted it, which is the one
        // thing the caller would then want to read.
        if let Some(index) = self
            .sessions
            .iter()
            .position(|session| session.id == id && !session.is_running())
        {
            self.sessions.remove(index);
            // The same fix-up `close` does, for the same reason and with the
            // same answer: focus must never name an entry that is gone. Doing
            // it identically is also what keeps this from changing what
            // `shell::resume_session` — which calls `close` and then `start` —
            // has always done. If nothing else can hold the keyboard, the
            // session pushed below takes it.
            if self.focused.as_ref() == Some(&id) {
                self.focused = self
                    .sessions
                    .iter()
                    .find(|session| {
                        session.is_running()
                            && session.presentation != SessionPresentation::Headless
                    })
                    .map(|session| session.id.clone());
            }
        }

        let focusable = presentation != SessionPresentation::Headless;
        self.sessions.push(LiveSession {
            id: id.clone(),
            process,
            scrollback,
            screen,
            pending_queries,
            output_ended,
            #[cfg(windows)]
            exit_seen: None,
            presentation,
            exit: None,
            launch: launch.into_owned(),
            started: Instant::now(),
            identity: pid.and_then(supervision::ProcessIdentity::of),
            verified_healthy: false,
            was_ever_healthy: false,
            restarts: 0,
            restart_halted: None,
            ended_deliberately: false,
            last_user_input: None,
            delivery: Arc::new(Mutex::new(())),
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
        //
        // Line 1719: the same fact, kept where a *decision* can read it. The
        // event below is a record for later; this is the state a machine
        // message arriving a second from now is refused against.
        self.note_user_input(&id, Instant::now());
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
        self.deliver(id, Delivery::Bytes(bytes))
    }

    /// The one path an input reaches a session by — Phase 10A's thirteenth
    /// line.
    ///
    /// *"Never deliver two inputs to the same session concurrently."* Two
    /// things make that true, and neither on its own would:
    ///
    /// - **One path.** Keystrokes, a line typed at the shell's prompt, a
    ///   machine-sent message and an interrupt all arrive here. A second place
    ///   that touched `session.process` would be a second order nobody
    ///   arbitrates, which is why `only_one_path_writes_to_a_session` fails if
    ///   one appears.
    /// - **A per-session lock, held across the whole delivery.** `try_lock`
    ///   rather than `lock`: a second concurrent delivery is *refused* and the
    ///   caller told, never queued behind the first. Queuing would deliver it
    ///   eventually, out of the order its sender believed, which is the
    ///   failure this project already paid for once in its own process — a
    ///   second message into a worker mid-turn ended that turn and stranded
    ///   it.
    ///
    /// In today's build the lock is never contended, because every delivery
    /// method takes `&mut self` and the shipped binary owns this runtime
    /// behind a `Mutex`. It is here for the shape of the change that would
    /// break it, which is a shape that compiles.
    fn deliver(&mut self, id: &SessionId, what: Delivery<'_>) -> Result<(), RuntimeError> {
        self.get_mut(id)?.deliver(what)
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
        // Line 1719, on the other half of the same seam: a line a person
        // typed at the shell's prompt, or sent through `glasshouse api send`,
        // is a person using this session exactly as a keystroke is.
        if origin == MessageOrigin::UserKeystroke {
            self.note_user_input(id, Instant::now());
        }
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
        // An interrupt is an input: it arrives at the same session through the
        // same terminal and is ordered against text by the same rule. Sending
        // it beside a line of text would let the harness see the interrupt in
        // the middle of the line it was meant to cancel.
        self.deliver(id, Delivery::Interrupt)?;
        // A harness that ends because it was interrupted did not exit
        // unexpectedly, so it is not restarted. The mark lasts only until the
        // session is next seen alive — see `SessionRuntime::poll_exits` — so
        // it excuses the exit this interrupt caused and no later one.
        if let Some(session) = self.sessions.iter_mut().find(|session| &session.id == id) {
            session.ended_deliberately = true;
        }
        // Line 1719. A person's `Ctrl-C` is a person using this session, and
        // starts the same window a typed line does: somebody who has just
        // stopped a worker is about to say why, and a machine line landing in
        // between is the collision the window exists to prevent.
        if origin == MessageOrigin::UserKeystroke {
            self.note_user_input(id, Instant::now());
        }
        self.events
            .publish(id, LifecycleEvent::InterruptDelivered { origin });
        Ok(())
    }

    /// Record that a person put something into this session at `at` —
    /// capability map line 1719.
    ///
    /// Called by every path a person's own input reaches a session by:
    /// [`SessionRuntime::write_to_focused`] (the keyboard),
    /// [`SessionRuntime::send_text_from`] (the shell's send-a-line prompt and
    /// `glasshouse api send`), and [`SessionRuntime::interrupt_from`].
    ///
    /// `at` is a parameter rather than read from the clock here so that the
    /// window's *expiry* is testable without a test sleeping through it. Every
    /// production caller passes `Instant::now()`; a test that needs to stand
    /// on the far side of [`USER_INPUT_PRECEDENCE`] passes a moment that far
    /// in the past, which is the same call the binary makes rather than a
    /// door beside it.
    ///
    /// Never moves the mark backwards: two inputs in the window leave the
    /// later one standing, so a person typing steadily keeps the keyboard
    /// rather than losing it to the age of their first line.
    ///
    /// A session this runtime does not hold is silently ignored — there is
    /// nothing to protect and nothing to say, and every caller here has
    /// already established liveness by writing to it.
    pub fn note_user_input(&mut self, id: &SessionId, at: Instant) {
        if let Some(session) = self.sessions.iter_mut().find(|session| &session.id == id) {
            let later = match session.last_user_input {
                Some(previous) if previous >= at => previous,
                _ => at,
            };
            session.last_user_input = Some(later);
        }
    }

    /// How much longer a person holds this session's keyboard at `now`, or
    /// `None` if nobody does — capability map line 1719.
    ///
    /// `Some(remaining)` is a refusal a caller owes the machine on the other
    /// end of it, with the time named; see
    /// [`crate::session::api::SessionApi::send_text`], which is where the
    /// refusal is actually taken.
    ///
    /// # A restart answers `None`, and that is the honest answer
    ///
    /// This lives in memory, in the process that owns the pseudo-terminal,
    /// and is gone when that process is. It is not read back from the event
    /// log — which does record `text_delivered` with its origin — because a
    /// process that has just started owns no pseudo-terminal anybody typed
    /// into: the sessions it holds are the ones *it* started, and a person
    /// cannot have been typing into a terminal that did not exist. The rule
    /// protects a live collision, so the state has exactly the lifetime of
    /// the thing it protects.
    pub fn user_input_precedence(&self, id: &SessionId, now: Instant) -> Option<Duration> {
        let last = self
            .sessions
            .iter()
            .find(|session| &session.id == id)?
            .last_user_input?;
        // `filter` rather than `checked_sub` alone: a duration that has run
        // out to exactly zero is over, and `Some(0s)` would be a refusal
        // naming no remaining time at all.
        USER_INPUT_PRECEDENCE
            .checked_sub(now.saturating_duration_since(last))
            .filter(|remaining| !remaining.is_zero())
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
    ///
    /// **Reported only for a session that stayed exited.** A death that this
    /// method answers by putting the harness back is published to the history
    /// as [`LifecycleEvent::ProcessExited`] and then dropped from the returned
    /// vector, because every caller of it treats an entry as the end of the
    /// session — see the comment on the `retain` below.
    ///
    /// # Windows: this is also where output is declared to have ended
    ///
    /// [`crate::pty`] wrote down, before the reader thread existed, that a
    /// reader "must not treat *no more bytes* as its stop condition, because
    /// on Windows that may never come while the pty is still held open", and
    /// prescribed observing the process instead. The prescription is right
    /// about the diagnosis and **cannot be carried out where it points**: by
    /// the time there are no more bytes, `pump` is parked inside a blocking
    /// `read` that will not return, so it can neither call `try_wait` nor
    /// notice a flag someone set for it. A stop condition is no use to a
    /// thread that has already stopped.
    ///
    /// So the thread that *does* observe the exit says so. When a session's
    /// process has been seen to end and `OUTPUT_DRAIN_WAIT` has passed
    /// since, this marks the session's output finished and publishes
    /// [`LifecycleEvent::OutputEnded`]. `pump` still publishes it on every
    /// platform that produces an end-of-file, and
    /// `OutputEnd::finish` decides which of the two got there first, so the
    /// event fires exactly once either way.
    ///
    /// **Nothing is truncated by this.** The reader is not stopped and not
    /// interrupted; it keeps draining into the scrollback for as long as the
    /// session is held. The grace is what keeps the *event* honest — a
    /// child's death outruns its last words by a millisecond or two, measured
    /// at 1.1–2.2ms on Linux — and a byte that somehow arrives after it is
    /// still recorded, only after the announcement.
    ///
    /// **Windows only, deliberately.** On Unix "the output ended" is a
    /// statement about a file descriptor and the descriptor can make it, so
    /// nothing here should redefine it — including the case this would
    /// otherwise change, a crashed harness whose grandchild still holds the
    /// pty slave open, where the strict meaning is the true one and *no*
    /// output-end really has happened. On Windows that descriptor cannot
    /// speak at all, so the honest meaning there is the weaker one:
    /// **the process exited and its output stopped arriving.**
    pub fn poll_exits(&mut self) -> Vec<(SessionId, ExitStatus)> {
        let mut ended = Vec::new();
        for session in &mut self.sessions {
            if session.exit.is_some() {
                continue;
            }
            match session.process.try_wait() {
                Ok(Some(status)) => {
                    session.exit = Some(status.clone());
                    #[cfg(windows)]
                    {
                        session.exit_seen = Some(Instant::now());
                    }
                    ended.push((session.id.clone(), status));
                }
                // Still running, and therefore the only moment at which this
                // session's *health* can be decided — Phase 10A's eleventh
                // line. Healthy is deliberately not "it was started": it is
                // alive here, now, having been alive for `HEALTHY_AFTER`, and
                // still verifying against the identity recorded for it. A
                // harness that crash-loops in milliseconds never reaches a
                // poll in that state, which is exactly what stops it from
                // clearing the bound it is about to run into.
                Ok(None) => {
                    // A signal the user asked for only excuses an exit that
                    // follows it. Having survived to this poll, it does not.
                    session.ended_deliberately = false;
                    if !session.verified_healthy
                        && session.started.elapsed() >= HEALTHY_AFTER
                        && session.identity_still_verifies()
                    {
                        session.verified_healthy = true;
                        session.was_ever_healthy = true;
                        // The reset, and the only one. See
                        // `MAX_CONSECUTIVE_RESTARTS`.
                        session.restarts = 0;
                    }
                }
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

        // Phase 10A, tenth line: *"restart a session that exits unexpectedly
        // up to a bounded number of consecutive attempts, and stop with a
        // stated reason when that bound is reached."*
        //
        // After the exits are published, so that the history records the death
        // before it records the session starting again — a consumer reading
        // the bus must never see a restart it has no exit for.
        for (id, status) in &ended {
            self.consider_restart(id, status);
        }

        // A session that was put back is not an ending anyone may act on.
        //
        // `ProcessExited` has already been published, so the history still
        // records the death — what must not travel out of here is the claim
        // that the session is *over*. Every consumer of this vector treats an
        // entry as terminal: `shell::run` writes
        // `ProcessExit::session_state()` into the durable record and runs
        // `session::native_id::capture`, and `main.rs`'s headless loop returns
        // that status as the run's own result. A record left reading `Failed`
        // or `Stopped` for a live harness is not merely wrong on a list:
        // `supervision::guard_start` returns `Ok(())` for any record whose
        // lifecycle is not live, so nothing downstream would refuse a start
        // over the top of the conversation this harness is still holding —
        // the duplicate `open_for_resume` exists to prevent, reached from the
        // outside. `native_id::capture` also runs its end-of-session discovery
        // window against a mid-life session, which is the widest that window
        // can be rather than the tightest it assumes. And the focus fix-up
        // below would move the keyboard off a session that is running again.
        //
        // The predicate is the session's *observed* state after the restart
        // attempt, not the fact that one was attempted, so every way
        // `consider_restart` can decline or fail — a clean exit, a deliberate
        // one, a harness that was never healthy, the bound reached, a spawn or
        // reader that failed — leaves `exit` set and the exit reported. A
        // session no longer in the runtime at all is kept for the same reason:
        // there is nothing alive to withhold the report for.
        //
        // A harness that is put back and dies again immediately is not lost,
        // only deferred: its new process is `exit: None` here, and the next
        // poll asks it, publishes a fresh `ProcessExited`, and reports the
        // exit as soon as one of them is the death it stays dead of.
        ended.retain(|(id, _)| !self.get(id).is_some_and(LiveSession::is_running));

        // On Windows, this is also where a session's output is declared
        // finished — see the `# Windows` section of this method's doc
        // comment. Collected first and published after the loop for the same
        // reason `ended` is: `self.events` cannot be borrowed while the
        // sessions are.
        #[cfg(windows)]
        {
            let finished: Vec<SessionId> = self
                .sessions
                .iter()
                .filter(|session| {
                    session
                        .exit_seen
                        .is_some_and(|seen| seen.elapsed() >= OUTPUT_DRAIN_WAIT)
                })
                .filter(|session| session.output_ended.finish())
                .map(|session| session.id.clone())
                .collect();
            for id in finished {
                self.events.publish(&id, LifecycleEvent::OutputEnded);
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
    /// Put a session's harness back, if this exit was one worth restarting
    /// for and the bound has not been reached — Phase 10A's tenth line.
    ///
    /// # What counts as *exiting unexpectedly*
    ///
    /// Four things exclude a restart, and each of them is a case where putting
    /// the harness back would be wrong rather than merely unnecessary:
    ///
    /// - **A clean exit.** A harness that did its work and left has not
    ///   failed; this project already refuses to call that finishing, and it
    ///   must not call it crashing either.
    /// - **An ending the user asked for.** `interrupt` marks the session, and
    ///   the mark survives only until the session is seen alive again, so it
    ///   excuses the exit it caused and no later one.
    /// - **A session that was never healthy.** This is the load-bearing one. A
    ///   harness that has not once come up did not *exit unexpectedly* — it is
    ///   a start that did not work, and restarting it three more times turns a
    ///   mistyped executable into four processes. It is also the reason this
    ///   line does not disturb `tests/events_lifecycle.rs`: a harness that
    ///   prints one line and dies has crashed, and Glasshouse keeps its output
    ///   and its history rather than trying again.
    /// - **A bound already reached, or a restart that itself failed.** Once
    ///   there is a stated reason, it stands.
    fn consider_restart(&mut self, id: &SessionId, status: &ExitStatus) {
        let events = self.events.clone();
        let Some(session) = self.sessions.iter_mut().find(|session| &session.id == id) else {
            return;
        };
        if status.success()
            || session.ended_deliberately
            || !session.was_ever_healthy
            || session.restart_halted.is_some()
        {
            return;
        }

        if session.restarts >= MAX_CONSECUTIVE_RESTARTS {
            let reason = format!(
                "the harness has failed {} times in a row without staying up for \
                 {}s; Glasshouse will not restart it again",
                session.restarts,
                HEALTHY_AFTER.as_secs()
            );
            tracing::warn!(session = %id, restarts = session.restarts, "restart bound reached");
            session.note(&reason);
            session.restart_halted = Some(reason);
            return;
        }

        // The size the surface last gave this session, not the one the first
        // launch was built with: a terminal resized between the crash and the
        // restart would otherwise put the new harness back at the old size.
        session.launch.set_size(session.process.size());
        let (process, output) = match session.launch.spawn() {
            Ok(started) => started,
            Err(error) => {
                let reason = format!("the harness could not be restarted: {error:#}");
                tracing::warn!(session = %id, %error, "a session could not be restarted");
                session.note(&reason);
                session.restart_halted = Some(reason);
                return;
            }
        };

        // A fresh end-of-output flag and a fresh screen, because both describe
        // the process rather than the session — but the **same scrollback**,
        // because what the harness said before it died is the session's, and
        // Phase 45 requires a crash not to cost it.
        let size = process.size();
        let screen = Arc::new(Mutex::new(vt100::Parser::new(size.rows, size.cols, 0)));
        let pending_queries = Arc::new(Mutex::new(Vec::new()));
        let output_ended = Arc::new(OutputEnd::default());
        if let Err(error) = spawn_reader(
            id,
            output,
            &session.scrollback,
            &screen,
            &pending_queries,
            &output_ended,
            &events,
        ) {
            // The process is running and nothing is reading its terminal,
            // which fills and blocks it. Ending it here is the one case in
            // this module where Glasshouse stops a process it started, and it
            // is the lesser harm: the alternative is a wedged harness nobody
            // can see or steer.
            let mut process = process;
            let _ = process.signal(crate::pty::ProcessSignal::Kill);
            let reason = format!("the restarted harness could not be read: {error:#}");
            session.note(&reason);
            session.restart_halted = Some(reason);
            return;
        }

        session.restarts += 1;
        session.note(&format!(
            "the harness exited unexpectedly ({status}); restarting it \
             (attempt {} of {MAX_CONSECUTIVE_RESTARTS})",
            session.restarts
        ));
        session.identity = process
            .process_id()
            .and_then(supervision::ProcessIdentity::of);
        session.process = process;
        session.screen = screen;
        session.pending_queries = pending_queries;
        session.output_ended = output_ended;
        session.exit = None;
        session.started = Instant::now();
        session.verified_healthy = false;
        #[cfg(windows)]
        {
            session.exit_seen = None;
        }
        tracing::info!(session = %id, attempt = session.restarts, "restarted a session");
        events.publish(id, LifecycleEvent::SessionStarted);
    }

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

                // Through the funnel, like every other byte — see
                // `LiveSession::deliver`. An answer to a terminal query is
                // still an input to the harness's terminal, and it is exactly
                // the kind of write that would otherwise be added beside the
                // ordered path rather than through it.
                if let Err(error) = session.deliver(Delivery::Bytes(reply.as_bytes())) {
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

/// Start the thread that drains one session's pseudo-terminal.
///
/// Factored out of [`SessionRuntime::start`] because a restarted session needs
/// exactly the same thread against a new process, and two copies of this would
/// eventually differ in one of the details below.
///
/// The bus travels *into* the thread rather than being consulted from outside,
/// because this thread is the only one that knows when the pseudo-terminal
/// stopped giving output. It is also the thread that must never wait: a reader
/// that blocks stops draining the terminal, whose buffer then fills, and the
/// harness itself blocks on `write`. `EventBus::publish` is bounded work with
/// no wait on any consumer, which is what makes putting it here safe — see
/// [`crate::events::bus`].
fn spawn_reader(
    id: &SessionId,
    output: PtyOutput,
    scrollback: &Arc<Mutex<Scrollback>>,
    screen: &Arc<Mutex<vt100::Parser>>,
    pending_queries: &Arc<Mutex<Vec<TerminalQuery>>>,
    output_ended: &Arc<OutputEnd>,
    events: &EventBus,
) -> Result<()> {
    let scrollback = Arc::clone(scrollback);
    let screen = Arc::clone(screen);
    let pending_queries = Arc::clone(pending_queries);
    let output_ended = Arc::clone(output_ended);
    let events = events.clone();
    let session = id.clone();
    std::thread::Builder::new()
        .name(format!("glasshouse-session-{}", short(id)))
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
    Ok(())
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
    // `Ok(0)` and an error mean the same thing: nothing more is coming. A
    // pseudo-terminal reports the end of a session as end-of-file on some
    // platforms and as a read error on others, and neither is a fault —
    // the exit status comes from the process, not from here.
    //
    // The one exception is a read a signal interrupted, which is not an
    // ending and must not stop this thread — see [`next_chunk`], which
    // owns that decision for every reader in the crate.
    while let Some(read) = next_chunk(&mut output, &mut buffer) {
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
    // A statement about a file descriptor, and nothing more. Publishing it as
    // its own event — rather than folding it into an exit, or letting a
    // consumer time the silence — is what keeps "the output stopped" from
    // ever being read as "the work finished".
    //
    // On Windows this line is unreachable while the pty is open: the read
    // above never returns `Ok(0)` and never errors, because nothing closes
    // the write end until the master is dropped. `SessionRuntime::poll_exits`
    // is the one that gets there, and `finish` decides which of the two
    // publishes. See `crate::pty`'s note on the dropped slave handle.
    if ended.finish() {
        events.publish(session, LifecycleEvent::OutputEnded);
    }
}

fn short(id: &SessionId) -> String {
    id.as_str().chars().take(8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An interrupted read is not an ending, and `pump` is where that matters
    /// most: it is the only thing draining a session's pseudo-terminal and
    /// nothing ever restarts it.
    ///
    /// Two properties, and both are needed. That output arriving *after* the
    /// interruption still reaches the scrollback proves the read was retried
    /// rather than abandoned. That no `OutputEnded` has been published while
    /// the harness is still there proves the other half of the defect — a
    /// live session declared finished — because a consumer that believes the
    /// output ended stops waiting for more.
    ///
    /// The reader blocks after its second chunk rather than returning
    /// end-of-file, because a `pump` that ran to completion would publish
    /// `OutputEnded` legitimately and the assertion would prove nothing.
    ///
    /// Non-vacuity: delete `next_chunk`'s `Interrupted` arm and this times
    /// out waiting for output the reader already produced.
    #[test]
    fn an_interrupted_read_neither_ends_the_reader_nor_the_session_output() {
        use std::io::ErrorKind;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

        /// One interruption, one chunk, then a live-but-quiet terminal.
        struct InterruptedThenQuiet {
            step: AtomicUsize,
            release: Arc<AtomicBool>,
        }

        impl std::io::Read for InterruptedThenQuiet {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                match self.step.fetch_add(1, Ordering::SeqCst) {
                    0 => Err(std::io::Error::from(ErrorKind::Interrupted)),
                    1 => {
                        let bytes = b"after the signal";
                        buf[..bytes.len()].copy_from_slice(bytes);
                        Ok(bytes.len())
                    }
                    // Blocked in a read on a terminal nobody is typing at,
                    // which is what a healthy quiet session looks like.
                    _ => {
                        while !self.release.load(Ordering::SeqCst) {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Ok(0)
                    }
                }
            }
        }

        let release = Arc::new(AtomicBool::new(false));
        let output = PtyOutput::from_reader(InterruptedThenQuiet {
            step: AtomicUsize::new(0),
            release: Arc::clone(&release),
        });

        let scrollback = Arc::new(Mutex::new(Scrollback::new(DEFAULT_SCROLLBACK_BYTES)));
        let screen = Arc::new(Mutex::new(vt100::Parser::new(24, 80, 0)));
        let pending_queries = Arc::new(Mutex::new(Vec::new()));
        let ended = Arc::new(OutputEnd::default());
        let events = EventBus::new();
        let session = SessionId::new("interrupted-reader");

        let reader = {
            let (scrollback, screen, pending_queries, ended, events, session) = (
                Arc::clone(&scrollback),
                Arc::clone(&screen),
                Arc::clone(&pending_queries),
                Arc::clone(&ended),
                events.clone(),
                session.clone(),
            );
            std::thread::spawn(move || {
                pump(
                    output,
                    &scrollback,
                    &screen,
                    &pending_queries,
                    &ended,
                    &events,
                    &session,
                )
            })
        };

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if scrollback
                .lock()
                .unwrap()
                .text()
                .contains("after the signal")
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the reader stopped at the interrupted read: nothing after it \
                 reached the scrollback"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        assert!(
            !events
                .history_for(&session)
                .iter()
                .any(|recorded| recorded.event() == &LifecycleEvent::OutputEnded),
            "a live session must not be told its output ended: {:?}",
            events.history_for(&session)
        );

        release.store(true, Ordering::SeqCst);
        reader.join().expect("the reader thread must not panic");

        // And the real ending is still reported, so the retry did not cost
        // the event it exists to protect.
        assert!(
            events
                .history_for(&session)
                .iter()
                .any(|recorded| recorded.event() == &LifecycleEvent::OutputEnded),
            "end-of-file must still end the output: {:?}",
            events.history_for(&session)
        );
    }

    /// Phase 10A's thirteenth line, as a structural guard rather than a
    /// promise — *"never deliver two inputs to the same session
    /// concurrently."*
    ///
    /// Two inputs can only interleave if there are two places that write. The
    /// funnel is `SessionRuntime::deliver`, and this fails the moment a second
    /// call site touches a session's process directly — which is how the
    /// invariant would actually be lost: not by someone removing the lock, but
    /// by someone adding a path that never takes it.
    ///
    /// The scan reads by lines, so it is blind to line endings by
    /// construction — see `docs/product/design-decisions.md`.
    #[test]
    fn only_one_path_writes_to_a_session() {
        // Whitespace is removed before matching, because `rustfmt` decides
        // where a method chain breaks and the invariant does not depend on
        // its decision. A scan that matched source lines would pass or fail
        // on formatting, which is the worst possible property for a guard.
        let code: String = include_str!("runtime.rs")
            .lines()
            .take_while(|line| !line.trim_end().starts_with("mod tests"))
            .filter(|line| !line.trim_start().starts_with("//"))
            .flat_map(|line| line.chars().filter(|c| !c.is_whitespace()))
            .collect();

        for (what, pattern) in [
            ("writes bytes to", ".process.write_input("),
            ("interrupts", ".process.interrupt()"),
        ] {
            let sites = code.matches(pattern).count();
            assert_eq!(
                sites, 1,
                "{sites} places in the runtime {what} a session; there must be exactly \
                 one, because two orders of delivery are no order at all"
            );
        }

        // And the one place must be the funnel, not whichever function
        // happened to be written first.
        let funnel = code
            .find("fndeliver(")
            .expect("the delivery funnel must still be called `deliver`");
        let write = code.find(".process.write_input(").expect("checked above");
        assert!(
            write > funnel,
            "the only write to a session must be inside `deliver`"
        );
    }

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
