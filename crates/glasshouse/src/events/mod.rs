//! The normalized Glasshouse lifecycle-event stream.
//! One stream, shared by the TUI, the router, memory, the API and the MCP
//! surface. Adapters translate a harness's own vocabulary into
//! [`LifecycleEvent`]; nothing downstream ever learns which harness produced
//! one — `no_harness_is_named_in_the_core_event_stream` keeps it that way,
//! and [`crate::session::lifecycle`] is the single place allowed to know
//! either harness's spelling.
//! A process exiting is not a turn completing: the exit status cannot tell
//! `/quit` mid-task from finished work, so [`ProcessExit`] has no
//! `success()`, no conversion to [`TurnOutcome`], and [`task_outcome`]
//! answers `None` for a session that only ever exited. Waiting for the user
//! is not idle either: [`LifecycleEvent::WaitingForUser`] is recorded only
//! when a harness says so, never promoted to or demoted from by silence.
//! Quiet cannot become completion by accident: [`LifecycleEvent::TurnEnded`]
//! is minted in exactly **one** production function, the harness translator
//! in [`crate::session::lifecycle`] (`turn_completion_is_minted_in_exactly_one_place`
//! scans for a second site), and [`task_outcome`] reads it and nothing else.
//! Durable storage is not here: this module offers [`EventSink`] as the seam
//! it attaches to.
// History: design-decisions.md, "Trims: api, events, harness and config module docs, second packet", crates/glasshouse/src/events/mod.rs module doc.

pub mod bus;
pub mod log;

pub use bus::{DEFAULT_HISTORY, EventBus, EventSink, RecordedEvent, Subscription};
pub use log::{EventLog, EventLogSink, LoggedEvent, Observation};

/// One thing that happened to one session, in Glasshouse's own vocabulary.
///
/// Deliberately small. Every variant is something a consumer can act on, and
/// nothing here is a harness's word for something.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleEvent {
    /// A session's process was started.
    ///
    /// Carries no harness name on purpose. Which harness a session runs is
    /// already a column on its record, and putting it in the event too would
    /// be a second source of truth for one fact — the same reason
    /// [`crate::session::SessionDisposition`] is derived rather than stored.
    /// A consumer that needs it looks the session up.
    SessionStarted,
    /// A recorded session's process was started again, continuing the
    /// harness's own conversation.
    ///
    /// Distinct from [`LifecycleEvent::SessionStarted`] rather than folded
    /// into it, because the two are different facts about the project and a
    /// reader would otherwise have to *infer* a resume from a session having
    /// started twice. Phase 18 asks for session creation and session resume
    /// as separate recordings; an inference is not a recording.
    SessionResumed,
    /// The harness reported that it started working.
    TurnStarted,
    /// The harness reported that a turn ended, and how.
    ///
    /// The only event that carries a statement about the work itself. See the
    /// module docs for why it has exactly one construction site.
    TurnEnded { outcome: TurnOutcome },
    /// The harness reported that it is blocked on the user and will not
    /// proceed until they answer. Never inferred from silence.
    WaitingForUser,
    /// Text reached a session's terminal, from a person or from a machine.
    TextDelivered { origin: MessageOrigin, bytes: usize },
    /// An interrupt reached a session's terminal.
    InterruptDelivered { origin: MessageOrigin },
    /// The operating system reported that the child process ended.
    ///
    /// Says nothing about whether the session's work was finished — see
    /// [`ProcessExit`].
    ProcessExited { exit: ProcessExit },
    /// The pseudo-terminal has no more output to give.
    ///
    /// A statement about a file descriptor. It is *not* a statement about the
    /// session's work, and [`LifecycleEvent::implied_state`] returns `None`
    /// for it precisely so that no consumer can treat it as one.
    OutputEnded,
    /// A backend resource stopped serving, separately from any harness
    /// process. See [`GatewayFailure`].
    GatewayUnhealthy {
        resource: String,
        reason: GatewayFailure,
    },
    /// The backend serving a gateway-backed session changed. Names only —
    /// a provider, a model and a cause, never a credential.
    GatewayBackendChanged {
        provider: String,
        model: String,
        cause: String,
    },
    /// A session **changed** a file, as the context firewall's `PostToolUse`
    /// hook saw it — one event per distinct path an `Edit`, `Write`,
    /// `MultiEdit` or `NotebookEdit` named.
    ///
    /// Touched means changed: `Read`, `Grep` and `Glob` paths are not
    /// recorded, because a memory can honestly reference a file the session
    /// *changed*, and admitting a mere read here would let map line 1139's
    /// `referenced` association be earned by a glance.
    ///
    /// The path is repo-relative and `/`-separated
    /// ([`crate::memory::normalize_observed_path`]'s spelling), so a path
    /// outside the project root is dropped before it ever reaches an event.
    /// It is the user's own file name and nothing else: no content, no diff,
    /// no tool output.
    ///
    /// Not a state transition: [`LifecycleEvent::implied_state`] answers
    /// `None`, because the hook fires mid-turn and promoting this would let
    /// a `PostToolUse` payload reach the session state machine, which
    /// `REPORTED_EVENTS` keeps out.
    // History: design-decisions.md, "Trims: api, events, harness and config module docs, second packet", crates/glasshouse/src/events/mod.rs `LifecycleEvent::FileTouched`.
    FileTouched { path: String },
}

