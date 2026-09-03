//! What the shell knows and how keys change it.
//!
//! Kept apart from rendering for the same reason the wizard is: state that
//! answers keys without drawing anything can be tested exhaustively without a
//! terminal, and a view that only reads state cannot accidentally become the
//! place a decision is made.
//!
//! Nothing here starts, stops, or touches a process. Moving between sessions
//! changes which one is *presented*, and that distinction is the whole point of
//! the capability: a user flipping through sessions must never be terminating
//! them.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Style;

use crate::config::{
    FreeResourceRef, Layer, Layered, PremiumReservePercent, ProfileApproval, ProfileBackend,
    ProfileConfig, ProviderConfig, RouterCostMicroUsd, RouterLatencyMs, RoutingModelChoice,
    StoredCredentialRef,
};
use crate::events::{LifecycleEvent, MessageOrigin, RecordedEvent, TurnOutcome};
use crate::harness::{Declared, WireProtocol};
use crate::integrations::{IntegrationId, IntegrationKind, IntegrationStatus};
use crate::platform::exec;
use crate::provider::cache::ModelCatalogue;
use crate::provider::discovery::{ProbeOutcome, ProbeTarget};
use crate::routing::disposable::DisposableChoice;
use crate::secret::native::{PreferNativeSecretStore, os_credential_for_variable};
use crate::secret::{SecretRef, SecretStore};
use crate::session::{SessionDisposition, SessionId, SessionPresentation, SessionRecord};

mod knowledge;
mod overview;
mod route;
mod settings;

#[cfg(test)]
#[path = "../tests/state_tests.rs"]
mod tests;

pub use knowledge::{KnowledgeSection, MemoryDetail, ProjectKnowledgeState, ProjectMemoryState};
pub use overview::{OverviewState, ProjectOverviewState};
pub use route::{
    RouteDecisionRow, RouteDecisionsState, RouteEvidenceRow, RouteEvidenceState, RouteHealthRow,
    RouteHealthState,
};
pub use settings::{
    HarnessRow, IntegrationRow, MemoryRow, MemorySettingsEdit, ModelRefresh, ProbeKind,
    ProfileInputView, ProfileRow, ProfileSettingsEdit, ProviderInputView, ProviderNotice,
    ProviderProbeIntent, ProviderProbeResult, ProviderRow, ProviderSettingsEdit, ReachabilityCheck,
    RoutingInputView, RoutingRow, RoutingSettingsEdit, SettingsEdit, SettingsPathInputView,
    SettingsSection, SettingsState, format_usd,
};

// Brought into `state`'s own namespace, unexported, purely so `state::tests`'s
// `use super::*;` can reach them — the same reasoning `routing::evidence::mod`
// keeps `row_to_observation` for: a private item in a parent module is visible
// to every descendant module, but only if the parent actually names it.
// `#[cfg(test)]`, matching `mod tests` itself, so a non-test build does not
// see these as unused.
#[cfg(test)]
use overview::{encode, is_session_escape};
#[cfg(test)]
use settings::probe_endpoint;

