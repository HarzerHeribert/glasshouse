//! The normalized Glasshouse lifecycle-event stream.
//!
//! One stream, shared by the TUI, the router, memory, the API and the MCP
//! surface. Adapters translate a harness's own vocabulary into
//! [`LifecycleEvent`]; nothing downstream ever learns which harness produced
//! one. That is the whole architectural requirement of the capability map's
//! Phase 12, and it is why this module names no harness at all —
//! `no_harness_is_named_in_the_core_event_stream` keeps it that way, and
//! [`crate::session::lifecycle`] is the single place allowed to know either
//! harness's spelling.
//!
//! # The two distinctions this module exists to preserve
//!
//! Both have already produced defects in products of this shape, so both are
//! expressed in the types rather than left to a reader's discipline.
//!
//! **A process exiting is not a turn completing.** A harness exits zero when
//! the user types `/quit` halfway through a task and exits zero when it has
//! finished; the exit status cannot tell those apart, because the information
//! is not in it. So [`ProcessExit`] has no `success()`, there is no
//! conversion from it to [`TurnOutcome`], and [`task_outcome`] — the one
//! function a consumer calls to ask "did the work finish?" — answers `None`
//! for a session that only ever exited.
//!
//! **Waiting for the user is not idle.** [`LifecycleEvent::WaitingForUser`]
//! is recorded only when a harness says so. Silence is never promoted to it,
//! and never demoted from it either.
//!
//! # Why quiet can never become completion by accident
//!
//! The map carries a standing rule: *do not infer successful task completion
//! solely because a child process became quiet.* Being careful is not a
//! mechanism, so two independent ones enforce it:
//!
//! 1. [`LifecycleEvent::TurnEnded`] is constructed in exactly **one**
//!    production function in this crate — the harness translator in
//!    [`crate::session::lifecycle`], whose only input is an event name a
//!    harness reported. `turn_completion_is_minted_in_exactly_one_place`
//!    scans the source and fails if a second site appears.
//! 2. [`task_outcome`] reads `TurnEnded` records and nothing else, so a
//!    history full of clean exits and ended output still answers "unknown".
//!
//! # What is *not* here
//!
//! Durable storage. The map splits raw event recording into its own phase,
//! and this module offers [`EventSink`] as the seam it will attach to: the
//! bus hands every recorded event to a sink if one is installed, and holds a
//! bounded in-memory history either way.

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
    /// # Touched means changed, and read-shaped tools are deliberately absent
    ///
    /// `Read`, `Grep` and `Glob` carry paths too and none of them is recorded.
    /// A memory can honestly reference a file the session *changed*; that the
    /// session looked at a file is a much weaker fact wearing the same shape,
    /// and admitting it here would let map line 1139's `referenced`
    /// association be earned by a glance.
    ///
    /// # The path, and what it is not
    ///
    /// Repo-relative and `/`-separated —
    /// [`crate::memory::normalize_observed_path`]'s spelling, applied
    /// by the writer, so a path outside the project root is dropped before it
    /// ever reaches an event rather than stored and filtered later. It is the
    /// user's own file name and nothing else: no content, no diff, no tool
    /// output.
    ///
    /// # Not a state transition
    ///
    /// [`LifecycleEvent::implied_state`] answers `None`. A session editing a
    /// file says nothing about whether it is running, idle or waiting — the
    /// hook that records this fires while the harness is mid-turn, and
    /// promoting that to `Running` would let a `PostToolUse` payload reach the
    /// session state machine, which is exactly what `REPORTED_EVENTS` keeps
    /// out.
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
/// # Why this takes records rather than guessing
///
/// A session is affected if, and only if, its own record says it resolved to
/// this backend resource. Nothing is inferred from the harness, from the
/// launch profile, or from the session being live.
///
/// The consequence is the capability line: **a session with no recorded
/// backend resource is never affected.** That is a native subscription — a
/// harness talking to its own vendor on the user's own account, which a
/// Glasshouse gateway is not in the path of — or a session recorded before
/// the column existed. Either way, a gateway that stopped answering has
/// nothing to do with it, and degrading it would take away a session that is
/// working perfectly.
///
/// # Why no session's lifecycle moves
///
/// A gateway failing is not a harness process failing, and the two need
/// opposite responses. This publishes [`LifecycleEvent::GatewayUnhealthy`],
/// which [`LifecycleEvent::implied_state`] deliberately maps to `None`: the
/// harness is still running, still on screen, and still steerable by the user
/// even while its backend is unreachable. Marking it failed would be a lie
/// about a live process.
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
mod tests {
    use super::*;
    use crate::session::{SessionId, SessionLifecycle};

