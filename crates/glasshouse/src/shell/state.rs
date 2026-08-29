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

/// The session overview's own data: where its cursor is, and the line being
/// typed at a session, if one is.
///
/// **The cursor is deliberately not the session bar's selection.** The bar's
/// selection is what the viewport presents and what the runtime focuses (see
/// `shell::sync_focus`); the overview's cursor is what the overview *acts
/// on*. Sharing one index would make "send this to a session I am not
/// looking at" impossible to express, which is precisely the capability the
/// overview exists to provide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverviewState {
    /// Index into [`ShellState::sessions`]. Reconciled by identity on every
    /// refresh, exactly like the bar's selection — sessions are ordered by
    /// last activity, so any refresh can reorder them and a held index would
    /// silently move the cursor onto a different session.
    cursor: usize,
    /// The line being typed at the session under the cursor, or `None` when
    /// no field is open. `Some("")` is an open, empty field — a different
    /// state from no field at all, which is why this is not a bare `String`.
    entry: Option<String>,
}

impl OverviewState {
    /// Which row the cursor is on.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The line being typed, or `None` when no field is open.
    pub fn entry(&self) -> Option<&str> {
        self.entry.as_deref()
    }
}

/// The project overview's own data: memory the run loop already read from
/// disk, formatted into one line per entry.
///
/// Sessions are not duplicated here — [`ShellState::sessions`] already holds
/// every session record, and the view groups them by role and lifecycle at
/// render time, the same way `render_overview` derives its columns from
/// [`SessionRecord`] rather than from a copy. Memory is different: reading
/// it is file I/O this module deliberately does not hold, exactly like
/// [`ShellState::open_settings`]'s rows, so the run loop reads it and hands
/// back plain strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectOverviewState {
    /// Current binding memory — decisions and constraints, most recently
    /// updated first. See [`crate::memory::store::MemoryStore::binding`].
    decisions: Vec<String>,
    /// Current, unresolved [`crate::memory::store::MemoryKind::Todo`]
    /// entries, most recently updated first.
    todos: Vec<String>,
    /// How many further open todos exist beyond `todos` — Phase 26's
    /// snapshot budget, not a number invented here.
    todos_omitted: usize,
    /// One already-formatted line per configured resource — capability map
    /// lines 1657, 1658, 1659, 1660 and 1663. Pre-formatted the same way
    /// `decisions` and `todos` are: reading `crate::config` and the on-disk
    /// gateway-quota cache is file I/O this module deliberately does not
    /// hold, so `shell::build_project_overview_capacity` builds the text and
    /// this struct only carries it. Empty means no resource is configured
    /// for this project, not that reading failed silently — see
    /// [`Self::memory_note`] for the one honest-failure case this section
    /// shares with the memory sections.
    resources: Vec<String>,
    /// Capability map line 1661: one already-formatted line naming the
    /// currently selected routing model and its recent latency —
    /// pre-formatted the same way `resources` is: reading `crate::config`
    /// and the routing evidence ledger is file I/O this module deliberately
    /// does not hold, so `shell::build_project_overview_routing` builds the
    /// text and this struct only carries it. Always present, unlike
    /// `resources`, which can legitimately be empty — this line always has
    /// something honest to say, even when that is "not applicable" or
    /// "unknown".
    routing: String,
    /// Set when the run loop could not read project memory at all — a
    /// missing or unreadable database, say. The overlay still opens and
    /// still shows sessions; only the memory sections are empty, and this
    /// explains why rather than leaving them silently blank.
    memory_note: Option<String>,
}

impl ProjectOverviewState {
    pub fn decisions(&self) -> &[String] {
        &self.decisions
    }

    pub fn todos(&self) -> &[String] {
        &self.todos
    }

    pub fn todos_omitted(&self) -> usize {
        self.todos_omitted
    }

    pub fn resources(&self) -> &[String] {
        &self.resources
    }

    pub fn routing(&self) -> &str {
        &self.routing
    }

    pub fn memory_note(&self) -> Option<&str> {
        self.memory_note.as_deref()
    }
}

/// One [`crate::memory::MemoryKind`]'s entries for the
/// project-knowledge view: already-formatted display lines, most recently
/// updated first, plus how many further matching entries exist beyond what
/// is shown. Built by `shell::build_project_knowledge_memory` — this module
/// never queries `crate::memory` itself, the same split
/// [`ProjectOverviewState`] keeps.
///
/// `details` is index-aligned with `lines` — `details[i]` is
/// [`MemoryDetail`] for the memory `lines[i]` summarizes — map line 1105's
/// drill-down. Kept as a parallel `Vec` rather than folding the two into one
/// per-entry type so every existing reader of `lines` (and every fixture
/// that builds one by hand) stays unaffected by a field it does not use.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KnowledgeSection {
    pub lines: Vec<String>,
    pub details: Vec<MemoryDetail>,
    pub omitted: usize,
}

/// One memory's rationale, source session, source commit and lifecycle
/// state — map line 1105: *"allow the user to open a memory item and
/// inspect its rationale, source session, source commit, and lifecycle
/// state."* Built by `shell::knowledge_detail` from
/// [`crate::memory::MemoryRecord`]'s own fields — this module holds plain
/// strings rather than importing `crate::memory`, the same split
/// [`KnowledgeSection`] itself keeps.
///
/// `None` on `rationale`, `source_session` or `source_commit` means the
/// producer never recorded one — never rendered as an empty field, always
/// as an honest "none recorded" note (see `view::render_knowledge_detail`).
/// `lifecycle` is never absent: every memory has a
/// [`crate::memory::MemoryStatus`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryDetail {
    pub rationale: Option<String>,
    pub source_session: Option<String>,
    pub source_commit: Option<String>,
    pub lifecycle: String,
}

/// The project-knowledge view's own data: every kind of durable project
/// memory the run loop already read from disk, grouped by kind and
/// formatted into display lines. Decisions, constraints and features are
/// filtered to current knowledge
/// ([`crate::memory::MemoryStatus::is_current`]); todos to open work
/// ([`crate::memory::MemoryStatus::is_open_work`], which — unlike
/// `is_current` — keeps one under review or in conflict); failed approaches
/// are shown regardless of status, because the historical record of what was
/// tried is the point of that section (map line 1103). See
/// [`ShellState::open_project_knowledge`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectKnowledgeState {
    decisions: KnowledgeSection,
    constraints: KnowledgeSection,
    features: KnowledgeSection,
    failed_attempts: KnowledgeSection,
    todos: KnowledgeSection,
    /// Set when the run loop could not read project memory at all. The
    /// overlay still opens with honest, empty sections rather than refusing
    /// to show anything — the same contract
    /// [`ProjectOverviewState::memory_note`] keeps.
    memory_note: Option<String>,
    /// Index into the entries of [`Self::sections`], concatenated in the
    /// same order the view renders them (decisions, constraints, features,
    /// failed attempts, todos) — map line 1105's selection, the same cursor
    /// idiom [`OverviewState::cursor`] uses. Meaningless when there are no
    /// entries at all; every accessor guards for that rather than trusting
    /// it, the same rule [`ShellState::selected`]'s own doc comment states.
    cursor: usize,
    /// Whether the detail popup for the entry under [`Self::cursor`] is
    /// currently shown. A separate flag rather than folding into `cursor`
    /// (say, a sentinel value) because "which entry" and "am I looking at
    /// its detail" are independent facts — the cursor keeps moving to the
    /// same place if the detail view is closed and reopened.
    detail_open: bool,
}

impl ProjectKnowledgeState {
    pub fn decisions(&self) -> &KnowledgeSection {
        &self.decisions
    }

    pub fn constraints(&self) -> &KnowledgeSection {
        &self.constraints
    }

    pub fn features(&self) -> &KnowledgeSection {
        &self.features
    }

    /// Kept as history regardless of status — map line 1103's dedicated
    /// section.
    pub fn failed_attempts(&self) -> &KnowledgeSection {
        &self.failed_attempts
    }

    pub fn todos(&self) -> &KnowledgeSection {
        &self.todos
    }

    pub fn memory_note(&self) -> Option<&str> {
        self.memory_note.as_deref()
    }

    /// The five sections, in the exact order the view renders them —
    /// [`Self::cursor`] and [`Self::selected`] both walk this order, and
    /// nothing else, so the two can never disagree about which entry is
    /// "the third one".
    fn sections(&self) -> [&KnowledgeSection; 5] {
        [
            &self.decisions,
            &self.constraints,
            &self.features,
            &self.failed_attempts,
            &self.todos,
        ]
    }

    /// How many selectable entries exist across every section, combined.
    pub fn total_entries(&self) -> usize {
        self.sections().iter().map(|s| s.lines.len()).sum()
    }

    /// Which entry the cursor is on, meaningless when [`Self::total_entries`]
    /// is zero.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Whether the detail popup for the entry under the cursor is open.
    pub fn detail_open(&self) -> bool {
        self.detail_open
    }

    /// The entry under the cursor — its display line and its
    /// [`MemoryDetail`] — or `None` when nothing is selectable.
    pub fn selected(&self) -> Option<(&str, &MemoryDetail)> {
        self.sections()
            .into_iter()
            .flat_map(|section| section.lines.iter().zip(section.details.iter()))
            .map(|(line, detail)| (line.as_str(), detail))
            .nth(self.cursor)
    }
}

/// The project-memory view's own data: every [`crate::memory::MemoryKind`]'s
/// records, at every [`crate::memory::MemoryStatus`], unfiltered and
/// ungrouped into one list — map line 234. [`ProjectKnowledgeState`]'s
/// sibling with the filtering removed: this view is "what does this project
/// remember," not "what has this project learned," so nothing here is
/// dropped for being superseded, resolved, or a kind
/// [`ProjectKnowledgeState`] has no section for. See
/// [`ShellState::open_project_memory`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectMemoryState {
    memory: KnowledgeSection,
    /// Set when the run loop could not read project memory at all. The
    /// overlay still opens with an honest, empty section rather than
    /// refusing to show anything — the same contract
    /// [`ProjectKnowledgeState::memory_note`] keeps.
    memory_note: Option<String>,
    /// Index into [`Self::memory`]'s entries — the same cursor idiom
    /// [`ProjectKnowledgeState::cursor`] uses, over one section instead of
    /// five. Meaningless when there are no entries at all; every accessor
    /// guards for that rather than trusting it.
    cursor: usize,
    /// Whether the detail popup for the entry under [`Self::cursor`] is
    /// currently shown — the same independent flag
    /// [`ProjectKnowledgeState::detail_open`] is, for the same reason.
    detail_open: bool,
}

impl ProjectMemoryState {
    /// Every memory record read for this view, most recently updated first.
    pub fn memory(&self) -> &KnowledgeSection {
        &self.memory
    }

    pub fn memory_note(&self) -> Option<&str> {
        self.memory_note.as_deref()
    }

    /// How many selectable entries exist.
    pub fn total_entries(&self) -> usize {
        self.memory.lines.len()
    }

    /// Which entry the cursor is on, meaningless when [`Self::total_entries`]
    /// is zero.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Whether the detail popup for the entry under the cursor is open.
    pub fn detail_open(&self) -> bool {
        self.detail_open
    }

    /// The entry under the cursor — its display line and its
    /// [`MemoryDetail`] — or `None` when nothing is selectable. The same
    /// shape [`ProjectKnowledgeState::selected`] returns, over the one
    /// section this view has instead of five.
    pub fn selected(&self) -> Option<(&str, &MemoryDetail)> {
        self.memory
            .lines
            .iter()
            .zip(self.memory.details.iter())
            .map(|(line, detail)| (line.as_str(), detail))
            .nth(self.cursor)
    }
}

/// One observed routing identity for the route-evidence table — Phase 47,
/// map lines 1762 and 1764. Built by `shell::build_route_evidence_table`
/// from `crate::routing::evidence::EvidenceLedger::observed_identities`'s own
/// [`crate::routing::evidence::ObservedIdentity`] — this module holds plain
/// data rather than importing `crate::routing::evidence`'s own types
/// directly, the same split [`KnowledgeSection`] keeps from `crate::memory`.
///
/// **Deliberately three columns, not line 1762's seven.** TTFC, effective
/// TTFC, TTFT, decode throughput and rounds-per-minute have no producer on
/// this gateway at all — see `crate::routing::evidence`'s own module header
/// — and this type has no fields for them, so there is nothing here a future
/// render could accidentally show as a fabricated zero. `context_state` is
/// already the string [`crate::routing::evidence::ContextState::as_str`]
/// produces (`"warm"`, `"cold"`, or `"unknown"`) — never blank, and never
/// upgraded to look like a measurement it is not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteEvidenceRow {
    pub provider: String,
    pub model: String,
    /// `None` means this identity's rows were recorded with no route.
    pub route: Option<String>,
    pub context_state: String,
    pub sample_count: usize,
    pub window_start_unix: i64,
    pub window_end_unix: i64,
}

/// The route-evidence table's own data: every distinct routing identity the
/// run loop already read from the evidence ledger. See
/// [`ShellState::open_route_evidence`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteEvidenceState {
    rows: Vec<RouteEvidenceRow>,
    /// Set when the run loop could not read the evidence ledger at all. The
    /// overlay still opens with an honest, empty table rather than refusing
    /// to show anything — the same contract
    /// [`ProjectOverviewState::memory_note`] and
    /// [`ProjectKnowledgeState::memory_note`] keep.
    note: Option<String>,
}

impl RouteEvidenceState {
    pub fn rows(&self) -> &[RouteEvidenceRow] {
        &self.rows
    }

    pub fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }
}

/// One observed free resource, with map line 1765's five concepts carried as
/// **five separate groups of fields** — Phase 47.
///
/// Built by `shell::build_route_health_table` from
/// `crate::provider::telemetry::GatewayHealthCache` and `GatewayQuotaCache`,
/// the two files a gateway process writes and any later process reads back.
/// This module holds plain data rather than importing those types directly,
/// the same split [`RouteEvidenceRow`] keeps from `crate::routing::evidence`.
///
/// # Why the fields are grouped and not summarised
///
/// Line 1765 asks for route health, immediate availability, cadence, quota
/// reset and failure-domain evidence *"as separate concepts"*. They are five
/// different questions with five different answers and five different ways of
/// being unknown, and collapsing them is not a simplification — it is a lost
/// distinction:
///
/// - a resource can be **healthy** (no failures) and **unavailable** (its
///   credential was refused);
/// - it can be **available now** and still have a **cadence** that will stop
///   it in one more request;
/// - a **cooldown** Glasshouse imposed and a **quota reset** the provider
///   stated are different clocks owned by different parties, and neither is
///   the other's estimate;
/// - **failure-domain evidence** is about a *pair* of resources and says
///   nothing about either one alone.
///
/// `crate::provider::resources::render_health` currently prints health,
/// availability and cadence as one `status` word on one line. That is the
/// shape this row exists not to reproduce.
///
/// # "unknown" is a real answer, and it is `None`
///
/// Three of the five concepts come from provider-stated headers that most
/// providers do not send. A `None` here is *"no response ever stated this"*,
/// never a zero and never a default — the same contract
/// `crate::provider::telemetry::RateLimitHeaders` keeps field by field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteHealthRow {
    /// The provider these observations belong to — also the *only* signal
    /// this build has for `failure_domain` below.
    pub provider: String,
    /// `crate::routing::CredentialId::label` — two names, never a secret.
    pub credential_label: String,
    pub model: String,

    // --- concept 1: route health -------------------------------------
    /// How many failures in a row this resource has had **since its last
    /// success**. A streak, not a total: any success resets it to zero (see
    /// `crate::routing::free::ResourceHealth::observe`), so the view must
    /// never present it as a count of everything that ever went wrong.
    pub consecutive_failures: u32,
    /// The provider refused the credential itself. A different fact from a
    /// failure streak, and it is kept separate because waiting does not fix
    /// it.
    pub credential_rejected: bool,

    // --- concept 2: immediate availability ---------------------------
    /// `crate::provider::telemetry::GatewayHealthReading::is_available`, as
    /// of the moment the run loop built this row. The producer's own
    /// decision, not a verdict this module re-derives from the two fields
    /// above — which would be a second spelling of the same rule.
    pub available_now: bool,

    // --- concept 3: cadence ------------------------------------------
    /// When Glasshouse's own bounded backoff stops pacing this resource.
    /// `None` means it is not pacing it. Pacing is a scheduling fact, never
    /// a verdict on the resource — `render_health`'s own wording, kept.
    pub cooling_down_until_unix: Option<i64>,
    /// The request ceiling the provider stated, if it stated one.
    pub stated_limit: Option<i64>,
    /// How long the stated ceiling's window is, in seconds, if the provider
    /// said. `stated_limit` per `stated_window_seconds` is the provider's own
    /// cadence; either half alone is not.
    pub stated_window_seconds: Option<i64>,

    // --- concept 4: quota reset --------------------------------------
    /// When the provider said the current window resets, as a unix second.
    /// `None` means no response ever carried a reset field — not "it never
    /// resets", and not "now".
    pub quota_resets_at_unix: Option<i64>,

    // --- concept 5: failure-domain evidence --------------------------
    /// `crate::routing::domain::FailureDomain`'s own vocabulary, and never
    /// `"independent"`: that state is one this build cannot earn, because
    /// nothing here does the temporal correlation it would need. Spelled by
    /// `shell::build_route_health_table` from the enum itself so there is
    /// exactly one spelling of these words in the process.
    pub failure_domain: String,
    /// How many *other* observed resources share this one's provider — the
    /// resources this one is known to fail together with. Zero does not mean
    /// isolated; it means nothing has been observed that shares its domain.
    pub failure_domain_peers: usize,
}

/// The route-health view's own data: every resource a local gateway has
/// observed, as the run loop read it. See [`ShellState::open_route_health`].
///
/// No `note` field, deliberately, unlike [`RouteEvidenceState`] — see
/// [`Action::OpenRouteHealth`] for why there is no read failure to report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteHealthState {
    rows: Vec<RouteHealthRow>,
}

impl RouteHealthState {
    pub fn rows(&self) -> &[RouteHealthRow] {
        &self.rows
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

    /// Open the session overview.
    ///
    /// The cursor starts on whichever session the bar is presenting, so the
    /// overview opens looking at the same place the user already was; moving
    /// it is how they reach a session they are *not* looking at.
    pub fn open_overview(&mut self) -> Action {
        if self.overlay == Some(Overlay::Overview) {
            return Action::None;
        }
        self.overlay = Some(Overlay::Overview);
        self.overview = Some(OverviewState {
            cursor: self.selected,
            entry: None,
        });
        Action::Redraw
    }

    /// The overview's own data, or `None` when it is not open.
    pub fn overview(&self) -> Option<&OverviewState> {
        self.overview.as_ref()
    }

    /// Open the project overview with memory the run loop already read from
    /// disk. Reading `crate::memory` is file I/O this module deliberately
    /// does not hold — see [`ShellState::open_settings`] for the same split.
    ///
    /// Opens even when `memory_note` is `Some`: a project whose memory
    /// database could not be read still has sessions to show, and closing
    /// the whole overlay over one failed section would hide the part that
    /// worked. See `shell::build_project_overview_memory`'s doc comment for
    /// why the two failure paths both reach this.
    pub fn open_project_overview(
        &mut self,
        decisions: Vec<String>,
        todos: Vec<String>,
        todos_omitted: usize,
        resources: Vec<String>,
        routing: String,
        memory_note: Option<String>,
    ) -> Action {
        self.overlay = Some(Overlay::ProjectOverview);
        self.project_overview = Some(ProjectOverviewState {
            decisions,
            todos,
            todos_omitted,
            resources,
            routing,
            memory_note,
        });
        Action::Redraw
    }

    /// The project overview's own data, or `None` when it is not open.
    pub fn project_overview(&self) -> Option<&ProjectOverviewState> {
        self.project_overview.as_ref()
    }

    /// Open the project-knowledge view with memory the run loop already read
    /// from disk, grouped by kind. Reading `crate::memory` is file I/O this
    /// module deliberately does not hold — see [`Self::open_project_overview`]
    /// for the same split.
    ///
    /// Opens even when `memory_note` is `Some`: a project whose memory
    /// database could not be read still gets an honest, empty view rather
    /// than no view at all — see `shell::build_project_knowledge_memory`'s
    /// doc comment for why both failure paths reach this.
    pub fn open_project_knowledge(
        &mut self,
        decisions: KnowledgeSection,
        constraints: KnowledgeSection,
        features: KnowledgeSection,
        failed_attempts: KnowledgeSection,
        todos: KnowledgeSection,
        memory_note: Option<String>,
    ) -> Action {
        self.overlay = Some(Overlay::ProjectKnowledge);
        self.project_knowledge = Some(ProjectKnowledgeState {
            decisions,
            constraints,
            features,
            failed_attempts,
            todos,
            memory_note,
            cursor: 0,
            detail_open: false,
        });
        Action::Redraw
    }

    /// The project-knowledge view's own data, or `None` when it is not open.
    pub fn project_knowledge(&self) -> Option<&ProjectKnowledgeState> {
        self.project_knowledge.as_ref()
    }

    /// Open the route-evidence table with rows the run loop already read
    /// from the evidence ledger. Reading `crate::routing::evidence` is file
    /// I/O this module deliberately does not hold — see
    /// [`Self::open_project_overview`] for the same split.
    ///
    /// Opens even when `note` is `Some`: a project whose evidence ledger
    /// could not be read still gets an honest, empty table rather than no
    /// view at all — see `shell::build_route_evidence_table`'s doc comment
    /// for why both failure paths reach this.
    pub fn open_route_evidence(
        &mut self,
        rows: Vec<RouteEvidenceRow>,
        note: Option<String>,
    ) -> Action {
        self.overlay = Some(Overlay::RouteEvidence);
        self.route_evidence = Some(RouteEvidenceState { rows, note });
        Action::Redraw
    }

    /// The route-evidence table's own data, or `None` when it is not open.
    pub fn route_evidence(&self) -> Option<&RouteEvidenceState> {
        self.route_evidence.as_ref()
    }

    /// Open the route-health view with rows the run loop already read from
    /// the two gateway telemetry caches. Reading
    /// `crate::provider::telemetry` is file I/O this module deliberately does
    /// not hold — see [`Self::open_route_evidence`] for the same split.
    ///
    /// Opens on an empty `rows` too: "no gateway exchange has been observed"
    /// is an honest answer and the one a fresh installation gives, so a view
    /// that refused to open would be hiding the most common true state.
    pub fn open_route_health(&mut self, rows: Vec<RouteHealthRow>) -> Action {
        self.overlay = Some(Overlay::RouteHealth);
        self.route_health = Some(RouteHealthState { rows });
        Action::Redraw
    }

    /// The route-health view's own data, or `None` when it is not open.
    pub fn route_health(&self) -> Option<&RouteHealthState> {
        self.route_health.as_ref()
    }

    /// Open the project-memory view with memory the run loop already read
    /// from disk — every kind, at every status, unfiltered. Reading
    /// `crate::memory` is file I/O this module deliberately does not hold —
    /// see [`Self::open_project_overview`] for the same split. Map line 234.
    ///
    /// Opens even when `memory_note` is `Some`: a project whose memory
    /// database could not be read still gets an honest, empty view rather
    /// than no view at all — the same contract
    /// [`Self::open_project_knowledge`] keeps.
    pub fn open_project_memory(
        &mut self,
        memory: KnowledgeSection,
        memory_note: Option<String>,
    ) -> Action {
        self.overlay = Some(Overlay::ProjectMemory);
        self.project_memory = Some(ProjectMemoryState {
            memory,
            memory_note,
            cursor: 0,
            detail_open: false,
        });
        Action::Redraw
    }

    /// The project-memory view's own data, or `None` when it is not open.
    pub fn project_memory(&self) -> Option<&ProjectMemoryState> {
        self.project_memory.as_ref()
    }

    /// Open the presented session's recent-lifecycle-events overlay — map
    /// line 1758.
    ///
    /// Unlike [`Self::open_project_overview`], nothing here needs the run
    /// loop's file I/O: the events this overlay shows are the same
    /// `activity` buffer [`Self::note_events`] already keeps up to date in
    /// production, whether or not this overlay is ever opened, so there is
    /// no data to hand in — only the marker, exactly like
    /// [`Self::open_overview`] before Phase 11 gave it a cursor to track.
    pub fn open_session_events(&mut self) -> Action {
        if self.overlay == Some(Overlay::SessionEvents) {
            return Action::None;
        }
        self.overlay = Some(Overlay::SessionEvents);
        Action::Redraw
    }

    /// The session the overview's cursor is on — the one an interrupt or a
    /// sent line acts on. `None` when the overview is closed or the project
    /// has no sessions.
    pub fn overview_target(&self) -> Option<&SessionRecord> {
        self.sessions.get(self.overview.as_ref()?.cursor)
    }

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
        self.project_memory = None;
        Action::Redraw
    }

    /// Every row and pending edit currently shown in the Settings overlay, or
    /// `None` when Settings is not open.
    pub fn settings(&self) -> Option<&SettingsState> {
        self.settings.as_ref()
    }

    /// Open the Settings overlay with rows the run loop already built from a
    /// fresh [`crate::integrations::Discovery`] pass and the configuration
    /// currently on disk. This module never runs that discovery or reads a
    /// configuration file itself — see the module documentation.
    pub fn open_settings(
        &mut self,
        harnesses: Vec<HarnessRow>,
        integrations: Vec<IntegrationRow>,
        providers: Vec<ProviderRow>,
        profiles: Vec<ProfileRow>,
    ) -> Action {
        let configured_providers = providers.iter().map(|row| row.name.clone()).collect();
        self.open_settings_with_routing(
            harnesses,
            integrations,
            providers,
            profiles,
            RoutingRow::defaults(configured_providers),
            MemoryRow::defaults(),
        )
    }

    /// Open Settings with the fully resolved routing-policy row and memory
    /// row supplied by the run loop. Kept separate from
    /// [`ShellState::open_settings`] so older in-module callers can construct
    /// unrelated settings fixtures without repeating routing/memory defaults.
    pub fn open_settings_with_routing(
        &mut self,
        harnesses: Vec<HarnessRow>,
        integrations: Vec<IntegrationRow>,
        providers: Vec<ProviderRow>,
        profiles: Vec<ProfileRow>,
        routing: RoutingRow,
        memory: MemoryRow,
    ) -> Action {
        self.overlay = Some(Overlay::Settings);
        self.settings = Some(SettingsState::new(
            harnesses,
            integrations,
            providers,
            profiles,
            routing,
            memory,
        ));
        Action::Redraw
    }

    /// Replace the Settings rows after a successful save, clearing every
    /// pending edit — it is now reflected on disk — while keeping the cursor
    /// in place. A no-op when Settings is not open.
    pub fn refresh_settings(
        &mut self,
        harnesses: Vec<HarnessRow>,
        integrations: Vec<IntegrationRow>,
        providers: Vec<ProviderRow>,
        profiles: Vec<ProfileRow>,
    ) {
        let configured_providers = providers.iter().map(|row| row.name.clone()).collect();
        self.refresh_settings_with_routing(
            harnesses,
            integrations,
            providers,
            profiles,
            RoutingRow::defaults(configured_providers),
            MemoryRow::defaults(),
        );
    }

    /// Refresh Settings with a freshly resolved routing-policy row and memory
    /// row.
    pub fn refresh_settings_with_routing(
        &mut self,
        harnesses: Vec<HarnessRow>,
        integrations: Vec<IntegrationRow>,
        providers: Vec<ProviderRow>,
        profiles: Vec<ProfileRow>,
        routing: RoutingRow,
        memory: MemoryRow,
    ) {
        if let Some(settings) = self.settings.as_mut() {
            settings.replace_rows(
                harnesses,
                integrations,
                providers,
                profiles,
                routing,
                memory,
            );
        }
    }

    /// Record the most recent disposable-job routing choice, so the Routing
    /// section can show why the free resource currently in use was chosen —
    /// Phase 9I line 540.
    ///
    /// **This batch wires the display, not the feed.** Nothing in this
    /// build calls this from a live router — there is no live router yet.
    /// Feeding it from `crate::routing::disposable::DisposableRouting`'s
    /// actual decisions, each time Glasshouse routes a disposable job, is
    /// `lead-route`'s to wire once that production call site exists; see
    /// this batch's report.
    ///
    /// A no-op when Settings is not open, matching every other
    /// `*_with_routing` setter here — there is nowhere to hold the choice
    /// otherwise, and the next [`ShellState::open_settings_with_routing`]
    /// resolves a fresh [`RoutingRow`] anyway.
    pub fn record_disposable_choice(&mut self, choice: DisposableChoice) {
        if let Some(settings) = self.settings.as_mut() {
            settings.record_disposable_choice(choice);
        }
    }

    /// The provider a credential was just typed for, and the typed value —
    /// **taken**, so the overlay no longer holds it once this returns.
    ///
    /// This is the only route by which a credential leaves the Settings
    /// overlay, and the run loop's only use for it is to hand it straight to
    /// [`crate::secret::native::NativeSecretStore::store`] and drop it. It
    /// returns a bare `String` rather than a [`crate::secret::Secret`]
    /// because a `Secret` is what comes *out* of a store — see that type's
    /// own documentation on why its constructor stays private.
    ///
    /// `None` when no credential field is open, so a stray
    /// [`Action::StoreProviderCredential`] does nothing rather than
    /// consuming some other field's text.
    pub fn take_provider_credential_entry(&mut self) -> Option<(String, String)> {
        self.settings.as_mut()?.take_credential_entry()
    }

    /// The provider probe the overlay just planned — **taken**, so it can
    /// only ever be made once.
    ///
    /// The mirror of [`ShellState::take_provider_credential_entry`], and for
    /// the same reason: this module works out what to do and the run loop
    /// owns everything that touches the world. `None` when nothing is
    /// planned, so a stray [`Action::RunProviderProbe`] opens no socket.
    pub fn take_provider_probe_intent(&mut self) -> Option<ProviderProbeIntent> {
        self.settings.as_mut()?.take_probe_intent()
    }

    /// Hand a finished probe back to the overlay.
    ///
    /// Returns [`Action::Redraw`] when Settings is open — the banner and the
    /// row both changed — and [`Action::None`] when it is not, so a result
    /// arriving after the user closed Settings costs no frame.
    pub fn apply_provider_probe_result(&mut self, result: ProviderProbeResult) -> Action {
        match self.settings.as_mut() {
            Some(settings) => {
                settings.apply_probe_result(result);
                Action::Redraw
            }
            None => Action::None,
        }
    }

    /// Whether any provider request is on the wire right now.
    ///
    /// The run loop asks each tick, so an interface with a request
    /// outstanding keeps repainting and keeps saying so. Without this the
    /// in-flight line would be drawn once and then sit there looking exactly
    /// like a hang.
    pub fn provider_probe_in_flight(&self) -> bool {
        self.settings
            .as_ref()
            .is_some_and(SettingsState::any_probe_in_flight)
    }