/// A Glasshouse-owned screen drawn over the session viewport.
///
/// Overlays are transient by design: they are somewhere the user visits and
/// leaves, and leaving returns to the session that was already active rather
/// than closing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    /// Every session in the project, with the detail the bar has no room for,
    /// and the one place a session can be acted on *without* being brought
    /// into the viewport first. See [`OverviewState`] for the data behind it
    /// — this marker carries none of it, exactly as [`Overlay::Settings`]
    /// carries none of the Settings rows.
    Overview,
    /// Harnesses and Integrations configuration. See [`SettingsState`] for
    /// the data behind it — this marker carries none of it, the same way
    /// [`Overlay::Overview`] carries none of the session list it shows.
    Settings,
    /// The project-level summary: sessions grouped by role and lifecycle
    /// rather than listed one by one, active decisions and constraints, and
    /// unresolved memory todos. [`Overlay::Overview`]'s project-level
    /// sibling (Phase 41) — that overlay answers "what is this session
    /// doing", this one answers "what is this project doing". See
    /// [`ProjectOverviewState`] for the data behind it.
    ProjectOverview,
    /// Phase 47 line 1758: the presented session's own recent lifecycle
    /// events, reached deliberately rather than shown by default — see
    /// [`ShellState::open_session_events`]. Carries no data of its own: the
    /// events themselves already live in `ShellState::activity`, populated
    /// in production by [`ShellState::note_events`] whether or not this
    /// overlay is ever opened, exactly as [`Overlay::Overview`] carries none
    /// of the session list it shows.
    SessionEvents,
    /// The project's durable knowledge — active decisions, known
    /// constraints, implemented-or-planned features, failed approaches (kept
    /// as history regardless of status), and unresolved todos — each in its
    /// own labelled, grouped-text section. Phase 25, map lines 1098-1107.
    /// [`Overlay::ProjectOverview`]'s sibling: that overlay summarizes what
    /// the project is *doing* right now (sessions, live memory); this one
    /// summarizes what the project has *learned*. Deliberately plain text —
    /// line 1107 rules out a decorative node graph. Line 1105: a cursor
    /// selects one entry and Enter opens its rationale, source session,
    /// source commit and lifecycle state. See [`ProjectKnowledgeState`] for
    /// the data behind it.
    ProjectKnowledge,
    /// Phase 47 lines 1762 and 1764: a compact table of the distinct
    /// `(provider, model, route)` identities this project's own gateway has
    /// actually routed, with each identity's sample count, observation
    /// window and context state (warm, cold, or unknown). Deliberately
    /// narrow — see [`RouteEvidenceRow`] for exactly which columns this
    /// build can honestly show and why the rest have no producer yet.
    /// Read-only, like [`Overlay::ProjectOverview`] and
    /// [`Overlay::SessionEvents`]. See [`RouteEvidenceState`] for the data
    /// behind it.
    RouteEvidence,
    /// The project's raw memory — every [`crate::memory::MemoryKind`], at
    /// every [`crate::memory::MemoryStatus`], unfiltered and ungrouped. Map
    /// line 234: "allow the user to open a project-memory view from the
    /// keyboard." [`Overlay::ProjectKnowledge`]'s sibling: that overlay
    /// answers "what has this project learned" through five curated,
    /// status-filtered sections; this one answers "what does this project
    /// remember" and includes kinds `ProjectKnowledge` never has a section
    /// for — [`crate::memory::MemoryKind::Finding`] — as well as records at
    /// statuses `ProjectKnowledge` filters out. Same cursor-and-drill-down
    /// shape as `ProjectKnowledge` — see [`ProjectMemoryState`] for the data
    /// behind it.
    ProjectMemory,
    /// Phase 47 line 1765: what a local gateway has observed about each free
    /// resource, with **route health, immediate availability, cadence, quota
    /// reset and failure-domain evidence kept as five separate concepts** —
    /// never folded into one status word, which is what the line forbids and
    /// what `crate::provider::resources`'s own `render_health` does today,
    /// on a single line, for three of the five.
    ///
    /// [`Overlay::RouteEvidence`]'s sibling: that table answers *which*
    /// identities this gateway has actually routed, this one answers what is
    /// known right now about whether each of them can serve. Read-only, like
    /// every overlay above it. See [`RouteHealthState`] for the data behind
    /// it, and [`RouteHealthRow`] for why "unknown" is a real answer in
    /// three of the five concepts.
    RouteHealth,
    /// Why Glasshouse routed its own recent support jobs the way it did —
    /// the disposable-routing rationales `glasshouse hook` records in
    /// [`crate::evaluation`] once per completed turn.
    ///
    /// [`Overlay::RouteEvidence`]'s and [`Overlay::RouteHealth`]'s sibling,
    /// and the one that answers a different question from both: those two are
    /// about the *gateway* — which identities it routed, and what is known
    /// about their health — and this one is about a decision Glasshouse made
    /// for itself, with the named contributions behind it. Read-only, like
    /// every overlay above it. See [`RouteDecisionsState`], and
    /// [`RouteDecisionRow`] for why the rationale is text rather than a
    /// reconstructed choice.
    RouteDecisions,
}

/// Who currently owns the keyboard.
///
/// See `.agent-runtime/design-shell-session-modes.md` for the full design;
/// this is the switch the whole thing hangs on. [`ShellState::handle_key`]
/// consults it before any binding, which is what keeps the decision in one
/// place — the only thing this module's Phase 3 documentation promised would
/// have to change once a native session could own the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Glasshouse owns the keyboard. The default, and where the shell
    /// starts. Today's single-key bindings all work unchanged.
    Control,
    /// The focused session's PTY owns the keyboard. Every key is forwarded
    /// untouched — including `q`, Tab, and Ctrl-C — except `Ctrl-]`, which
    /// returns to [`Mode::Control`].
    Session,
}