    fn bus() -> EventBus {
        EventBus::with_history_and_clock(64, std::sync::Arc::new(|| 1_000))
    }

    fn session(name: &str) -> SessionId {
        SessionId::new(name)
    }

    /// The capability map's standing rule, as behaviour: *do not infer
    /// successful task completion solely because a child process became
    /// quiet.*
    ///
    /// This session did everything a quiet, cleanly finished one does. Its
    /// output ended and its process exited with code zero, which is what a
    /// harness does when the user types `/quit` in the middle of a task. No
    /// harness ever said a turn finished, so Glasshouse does not know that one
    /// did — and `None` is the only honest answer.
    #[test]
    fn a_quiet_process_that_exited_cleanly_reports_no_task_outcome() {
        let bus = bus();
        let id = session("quiet");
        bus.publish(&id, LifecycleEvent::SessionStarted);
        bus.publish(&id, LifecycleEvent::TurnStarted);
        bus.publish(&id, LifecycleEvent::OutputEnded);
        bus.publish(
            &id,
            LifecycleEvent::ProcessExited {
                exit: ProcessExit {
                    code: 0,
                    signal: None,
                },
            },
        );

        assert_eq!(
            task_outcome(&bus.history_for(&id)),
            None,
            "a clean exit and no more output is not the harness saying it finished"
        );
    }

    /// The other half of the same rule: when the harness *does* say so, that
    /// is exactly what is reported. A test that only proved `None` would pass
    /// on a function that always returned `None`.
    #[test]
    fn only_a_harness_report_says_the_work_finished() {
        let bus = bus();
        let id = session("reported");
        bus.publish(&id, LifecycleEvent::TurnStarted);
        bus.publish(
            &id,
            LifecycleEvent::TurnEnded {
                outcome: TurnOutcome::Completed,
            },
        );
        assert_eq!(
            task_outcome(&bus.history_for(&id)),
            Some(TurnOutcome::Completed)
        );

        // And a turn that ended badly is reported as having ended badly,
        // never as unknown and never as completed.
        let failed = session("failed-turn");
        bus.publish(
            &failed,
            LifecycleEvent::TurnEnded {
                outcome: TurnOutcome::Failed,
            },
        );
        assert_eq!(
            task_outcome(&bus.history_for(&failed)),
            Some(TurnOutcome::Failed)
        );
    }

    /// A turn in flight erases an older verdict. Asking "did the work finish?"
    /// about a session that has since started something else must not answer
    /// with the previous turn.
    #[test]
    fn a_turn_in_flight_is_not_the_previous_turns_verdict() {
        let bus = bus();
        let id = session("busy");
        bus.publish(
            &id,
            LifecycleEvent::TurnEnded {
                outcome: TurnOutcome::Completed,
            },
        );
        bus.publish(&id, LifecycleEvent::TurnStarted);
        assert_eq!(task_outcome(&bus.history_for(&id)), None);
    }

    /// Waiting for the user is not idle, and neither is ever inferred from
    /// the other. Only one event in the whole enum implies `Idle`; if a
    /// second ever does, this fails.
    #[test]
    fn waiting_for_user_is_a_state_of_its_own() {
        assert_eq!(
            LifecycleEvent::WaitingForUser.implied_state(),
            Some(SessionLifecycle::WaitingForUser)
        );

        let every = [
            LifecycleEvent::SessionStarted,
            LifecycleEvent::SessionResumed,
            LifecycleEvent::TurnStarted,
            LifecycleEvent::TurnEnded {
                outcome: TurnOutcome::Completed,
            },
            LifecycleEvent::WaitingForUser,
            LifecycleEvent::TextDelivered {
                origin: MessageOrigin::Machine,
                bytes: 1,
            },
            LifecycleEvent::InterruptDelivered {
                origin: MessageOrigin::UserKeystroke,
            },
            LifecycleEvent::ProcessExited {
                exit: ProcessExit {
                    code: 0,
                    signal: None,
                },
            },
            LifecycleEvent::OutputEnded,
            LifecycleEvent::GatewayUnhealthy {
                resource: "r".to_owned(),
                reason: GatewayFailure::TimedOut,
            },
            LifecycleEvent::GatewayBackendChanged {
                provider: "p".to_owned(),
                model: "m".to_owned(),
                cause: "c".to_owned(),
            },
        ];
        let idle: Vec<&LifecycleEvent> = every
            .iter()
            .filter(|event| event.implied_state() == Some(SessionLifecycle::Idle))
            .collect();
        assert_eq!(
            idle.len(),
            1,
            "exactly one event may mean idle, and it is a turn ending: {idle:?}"
        );
        assert!(matches!(idle[0], LifecycleEvent::TurnEnded { .. }));
    }