    /// The credential variable name `provider` declares first, which is the
    /// name a newly stored credential is filed under.
    ///
    /// The **first** rather than a chosen one: a provider declaring several
    /// is a pool, and choosing between them on cost or quota is a routing
    /// decision this overlay does not make — the same rule
    /// [`crate::provider::Provider::secret_refs`] and
    /// `crate::profile::apply_direct_provider` already follow. The status
    /// line names the variable actually used, so nothing is chosen silently.
    pub fn provider_credential_variable(&self, provider: &str) -> Option<String> {
        self.settings
            .as_ref()?
            .providers()
            .iter()
            .find(|row| row.name == provider)?
            .config
            .credential_env()
            .first()
            .cloned()
    }

    /// The selected provider and every reference its credential could be
    /// stored under, for the run loop to delete — see
    /// `SettingsState::selected_provider_stored_credentials`.
    pub fn selected_provider_stored_credentials(&self) -> Option<(String, Vec<SecretRef>)> {
        self.settings
            .as_ref()?
            .selected_provider_stored_credentials()
    }

    /// Record a successful store: the row shows it, and the configuration
    /// change is staged like every other provider edit, to be written by the
    /// next `w`/`W`.
    pub fn record_provider_credential_stored(
        &mut self,
        provider: &str,
        stored: StoredCredentialRef,
    ) {
        if let Some(settings) = self.settings.as_mut() {
            settings.record_credential_stored(provider, stored);
        }
    }

    /// Record a successful deletion — the configuration half of line 3.
    pub fn record_provider_credential_cleared(&mut self, provider: &str) {
        if let Some(settings) = self.settings.as_mut() {
            settings.record_credential_cleared(provider);
        }
    }

    /// Every pending, unsaved harness Settings edit, for the run loop to
    /// apply to whichever configuration layer is being saved. Empty when
    /// Settings is not open or nothing has been edited yet.
    pub fn settings_edits(&self) -> Vec<SettingsEdit> {
        self.settings
            .as_ref()
            .map(SettingsState::edits)
            .unwrap_or_default()
    }

    /// Every pending, unsaved provider Settings edit — see
    /// [`ShellState::settings_edits`]'s own doc.
    pub fn settings_provider_edits(&self) -> Vec<ProviderSettingsEdit> {
        self.settings
            .as_ref()
            .map(SettingsState::provider_edits)
            .unwrap_or_default()
    }

    /// Every pending, unsaved launch-profile Settings edit — see
    /// [`ShellState::settings_edits`]'s own doc.
    pub fn settings_profile_edits(&self) -> Vec<ProfileSettingsEdit> {
        self.settings
            .as_ref()
            .map(SettingsState::profile_edits)
            .unwrap_or_default()
    }

    /// The independently staged routing fields, if this Settings session
    /// changed at least one of them.
    pub fn settings_routing_edit(&self) -> Option<RoutingSettingsEdit> {
        self.settings.as_ref()?.routing_edit()
    }

    /// The independently staged Memory field, if this Settings session
    /// changed it.
    pub fn settings_memory_edit(&self) -> Option<MemorySettingsEdit> {
        self.settings.as_ref()?.memory_edit()
    }

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

    /// Answer one key while the Settings overlay is open. Everything is
    /// handled here rather than falling through to the bindings above: Tab,
    /// the arrows, and Enter all mean something different inside Settings.
    fn handle_settings_key(&mut self, key: KeyEvent) -> Action {
        let Some(settings) = self.settings.as_mut() else {
            // Defensive: the overlay marker outlived its data somehow. Leave
            // rather than answering keys with nothing behind them.
            self.overlay = None;
            return Action::Redraw;
        };
        match settings.handle_key(key) {
            SettingsAction::None => Action::None,
            SettingsAction::Redraw => Action::Redraw,
            SettingsAction::Close => self.close_overlay(),
            SettingsAction::SaveUser => Action::SaveUserSettings,
            SettingsAction::SaveProject => Action::SaveProjectSettings,
            SettingsAction::ReopenOnboarding => Action::ReopenOnboarding,
            SettingsAction::StoreCredential => Action::StoreProviderCredential,
            SettingsAction::DeleteCredential => Action::DeleteProviderCredential,
            SettingsAction::RunProviderProbe => Action::RunProviderProbe,
        }
    }

    /// Answer one key while the session overview is open.
    ///
    /// Unlike Settings, which owns every key, the Overview claims only the
    /// keys it has a meaning for and passes the rest down: the popup is drawn
    /// over a live shell, and Tab still moving between sessions underneath it
    /// is a property worth keeping.
    fn handle_overview_key(&mut self, key: KeyEvent, had_status: bool) -> Action {
        // While a line is being typed every key belongs to it — the letters
        // of a message must not also fire Glasshouse bindings, or typing
        // "not now" would quit.
        if self.overview.as_ref().is_some_and(|o| o.entry.is_some()) {
            return self.handle_overview_entry_key(key);
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc | KeyCode::Char('o') => self.close_overlay(),
            KeyCode::Up => self.move_overview_cursor(-1),
            KeyCode::Down => self.move_overview_cursor(1),
            // Not `Ctrl-C`, which still quits: this is the overview's own
            // key for "interrupt the session on this row", and it acts on the
            // cursor, never on the session in the viewport.
            KeyCode::Char('c') if !ctrl => self.interrupt_overview_target(),
            KeyCode::Char('m') if !ctrl => self.begin_overview_send(),
            // Phase 11 line 687: bring the cursor's session into the
            // viewport. Bound to Enter specifically because it was unclaimed
            // here — `handle_overview_entry_key` above claims Enter too, but
            // only while the send field is open, which the guard at the top
            // of this function already routes around before this match is
            // ever reached.
            KeyCode::Enter => self.focus_overview_target(),
            // Phase 11 line 688: reopen the cursor's session where it left
            // off. Not `c`/`m`'s neighbour by coincidence — `r` for resume,
            // unclaimed here and unclaimed by Settings' own `r` binding,
            // which belongs to a different overlay entirely.
            KeyCode::Char('r') if !ctrl => self.resume_overview_target(),
            _ => self.handle_control_key(key, had_status),
        }
    }

    /// Answer one key while the project overview is open.
    ///
    /// Unlike the session [`Overlay::Overview`], this popup has no cursor and
    /// nothing to act on — the map's boxes ask it to *show* things, never to
    /// act on them from here — so every key but its own close key passes
    /// through to ordinary navigation underneath, exactly like the Overview
    /// does for the keys it does not claim.
    fn handle_project_overview_key(&mut self, key: KeyEvent, had_status: bool) -> Action {
        match key.code {
            KeyCode::Esc | KeyCode::Char('p') => self.close_overlay(),
            _ => self.handle_control_key(key, had_status),
        }
    }

    /// Answer one key while the session-events overlay is open — the same
    /// shape as [`Self::handle_project_overview_key`], for the same reason:
    /// nothing here is acted on, only shown.
    fn handle_session_events_key(&mut self, key: KeyEvent, had_status: bool) -> Action {
        match key.code {
            KeyCode::Esc | KeyCode::Char('e') => self.close_overlay(),
            _ => self.handle_control_key(key, had_status),
        }
    }

    /// Answer one key while the route-evidence table is open — the same
    /// shape as [`Self::handle_project_overview_key`] and
    /// [`Self::handle_session_events_key`], for the same reason: nothing
    /// here is acted on, only shown.
    fn handle_route_evidence_key(&mut self, key: KeyEvent, had_status: bool) -> Action {
        match key.code {
            KeyCode::Esc | KeyCode::Char('r') => self.close_overlay(),
            _ => self.handle_control_key(key, had_status),
        }
    }

    /// Answer one key while the route-health view is open — the same shape as
    /// [`Self::handle_route_evidence_key`], for the same reason: nothing here
    /// is acted on, only shown.
    fn handle_route_health_key(&mut self, key: KeyEvent, had_status: bool) -> Action {
        match key.code {
            KeyCode::Esc | KeyCode::Char('h') => self.close_overlay(),
            _ => self.handle_control_key(key, had_status),
        }
    }

    /// Answer one key while the project-knowledge view is open.
    ///
    /// Unlike [`Self::handle_project_overview_key`], this overlay now has a
    /// cursor and something to act on — map line 1105's drill-down — so it
    /// claims Up/Down and Enter the same way [`Self::handle_overview_key`]
    /// does, and passes everything else through unchanged. While the detail
    /// popup is open every key but its own close key is swallowed rather
    /// than passed through: a live shell moving underneath a popup that is
    /// itself showing detail on a *specific* entry would let the cursor
    /// wander before the detail closes, silently showing the wrong memory's
    /// detail next time.
    fn handle_project_knowledge_key(&mut self, key: KeyEvent, had_status: bool) -> Action {
        if self
            .project_knowledge
            .as_ref()
            .is_some_and(ProjectKnowledgeState::detail_open)
        {
            return match key.code {
                KeyCode::Esc => self.close_knowledge_detail(),
                _ => Action::None,
            };
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('k') => self.close_overlay(),
            KeyCode::Up => self.move_knowledge_cursor(-1),
            KeyCode::Down => self.move_knowledge_cursor(1),
            KeyCode::Enter => self.open_knowledge_detail(),
            _ => self.handle_control_key(key, had_status),
        }
    }

    /// Move the project-knowledge cursor, wrapping — the same ring
    /// [`Self::move_overview_cursor`] is, for the same reason.
    fn move_knowledge_cursor(&mut self, delta: isize) -> Action {
        let total = self
            .project_knowledge
            .as_ref()
            .map(ProjectKnowledgeState::total_entries)
            .unwrap_or(0);
        if total == 0 {
            self.set_status("nothing to select in the project-knowledge view");
            return Action::Redraw;
        }
        if let Some(knowledge) = self.project_knowledge.as_mut() {
            knowledge.cursor =
                (knowledge.cursor as isize + delta).rem_euclid(total as isize) as usize;
        }
        Action::Redraw
    }

    /// Open the detail popup for the entry under the cursor — map line
    /// 1105. A project with nothing recorded yet has nothing to select, so
    /// this refuses rather than opening a detail popup with nothing in it.
    fn open_knowledge_detail(&mut self) -> Action {
        let has_selection = self
            .project_knowledge
            .as_ref()
            .is_some_and(|knowledge| knowledge.total_entries() > 0);
        if !has_selection {
            self.set_status("nothing selected to inspect");
            return Action::Redraw;
        }
        if let Some(knowledge) = self.project_knowledge.as_mut() {
            knowledge.detail_open = true;
        }
        Action::Redraw
    }

    /// Close the detail popup, returning to the entry list — the cursor is
    /// left exactly where it was, so reopening the same key shows the same
    /// memory.
    fn close_knowledge_detail(&mut self) -> Action {
        if let Some(knowledge) = self.project_knowledge.as_mut() {
            knowledge.detail_open = false;
        }
        Action::Redraw
    }

    /// Answer one key while the project-memory view is open — the same
    /// shape as [`Self::handle_project_knowledge_key`], over one unfiltered
    /// list instead of five curated sections.
    fn handle_project_memory_key(&mut self, key: KeyEvent, had_status: bool) -> Action {
        if self
            .project_memory
            .as_ref()
            .is_some_and(ProjectMemoryState::detail_open)
        {
            return match key.code {
                KeyCode::Esc => self.close_memory_detail(),
                _ => Action::None,
            };
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('M') => self.close_overlay(),
            KeyCode::Up => self.move_memory_cursor(-1),
            KeyCode::Down => self.move_memory_cursor(1),
            KeyCode::Enter => self.open_memory_detail(),
            _ => self.handle_control_key(key, had_status),
        }
    }

    /// Move the project-memory cursor, wrapping — the same ring
    /// [`Self::move_knowledge_cursor`] is, for the same reason.
    fn move_memory_cursor(&mut self, delta: isize) -> Action {
        let total = self
            .project_memory
            .as_ref()
            .map(ProjectMemoryState::total_entries)
            .unwrap_or(0);
        if total == 0 {
            self.set_status("nothing to select in the project-memory view");
            return Action::Redraw;
        }
        if let Some(memory) = self.project_memory.as_mut() {
            memory.cursor = (memory.cursor as isize + delta).rem_euclid(total as isize) as usize;
        }
        Action::Redraw
    }

    /// Open the detail popup for the entry under the cursor. A project with
    /// nothing recorded yet has nothing to select, so this refuses rather
    /// than opening a detail popup with nothing in it — the same rule
    /// [`Self::open_knowledge_detail`] follows.
    fn open_memory_detail(&mut self) -> Action {
        let has_selection = self
            .project_memory
            .as_ref()
            .is_some_and(|memory| memory.total_entries() > 0);
        if !has_selection {
            self.set_status("nothing selected to inspect");
            return Action::Redraw;
        }
        if let Some(memory) = self.project_memory.as_mut() {
            memory.detail_open = true;
        }
        Action::Redraw
    }

    /// Close the detail popup, returning to the entry list — the cursor is
    /// left exactly where it was, the same rule
    /// [`Self::close_knowledge_detail`] follows.
    fn close_memory_detail(&mut self) -> Action {
        if let Some(memory) = self.project_memory.as_mut() {
            memory.detail_open = false;
        }
        Action::Redraw
    }

    /// Move the overview's cursor, wrapping — the same ring the session bar
    /// is, for the same reason: stopping dead at the last row reads as a
    /// broken key.
    fn move_overview_cursor(&mut self, delta: isize) -> Action {
        if self.sessions.is_empty() {
            self.set_status("this project has no sessions to move between");
            return Action::Redraw;
        }
        let len = self.sessions.len() as isize;
        if let Some(overview) = self.overview.as_mut() {
            overview.cursor = (overview.cursor as isize + delta).rem_euclid(len) as usize;
        }
        Action::Redraw
    }

    /// The session under the cursor, if something may be sent to it — and a
    /// spoken refusal naming the session and the state it is actually in if
    /// not.
    ///
    /// **Never a silent no-op.** A key that quietly does nothing is
    /// indistinguishable from a frozen screen, and a user who has just asked
    /// a background session to stop needs to know whether it was asked. The
    /// same rule the provider probes are already held to.
    ///
    /// The state is read from the session record rather than from a process,
    /// because this module holds no processes — the run loop reports the
    /// runtime's own refusal on top of this one, for the narrower case of a
    /// session whose record still says live because its exit has not been
    /// polled yet.
    fn actionable_overview_target(&mut self, verb: &str) -> Option<SessionId> {
        let target = self
            .overview_target()
            .map(|record| (record.id.clone(), record.lifecycle));
        match target {
            None => {
                self.set_status(format!("nothing to {verb}: this project has no sessions"));
                None
            }
            Some((id, lifecycle)) if !lifecycle.is_live() => {
                self.set_status(format!(
                    "cannot {verb} session `{}`: it is {lifecycle}, not running",
                    short_session_id(&id)
                ));
                None
            }
            Some((id, _)) => Some(id),
        }
    }

    /// Interrupt the session under the cursor.
    ///
    /// Nothing about the shell changes: not the presented session, not focus,
    /// not the session's recorded state. An interrupt is a byte delivered to
    /// a terminal, and what the harness does about it is the harness's
    /// business — a session that handles it is still running afterwards, and
    /// one that exits is noticed by the ordinary exit detection.
    fn interrupt_overview_target(&mut self) -> Action {
        match self.actionable_overview_target("interrupt") {
            Some(id) => Action::InterruptSession(id),
            None => Action::Redraw,
        }
    }

    /// Open the one-line field for sending text to the session under the
    /// cursor, refusing up front if that session is not running.
    fn begin_overview_send(&mut self) -> Action {
        if self.actionable_overview_target("send text to").is_none() {
            return Action::Redraw;
        }
        if let Some(overview) = self.overview.as_mut() {
            overview.entry = Some(String::new());
        }
        Action::Redraw
    }

    /// Bring the session under the cursor into the viewport and hand it the
    /// keyboard — Phase 11 line 687: "focus any live embedded session from
    /// the overview".
    ///
    /// Deliberately not built on `actionable_overview_target` alone. That
    /// helper's liveness refusal is exactly right here too — a stopped
    /// session has no process to give the keyboard to — but the box names a
    /// second adjective the helper knows nothing about: *embedded*. A
    /// headless session is live and still has no viewport to focus into, so
    /// it needs its own refusal on top, spoken rather than silent for the
    /// same reason every other overview key is.
    fn focus_overview_target(&mut self) -> Action {
        let Some(id) = self.actionable_overview_target("focus") else {
            return Action::Redraw;
        };
        // Re-read after the liveness check rather than trusting the id
        // alone: `actionable_overview_target` looked this session up by the
        // cursor's position, and re-finding it by identity here is what
        // keeps this correct if that ever changes.
        let Some(index) = self.sessions.iter().position(|record| record.id == id) else {
            return Action::Redraw;
        };
        if self.sessions[index].presentation != SessionPresentation::Embedded {
            self.set_status(format!(
                "cannot focus session `{}`: it is {}, not embedded — there is no viewport to focus into",
                short_session_id(&id),
                self.sessions[index].presentation,
            ));
            return Action::Redraw;
        }
        self.selected = index;
        self.overlay = None;
        self.overview = None;
        self.mode = Mode::Session;
        Action::Redraw
    }

    /// The session under the cursor, if it may be resumed — and a spoken
    /// refusal naming the session and its actual state if not. The resume
    /// half of Phase 11 line 688.
    ///
    /// Deliberately not `actionable_overview_target`: that helper refuses
    /// every session whose lifecycle is not live, which is backwards for
    /// resume — a live session has nothing to resume *to*, and the whole
    /// point of this key is the session that is *not* running. Gated on
    /// [`SessionRecord::disposition`] instead, so the session this key acts
    /// on is exactly the one the STATE column already labels `resumable`.
    fn resumable_overview_target(&mut self) -> Option<SessionId> {
        let target = self
            .overview_target()
            .map(|record| (record.id.clone(), record.disposition()));
        match target {
            None => {
                self.set_status("nothing to resume: this project has no sessions");
                None
            }
            Some((id, SessionDisposition::Resumable)) => Some(id),
            Some((id, SessionDisposition::Active)) => {
                self.set_status(format!(
                    "cannot resume session `{}`: it is still running",
                    short_session_id(&id)
                ));
                None
            }
            Some((id, SessionDisposition::Failed)) => {
                self.set_status(format!(
                    "cannot resume session `{}`: it failed, with no session to reopen",
                    short_session_id(&id)
                ));
                None
            }
            Some((id, SessionDisposition::Closed)) => {
                self.set_status(format!(
                    "cannot resume session `{}`: it is closed, with no native session id \
                     recorded to resume to",
                    short_session_id(&id)
                ));
                None
            }
        }
    }

    /// Resume the session under the cursor.
    fn resume_overview_target(&mut self) -> Action {
        match self.resumable_overview_target() {
            Some(id) => Action::ResumeSession(id),
            None => Action::Redraw,
        }
    }

    /// Answer one key while the send field is open.
    fn handle_overview_entry_key(&mut self, key: KeyEvent) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => {
                if let Some(overview) = self.overview.as_mut() {
                    overview.entry = None;
                }
                Action::Redraw
            }
            KeyCode::Enter => self.submit_overview_send(),
            KeyCode::Backspace => {
                if let Some(entry) = self.overview.as_mut().and_then(|o| o.entry.as_mut()) {
                    entry.pop();
                }
                Action::Redraw
            }
            KeyCode::Char(c) if !ctrl => {
                if let Some(entry) = self.overview.as_mut().and_then(|o| o.entry.as_mut()) {
                    entry.push(c);
                }
                Action::Redraw
            }
            _ => Action::None,
        }
    }

    /// Send what has been typed to the session under the cursor.
    ///
    /// The field is closed on every path out — a refused line and an empty
    /// one both leave the overview where it was, rather than trapping the
    /// user in a field that will not accept anything.
    fn submit_overview_send(&mut self) -> Action {
        let text = self
            .overview
            .as_ref()
            .and_then(|overview| overview.entry.clone())
            .unwrap_or_default();
        if let Some(overview) = self.overview.as_mut() {
            overview.entry = None;
        }
        if text.is_empty() {
            self.set_status("nothing to send: the line was empty");
            return Action::Redraw;
        }
        // Checked again here, not only when the field opened: a session can
        // end while a line is being typed at it, and sending into a dead
        // session must be refused out loud at the moment of sending.
        match self.actionable_overview_target("send text to") {
            Some(id) => Action::SendSessionText { id, text },
            None => Action::Redraw,
        }
    }

    /// Enter session mode, giving the focused session's PTY the keyboard.
    ///
    /// Refused with nothing active: there would be nowhere to send the keys,
    /// and a mode with no destination is worse than staying put — see
    /// invariant 5 in the design note. Any open overlay is closed first:
    /// session mode and an overlay must never coexist, which is what keeps
    /// "leaving an overlay returns to the mode you were in" simple, since
    /// that mode is by construction always control.
    ///
    /// Refused for a **headless** session for a sharper version of the same
    /// reason. A headless session has no viewport, so the runtime refuses to
    /// focus it (`RuntimeError::Headless`) and keystrokes would go to
    /// whichever session held focus before — the user would be typing into a
    /// session the bar is not showing. Saying no is the only honest answer.
    fn enter_session_mode(&mut self) -> Action {
        let Some(record) = self.active_session() else {
            self.set_status("no session to enter — start one with `n`");
            return Action::Redraw;
        };
        if record.presentation == SessionPresentation::Headless {
            let id = record.id.clone();
            // Short on purpose: a status note shares its row with the key
            // bindings, which are written first, so a long refusal is a
            // clipped one. The viewport itself carries the full explanation
            // on every frame — see `view::render_viewport`.
            self.set_status(format!(
                "`{}` is headless — no viewport to enter",
                short_session_id(&id)
            ));
            return Action::Redraw;
        }
        self.overlay = None;
        self.mode = Mode::Session;
        Action::Redraw
    }

    /// Answer one key while a session owns the keyboard.
    ///
    /// Everything is forwarded to the focused PTY untouched — including `q`,
    /// Tab, and Ctrl-C — except the one reserved escape chord, which is
    /// intercepted here and never forwarded.
    fn handle_session_key(&mut self, key: KeyEvent) -> Action {
        if is_session_escape(&key) {
            self.mode = Mode::Control;
            return Action::Redraw;
        }
        match encode(key) {
            Some(bytes) => Action::Forward(bytes),
            None => Action::None,
        }
    }

    /// Called when the session currently presented has exited.
    ///
    /// Session mode with nowhere left to send keystrokes would leave every
    /// keypress going nowhere with no visible way out, so an exit always
    /// drops back to control mode — see invariant 6 in the design note.
    pub fn session_exited(&mut self) -> Action {
        if self.mode == Mode::Session {
            self.mode = Mode::Control;
            Action::Redraw
        } else {
            Action::None
        }
    }
}

// -----------------------------------------------------------------------
// Settings — see `docs/product/design-decisions.md`'s "Settings" section for
// the invariants this data model exists to hold to.
// -----------------------------------------------------------------------

/// Which section of the Settings overlay has the cursor.
///
/// Harnesses and Integrations shipped first. Providers and Launch Profiles
/// followed once their configuration existed. Phase 2D adds Routing now that
/// its policy fields are real, plus an explicitly transparent, read-only
/// Memory section: memory itself is not in this build, so that tab offers no
/// inert controls or speculative configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsSection {
    Harnesses,
    Integrations,
    Providers,
    LaunchProfiles,
    Routing,
    Memory,
}

impl SettingsSection {
    /// Tab order. `next`/`previous` cycle through this, so adding a section
    /// only ever means inserting it here.
    const ORDER: [SettingsSection; 6] = [
        SettingsSection::Harnesses,
        SettingsSection::Integrations,
        SettingsSection::Providers,
        SettingsSection::LaunchProfiles,
        SettingsSection::Routing,
        SettingsSection::Memory,
    ];

    fn index(self) -> usize {
        Self::ORDER
            .iter()
            .position(|&section| section == self)
            .expect("every variant appears in ORDER")
    }

    fn next(self) -> Self {
        Self::ORDER[(self.index() + 1) % Self::ORDER.len()]
    }

    fn previous(self) -> Self {
        Self::ORDER[(self.index() + Self::ORDER.len() - 1) % Self::ORDER.len()]
    }
}

/// One row of the Settings "Harnesses" section.
///
/// `enabled`/`executable` are the live, possibly-edited values shown and
/// acted on; `enabled_layer`/`executable_layer` name which configuration
/// layer supplied them, per the design decision's "provenance is shown, not
/// inferred". Editing a row updates both the value and its layer to
/// [`Layer::User`] immediately, since that is where an edit lands once saved
/// with the default `w` — see [`SettingsState`]'s documentation for why
/// nothing here waits for the actual write to relabel itself.
///
/// Deliberately holds nothing that could be a secret: a boolean, a
/// filesystem path, and a [`Layer`] tag are everything
/// [`crate::config::IntegrationConfig`] itself is able to store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessRow {
    pub id: IntegrationId,
    /// Whether `Discovery` found a usable executable for this harness.
    pub detected: bool,
    pub enabled: bool,
    pub enabled_layer: Layer,
    /// An explicit executable override, if any layer has recorded one. Not
    /// the auto-discovered `PATH` resolution — only a value some
    /// configuration layer actually supplied has a layer to show alongside
    /// it (see [`crate::config::EffectiveConfig::executable`]'s own doc for
    /// why there is no "default" case for this field).
    pub executable: Option<PathBuf>,
    pub executable_layer: Option<Layer>,
}

/// One row of the read-only Settings "Integrations" section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrationRow {
    pub id: IntegrationId,
    pub detected: bool,
    pub status: IntegrationStatus,
}

/// One row of the Settings "Providers" section: a provider configured on
/// either layer. Unlike [`HarnessRow`], there is no implied entry for a
/// built-in template with nothing configured — see
/// [`crate::config::EffectiveConfig::provider_names`]'s own doc for why.
///
/// Holds the whole [`ProviderConfig`] rather than duplicating its fields:
/// every field that type can hold is already guaranteed non-secret (see its
/// module documentation's "No secrets here"), so embedding it here adds no
/// new surface for a credential to leak through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRow {
    pub name: String,
    pub config: ProviderConfig,
    /// Which layer this whole entry came from. A provider is atomic — one
    /// name resolves to exactly one layer's definition, project winning over
    /// user, matching [`crate::config::EffectiveConfig::configured_provider`]
    /// — so one tag covers every field, unlike [`HarnessRow`], where
    /// `enabled` and `executable` can come from different layers.
    pub layer: Layer,
    /// This provider's cached model catalogue, read from disk when Settings
    /// opened, or `None` if it has never been fetched.
    ///
    /// **Read from the cache, never fetched here.** Opening Settings must not
    /// make a network request — that is Phase 9D line 3 — so this is
    /// whatever `provider::cache::ModelCache::load` had on disk and nothing
    /// else. It carries its own timestamp, which the renderer shows.
    pub models: Option<ModelCatalogue>,
    /// A probe currently on the wire for this provider, if any.
    ///
    /// On the row rather than in the bottom-panel banner deliberately. The
    /// banner is cleared by the next keystroke — that is what stops a stale
    /// result shadowing a field editor — and an in-flight indicator that
    /// vanished the moment the user pressed an arrow key would leave a
    /// running request invisible. A frozen interface and a busy one look
    /// identical unless the busy one says so.
    pub activity: Option<ProbeKind>,
}

impl ProviderRow {
    /// A row with no cached catalogue and nothing in flight.
    pub fn new(name: impl Into<String>, config: ProviderConfig, layer: Layer) -> Self {
        Self {
            name: name.into(),
            config,
            layer,
            models: None,
            activity: None,
        }
    }

    /// The same row, carrying whatever the cache had for it.
    pub fn with_models(mut self, models: Option<ModelCatalogue>) -> Self {
        self.models = models;
        self
    }
}

/// One row of the Settings "Launch Profiles" section, matching
/// [`ProviderRow`]'s shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileRow {
    pub name: String,
    pub config: ProfileConfig,
    pub layer: Layer,
}

/// The effective Routing section and the provenance of each independently
/// layered field. Configured-provider names are retained only to validate a
/// pinned `provider:model` choice before it is staged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingRow {
    pub model: RoutingModelChoice,
    pub model_layer: Layer,
    pub max_latency: RouterLatencyMs,
    pub max_latency_layer: Layer,
    pub max_cost: RouterCostMicroUsd,
    pub max_cost_layer: Layer,
    pub prefer_free: bool,
    pub prefer_free_layer: Layer,
    pub premium_reserve: PremiumReservePercent,
    pub premium_reserve_layer: Layer,
    /// Phase 9I line 536: the user's preferred order over free resources.
    pub free_order: Vec<FreeResourceRef>,
    pub free_order_layer: Layer,
    /// Free resources the user has disabled.
    pub free_disabled: Vec<FreeResourceRef>,
    pub free_disabled_layer: Layer,
    /// The user's pinned free resource, if any.
    pub free_pin: Option<FreeResourceRef>,
    pub free_pin_layer: Layer,
    configured_providers: Vec<String>,
}

impl RoutingRow {
    pub fn new(
        model: Layered<RoutingModelChoice>,
        max_latency: Layered<RouterLatencyMs>,
        max_cost: Layered<RouterCostMicroUsd>,
        prefer_free: Layered<bool>,
        premium_reserve: Layered<PremiumReservePercent>,
        configured_providers: Vec<String>,
    ) -> Self {
        Self {
            model: model.value,
            model_layer: model.layer,
            max_latency: max_latency.value,
            max_latency_layer: max_latency.layer,
            max_cost: max_cost.value,
            max_cost_layer: max_cost.layer,
            prefer_free: prefer_free.value,
            prefer_free_layer: prefer_free.layer,
            premium_reserve: premium_reserve.value,
            premium_reserve_layer: premium_reserve.layer,
            free_order: Vec::new(),
            free_order_layer: Layer::Default,
            free_disabled: Vec::new(),
            free_disabled_layer: Layer::Default,
            free_pin: None,
            free_pin_layer: Layer::Default,
            configured_providers,
        }
    }

    /// The same row, carrying the free-resource preferences resolved for it.
    /// Kept as a builder rather than a wider [`RoutingRow::new`] so an
    /// existing call site that has not been updated to resolve them still
    /// compiles and gets [`Layer::Default`] empty preferences — see this
    /// batch's report for the one call site (`shell::mod`'s `build_settings`)
    /// that still needs to call this.
    pub fn with_free_preferences(
        mut self,
        order: Layered<Vec<FreeResourceRef>>,
        disabled: Layered<Vec<FreeResourceRef>>,
        pin: Layered<Option<FreeResourceRef>>,
    ) -> Self {
        self.free_order = order.value;
        self.free_order_layer = order.layer;
        self.free_disabled = disabled.value;
        self.free_disabled_layer = disabled.layer;
        self.free_pin = pin.value;
        self.free_pin_layer = pin.layer;
        self
    }

    /// This row's three free-resource preferences, folded into the shape
    /// [`crate::routing::disposable::DisposableRouting`] consumes — the
    /// Settings-side counterpart to [`crate::config::RoutingConfig::free_preferences`].
    pub fn free_preferences(&self) -> crate::routing::free::FreePreferences {
        crate::routing::free::FreePreferences::new()
            .with_order(
                self.free_order
                    .iter()
                    .map(FreeResourceRef::to_key)
                    .collect(),
            )
            .with_disabled(
                self.free_disabled
                    .iter()
                    .map(FreeResourceRef::to_key)
                    .collect(),
            )
            .with_pin(self.free_pin.as_ref().map(FreeResourceRef::to_key))
    }