/// What the run loop should do after a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Nothing changed; do not spend a frame.
    None,
    Redraw,
    /// Leave Glasshouse. Sessions are not affected — see [`ShellState`]'s note
    /// about presentation versus lifecycle.
    Quit,
    /// Bytes to write to whichever session currently holds the keyboard.
    /// Carried here rather than written from inside [`ShellState::handle_key`]
    /// — that is what keeps this module free of any process handling; the run
    /// loop is the only thing that ever touches a [`crate::session::SessionRuntime`].
    Forward(Vec<u8>),
    /// Start a new session. Resolving a harness, recording it, and spawning it
    /// all need machinery this module deliberately does not hold, so the run
    /// loop does the work and reports failure back with `set_status`.
    StartSession,
    /// Open Settings. Running discovery and reading `UserConfig`/
    /// `ProjectConfig` is file I/O this module deliberately does not hold —
    /// the run loop builds the rows and calls [`ShellState::open_settings`],
    /// reporting failure back with `set_status` exactly like
    /// [`Action::StartSession`].
    OpenSettings,
    /// Persist every pending Settings edit to the user-level configuration
    /// file. The run loop performs the write and refreshes the rows shown.
    SaveUserSettings,
    /// Persist every pending Settings edit to the project-level
    /// configuration file. Only ever produced after the user has explicitly
    /// confirmed inside the Settings overlay — see [`SettingsState`].
    SaveProjectSettings,
    /// Write the credential the user just typed into the operating system's
    /// own secure store. The run loop performs the write — see
    /// [`ShellState::take_provider_credential_entry`], which is also the
    /// only way the typed value ever leaves the Settings overlay.
    StoreProviderCredential,
    /// Remove the selected provider's stored credential from the OS store
    /// and drop the reference from its configuration. The run loop performs
    /// the deletion — see
    /// [`ShellState::selected_provider_stored_credentials`].
    DeleteProviderCredential,
    /// Make the network request the Settings overlay just planned — Phase
    /// 9D lines 1 and 2.
    ///
    /// The run loop performs it, on a thread of its own, for the reason this
    /// whole batch exists: a blocking network call on the thread that draws
    /// the terminal freezes it, and a frozen terminal and a slow one look
    /// identical to the person in front of them. See
    /// [`ShellState::take_provider_probe_intent`], which is the only way the
    /// request leaves this module, and `shell::spawn_provider_probe`, which
    /// is the only thing that makes it.
    RunProviderProbe,
    /// Start a new session with no viewport — Phase 4's headless
    /// presentation mode. Identical to [`Action::StartSession`] in every
    /// respect except the presentation the session is recorded and started
    /// under, so there is exactly one place that knows how to start a
    /// session; see `shell::start_session`.
    StartHeadlessSession,
    /// Interrupt the session the overview's cursor is on — Phase 4's "send
    /// interrupt signals to a PTY session".
    ///
    /// Carries the session rather than leaving the run loop to re-derive it,
    /// because the session acted on is deliberately **not** the one the bar
    /// is presenting: re-reading "the active session" there would send the
    /// interrupt to the wrong process, which is the entire failure this
    /// capability exists to avoid.
    ///
    /// Interrupting is not killing. Nothing here moves a session's
    /// lifecycle: a harness that handles the signal keeps running, and one
    /// that exits because of it is noticed by the ordinary exit detection on
    /// the next tick, from the process rather than inferred here.
    InterruptSession(SessionId),
    /// Send one line to the session the overview's cursor is on — Phase 4's
    /// "send text programmatically without requiring the user to focus it".
    ///
    /// Carries its target for the same reason [`Action::InterruptSession`]
    /// does. The run loop writes it and nothing else: **producing this must
    /// not change which session has focus**, which is the half of the
    /// capability that is easy to lose and the reason the tests assert it
    /// separately from the text arriving.
    SendSessionText {
        id: SessionId,
        text: String,
    },
    /// Reopen the first-run wizard for a "reconfigure" invocation (Phase 2C:
    /// "Allow the onboarding wizard to be reopened later from settings").
    /// Discovery and reading `UserConfig` are file I/O this module
    /// deliberately does not hold, and driving `crate::onboarding::run`
    /// needs the terminal this shell's own [`crate::tui::Screen`] already
    /// holds — both are the run loop's job, exactly like
    /// [`Action::OpenSettings`].
    ReopenOnboarding,
    /// Reopen the overview's cursor session where it left off — Phase 11's
    /// "resume any compatible stopped session from the overview".
    ///
    /// Carries its target for the same reason [`Action::InterruptSession`]
    /// does: the session acted on is the one under the cursor, not whichever
    /// one the bar happens to be presenting. Selecting a harness, building
    /// its resume arguments, and starting the process are all I/O this
    /// module deliberately does not hold — see `shell::resume_session`, the
    /// run loop's counterpart to `shell::start_session`.
    ResumeSession(SessionId),
    /// Open the project overview. Reading current binding memory and
    /// unresolved todos is file I/O this module deliberately does not hold —
    /// the run loop reads them and calls
    /// [`ShellState::open_project_overview`], reporting a read failure back
    /// through `memory_note` rather than `set_status`, because sessions
    /// still display and the overlay still opens either way. Phase 41's
    /// project-level sibling of [`Action::OpenSettings`].
    OpenProjectOverview,
    /// Open the project-knowledge view. Reading project memory is file I/O
    /// this module deliberately does not hold — the run loop reads it and
    /// calls [`ShellState::open_project_knowledge`], reporting a read
    /// failure back through its own `memory_note` rather than refusing to
    /// open, the same contract [`Action::OpenProjectOverview`] already
    /// keeps. Phase 25, map lines 1098-1107.
    OpenProjectKnowledge,
    /// Open the route-evidence table. Reading the routing evidence ledger
    /// (`crate::routing::evidence::EvidenceLedger`) is file I/O this module
    /// deliberately does not hold — the run loop reads it and calls
    /// [`ShellState::open_route_evidence`], reporting a read failure back
    /// through its own note rather than refusing to open, the same contract
    /// [`Action::OpenProjectOverview`] and [`Action::OpenProjectKnowledge`]
    /// already keep. Phase 47, map lines 1762 and 1764.
    OpenRouteEvidence,
    /// Open the project-memory view. Reading project memory is file I/O this
    /// module deliberately does not hold — the run loop reads it and calls
    /// [`ShellState::open_project_memory`], reporting a read failure back
    /// through its own note rather than refusing to open, the same contract
    /// [`Action::OpenProjectKnowledge`] already keeps. Map line 234.
    OpenProjectMemory,
    /// Open the route-health view. Reading the two gateway telemetry caches
    /// (`crate::provider::telemetry::GatewayHealthCache` and
    /// `GatewayQuotaCache`) is file I/O this module deliberately does not
    /// hold — the run loop reads them and calls
    /// [`ShellState::open_route_health`].
    ///
    /// **No error arm, unlike [`Action::OpenRouteEvidence`].** Both caches
    /// are documented as returning no error ever: an absent, unreadable,
    /// truncated or wrong-version file reads as *nothing observed*, which is
    /// a complete answer this view can render honestly. There is therefore
    /// no failure for a note to report, and adding one would be a field
    /// nothing sets. Phase 47, map line 1765.
    OpenRouteHealth,
    /// Open the routing-decisions view. Reading the evaluation ledger
    /// (`crate::evaluation::EvaluationObservations`) is file I/O this module
    /// deliberately does not hold — the run loop reads it and calls
    /// [`ShellState::open_route_decisions`], reporting a read failure back
    /// through its own note rather than refusing to open, the same contract
    /// [`Action::OpenRouteEvidence`] keeps and for the same reason: that
    /// ledger is SQLite and really can fail to open.
    OpenRouteDecisions,
}