    /// Neither a process ending nor its output ending moves a session's state
    /// through translation. The exit path decides that, with the status in
    /// hand — see [`ProcessExit::session_state`].
    #[test]
    fn an_exit_and_a_silence_imply_nothing_about_session_state() {
        assert_eq!(LifecycleEvent::OutputEnded.implied_state(), None);
        for exit in [
            ProcessExit {
                code: 0,
                signal: None,
            },
            ProcessExit {
                code: 137,
                signal: Some("SIGKILL".to_owned()),
            },
        ] {
            assert_eq!(
                LifecycleEvent::ProcessExited { exit }.implied_state(),
                None,
                "a translated exit must not race the operating system"
            );
        }
    }

    #[test]
    fn a_crash_and_a_departure_are_different_session_states() {
        let clean = ProcessExit {
            code: 0,
            signal: None,
        };
        assert!(!clean.is_crash());
        assert_eq!(clean.session_state(), SessionLifecycle::Stopped);

        for crashed in [
            ProcessExit {
                code: 3,
                signal: None,
            },
            ProcessExit {
                code: 0,
                signal: Some("SIGKILL".to_owned()),
            },
        ] {
            assert!(crashed.is_crash(), "{crashed:?}");
            assert_eq!(crashed.session_state(), SessionLifecycle::Failed);
        }
    }