    pub fn defaults(configured_providers: Vec<String>) -> Self {
        Self::new(
            Layered::new(RoutingModelChoice::Deterministic, Layer::Default),
            Layered::new(RouterLatencyMs::DEFAULT, Layer::Default),
            Layered::new(RouterCostMicroUsd::DEFAULT, Layer::Default),
            Layered::new(true, Layer::Default),
            Layered::new(PremiumReservePercent::DEFAULT, Layer::Default),
            configured_providers,
        )
    }
}

/// The effective Memory section: the automatic post-turn memory-extraction
/// trigger and the layer that supplied it, matching [`RoutingRow`]'s shape at
/// one field instead of several. Only `memory_extraction` exists as a
/// producer today — see the packet's "do not add a second memory setting"
/// for why this stays this small.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryRow {
    pub memory_extraction: bool,
    pub memory_extraction_layer: Layer,
}

impl MemoryRow {
    pub fn new(memory_extraction: Layered<bool>) -> Self {
        Self {
            memory_extraction: memory_extraction.value,
            memory_extraction_layer: memory_extraction.layer,
        }
    }

    /// Matches [`crate::config::EffectiveConfig::memory_extraction_enabled`]'s
    /// own default: enabled, at [`Layer::Default`].
    pub fn defaults() -> Self {
        Self::new(Layered::new(true, Layer::Default))
    }
}

/// One edit made to a [`HarnessRow`] this Settings session, not yet written
/// anywhere. `None` in a field means that field was never touched this
/// session; `Some(None)` in `executable` would mean "clear it", though
/// nothing in this module's keymap produces that today — only setting an
/// explicit path does.
#[derive(Debug, Default)]
struct PendingEdit {
    enabled: Option<bool>,
    executable: Option<Option<PathBuf>>,
}

/// A `PendingEdit` together with the harness it belongs to, in the shape
/// the run loop applies to a [`crate::config::IntegrationTable`] when saving.
#[derive(Debug, Clone)]
pub struct SettingsEdit {
    pub id: IntegrationId,
    pub enabled: Option<bool>,
    pub executable: Option<Option<PathBuf>>,
}

/// One staged edit to a provider this Settings session, not yet written
/// anywhere. Unlike [`SettingsEdit`], this carries the whole
/// [`ProviderConfig`] rather than per-field changes: every provider edit —
/// add, edit a field, toggle enabled — already produces a complete new value,
/// so there is no partial-field state worth tracking separately.
#[derive(Debug, Clone)]
pub struct ProviderSettingsEdit {
    pub name: String,
    /// `Some` to add or replace this provider's configuration; `None` to
    /// remove it.
    pub upsert: Option<ProviderConfig>,
}

/// A [`ProfileConfig`] counterpart to [`ProviderSettingsEdit`].
#[derive(Debug, Clone)]
pub struct ProfileSettingsEdit {
    pub name: String,
    pub upsert: Option<ProfileConfig>,
}

/// Routing edits stay per-field so saving one preference never promotes the
/// effective value of another field from its default or opposite layer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoutingSettingsEdit {
    pub model: Option<RoutingModelChoice>,
    pub max_latency: Option<RouterLatencyMs>,
    pub max_cost: Option<RouterCostMicroUsd>,
    pub prefer_free: Option<bool>,
    pub premium_reserve: Option<PremiumReservePercent>,
    /// `Some` when this session set a new order this session — including
    /// `Some(Vec::new())`, an explicit clear.
    pub free_order: Option<Vec<FreeResourceRef>>,
    /// `Some` when this session set a new disabled list — see
    /// [`RoutingSettingsEdit::free_order`].
    pub free_disabled: Option<Vec<FreeResourceRef>>,
    /// `Some(None)` when this session explicitly cleared the pin;
    /// `Some(Some(_))` when it set one; `None` when untouched this session —
    /// the same double-option shape `PendingEdit::executable` uses for the
    /// same reason.
    pub free_pin: Option<Option<FreeResourceRef>>,
}

impl RoutingSettingsEdit {
    pub fn is_empty(&self) -> bool {
        self.model.is_none()
            && self.max_latency.is_none()
            && self.max_cost.is_none()
            && self.prefer_free.is_none()
            && self.premium_reserve.is_none()
            && self.free_order.is_none()
            && self.free_disabled.is_none()
            && self.free_pin.is_none()
    }
}

/// A staged edit to the Memory section this Settings session, not yet
/// written anywhere — [`RoutingSettingsEdit`]'s shape at one field.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemorySettingsEdit {
    pub memory_extraction: Option<bool>,
}

impl MemorySettingsEdit {
    pub fn is_empty(&self) -> bool {
        self.memory_extraction.is_none()
    }
}

/// The inline path editor's state while it is open, for the selected
/// harness row. Mirrors `onboarding::state`'s `PathInput` — same sub-mode,
/// same validate-on-`Enter` behavior via [`exec::resolve_explicit`], same
/// "Esc cancels without changing anything".
#[derive(Debug, Default)]
struct SettingsPathInput {
    buffer: String,
    error: Option<String>,
}

/// Read-only view of the active path-input sub-mode, for rendering.
#[derive(Debug, Clone, Copy)]
pub struct SettingsPathInputView<'a> {
    pub harness_name: &'static str,
    pub buffer: &'a str,
    pub error: Option<&'a str>,
}

/// What a single Providers-section text input is for. Every editable
/// provider field — a brand new provider's name, then its template, or an
/// existing one's base URL or credential variable names — goes through one
/// [`ProviderTextInput`]; only the purpose and what Enter does with the typed
/// text differ. Mirrors [`SettingsPathInput`]'s "type, validate on Enter, Esc
/// cancels without changing anything" shape, generalized to more than one
/// field and chained for the two-step "add a provider" flow.
#[derive(Debug, Clone)]
enum ProviderInputPurpose {
    /// Adding a new provider: this is the name, typed first.
    NewName,
    /// Second step of adding a new provider: which built-in template it is
    /// based on, for the name already accepted in [`ProviderInputPurpose::NewName`].
    NewTemplate {
        name: String,
    },
    EditBaseUrl {
        name: String,
    },
    EditCredentialEnv {
        name: String,
    },
    /// Phase 9I line 527: which of this provider's models the user has
    /// marked free-tier or zero-marginal-cost. Names only, comma-separated,
    /// exactly like [`ProviderInputPurpose::EditCredentialEnv`].
    EditFreeModels {
        name: String,
    },
    /// Typing a credential to put in the OS's own secure store. **The one
    /// purpose whose buffer is a value rather than a name**, which is why
    /// [`ProviderTextInput`]'s `Debug` and
    /// [`SettingsState::provider_input`] both treat every buffer as though
    /// it were this one.
    SetCredential {
        name: String,
    },
}

impl ProviderInputPurpose {
    /// Whether the text being typed is a credential.
    ///
    /// Drives masking on screen. Deliberately a method on the purpose rather
    /// than a flag set at each call site: a new secret-carrying purpose is
    /// then one match arm away from being masked, not one forgotten
    /// `masked: true` away from being echoed.
    fn is_secret(&self) -> bool {
        matches!(self, Self::SetCredential { .. })
    }
}

struct ProviderTextInput {
    purpose: ProviderInputPurpose,
    buffer: String,
    error: Option<String>,
}

/// Renders the buffer as [`crate::secret::REDACTED`] **whatever the
/// purpose**, so a credential cannot reach a log or a panic message through
/// the derived `Debug` of any type that contains one — and so no purpose
/// added later can leak by being forgotten here. What a user is typing into
/// a field is not something a diagnostic needs; the purpose it is being
/// typed for is, and that is kept.
impl fmt::Debug for ProviderTextInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderTextInput")
            .field("purpose", &self.purpose)
            .field("buffer", &crate::secret::REDACTED)
            .field("error", &self.error)
            .finish()
    }
}

/// Read-only view of the active Providers-section text input, for rendering.
///
/// `buffer` is an owned `String` rather than a borrow of the input's own
/// text on purpose: for a credential it is the **masked** rendering, built
/// here, so the typed value never leaves [`SettingsState`] at all. A view
/// that borrowed the real buffer would put the decision "mask it" in the
/// renderer, where forgetting it once is a leak.
pub struct ProviderInputView<'a> {
    pub label: String,
    pub buffer: String,
    pub error: Option<&'a str>,
}

/// The Launch-Profiles-section counterpart to [`ProviderInputPurpose`].
#[derive(Debug, Clone)]
enum ProfileInputPurpose {
    NewName,
    /// Second step of adding a new profile: which harness it applies to, by
    /// slug — see [`IntegrationId::slug`] — for the name already accepted in
    /// [`ProfileInputPurpose::NewName`]. Typed rather than picked from a
    /// list so an unknown harness can be refused with a message naming it,
    /// the same way [`ProviderInputPurpose::NewTemplate`] refuses an unknown
    /// template.
    NewHarness {
        name: String,
    },
    EditModel {
        name: String,
    },
    /// `native`, or the name of a configured provider — see
    /// [`crate::config::ProfileBackend::DirectProvider`].
    EditBackend {
        name: String,
    },
    /// Duplicating an existing profile: the new name, typed once; the
    /// profile named `source` is cloned under it, independent of the
    /// original from the moment it is created.
    Duplicate {
        source: String,
    },
}

#[derive(Debug)]
struct ProfileTextInput {
    purpose: ProfileInputPurpose,
    buffer: String,
    error: Option<String>,
}

/// Read-only view of the active Launch-Profiles-section text input, for
/// rendering.
pub struct ProfileInputView<'a> {
    pub label: String,
    pub buffer: &'a str,
    pub error: Option<&'a str>,
}

#[derive(Debug, Clone, Copy)]
enum RoutingInputPurpose {
    Model,
    MaxLatency,
    MaxCost,
    PremiumReserve,
    /// Phase 9I line 536: `provider:model` pairs, comma-separated, in the
    /// user's preferred order.
    FreeOrder,
    /// Same shape as [`RoutingInputPurpose::FreeOrder`], for the resources
    /// the user has disabled.
    FreeDisabled,
    /// A single `provider:model`, or empty to clear the pin.
    FreePin,
}

#[derive(Debug)]
struct RoutingTextInput {
    purpose: RoutingInputPurpose,
    buffer: String,
    error: Option<String>,
}

/// Read-only view of the active Routing-section field editor.
pub struct RoutingInputView<'a> {
    pub label: &'static str,
    pub buffer: &'a str,
    pub error: Option<&'a str>,
}

/// The outcome of Line 5's connectivity check.
///
/// **This is a real network request now.** It did not used to be: the batch
/// that first shipped this check had no HTTP client on its branch, so it
/// proved only what could be proven without one and said so on screen. `ureq`
/// arrived with the gateway, so the check opens a socket, and the wording
/// that apologised for not doing so is gone.
///
/// The preconditions are still checked first — the provider resolves to a
/// real template, it declares a protocol, that protocol's base URL is
/// non-empty, and when the provider names credential variables at all one of
/// them is set — because a request that cannot possibly work is not worth a
/// socket. But a passing precondition is no longer the answer; it is the
/// permission to go and get one.
///
/// **This reports; it decides nothing.** A failure here must never disable a
/// provider and a success must never enable one — Phase 9D line 1 says
/// "before enabling it for routing", and what happens after the report is the
/// user's to choose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReachabilityCheck {
    /// Preconditions met, request on the wire, no answer yet.
    ///
    /// A state and not a transient: the interface renders this, which is what
    /// makes a slow provider distinguishable from a frozen terminal.
    InFlight {
        protocol: &'static str,
        base_url: String,
        endpoint: String,
    },
    /// The request came back. `endpoint` is the exact URL that was
    /// requested, so "reached" is a claim the user can check.
    Answered {
        protocol: &'static str,
        base_url: String,
        endpoint: String,
        outcome: ProbeOutcome,
    },
    /// A precondition failed and **no request was made**. Kept from the
    /// original shape, and still the right answer for a provider with no base
    /// URL: there is nowhere to send anything.
    Failed(String),
}

/// Which of the two probes a provider row has in flight, or is being asked
/// for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeKind {
    /// Phase 9D line 1: does this provider answer at all?
    Connectivity,
    /// Phase 9D line 2: fetch the model list and replace the cache.
    ModelRefresh,
}

/// What a manual model refresh produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelRefresh {
    /// Request on the wire, no answer yet.
    InFlight { endpoint: String },
    /// The catalogue was replaced. The count and the timestamp are both here
    /// because a refresh that moved neither is a refresh that did nothing.
    Refreshed {
        count: usize,
        fetched_at: i64,
        endpoint: String,
    },
    /// **Not an error.** Phase 9D line 2 says "when the provider exposes
    /// model discovery", so a provider that does not expose it has to produce
    /// a plain sentence rather than a red failure or a control that is
    /// silently dead. The `String` is that sentence, and it distinguishes
    /// "known not to offer one" from "nobody has established whether it
    /// does", which are different facts about the world.
    NotOffered(String),
    /// The request was made and did not produce a catalogue.
    Failed(String),
}

/// The one bottom-panel notice a provider action leaves behind.
///
/// One slot rather than two, so a connectivity result and a refresh result
/// can never both be showing and disagree about which was the last thing the
/// user did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderNotice {
    Reachability(ReachabilityCheck),
    Models(ModelRefresh),
}

/// Everything the run loop needs to make one provider request, and nothing
/// it does not.
///
/// **Names, never a value.** `secret_refs` is a list of
/// [`SecretRef`]s — see that type's own documentation on why holding one
/// reveals nothing — and resolving them is the run loop's job, in the one
/// place that is allowed to touch a credential store. This module works out
/// *what* to ask and never *what the answer is*, exactly as it does for
/// [`Action::StartSession`] and [`Action::OpenSettings`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderProbeIntent {
    pub provider: String,
    pub kind: ProbeKind,
    pub protocol: WireProtocol,
    pub base_url: String,
    pub target: ProbeTarget,
    pub headers: Vec<(String, String)>,
    pub secret_refs: Vec<SecretRef>,
}

/// One finished probe, on its way back from the worker thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderProbeResult {
    pub provider: String,
    pub notice: ProviderNotice,
    /// A refreshed catalogue to put on the row, when there is one.
    pub catalogue: Option<ModelCatalogue>,
}

/// Every [`IntegrationId`] a launch profile may actually name, in
/// [`IntegrationId::ALL`]'s own order.
///
/// Narrower than [`IntegrationId::ALL`] on purpose: `cmux`, Ollama and
/// llama.cpp are real integrations but not launchable coding harnesses — a
/// `ProfileConfig` naming one would be structurally accepted and
/// semantically meaningless, the exact class of mistake "an unknown harness
/// is refused" exists to catch. Found by driving the real binary: an
/// earlier version of this module validated against every
/// [`IntegrationId::ALL`] entry, so typing `cmux` here was silently
/// accepted as a profile's harness.
fn known_launch_harnesses() -> impl Iterator<Item = IntegrationId> {
    IntegrationId::ALL
        .iter()
        .copied()
        .filter(|id| id.kind() == IntegrationKind::Harness)
}

/// What a probe of `name` would ask for, or why it cannot be asked.
///
/// The preconditions come first because a request that cannot possibly work
/// is not worth opening a socket for — and because the failures here are the
/// ones a user can fix without leaving the screen they are on.
///
/// `target` says which URL the probe will request. A provider whose
/// model-list endpoint is established gets [`ProbeTarget::ModelList`], which
/// is the better probe: one request exercises the base URL, TLS, the
/// credential and a real route. A provider whose model list nobody has
/// established gets [`ProbeTarget::BaseUrl`] instead — appending `/models`
/// anyway would be guessing at a path, which is the same failure
/// [`mod@crate::provider`] refuses for a base URL.
///
/// **The first protocol's base URL, exactly as the precondition check has
/// always used.** A provider serving several protocols at different roots —
/// `openrouter` is the one that does — has one model list, and it is under
/// the OpenAI-shaped base URL rather than the Anthropic root. Should a
/// provider ever appear whose first protocol is not the one its model list
/// lives under, this is the line that has to grow a per-protocol answer.
///
/// Presence is checked with [`SecretStore::is_present`], never
/// [`SecretStore::resolve`]: nothing here needs a credential's value, so
/// nothing here asks for one. The value is resolved once, later, by the run
/// loop, immediately before it is put in a header.
fn plan_provider_probe(
    name: &str,
    config: &ProviderConfig,
    kind: ProbeKind,
    secrets: &dyn SecretStore,
) -> Result<ProviderProbeIntent, String> {
    let provider = match config.to_provider(name) {
        Ok(provider) => provider,
        Err(err) => return Err(err.to_string()),
    };
    let Some(support) = provider.protocols.first() else {
        return Err(format!("provider `{name}` declares no protocol"));
    };
    if support.base_url.is_empty() {
        return Err(format!(
            "provider `{name}` has no base URL configured for {}",
            support.protocol
        ));
    }
    if !provider.credential_env.is_empty() {
        let present = provider
            .secret_refs()
            .iter()
            .any(|reference| secrets.is_present(reference));
        if !present {
            return Err(format!(
                "none of provider `{name}`'s credential variable(s) ({}) is set",
                provider.credential_env.join(", ")
            ));
        }
    }

    let target = if provider.model_list_endpoint.is_known_present() {
        ProbeTarget::ModelList
    } else {
        ProbeTarget::BaseUrl
    };

    // Every reference the credential could come from, in the order the
    // provider declares them, with the OS store's own reference first when
    // the configuration records one. The run loop resolves the first that
    // answers; which key of a pool to use is a routing decision neither this
    // function nor `Provider::secret_refs` makes silently.
    let mut secret_refs: Vec<SecretRef> = Vec::new();
    if let Some(stored) = config.credential_store() {
        secret_refs.push(stored.to_secret_ref());
    }
    for reference in provider.secret_refs() {
        if !secret_refs.contains(&reference) {
            secret_refs.push(reference);
        }
    }

    Ok(ProviderProbeIntent {
        provider: name.to_owned(),
        kind,
        protocol: support.protocol,
        base_url: support.base_url.clone(),
        target,
        headers: provider.headers.clone(),
        secret_refs,
    })
}

/// The exact URL `intent` will request.
///
/// Composed here, from the same two fields the run loop hands to
/// [`crate::provider::discovery::ProbeRequest`], so the URL shown in the
/// in-flight line is the URL that is actually requested rather than a second
/// guess at it.
fn probe_endpoint(intent: &ProviderProbeIntent) -> String {
    match intent.target {
        ProbeTarget::BaseUrl => intent.base_url.clone(),
        ProbeTarget::ModelList => format!("{}/models", intent.base_url.trim_end_matches('/')),
    }
}

/// Whether `config`'s provider offers model discovery, or the plain sentence
/// saying why it does not.
///
/// Three answers, not two, because [`Declared`] has three states and the
/// difference matters to the person reading it. "This service is known not to
/// serve a model list" and "nobody has established whether it does" call for
/// different next actions: the first is final, the second is an invitation to
/// go and read the service's documentation. Collapsing them into one
/// "unavailable" would throw that away — the same reason
/// [`mod@crate::harness`] keeps `Unverified` distinct from a verified `false`
/// in the first place.
fn model_discovery_availability(name: &str, config: &ProviderConfig) -> Result<(), String> {
    let provider = match config.to_provider(name) {
        Ok(provider) => provider,
        Err(err) => return Err(err.to_string()),
    };
    match provider.model_list_endpoint {
        Declared::Verified { value: true, .. } => Ok(()),
        Declared::Verified { value: false, .. } => Err(format!(
            "`{name}` is known not to serve a model list, so there is nothing to refresh"
        )),
        Declared::Unverified => Err(format!(
            "no model-discovery endpoint has been established for `{name}`, and Glasshouse \
             will not guess one; read one from the service's own documentation first"
        )),
    }
}

/// What [`SettingsState::handle_key`] wants
/// [`ShellState::handle_settings_key`] to do. Kept separate from [`Action`]
/// because opening and saving Settings need the run loop's file I/O, while
/// everything else here is answered entirely from this module's own data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsAction {
    None,
    Redraw,
    /// Leave the Settings overlay entirely, discarding any unsaved edits —
    /// nothing here asks "are you sure", exactly like leaving the Overview.
    Close,
    /// `w`: apply every pending edit to the user-level configuration.
    SaveUser,
    /// `t` or `m`: a provider probe is planned and the run loop should make
    /// it. Only ever produced once the preconditions passed, so the run loop
    /// never has to re-check them.
    RunProviderProbe,
    /// The confirmed half of `W`: apply every pending edit to the
    /// project-level configuration. Only ever produced after the user
    /// answered the confirmation with `y` or `Enter`.
    SaveProject,
    /// `r`: reopen the first-run wizard. See [`Action::ReopenOnboarding`].
    ReopenOnboarding,
    /// Enter was pressed on a non-empty credential field. The typed value is
    /// still in the input; the run loop takes it with
    /// [`ShellState::take_provider_credential_entry`] and writes it to the
    /// OS store, because touching a keychain is I/O this module deliberately
    /// does not hold — exactly like [`SettingsAction::SaveUser`].
    StoreCredential,
    /// `x`: delete the selected provider's stored credential.
    DeleteCredential,
}

/// Everything the Settings overlay displays and edits.
///
/// # Why an edit shows `Layer::User` before anything is saved
///
/// The design decision says "edits stage in memory and apply to the user
/// layer when saved with `w`" — `w` is the default, one-key save; `W`
/// (project) is the deliberately heavier action requiring confirmation. So
/// the moment a row is edited, it is shown as destined for the user layer,
/// even though nothing has been written yet. If the user instead saves with
/// `W`, the row's layer is corrected the next time the run loop calls
/// `SettingsState::replace_rows` after that write succeeds — which is also
/// what clears `edits`, since by then every pending change has actually
/// landed on disk and a fresh read is the honest source of truth for "which
/// layer supplied this value" from then on.
#[derive(Debug)]
pub struct SettingsState {
    section: SettingsSection,
    harnesses: Vec<HarnessRow>,
    integrations: Vec<IntegrationRow>,
    providers: Vec<ProviderRow>,
    profiles: Vec<ProfileRow>,
    routing: RoutingRow,
    memory: MemoryRow,
    selected_harness: usize,
    selected_integration: usize,
    selected_provider: usize,
    selected_profile: usize,
    edits: HashMap<IntegrationId, PendingEdit>,
    /// Staged provider edits this session, keyed by name — `Some(config)` to
    /// add/replace, `None` to remove. See [`ProviderSettingsEdit`].
    provider_edits: HashMap<String, Option<ProviderConfig>>,
    /// Staged profile edits this session, keyed by name — see
    /// [`ProfileSettingsEdit`].
    profile_edits: HashMap<String, Option<ProfileConfig>>,
    routing_edit: RoutingSettingsEdit,
    memory_edit: MemorySettingsEdit,
    path_input: Option<SettingsPathInput>,
    /// Whether the `W` confirmation prompt (design decision: "first shows
    /// the exact path to be created and requires a distinct confirmation")
    /// is currently showing.
    confirm_project_write: bool,
    /// The provider whose stored credential `x` is offering to delete, while
    /// that confirmation is showing.
    ///
    /// Confirmed for the same reason `W` is, and more so: removing an item
    /// from the operating system's own store is the one action in this
    /// overlay that cannot be undone by declining to save. Every other
    /// provider edit — `d` included — is staged in memory until `w`.
    confirm_credential_delete: Option<String>,
    provider_input: Option<ProviderTextInput>,
    profile_input: Option<ProfileTextInput>,
    routing_input: Option<RoutingTextInput>,
    /// The last provider notice this session, and which provider it was for
    /// — a connectivity result or a model refresh, never both. Cleared by any
    /// other key the general dispatcher in [`SettingsState::handle_key`]
    /// handles — exactly like the status note in the outer shell footer — so
    /// it can never shadow a wizard or field editor that opens afterward.
    ///
    /// Clearing this does **not** cancel a request; an in-flight probe lives
    /// on [`ProviderRow::activity`], which no keystroke touches.
    provider_notice: Option<(String, ProviderNotice)>,
    /// A probe the run loop has not collected yet — see
    /// [`ShellState::take_provider_probe_intent`], which is the only way one
    /// leaves this overlay.
    pending_probe: Option<ProviderProbeIntent>,
    /// The most recent disposable-job routing choice, for Phase 9I line 540
    /// — "show whether a free resource is being used because of user
    /// preference, quota preservation, or fallback". Recorded by
    /// [`ShellState::record_disposable_choice`]; there is no live router in
    /// this build, so nothing here ever sets this on its own. See that
    /// method's own doc for what still has to feed it.
    last_disposable_choice: Option<DisposableChoice>,
}

impl SettingsState {
    fn new(
        harnesses: Vec<HarnessRow>,
        integrations: Vec<IntegrationRow>,
        providers: Vec<ProviderRow>,
        profiles: Vec<ProfileRow>,
        routing: RoutingRow,
        memory: MemoryRow,
    ) -> Self {
        Self {
            section: SettingsSection::Harnesses,
            harnesses,
            integrations,
            providers,
            profiles,
            routing,
            memory,
            selected_harness: 0,
            selected_integration: 0,
            selected_provider: 0,
            selected_profile: 0,
            edits: HashMap::new(),
            provider_edits: HashMap::new(),
            profile_edits: HashMap::new(),
            routing_edit: RoutingSettingsEdit::default(),
            memory_edit: MemorySettingsEdit::default(),
            path_input: None,
            confirm_project_write: false,
            confirm_credential_delete: None,
            provider_input: None,
            profile_input: None,
            routing_input: None,
            provider_notice: None,
            pending_probe: None,
            last_disposable_choice: None,
        }
    }

    pub fn section(&self) -> SettingsSection {
        self.section
    }

    pub fn harnesses(&self) -> &[HarnessRow] {
        &self.harnesses
    }

    pub fn integrations(&self) -> &[IntegrationRow] {
        &self.integrations
    }

    pub fn providers(&self) -> &[ProviderRow] {
        &self.providers
    }

    pub fn profiles(&self) -> &[ProfileRow] {
        &self.profiles
    }

    pub fn routing(&self) -> &RoutingRow {
        &self.routing
    }

    pub fn memory(&self) -> &MemoryRow {
        &self.memory
    }

    /// The most recent disposable-job routing choice, for the Routing
    /// section to render its reason from — see
    /// [`SettingsState::last_disposable_choice`]'s own field doc.
    pub fn last_disposable_choice(&self) -> Option<&DisposableChoice> {
        self.last_disposable_choice.as_ref()
    }

    fn record_disposable_choice(&mut self, choice: DisposableChoice) {
        self.last_disposable_choice = Some(choice);
    }

    pub fn selected_harness(&self) -> usize {
        self.selected_harness
    }

    pub fn selected_integration(&self) -> usize {
        self.selected_integration
    }

    pub fn selected_provider(&self) -> usize {
        self.selected_provider
    }

    pub fn selected_profile(&self) -> usize {
        self.selected_profile
    }

    pub fn confirming_project_write(&self) -> bool {
        self.confirm_project_write
    }

    /// The provider whose stored credential is awaiting a `y`/Esc, if any.
    pub fn confirming_credential_delete(&self) -> Option<&str> {
        self.confirm_credential_delete.as_deref()
    }