/// A session's screen, as a terminal would have drawn it, ready to draw.
///
/// Built once per tick in `shell::build_viewport_grid` from the focused
/// session's `vt100::Screen` and handed here via
/// [`ShellState::set_viewport_grid`] — this module never touches `vt100`
/// itself, the same way it never touches a [`crate::session::SessionRuntime`].
/// `cells` is row-major: the cell at `(row, col)` lives at
/// `row * cols + col`.
///
/// Empty — `rows` or `cols` is zero — when no live session has produced a
/// screen yet, which `super::view::render_viewport` takes as its signal to
/// fall back to the placeholder.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ViewportGrid {
    rows: u16,
    cols: u16,
    cells: Vec<(String, Style)>,
    cursor: Option<(u16, u16)>,
}

impl ViewportGrid {
    pub fn new(
        rows: u16,
        cols: u16,
        cells: Vec<(String, Style)>,
        cursor: Option<(u16, u16)>,
    ) -> Self {
        Self {
            rows,
            cols,
            cells,
            cursor,
        }
    }

    /// True when there is no screen to draw — no live session has produced
    /// one yet.
    pub fn is_empty(&self) -> bool {
        self.rows == 0 || self.cols == 0
    }

    pub fn rows(&self) -> u16 {
        self.rows
    }