    /// A backend resource going unhealthy touches the sessions that were on
    /// it and nothing else. A session with no recorded backend resource — a
    /// harness talking to its own vendor on the user's own subscription — is
    /// not Glasshouse's gateway's business, and degrading it would take away
    /// a session that is working.
    #[test]
    fn degrading_a_gateway_leaves_unrelated_native_subscriptions_alone() {
        let bus = bus();
        let on_gateway = record("a", Some("glasshouse-gateway"));
        let native = record("b", None);
        let other_gateway = record("c", Some("some-other-backend"));
        let records = vec![on_gateway.clone(), native.clone(), other_gateway.clone()];

        let degradation = degrade_resource(
            &bus,
            &records,
            "glasshouse-gateway",
            GatewayFailure::TimedOut,
        );

        assert_eq!(degradation.affected, vec![on_gateway.id.clone()]);
        assert!(bus.history_for(&native.id).is_empty(), "native untouched");
        assert!(bus.history_for(&other_gateway.id).is_empty());

        let events = bus.history_for(&on_gateway.id);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].event(),
            &LifecycleEvent::GatewayUnhealthy {
                resource: "glasshouse-gateway".to_owned(),
                reason: GatewayFailure::TimedOut,
            }
        );
    }

    /// A gateway failing is not a harness process failing. The harness is
    /// still running and still steerable, so nothing here may mark it dead.
    #[test]
    fn a_gateway_failure_never_ends_a_session() {
        let bus = bus();
        let affected = record("a", Some("gw"));
        degrade_resource(
            &bus,
            std::slice::from_ref(&affected),
            "gw",
            GatewayFailure::Unreachable,
        );

        for recorded in bus.history_for(&affected.id) {
            assert_eq!(
                recorded.event().implied_state(),
                None,
                "a gateway failure must not move a live session's state"
            );
        }
        assert_eq!(task_outcome(&bus.history_for(&affected.id)), None);
    }

    fn record(id: &str, backend_resource: Option<&str>) -> crate::session::SessionRecord {
        crate::session::SessionRecord {
            id: SessionId::new(id),
            project_id: "project".to_owned(),
            harness: "claude-code".to_owned(),
            native_session_id: None,
            role: crate::session::SessionRole::Normal,
            lifecycle: SessionLifecycle::Running,
            presentation: crate::session::SessionPresentation::Embedded,
            created_at: 1,
            last_activity_at: 2,
            launch_profile: None,
            backend_resource: backend_resource.map(str::to_owned),
            model: None,
            pairing_class: None,
            protocol: None,
            response_profile: None,
            response_mechanism: None,
            display_name: None,
            purpose: None,
            source_session_id: None,
            observed_compactions: None,
            presentation_ref: None,
            last_seen_commit: None,
            entitlement: None,
        }
    }

    // ---------------------------------------------------------------
    // Source guards
    //
    // Both scan by `str::lines`, which strips a carriage return for us, and
    // both are exercised against a CRLF copy of their own input below. A
    // multi-line literal search would find nothing on a checkout where Git
    // converted line endings, and would do it silently — see
    // `docs/product/design-decisions.md`, "A source-scanning guard reads by
    // lines".
    // ---------------------------------------------------------------

    /// Production source of a module: everything before its
    /// `#[cfg(test)] mod tests` block, with comment lines removed.
    ///
    /// **Not** "everything before the first `#[cfg(test)]`", which is what
    /// this helper did until a mutation caught it. `session/runtime.rs`
    /// carries a `#[cfg(test)] const` two hundred lines in, so cutting at the
    /// first attribute read a fifth of the file and silently exempted the
    /// rest — including the exit path, which is exactly where a forbidden
    /// inference would be written. A `TurnEnded` planted there survived the
    /// scan. Anchoring on the attribute that actually introduces `mod tests`
    /// is what makes the scan cover what it claims to cover.
    ///
    /// Reads by `str::lines`, which strips a carriage return for us, so the
    /// scan is blind to line endings by construction rather than by anyone
    /// remembering — see `docs/product/design-decisions.md`, "A source-scanning
    /// guard reads by lines".
    fn production_code(source: &str) -> String {
        let lines: Vec<&str> = source.lines().collect();
        let end = lines
            .windows(2)
            .position(|pair| {
                pair[0].trim_end() == "#[cfg(test)]" && pair[1].trim_end().starts_with("mod tests")
            })
            .unwrap_or(lines.len());
        lines[..end]
            .iter()
            .filter(|line| !line.trim_start().starts_with("//"))
            .copied()
            .collect::<Vec<_>>()
            .join("\n")
    }

    const EVENT_MODULES: [(&str, &str); 3] = [
        ("events/mod.rs", include_str!("mod.rs")),
        ("events/bus.rs", include_str!("bus.rs")),
        ("events/log.rs", include_str!("log.rs")),
    ];

    /// "Adapters translate native observations into core events; consumers
    /// must not create competing harness-specific lifecycle architectures."
    ///
    /// The core stream is delivered to the orchestration layer without
    /// coupling it to any harness, and this is what keeps that true: if the
    /// word `codex` ever appears in production code here, some consumer can
    /// branch on it, and the single normalized stream has quietly become two.
    ///
    /// `crate::session::lifecycle` is the one module allowed to know a
    /// harness's vocabulary, and it converts *into* these types.
    #[test]
    fn no_harness_is_named_in_the_core_event_stream() {
        for (name, source) in EVENT_MODULES {
            let code = production_code(source).to_ascii_lowercase();
            for harness in [
                "claude",
                "codex",
                "antigravity",
                "opencode",
                "cursor",
                "gemini",
            ] {
                assert!(
                    !code.contains(harness),
                    "{name} names `{harness}`; the core event stream must be \
                     harness-independent, and a consumer that can see a harness \
                     here will eventually branch on it"
                );
            }
        }
    }

    /// The one mechanism that makes the map's standing rule impossible to
    /// break by accident rather than merely absent.
    ///
    /// `LifecycleEvent::TurnEnded` is the only event carrying a claim about
    /// the work itself, and it is constructed in exactly one production
    /// function in the crate: the harness translator in
    /// `crate::session::lifecycle`, whose entire input is an event name a
    /// harness reported. A second construction site anywhere — in the exit
    /// path, in a quiet-timer, in a consumer — fails this test, which is the
    /// point: writing the forbidden inference requires deleting a test that
    /// says why it is forbidden.
    #[test]
    fn turn_completion_is_minted_in_exactly_one_place() {
        let sites = mint_sites();
        assert!(
            !sites.is_empty(),
            "the scan found no construction at all, so it is proving nothing"
        );
        let files: std::collections::BTreeSet<&str> = sites.iter().map(|(file, _)| *file).collect();
        assert_eq!(
            files,
            ["session/lifecycle.rs"].into_iter().collect(),
            "`LifecycleEvent::TurnEnded` may be constructed only by the harness \
             translator, whose whole input is an event name a harness reported. \
             A second site is the forbidden inference growing a home: {sites:#?}"
        );
    }

    /// Every place a `LifecycleEvent::TurnEnded` value is *built*.
    ///
    /// A match arm mentions the variant without building one, so it is
    /// excluded by the position of the `=>`: in an arm the fat arrow comes
    /// after the pattern, and in a construction any `=>` on the line comes
    /// before the expression being built. The bare variant name in the enum's
    /// own declaration carries no `LifecycleEvent::` path and is excluded by
    /// that.
    fn mint_sites() -> Vec<(&'static str, String)> {
        let mut sites = Vec::new();
        for (name, source) in SOURCES_THAT_MAY_NOT_MINT_A_TURN {
            for line in production_code(source).lines() {
                sites.extend(is_mint_site(line).then(|| (name, line.trim().to_owned())));
            }
        }
        sites
    }

    fn is_mint_site(line: &str) -> bool {
        let Some(at) = line.find("LifecycleEvent::TurnEnded") else {
            return false;
        };
        !line[at..].contains("=>")
    }

    /// Every production module that could plausibly reach for a turn verdict:
    /// the event stream itself, the runtime that watches processes exit, the
    /// API that drives sessions, and the recovery planner that reasons about
    /// failed ones. The translator is in the list so the one legitimate site
    /// is counted rather than exempted.
    const SOURCES_THAT_MAY_NOT_MINT_A_TURN: [(&str, &str); 6] = [
        ("events/mod.rs", include_str!("mod.rs")),
        ("events/bus.rs", include_str!("bus.rs")),
        (
            "session/lifecycle.rs",
            include_str!("../session/lifecycle.rs"),
        ),
        ("session/runtime.rs", include_str!("../session/runtime.rs")),
        ("session/api.rs", include_str!("../session/api.rs")),
        (
            "session/recovery.rs",
            include_str!("../session/recovery.rs"),
        ),
    ];

    /// The scan must actually read the file it names.
    ///
    /// This is the assertion that would have caught the defect a surviving
    /// mutation found: `production_code` used to cut at the *first*
    /// `#[cfg(test)]`, and `session/runtime.rs` has one two hundred lines in,
    /// so the guard above was silently reading a fifth of its target and
    /// passing for that reason. A source scan whose reach nobody checked is
    /// the third way this project has produced a test that passed for the
    /// wrong reason.
    #[test]
    fn the_scan_reaches_the_end_of_every_file_it_claims_to_read() {
        // The last item in each file's production code. Named rather than
        // counted: a ratio would say nothing about a module that is mostly
        // tests, and it is *reach past an early `#[cfg(test)]`* that was
        // broken, which an anchor states directly.
        let anchors = [
            (
                "session/runtime.rs",
                include_str!("../session/runtime.rs"),
                "fn short(",
            ),
            ("events/bus.rs", include_str!("bus.rs"), "fn system_clock("),
            (
                "events/mod.rs",
                include_str!("mod.rs"),
                "pub fn task_outcome(",
            ),
            (
                "session/lifecycle.rs",
                include_str!("../session/lifecycle.rs"),
                "pub fn may_apply(",
            ),
        ];
        for (name, source, anchor) in anchors {
            assert!(
                source.contains(anchor),
                "{name}: the anchor `{anchor}` is gone, so this test has \
                 stopped proving anything — pick a new one at the end of the \
                 production code"
            );
            assert!(
                production_code(source).contains(anchor),
                "{name}: the scan stops before `{anchor}`, so the code between \
                 there and wherever it stopped is exempt from a guard that \
                 claims to cover the file"
            );
        }

        // And the scan must stop at the test module rather than run into it.
        for (name, source) in SOURCES_THAT_MAY_NOT_MINT_A_TURN {
            let code = production_code(source);
            assert!(
                !code.contains("mod tests {"),
                "{name}: the scan ran into the test module, so a construction \
                 written in a test would be counted as production"
            );
        }
    }

    /// Both scans must be blind to line endings, and the only way to know
    /// that is to run them against a CRLF copy. An LF checkout never
    /// exercises the broken path, so without this the property is untested
    /// exactly where it was needed.
    #[test]
    fn the_source_guards_are_blind_to_line_endings() {
        for (name, source) in SOURCES_THAT_MAY_NOT_MINT_A_TURN {
            // Build both sides from a normalised base, so the input does not
            // vary with how this file happened to be checked out.
            let lf = source.replace("\r\n", "\n");
            let crlf = lf.replace('\n', "\r\n");

            let lf_code = production_code(&lf);
            let crlf_code = production_code(&crlf);
            assert_eq!(
                lf_code.lines().count(),
                crlf_code.lines().count(),
                "{name}: the scan reads a different number of lines under CRLF"
            );

            let count = |code: &str| code.lines().filter(|line| is_mint_site(line)).count();
            assert_eq!(
                count(&lf_code),
                count(&crlf_code),
                "{name}: the mint-site scan disagrees with itself under CRLF"
            );
        }
    }
}