    /// The active "add an explicit path" sub-mode, if any.
    pub fn path_input(&self) -> Option<SettingsPathInputView<'_>> {
        let input = self.path_input.as_ref()?;
        let harness_name = self.harnesses.get(self.selected_harness)?.id.display_name();
        Some(SettingsPathInputView {
            harness_name,
            buffer: input.buffer.as_str(),
            error: input.error.as_deref(),
        })
    }

    /// The active Providers-section text input, if any — a new provider's
    /// name then template, or an existing one's base URL or credential
    /// variable names.
    pub fn provider_input(&self) -> Option<ProviderInputView<'_>> {
        let input = self.provider_input.as_ref()?;
        let label = match &input.purpose {
            ProviderInputPurpose::NewName => "New provider name".to_owned(),
            ProviderInputPurpose::NewTemplate { name } => {
                format!("Template for `{name}` (openrouter, zai, openai-compatible, ...)")
            }
            ProviderInputPurpose::EditBaseUrl { name } => format!("Base URL for `{name}`"),
            ProviderInputPurpose::EditCredentialEnv { name } => {
                format!("Credential variable name(s) for `{name}`, comma-separated")
            }
            ProviderInputPurpose::EditFreeModels { name } => {
                format!("Free-tier model name(s) for `{name}`, comma-separated")
            }
            ProviderInputPurpose::SetCredential { name } => {
                format!("Credential for `{name}` (stored in the OS secure store, not shown)")
            }
        };
        // Masked here rather than in the renderer: the typed characters
        // never leave this method, so no view, snapshot or test harness can
        // reach them however it renders. The count of `*` follows the
        // buffer's length, which is what makes a field a user is typing into
        // usable at all — and is on that user's own screen, unlike a `Debug`
        // or a log line, where `ProviderTextInput` reveals nothing.
        let buffer = if input.purpose.is_secret() {
            "*".repeat(input.buffer.chars().count())
        } else {
            input.buffer.clone()
        };
        Some(ProviderInputView {
            label,
            buffer,
            error: input.error.as_deref(),
        })
    }

    /// The most recent connectivity check, if that is what the notice is —
    /// see [`ReachabilityCheck`].
    pub fn provider_test_result(&self) -> Option<(&str, &ReachabilityCheck)> {
        match self.provider_notice.as_ref()? {
            (name, ProviderNotice::Reachability(check)) => Some((name.as_str(), check)),
            (_, ProviderNotice::Models(_)) => None,
        }
    }

    /// The most recent model refresh, if that is what the notice is.
    pub fn provider_models_result(&self) -> Option<(&str, &ModelRefresh)> {
        match self.provider_notice.as_ref()? {
            (name, ProviderNotice::Models(refresh)) => Some((name.as_str(), refresh)),
            (_, ProviderNotice::Reachability(_)) => None,
        }
    }

    /// The active Launch-Profiles-section text input, if any — a new
    /// profile's name then harness, or an existing one's model or backend.
    pub fn profile_input(&self) -> Option<ProfileInputView<'_>> {
        let input = self.profile_input.as_ref()?;
        let label = match &input.purpose {
            ProfileInputPurpose::NewName => "New launch profile name".to_owned(),
            ProfileInputPurpose::NewHarness { name } => format!(
                "Harness for `{name}` ({})",
                known_launch_harnesses()
                    .map(|id| id.slug())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            ProfileInputPurpose::EditModel { name } => format!("Model override for `{name}`"),
            ProfileInputPurpose::EditBackend { name } => {
                format!("Backend for `{name}`: `native` or a configured provider name")
            }
            ProfileInputPurpose::Duplicate { source } => {
                format!("New name for a copy of `{source}`")
            }
        };
        Some(ProfileInputView {
            label,
            buffer: input.buffer.as_str(),
            error: input.error.as_deref(),
        })
    }

    /// The active Routing-section editor, if any.
    pub fn routing_input(&self) -> Option<RoutingInputView<'_>> {
        let input = self.routing_input.as_ref()?;
        let label = match input.purpose {
            RoutingInputPurpose::Model => {
                "Routing model (automatic, deterministic, or provider:model)"
            }
            RoutingInputPurpose::MaxLatency => "Maximum router latency (milliseconds)",
            RoutingInputPurpose::MaxCost => "Maximum marginal cost (USD per decision)",
            RoutingInputPurpose::PremiumReserve => "Premium reserve threshold (percent)",
            RoutingInputPurpose::FreeOrder => {
                "Free-resource order: provider:model, comma-separated"
            }
            RoutingInputPurpose::FreeDisabled => {
                "Disabled free resources: provider:model, comma-separated"
            }
            RoutingInputPurpose::FreePin => "Pinned free resource: provider:model, or empty",
        };
        Some(RoutingInputView {
            label,
            buffer: input.buffer.as_str(),
            error: input.error.as_deref(),
        })
    }

    /// Every pending harness edit, for the run loop to apply when saving.
    fn edits(&self) -> Vec<SettingsEdit> {
        self.edits
            .iter()
            .map(|(&id, edit)| SettingsEdit {
                id,
                enabled: edit.enabled,
                executable: edit.executable.clone(),
            })
            .collect()
    }

    /// Every pending provider edit, for the run loop to apply when saving.
    fn provider_edits(&self) -> Vec<ProviderSettingsEdit> {
        self.provider_edits
            .iter()
            .map(|(name, upsert)| ProviderSettingsEdit {
                name: name.clone(),
                upsert: upsert.clone(),
            })
            .collect()
    }

    /// Every pending profile edit, for the run loop to apply when saving.
    fn profile_edits(&self) -> Vec<ProfileSettingsEdit> {
        self.profile_edits
            .iter()
            .map(|(name, upsert)| ProfileSettingsEdit {
                name: name.clone(),
                upsert: upsert.clone(),
            })
            .collect()
    }

    fn routing_edit(&self) -> Option<RoutingSettingsEdit> {
        (!self.routing_edit.is_empty()).then(|| self.routing_edit.clone())
    }

    fn memory_edit(&self) -> Option<MemorySettingsEdit> {
        (!self.memory_edit.is_empty()).then(|| self.memory_edit.clone())
    }

    /// Replace the rows with freshly loaded ones (after a successful save)
    /// and clear every pending edit. The catalog is fixed-size, so the
    /// cursor is only ever clamped, never reset, and always stays on a real
    /// row.
    fn replace_rows(
        &mut self,
        harnesses: Vec<HarnessRow>,
        integrations: Vec<IntegrationRow>,
        providers: Vec<ProviderRow>,
        profiles: Vec<ProfileRow>,
        routing: RoutingRow,
        memory: MemoryRow,
    ) {
        self.selected_harness = self.selected_harness.min(harnesses.len().saturating_sub(1));
        self.selected_integration = self
            .selected_integration
            .min(integrations.len().saturating_sub(1));
        self.selected_provider = self
            .selected_provider
            .min(providers.len().saturating_sub(1));
        self.selected_profile = self.selected_profile.min(profiles.len().saturating_sub(1));
        self.harnesses = harnesses;
        self.integrations = integrations;
        self.providers = providers;
        self.profiles = profiles;
        self.routing = routing;
        self.memory = memory;
        self.edits.clear();
        self.provider_edits.clear();
        self.profile_edits.clear();
        self.routing_edit = RoutingSettingsEdit::default();
        self.memory_edit = MemorySettingsEdit::default();
    }

    fn handle_key(&mut self, key: KeyEvent) -> SettingsAction {
        if self.path_input.is_some() {
            return self.handle_path_input_key(key);
        }
        if self.provider_input.is_some() {
            return self.handle_provider_input_key(key);
        }
        if self.profile_input.is_some() {
            return self.handle_profile_input_key(key);
        }
        if self.routing_input.is_some() {
            return self.handle_routing_input_key(key);
        }
        if let Some(provider) = self.confirm_credential_delete.clone() {
            return match key.code {
                KeyCode::Enter | KeyCode::Char('y') => {
                    self.confirm_credential_delete = None;
                    // The row must still be the one that was confirmed: a
                    // confirmation that deleted whatever happens to be
                    // selected now would be a different action from the one
                    // the user agreed to.
                    if self
                        .providers
                        .get(self.selected_provider)
                        .is_some_and(|row| row.name == provider)
                    {
                        SettingsAction::DeleteCredential
                    } else {
                        SettingsAction::Redraw
                    }
                }
                KeyCode::Esc | KeyCode::Char('n') => {
                    self.confirm_credential_delete = None;
                    SettingsAction::Redraw
                }
                // Swallowed, exactly like the project-write confirmation:
                // an explicit y/Enter or Esc/n, never "any key dismisses".
                _ => SettingsAction::None,
            };
        }

        if self.confirm_project_write {
            return match key.code {
                KeyCode::Enter | KeyCode::Char('y') => {
                    self.confirm_project_write = false;
                    SettingsAction::SaveProject
                }
                KeyCode::Esc | KeyCode::Char('n') => {
                    self.confirm_project_write = false;
                    SettingsAction::Redraw
                }
                // Anything else is swallowed: the design decision requires
                // an explicit y/Enter or Esc/n, not "any key dismisses".
                _ => SettingsAction::None,
            };
        }

        // A stale provider banner must not permanently shadow another
        // bottom-panel view — the wizard/field editors this session opens
        // afterward all render in that same area — so any key that reaches
        // this general dispatcher clears it first, exactly like the outer
        // shell's own status note clears on the next keystroke. The `t` and
        // `m` arms below set it again in the same keypress when that is what
        // was actually pressed.
        //
        // This clears a *banner*, never a request. An in-flight probe is on
        // `ProviderRow::activity` precisely so that pressing an arrow key
        // cannot make a running request invisible.
        self.provider_notice = None;

        match key.code {
            KeyCode::Esc => SettingsAction::Close,
            KeyCode::Char('w') => SettingsAction::SaveUser,
            KeyCode::Char('W') => {
                self.confirm_project_write = true;
                SettingsAction::Redraw
            }
            KeyCode::Char('r') => SettingsAction::ReopenOnboarding,
            KeyCode::Tab | KeyCode::Right => {
                self.section = self.section.next();
                SettingsAction::Redraw
            }
            KeyCode::BackTab | KeyCode::Left => {
                self.section = self.section.previous();
                SettingsAction::Redraw
            }
            KeyCode::Up => {
                self.move_selection(-1);
                SettingsAction::Redraw
            }
            KeyCode::Down => {
                self.move_selection(1);
                SettingsAction::Redraw
            }
            KeyCode::Char(' ') if self.section == SettingsSection::Harnesses => {
                self.toggle_selected_harness();
                SettingsAction::Redraw
            }
            KeyCode::Enter if self.section == SettingsSection::Harnesses => {
                if self.harnesses.get(self.selected_harness).is_some() {
                    self.path_input = Some(SettingsPathInput::default());
                }
                SettingsAction::Redraw
            }
            KeyCode::Char('a') if self.section == SettingsSection::Providers => {
                self.provider_input = Some(ProviderTextInput {
                    purpose: ProviderInputPurpose::NewName,
                    buffer: String::new(),
                    error: None,
                });
                SettingsAction::Redraw
            }
            KeyCode::Char('e') if self.section == SettingsSection::Providers => {
                self.start_edit_provider_base_url();
                SettingsAction::Redraw
            }
            KeyCode::Char('c') if self.section == SettingsSection::Providers => {
                self.start_edit_provider_credential_env();
                SettingsAction::Redraw
            }
            KeyCode::Char('f') if self.section == SettingsSection::Providers => {
                self.start_edit_provider_free_models();
                SettingsAction::Redraw
            }
            KeyCode::Char(' ') if self.section == SettingsSection::Providers => {
                self.toggle_selected_provider();
                SettingsAction::Redraw
            }
            KeyCode::Char('d') if self.section == SettingsSection::Providers => {
                self.remove_selected_provider();
                SettingsAction::Redraw
            }
            KeyCode::Char('t') if self.section == SettingsSection::Providers => {
                if self.begin_provider_test() {
                    SettingsAction::RunProviderProbe
                } else {
                    SettingsAction::Redraw
                }
            }
            KeyCode::Char('m') if self.section == SettingsSection::Providers => {
                if self.begin_provider_model_refresh() {
                    SettingsAction::RunProviderProbe
                } else {
                    SettingsAction::Redraw
                }
            }
            KeyCode::Char('s') if self.section == SettingsSection::Providers => {
                self.start_set_provider_credential();
                SettingsAction::Redraw
            }
            KeyCode::Char('x') if self.section == SettingsSection::Providers => {
                self.confirm_credential_delete = self
                    .providers
                    .get(self.selected_provider)
                    .map(|row| row.name.clone());
                SettingsAction::Redraw
            }
            KeyCode::Char('a') if self.section == SettingsSection::LaunchProfiles => {
                self.profile_input = Some(ProfileTextInput {
                    purpose: ProfileInputPurpose::NewName,
                    buffer: String::new(),
                    error: None,
                });
                SettingsAction::Redraw
            }
            KeyCode::Char('e') if self.section == SettingsSection::LaunchProfiles => {
                self.start_edit_profile_model();
                SettingsAction::Redraw
            }
            KeyCode::Char('b') if self.section == SettingsSection::LaunchProfiles => {
                self.start_edit_profile_backend();
                SettingsAction::Redraw
            }
            KeyCode::Char('p') if self.section == SettingsSection::LaunchProfiles => {
                self.cycle_selected_profile_approval();
                SettingsAction::Redraw
            }
            KeyCode::Char('u') if self.section == SettingsSection::LaunchProfiles => {
                self.start_duplicate_profile();
                SettingsAction::Redraw
            }
            KeyCode::Char(' ') if self.section == SettingsSection::LaunchProfiles => {
                self.toggle_selected_profile();
                SettingsAction::Redraw
            }
            KeyCode::Char('d') if self.section == SettingsSection::LaunchProfiles => {
                self.remove_selected_profile();
                SettingsAction::Redraw
            }
            KeyCode::Char('m') if self.section == SettingsSection::Routing => {
                self.start_routing_input(RoutingInputPurpose::Model);
                SettingsAction::Redraw
            }
            KeyCode::Char('l') if self.section == SettingsSection::Routing => {
                self.start_routing_input(RoutingInputPurpose::MaxLatency);
                SettingsAction::Redraw
            }
            KeyCode::Char('c') if self.section == SettingsSection::Routing => {
                self.start_routing_input(RoutingInputPurpose::MaxCost);
                SettingsAction::Redraw
            }
            KeyCode::Char('f') if self.section == SettingsSection::Routing => {
                self.routing.prefer_free = !self.routing.prefer_free;
                self.routing.prefer_free_layer = Layer::User;
                self.routing_edit.prefer_free = Some(self.routing.prefer_free);
                SettingsAction::Redraw
            }
            KeyCode::Char('p') if self.section == SettingsSection::Routing => {
                self.start_routing_input(RoutingInputPurpose::PremiumReserve);
                SettingsAction::Redraw
            }
            KeyCode::Char('o') if self.section == SettingsSection::Routing => {
                self.start_routing_input(RoutingInputPurpose::FreeOrder);
                SettingsAction::Redraw
            }
            KeyCode::Char('d') if self.section == SettingsSection::Routing => {
                self.start_routing_input(RoutingInputPurpose::FreeDisabled);
                SettingsAction::Redraw
            }
            KeyCode::Char('n') if self.section == SettingsSection::Routing => {
                self.start_routing_input(RoutingInputPurpose::FreePin);
                SettingsAction::Redraw
            }
            KeyCode::Char(' ') if self.section == SettingsSection::Memory => {
                self.memory.memory_extraction = !self.memory.memory_extraction;
                self.memory.memory_extraction_layer = Layer::User;
                self.memory_edit.memory_extraction = Some(self.memory.memory_extraction);
                SettingsAction::Redraw
            }
            _ => SettingsAction::None,
        }
    }

    fn move_selection(&mut self, delta: i32) {
        match self.section {
            SettingsSection::Harnesses => {
                if self.harnesses.is_empty() {
                    return;
                }
                let last = self.harnesses.len() as i32 - 1;
                self.selected_harness =
                    (self.selected_harness as i32 + delta).clamp(0, last) as usize;
            }
            SettingsSection::Integrations => {
                if self.integrations.is_empty() {
                    return;
                }
                let last = self.integrations.len() as i32 - 1;
                self.selected_integration =
                    (self.selected_integration as i32 + delta).clamp(0, last) as usize;
            }
            SettingsSection::Providers => {
                if self.providers.is_empty() {
                    return;
                }
                let last = self.providers.len() as i32 - 1;
                self.selected_provider =
                    (self.selected_provider as i32 + delta).clamp(0, last) as usize;
            }
            SettingsSection::LaunchProfiles => {
                if self.profiles.is_empty() {
                    return;
                }
                let last = self.profiles.len() as i32 - 1;
                self.selected_profile =
                    (self.selected_profile as i32 + delta).clamp(0, last) as usize;
            }
            SettingsSection::Routing | SettingsSection::Memory => {}
        }
    }

    fn toggle_selected_harness(&mut self) {
        let Some(row) = self.harnesses.get_mut(self.selected_harness) else {
            return;
        };
        row.enabled = !row.enabled;
        row.enabled_layer = Layer::User;
        self.edits.entry(row.id).or_default().enabled = Some(row.enabled);
    }

    // -------------------------------------------------------------
    // Providers
    // -------------------------------------------------------------

    fn start_edit_provider_base_url(&mut self) {
        let Some(row) = self.providers.get(self.selected_provider) else {
            return;
        };
        self.provider_input = Some(ProviderTextInput {
            purpose: ProviderInputPurpose::EditBaseUrl {
                name: row.name.clone(),
            },
            buffer: row.config.base_url().unwrap_or_default().to_owned(),
            error: None,
        });
    }

    fn start_edit_provider_credential_env(&mut self) {
        let Some(row) = self.providers.get(self.selected_provider) else {
            return;
        };
        self.provider_input = Some(ProviderTextInput {
            purpose: ProviderInputPurpose::EditCredentialEnv {
                name: row.name.clone(),
            },
            buffer: row.config.credential_env().join(","),
            error: None,
        });
    }

    fn start_edit_provider_free_models(&mut self) {
        let Some(row) = self.providers.get(self.selected_provider) else {
            return;
        };
        self.provider_input = Some(ProviderTextInput {
            purpose: ProviderInputPurpose::EditFreeModels {
                name: row.name.clone(),
            },
            buffer: row.config.free_models().join(","),
            error: None,
        });
    }

    /// Open the masked credential field for the selected provider.
    ///
    /// The buffer starts empty and is never pre-filled from anywhere: there
    /// is nothing to pre-fill it *with* that would not mean reading a
    /// credential out of a store in order to display it.
    fn start_set_provider_credential(&mut self) {
        let Some(row) = self.providers.get(self.selected_provider) else {
            return;
        };
        if row.config.credential_env().is_empty() {
            self.provider_input = Some(ProviderTextInput {
                purpose: ProviderInputPurpose::SetCredential {
                    name: row.name.clone(),
                },
                buffer: String::new(),
                error: Some(
                    "this provider names no credential variable yet — set one with `c` first, \
                     so the stored credential has a name to be found by"
                        .to_owned(),
                ),
            });
            return;
        }
        self.provider_input = Some(ProviderTextInput {
            purpose: ProviderInputPurpose::SetCredential {
                name: row.name.clone(),
            },
            buffer: String::new(),
            error: None,
        });
    }

    /// The provider the credential field is for, and what the user typed —
    /// **taken**, so this method can be called once and the value is gone
    /// from the overlay afterwards.
    fn take_credential_entry(&mut self) -> Option<(String, String)> {
        let input = self.provider_input.take()?;
        let ProviderInputPurpose::SetCredential { name } = input.purpose else {
            // Not a credential field: put it back rather than discarding an
            // edit the user is in the middle of making.
            self.provider_input = Some(input);
            return None;
        };
        Some((name, input.buffer))
    }

    /// The selected provider and every reference under which its credential
    /// could be stored.
    ///
    /// More than one, because a provider may declare a pool of credential
    /// variable names and a stored reference may also have been recorded in
    /// configuration. Deleting means deleting all of them: a "delete my
    /// stored key" that left one of two copies behind would be a worse
    /// answer than raising.
    fn selected_provider_stored_credentials(&self) -> Option<(String, Vec<SecretRef>)> {
        let row = self.providers.get(self.selected_provider)?;
        let mut references: Vec<SecretRef> = Vec::new();
        if let Some(stored) = row.config.credential_store() {
            references.push(stored.to_secret_ref());
        }
        for var in row.config.credential_env() {
            let reference = os_credential_for_variable(var);
            if !references.contains(&reference) {
                references.push(reference);
            }
        }
        Some((row.name.clone(), references))
    }

    /// Record that `provider`'s credential now lives in the OS store, and
    /// stage the configuration change that says so.
    fn record_credential_stored(&mut self, provider: &str, stored: StoredCredentialRef) {
        let Some(row) = self.providers.iter_mut().find(|row| row.name == provider) else {
            return;
        };
        row.config.set_credential_store(Some(stored));
        row.layer = Layer::User;
        self.provider_edits
            .insert(row.name.clone(), Some(row.config.clone()));
    }

    /// The configuration half of deleting a stored credential: the reference
    /// goes, every other field stays.
    fn record_credential_cleared(&mut self, provider: &str) {
        let Some(row) = self.providers.iter_mut().find(|row| row.name == provider) else {
            return;
        };
        if row.config.credential_store().is_none() {
            return;
        }
        row.config.set_credential_store(None);
        row.layer = Layer::User;
        self.provider_edits
            .insert(row.name.clone(), Some(row.config.clone()));
    }

    fn toggle_selected_provider(&mut self) {
        let Some(row) = self.providers.get_mut(self.selected_provider) else {
            return;
        };
        row.config.set_enabled(!row.config.enabled());
        row.layer = Layer::User;
        self.provider_edits
            .insert(row.name.clone(), Some(row.config.clone()));
    }

    fn remove_selected_provider(&mut self) {
        if self.selected_provider >= self.providers.len() {
            return;
        }
        let row = self.providers.remove(self.selected_provider);
        self.provider_edits.insert(row.name, None);
        self.selected_provider = self
            .selected_provider
            .min(self.providers.len().saturating_sub(1));
        self.provider_notice = None;
    }

    /// `t`: plan a connectivity probe of the selected provider and hand it
    /// to the run loop.
    ///
    /// Returns `true` when there is something for the run loop to do, so the
    /// caller can raise [`SettingsAction::RunProviderProbe`] rather than
    /// guessing. A precondition failure sets the banner here and returns
    /// `false`: nothing was asked of the network, so nothing needs the run
    /// loop.
    fn begin_provider_test(&mut self) -> bool {
        self.begin_provider_probe(ProbeKind::Connectivity)
    }

    /// `m`: Phase 9D line 2, and manual by construction — this runs because
    /// a key was pressed and there is no other caller.
    fn begin_provider_model_refresh(&mut self) -> bool {
        let Some(row) = self.providers.get(self.selected_provider) else {
            return false;
        };
        // Asked before anything else: a provider with no model list must
        // produce a sentence, not a failed request against a guessed path.
        if let Err(why) = model_discovery_availability(&row.name, &row.config) {
            let name = row.name.clone();
            self.provider_notice =
                Some((name, ProviderNotice::Models(ModelRefresh::NotOffered(why))));
            return false;
        }
        self.begin_provider_probe(ProbeKind::ModelRefresh)
    }

    fn begin_provider_probe(&mut self, kind: ProbeKind) -> bool {
        let Some(row) = self.providers.get(self.selected_provider) else {
            return false;
        };
        let name = row.name.clone();

        // A second press while one is already running would open a second
        // socket and leave two results racing for one banner. Refused, and
        // said out loud rather than ignored — a key that silently does
        // nothing is indistinguishable from a frozen screen.
        if row.activity.is_some() {
            self.provider_notice = Some((
                name.clone(),
                ProviderNotice::Reachability(ReachabilityCheck::Failed(format!(
                    "a request for `{name}` is already running; wait for it to come back"
                ))),
            ));
            return false;
        }

        // The store a launch would actually use, not just the environment: a
        // key the user put in the Keychain is a key this check must count as
        // present, or `t` would report a provider as unusable that launches
        // perfectly well.
        let intent =
            match plan_provider_probe(&name, &row.config, kind, &PreferNativeSecretStore::detect())
            {
                Ok(intent) => intent,
                Err(why) => {
                    self.provider_notice = Some(match kind {
                        ProbeKind::Connectivity => (
                            name,
                            ProviderNotice::Reachability(ReachabilityCheck::Failed(why)),
                        ),
                        ProbeKind::ModelRefresh => {
                            (name, ProviderNotice::Models(ModelRefresh::Failed(why)))
                        }
                    });
                    return false;
                }
            };

        let endpoint = probe_endpoint(&intent);
        self.provider_notice = Some(match kind {
            ProbeKind::Connectivity => (
                name.clone(),
                ProviderNotice::Reachability(ReachabilityCheck::InFlight {
                    protocol: intent.protocol.slug(),
                    base_url: intent.base_url.clone(),
                    endpoint,
                }),
            ),
            ProbeKind::ModelRefresh => (
                name.clone(),
                ProviderNotice::Models(ModelRefresh::InFlight { endpoint }),
            ),
        });
        if let Some(row) = self.providers.get_mut(self.selected_provider) {
            row.activity = Some(kind);
        }
        self.pending_probe = Some(intent);
        true
    }

    /// A finished probe, back from the run loop.
    ///
    /// The row's in-flight marker is cleared whatever the outcome, including
    /// for a provider the user has since deleted — the lookup simply finds
    /// nothing and the banner still tells them what happened to the request
    /// they started.
    fn apply_probe_result(&mut self, result: ProviderProbeResult) {
        if let Some(row) = self
            .providers
            .iter_mut()
            .find(|row| row.name == result.provider)
        {
            row.activity = None;
            if let Some(catalogue) = result.catalogue {
                row.models = Some(catalogue);
            }
        }
        self.provider_notice = Some((result.provider, result.notice));
    }

    fn take_probe_intent(&mut self) -> Option<ProviderProbeIntent> {
        self.pending_probe.take()
    }

    /// Whether any provider row has a request on the wire.
    fn any_probe_in_flight(&self) -> bool {
        self.providers.iter().any(|row| row.activity.is_some())
    }

    fn handle_provider_input_key(&mut self, key: KeyEvent) -> SettingsAction {
        match key.code {
            KeyCode::Esc => {
                self.provider_input = None;
                SettingsAction::Redraw
            }
            KeyCode::Enter => {
                // A credential is not applied here: writing one to the OS
                // store is I/O, so the input is left standing and the run
                // loop takes the value out of it.
                if self
                    .provider_input
                    .as_ref()
                    .is_some_and(|input| input.purpose.is_secret())
                {
                    let empty = self
                        .provider_input
                        .as_ref()
                        .is_some_and(|input| input.buffer.trim().is_empty());
                    if empty {
                        if let Some(input) = self.provider_input.as_mut() {
                            input.error = Some(
                                "a credential needs a value; press Esc to leave it unchanged"
                                    .to_owned(),
                            );
                        }
                        return SettingsAction::Redraw;
                    }
                    return SettingsAction::StoreCredential;
                }
                self.confirm_provider_input();
                SettingsAction::Redraw
            }
            KeyCode::Backspace => {
                if let Some(input) = self.provider_input.as_mut() {
                    input.buffer.pop();
                    input.error = None;
                }
                SettingsAction::Redraw
            }
            KeyCode::Char(c) => {
                if let Some(input) = self.provider_input.as_mut() {
                    input.buffer.push(c);
                    input.error = None;
                }
                SettingsAction::Redraw
            }
            _ => SettingsAction::None,
        }
    }

    /// Apply the typed text for whichever [`ProviderInputPurpose`] is
    /// active. On success this closes the input (`self.provider_input =
    /// None`, already true from the `take()` below unless a validation
    /// failure re-opens it with an error attached); on failure it re-opens
    /// the same input with `error` set, so Esc still cancels and the buffer
    /// is not lost.
    fn confirm_provider_input(&mut self) {
        let Some(input) = self.provider_input.take() else {
            return;
        };
        let typed = input.buffer.trim().to_owned();
        match input.purpose {
            ProviderInputPurpose::NewName => {
                if typed.is_empty() {
                    self.provider_input = Some(ProviderTextInput {
                        purpose: ProviderInputPurpose::NewName,
                        buffer: input.buffer,
                        error: Some("a provider needs a name".to_owned()),
                    });
                    return;
                }
                if self.providers.iter().any(|row| row.name == typed) {
                    self.provider_input = Some(ProviderTextInput {
                        purpose: ProviderInputPurpose::NewName,
                        buffer: input.buffer,
                        error: Some(format!("a provider named `{typed}` already exists")),
                    });
                    return;
                }
                self.provider_input = Some(ProviderTextInput {
                    purpose: ProviderInputPurpose::NewTemplate { name: typed },
                    buffer: String::new(),
                    error: None,
                });
            }
            ProviderInputPurpose::NewTemplate { name } => {
                if crate::provider::template(&typed).is_none() {
                    let known: Vec<String> = crate::provider::templates()
                        .into_iter()
                        .map(|p| p.name)
                        .collect();
                    self.provider_input = Some(ProviderTextInput {
                        purpose: ProviderInputPurpose::NewTemplate { name },
                        buffer: input.buffer,
                        error: Some(format!(
                            "`{typed}` is not a known provider template; known templates are: {}",
                            known.join(", ")
                        )),
                    });
                    return;
                }
                let config = ProviderConfig::new(typed);
                self.providers
                    .push(ProviderRow::new(name.clone(), config.clone(), Layer::User));
                self.providers.sort_by(|a, b| a.name.cmp(&b.name));
                self.selected_provider = self
                    .providers
                    .iter()
                    .position(|row| row.name == name)
                    .unwrap_or(0);
                self.provider_edits.insert(name, Some(config));
            }
            ProviderInputPurpose::EditBaseUrl { name } => {
                let value = (!typed.is_empty()).then_some(typed);
                if let Some(row) = self.providers.iter_mut().find(|row| row.name == name) {
                    row.config.set_base_url(value);
                    row.layer = Layer::User;
                    self.provider_edits.insert(name, Some(row.config.clone()));
                }
            }
            ProviderInputPurpose::EditCredentialEnv { name } => {
                let names: Vec<String> = typed
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect();
                if let Some(row) = self.providers.iter_mut().find(|row| row.name == name) {
                    row.config.set_credential_env(names);
                    row.layer = Layer::User;
                    self.provider_edits.insert(name, Some(row.config.clone()));
                }
            }
            ProviderInputPurpose::EditFreeModels { name } => {
                let names: Vec<String> = typed
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect();
                if let Some(row) = self.providers.iter_mut().find(|row| row.name == name) {
                    row.config.set_free_models(names);
                    row.layer = Layer::User;
                    self.provider_edits.insert(name, Some(row.config.clone()));
                }
            }
            // Never reached: `handle_provider_input_key` answers Enter on a
            // credential field with `SettingsAction::StoreCredential` and
            // leaves the input standing, because storing one is I/O. Written
            // as a no-op rather than a panic so that a future path into here
            // discards the value instead of printing a backtrace next to it.
            ProviderInputPurpose::SetCredential { .. } => {}
        }
    }

    // -------------------------------------------------------------
    // Launch profiles
    // -------------------------------------------------------------

    fn start_edit_profile_model(&mut self) {
        let Some(row) = self.profiles.get(self.selected_profile) else {
            return;
        };
        self.profile_input = Some(ProfileTextInput {
            purpose: ProfileInputPurpose::EditModel {
                name: row.name.clone(),
            },
            buffer: row.config.model().unwrap_or_default().to_owned(),
            error: None,
        });
    }

    fn start_edit_profile_backend(&mut self) {
        let Some(row) = self.profiles.get(self.selected_profile) else {
            return;
        };
        let buffer = match row.config.backend() {
            ProfileBackend::Native => "native".to_owned(),
            ProfileBackend::DirectProvider { provider } => provider.clone(),
            ProfileBackend::GlasshouseGateway => String::new(),
        };
        self.profile_input = Some(ProfileTextInput {
            purpose: ProfileInputPurpose::EditBackend {
                name: row.name.clone(),
            },
            buffer,
            error: None,
        });
    }

    fn start_duplicate_profile(&mut self) {
        let Some(row) = self.profiles.get(self.selected_profile) else {
            return;
        };
        self.profile_input = Some(ProfileTextInput {
            purpose: ProfileInputPurpose::Duplicate {
                source: row.name.clone(),
            },
            buffer: String::new(),
            error: None,
        });
    }

    fn toggle_selected_profile(&mut self) {
        let Some(row) = self.profiles.get_mut(self.selected_profile) else {
            return;
        };
        row.config.set_enabled(!row.config.enabled());
        row.layer = Layer::User;
        self.profile_edits
            .insert(row.name.clone(), Some(row.config.clone()));
    }

    fn remove_selected_profile(&mut self) {
        if self.selected_profile >= self.profiles.len() {
            return;
        }
        let row = self.profiles.remove(self.selected_profile);
        self.profile_edits.insert(row.name, None);
        self.selected_profile = self
            .selected_profile
            .min(self.profiles.len().saturating_sub(1));
    }

    fn cycle_selected_profile_approval(&mut self) {
        let Some(row) = self.profiles.get_mut(self.selected_profile) else {
            return;
        };
        let next = match row.config.approval() {
            ProfileApproval::Default => ProfileApproval::AutomaticReview,
            ProfileApproval::AutomaticReview => ProfileApproval::Bypass,
            ProfileApproval::Bypass => ProfileApproval::Default,
        };
        row.config.set_approval(next);
        row.layer = Layer::User;
        self.profile_edits
            .insert(row.name.clone(), Some(row.config.clone()));
    }

    fn handle_profile_input_key(&mut self, key: KeyEvent) -> SettingsAction {
        match key.code {
            KeyCode::Esc => {
                self.profile_input = None;
                SettingsAction::Redraw
            }
            KeyCode::Enter => {
                self.confirm_profile_input();
                SettingsAction::Redraw
            }
            KeyCode::Backspace => {
                if let Some(input) = self.profile_input.as_mut() {
                    input.buffer.pop();
                    input.error = None;
                }
                SettingsAction::Redraw
            }
            KeyCode::Char(c) => {
                if let Some(input) = self.profile_input.as_mut() {
                    input.buffer.push(c);
                    input.error = None;
                }
                SettingsAction::Redraw
            }
            _ => SettingsAction::None,
        }
    }

    /// Apply the typed text for whichever [`ProfileInputPurpose`] is active
    /// — see [`SettingsState::confirm_provider_input`]'s doc for the
    /// success/failure shape this mirrors.
    fn confirm_profile_input(&mut self) {
        let Some(input) = self.profile_input.take() else {
            return;
        };
        let typed = input.buffer.trim().to_owned();
        match input.purpose {
            ProfileInputPurpose::NewName => {
                if typed.is_empty() {
                    self.profile_input = Some(ProfileTextInput {
                        purpose: ProfileInputPurpose::NewName,
                        buffer: input.buffer,
                        error: Some("a launch profile needs a name".to_owned()),
                    });
                    return;
                }
                if self.profiles.iter().any(|row| row.name == typed) {
                    self.profile_input = Some(ProfileTextInput {
                        purpose: ProfileInputPurpose::NewName,
                        buffer: input.buffer,
                        error: Some(format!("a launch profile named `{typed}` already exists")),
                    });
                    return;
                }
                self.profile_input = Some(ProfileTextInput {
                    purpose: ProfileInputPurpose::NewHarness { name: typed },
                    buffer: String::new(),
                    error: None,
                });
            }
            ProfileInputPurpose::NewHarness { name } => {
                let Some(harness) = known_launch_harnesses().find(|id| id.slug() == typed) else {
                    let known: Vec<&str> = known_launch_harnesses().map(|id| id.slug()).collect();
                    self.profile_input = Some(ProfileTextInput {
                        purpose: ProfileInputPurpose::NewHarness { name },
                        buffer: input.buffer,
                        error: Some(format!(
                            "`{typed}` is not a harness Glasshouse knows; known harnesses are: {}",
                            known.join(", ")
                        )),
                    });
                    return;
                };
                let config = ProfileConfig::new(harness);
                self.profiles.push(ProfileRow {
                    name: name.clone(),
                    config: config.clone(),
                    layer: Layer::User,
                });
                self.profiles.sort_by(|a, b| a.name.cmp(&b.name));
                self.selected_profile = self
                    .profiles
                    .iter()
                    .position(|row| row.name == name)
                    .unwrap_or(0);
                self.profile_edits.insert(name, Some(config));
            }
            ProfileInputPurpose::EditModel { name } => {
                let value = (!typed.is_empty()).then_some(typed);
                if let Some(row) = self.profiles.iter_mut().find(|row| row.name == name) {
                    row.config.set_model(value);
                    row.layer = Layer::User;
                    self.profile_edits.insert(name, Some(row.config.clone()));
                }
            }
            ProfileInputPurpose::EditBackend { name } => {
                let backend = if typed.is_empty() || typed.eq_ignore_ascii_case("native") {
                    Some(ProfileBackend::Native)
                } else if self.providers.iter().any(|row| row.name == typed) {
                    Some(ProfileBackend::DirectProvider {
                        provider: typed.clone(),
                    })
                } else {
                    None
                };
                let Some(backend) = backend else {
                    self.profile_input = Some(ProfileTextInput {
                        purpose: ProfileInputPurpose::EditBackend { name },
                        buffer: input.buffer,
                        error: Some(format!(
                            "`{typed}` is not `native` or a configured provider name"
                        )),
                    });
                    return;
                };
                if let Some(row) = self.profiles.iter_mut().find(|row| row.name == name) {
                    row.config.set_backend(backend);
                    row.layer = Layer::User;
                    self.profile_edits.insert(name, Some(row.config.clone()));
                }
            }
            ProfileInputPurpose::Duplicate { source } => {
                if typed.is_empty() {
                    self.profile_input = Some(ProfileTextInput {
                        purpose: ProfileInputPurpose::Duplicate { source },
                        buffer: input.buffer,
                        error: Some("a new profile needs a name".to_owned()),
                    });
                    return;
                }
                if self.profiles.iter().any(|row| row.name == typed) {
                    self.profile_input = Some(ProfileTextInput {
                        purpose: ProfileInputPurpose::Duplicate { source },
                        buffer: input.buffer,
                        error: Some(format!("a launch profile named `{typed}` already exists")),
                    });
                    return;
                }
                let Some(source_row) = self.profiles.iter().find(|row| row.name == source) else {
                    return;
                };
                let config = source_row.config.clone();
                self.profiles.push(ProfileRow {
                    name: typed.clone(),
                    config: config.clone(),
                    layer: Layer::User,
                });
                self.profiles.sort_by(|a, b| a.name.cmp(&b.name));
                self.selected_profile = self
                    .profiles
                    .iter()
                    .position(|row| row.name == typed)
                    .unwrap_or(0);
                self.profile_edits.insert(typed, Some(config));
            }
        }
    }

    // -------------------------------------------------------------
    // Routing
    // -------------------------------------------------------------

    fn start_routing_input(&mut self, purpose: RoutingInputPurpose) {
        let buffer = match purpose {
            RoutingInputPurpose::Model => match &self.routing.model {
                RoutingModelChoice::Automatic => "automatic".to_owned(),
                RoutingModelChoice::Deterministic => "deterministic".to_owned(),
                RoutingModelChoice::Pinned { provider, model } => {
                    format!("{provider}:{model}")
                }
            },
            RoutingInputPurpose::MaxLatency => self.routing.max_latency.get().to_string(),
            RoutingInputPurpose::MaxCost => format_usd(self.routing.max_cost),
            RoutingInputPurpose::PremiumReserve => self.routing.premium_reserve.get().to_string(),
            RoutingInputPurpose::FreeOrder => format_free_resource_list(&self.routing.free_order),
            RoutingInputPurpose::FreeDisabled => {
                format_free_resource_list(&self.routing.free_disabled)
            }
            RoutingInputPurpose::FreePin => self
                .routing
                .free_pin
                .as_ref()
                .map(format_free_resource_ref)
                .unwrap_or_default(),
        };
        self.routing_input = Some(RoutingTextInput {
            purpose,
            buffer,
            error: None,
        });
    }

    fn handle_routing_input_key(&mut self, key: KeyEvent) -> SettingsAction {
        match key.code {
            KeyCode::Esc => {
                self.routing_input = None;
                SettingsAction::Redraw
            }
            KeyCode::Enter => {
                self.confirm_routing_input();
                SettingsAction::Redraw
            }
            KeyCode::Backspace => {
                if let Some(input) = self.routing_input.as_mut() {
                    input.buffer.pop();
                    input.error = None;
                }
                SettingsAction::Redraw
            }
            KeyCode::Char(c) => {
                if let Some(input) = self.routing_input.as_mut() {
                    input.buffer.push(c);
                    input.error = None;
                }
                SettingsAction::Redraw
            }
            _ => SettingsAction::None,
        }
    }

    fn confirm_routing_input(&mut self) {
        let Some(input) = self.routing_input.take() else {
            return;
        };
        let typed = input.buffer.trim();
        let result = match input.purpose {
            RoutingInputPurpose::Model => self.apply_routing_model(typed),
            RoutingInputPurpose::MaxLatency => typed
                .parse::<u32>()
                .map_err(|_| "latency must be a whole number of milliseconds".to_owned())
                .and_then(|value| RouterLatencyMs::try_from(value).map_err(|err| err.to_string()))
                .map(|value| {
                    self.routing.max_latency = value;
                    self.routing.max_latency_layer = Layer::User;
                    self.routing_edit.max_latency = Some(value);
                }),
            RoutingInputPurpose::MaxCost => parse_usd_micro(typed).map(|value| {
                self.routing.max_cost = value;
                self.routing.max_cost_layer = Layer::User;
                self.routing_edit.max_cost = Some(value);
            }),
            RoutingInputPurpose::PremiumReserve => typed
                .parse::<u16>()
                .map_err(|_| "reserve must be a whole-number percentage".to_owned())
                .and_then(|value| {
                    PremiumReservePercent::try_from(value).map_err(|err| err.to_string())
                })
                .map(|value| {
                    self.routing.premium_reserve = value;
                    self.routing.premium_reserve_layer = Layer::User;
                    self.routing_edit.premium_reserve = Some(value);
                }),
            RoutingInputPurpose::FreeOrder => parse_free_resource_list(typed).map(|value| {
                self.routing.free_order = value.clone();
                self.routing.free_order_layer = Layer::User;
                self.routing_edit.free_order = Some(value);
            }),
            RoutingInputPurpose::FreeDisabled => parse_free_resource_list(typed).map(|value| {
                self.routing.free_disabled = value.clone();
                self.routing.free_disabled_layer = Layer::User;
                self.routing_edit.free_disabled = Some(value);
            }),
            RoutingInputPurpose::FreePin => {
                let pin = if typed.is_empty() {
                    Ok(None)
                } else {
                    parse_free_resource_ref(typed).map(Some)
                };
                pin.map(|value| {
                    self.routing.free_pin = value.clone();
                    self.routing.free_pin_layer = Layer::User;
                    self.routing_edit.free_pin = Some(value);
                })
            }
        };
        if let Err(error) = result {
            self.routing_input = Some(RoutingTextInput {
                purpose: input.purpose,
                buffer: input.buffer,
                error: Some(error),
            });
        }
    }

    fn apply_routing_model(&mut self, typed: &str) -> Result<(), String> {
        let choice = if typed.eq_ignore_ascii_case("automatic") {
            RoutingModelChoice::Automatic
        } else if typed.eq_ignore_ascii_case("deterministic") {
            RoutingModelChoice::Deterministic
        } else {
            let Some((provider, model)) = typed.split_once(':') else {
                return Err("use `automatic`, `deterministic`, or `provider:model`".to_owned());
            };
            let provider = provider.trim();
            let model = model.trim();
            if provider.is_empty() || model.is_empty() {
                return Err("a pinned choice needs both a provider and model".to_owned());
            }
            if !self
                .routing
                .configured_providers
                .iter()
                .any(|configured| configured == provider)
            {
                return Err(format!("`{provider}` is not a configured provider"));
            }
            RoutingModelChoice::Pinned {
                provider: provider.to_owned(),
                model: model.to_owned(),
            }
        };
        self.routing.model = choice.clone();
        self.routing.model_layer = Layer::User;
        self.routing_edit.model = Some(choice);
        Ok(())
    }

    fn handle_path_input_key(&mut self, key: KeyEvent) -> SettingsAction {
        match key.code {
            KeyCode::Esc => {
                self.path_input = None;
                SettingsAction::Redraw
            }
            KeyCode::Enter => {
                let typed = {
                    let input = self.path_input.as_ref().expect("checked above");
                    PathBuf::from(input.buffer.trim())
                };
                match exec::resolve_explicit(&typed) {
                    Ok(resolved) => {
                        let index = self.selected_harness;
                        if let Some(row) = self.harnesses.get_mut(index) {
                            let path = resolved.path().to_path_buf();
                            row.executable = Some(path.clone());
                            row.executable_layer = Some(Layer::User);
                            self.edits.entry(row.id).or_default().executable = Some(Some(path));
                        }
                        self.path_input = None;
                    }
                    Err(err) => {
                        if let Some(input) = self.path_input.as_mut() {
                            input.error = Some(err.to_string());
                        }
                    }
                }
                SettingsAction::Redraw
            }
            KeyCode::Backspace => {
                if let Some(input) = self.path_input.as_mut() {
                    input.buffer.pop();
                    input.error = None;
                }
                SettingsAction::Redraw
            }
            KeyCode::Char(c) => {
                if let Some(input) = self.path_input.as_mut() {
                    input.buffer.push(c);
                    input.error = None;
                }
                SettingsAction::Redraw
            }
            _ => SettingsAction::None,
        }
    }
}