    pub fn cols(&self) -> u16 {
        self.cols
    }

    /// The cell at `(row, col)`, or `None` outside the grid — the view clips
    /// to this rather than trusting `rows`/`cols` to match the render area.
    pub fn cell(&self, row: u16, col: u16) -> Option<&(String, Style)> {
        if row >= self.rows || col >= self.cols {
            return None;
        }
        self.cells
            .get(usize::from(row) * usize::from(self.cols) + usize::from(col))
    }

    /// The cursor's `(row, col)`, if the session's screen has one to show.
    pub fn cursor(&self) -> Option<(u16, u16)> {
        self.cursor
    }
}

/// The abbreviated session identifier the overview shows.
///
/// One definition, shared by the overview's rows and by every status note
/// that names a session, so a refusal can be matched by eye to the row it is
/// about. A full identifier is a UUID and would crowd the status row out.
pub(crate) fn short_session_id(id: &SessionId) -> String {
    id.as_str().chars().take(12).collect()
}

/// How many recent events the activity view keeps and shows.
///
/// Bounded, oldest discarded, for the same reason a scrollback is bounded: an
/// activity list that grew without limit would eventually cost more to hold
/// and draw than it is worth to a user who only ever looks at the tail of it.
pub const ACTIVITY_ROWS: usize = 8;

/// One line for the activity view, naming exactly what happened.
///
/// Exhaustive with **no `_` arm**: every [`LifecycleEvent`] variant this
/// module knows about gets its own summary, so a new variant is a compile
/// error here rather than a silently blank row. The two distinctions the
/// event model is careful to preserve — a turn ending `Completed` versus
/// `Failed`, and a [`MessageOrigin`] of `Machine` versus `UserKeystroke` — are
/// preserved here too, on purpose: collapsing either would throw away the one
/// fact Glasshouse keeps that the harness cannot.
pub(crate) fn describe_event(event: &LifecycleEvent) -> String {
    match event {
        LifecycleEvent::SessionStarted => "session started".to_owned(),
        // Not "session started" again. A resume is a different fact and the
        // event model keeps it separate precisely so a reader never has to
        // infer one from a session having started twice.
        LifecycleEvent::SessionResumed => "session resumed".to_owned(),
        LifecycleEvent::TurnStarted => "turn started".to_owned(),
        LifecycleEvent::TurnEnded {
            outcome: TurnOutcome::Completed,
        } => "turn ended (completed)".to_owned(),
        LifecycleEvent::TurnEnded {
            outcome: TurnOutcome::Failed,
        } => "turn ended (failed)".to_owned(),
        // Never "idle" — see the module docs on `LifecycleEvent::WaitingForUser`:
        // silence is never promoted to this, so this must never read like silence.
        LifecycleEvent::WaitingForUser => "waiting for the user".to_owned(),
        LifecycleEvent::TextDelivered {
            origin: MessageOrigin::Machine,
            bytes,
        } => format!("sent {bytes} bytes (machine)"),
        LifecycleEvent::TextDelivered {
            origin: MessageOrigin::UserKeystroke,
            bytes,
        } => format!("sent {bytes} bytes (typed)"),
        LifecycleEvent::InterruptDelivered {
            origin: MessageOrigin::Machine,
        } => "interrupt sent (machine)".to_owned(),
        LifecycleEvent::InterruptDelivered {
            origin: MessageOrigin::UserKeystroke,
        } => "interrupt sent (typed)".to_owned(),
        LifecycleEvent::ProcessExited { exit } => format!("process {exit}"),
        LifecycleEvent::OutputEnded => "output ended".to_owned(),
        LifecycleEvent::GatewayUnhealthy { resource, reason } => {
            format!("{resource} gateway {reason}")
        }
        LifecycleEvent::GatewayBackendChanged {
            provider,
            model,
            cause,
        } => format!("gateway backend changed to {provider}/{model} ({cause})"),
        // Migration 26. The path is already repo-relative — the writer
        // applies `crate::memory::store::normalize_observed_path` before the
        // event exists — so this row names a file the user recognises without
        // spilling their absolute directory layout into a view that is often
        // on screen while someone else is looking.
        LifecycleEvent::FileTouched { path } => format!("edited {path}"),
    }
}