impl LifecycleEvent {
    /// The session state this event implies, when it implies one.
    ///
    /// `None` is the common and correct answer. An event that says nothing
    /// about a *session's* state — output ending, a keystroke arriving, a
    /// process exiting — leaves the record exactly as it was.
    ///
    /// [`LifecycleEvent::ProcessExited`] is `None` on purpose and not by
    /// omission. A session ending is decided on the exit path, which has the
    /// status in hand and calls [`ProcessExit::session_state`]; routing it
    /// through here as well would let a translated event and the operating
    /// system race to describe the same fact.
    /// The stored name of this event's variant.
    ///
    /// One word per variant, and the same word the project database's
    /// `lifecycle_events.kind` column is constrained to. It lives here rather
    /// than in [`mod@crate::events::log`] so that adding a variant to the
    /// enum is a compile error in the one place that has to classify it,
    /// instead of a storage error much later.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::SessionStarted => "session_started",
            Self::SessionResumed => "session_resumed",
            Self::TurnStarted => "turn_started",
            Self::TurnEnded { .. } => "turn_ended",
            Self::WaitingForUser => "waiting_for_user",
            Self::TextDelivered { .. } => "text_delivered",
            Self::InterruptDelivered { .. } => "interrupt_delivered",
            Self::ProcessExited { .. } => "process_exited",
            Self::OutputEnded => "output_ended",
            Self::GatewayUnhealthy { .. } => "gateway_unhealthy",
            Self::GatewayBackendChanged { .. } => "gateway_backend_changed",
            Self::FileTouched { .. } => "file_touched",
        }
    }

    pub fn implied_state(&self) -> Option<crate::session::SessionLifecycle> {
        use crate::session::SessionLifecycle as State;
        match self {
            Self::SessionStarted | Self::SessionResumed | Self::TurnStarted => Some(State::Running),
            // The turn is over and the session is alive and waiting for
            // whatever comes next. A turn that ended badly is still an alive
            // session: recording it as failed would make a perfectly usable
            // session look dead in every listing.
            Self::TurnEnded { .. } => Some(State::Idle),
            Self::WaitingForUser => Some(State::WaitingForUser),
            Self::TextDelivered { .. }
            | Self::InterruptDelivered { .. }
            | Self::ProcessExited { .. }
            | Self::OutputEnded
            | Self::GatewayUnhealthy { .. }
            // A backend moving says nothing about whether the session
            // itself is running: the harness is still alive on screen
            // either side of a failover.
            | Self::GatewayBackendChanged { .. }
            // Migration 26. A file being edited is not a transition: the
            // hook that records it fires *during* a turn the state machine
            // already has a `TurnStarted` for, so answering `Running` here
            // would be a second, later source for a fact `crate::session::
            // lifecycle` already established from a harness report — and a
            // source whose input is a `PostToolUse` payload.
            | Self::FileTouched { .. } => None,
        }
    }
}

/// How a turn ended, according to the harness that ran it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnOutcome {
    /// The harness finished what it was asked to do.
    Completed,
    /// The turn ended badly. The *session* is still alive.
    Failed,
}