/// Render exact micro-USD as a compact decimal dollar amount.
pub fn format_usd(value: RouterCostMicroUsd) -> String {
    let raw = value.get();
    let dollars = raw / 1_000_000;
    let fraction = raw % 1_000_000;
    format!("{dollars}.{fraction:06}")
}

fn parse_usd_micro(text: &str) -> Result<RouterCostMicroUsd, String> {
    let text = text.trim().strip_prefix('$').unwrap_or(text.trim());
    if text.is_empty() || text.starts_with('-') {
        return Err("cost must be a non-negative USD amount".to_owned());
    }
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        || fraction.len() > 6
    {
        return Err("cost must be USD with at most six decimal places".to_owned());
    }
    let whole = whole
        .parse::<u32>()
        .map_err(|_| "cost is too large".to_owned())?;
    let fraction = format!("{fraction:0<6}")
        .parse::<u32>()
        .map_err(|_| "cost must be USD with at most six decimal places".to_owned())?;
    let raw = whole
        .checked_mul(1_000_000)
        .and_then(|value| value.checked_add(fraction))
        .ok_or_else(|| "cost is too large".to_owned())?;
    RouterCostMicroUsd::try_from(raw).map_err(|err| err.to_string())
}

/// One `provider:model` field, as the Routing section's free-resource
/// editors type it. Mirrors [`SettingsState::apply_routing_model`]'s own
/// `provider:model` parsing for [`RoutingModelChoice::Pinned`], with the same
/// deliberate omission: it does not require `provider` to already be a
/// configured provider, because a free-resource preference — unlike a
/// classifier pin — is allowed to name a provider not yet configured, and
/// [`crate::config::RoutingConfig::free_resource_pin`]'s own doc is where
/// that degrades visibly rather than failing.
fn parse_free_resource_ref(typed: &str) -> Result<FreeResourceRef, String> {
    let Some((provider, model)) = typed.split_once(':') else {
        return Err(format!("`{typed}` must be `provider:model`"));
    };
    let provider = provider.trim();
    let model = model.trim();
    if provider.is_empty() || model.is_empty() {
        return Err(format!(
            "`{typed}` needs both a provider and a model, as `provider:model`"
        ));
    }
    Ok(FreeResourceRef::new(provider, model))
}

/// A comma-separated list of `provider:model` fields, in the order typed —
/// the shape both [`RoutingInputPurpose::FreeOrder`] and
/// [`RoutingInputPurpose::FreeDisabled`] share.
fn parse_free_resource_list(typed: &str) -> Result<Vec<FreeResourceRef>, String> {
    typed
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(parse_free_resource_ref)
        .collect()
}

fn format_free_resource_ref(entry: &FreeResourceRef) -> String {
    format!("{}:{}", entry.provider(), entry.model())
}

fn format_free_resource_list(entries: &[FreeResourceRef]) -> String {
    entries
        .iter()
        .map(format_free_resource_ref)
        .collect::<Vec<_>>()
        .join(",")
}

/// `Ctrl-]` — the one chord that returns to control mode from session mode.
///
/// See the design note for why this chord: it is what `telnet` has used for
/// decades, no ordinary key produces it, and it is exactly one chord rather
/// than a prefix that would double the latency of escaping a runaway session.
///
/// **It has more than one spelling, and all of them must be accepted.** The
/// chord is really the byte `0x1D` (ASCII group separator), and Crossterm's
/// Unix parser decodes the control range `0x1C..=0x1F` arithmetically, as
/// `Char((c - 0x1C + b'4'))` — so a real terminal's `Ctrl-]` arrives as
/// `Ctrl` + `'5'`, never as `Ctrl` + `']'`.
///
/// Matching too narrowly is not a cosmetic bug: it leaves the user in session
/// mode with no way back, which is precisely the failure the single-chord
/// escape exists to prevent. It survived unit testing because a synthetic
/// `KeyEvent::new(KeyCode::Char(']'), CONTROL)` is not what any terminal
/// sends; only driving the real binary through a real pseudo-terminal caught
/// it — twice, once per platform.
///
/// # Windows delivers a character chosen by the keyboard layout
///
/// The `']'` spelling was written for Windows, where Crossterm reads console
/// records rather than a byte stream. That is right only by accident of
/// whoever wrote it having a US keyboard. Measured on the ARM64 CI machine,
/// whose input locale is `0x04070409` — US English on a **German** physical
/// layout — the byte `0x1D` arrives from ConPTY as this console record:
///
/// ```text
/// vk=0xBB (VK_OEM_PLUS)  scan=0x1B  uChar=0x001D  ctrl=LEFT_CTRL_PRESSED
/// ```
///
/// `uChar` is the right answer and Crossterm throws it away: its Windows
/// parser treats any `uChar` in `0x00..=0x1f` as "some chord produced a
/// control code, ask the layout which character that key really types" and
/// calls `ToUnicodeEx` on the *virtual key*. Scan code `0x1B` is the physical
/// key right of `P`; a US layout types `']'` there and a German one types
/// `'+'`. So Glasshouse is handed `Ctrl` + `'+'` on this machine and
/// `Ctrl` + `']'` on the machine the original spelling came from, for the
/// same keypress.
///
/// **No fixed set of characters can be correct**, because the set is the set
/// of layouts. So on Windows the test is the modifier and the *shape* of the
/// character rather than its identity: any non-alphanumeric character with
/// Control. Its failure mode is a spurious escape — a user typing some other
/// `Ctrl`+punctuation chord lands back in control mode — which is the
/// direction to fail in, because the other direction traps them with no way
/// out. Nothing is lost by it either: [`encode`] has never translated
/// `Ctrl`+punctuation to a control byte, so those chords reached the harness
/// as bare punctuation, which is not what the user pressed.
///
/// **`AltGr` is excluded, and that is measured too.** Windows reports `AltGr`
/// as `CONTROL | ALT`, so on a layout where `']'` itself needs `AltGr` — as
/// it does on the German one — typing a literal `']'` arrived as
/// `Ctrl` + `Alt` + `']'` and matched the old rule. Typing a bracket into a
/// harness would have thrown the user out of the session. The real chord
/// never carries `ALT`: the record above has `LEFT_CTRL_PRESSED` alone.
///
/// The durable fix is upstream of here, in whatever reads the console
/// records: `uChar` already carries `0x1D` exactly. See the report for the
/// shape that would take.
fn is_session_escape(key: &KeyEvent) -> bool {
    if !key.modifiers.contains(KeyModifiers::CONTROL) {
        return false;
    }
    // See the doc comment: `AltGr` is `CONTROL | ALT` on Windows, and the
    // chord itself never carries `ALT`, so this only ever excludes a
    // character the user meant to type.
    #[cfg(windows)]
    if key.modifiers.contains(KeyModifiers::ALT) {
        return false;
    }
    #[cfg(windows)]
    if matches!(key.code, KeyCode::Char(c) if !c.is_ascii_alphanumeric()) {
        return true;
    }
    matches!(key.code, KeyCode::Char(']') | KeyCode::Char('5'))
}

/// Turn one key event into the bytes a PTY expects.
///
/// `None` for a key with no sensible byte encoding (a bare modifier, a
/// function key Glasshouse does not translate) — session mode simply has
/// nothing to send for it.
fn encode(key: KeyEvent) -> Option<Vec<u8>> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        // Ctrl-A..Ctrl-Z as the control byte 0x01..0x1a. Checked before the
        // plain `Char` arm below so a control chord never encodes as its
        // literal character instead.
        KeyCode::Char(c) if ctrl && c.is_ascii_alphabetic() => {
            Some(vec![c.to_ascii_lowercase() as u8 - b'a' + 1])
        }
        KeyCode::Char(c) => {
            let mut buf = [0u8; 4];
            Some(c.encode_utf8(&mut buf).as_bytes().to_vec())
        }
        KeyCode::Enter => Some(vec![b'\r']),
        KeyCode::Backspace => Some(vec![0x7f]),
        KeyCode::Tab => Some(vec![b'\t']),
        KeyCode::Esc => Some(vec![0x1b]),
        KeyCode::Up => Some(b"\x1b[A".to_vec()),
        KeyCode::Down => Some(b"\x1b[B".to_vec()),
        KeyCode::Right => Some(b"\x1b[C".to_vec()),
        KeyCode::Left => Some(b"\x1b[D".to_vec()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{SessionId, SessionLifecycle, SessionPresentation, SessionRole};

    fn record(id: &str, harness: &str) -> SessionRecord {
        SessionRecord {
            id: SessionId::new(id),
            project_id: "project".to_owned(),
            harness: harness.to_owned(),
            native_session_id: None,
            role: SessionRole::Normal,
            lifecycle: SessionLifecycle::Running,
            presentation: SessionPresentation::Embedded,
            created_at: 1_000,
            last_activity_at: 1_000,
            launch_profile: None,
            backend_resource: None,
            model: None,
            pairing_class: None,
            protocol: None,
            response_profile: None,
            response_mechanism: None,
            display_name: None,
            purpose: None,
            source_session_id: None,
            observed_compactions: None,
        }
    }

    fn state_with(count: usize) -> ShellState {
        let sessions = (0..count)
            .map(|i| record(&format!("id-{i}"), "claude-code"))
            .collect();
        ShellState::new("glasshouse", "/projects/glasshouse", "0.1.0", sessions)
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn the_active_project_root_is_always_available_to_the_view() {
        let state = state_with(1);
        assert_eq!(state.project_root(), Path::new("/projects/glasshouse"));
        assert_eq!(state.project_name(), "glasshouse");
    }

    #[test]
    fn tab_moves_to_the_next_session_and_wraps() {
        let mut state = state_with(3);
        assert_eq!(state.selected_index(), 0);
        assert_eq!(state.handle_key(press(KeyCode::Tab)), Action::Redraw);
        assert_eq!(state.selected_index(), 1);
        state.handle_key(press(KeyCode::Tab));
        assert_eq!(state.selected_index(), 2);
        state.handle_key(press(KeyCode::Tab));
        assert_eq!(
            state.selected_index(),
            0,
            "the last entry wraps to the first"
        );
    }

    #[test]
    fn shift_tab_moves_to_the_previous_session_and_wraps() {
        let mut state = state_with(3);
        assert_eq!(state.handle_key(press(KeyCode::BackTab)), Action::Redraw);
        assert_eq!(
            state.selected_index(),
            2,
            "the first entry wraps to the last"
        );
        state.handle_key(press(KeyCode::BackTab));
        assert_eq!(state.selected_index(), 1);
    }

    #[test]
    fn arrow_keys_navigate_the_same_way_as_tab() {
        let mut state = state_with(3);
        state.handle_key(press(KeyCode::Right));
        assert_eq!(state.selected_index(), 1);
        state.handle_key(press(KeyCode::Left));
        assert_eq!(state.selected_index(), 0);
    }

    /// Navigation presents a different session; it must never look like
    /// anything happened to the sessions themselves.
    #[test]
    fn navigating_changes_only_which_session_is_presented() {
        let mut state = state_with(3);
        let before: Vec<_> = state.sessions().to_vec();

        state.handle_key(press(KeyCode::Tab));
        state.handle_key(press(KeyCode::Tab));
        state.handle_key(press(KeyCode::BackTab));

        assert_eq!(
            state.sessions(),
            before.as_slice(),
            "no session was altered"
        );
        assert_eq!(state.selected_index(), 1);
    }

    /// With nothing to move between, the key must explain itself rather than
    /// appear broken — but it must not move the selection.
    #[test]
    fn navigating_with_fewer_than_two_sessions_explains_itself() {
        for count in [0, 1] {
            let mut state = state_with(count);
            assert_eq!(state.handle_key(press(KeyCode::Tab)), Action::Redraw);
            assert_eq!(state.selected_index(), 0, "{count} sessions: nothing moved");
            let status = state.status().unwrap_or_else(|| {
                panic!("{count} sessions: the key must leave a note explaining itself")
            });
            assert!(!status.is_empty());
        }
        assert!(
            state_with(0).sessions().is_empty(),
            "the empty case must not invent a session"
        );
    }

    /// A note explains the key just pressed, so the next key clears it instead
    /// of leaving stale text sitting under a new action.
    #[test]
    fn a_status_note_is_cleared_by_the_next_keystroke() {
        let mut state = state_with(1);
        state.handle_key(press(KeyCode::Tab));
        assert!(state.status().is_some());

        assert_eq!(
            state.handle_key(press(KeyCode::Char('z'))),
            Action::Redraw,
            "clearing a note is a visible change"
        );
        assert!(state.status().is_none(), "the note must not linger");

        assert_eq!(
            state.handle_key(press(KeyCode::Char('z'))),
            Action::None,
            "with no note to clear, an unknown key costs nothing"
        );
    }

    #[test]
    fn an_empty_project_has_no_active_session_and_does_not_panic() {
        let mut state = state_with(0);
        assert!(state.active_session().is_none());
        state.handle_key(press(KeyCode::Tab));
        state.handle_key(press(KeyCode::BackTab));
        assert!(state.active_session().is_none());
    }

    #[test]
    fn o_opens_the_session_overview_from_the_keyboard() {
        let mut state = state_with(2);
        assert_eq!(state.overlay(), None);
        assert_eq!(state.handle_key(press(KeyCode::Char('o'))), Action::Redraw);
        assert_eq!(state.overlay(), Some(Overlay::Overview));
        assert_eq!(
            state.handle_key(press(KeyCode::Char('o'))),
            Action::Redraw,
            "the same key closes it again"
        );
        assert_eq!(state.overlay(), None);
    }

    /// The capability that separates an overlay from a modal takeover: leaving
    /// it returns to the session that was already active, and the session is
    /// untouched.
    #[test]
    fn leaving_an_overlay_returns_to_the_active_session_without_ending_it() {
        let mut state = state_with(3);
        state.handle_key(press(KeyCode::Tab));
        let active = state.active_session().expect("a session").clone();

        state.handle_key(press(KeyCode::Char('o')));
        assert_eq!(state.overlay(), Some(Overlay::Overview));
        assert_eq!(
            state.active_session(),
            Some(&active),
            "opening an overlay must not change which session is active"
        );

        assert_eq!(state.handle_key(press(KeyCode::Esc)), Action::Redraw);
        assert_eq!(state.overlay(), None, "Escape leaves the overlay");
        assert_eq!(
            state.active_session(),
            Some(&active),
            "the same session is still presented, unchanged"
        );
        assert_eq!(state.sessions().len(), 3, "no session was closed");
    }

    /// Escape has to mean two things, and which one depends on whether an
    /// overlay is open. Getting this backwards would make Escape close
    /// Glasshouse from inside the overview.
    #[test]
    fn escape_leaves_an_overlay_first_and_only_then_leaves_glasshouse() {
        let mut state = state_with(2);
        state.handle_key(press(KeyCode::Char('o')));
        assert_eq!(state.handle_key(press(KeyCode::Esc)), Action::Redraw);
        assert_eq!(state.handle_key(press(KeyCode::Esc)), Action::Quit);
    }

    #[test]
    fn quit_keys_are_q_escape_and_ctrl_c() {
        for key in [
            press(KeyCode::Char('q')),
            press(KeyCode::Esc),
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        ] {
            let mut state = state_with(2);
            assert_eq!(state.handle_key(key), Action::Quit, "{key:?} should quit");
        }
    }

    #[test]
    fn an_unrecognized_key_costs_nothing() {
        let mut state = state_with(2);
        assert_eq!(state.handle_key(press(KeyCode::Char('z'))), Action::None);
        // `Enter` is no longer unbound: it now enters session mode (see the
        // tests below). `F(1)` stands in as a key with genuinely nothing
        // bound to it in control mode.
        assert_eq!(state.handle_key(press(KeyCode::F(1))), Action::None);
    }

    /// A refresh reorders the list, because sessions sort by last activity.
    /// Following the identifier rather than the index is what stops the user
    /// being moved to a different session by a background update.
    #[test]
    fn a_refresh_keeps_the_same_session_presented_even_when_the_order_changes() {
        let mut state = ShellState::new(
            "p",
            "/p",
            "0.1.0",
            vec![record("a", "claude-code"), record("b", "codex")],
        );
        state.handle_key(press(KeyCode::Tab));
        assert_eq!(state.active_session().unwrap().id.as_str(), "b");

        // "b" is now first, as a fresh `list()` ordered by activity would give.
        state.refresh(vec![record("b", "codex"), record("a", "claude-code")]);
        assert_eq!(
            state.active_session().unwrap().id.as_str(),
            "b",
            "the presented session must follow its identifier, not its position"
        );
        assert_eq!(state.selected_index(), 0);
    }

    #[test]
    fn a_refresh_that_removes_the_active_session_falls_back_to_the_first() {
        let mut state = ShellState::new(
            "p",
            "/p",
            "0.1.0",
            vec![record("a", "claude-code"), record("b", "codex")],
        );
        state.handle_key(press(KeyCode::Tab));
        state.refresh(vec![record("a", "claude-code")]);
        assert_eq!(state.active_session().unwrap().id.as_str(), "a");
    }

    /// Redrawing on every poll would make the interface flicker and burn CPU
    /// on an idle project, so an unchanged list must report no change.
    #[test]
    fn a_refresh_with_identical_sessions_does_not_ask_for_a_redraw() {
        let mut state = state_with(2);
        let same: Vec<_> = state.sessions().to_vec();
        assert_eq!(state.refresh(same), Action::None);
    }

    // -----------------------------------------------------------------
    // Session modes — `.agent-runtime/design-shell-session-modes.md`.
    // -----------------------------------------------------------------

    /// Invariant 1: in session mode, `q` reaches the harness and does not
    /// quit Glasshouse.
    #[test]
    fn in_session_mode_q_reaches_the_harness_and_does_not_quit() {
        let mut state = state_with(1);
        assert_eq!(state.handle_key(press(KeyCode::Enter)), Action::Redraw);
        assert_eq!(state.mode(), Mode::Session);

        assert_eq!(
            state.handle_key(press(KeyCode::Char('q'))),
            Action::Forward(b"q".to_vec())
        );
        assert_eq!(
            state.mode(),
            Mode::Session,
            "q must not quit Glasshouse in session mode"
        );
    }

    /// Invariant 2: in session mode, Ctrl-C reaches the harness as a byte,
    /// and does not quit. This is the entire reason `RawModeGuard` exists
    /// rather than `TerminalGuard` — Ctrl-C belongs to the harness here.
    #[test]
    fn in_session_mode_ctrl_c_reaches_the_harness_as_a_byte_and_does_not_quit() {
        let mut state = state_with(1);
        state.handle_key(press(KeyCode::Enter));
        assert_eq!(state.mode(), Mode::Session);

        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(state.handle_key(ctrl_c), Action::Forward(vec![0x03]));
        assert_eq!(
            state.mode(),
            Mode::Session,
            "Ctrl-C belongs to the harness in session mode, not to Glasshouse"
        );
    }

    /// Invariant 3: `Ctrl-]` returns to control mode from session mode, and
    /// is never forwarded — it is intercepted before `encode` ever runs.
    #[test]
    fn ctrl_bracket_returns_to_control_mode_and_is_never_forwarded() {
        let mut state = state_with(1);
        state.handle_key(press(KeyCode::Enter));
        assert_eq!(state.mode(), Mode::Session);

        let escape = KeyEvent::new(KeyCode::Char(']'), KeyModifiers::CONTROL);
        assert_eq!(
            state.handle_key(escape),
            Action::Redraw,
            "the escape chord must not be forwarded as an Action::Forward"
        );
        assert_eq!(state.mode(), Mode::Control);
    }

    /// Invariant 4: entering and leaving session mode never touches any
    /// process. `ShellState` has no field for one — it holds only the
    /// viewport text the run loop hands it — so this proves that guarantee
    /// against a real running process rather than by inspection of the type
    /// alone: same pid, still running, and its output (stood in for here by
    /// the viewport text, since `ShellState` does not read a scrollback
    /// itself) is untouched by the mode switch either way.
    #[test]
    fn entering_and_leaving_session_mode_never_touches_a_real_process() {
        let mut child = spawn_long_lived();
        let pid_before = child.id();
        assert!(
            child.try_wait().expect("try_wait").is_none(),
            "the process must start out running"
        );

        let mut state = state_with(1);
        let growing = ViewportGrid::new(1, 1, vec![("g".to_owned(), Style::default())], None);
        state.set_viewport_grid(growing.clone());
        state.handle_key(press(KeyCode::Enter));
        assert_eq!(state.mode(), Mode::Session);
        assert_eq!(state.viewport_grid(), &growing);

        let grown = ViewportGrid::new(1, 1, vec![("m".to_owned(), Style::default())], None);
        state.set_viewport_grid(grown.clone());
        let escape = KeyEvent::new(KeyCode::Char(']'), KeyModifiers::CONTROL);
        state.handle_key(escape);
        assert_eq!(state.mode(), Mode::Control);
        assert_eq!(state.viewport_grid(), &grown);

        assert_eq!(
            child.id(),
            pid_before,
            "the pid must be unaffected by a mode switch"
        );
        assert!(
            child.try_wait().expect("try_wait").is_none(),
            "the process must still be running after the mode switch"
        );

        child.kill().expect("kill");
        child.wait().expect("reap");
    }

    /// Invariant 5: with nothing focused, session mode cannot be entered —
    /// there is nowhere to send the keys.
    #[test]
    fn session_mode_cannot_be_entered_with_nothing_focused() {
        let mut state = state_with(0);
        assert!(state.active_session().is_none());

        assert_eq!(state.handle_key(press(KeyCode::Enter)), Action::Redraw);
        assert_eq!(
            state.mode(),
            Mode::Control,
            "there is nowhere to send session-mode keystrokes"
        );
        assert!(
            state.status().is_some(),
            "the key must explain why nothing happened"
        );
    }

    /// Invariant 6: a session exiting while in session mode drops back to
    /// control mode rather than leaving keystrokes going nowhere.
    #[test]
    fn a_session_exiting_in_session_mode_drops_back_to_control_mode() {
        let mut state = state_with(1);
        state.handle_key(press(KeyCode::Enter));
        assert_eq!(state.mode(), Mode::Session);

        assert_eq!(state.session_exited(), Action::Redraw);
        assert_eq!(state.mode(), Mode::Control);

        // Idempotent: already in control mode, so there is nothing left to do.
        assert_eq!(state.session_exited(), Action::None);
    }

    #[test]
    fn session_mode_is_entered_with_enter_or_i() {
        for key in [press(KeyCode::Enter), press(KeyCode::Char('i'))] {
            let mut state = state_with(1);
            assert_eq!(state.handle_key(key), Action::Redraw);
            assert_eq!(
                state.mode(),
                Mode::Session,
                "{key:?} should enter session mode"
            );
        }
    }

    /// Session mode and an overlay must never coexist — see the design
    /// note's "Overlays are control-mode only."
    #[test]
    fn entering_session_mode_closes_any_open_overlay() {
        let mut state = state_with(2);
        state.handle_key(press(KeyCode::Char('o')));
        assert_eq!(state.overlay(), Some(Overlay::Overview));

        state.handle_key(press(KeyCode::Enter));
        assert_eq!(state.mode(), Mode::Session);
        assert_eq!(
            state.overlay(),
            None,
            "session mode and an overlay must never coexist"
        );
    }

    #[test]
    fn n_starts_a_new_session_from_control_mode() {
        let mut state = state_with(1);
        assert_eq!(
            state.handle_key(press(KeyCode::Char('n'))),
            Action::StartSession
        );
        assert_eq!(
            state.mode(),
            Mode::Control,
            "requesting a session does not itself change mode"
        );
    }

    #[test]
    fn encode_translates_the_documented_keys_to_their_bytes() {
        assert_eq!(encode(press(KeyCode::Char('a'))), Some(b"a".to_vec()));
        assert_eq!(encode(press(KeyCode::Enter)), Some(vec![b'\r']));
        assert_eq!(encode(press(KeyCode::Backspace)), Some(vec![0x7f]));
        assert_eq!(encode(press(KeyCode::Tab)), Some(vec![b'\t']));
        assert_eq!(encode(press(KeyCode::Esc)), Some(vec![0x1b]));
        assert_eq!(encode(press(KeyCode::Up)), Some(b"\x1b[A".to_vec()));
        assert_eq!(encode(press(KeyCode::Down)), Some(b"\x1b[B".to_vec()));
        assert_eq!(encode(press(KeyCode::Right)), Some(b"\x1b[C".to_vec()));
        assert_eq!(encode(press(KeyCode::Left)), Some(b"\x1b[D".to_vec()));

        let ctrl_a = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL);
        assert_eq!(encode(ctrl_a), Some(vec![0x01]), "Ctrl-A is 0x01");
        let ctrl_z = KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL);
        assert_eq!(encode(ctrl_z), Some(vec![0x1a]), "Ctrl-Z is 0x1a");

        // A multi-byte character must survive as UTF-8, not be truncated.
        assert_eq!(
            encode(press(KeyCode::Char('é'))),
            Some("é".as_bytes().to_vec())
        );
    }

    #[test]
    fn encode_has_nothing_to_send_for_an_unmapped_key() {
        assert_eq!(encode(press(KeyCode::F(1))), None);
    }

    /// A real, long-lived child process for [`entering_and_leaving_session_mode_never_touches_a_real_process`].
    /// Not a PTY session — nothing here is testing the harness launch seam,
    /// only that `ShellState`'s own methods cannot reach a process at all.
    fn spawn_long_lived() -> std::process::Child {
        if cfg!(windows) {
            std::process::Command::new("ping")
                .args(["-n", "5", "127.0.0.1"])
                .stdout(std::process::Stdio::null())
                .spawn()
                .expect("spawn ping")
        } else {
            std::process::Command::new("sleep")
                .arg("5")
                .spawn()
                .expect("spawn sleep")
        }
    }
}