/// Everything the shell displays.
pub struct ShellState {
    project_name: String,
    project_root: PathBuf,
    version: String,
    sessions: Vec<SessionRecord>,
    /// Index into `sessions`. Meaningless when `sessions` is empty, and every
    /// accessor guards for that rather than trusting it.
    selected: usize,
    overlay: Option<Overlay>,
    /// A one-line note for the status bar, cleared by the next keystroke.
    ///
    /// Its job is to explain a key that appeared to do nothing. Without it a
    /// user pressing Tab in a project with one session sees a dead keyboard
    /// and has no way to tell that from a bug.
    status: Option<String>,
    /// Who currently owns the keyboard. See [`Mode`].
    mode: Mode,
    /// The screen shown in the session viewport — the focused session's
    /// `vt100` screen, converted by the run loop and set via
    /// [`ShellState::set_viewport_grid`]. Not the runtime itself: see the
    /// module doc.
    viewport_grid: ViewportGrid,
    /// The Settings overlay's own data, or `None` when it is not open. Kept
    /// separate from `overlay` because it carries real data (rows, pending
    /// edits, sub-mode) that a plain `Copy` marker cannot.
    settings: Option<SettingsState>,
    /// The Overview overlay's own data, or `None` when it is not open — the
    /// same split as `settings`, and for the same reason. See
    /// [`OverviewState`] for why its cursor is not `selected`.
    overview: Option<OverviewState>,
    /// The project overview's own data, or `None` when it is not open — the
    /// same split as `settings` and `overview`.
    project_overview: Option<ProjectOverviewState>,
    /// The project-knowledge view's own data, or `None` when it is not open —
    /// the same split as `project_overview`.
    project_knowledge: Option<ProjectKnowledgeState>,
    /// The route-evidence table's own data, or `None` when it is not open —
    /// the same split as `project_overview` and `project_knowledge`.
    route_evidence: Option<RouteEvidenceState>,
    /// The route-health view's own data, or `None` when it is not open — the
    /// same split as `route_evidence`.
    route_health: Option<RouteHealthState>,
    /// The routing-decisions view's own data, or `None` when it is not open —
    /// the same split as `route_evidence`.
    route_decisions: Option<RouteDecisionsState>,
    /// The project-memory view's own data, or `None` when it is not open —
    /// the same split as `project_overview`, `project_knowledge` and
    /// `route_evidence`.
    project_memory: Option<ProjectMemoryState>,
    /// Recent lifecycle events, newest first, bounded at [`ACTIVITY_ROWS`].
    /// See [`ShellState::note_events`].
    activity: Vec<RecordedEvent>,
}

impl ShellState {
    pub fn new(
        project_name: impl Into<String>,
        project_root: impl Into<PathBuf>,
        version: impl Into<String>,
        sessions: Vec<SessionRecord>,
    ) -> Self {
        Self {
            project_name: project_name.into(),
            project_root: project_root.into(),
            version: version.into(),
            sessions,
            selected: 0,
            overlay: None,
            status: None,
            mode: Mode::Control,
            viewport_grid: ViewportGrid::default(),
            settings: None,
            overview: None,
            project_overview: None,
            project_knowledge: None,
            route_evidence: None,
            route_health: None,
            route_decisions: None,
            project_memory: None,
            activity: Vec::new(),
        }
    }

    pub fn project_name(&self) -> &str {
        &self.project_name
    }