/// Who put something into a session's terminal.
///
/// A harness cannot tell the difference — bytes are bytes — which is exactly
/// why Glasshouse's own log must. An orchestrator driving a worker and a
/// person typing into it produce identical input and very different
/// accountability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageOrigin {
    /// A person, at a keyboard.
    UserKeystroke,
    /// Glasshouse, or an orchestrator through it.
    Machine,
}

impl MessageOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UserKeystroke => "user_keystroke",
            Self::Machine => "machine",
        }
    }
}

impl std::fmt::Display for MessageOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

/// How a child process ended, as the operating system reported it.
///
/// # This type deliberately cannot say whether the work finished
///
/// There is no `success()` here, no `From<ProcessExit>` for [`TurnOutcome`],
/// and no function in this crate that takes one and returns the other. The
/// omission is the point: an exit status genuinely does not contain that
/// information, and a type that offered it would be inviting every caller to
/// make the one inference the capability map forbids.
///
/// What it *can* say is whether the session is still usable, which is a
/// different question with an answer that is actually in the status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessExit {
    code: u32,
    signal: Option<String>,
}

impl ProcessExit {
    /// Read an exit as the pseudo-terminal layer reported it.
    pub fn from_status(status: &crate::pty::ExitStatus) -> Self {
        Self {
            code: status.code(),
            signal: status.signal().map(str::to_owned),
        }
    }

    /// Rebuild an exit from what the event log stored.
    ///
    /// Crate-private and deliberately narrow: the only production caller is
    /// [`mod@crate::events::log`], reconstructing a row it wrote itself. It
    /// exists because reading the raw stream back means rebuilding the typed
    /// event, and a struct nobody outside the operating-system path can
    /// construct cannot be read back at all.
    ///
    /// Note what it still does not offer — there is no `success()` here
    /// either, so a caller holding a reconstructed exit is in exactly the
    /// same position as one holding a fresh one.
    pub(crate) fn from_parts(code: u32, signal: Option<String>) -> Self {
        Self { code, signal }
    }

    pub fn code(&self) -> u32 {
        self.code
    }

    /// Name of the signal that killed the process, when one did.
    pub fn signal(&self) -> Option<&str> {
        self.signal.as_deref()
    }

    /// Whether the process died rather than finished.
    ///
    /// A signal, or a non-zero code. Both mean the same thing for a session:
    /// there is no harness there any more and it did not leave on its own
    /// terms. Note what this is *not* — it is not "the task failed". A
    /// harness killed mid-way through successful work crashes; a harness that
    /// exits zero having achieved nothing does not.
    pub fn is_crash(&self) -> bool {
        self.signal.is_some() || self.code != 0
    }

    /// The state a session moves to when its process ends this way.
    ///
    /// This is the exit path's authority and the only place a session is
    /// ended. See [`LifecycleEvent::implied_state`] for why a translated
    /// event never does it.
    pub fn session_state(&self) -> crate::session::SessionLifecycle {
        if self.is_crash() {
            crate::session::SessionLifecycle::Failed
        } else {
            crate::session::SessionLifecycle::Stopped
        }
    }
}

impl std::fmt::Display for ProcessExit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (&self.signal, self.code) {
            (Some(signal), _) => write!(f, "terminated by {signal}"),
            (None, 0) => f.write_str("exited with code 0"),
            (None, code) => write!(f, "exited with code {code}"),
        }
    }
}

/// Why a backend resource stopped serving.
///
/// Kept apart from process failure because the two need opposite responses: a
/// harness process that dies is one session's problem, and a backend that
/// stops answering is every session pointed at it. Collapsing them would
/// either kill sessions that are fine or leave a dead backend serving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatewayFailure {
    /// Nothing is listening, or the connection was refused.
    Unreachable,
    /// It accepted the request and never answered within the bound.
    TimedOut,
    /// It answered, and the answer was an error.
    Rejected,
}

impl GatewayFailure {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unreachable => "unreachable",
            Self::TimedOut => "timed out",
            Self::Rejected => "rejected",
        }
    }
}

impl std::fmt::Display for GatewayFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