#[cfg(test)]
mod escape_chord_tests {
    use super::*;

    /// Both spellings of the escape chord must work, because a terminal sends
    /// the byte `0x1D` and the two platforms' parsers name it differently.
    ///
    /// Crossterm's Unix parser turns `0x1C..=0x1F` into `Char('4'..='7')` with
    /// CONTROL, so `Ctrl-]` arrives as `Ctrl-5`; Windows reads virtual key
    /// codes and gives `Char(']')`. Accepting only one leaves users of the
    /// other platform trapped in session mode.
    #[test]
    fn the_escape_chord_is_recognised_in_both_platform_spellings() {
        for code in [KeyCode::Char(']'), KeyCode::Char('5')] {
            assert!(
                is_session_escape(&KeyEvent::new(code, KeyModifiers::CONTROL)),
                "{code:?} with CONTROL must escape session mode"
            );
        }
    }

    /// The byte Crossterm's Unix parser produces for `Ctrl-]`, derived the same
    /// way Crossterm derives it, so this test fails if that mapping is ever
    /// misremembered.
    #[test]
    fn the_unix_spelling_matches_how_crossterm_decodes_the_byte() {
        const CTRL_RIGHT_BRACKET: u8 = 0x1D;
        let decoded = (CTRL_RIGHT_BRACKET - 0x1C + b'4') as char;
        assert_eq!(decoded, '5');
        assert!(is_session_escape(&KeyEvent::new(
            KeyCode::Char(decoded),
            KeyModifiers::CONTROL
        )));
    }

    /// Without CONTROL these are ordinary characters a harness must receive.
    #[test]
    fn the_bare_characters_are_not_an_escape() {
        for code in [KeyCode::Char(']'), KeyCode::Char('5')] {
            assert!(!is_session_escape(&KeyEvent::new(code, KeyModifiers::NONE)));
        }
    }

    /// The spelling the CI machine actually delivers, and the reason the
    /// Windows arm cannot be a list of characters.
    ///
    /// `'+'` is what a German physical layout puts on the key a US layout
    /// calls `']'`, and Crossterm reports the layout's character rather than
    /// the `0x1D` the console record carries. `'#'`, `'ü'` and `'$'` are the
    /// same key on three more layouts; none of them could have been guessed,
    /// and the rule has to hold for all of them.
    #[cfg(windows)]
    #[test]
    fn the_windows_spelling_is_whatever_the_layout_puts_on_that_key() {
        for code in [
            KeyCode::Char('+'),
            KeyCode::Char('#'),
            KeyCode::Char('ü'),
            KeyCode::Char('$'),
        ] {
            assert!(
                is_session_escape(&KeyEvent::new(code, KeyModifiers::CONTROL)),
                "{code:?} with CONTROL must escape session mode on Windows"
            );
        }
    }

    /// A bracket typed with `AltGr` is a bracket, not an escape.
    ///
    /// Windows reports `AltGr` as `CONTROL | ALT`, and on every layout where
    /// `']'` needs `AltGr` this is how a user types one into a harness. The
    /// chord itself never carries `ALT`.
    #[cfg(windows)]
    #[test]
    fn a_character_typed_with_altgr_is_not_an_escape() {
        for code in [KeyCode::Char(']'), KeyCode::Char('+'), KeyCode::Char('}')] {
            assert!(
                !is_session_escape(&KeyEvent::new(
                    code,
                    KeyModifiers::CONTROL | KeyModifiers::ALT
                )),
                "{code:?} with AltGr is a character the harness must receive"
            );
        }
    }

    /// The widened Windows rule must not eat the chords a harness lives on.
    ///
    /// `Ctrl-C` cancelling a turn is the one that would hurt, and it is the
    /// reason the rule tests for a non-alphanumeric character rather than for
    /// "not `']'`".
    #[cfg(windows)]
    #[test]
    fn a_control_letter_or_digit_still_reaches_the_harness() {
        for code in [
            KeyCode::Char('c'),
            KeyCode::Char('C'),
            KeyCode::Char('d'),
            KeyCode::Char('z'),
            KeyCode::Char('0'),
        ] {
            assert!(
                !is_session_escape(&KeyEvent::new(code, KeyModifiers::CONTROL)),
                "{code:?} with CONTROL belongs to the harness"
            );
        }
    }
}

#[cfg(test)]
mod native_input_tests {
    use super::*;
    use crate::session::{
        SessionId, SessionLifecycle, SessionPresentation, SessionRecord, SessionRole,
    };

    fn session_state() -> ShellState {
        let record = SessionRecord {
            id: SessionId::new("live"),
            project_id: "p".to_owned(),
            harness: "claude-code".to_owned(),
            native_session_id: None,
            role: SessionRole::Normal,
            lifecycle: SessionLifecycle::Running,
            presentation: SessionPresentation::Embedded,
            created_at: 0,
            last_activity_at: 0,
            launch_profile: None,
            backend_resource: None,
            model: None,
            pairing_class: None,
            protocol: None,
            response_profile: None,
            response_mechanism: None,
            display_name: None,
            purpose: None,
            source_session_id: None,
            observed_compactions: None,
        };
        let mut state = ShellState::new("p", "/p", "0.1.0", vec![record]);
        state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(state.mode(), Mode::Session);
        state
    }

    /// A harness's own slash commands must reach it verbatim. Glasshouse never
    /// interprets `/`, so `/compact` or `/model` is typed at the harness the
    /// same way it would be with no Glasshouse in between.
    #[test]
    fn a_slash_command_passes_straight_through_to_the_harness() {
        let mut state = session_state();
        let mut sent = Vec::new();
        for key in "/compact".chars() {
            match state.handle_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE)) {
                Action::Forward(bytes) => sent.extend(bytes),
                other => panic!("`{key}` was intercepted as {other:?} instead of forwarded"),
            }
        }
        assert_eq!(String::from_utf8(sent).unwrap(), "/compact");
        assert_eq!(state.mode(), Mode::Session, "typing must not change mode");
    }

    /// There is no Glasshouse composer: input is forwarded key by key as the
    /// harness's own interface expects, so its editing, history, and completion
    /// all keep working. Every key Glasshouse binds in control mode must be
    /// forwarded here instead.
    #[test]
    fn keys_glasshouse_binds_elsewhere_belong_to_the_harness_in_session_mode() {
        for (code, expected) in [
            (KeyCode::Char('q'), vec![b'q']),
            (KeyCode::Char('n'), vec![b'n']),
            (KeyCode::Char('o'), vec![b'o']),
            (KeyCode::Char('i'), vec![b'i']),
            (KeyCode::Tab, vec![b'\t']),
            (KeyCode::Esc, vec![0x1b]),
            (KeyCode::Enter, vec![b'\r']),
            (KeyCode::Backspace, vec![0x7f]),
            (KeyCode::Up, b"\x1b[A".to_vec()),
        ] {
            let mut state = session_state();
            assert_eq!(
                state.handle_key(KeyEvent::new(code, KeyModifiers::NONE)),
                Action::Forward(expected),
                "{code:?} must reach the harness in session mode"
            );
            assert_eq!(state.mode(), Mode::Session);
        }
    }

    /// The escape captures input for Glasshouse only until the user hands it
    /// back, which is what "temporarily, without permanently stealing" means:
    /// the session is untouched and re-entering resumes forwarding.
    #[test]
    fn the_escape_captures_input_only_until_it_is_handed_back() {
        let mut state = session_state();
        let before = state.sessions().to_vec();

        state.handle_key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::CONTROL));
        assert_eq!(state.mode(), Mode::Control);
        assert_eq!(
            state.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE)),
            Action::Redraw,
            "control mode's own bindings work again"
        );
        state.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            state.mode(),
            Mode::Session,
            "input goes back to the harness"
        );
        assert_eq!(
            state.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)),
            Action::Forward(vec![b'q'])
        );
        assert_eq!(
            state.sessions(),
            before.as_slice(),
            "no session was touched"
        );
    }
}

#[cfg(test)]
mod settings_tests {
    use super::*;
    use crate::session::{SessionId, SessionLifecycle, SessionPresentation, SessionRole};

    fn record(id: &str) -> SessionRecord {
        SessionRecord {
            id: SessionId::new(id),
            project_id: "p".to_owned(),
            harness: "claude-code".to_owned(),
            native_session_id: None,
            role: SessionRole::Normal,
            lifecycle: SessionLifecycle::Running,
            presentation: SessionPresentation::Embedded,
            created_at: 0,
            last_activity_at: 0,
            launch_profile: None,
            backend_resource: None,
            model: None,
            pairing_class: None,
            protocol: None,
            response_profile: None,
            response_mechanism: None,
            display_name: None,
            purpose: None,
            source_session_id: None,
            observed_compactions: None,
        }
    }