    /// The active canonical project root, which the top bar shows on every
    /// frame. This is the value the whole isolation model is built on, so it
    /// is never abbreviated away entirely — the view truncates from the left
    /// when it must, keeping the tail, which is the part that identifies the
    /// project.
    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn sessions(&self) -> &[SessionRecord] {
        &self.sessions
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    /// The session currently presented, or `None` when the project has none
    /// yet.
    pub fn active_session(&self) -> Option<&SessionRecord> {
        self.sessions.get(self.selected)
    }

    pub fn overlay(&self) -> Option<Overlay> {
        self.overlay
    }

    /// The current status note, if any.
    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    /// Show a note in the status bar until the next keystroke.
    pub fn set_status(&mut self, message: impl Into<String>) {
        self.status = Some(message.into());
    }

    /// Who currently owns the keyboard.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The screen currently shown in the session viewport. Empty until the
    /// run loop has set it, which `super::view::render_viewport` uses to
    /// decide whether to draw it in place of the placeholder.
    pub fn viewport_grid(&self) -> &ViewportGrid {
        &self.viewport_grid
    }

    /// Replace the viewport grid. The run loop calls this with the focused
    /// session's screen, rebuilt from its `vt100::Parser`, whenever it
    /// changes.
    pub fn set_viewport_grid(&mut self, grid: ViewportGrid) {
        self.viewport_grid = grid;
    }

    /// Present the next session, wrapping at the end.
    ///
    /// Wrapping rather than stopping: a session bar is a ring the user tabs
    /// through, and stopping dead at the last entry reads as a broken key.
    pub fn next_session(&mut self) -> Action {
        self.step(1)
    }

    /// Present the previous session, wrapping at the start.
    pub fn previous_session(&mut self) -> Action {
        self.step(-1)
    }

    fn step(&mut self, delta: isize) -> Action {
        if self.sessions.len() < 2 {
            // Nothing to move between. Say so rather than letting the key look
            // broken — but the selection genuinely did not change, so this is
            // the only reason there is to redraw.
            self.set_status(match self.sessions.len() {
                0 => "no sessions yet — start one with `glasshouse launch`",
                _ => "this project has only one session",
            });
            return Action::Redraw;
        }
        let len = self.sessions.len() as isize;
        let next = (self.selected as isize + delta).rem_euclid(len);
        self.selected = next as usize;
        Action::Redraw
    }
}

impl ShellState {
    /// Leave whatever overlay is open and go back to the active session.
    ///
    /// Returns [`Action::None`] when no overlay is open, which is what lets the
    /// caller distinguish "the user dismissed an overlay" from "the user asked
    /// to leave Glasshouse" using the same key.
    pub fn close_overlay(&mut self) -> Action {
        if self.overlay.is_none() {
            return Action::None;
        }
        self.overlay = None;
        self.settings = None;
        self.overview = None;
        self.project_overview = None;
        self.project_knowledge = None;
        self.route_evidence = None;
        self.route_health = None;
        self.route_decisions = None;
        self.project_memory = None;
        Action::Redraw
    }
}

impl ShellState {
    /// Replace the session list, keeping the same session presented if it is
    /// still there.
    ///
    /// Reconciling by identity rather than by index: sessions are ordered by
    /// last activity, so any refresh can reorder them, and holding an index
    /// would silently move the user to a different session.
    pub fn refresh(&mut self, sessions: Vec<SessionRecord>) -> Action {
        let active = self.active_session().map(|record| record.id.clone());
        // The overview's cursor is reconciled the same way and for the same
        // reason: it decides which session an interrupt is sent to, so a
        // reorder that moved it silently would aim a signal at a process the
        // user never pointed at.
        let target = self.overview_target().map(|record| record.id.clone());
        let unchanged = sessions == self.sessions;
        self.sessions = sessions;
        self.selected = active
            .and_then(|id| self.sessions.iter().position(|record| record.id == id))
            .unwrap_or(0);
        if let Some(overview) = self.overview.as_mut() {
            overview.cursor = target
                .and_then(|id| self.sessions.iter().position(|record| record.id == id))
                .unwrap_or(0);
        }
        if unchanged {
            Action::None
        } else {
            Action::Redraw
        }
    }

    /// Take lifecycle events drained from the bus.
    ///
    /// `events` arrives oldest first, matching [`crate::events::Subscription::drain`];
    /// this keeps `activity` newest first by inserting each one at the front
    /// in that order, then discards anything past [`ACTIVITY_ROWS`] — the
    /// oldest events, exactly like a bounded scrollback.
    ///
    /// This is a window, not a writer: it never touches a session's
    /// lifecycle, its order in the table, the cursor, or the status note.
    pub fn note_events(&mut self, events: &[RecordedEvent]) -> Action {
        if events.is_empty() {
            return Action::None;
        }
        for event in events {
            self.activity.insert(0, event.clone());
        }
        self.activity.truncate(ACTIVITY_ROWS);
        Action::Redraw
    }

    /// The most recent events, newest first, at most [`ACTIVITY_ROWS`].
    pub fn activity(&self) -> &[RecordedEvent] {
        &self.activity
    }

    /// Answer one key.
    ///
    /// [`Mode`] is consulted first, before any binding: in [`Mode::Session`]
    /// every key belongs to the focused PTY, and Glasshouse's own bindings
    /// below must never see it.
    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        // A note explains the key that was just pressed, so the next key
        // clears it rather than leaving stale text under a new action.
        let had_status = self.status.take().is_some();

        if self.mode == Mode::Session {
            return self.handle_session_key(key);
        }

        // Settings owns every key while it is open — Tab/Left/Right/Up/Down
        // mean something completely different there than session
        // navigation, unlike the read-only Overview below, whose passive
        // popup lets ordinary navigation keep working underneath it.
        if self.overlay == Some(Overlay::Settings) {
            return self.handle_settings_key(key);
        }