/// What an adapter saw, before translation.
///
/// Kept for troubleshooting only: when a harness reports something this build
/// does not recognise, the log line naming what arrived is the difference
/// between a five-minute fix and a bisect.
///
/// # What an adapter may put in `detail`, and what it may not
///
/// `detail` is free for an adapter to fill with the parts of its native
/// payload that are *about the event*. It must never carry the conversation.
/// Claude Code's and Codex's hook payloads both include the user's prompt and
/// the model's last message; Glasshouse's handler drains that stream without
/// reading it, and neither adapter supplies a `detail` at all. The mechanism
/// preserves whatever an adapter hands it — the policy about what an adapter
/// hands it belongs to the adapter, and is recorded in
/// `docs/product/design-decisions.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawObservation<'a> {
    /// The harness that reported it, as an integration slug.
    pub harness: &'a str,
    /// The event's own name, exactly as the harness spelled it.
    pub event: &'a str,
    /// Anything else the adapter judged useful, or `None`.
    pub detail: Option<&'a str>,
}

impl<'a> RawObservation<'a> {
    pub fn new(harness: &'a str, event: &'a str) -> Self {
        Self {
            harness,
            event,
            detail: None,
        }
    }

    pub fn with_detail(mut self, detail: &'a str) -> Self {
        self.detail = Some(detail);
        self
    }

    /// Preserve this observation in the debug log.
    ///
    /// `debug`, not `info`: this is diagnostic volume nobody wants by
    /// default, and it is exactly what someone wants when an event is not
    /// arriving.
    pub fn preserve(&self) {
        tracing::debug!(
            harness = self.harness,
            event = self.event,
            detail = self.detail,
            "raw harness observation"
        );
    }
}

/// What happened when one backend resource was found unhealthy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Degradation {
    /// The backend resource, by [`crate::profile::BackendResource::slug`].
    pub resource: String,
    pub reason: GatewayFailure,
    /// The sessions that were actually running on it.
    pub affected: Vec<crate::session::SessionId>,
}

/// Degrade one unhealthy backend resource, and publish the fact against every
/// session that was running on it.
///
/// Takes records rather than guessing: a session is affected if, and only
/// if, its own record says it resolved to this backend resource — nothing
/// is inferred from the harness, the launch profile, or a session being
/// live. A session with no recorded backend resource is never affected
/// (a native subscription, or one recorded before the column existed).
///
/// No session's lifecycle moves: a gateway failing is not a harness process
/// failing, so this publishes [`LifecycleEvent::GatewayUnhealthy`], which
/// [`LifecycleEvent::implied_state`] deliberately maps to `None` — the
/// harness is still running, still on screen, still steerable, and marking
/// it failed would be a lie about a live process.
// History: design-decisions.md, "Trims: api, events, harness and config module docs, second packet", crates/glasshouse/src/events/mod.rs `degrade_resource`.
pub fn degrade_resource(
    bus: &EventBus,
    records: &[crate::session::SessionRecord],
    resource: &str,
    reason: GatewayFailure,
) -> Degradation {
    let affected: Vec<crate::session::SessionId> = records
        .iter()
        .filter(|record| record.backend_resource.as_deref() == Some(resource))
        .map(|record| record.id.clone())
        .collect();

    for id in &affected {
        bus.publish(
            id,
            LifecycleEvent::GatewayUnhealthy {
                resource: resource.to_owned(),
                reason,
            },
        );
    }

    Degradation {
        resource: resource.to_owned(),
        reason,
        affected,
    }
}

/// What a consumer may conclude about a session's work from its history.
///
/// `None` means Glasshouse does not know — which is the answer for a session
/// whose process exited cleanly, whose output ended, and whose harness never
/// said it had finished anything. That is not a gap in this function; it is
/// the capability map's standing rule, in the only place a caller could
/// otherwise have got a different answer.
///
/// The most recent turn wins: a session that completed a turn and then
/// started and failed another reports the failure.
pub fn task_outcome(history: &[RecordedEvent]) -> Option<TurnOutcome> {
    // Walking backwards and *stopping* at the first turn boundary, rather
    // than searching past it. A turn that started and has not ended yet
    // erases an older verdict: the work in flight is what the caller is
    // asking about, and answering with the previous turn's result would be
    // the same class of mistake as reading a completion out of silence.
    for recorded in history.iter().rev() {
        match recorded.event() {
            LifecycleEvent::TurnEnded { outcome } => return Some(*outcome),
            LifecycleEvent::TurnStarted => return None,
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests;