    fn state_with_a_session() -> ShellState {
        ShellState::new("p", "/p", "0.1.0", vec![record("only")])
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn sample_harness_rows() -> Vec<HarnessRow> {
        vec![
            HarnessRow {
                id: IntegrationId::ClaudeCode,
                detected: true,
                enabled: true,
                enabled_layer: Layer::User,
                executable: Some(PathBuf::from("/usr/bin/claude")),
                executable_layer: Some(Layer::User),
            },
            HarnessRow {
                id: IntegrationId::Codex,
                detected: false,
                enabled: false,
                enabled_layer: Layer::Default,
                executable: None,
                executable_layer: None,
            },
        ]
    }

    fn sample_integration_rows() -> Vec<IntegrationRow> {
        vec![IntegrationRow {
            id: IntegrationId::Ollama,
            detected: false,
            status: IntegrationStatus::NotFound,
        }]
    }

    fn sample_provider_rows() -> Vec<ProviderRow> {
        let mut config = ProviderConfig::new("openrouter");
        config.set_base_url(Some("https://mirror.example.com/v1".to_owned()));
        vec![ProviderRow::new("my-router", config, Layer::User)]
    }

    fn sample_profile_rows() -> Vec<ProfileRow> {
        vec![ProfileRow {
            name: "fast".to_owned(),
            config: ProfileConfig::new(IntegrationId::ClaudeCode),
            layer: Layer::User,
        }]
    }

    fn type_text(state: &mut ShellState, text: &str) {
        for c in text.chars() {
            state.handle_key(press(KeyCode::Char(c)));
        }
    }

    // Invariant 1 (design note): opening settings does not disturb the
    // session, and leaving it returns to the mode the user was in.
    #[test]
    fn opening_and_closing_settings_leaves_mode_and_session_untouched() {
        let mut state = state_with_a_session();
        let before = state.sessions().to_vec();

        assert_eq!(
            state.handle_key(press(KeyCode::Char('s'))),
            Action::OpenSettings
        );
        assert_eq!(
            state.overlay(),
            None,
            "opening settings needs data only the run loop can gather"
        );

        state.open_settings(
            sample_harness_rows(),
            sample_integration_rows(),
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(state.overlay(), Some(Overlay::Settings));
        assert_eq!(state.mode(), Mode::Control);

        assert_eq!(state.handle_key(press(KeyCode::Esc)), Action::Redraw);
        assert_eq!(state.overlay(), None);
        assert_eq!(
            state.mode(),
            Mode::Control,
            "closing settings must not disturb the mode"
        );
        assert_eq!(
            state.sessions(),
            before.as_slice(),
            "no session was touched"
        );
    }

    /// `s` is an ordinary character to a harness in session mode: it must
    /// not open settings out from under it.
    #[test]
    fn s_is_forwarded_to_the_harness_in_session_mode_not_intercepted() {
        let mut state = state_with_a_session();
        state.handle_key(press(KeyCode::Enter));
        assert_eq!(state.mode(), Mode::Session);

        assert_eq!(
            state.handle_key(press(KeyCode::Char('s'))),
            Action::Forward(b"s".to_vec())
        );
        assert_eq!(state.overlay(), None);
    }

    #[test]
    fn tab_moves_between_sections_and_up_down_moves_within_one() {
        let mut state = state_with_a_session();
        state.open_settings(
            sample_harness_rows(),
            sample_integration_rows(),
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(
            state.settings().unwrap().section(),
            SettingsSection::Harnesses
        );

        state.handle_key(press(KeyCode::Tab));
        assert_eq!(
            state.settings().unwrap().section(),
            SettingsSection::Integrations
        );
        state.handle_key(press(KeyCode::Left));
        assert_eq!(
            state.settings().unwrap().section(),
            SettingsSection::Harnesses
        );

        assert_eq!(state.settings().unwrap().selected_harness(), 0);
        state.handle_key(press(KeyCode::Down));
        assert_eq!(state.settings().unwrap().selected_harness(), 1);
        state.handle_key(press(KeyCode::Down));
        assert_eq!(
            state.settings().unwrap().selected_harness(),
            1,
            "selection clamps at the last row"
        );
        state.handle_key(press(KeyCode::Up));
        assert_eq!(state.settings().unwrap().selected_harness(), 0);
    }

    #[test]
    fn space_toggles_enabled_on_the_selected_harness_and_stages_the_user_layer() {
        let mut state = state_with_a_session();
        state.open_settings(sample_harness_rows(), Vec::new(), Vec::new(), Vec::new());
        let before = state.settings().unwrap().harnesses()[0].enabled;

        state.handle_key(press(KeyCode::Char(' ')));

        let row = &state.settings().unwrap().harnesses()[0];
        assert_eq!(row.enabled, !before);
        assert_eq!(row.enabled_layer, Layer::User);
        assert_eq!(state.settings_edits().len(), 1);
        assert_eq!(state.settings_edits()[0].id, IntegrationId::ClaudeCode);
        assert_eq!(state.settings_edits()[0].enabled, Some(!before));
    }

    /// "Esc in the editor cancels without changing anything."
    #[test]
    fn esc_in_the_path_editor_cancels_without_changing_anything() {
        let mut state = state_with_a_session();
        state.open_settings(sample_harness_rows(), Vec::new(), Vec::new(), Vec::new());
        let before = state.settings().unwrap().harnesses()[0].clone();

        state.handle_key(press(KeyCode::Enter));
        assert!(state.settings().unwrap().path_input().is_some());
        for c in "/bogus/path".chars() {
            state.handle_key(press(KeyCode::Char(c)));
        }

        assert_eq!(state.handle_key(press(KeyCode::Esc)), Action::Redraw);
        assert!(state.settings().unwrap().path_input().is_none());
        assert_eq!(
            state.settings().unwrap().harnesses()[0],
            before,
            "cancelling the editor must not change the row"
        );
        assert!(state.settings_edits().is_empty());
        assert_eq!(
            state.overlay(),
            Some(Overlay::Settings),
            "Esc in the editor only closes the editor, not all of settings"
        );
    }

    #[test]
    fn a_valid_explicit_path_is_recorded_with_the_user_layer() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("my-claude");
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let mut state = state_with_a_session();
        state.open_settings(sample_harness_rows(), Vec::new(), Vec::new(), Vec::new());
        state.handle_key(press(KeyCode::Enter));
        for c in exe.to_str().unwrap().chars() {
            state.handle_key(press(KeyCode::Char(c)));
        }
        state.handle_key(press(KeyCode::Enter));

        assert!(state.settings().unwrap().path_input().is_none());
        let row = &state.settings().unwrap().harnesses()[0];
        assert_eq!(
            std::fs::canonicalize(row.executable.as_ref().unwrap()).unwrap(),
            std::fs::canonicalize(&exe).unwrap()
        );
        assert_eq!(row.executable_layer, Some(Layer::User));
        assert_eq!(state.settings_edits().len(), 1);
    }

    #[test]
    fn an_invalid_explicit_path_surfaces_an_error_and_keeps_the_editor_open() {
        let mut state = state_with_a_session();
        state.open_settings(sample_harness_rows(), Vec::new(), Vec::new(), Vec::new());
        state.handle_key(press(KeyCode::Enter));
        for c in "/definitely/not/a/real/executable".chars() {
            state.handle_key(press(KeyCode::Char(c)));
        }
        assert_eq!(state.handle_key(press(KeyCode::Enter)), Action::Redraw);

        let input = state.settings().unwrap().path_input().expect("still open");
        assert!(input.error.is_some());
        assert!(state.settings_edits().is_empty());
    }

    #[test]
    fn lowercase_w_signals_an_immediate_user_level_save() {
        let mut state = state_with_a_session();
        state.open_settings(sample_harness_rows(), Vec::new(), Vec::new(), Vec::new());
        state.handle_key(press(KeyCode::Char(' ')));
        assert_eq!(
            state.handle_key(press(KeyCode::Char('w'))),
            Action::SaveUserSettings
        );
    }

    /// "It must first show a confirmation ... Only an explicit `y` (or
    /// Enter on the confirm) proceeds; `Esc`/`n` cancels."
    #[test]
    fn shift_w_requires_a_separate_explicit_confirmation() {
        let mut state = state_with_a_session();
        state.open_settings(sample_harness_rows(), Vec::new(), Vec::new(), Vec::new());

        assert_eq!(state.handle_key(press(KeyCode::Char('W'))), Action::Redraw);
        assert!(state.settings().unwrap().confirming_project_write());

        // An unrelated key does not proceed.
        assert_eq!(state.handle_key(press(KeyCode::Char('z'))), Action::None);
        assert!(state.settings().unwrap().confirming_project_write());

        assert_eq!(
            state.handle_key(press(KeyCode::Char('y'))),
            Action::SaveProjectSettings
        );
    }

    #[test]
    fn enter_on_the_confirmation_also_proceeds() {
        let mut state = state_with_a_session();
        state.open_settings(sample_harness_rows(), Vec::new(), Vec::new(), Vec::new());
        state.handle_key(press(KeyCode::Char('W')));
        assert_eq!(
            state.handle_key(press(KeyCode::Enter)),
            Action::SaveProjectSettings
        );
    }

    #[test]
    fn esc_or_n_cancels_the_confirmation_without_leaving_settings() {
        for cancel in [press(KeyCode::Esc), press(KeyCode::Char('n'))] {
            let mut state = state_with_a_session();
            state.open_settings(sample_harness_rows(), Vec::new(), Vec::new(), Vec::new());
            state.handle_key(press(KeyCode::Char('W')));

            assert_eq!(state.handle_key(cancel), Action::Redraw);
            assert!(!state.settings().unwrap().confirming_project_write());
            assert_eq!(state.overlay(), Some(Overlay::Settings));
        }
    }

    #[test]
    fn refresh_settings_clears_pending_edits_and_keeps_the_cursor() {
        let mut state = state_with_a_session();
        state.open_settings(sample_harness_rows(), Vec::new(), Vec::new(), Vec::new());
        state.handle_key(press(KeyCode::Down));
        state.handle_key(press(KeyCode::Char(' ')));
        assert!(!state.settings_edits().is_empty());
        assert_eq!(state.settings().unwrap().selected_harness(), 1);

        state.refresh_settings(sample_harness_rows(), Vec::new(), Vec::new(), Vec::new());
        assert!(state.settings_edits().is_empty());
        assert_eq!(
            state.settings().unwrap().selected_harness(),
            1,
            "the cursor position is preserved across a refresh"
        );
    }

    // -----------------------------------------------------------------
    // Phase 2D: Providers and Launch Profiles.
    // -----------------------------------------------------------------

    fn to_providers(mut state: ShellState) -> ShellState {
        state.handle_key(press(KeyCode::Tab));
        state.handle_key(press(KeyCode::Tab));
        assert_eq!(
            state.settings().unwrap().section(),
            SettingsSection::Providers
        );
        state
    }

    fn to_launch_profiles(mut state: ShellState) -> ShellState {
        state.handle_key(press(KeyCode::Tab));
        state.handle_key(press(KeyCode::Tab));
        state.handle_key(press(KeyCode::Tab));
        assert_eq!(
            state.settings().unwrap().section(),
            SettingsSection::LaunchProfiles
        );
        state
    }

    /// Acceptance 1: an empty Providers section must not panic on
    /// navigation or any of its keys.
    #[test]
    fn an_empty_providers_section_does_not_panic_on_navigation_or_actions() {
        let mut state = state_with_a_session();
        state.open_settings(Vec::new(), Vec::new(), Vec::new(), Vec::new());
        state = to_providers(state);
        assert!(state.settings().unwrap().providers().is_empty());

        for key in [
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Char(' '),
            KeyCode::Char('d'),
            KeyCode::Char('t'),
        ] {
            state.handle_key(press(key));
        }
        assert!(state.settings().unwrap().providers().is_empty());
    }

    /// Acceptance 2 (the staging half — see `shell::tests` for the
    /// save/reload half): adding a provider from a built-in template stages
    /// it at the user layer.
    #[test]
    fn adding_a_provider_from_a_template_stages_it_at_the_user_layer() {
        let mut state = state_with_a_session();
        state.open_settings(Vec::new(), Vec::new(), Vec::new(), Vec::new());
        state = to_providers(state);

        assert_eq!(state.handle_key(press(KeyCode::Char('a'))), Action::Redraw);
        assert!(state.settings().unwrap().provider_input().is_some());
        type_text(&mut state, "my-router");
        state.handle_key(press(KeyCode::Enter));
        assert!(
            state.settings().unwrap().provider_input().is_some(),
            "the wizard's second step (template) must still be open"
        );
        type_text(&mut state, "openrouter");
        state.handle_key(press(KeyCode::Enter));

        assert!(state.settings().unwrap().provider_input().is_none());
        let providers = state.settings().unwrap().providers();
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].name, "my-router");
        assert_eq!(providers[0].config.template(), "openrouter");
        assert_eq!(providers[0].layer, Layer::User);

        let edits = state.settings_provider_edits();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].name, "my-router");
        assert!(edits[0].upsert.is_some());
    }

    /// An unknown template is refused with a message naming it, mirroring
    /// how an unknown harness is refused for a launch profile.
    #[test]
    fn adding_a_provider_with_an_unknown_template_is_refused_with_a_message_naming_it() {
        let mut state = state_with_a_session();
        state.open_settings(Vec::new(), Vec::new(), Vec::new(), Vec::new());
        state = to_providers(state);

        state.handle_key(press(KeyCode::Char('a')));
        type_text(&mut state, "my-router");
        state.handle_key(press(KeyCode::Enter));
        type_text(&mut state, "not-a-real-template");
        assert_eq!(state.handle_key(press(KeyCode::Enter)), Action::Redraw);

        let input = state
            .settings()
            .unwrap()
            .provider_input()
            .expect("still open on error");
        let error = input.error.expect("an error must be shown");
        assert!(
            error.contains("not-a-real-template"),
            "the error must name the template: {error}"
        );
        assert!(state.settings().unwrap().providers().is_empty());
    }

    /// Acceptance 3 (the staging half): editing a provider's base URL
    /// persists in the row and the staged edit.
    #[test]
    fn editing_a_providers_base_url_persists_in_the_row_and_the_staged_edit() {
        let mut state = state_with_a_session();
        state.open_settings(Vec::new(), Vec::new(), sample_provider_rows(), Vec::new());
        state = to_providers(state);

        let backspaces = {
            assert_eq!(state.handle_key(press(KeyCode::Char('e'))), Action::Redraw);
            let input = state
                .settings()
                .unwrap()
                .provider_input()
                .expect("base url editor open");
            assert_eq!(input.buffer, "https://mirror.example.com/v1");
            input.buffer.chars().count()
        };
        for _ in 0..backspaces {
            state.handle_key(press(KeyCode::Backspace));
        }
        type_text(&mut state, "https://new.example.com/v1");
        state.handle_key(press(KeyCode::Enter));

        let row = &state.settings().unwrap().providers()[0];
        assert_eq!(row.config.base_url(), Some("https://new.example.com/v1"));
        assert_eq!(row.layer, Layer::User);

        let edits = state.settings_provider_edits();
        assert_eq!(edits.len(), 1);
        assert_eq!(
            edits[0].upsert.as_ref().unwrap().base_url(),
            Some("https://new.example.com/v1")
        );
    }

    /// Acceptance 4: removing a provider removes it; disabling one keeps its
    /// configuration intact and reversible. Both halves are asserted.
    #[test]
    fn removing_a_provider_removes_it_and_disabling_keeps_it_reversible() {
        let mut state = state_with_a_session();
        state.open_settings(Vec::new(), Vec::new(), sample_provider_rows(), Vec::new());
        state = to_providers(state);

        // Disable half.
        let before = state.settings().unwrap().providers()[0].config.clone();
        assert!(before.enabled());
        state.handle_key(press(KeyCode::Char(' ')));
        let disabled = state.settings().unwrap().providers()[0].config.clone();
        assert!(!disabled.enabled(), "the provider must be disabled");
        assert_eq!(
            disabled.base_url(),
            before.base_url(),
            "disabling must not touch other fields"
        );
        // Reversible without retyping anything.
        state.handle_key(press(KeyCode::Char(' ')));
        let re_enabled = state.settings().unwrap().providers()[0].config.clone();
        assert!(re_enabled.enabled());
        assert_eq!(re_enabled.base_url(), before.base_url());

        // Remove half.
        assert_eq!(state.handle_key(press(KeyCode::Char('d'))), Action::Redraw);
        assert!(
            state.settings().unwrap().providers().is_empty(),
            "removing must actually remove the row"
        );
        let edits = state.settings_provider_edits();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].name, "my-router");
        assert!(
            edits[0].upsert.is_none(),
            "a removal edit carries no config to upsert"
        );
    }

    /// Acceptance 5 (the staging half): duplicating a launch profile
    /// produces an independent copy — editing the copy must not change the
    /// original.
    #[test]
    fn duplicating_a_profile_produces_an_independent_copy() {
        let mut state = state_with_a_session();
        state.open_settings(Vec::new(), Vec::new(), Vec::new(), sample_profile_rows());
        state = to_launch_profiles(state);

        assert_eq!(state.handle_key(press(KeyCode::Char('u'))), Action::Redraw);
        type_text(&mut state, "fast-copy");
        state.handle_key(press(KeyCode::Enter));

        assert_eq!(state.settings().unwrap().profiles().len(), 2);
        let copy_index = state
            .settings()
            .unwrap()
            .profiles()
            .iter()
            .position(|row| row.name == "fast-copy")
            .expect("the copy exists");
        assert_eq!(state.settings().unwrap().selected_profile(), copy_index);

        // Edit the copy's model.
        assert_eq!(state.handle_key(press(KeyCode::Char('e'))), Action::Redraw);
        type_text(&mut state, "claude-opus");
        state.handle_key(press(KeyCode::Enter));

        let original = state
            .settings()
            .unwrap()
            .profiles()
            .iter()
            .find(|row| row.name == "fast")
            .unwrap();
        let copy = state
            .settings()
            .unwrap()
            .profiles()
            .iter()
            .find(|row| row.name == "fast-copy")
            .unwrap();
        assert_eq!(
            original.config.model(),
            None,
            "editing the copy must not change the original"
        );
        assert_eq!(copy.config.model(), Some("claude-opus"));
    }

    /// Acceptance 6: creating a profile that names an unknown harness is
    /// refused with a message naming the harness.
    #[test]
    fn creating_a_profile_naming_an_unknown_harness_is_refused_with_a_message_naming_it() {
        let mut state = state_with_a_session();
        state.open_settings(Vec::new(), Vec::new(), Vec::new(), Vec::new());
        state = to_launch_profiles(state);

        state.handle_key(press(KeyCode::Char('a')));
        type_text(&mut state, "custom");
        state.handle_key(press(KeyCode::Enter));
        type_text(&mut state, "not-a-real-harness");
        assert_eq!(state.handle_key(press(KeyCode::Enter)), Action::Redraw);

        let input = state
            .settings()
            .unwrap()
            .profile_input()
            .expect("still open on error");
        let error = input.error.expect("an error must be shown");
        assert!(
            error.contains("not-a-real-harness"),
            "the error must name the harness: {error}"
        );
        assert!(
            state.settings().unwrap().profiles().is_empty(),
            "no profile must have been created"
        );
    }

    /// Found running the real binary: `cmux`, Ollama and llama.cpp are real
    /// integration slugs, but none is a launchable coding harness — naming
    /// one for a launch profile must be refused exactly like a truly unknown
    /// slug, not silently accepted because it happens to appear in
    /// `IntegrationId::ALL`.
    #[test]
    fn creating_a_profile_naming_a_non_harness_integration_is_also_refused() {
        for slug in ["cmux", "ollama", "llama-cpp"] {
            let mut state = state_with_a_session();
            state.open_settings(Vec::new(), Vec::new(), Vec::new(), Vec::new());
            state = to_launch_profiles(state);

            state.handle_key(press(KeyCode::Char('a')));
            type_text(&mut state, "custom");
            state.handle_key(press(KeyCode::Enter));
            type_text(&mut state, slug);
            state.handle_key(press(KeyCode::Enter));

            let input = state
                .settings()
                .unwrap()
                .profile_input()
                .unwrap_or_else(|| panic!("`{slug}` must be refused, not accepted"));
            assert!(input.error.is_some(), "`{slug}` must be refused");
            assert!(state.settings().unwrap().profiles().is_empty());
        }
    }

    /// Removing a launch profile stages its removal, matching
    /// [`removing_a_provider_removes_it_and_disabling_keeps_it_reversible`].
    #[test]
    fn removing_a_profile_stages_its_removal() {
        let mut state = state_with_a_session();
        state.open_settings(Vec::new(), Vec::new(), Vec::new(), sample_profile_rows());
        state = to_launch_profiles(state);

        assert_eq!(state.handle_key(press(KeyCode::Char('d'))), Action::Redraw);
        assert!(state.settings().unwrap().profiles().is_empty());
        let edits = state.settings_profile_edits();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].name, "fast");
        assert!(edits[0].upsert.is_none());
    }

    /// Disabling a launch profile is reversible without retyping, matching
    /// the provider half of acceptance 4.
    #[test]
    fn disabling_a_profile_keeps_it_reversible() {
        let mut state = state_with_a_session();
        state.open_settings(Vec::new(), Vec::new(), Vec::new(), sample_profile_rows());
        state = to_launch_profiles(state);

        assert!(state.settings().unwrap().profiles()[0].config.enabled());
        state.handle_key(press(KeyCode::Char(' ')));
        assert!(!state.settings().unwrap().profiles()[0].config.enabled());
        state.handle_key(press(KeyCode::Char(' ')));
        assert!(state.settings().unwrap().profiles()[0].config.enabled());
    }

    /// Acceptance 9: Line 5's check reports failure without disabling the
    /// provider. Uses a uniquely named, never-set variable rather than any
    /// built-in template's real one, so the test cannot pass by accident
    /// because of something set in the ambient environment.
    #[test]
    fn a_failed_reachability_check_reports_failure_without_disabling_the_provider() {
        const VAR: &str = "GLASSHOUSE_SETTINGS_TEST_ONLY_MISSING_CRED_VAR";
        // SAFETY: `VAR` is unique to this test and is not set by anything
        // else; removed again below regardless of how the test proceeds.
        unsafe {
            std::env::remove_var(VAR);
        }

        let mut config = ProviderConfig::new("openrouter");
        config.set_credential_env(vec![VAR.to_owned()]);
        let rows = vec![ProviderRow::new("unset-cred", config, Layer::User)];

        let mut state = state_with_a_session();
        state.open_settings(Vec::new(), Vec::new(), rows, Vec::new());
        state = to_providers(state);

        assert_eq!(state.handle_key(press(KeyCode::Char('t'))), Action::Redraw);
        let (name, outcome) = state
            .settings()
            .unwrap()
            .provider_test_result()
            .expect("a test ran");
        assert_eq!(name, "unset-cred");
        match outcome {
            ReachabilityCheck::Failed(reason) => assert!(
                reason.contains(VAR),
                "the failure must name the missing variable: {reason}"
            ),
            other => panic!("expected a failure, got {other:?}"),
        }
        assert!(
            state.settings().unwrap().providers()[0].config.enabled(),
            "a failed test must not disable the provider"
        );
    }

    /// A provider whose preconditions hold now produces a *request*, not a
    /// verdict. `t` hands the run loop something to do, the row says a
    /// request is running, and the intent names the exact URL that will be
    /// asked for.
    ///
    /// The `openrouter` template's model-list endpoint is verified, so the
    /// planned target is that endpoint rather than the bare base URL — one
    /// request that exercises the base URL, TLS, the credential and a real
    /// route.
    #[test]
    fn a_passing_precondition_check_plans_a_real_request_and_says_it_is_in_flight() {
        const VAR: &str = "GLASSHOUSE_SETTINGS_TEST_ONLY_PRESENT_CRED_VAR";
        // SAFETY: `VAR` is unique to this test and is removed again below.
        unsafe {
            std::env::set_var(VAR, "sk-fabricated-test-value-not-a-real-credential");
        }

        let mut config = ProviderConfig::new("openrouter");
        config.set_credential_env(vec![VAR.to_owned()]);
        let rows = vec![ProviderRow::new("set-cred", config, Layer::User)];

        let mut state = state_with_a_session();
        state.open_settings(Vec::new(), Vec::new(), rows, Vec::new());
        state = to_providers(state);
        let action = state.handle_key(press(KeyCode::Char('t')));

        unsafe {
            std::env::remove_var(VAR);
        }

        assert_eq!(
            action,
            Action::RunProviderProbe,
            "a passing precondition check must hand the run loop a request to make"
        );

        let (_, outcome) = state
            .settings()
            .unwrap()
            .provider_test_result()
            .expect("a test ran");
        match outcome {
            ReachabilityCheck::InFlight {
                protocol,
                base_url,
                endpoint,
            } => {
                assert_eq!(*protocol, "openai-chat");
                assert_eq!(*base_url, "https://openrouter.ai/api/v1");
                assert_eq!(*endpoint, "https://openrouter.ai/api/v1/models");
            }
            other => panic!("expected a request in flight, got {other:?}"),
        }

        assert_eq!(
            state.settings().unwrap().providers()[0].activity,
            Some(ProbeKind::Connectivity),
            "the row must say a request is running, or a busy interface looks frozen"
        );

        let intent = state
            .take_provider_probe_intent()
            .expect("the run loop is given exactly one request");
        assert_eq!(intent.provider, "set-cred");
        assert_eq!(intent.kind, ProbeKind::Connectivity);
        assert_eq!(intent.target, ProbeTarget::ModelList);
        assert_eq!(
            intent.secret_refs,
            vec![SecretRef::Environment {
                var: VAR.to_owned()
            }]
        );
        assert!(
            state.take_provider_probe_intent().is_none(),
            "an intent is taken, so one keystroke can only ever open one socket"
        );
    }

    /// **Acceptance test 6.** Phase 9D line 2 says "when the provider
    /// exposes model discovery", so a provider that does not must produce a
    /// plain sentence — not an error, and not a control that silently does
    /// nothing.
    ///
    /// Both negative states are asserted, because they are different facts
    /// and call for different next actions. `ollama`'s model list is
    /// `Unverified`: nobody has established it, and the sentence has to say
    /// so rather than claiming the service lacks one.
    #[test]
    fn a_provider_with_no_established_model_discovery_says_so_and_is_not_an_error() {
        let rows = vec![ProviderRow::new(
            "local",
            ProviderConfig::new("ollama"),
            Layer::User,
        )];
        let mut state = state_with_a_session();
        state.open_settings(Vec::new(), Vec::new(), rows, Vec::new());
        state = to_providers(state);

        let action = state.handle_key(press(KeyCode::Char('m')));
        assert_eq!(
            action,
            Action::Redraw,
            "no request may be planned for a provider with no established model list"
        );
        assert!(
            state.take_provider_probe_intent().is_none(),
            "nothing may be sent to a path nobody established"
        );

        let (name, refresh) = state
            .settings()
            .unwrap()
            .provider_models_result()
            .expect("the user must be told something");
        assert_eq!(name, "local");
        match refresh {
            ModelRefresh::NotOffered(reason) => {
                assert!(
                    reason.contains("has been established"),
                    "the sentence must say nobody established one, not that none exists: \
                     {reason}"
                );
                assert!(
                    reason.contains("local"),
                    "and it must name the provider: {reason}"
                );
            }
            other => panic!("expected a plain explanation, not an error: {other:?}"),
        }
        assert!(
            state.settings().unwrap().providers()[0].activity.is_none(),
            "nothing is in flight, so nothing may claim to be"
        );
    }

    /// A provider whose model list *is* established plans a refresh. The
    /// counterpart to the test above, so "not offered" cannot be passing
    /// because `m` does nothing at all.
    #[test]
    fn a_provider_with_an_established_model_list_plans_a_refresh_of_it() {
        let rows = vec![ProviderRow::new(
            "router",
            ProviderConfig::new("litellm"),
            Layer::User,
        )];
        let mut state = state_with_a_session();
        state.open_settings(Vec::new(), Vec::new(), rows, Vec::new());
        state = to_providers(state);

        assert_eq!(
            state.handle_key(press(KeyCode::Char('m'))),
            Action::RunProviderProbe
        );
        let intent = state.take_provider_probe_intent().expect("a request");
        assert_eq!(intent.kind, ProbeKind::ModelRefresh);
        assert_eq!(intent.target, ProbeTarget::ModelList);
        assert_eq!(probe_endpoint(&intent), "http://0.0.0.0:4000/models");
    }

    /// **Phase 9D line 1's own words: "before enabling it for routing".**
    ///
    /// Testing reports. It does not decide. A provider that was enabled stays
    /// enabled through a timeout, and a provider that was disabled stays
    /// disabled through a success — neither outcome touches the flag, and
    /// this asserts both directions because only checking one would let a
    /// "helpful" auto-disable through.
    #[test]
    fn a_connectivity_result_never_enables_or_disables_the_provider_it_is_about() {
        for (starts_enabled, outcome) in [
            (true, ProbeOutcome::TimedOut { waited_ms: 10_000 }),
            (true, ProbeOutcome::Rejected { status: 401 }),
            (
                true,
                ProbeOutcome::Unreachable {
                    reason: "the connection was refused".to_owned(),
                },
            ),
            (false, ProbeOutcome::Reached { status: 200 }),
        ] {
            let mut config = ProviderConfig::new("openrouter");
            config.set_enabled(starts_enabled);
            let rows = vec![ProviderRow::new("router", config, Layer::User)];
            let mut state = state_with_a_session();
            state.open_settings(Vec::new(), Vec::new(), rows, Vec::new());

            state.apply_provider_probe_result(ProviderProbeResult {
                provider: "router".to_owned(),
                notice: ProviderNotice::Reachability(ReachabilityCheck::Answered {
                    protocol: "openai-chat",
                    base_url: "https://openrouter.ai/api/v1".to_owned(),
                    endpoint: "https://openrouter.ai/api/v1/models".to_owned(),
                    outcome: outcome.clone(),
                }),
                catalogue: None,
            });

            assert_eq!(
                state.settings().unwrap().providers()[0].config.enabled(),
                starts_enabled,
                "a {outcome:?} result changed whether the provider was enabled; testing \
                 reports and the user decides"
            );
        }
    }

    /// **Acceptance test 4, at the state level.** A refresh replaces the
    /// cached list and moves the timestamp.
    #[test]
    fn a_manual_refresh_replaces_the_cached_list_and_moves_the_timestamp() {
        let rows = vec![
            ProviderRow::new("router", ProviderConfig::new("openrouter"), Layer::User).with_models(
                Some(ModelCatalogue::new(
                    "router",
                    "https://openrouter.ai/api/v1",
                    "https://openrouter.ai/api/v1/models",
                    1_000,
                    vec![
                        crate::provider::cache::ModelEntry::new("old/one"),
                        crate::provider::cache::ModelEntry::new("old/two"),
                    ],
                )),
            ),
        ];
        let mut state = state_with_a_session();
        state.open_settings(Vec::new(), Vec::new(), rows, Vec::new());

        let refreshed = ModelCatalogue::new(
            "router",
            "https://openrouter.ai/api/v1",
            "https://openrouter.ai/api/v1/models",
            2_000,
            vec![crate::provider::cache::ModelEntry::new("new/one")],
        );
        state.apply_provider_probe_result(ProviderProbeResult {
            provider: "router".to_owned(),
            notice: ProviderNotice::Models(ModelRefresh::Refreshed {
                count: 1,
                fetched_at: 2_000,
                endpoint: "https://openrouter.ai/api/v1/models".to_owned(),
            }),
            catalogue: Some(refreshed),
        });

        let row = &state.settings().unwrap().providers()[0];
        let models = row.models.as_ref().expect("a catalogue");
        assert_eq!(models.fetched_at(), 2_000, "the timestamp must move");
        assert_eq!(models.len(), 1);
        assert!(
            !models.models().iter().any(|m| m.id().starts_with("old/")),
            "a refresh replaces the list; it must never append to it"
        );
    }

    /// A request on the wire must be visible on the row, and must survive
    /// every keystroke that clears the banner beneath it.
    ///
    /// This is the state half of "a frozen screen and a slow screen look
    /// identical, and only one of them is acceptable". The banner is
    /// deliberately transient — that is what stops it shadowing a field
    /// editor — so if the in-flight marker lived there too, scrolling the
    /// list would make a running request invisible.
    #[test]
    fn an_in_flight_request_survives_the_keystrokes_that_clear_its_banner() {
        const VAR: &str = "GLASSHOUSE_SETTINGS_TEST_ONLY_INFLIGHT_CRED_VAR";
        // SAFETY: `VAR` is unique to this test and is removed again below.
        unsafe {
            std::env::set_var(VAR, "sk-fabricated-test-value-not-a-real-credential");
        }
        let mut config = ProviderConfig::new("openrouter");
        config.set_credential_env(vec![VAR.to_owned()]);
        let rows = vec![
            ProviderRow::new("first", config.clone(), Layer::User),
            ProviderRow::new("second", config, Layer::User),
        ];
        let mut state = state_with_a_session();
        state.open_settings(Vec::new(), Vec::new(), rows, Vec::new());
        state = to_providers(state);

        assert_eq!(
            state.handle_key(press(KeyCode::Char('t'))),
            Action::RunProviderProbe
        );
        unsafe {
            std::env::remove_var(VAR);
        }
        assert!(state.provider_probe_in_flight());

        // The interface still answers keys — this is the responsiveness
        // claim, made against the same state a real keystroke would reach.
        assert_eq!(state.handle_key(press(KeyCode::Down)), Action::Redraw);
        assert_eq!(state.settings().unwrap().selected_provider(), 1);
        assert!(
            state.settings().unwrap().provider_test_result().is_none(),
            "the banner clears on the next key, exactly as it always did"
        );
        assert!(
            state.provider_probe_in_flight(),
            "but the request is still running, and the interface must still say so"
        );
        assert_eq!(
            state.settings().unwrap().providers()[0].activity,
            Some(ProbeKind::Connectivity),
            "and it says so on the row the request is about, not on the selected one"
        );
    }

    /// A second press while one request is running must not open a second
    /// socket, and must say why rather than doing nothing — a key that
    /// silently does nothing is indistinguishable from a frozen screen.
    #[test]
    fn a_second_test_while_one_is_running_is_refused_out_loud() {
        const VAR: &str = "GLASSHOUSE_SETTINGS_TEST_ONLY_DOUBLE_PRESS_VAR";
        // SAFETY: `VAR` is unique to this test and is removed again below.
        unsafe {
            std::env::set_var(VAR, "sk-fabricated-test-value-not-a-real-credential");
        }
        let mut config = ProviderConfig::new("openrouter");
        config.set_credential_env(vec![VAR.to_owned()]);
        let rows = vec![ProviderRow::new("router", config, Layer::User)];
        let mut state = state_with_a_session();
        state.open_settings(Vec::new(), Vec::new(), rows, Vec::new());
        state = to_providers(state);

        assert_eq!(
            state.handle_key(press(KeyCode::Char('t'))),
            Action::RunProviderProbe
        );
        let _first = state.take_provider_probe_intent().expect("one request");

        let action = state.handle_key(press(KeyCode::Char('t')));
        unsafe {
            std::env::remove_var(VAR);
        }
        assert_eq!(action, Action::Redraw, "the second press plans nothing");
        assert!(
            state.take_provider_probe_intent().is_none(),
            "a second socket must not be opened while the first is outstanding"
        );
        match state.settings().unwrap().provider_test_result() {
            Some((_, ReachabilityCheck::Failed(reason))) => assert!(
                reason.contains("already running"),
                "the refusal must say why: {reason}"
            ),
            other => panic!("expected an out-loud refusal, got {other:?}"),
        }
    }

    /// **Acceptance test 7, at the state boundary.** A probe intent is names
    /// only. Asserted with `!contains` rather than `assert_eq!`, because a
    /// failing equality assertion on secret material prints both sides.
    #[test]
    fn a_probe_intent_and_its_debug_carry_names_only_and_never_a_credential() {
        const VAR: &str = "GLASSHOUSE_SETTINGS_TEST_ONLY_LEAK_CHECK_VAR";
        const VALUE: &str = "sk-planted-state-credential-9d";
        // SAFETY: `VAR` is unique to this test and is removed again below.
        unsafe {
            std::env::set_var(VAR, VALUE);
        }
        let mut config = ProviderConfig::new("openrouter");
        config.set_credential_env(vec![VAR.to_owned()]);
        let rows = vec![ProviderRow::new("router", config, Layer::User)];
        let mut state = state_with_a_session();
        state.open_settings(Vec::new(), Vec::new(), rows, Vec::new());
        state = to_providers(state);
        state.handle_key(press(KeyCode::Char('t')));
        let intent = state.take_provider_probe_intent().expect("a request");
        unsafe {
            std::env::remove_var(VAR);
        }

        for rendered in [
            format!("{intent:?}"),
            format!("{:?}", state.settings().unwrap().providers()),
            format!("{:?}", state.settings().unwrap().provider_test_result()),
        ] {
            assert!(
                !rendered.contains(VALUE),
                "a credential value reached a Debug rendering"
            );
        }
        assert!(
            format!("{intent:?}").contains(VAR),
            "the variable NAME is not a secret and is what makes the intent readable"
        );
    }

    /// A probe answering after Settings has been closed is dropped quietly
    /// rather than reopening anything or costing a frame.
    #[test]
    fn a_result_arriving_after_settings_closed_changes_nothing() {
        let mut state = state_with_a_session();
        assert_eq!(
            state.apply_provider_probe_result(ProviderProbeResult {
                provider: "router".to_owned(),
                notice: ProviderNotice::Models(ModelRefresh::Failed("late".to_owned())),
                catalogue: None,
            }),
            Action::None
        );
        assert!(state.settings().is_none());
    }

    /// Found by asking what happens to a request whose provider the user
    /// deletes while it is in flight: the row is gone, so there is nothing
    /// to clear, and the banner still reports what happened to the request
    /// they started.
    #[test]
    fn a_result_for_a_provider_that_has_since_been_deleted_still_reports() {
        let rows = vec![ProviderRow::new(
            "router",
            ProviderConfig::new("openrouter"),
            Layer::User,
        )];
        let mut state = state_with_a_session();
        state.open_settings(Vec::new(), Vec::new(), rows, Vec::new());

        assert_eq!(
            state.apply_provider_probe_result(ProviderProbeResult {
                provider: "already-deleted".to_owned(),
                notice: ProviderNotice::Models(ModelRefresh::Failed("gone".to_owned())),
                catalogue: None,
            }),
            Action::Redraw
        );
        assert!(
            state
                .settings()
                .unwrap()
                .provider_models_result()
                .is_some_and(|(name, _)| name == "already-deleted")
        );
    }

    /// Found running the real binary: a reachability-check result must not
    /// permanently shadow the bottom panel. Any other key clears it, and an
    /// active Launch-Profiles input takes priority over it even if it did
    /// not — see `SettingsState::handle_key`'s clearing line and
    /// `render_settings`'s panel-priority comment in `view.rs`.
    #[test]
    fn a_stale_reachability_result_does_not_shadow_a_later_profile_input() {
        let mut state = state_with_a_session();
        state.open_settings(
            Vec::new(),
            Vec::new(),
            sample_provider_rows(),
            sample_profile_rows(),
        );
        state = to_providers(state);
        state.handle_key(press(KeyCode::Char('t')));
        assert!(state.settings().unwrap().provider_test_result().is_some());

        // Move to Launch Profiles and open the "add" wizard.
        state.handle_key(press(KeyCode::Tab));
        assert_eq!(
            state.settings().unwrap().section(),
            SettingsSection::LaunchProfiles
        );
        assert!(
            state.settings().unwrap().provider_test_result().is_none(),
            "switching sections must clear the stale banner"
        );
        state.handle_key(press(KeyCode::Char('a')));
        assert!(state.settings().unwrap().profile_input().is_some());
        assert!(state.settings().unwrap().provider_test_result().is_none());
    }

    // --- storing and deleting a provider credential (Phase 9E) ----------

    /// A provider row with a credential variable, so `s` has a name to file
    /// a stored credential under.
    fn credential_provider_rows() -> Vec<ProviderRow> {
        let mut config = ProviderConfig::new("openrouter");
        config
            .set_base_url(Some("https://mirror.example.com/v1".to_owned()))
            .set_credential_env(vec!["MY_ROUTER_KEY".to_owned()]);
        vec![ProviderRow::new("my-router", config, Layer::User)]
    }

    fn settings_with_credential_provider() -> ShellState {
        let mut state = ShellState::new("glasshouse", "/work", "0.1.0", Vec::new());
        state.open_settings(
            Vec::new(),
            Vec::new(),
            credential_provider_rows(),
            Vec::new(),
        );
        to_providers(state)
    }

    /// **Acceptance 5 for this module.** A credential typed into Settings is
    /// masked on the way out to the renderer and redacted in every `Debug`
    /// that could reach a log or a panic message. Asserted with
    /// `!contains`, never `assert_eq!` on the secret material — a failing
    /// `assert_eq!` prints both sides.
    #[test]
    fn a_typed_credential_is_masked_for_the_view_and_redacted_in_every_debug() {
        const VALUE: &str = "sk-typed-into-settings-0123456789abcdef";

        let mut state = settings_with_credential_provider();
        state.handle_key(press(KeyCode::Char('s')));
        type_text(&mut state, VALUE);

        let view = state.settings().unwrap().provider_input().unwrap();
        assert!(
            !view.buffer.contains(VALUE),
            "the typed credential reached the view: {}",
            view.buffer
        );
        assert_eq!(
            view.buffer,
            "*".repeat(VALUE.chars().count()),
            "a credential field is masked character for character"
        );
        assert!(
            view.label.contains("my-router"),
            "the field must say which provider it is for: {}",
            view.label
        );

        let rendered = format!("{:?}", state.settings().unwrap());
        assert!(
            !rendered.contains(VALUE),
            "a credential reached a Debug rendering:\n{rendered}"
        );
        assert!(
            rendered.contains(crate::secret::REDACTED),
            "the buffer must render as the same marker `Secret` uses:\n{rendered}"
        );

        // ... and a field that is NOT a credential is still shown, so the
        // masking is the purpose's doing rather than a blanket rule that
        // would make every editor unusable.
        state.handle_key(press(KeyCode::Esc));
        state.handle_key(press(KeyCode::Char('e')));
        type_text(&mut state, "https://example.invalid/v1");
        let view = state.settings().unwrap().provider_input().unwrap();
        assert!(
            view.buffer.contains("example.invalid"),
            "got {}",
            view.buffer
        );
    }

    /// Enter on a credential field does not apply it here — writing to a
    /// keychain is I/O this module does not hold — it escalates, and the
    /// value leaves the overlay exactly once.
    #[test]
    fn enter_on_a_credential_field_hands_the_value_to_the_run_loop_once() {
        const VALUE: &str = "sk-handed-to-the-run-loop-0123456789abcd";

        let mut state = settings_with_credential_provider();
        state.handle_key(press(KeyCode::Char('s')));
        type_text(&mut state, VALUE);
        assert_eq!(
            state.handle_key(press(KeyCode::Enter)),
            Action::StoreProviderCredential
        );

        let taken = state.take_provider_credential_entry();
        assert_eq!(
            taken,
            Some(("my-router".to_owned(), VALUE.to_owned())),
            "the run loop gets the provider and the value"
        );
        // Taken means taken: a second call finds nothing, and the overlay
        // no longer holds the value.
        assert_eq!(state.take_provider_credential_entry(), None);
        assert!(state.settings().unwrap().provider_input().is_none());
        let rendered = format!("{:?}", state.settings().unwrap());
        assert!(!rendered.contains(VALUE), "got {rendered}");
    }

    /// An empty field is refused with a message rather than storing an empty
    /// credential, and Esc still leaves everything unchanged.
    #[test]
    fn an_empty_credential_field_is_refused_and_esc_changes_nothing() {
        let mut state = settings_with_credential_provider();
        state.handle_key(press(KeyCode::Char('s')));
        assert_eq!(state.handle_key(press(KeyCode::Enter)), Action::Redraw);

        let view = state.settings().unwrap().provider_input().unwrap();
        assert!(
            view.error.unwrap().contains("needs a value"),
            "{:?}",
            view.error
        );

        state.handle_key(press(KeyCode::Esc));
        assert!(state.settings().unwrap().provider_input().is_none());
        assert!(state.settings_provider_edits().is_empty());
    }

    /// A provider with no credential variable has no name to file a stored
    /// credential under, and is told so rather than having one invented.
    #[test]
    fn storing_a_credential_needs_a_credential_variable_name_first() {
        let mut state = ShellState::new("glasshouse", "/work", "0.1.0", Vec::new());
        state.open_settings(Vec::new(), Vec::new(), sample_provider_rows(), Vec::new());
        let mut state = to_providers(state);

        state.handle_key(press(KeyCode::Char('s')));
        let view = state.settings().unwrap().provider_input().unwrap();
        assert!(
            view.error.unwrap().contains("names no credential variable"),
            "{:?}",
            view.error
        );
    }

    /// **Acceptance 3, the configuration half.** Recording a stored
    /// credential stages the *reference* — two names — and clearing it
    /// removes exactly that, leaving every other field alone.
    #[test]
    fn recording_and_clearing_a_stored_credential_stages_only_the_reference() {
        let mut state = settings_with_credential_provider();

        let stored = StoredCredentialRef::new("glasshouse", "MY_ROUTER_KEY");
        state.record_provider_credential_stored("my-router", stored.clone());

        let row = &state.settings().unwrap().providers()[0];
        assert_eq!(row.config.credential_store(), Some(&stored));
        assert_eq!(
            row.config.base_url(),
            Some("https://mirror.example.com/v1"),
            "recording a stored credential must not disturb any other field"
        );
        let edits = state.settings_provider_edits();
        assert_eq!(edits.len(), 1);
        assert_eq!(
            edits[0].upsert.as_ref().unwrap().credential_store(),
            Some(&stored)
        );

        state.record_provider_credential_cleared("my-router");
        let row = &state.settings().unwrap().providers()[0];
        assert_eq!(row.config.credential_store(), None);
        assert_eq!(
            row.config.credential_env(),
            &["MY_ROUTER_KEY".to_owned()],
            "clearing the reference is not clearing the provider"
        );
        let edits = state.settings_provider_edits();
        assert_eq!(edits.len(), 1);
        assert_eq!(
            edits[0].upsert.as_ref().unwrap().credential_store(),
            None,
            "the staged edit must carry the cleared reference, or `w` would write it back"
        );
    }

    /// Deleting reaches the OS store, which no other edit in this overlay
    /// does, so it is confirmed first and Esc really does cancel.
    #[test]
    fn deleting_a_stored_credential_is_confirmed_first_and_esc_cancels() {
        let mut state = settings_with_credential_provider();
        state.record_provider_credential_stored(
            "my-router",
            StoredCredentialRef::new("glasshouse", "MY_ROUTER_KEY"),
        );

        state.handle_key(press(KeyCode::Char('x')));
        assert_eq!(
            state.settings().unwrap().confirming_credential_delete(),
            Some("my-router")
        );

        // An unrelated key is swallowed, exactly like the project-write
        // confirmation: never "any key dismisses".
        assert_eq!(state.handle_key(press(KeyCode::Char('z'))), Action::None);
        assert_eq!(
            state.settings().unwrap().confirming_credential_delete(),
            Some("my-router")
        );

        state.handle_key(press(KeyCode::Esc));
        assert_eq!(
            state.settings().unwrap().confirming_credential_delete(),
            None
        );
        assert!(
            state.settings().unwrap().providers()[0]
                .config
                .credential_store()
                .is_some(),
            "cancelling must change nothing"
        );

        // ... and confirming escalates to the run loop, which owns the I/O.
        state.handle_key(press(KeyCode::Char('x')));
        assert_eq!(
            state.handle_key(press(KeyCode::Char('y'))),
            Action::DeleteProviderCredential
        );
    }

    /// Every reference the selected provider's credential could be stored
    /// under, so a deletion cannot leave one of two copies behind.
    #[test]
    fn deletion_targets_both_the_recorded_reference_and_every_declared_variable() {
        let mut config = ProviderConfig::new("openrouter");
        config
            .set_credential_env(vec!["FIRST_KEY".to_owned(), "SECOND_KEY".to_owned()])
            .set_credential_store(Some(StoredCredentialRef::new("glasshouse", "FIRST_KEY")));
        let rows = vec![ProviderRow::new("pool", config, Layer::User)];

        let mut state = ShellState::new("glasshouse", "/work", "0.1.0", Vec::new());
        state.open_settings(Vec::new(), Vec::new(), rows, Vec::new());
        let state = to_providers(state);

        let (name, references) = state.selected_provider_stored_credentials().unwrap();
        assert_eq!(name, "pool");
        assert_eq!(
            references,
            vec![
                os_credential_for_variable("FIRST_KEY"),
                os_credential_for_variable("SECOND_KEY"),
            ],
            "the recorded reference is not duplicated, and no declared variable is skipped"
        );
    }

    /// Phase 2D's Routing tab offers all three model modes and stages each
    /// policy field independently. Invalid pins stay in the editor with an
    /// explanation instead of silently degrading before they are saved.
    #[test]
    fn routing_settings_validate_and_stage_every_policy_control() {
        fn replace_input(state: &mut ShellState, text: &str) {
            for _ in 0..80 {
                state.handle_key(press(KeyCode::Backspace));
            }
            type_text(state, text);
            state.handle_key(press(KeyCode::Enter));
        }

        let routing = RoutingRow::new(
            Layered::new(RoutingModelChoice::Deterministic, Layer::Default),
            Layered::new(RouterLatencyMs::DEFAULT, Layer::Default),
            Layered::new(RouterCostMicroUsd::DEFAULT, Layer::Default),
            Layered::new(true, Layer::Default),
            Layered::new(PremiumReservePercent::DEFAULT, Layer::Default),
            vec!["openrouter".to_owned()],
        );
        let mut state = ShellState::new("glasshouse", "/work", "0.1.0", Vec::new());
        state.open_settings_with_routing(
            Vec::new(),
            Vec::new(),
            sample_provider_rows(),
            Vec::new(),
            routing,
            MemoryRow::defaults(),
        );
        for _ in 0..4 {
            state.handle_key(press(KeyCode::Tab));
        }
        assert_eq!(
            state.settings().unwrap().section(),
            SettingsSection::Routing
        );

        state.handle_key(press(KeyCode::Char('m')));
        replace_input(&mut state, "missing:model");
        let error = state
            .settings()
            .unwrap()
            .routing_input()
            .and_then(|input| input.error)
            .unwrap_or_default();
        assert!(error.contains("not a configured provider"), "{error}");
        state.handle_key(press(KeyCode::Esc));

        state.handle_key(press(KeyCode::Char('m')));
        replace_input(&mut state, "automatic");
        assert_eq!(
            state.settings().unwrap().routing().model,
            RoutingModelChoice::Automatic
        );
        state.handle_key(press(KeyCode::Char('m')));
        replace_input(&mut state, "openrouter:openai/gpt-5-mini");
        assert!(matches!(
            &state.settings().unwrap().routing().model,
            RoutingModelChoice::Pinned { .. }
        ));
        state.handle_key(press(KeyCode::Char('m')));
        replace_input(&mut state, "deterministic");

        state.handle_key(press(KeyCode::Char('l')));
        replace_input(&mut state, "750");
        state.handle_key(press(KeyCode::Char('c')));
        replace_input(&mut state, "0.002500");
        state.handle_key(press(KeyCode::Char('f')));
        state.handle_key(press(KeyCode::Char('p')));
        replace_input(&mut state, "12");

        let edit = state.settings_routing_edit().expect("routing edit staged");
        assert_eq!(edit.model, Some(RoutingModelChoice::Deterministic));
        assert_eq!(edit.max_latency.unwrap().get(), 750);
        assert_eq!(edit.max_cost.unwrap().get(), 2_500);
        assert_eq!(edit.prefer_free, Some(false));
        assert_eq!(edit.premium_reserve.unwrap().get(), 12);
        let row = state.settings().unwrap().routing();
        assert_eq!(row.model_layer, Layer::User);
        assert_eq!(row.max_latency_layer, Layer::User);
        assert_eq!(row.max_cost_layer, Layer::User);
        assert_eq!(row.prefer_free_layer, Layer::User);
        assert_eq!(row.premium_reserve_layer, Layer::User);
    }
}