        // The Overview takes the keys it has meanings for — its own cursor,
        // and the two that act on the session under it — and passes
        // everything else through, so ordinary navigation keeps working
        // underneath the popup.
        if self.overlay == Some(Overlay::Overview) {
            return self.handle_overview_key(key, had_status);
        }

        // Read-only, like the Overview above: it owns nothing but its own
        // close key and lets ordinary navigation pass through underneath.
        if self.overlay == Some(Overlay::ProjectOverview) {
            return self.handle_project_overview_key(key, had_status);
        }

        // Read-only, like the two above: nothing to act on, so only its own
        // close key is claimed.
        if self.overlay == Some(Overlay::SessionEvents) {
            return self.handle_session_events_key(key, had_status);
        }

        // Unlike the three read-only overlays above, this one now has a
        // cursor and a drill-down to open — map line 1105 — so it claims
        // Up/Down/Enter the same way the Overview claims its own keys, and
        // passes everything else through underneath.
        if self.overlay == Some(Overlay::ProjectKnowledge) {
            return self.handle_project_knowledge_key(key, had_status);
        }

        // Read-only, like the project overview and session events above:
        // nothing to act on, so only its own close key is claimed.
        if self.overlay == Some(Overlay::RouteEvidence) {
            return self.handle_route_evidence_key(key, had_status);
        }

        // Read-only, exactly like `RouteEvidence` above and for the same
        // reason: it is a table with nothing on it to act on.
        if self.overlay == Some(Overlay::RouteHealth) {
            return self.handle_route_health_key(key, had_status);
        }

        // Read-only for the same reason again: a list of decisions already
        // made has nothing on it to act on.
        if self.overlay == Some(Overlay::RouteDecisions) {
            return self.handle_route_decisions_key(key, had_status);
        }

        // The same cursor-and-drill-down shape as `ProjectKnowledge` above,
        // over one unfiltered list instead of five curated sections.
        if self.overlay == Some(Overlay::ProjectMemory) {
            return self.handle_project_memory_key(key, had_status);
        }

        self.handle_control_key(key, had_status)
    }

    /// Glasshouse's own bindings, with no overlay claiming the key first.
    fn handle_control_key(&mut self, key: KeyEvent, had_status: bool) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('c' | 'C') if ctrl => Action::Quit,
            KeyCode::Char('q') => Action::Quit,
            KeyCode::Esc => Action::Quit,
            KeyCode::Tab | KeyCode::Right => self.next_session(),
            KeyCode::BackTab | KeyCode::Left => self.previous_session(),
            KeyCode::Char('o') => self.open_overview(),
            KeyCode::Char('s') => Action::OpenSettings,
            KeyCode::Char('p') => Action::OpenProjectOverview,
            KeyCode::Char('k') => Action::OpenProjectKnowledge,
            KeyCode::Char('e') => self.open_session_events(),
            KeyCode::Char('r') => Action::OpenRouteEvidence,
            // `h` for health, and it was free: every other `Char` binding in
            // this table is listed above and below, and no overlay handler
            // claims `h` either — the overlay handlers that run instead of
            // this one claim only their own close key (and, for the two with
            // a cursor, Up/Down/Enter).
            KeyCode::Char('h') => Action::OpenRouteHealth,
            // `d` for decisions, and it was free: no binding in this table
            // used it, and the only other `Char('d')` in this file is inside
            // the Settings overlay's own handler, which runs instead of this
            // one and never falls through to it.
            KeyCode::Char('d') => Action::OpenRouteDecisions,
            // Capital, not lowercase `m`: that letter is already the
            // Overview's own "begin sending text" key (`handle_overview_key`'s
            // `Char('m') if !ctrl`), and giving the same key a second,
            // context-dependent meaning would be confusing even though the
            // two never overlap at runtime.
            KeyCode::Char('M') => Action::OpenProjectMemory,
            KeyCode::Enter | KeyCode::Char('i') => self.enter_session_mode(),
            KeyCode::Char('n') => Action::StartSession,
            // Shift-N is the same session `n` starts, minus the viewport —
            // Phase 4's headless presentation mode. Deliberately next to `n`
            // rather than behind a prompt: the only difference between the
            // two is where the session is shown, and a user who wants one
            // wants the other's key.
            KeyCode::Char('N') => Action::StartHeadlessSession,
            // Clearing a note is itself a visible change.
            _ if had_status => Action::Redraw,
            _ => Action::None,
        }
    }
}
