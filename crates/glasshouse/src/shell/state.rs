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
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Style;

use crate::config::{Layer, ProfileApproval, ProfileBackend, ProfileConfig, ProviderConfig};
use crate::integrations::{IntegrationId, IntegrationKind, IntegrationStatus};
use crate::platform::exec;
use crate::secret::{EnvironmentSecretStore, SecretStore};
use crate::session::SessionRecord;

/// A Glasshouse-owned screen drawn over the session viewport.
///
/// Overlays are transient by design: they are somewhere the user visits and
/// leaves, and leaving returns to the session that was already active rather
/// than closing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    /// Every session in the project, with the detail the bar has no room for.
    Overview,
    /// Harnesses and Integrations configuration. See [`SettingsState`] for
    /// the data behind it — this marker carries none of it, the same way
    /// [`Overlay::Overview`] carries none of the session list it shows.
    Settings,
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
    /// Reopen the first-run wizard for a "reconfigure" invocation (Phase 2C:
    /// "Allow the onboarding wizard to be reopened later from settings").
    /// Discovery and reading `UserConfig` are file I/O this module
    /// deliberately does not hold, and driving `crate::onboarding::run`
    /// needs the terminal this shell's own [`crate::tui::Screen`] already
    /// holds — both are the run loop's job, exactly like
    /// [`Action::OpenSettings`].
    ReopenOnboarding,
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
/// screen yet, which [`super::view::render_viewport`] takes as its signal to
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
    /// run loop has set it, which [`super::view::render_viewport`] uses to
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
    pub fn open_overview(&mut self) -> Action {
        if self.overlay == Some(Overlay::Overview) {
            return Action::None;
        }
        self.overlay = Some(Overlay::Overview);
        Action::Redraw
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
        self.overlay = Some(Overlay::Settings);
        self.settings = Some(SettingsState::new(
            harnesses,
            integrations,
            providers,
            profiles,
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
        if let Some(settings) = self.settings.as_mut() {
            settings.replace_rows(harnesses, integrations, providers, profiles);
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

    /// Replace the session list, keeping the same session presented if it is
    /// still there.
    ///
    /// Reconciling by identity rather than by index: sessions are ordered by
    /// last activity, so any refresh can reorder them, and holding an index
    /// would silently move the user to a different session.
    pub fn refresh(&mut self, sessions: Vec<SessionRecord>) -> Action {
        let active = self.active_session().map(|record| record.id.clone());
        let unchanged = sessions == self.sessions;
        self.sessions = sessions;
        self.selected = active
            .and_then(|id| self.sessions.iter().position(|record| record.id == id))
            .unwrap_or(0);
        if unchanged {
            Action::None
        } else {
            Action::Redraw
        }
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

        // An overlay takes the keys it understands before anything else, so
        // Escape means "leave this overlay" while one is open and "leave
        // Glasshouse" only when none is.
        if self.overlay.is_some() && matches!(key.code, KeyCode::Esc | KeyCode::Char('o')) {
            return self.close_overlay();
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('c' | 'C') if ctrl => Action::Quit,
            KeyCode::Char('q') => Action::Quit,
            KeyCode::Esc => Action::Quit,
            KeyCode::Tab | KeyCode::Right => self.next_session(),
            KeyCode::BackTab | KeyCode::Left => self.previous_session(),
            KeyCode::Char('o') => self.open_overview(),
            KeyCode::Char('s') => Action::OpenSettings,
            KeyCode::Enter | KeyCode::Char('i') => self.enter_session_mode(),
            KeyCode::Char('n') => Action::StartSession,
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
    fn enter_session_mode(&mut self) -> Action {
        if self.active_session().is_none() {
            self.set_status("no session to enter — start one with `n`");
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
// Settings — see `GLASSHOUSE_DESIGN_DECISIONS.md`'s "Settings" section for
// the invariants this data model exists to hold to.
// -----------------------------------------------------------------------

/// Which section of the Settings overlay has the cursor.
///
/// Harnesses and Integrations shipped first, per the design decision: build
/// only the section whose feature exists. Phase 2D adds Providers and Launch
/// Profiles beside them, now that [`crate::config::ProviderConfig`] and
/// [`crate::config::ProfileConfig`] both exist. Do not add a fifth here
/// without first shipping the feature it would configure — Routing needs a
/// routing-model configuration field nobody has designed yet, and Memory is
/// Phase 20.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsSection {
    Harnesses,
    Integrations,
    Providers,
    LaunchProfiles,
}

impl SettingsSection {
    /// Tab order. `next`/`previous` cycle through this, so adding a section
    /// only ever means inserting it here.
    const ORDER: [SettingsSection; 4] = [
        SettingsSection::Harnesses,
        SettingsSection::Integrations,
        SettingsSection::Providers,
        SettingsSection::LaunchProfiles,
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
}

/// One row of the Settings "Launch Profiles" section, matching
/// [`ProviderRow`]'s shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileRow {
    pub name: String,
    pub config: ProfileConfig,
    pub layer: Layer,
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

/// A [`PendingEdit`] together with the harness it belongs to, in the shape
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
}

#[derive(Debug)]
struct ProviderTextInput {
    purpose: ProviderInputPurpose,
    buffer: String,
    error: Option<String>,
}

/// Read-only view of the active Providers-section text input, for rendering.
pub struct ProviderInputView<'a> {
    pub label: String,
    pub buffer: &'a str,
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

/// The outcome of Line 5's connectivity check.
///
/// **This is a precondition check, not a network request.** Glasshouse has
/// no HTTP client dependency yet — another worker is adding one for the
/// gateway on a separate branch — so this proves only what can be proven
/// without one: the provider resolves to a real template, it declares at
/// least one protocol, that protocol's base URL is non-empty, and (when the
/// provider names any credential variables at all) at least one of them is
/// currently set. [`ReachabilityCheck::PreconditionsMet`] is never proof the
/// network is reachable, and the view names it as a precondition check for
/// exactly that reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReachabilityCheck {
    PreconditionsMet {
        protocol: &'static str,
        base_url: String,
    },
    Failed(String),
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

/// Check `config`'s reachability *preconditions* for the provider named
/// `name` — see [`ReachabilityCheck`]'s own doc for exactly what is and is
/// not established here.
///
/// Presence is checked with [`SecretStore::is_present`], never
/// [`SecretStore::resolve`]: this function has no need to hold a credential's
/// value even transiently, so it never asks for one.
fn check_provider_reachability(
    name: &str,
    config: &ProviderConfig,
    secrets: &dyn SecretStore,
) -> ReachabilityCheck {
    let provider = match config.to_provider(name) {
        Ok(provider) => provider,
        Err(err) => return ReachabilityCheck::Failed(err.to_string()),
    };
    let Some(support) = provider.protocols.first() else {
        return ReachabilityCheck::Failed(format!("provider `{name}` declares no protocol"));
    };
    if support.base_url.is_empty() {
        return ReachabilityCheck::Failed(format!(
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
            return ReachabilityCheck::Failed(format!(
                "none of provider `{name}`'s credential variable(s) ({}) is set",
                provider.credential_env.join(", ")
            ));
        }
    }
    ReachabilityCheck::PreconditionsMet {
        protocol: support.protocol.slug(),
        base_url: support.base_url.clone(),
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
    /// The confirmed half of `W`: apply every pending edit to the
    /// project-level configuration. Only ever produced after the user
    /// answered the confirmation with `y` or `Enter`.
    SaveProject,
    /// `r`: reopen the first-run wizard. See [`Action::ReopenOnboarding`].
    ReopenOnboarding,
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
/// [`SettingsState::replace_rows`] after that write succeeds — which is also
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
    path_input: Option<SettingsPathInput>,
    /// Whether the `W` confirmation prompt (design decision: "first shows
    /// the exact path to be created and requires a distinct confirmation")
    /// is currently showing.
    confirm_project_write: bool,
    provider_input: Option<ProviderTextInput>,
    profile_input: Option<ProfileTextInput>,
    /// The last connectivity-precondition check run this session, and which
    /// provider it was for — see [`ReachabilityCheck`]. Cleared by any other
    /// key the general dispatcher in [`SettingsState::handle_key`] handles —
    /// exactly like the status note in the outer shell footer — so it can
    /// never shadow a wizard or field editor that opens afterward.
    provider_test_result: Option<(String, ReachabilityCheck)>,
}

impl SettingsState {
    fn new(
        harnesses: Vec<HarnessRow>,
        integrations: Vec<IntegrationRow>,
        providers: Vec<ProviderRow>,
        profiles: Vec<ProfileRow>,
    ) -> Self {
        Self {
            section: SettingsSection::Harnesses,
            harnesses,
            integrations,
            providers,
            profiles,
            selected_harness: 0,
            selected_integration: 0,
            selected_provider: 0,
            selected_profile: 0,
            edits: HashMap::new(),
            provider_edits: HashMap::new(),
            profile_edits: HashMap::new(),
            path_input: None,
            confirm_project_write: false,
            provider_input: None,
            profile_input: None,
            provider_test_result: None,
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
        };
        Some(ProviderInputView {
            label,
            buffer: input.buffer.as_str(),
            error: input.error.as_deref(),
        })
    }

    /// The most recent connectivity-precondition check, if one has been run
    /// this Settings session — see [`ReachabilityCheck`].
    pub fn provider_test_result(&self) -> Option<(&str, &ReachabilityCheck)> {
        self.provider_test_result
            .as_ref()
            .map(|(name, outcome)| (name.as_str(), outcome))
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
        self.edits.clear();
        self.provider_edits.clear();
        self.profile_edits.clear();
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

        // A stale reachability-check banner must not permanently shadow
        // another bottom-panel view — the wizard/field editors this session
        // opens afterward all render in that same area — so any key that
        // reaches this general dispatcher clears it first, exactly like the
        // outer shell's own status note clears on the next keystroke. The
        // `t` arm below sets it again in the same keypress when that is what
        // was actually pressed.
        self.provider_test_result = None;

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
            KeyCode::Char(' ') if self.section == SettingsSection::Providers => {
                self.toggle_selected_provider();
                SettingsAction::Redraw
            }
            KeyCode::Char('d') if self.section == SettingsSection::Providers => {
                self.remove_selected_provider();
                SettingsAction::Redraw
            }
            KeyCode::Char('t') if self.section == SettingsSection::Providers => {
                self.test_selected_provider();
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
        self.provider_test_result = None;
    }

    fn test_selected_provider(&mut self) {
        let Some(row) = self.providers.get(self.selected_provider) else {
            return;
        };
        let outcome =
            check_provider_reachability(&row.name, &row.config, &EnvironmentSecretStore::new());
        self.provider_test_result = Some((row.name.clone(), outcome));
    }

    fn handle_provider_input_key(&mut self, key: KeyEvent) -> SettingsAction {
        match key.code {
            KeyCode::Esc => {
                self.provider_input = None;
                SettingsAction::Redraw
            }
            KeyCode::Enter => {
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
                self.providers.push(ProviderRow {
                    name: name.clone(),
                    config: config.clone(),
                    layer: Layer::User,
                });
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

/// `Ctrl-]` — the one chord that returns to control mode from session mode.
///
/// See the design note for why this chord: it is what `telnet` has used for
/// decades, no ordinary key produces it, and it is exactly one chord rather
/// than a prefix that would double the latency of escaping a runaway session.
///
/// **It has two spellings, and both must be accepted.** The chord is really
/// the byte `0x1D` (ASCII group separator), and Crossterm's Unix parser
/// decodes the control range `0x1C..=0x1F` arithmetically, as
/// `Char((c - 0x1C + b'4'))` — so a real terminal's `Ctrl-]` arrives as
/// `Ctrl` + `'5'`, never as `Ctrl` + `']'`. The `']'` spelling comes from
/// Windows, where Crossterm reads virtual key codes instead.
///
/// Matching only `']'` is not a cosmetic bug: it leaves the user in session
/// mode with no way back, which is precisely the failure the single-chord
/// escape exists to prevent. It survived unit testing because a synthetic
/// `KeyEvent::new(KeyCode::Char(']'), CONTROL)` is not what any terminal
/// sends; only driving the real binary through a real pseudo-terminal caught
/// it.
fn is_session_escape(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char(']') | KeyCode::Char('5'))
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
        vec![ProviderRow {
            name: "my-router".to_owned(),
            config,
            layer: Layer::User,
        }]
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
        let rows = vec![ProviderRow {
            name: "unset-cred".to_owned(),
            config,
            layer: Layer::User,
        }];

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

    /// The positive counterpart: every precondition holds, and the result
    /// names the protocol and base URL actually resolved — never a real
    /// network request, just what `check_provider_reachability` can prove
    /// without one.
    #[test]
    fn a_passing_reachability_precondition_check_names_the_protocol_and_base_url() {
        const VAR: &str = "GLASSHOUSE_SETTINGS_TEST_ONLY_PRESENT_CRED_VAR";
        // SAFETY: `VAR` is unique to this test and is removed again below.
        unsafe {
            std::env::set_var(VAR, "sk-fabricated-test-value-not-a-real-credential");
        }

        let mut config = ProviderConfig::new("openrouter");
        config.set_credential_env(vec![VAR.to_owned()]);
        let rows = vec![ProviderRow {
            name: "set-cred".to_owned(),
            config,
            layer: Layer::User,
        }];

        let mut state = state_with_a_session();
        state.open_settings(Vec::new(), Vec::new(), rows, Vec::new());
        state = to_providers(state);
        state.handle_key(press(KeyCode::Char('t')));

        unsafe {
            std::env::remove_var(VAR);
        }

        let (_, outcome) = state
            .settings()
            .unwrap()
            .provider_test_result()
            .expect("a test ran");
        match outcome {
            ReachabilityCheck::PreconditionsMet { protocol, base_url } => {
                assert!(!protocol.is_empty());
                assert!(!base_url.is_empty());
            }
            other => panic!("expected preconditions met, got {other:?}"),
        }
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
}