/// Phase 4's last three lines, at the layer that decides them: which session
/// the overview acts on, that acting on it leaves the presented session
/// alone, and that a session which is not running is refused out loud.
///
/// No processes here. What a byte does once it reaches a pseudo-terminal is
/// `tests/pty_smoke.rs`'s business; what this module has to get right is the
/// *aim* — and the aim is exactly what a test with a real process is worst
/// at proving, because a runtime with one session cannot tell "sent to the
/// session under the cursor" from "sent to whatever had focus".
#[cfg(test)]
mod overview_tests {
    use super::*;
    use crate::session::{SessionId, SessionLifecycle, SessionPresentation, SessionRole};

    fn record(id: &str, lifecycle: SessionLifecycle) -> SessionRecord {
        SessionRecord {
            id: SessionId::new(id),
            project_id: "p".to_owned(),
            harness: "claude-code".to_owned(),
            native_session_id: None,
            role: SessionRole::Normal,
            lifecycle,
            presentation: SessionPresentation::Embedded,
            created_at: 0,
            last_activity_at: 0,
            launch_profile: None,
            backend_resource: None,
            model: None,
            pairing_class: None,
            protocol: None,
            response_profile: None,
            response_mechanism: None,
            display_name: None,
            purpose: None,
            source_session_id: None,
            observed_compactions: None,
        }
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn typed(state: &mut ShellState, text: &str) {
        for c in text.chars() {
            state.handle_key(press(KeyCode::Char(c)));
        }
    }

    /// Two live sessions, the overview open, and the cursor moved onto the
    /// one the shell is *not* presenting — the situation both of the first
    /// two capability lines are about.
    fn overview_on_the_other_session() -> ShellState {
        let mut state = ShellState::new(
            "p",
            "/p",
            "0.1.0",
            vec![
                record("presented", SessionLifecycle::Running),
                record("background", SessionLifecycle::Running),
            ],
        );
        state.handle_key(press(KeyCode::Char('o')));
        state.handle_key(press(KeyCode::Down));
        assert_eq!(
            state.overview_target().map(|r| r.id.as_str()),
            Some("background")
        );
        assert_eq!(
            state.active_session().map(|r| r.id.as_str()),
            Some("presented"),
            "moving the overview cursor must not move the session bar"
        );
        state
    }

    /// Line 1, first half: a line typed in the overview is aimed at the
    /// session under the cursor.
    #[test]
    fn a_line_sent_from_the_overview_is_aimed_at_the_session_under_the_cursor() {
        let mut state = overview_on_the_other_session();

        state.handle_key(press(KeyCode::Char('m')));
        typed(&mut state, "status");
        let action = state.handle_key(press(KeyCode::Enter));

        assert_eq!(
            action,
            Action::SendSessionText {
                id: SessionId::new("background"),
                text: "status".to_owned(),
            }
        );
    }

    /// Line 1, second half, and the half that is the whole point: **the
    /// presented session does not change**. A capability that delivered the
    /// text but pulled the user into the session receiving it would satisfy
    /// the word "sending" and none of the intent.
    #[test]
    fn sending_a_line_leaves_the_presented_session_exactly_where_it_was() {
        let mut state = overview_on_the_other_session();
        let before = state.active_session().map(|record| record.id.clone());
        let before_index = state.selected_index();

        state.handle_key(press(KeyCode::Char('m')));
        typed(&mut state, "hello");
        state.handle_key(press(KeyCode::Enter));

        assert_eq!(state.active_session().map(|r| r.id.clone()), before);
        assert_eq!(state.selected_index(), before_index);
        assert_eq!(
            state.mode(),
            Mode::Control,
            "sending text must not hand the keyboard to anything"
        );
    }

    /// Line 2: the interrupt is aimed at the cursor's session, and nothing
    /// else moves — including, deliberately, the session's own recorded
    /// state. Interrupting is not killing.
    #[test]
    fn an_interrupt_from_the_overview_targets_the_cursor_and_moves_nothing_else() {
        let mut state = overview_on_the_other_session();
        let before = state.sessions().to_vec();
        let presented = state.active_session().map(|record| record.id.clone());

        let action = state.handle_key(press(KeyCode::Char('c')));

        assert_eq!(
            action,
            Action::InterruptSession(SessionId::new("background"))
        );
        assert_eq!(state.active_session().map(|r| r.id.clone()), presented);
        assert_eq!(
            state.sessions(),
            before.as_slice(),
            "an interrupt must not move any session's lifecycle"
        );
    }

    /// `Ctrl-C` is still how a user leaves Glasshouse. The overview's own
    /// `c` must not have swallowed it.
    #[test]
    fn control_c_still_quits_while_the_overview_is_open() {
        let mut state = overview_on_the_other_session();
        assert_eq!(
            state.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Action::Quit
        );
    }

    /// A refusal names the session and the state it is actually in, and
    /// produces no action at all — the acceptance condition for line 5 of the
    /// packet, and the rule this project already applies to the provider
    /// probes: a key that silently does nothing is indistinguishable from a
    /// frozen screen.
    #[test]
    fn acting_on_a_session_that_is_not_running_is_refused_by_name_and_changes_nothing() {
        for lifecycle in [
            SessionLifecycle::Stopped,
            SessionLifecycle::Failed,
            SessionLifecycle::Closed,
        ] {
            for key in [KeyCode::Char('c'), KeyCode::Char('m')] {
                let mut state = ShellState::new(
                    "p",
                    "/p",
                    "0.1.0",
                    vec![
                        record("alive", SessionLifecycle::Running),
                        record("finished", lifecycle),
                    ],
                );
                state.handle_key(press(KeyCode::Char('o')));
                state.handle_key(press(KeyCode::Down));
                let before = state.sessions().to_vec();

                let action = state.handle_key(press(key));

                assert_eq!(action, Action::Redraw, "{lifecycle:?} {key:?}");
                let status = state.status().unwrap_or_default().to_owned();
                assert!(
                    status.contains("finished"),
                    "the refusal must name the session; got {status:?}"
                );
                assert!(
                    status.contains(lifecycle.as_str()),
                    "the refusal must name the state; got {status:?}"
                );
                assert_eq!(
                    state.sessions(),
                    before.as_slice(),
                    "a refusal must change nothing"
                );
                assert!(
                    state.overview().and_then(OverviewState::entry).is_none(),
                    "a refused send must not leave a field open"
                );
            }
        }
    }

    /// The session can end while the line is being typed at it. Checking only
    /// when the field opened would send into a dead session and report
    /// success.
    #[test]
    fn a_session_that_dies_while_the_line_is_typed_is_refused_at_the_moment_of_sending() {
        let mut state = overview_on_the_other_session();
        state.handle_key(press(KeyCode::Char('m')));
        typed(&mut state, "too late");

        state.refresh(vec![
            record("presented", SessionLifecycle::Running),
            record("background", SessionLifecycle::Stopped),
        ]);

        let action = state.handle_key(press(KeyCode::Enter));
        assert_eq!(action, Action::Redraw, "nothing may be sent");
        let status = state.status().unwrap_or_default().to_owned();
        assert!(status.contains("background"), "got {status:?}");
        assert!(status.contains("stopped"), "got {status:?}");
    }

    /// Every key belongs to the field while one is open. Without this,
    /// typing a message containing `q` would quit Glasshouse — the same
    /// class of failure the session-mode split exists to prevent.
    #[test]
    fn the_send_field_owns_every_key_including_the_bindings() {
        let mut state = overview_on_the_other_session();
        state.handle_key(press(KeyCode::Char('m')));

        typed(&mut state, "quit now");
        assert_eq!(
            state.overview().and_then(OverviewState::entry),
            Some("quit now")
        );

        state.handle_key(press(KeyCode::Backspace));
        assert_eq!(
            state.overview().and_then(OverviewState::entry),
            Some("quit no")
        );

        let action = state.handle_key(press(KeyCode::Enter));
        assert_eq!(
            action,
            Action::SendSessionText {
                id: SessionId::new("background"),
                text: "quit no".to_owned(),
            }
        );
    }

    /// Escape cancels the field and returns to the overview rather than
    /// closing the overlay: leaving is one more Escape away, and a field that
    /// took the whole overlay with it would lose the row the user was aiming
    /// at.
    #[test]
    fn escape_cancels_the_field_and_keeps_the_overview_open() {
        let mut state = overview_on_the_other_session();
        state.handle_key(press(KeyCode::Char('m')));
        typed(&mut state, "never mind");

        assert_eq!(state.handle_key(press(KeyCode::Esc)), Action::Redraw);
        assert_eq!(state.overlay(), Some(Overlay::Overview));
        assert!(state.overview().and_then(OverviewState::entry).is_none());
    }

    /// An empty line is refused rather than sent as a bare carriage return —
    /// which a harness would read as a submitted empty prompt.
    #[test]
    fn an_empty_line_is_not_sent() {
        let mut state = overview_on_the_other_session();
        state.handle_key(press(KeyCode::Char('m')));

        assert_eq!(state.handle_key(press(KeyCode::Enter)), Action::Redraw);
        assert!(
            state.status().unwrap_or_default().contains("empty"),
            "got {:?}",
            state.status()
        );
    }

    /// Sessions are ordered by last activity, so any refresh can reorder
    /// them. A cursor held as a bare index would then be pointing at a
    /// different session — and the next `c` would interrupt the wrong
    /// process, silently.
    #[test]
    fn a_reorder_carries_the_overview_cursor_with_its_session() {
        let mut state = overview_on_the_other_session();

        state.refresh(vec![
            record("background", SessionLifecycle::Running),
            record("presented", SessionLifecycle::Running),
        ]);

        assert_eq!(
            state.overview_target().map(|r| r.id.as_str()),
            Some("background"),
            "the cursor must follow its session, not its row"
        );
        assert_eq!(
            state.active_session().map(|r| r.id.as_str()),
            Some("presented"),
            "and so must the bar's own selection"
        );
    }

    /// The cursor is a ring, like the session bar.
    #[test]
    fn the_overview_cursor_wraps_in_both_directions() {
        let mut state = overview_on_the_other_session();
        state.handle_key(press(KeyCode::Down));
        assert_eq!(
            state.overview_target().map(|r| r.id.as_str()),
            Some("presented")
        );
        state.handle_key(press(KeyCode::Up));
        assert_eq!(
            state.overview_target().map(|r| r.id.as_str()),
            Some("background")
        );
    }

    /// Closing and reopening the overview puts the cursor back on the session
    /// the bar is presenting, rather than resuming wherever it was left. A
    /// stale cursor is how an interrupt reaches a session the user has
    /// forgotten they pointed at.
    #[test]
    fn reopening_the_overview_starts_the_cursor_on_the_presented_session() {
        let mut state = overview_on_the_other_session();
        state.handle_key(press(KeyCode::Esc));
        assert_eq!(state.overlay(), None);

        state.handle_key(press(KeyCode::Char('o')));
        assert_eq!(
            state.overview_target().map(|r| r.id.as_str()),
            Some("presented")
        );
    }

    /// Tab still moves the session bar underneath the popup — the Overview is
    /// a passive overlay and stayed one.
    #[test]
    fn ordinary_navigation_still_works_underneath_the_overview() {
        let mut state = overview_on_the_other_session();
        state.handle_key(press(KeyCode::Tab));
        assert_eq!(
            state.active_session().map(|r| r.id.as_str()),
            Some("background")
        );
        assert_eq!(state.overlay(), Some(Overlay::Overview));
    }

    /// Line 3, at this layer: a headless session has no viewport, so there is
    /// nothing to enter. Letting the user in would put their keystrokes into
    /// whichever session held focus instead — the bar showing one session
    /// while another receives the typing.
    #[test]
    fn a_headless_session_cannot_be_entered() {
        let mut headless = record("hidden", SessionLifecycle::Running);
        headless.presentation = SessionPresentation::Headless;
        let mut state = ShellState::new("p", "/p", "0.1.0", vec![headless]);

        for key in [press(KeyCode::Enter), press(KeyCode::Char('i'))] {
            assert_eq!(state.handle_key(key), Action::Redraw);
            assert_eq!(
                state.mode(),
                Mode::Control,
                "a headless session must never take the keyboard"
            );
            let status = state.status().unwrap_or_default().to_owned();
            assert!(status.contains("hidden"), "got {status:?}");
            assert!(status.contains("headless"), "got {status:?}");
        }
    }

    /// `N` is the headless twin of `n`, and `n` still starts an ordinary one.
    #[test]
    fn shift_n_starts_a_headless_session_and_n_still_starts_an_embedded_one() {
        let mut state = ShellState::new("p", "/p", "0.1.0", vec![]);
        assert_eq!(
            state.handle_key(press(KeyCode::Char('n'))),
            Action::StartSession
        );
        assert_eq!(
            state.handle_key(KeyEvent::new(KeyCode::Char('N'), KeyModifiers::SHIFT)),
            Action::StartHeadlessSession
        );
    }

    /// An empty project answers both overview keys with a note rather than
    /// panicking on an index into nothing.
    #[test]
    fn an_empty_project_refuses_both_overview_actions() {
        let mut state = ShellState::new("p", "/p", "0.1.0", vec![]);
        state.handle_key(press(KeyCode::Char('o')));
        assert!(state.overview_target().is_none());

        for key in [
            press(KeyCode::Char('c')),
            press(KeyCode::Char('m')),
            press(KeyCode::Down),
        ] {
            assert_eq!(state.handle_key(key), Action::Redraw);
            assert!(state.status().is_some(), "{key:?} said nothing");
        }
    }

    /// Line 687: Enter brings the cursor's session into the viewport and
    /// hands it the keyboard — not the session the bar was already
    /// presenting, which is the entire reason `focus_overview_target` reads
    /// the cursor rather than reusing `active_session`.
    #[test]
    fn enter_focuses_the_cursors_session_not_the_presented_one() {
        let mut state = overview_on_the_other_session();

        let action = state.handle_key(press(KeyCode::Enter));

        assert_eq!(action, Action::Redraw);
        assert_eq!(
            state.active_session().map(|r| r.id.as_str()),
            Some("background"),
            "focus must move the viewport onto the cursor's session"
        );
        assert_eq!(
            state.mode(),
            Mode::Session,
            "focus must hand the keyboard to the focused session"
        );
        assert_eq!(
            state.overlay(),
            None,
            "focusing a session must close the overview it was opened from"
        );
    }

    /// A headless session is live, so `actionable_overview_target` alone
    /// would let it through — but it has no viewport to focus into, and the
    /// box names both adjectives. Refused by name, and nothing moves.
    #[test]
    fn enter_refuses_a_live_headless_session_by_name() {
        let mut state = ShellState::new(
            "p",
            "/p",
            "0.1.0",
            vec![SessionRecord {
                presentation: SessionPresentation::Headless,
                ..record("bg-headless", SessionLifecycle::Running)
            }],
        );
        state.handle_key(press(KeyCode::Char('o')));
        let before_mode = state.mode();
        let before_selected = state.selected_index();

        let action = state.handle_key(press(KeyCode::Enter));

        assert_eq!(action, Action::Redraw);
        let status = state.status().unwrap_or_default().to_owned();
        assert!(
            status.contains("bg-headless") && status.contains("headless"),
            "the refusal must name the session and why; got {status:?}"
        );
        assert_eq!(
            state.mode(),
            before_mode,
            "a refusal must not enter Session mode"
        );
        assert_eq!(state.selected_index(), before_selected);
        assert_eq!(
            state.overlay(),
            Some(Overlay::Overview),
            "a refusal must leave the overview open"
        );
    }

    /// Enter still refuses a stopped session, through the same liveness gate
    /// `c` and `m` already use — resume is `r`'s job, not Enter's.
    #[test]
    fn enter_refuses_a_stopped_session() {
        let mut state = ShellState::new(
            "p",
            "/p",
            "0.1.0",
            vec![
                record("alive", SessionLifecycle::Running),
                record("finished", SessionLifecycle::Stopped),
            ],
        );
        state.handle_key(press(KeyCode::Char('o')));
        state.handle_key(press(KeyCode::Down));

        let action = state.handle_key(press(KeyCode::Enter));

        assert_eq!(action, Action::Redraw);
        assert_eq!(state.mode(), Mode::Control);
        let status = state.status().unwrap_or_default().to_owned();
        assert!(status.contains("finished"), "got {status:?}");
    }

    /// Line 688: `r` resumes the session under the cursor — a *stopped* one,
    /// which is exactly what `actionable_overview_target`'s liveness gate
    /// would refuse, and the reason `resumable_overview_target` is its own
    /// gate rather than a reuse of that helper.
    #[test]
    fn r_resumes_a_stopped_session_with_a_native_identifier() {
        let mut state = ShellState::new(
            "p",
            "/p",
            "0.1.0",
            vec![SessionRecord {
                native_session_id: Some("native-42".to_owned()),
                ..record("finished", SessionLifecycle::Stopped)
            }],
        );
        state.handle_key(press(KeyCode::Char('o')));

        let action = state.handle_key(press(KeyCode::Char('r')));

        assert_eq!(action, Action::ResumeSession(SessionId::new("finished")));
    }

    /// A stopped session with no native identifier is `closed`, not
    /// `resumable` — `SessionRecord::disposition`'s own rule, so `r` must
    /// refuse it exactly as the STATE column already reports it.
    #[test]
    fn r_refuses_a_stopped_session_with_no_native_identifier() {
        let mut state = ShellState::new(
            "p",
            "/p",
            "0.1.0",
            vec![record("finished", SessionLifecycle::Stopped)],
        );
        state.handle_key(press(KeyCode::Char('o')));

        let action = state.handle_key(press(KeyCode::Char('r')));

        assert_eq!(action, Action::Redraw);
        let status = state.status().unwrap_or_default().to_owned();
        assert!(
            status.contains("finished") && status.contains("closed"),
            "got {status:?}"
        );
    }

    /// `r` must refuse a *live* session — the opposite mistake from every
    /// other overview action, and the one `actionable_overview_target` would
    /// make if resume reused it: that helper accepts exactly the sessions
    /// resume must refuse.
    #[test]
    fn r_refuses_a_live_session() {
        let mut state = overview_on_the_other_session();

        let action = state.handle_key(press(KeyCode::Char('r')));

        assert_eq!(action, Action::Redraw);
        let status = state.status().unwrap_or_default().to_owned();
        assert!(
            status.contains("background") && status.contains("running"),
            "got {status:?}"
        );
    }
}

/// Map line 1105: the project-knowledge overlay's cursor and detail popup,
/// exercised through [`ShellState::handle_key`] — the same production key
/// path a real terminal drives — rather than by poking `ProjectKnowledgeState`
/// fields directly.
#[cfg(test)]
mod project_knowledge_state_tests {
    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn entry(text: &str) -> KnowledgeSection {
        KnowledgeSection {
            lines: vec![text.to_owned()],
            details: vec![MemoryDetail {
                rationale: Some(format!("why: {text}")),
                source_session: Some("sess_fixture".to_owned()),
                source_commit: Some("abc1234".to_owned()),
                lifecycle: "active".to_owned(),
            }],
            omitted: 0,
        }
    }

    fn opened_with_three_entries() -> ShellState {
        let mut state = ShellState::new("p", "/p", "0.1.0", Vec::new());
        state.handle_key(press(KeyCode::Char('k')));
        state.open_project_knowledge(
            entry("decision: first"),
            entry("constraint: second"),
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            entry("todo: third"),
            None,
        );
        state
    }

    /// The cursor walks every section's entries in render order — decisions,
    /// constraints, features, failed attempts, todos — and wraps at both
    /// ends, the same ring [`ShellState::move_overview_cursor`] is.
    #[test]
    fn moving_the_knowledge_cursor_wraps_across_every_section() {
        let mut state = opened_with_three_entries();
        assert_eq!(state.project_knowledge().unwrap().cursor(), 0);

        state.handle_key(press(KeyCode::Down));
        assert_eq!(state.project_knowledge().unwrap().cursor(), 1);
        state.handle_key(press(KeyCode::Down));
        assert_eq!(state.project_knowledge().unwrap().cursor(), 2);
        state.handle_key(press(KeyCode::Down));
        assert_eq!(
            state.project_knowledge().unwrap().cursor(),
            0,
            "must wrap forward past the last entry"
        );

        state.handle_key(press(KeyCode::Up));
        assert_eq!(
            state.project_knowledge().unwrap().cursor(),
            2,
            "must wrap backward past the first entry"
        );
    }

    /// Enter opens the detail popup for whichever entry the cursor is on —
    /// not always the first one — and Esc returns to the list without
    /// closing the whole overlay.
    #[test]
    fn enter_opens_detail_for_the_entry_under_the_cursor_and_esc_returns_to_the_list() {
        let mut state = opened_with_three_entries();
        state.handle_key(press(KeyCode::Down));
        assert_eq!(state.project_knowledge().unwrap().cursor(), 1);

        state.handle_key(press(KeyCode::Enter));
        assert!(state.project_knowledge().unwrap().detail_open());
        let (text, detail) = state.project_knowledge().unwrap().selected().unwrap();
        assert_eq!(text, "constraint: second");
        assert_eq!(detail.rationale.as_deref(), Some("why: constraint: second"));

        state.handle_key(press(KeyCode::Esc));
        assert!(!state.project_knowledge().unwrap().detail_open());
        assert_eq!(
            state.overlay(),
            Some(Overlay::ProjectKnowledge),
            "Esc must close only the detail popup, not the whole overlay"
        );
    }

    /// A project-knowledge view with nothing recorded has nothing to
    /// select: Enter must not open an empty detail popup, and must say why
    /// rather than doing nothing silently.
    #[test]
    fn enter_refuses_when_nothing_is_selectable() {
        let mut state = ShellState::new("p", "/p", "0.1.0", Vec::new());
        state.handle_key(press(KeyCode::Char('k')));
        state.open_project_knowledge(
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            KnowledgeSection::default(),
            None,
        );

        state.handle_key(press(KeyCode::Enter));

        assert!(!state.project_knowledge().unwrap().detail_open());
        assert!(state.status().unwrap_or_default().contains("nothing"));
    }
}

#[cfg(test)]
mod activity_tests {
    use super::*;
    use crate::events::{EventBus, GatewayFailure, ProcessExit};

    fn events_of(events: &[LifecycleEvent]) -> Vec<RecordedEvent> {
        let bus = EventBus::new();
        let session = SessionId::new("s-1");
        events
            .iter()
            .map(|event| bus.publish(&session, event.clone()))
            .collect()
    }

    /// One representative of every [`LifecycleEvent`] variant this crate
    /// defines.
    ///
    /// Held to that claim by `the_variant_list_covers_every_kind` rather than
    /// by a comment: a list that merely *said* it was complete would go on
    /// saying so after somebody added a variant, and the distinctness check
    /// below would then be proving less than it looks like it proves.
    fn one_of_each_variant() -> Vec<LifecycleEvent> {
        vec![
            LifecycleEvent::SessionStarted,
            LifecycleEvent::SessionResumed,
            LifecycleEvent::TurnStarted,
            LifecycleEvent::TurnEnded {
                outcome: TurnOutcome::Completed,
            },
            LifecycleEvent::WaitingForUser,
            LifecycleEvent::TextDelivered {
                origin: MessageOrigin::Machine,
                bytes: 41,
            },
            LifecycleEvent::InterruptDelivered {
                origin: MessageOrigin::Machine,
            },
            LifecycleEvent::ProcessExited {
                exit: ProcessExit::from_parts(0, None),
            },
            LifecycleEvent::OutputEnded,
            LifecycleEvent::GatewayUnhealthy {
                resource: "db".to_owned(),
                reason: GatewayFailure::Unreachable,
            },
            LifecycleEvent::GatewayBackendChanged {
                provider: "anthropic".to_owned(),
                model: "claude".to_owned(),
                cause: "failover".to_owned(),
            },
        ]
    }

    #[test]
    fn an_empty_slice_notes_nothing() {
        let mut state = ShellState::new("p", "/p", "0.1.0", vec![]);
        assert_eq!(state.note_events(&[]), Action::None);
        assert!(state.activity().is_empty());
    }

    #[test]
    fn note_events_keeps_the_newest_and_is_bounded_at_activity_rows() {
        let mut state = ShellState::new("p", "/p", "0.1.0", vec![]);
        let variants: Vec<LifecycleEvent> = (0..ACTIVITY_ROWS + 3)
            .map(|n| LifecycleEvent::TextDelivered {
                origin: MessageOrigin::Machine,
                bytes: n,
            })
            .collect();
        let recorded = events_of(&variants);

        assert_eq!(state.note_events(&recorded), Action::Redraw);

        assert_eq!(state.activity().len(), ACTIVITY_ROWS);
        // Newest first: the last event published (largest `bytes`) leads.
        assert_eq!(
            state.activity()[0].event(),
            &LifecycleEvent::TextDelivered {
                origin: MessageOrigin::Machine,
                bytes: ACTIVITY_ROWS + 2,
            }
        );
        // Bounded: the oldest events (smallest `bytes`) were discarded.
        for kept in state.activity() {
            let LifecycleEvent::TextDelivered { bytes, .. } = kept.event() else {
                panic!("only TextDelivered events were published");
            };
            assert!(*bytes >= 3, "an old event survived: {bytes}");
        }
    }

    /// The list above really is every variant.
    ///
    /// Anchored on `LifecycleEvent::kind`, which is exhaustive at the
    /// compiler's insistence and is already pinned to the project database's
    /// own `CHECK` constraint. So adding a variant fails here until the list
    /// grows, which is what keeps the distinctness test below honest.
    #[test]
    fn the_variant_list_covers_every_kind() {
        let covered: std::collections::BTreeSet<&str> = one_of_each_variant()
            .iter()
            .map(LifecycleEvent::kind)
            .collect();
        let known: std::collections::BTreeSet<&str> =
            crate::database::LIFECYCLE_EVENT_KINDS.into_iter().collect();
        assert_eq!(
            covered, known,
            "the activity view's variant list has drifted from the event enum"
        );
    }

    /// Catches a `_` arm creeping back into `describe_event`: a collapsed
    /// variant would make two of these summaries equal.
    #[test]
    fn every_variant_renders_a_distinct_non_empty_summary() {
        let variants = one_of_each_variant();
        let mut summaries: Vec<String> = variants.iter().map(describe_event).collect();
        for summary in &summaries {
            assert!(!summary.is_empty());
        }
        let before = summaries.len();
        summaries.sort();
        summaries.dedup();
        assert_eq!(
            summaries.len(),
            before,
            "two variants rendered the same summary"
        );
    }

    #[test]
    fn a_completed_turn_reads_differently_from_a_failed_one() {
        let completed = describe_event(&LifecycleEvent::TurnEnded {
            outcome: TurnOutcome::Completed,
        });
        let failed = describe_event(&LifecycleEvent::TurnEnded {
            outcome: TurnOutcome::Failed,
        });
        assert_ne!(completed, failed);
    }

    #[test]
    fn machine_text_reads_differently_from_a_user_keystroke() {
        let machine = describe_event(&LifecycleEvent::TextDelivered {
            origin: MessageOrigin::Machine,
            bytes: 10,
        });
        let typed = describe_event(&LifecycleEvent::TextDelivered {
            origin: MessageOrigin::UserKeystroke,
            bytes: 10,
        });
        assert_ne!(machine, typed);
    }
}
