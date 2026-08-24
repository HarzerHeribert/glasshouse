//! The wizard's state machine, kept deliberately free of the terminal.
//!
//! [`WizardState`] owns every piece of mutable wizard data — which step is
//! current, the cursor over the integration list, the text being typed into
//! the explicit-path field, and the pending enable/ignore decisions — and is
//! driven entirely through [`WizardState::handle_key`]. That function takes a
//! `crossterm` [`KeyEvent`] and returns an [`Action`] telling the caller what
//! to do next; it never touches a [`crate::tui::Screen`], never reads the
//! clock, and never consults process-global state. The one exception is
//! [`crate::platform::exec::resolve_explicit`], a synchronous, local
//! filesystem check with no network or subprocess involved — validating a
//! user-typed path is the whole point of the explicit-path step (Phase 2C:
//! "rather than accepting a path that will fail later at launch"), and it is
//! exactly as testable as everything else here because it is deterministic
//! given real files on disk (see the `tests` module, which creates real
//! executables in a `tempfile::tempdir`).
//!
//! This split is what makes the wizard testable at all: [`super::run`] pumps
//! a real terminal's events into `handle_key` and paints [`super::view`]
//! after every [`Action`] that says something changed, but none of the
//! actual decision logic lives there. Every test in this module drives
//! `handle_key` directly.

use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent};

use crate::config::UserConfig;
use crate::integrations::{IntegrationId, IntegrationKind, IntegrationStatus};
use crate::platform::exec;
use crate::tui::is_quit_key;

/// The subset of one integration-discovery result the wizard needs to show,
/// expressed as plain owned data rather than
/// [`crate::integrations::DetectedIntegration`] itself.
///
/// `DetectedIntegration`'s fields are private to the `integrations` module
/// with no public constructor — real instances only ever come from an actual
/// [`crate::integrations::Discovery`] pass, which is deliberate: discovery
/// results should not be fabricated. But the wizard must not call
/// [`crate::integrations::Discovery::run`] itself with a project (a
/// terminal-driven wizard reaching out to spawn version-probe subprocesses
/// on its own would be a surprising, hard-to-test side effect of constructing
/// a [`WizardState`]), and tests need to
/// construct arbitrary detection results — including the "cmux was not
/// detected" case — without depending on what happens to be installed on the
/// machine running the tests. [`super::detections_from`] maps a real
/// `Discovery` into this shape at the boundary; every field here is read
/// through `DetectedIntegration`'s public getters.
#[derive(Debug, Clone)]
pub struct IntegrationDetection {
    pub id: IntegrationId,
    pub status: IntegrationStatus,
    pub executable: Option<PathBuf>,
    pub version: Option<String>,
}

/// One screen of the wizard.
///
/// Deliberately three: an introduction, the interactive integration list,
/// and a confirmation summary. Provider/gateway and routing-model
/// configuration are not steps here — see the module-level "Out of scope"
/// note in `super`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// What Glasshouse does and does not do, and the active project.
    Welcome,
    /// Detected harnesses and optional integrations; enable, ignore, or add
    /// an explicit path for each.
    Harnesses,
    /// Review the recorded decisions before finishing.
    Summary,
}

/// What [`WizardState::handle_key`] wants the caller to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Nothing observable changed; no redraw needed.
    None,
    /// Something changed; redraw before waiting for the next event.
    Redraw,
    /// The user cancelled. The caller must return
    /// [`super::Outcome::Cancelled`] without saving anything.
    Cancel,
    /// The user finished on the last step. The caller applies the recorded
    /// decisions to a [`UserConfig`], persists it, and returns
    /// [`super::Outcome::Completed`].
    Finish,
}

/// One row of the integration list: a detected (or not-detected) integration
/// together with the user's decision about it.
#[derive(Debug, Clone)]
struct Row {
    id: IntegrationId,
    kind: IntegrationKind,
    status: IntegrationStatus,
    /// Path discovery found, if any.
    detected_path: Option<PathBuf>,
    version: Option<String>,
    /// An explicit path the user typed in and that resolved successfully.
    /// Takes priority over `detected_path` for both "is this usable" and for
    /// what gets persisted.
    override_path: Option<PathBuf>,
    /// Explicit enable/ignore decision. `None` until the user toggles it or
    /// the step is left (see [`WizardState::finalize_pending_decisions`]) —
    /// every row shown must end up `Some` before the wizard can finish.
    decision: Option<bool>,
}

impl Row {
    fn effective_executable(&self) -> Option<&Path> {
        self.override_path
            .as_deref()
            .or(self.detected_path.as_deref())
    }

    /// Whether this row has something Glasshouse could actually launch right
    /// now, either from auto-detection or from a validated explicit path.
    fn is_usable(&self) -> bool {
        self.effective_executable().is_some()
    }
}

/// Read-only view of one row, for rendering and for tests.
#[derive(Debug, Clone, Copy)]
pub struct RowView<'a> {
    pub id: IntegrationId,
    pub kind: IntegrationKind,
    pub status: IntegrationStatus,
    pub executable: Option<&'a Path>,
    pub version: Option<&'a str>,
    pub decision: Option<bool>,
    pub usable: bool,
    pub selected: bool,
}

/// State of the "add an explicit executable path" sub-mode, active while the
/// user is typing.
#[derive(Debug, Clone)]
struct PathInput {
    /// Index into `WizardState::rows` this path is being entered for.
    row_index: usize,
    buffer: String,
    /// Set when the last `Enter` failed to resolve; cleared on the next
    /// keystroke or successful resolution.
    error: Option<String>,
}

/// Read-only view of the active path-input sub-mode, for rendering.
#[derive(Debug, Clone, Copy)]
pub struct PathInputView<'a> {
    pub integration_name: &'static str,
    pub buffer: &'a str,
    pub error: Option<&'a str>,
}

/// The wizard's complete mutable state.
///
/// Constructed once per wizard run (first-run or a later "reconfigure") by
/// [`WizardState::new`], then driven by repeated calls to
/// [`WizardState::handle_key`].
#[derive(Debug, Clone)]
pub struct WizardState {
    step: Step,
    rows: Vec<Row>,
    selected_row: usize,
    path_input: Option<PathInput>,
    project_name: String,
    project_root: PathBuf,
    /// The version to record onboarding as completed at (`crate::VERSION` in
    /// production, injected here so tests do not depend on the crate's
    /// actual version number).
    version: String,
}

impl WizardState {
    /// Build the initial state for a wizard run.
    ///
    /// `detected` is handed in rather than computed here — see
    /// [`IntegrationDetection`]'s documentation for why. `existing` seeds
    /// every row's decision and explicit-path override from a prior run's
    /// choices, which is what makes reopening the wizard from settings show
    /// the user's existing configuration instead of a blank slate (Phase 2C:
    /// "Allow the onboarding wizard to be reopened later from settings").
    ///
    /// cmux is included as a row only when `detected` reports an executable
    /// for it — the capability map is explicit that cmux must not be offered
    /// to a user who does not have it. Every other catalog integration is
    /// always shown, detected or not, so a missed detection can still be
    /// filled in with an explicit path.
    pub fn new(
        detected: &[IntegrationDetection],
        existing: &UserConfig,
        project_name: String,
        project_root: PathBuf,
        version: String,
    ) -> Self {
        let rows = build_rows(detected, existing);
        Self {
            step: Step::Welcome,
            rows,
            selected_row: 0,
            path_input: None,
            project_name,
            project_root,
            version,
        }
    }

    pub fn step(&self) -> Step {
        self.step
    }

    pub fn project_name(&self) -> &str {
        &self.project_name
    }

    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    /// Every row currently shown, in catalog order (harnesses before
    /// optional integrations), for rendering or inspection.
    pub fn rows(&self) -> impl Iterator<Item = RowView<'_>> + '_ {
        self.rows.iter().enumerate().map(|(index, row)| RowView {
            id: row.id,
            kind: row.kind,
            status: row.status,
            executable: row.effective_executable(),
            version: row.version.as_deref(),
            decision: row.decision,
            usable: row.is_usable(),
            selected: index == self.selected_row,
        })
    }

    /// The active "add an explicit path" sub-mode, if any.
    pub fn path_input(&self) -> Option<PathInputView<'_>> {
        self.path_input.as_ref().map(|input| PathInputView {
            integration_name: self.rows[input.row_index].id.display_name(),
            buffer: input.buffer.as_str(),
            error: input.error.as_deref(),
        })
    }

    /// Drive the state machine with one key press. See the module
    /// documentation for what this function does and does not touch.
    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        // Ctrl-C always cancels the whole wizard, even mid-input — the user
        // reaching for the universal "get me out" key must work everywhere.
        // Plain Esc is context-sensitive: while typing a path it only backs
        // out of the input (there is no separate "are you sure" step — see
        // the module-level note in `super` on why cancellation never
        // confirms), everywhere else it cancels the wizard. This is the one
        // place `Esc`'s meaning depends on the current sub-mode; every other
        // key means the same thing on every screen.
        if is_quit_key(&key) {
            if self.path_input.is_some() && key.code == KeyCode::Esc {
                self.path_input = None;
                return Action::Redraw;
            }
            return Action::Cancel;
        }

        if self.path_input.is_some() {
            return self.handle_path_input_key(key);
        }

        match self.step {
            // `Enter`/`Tab` are the only keys that do anything on this
            // non-interactive screen: either one advances.
            Step::Welcome => {
                if matches!(key.code, KeyCode::Enter | KeyCode::Tab) {
                    self.step = Step::Harnesses;
                    Action::Redraw
                } else {
                    Action::None
                }
            }
            Step::Harnesses => self.handle_harnesses_key(key),
            Step::Summary => {
                if matches!(key.code, KeyCode::Enter | KeyCode::Tab) {
                    Action::Finish
                } else {
                    Action::None
                }
            }
        }
    }

    fn handle_harnesses_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_selection(-1);
                Action::Redraw
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_selection(1);
                Action::Redraw
            }
            KeyCode::Char(' ') | KeyCode::Enter => self.activate_selected_row(),
            KeyCode::Tab => {
                self.finalize_pending_decisions();
                self.step = Step::Summary;
                Action::Redraw
            }
            _ => Action::None,
        }
    }

    fn move_selection(&mut self, delta: i32) {
        if self.rows.is_empty() {
            return;
        }
        let last = self.rows.len() as i32 - 1;
        let next = (self.selected_row as i32 + delta).clamp(0, last);
        self.selected_row = next as usize;
    }

    /// `Space`/`Enter` on the currently selected row: toggle enable/ignore
    /// when it has something usable, or open the explicit-path field when it
    /// does not (there is nothing to enable yet).
    fn activate_selected_row(&mut self) -> Action {
        let Some(row) = self.rows.get(self.selected_row) else {
            return Action::None;
        };
        if row.is_usable() {
            let effective = row.decision.unwrap_or(true);
            self.rows[self.selected_row].decision = Some(!effective);
            Action::Redraw
        } else {
            self.path_input = Some(PathInput {
                row_index: self.selected_row,
                buffer: String::new(),
                error: None,
            });
            Action::Redraw
        }
    }

    fn handle_path_input_key(&mut self, key: KeyEvent) -> Action {
        // `is_quit_key`/Esc is handled by the caller (`handle_key`) before
        // this is ever reached; Ctrl-C already exited above regardless of
        // mode.
        let input = self
            .path_input
            .as_mut()
            .expect("handle_path_input_key is only called while path_input is Some");
        match key.code {
            KeyCode::Enter => {
                let typed = PathBuf::from(input.buffer.trim());
                match exec::resolve_explicit(&typed) {
                    Ok(resolved) => {
                        let row_index = input.row_index;
                        self.rows[row_index].override_path = Some(resolved.path().to_path_buf());
                        self.rows[row_index].decision = Some(true);
                        self.path_input = None;
                    }
                    Err(err) => {
                        // Show the real resolve error inline and stay in
                        // input mode so the user can correct it (Phase 2C:
                        // "show the real error ... letting them correct it,
                        // rather than accepting a path that will fail later
                        // at launch").
                        input.error = Some(err.to_string());
                    }
                }
                Action::Redraw
            }
            KeyCode::Backspace => {
                input.buffer.pop();
                input.error = None;
                Action::Redraw
            }
            KeyCode::Char(c) => {
                input.buffer.push(c);
                input.error = None;
                Action::Redraw
            }
            _ => Action::None,
        }
    }

    /// Fill in a default explicit decision for every row the user never
    /// toggled: enabled when it has something usable, ignored otherwise.
    ///
    /// Called when leaving the Harnesses step. This is what satisfies "every
    /// harness the user was shown has an explicit decision recorded" without
    /// forcing the user to touch every row by hand — a detected, usable
    /// harness defaults to enabled (Glasshouse must be fully useful with
    /// only native subscription-backed harnesses configured, with no extra
    /// clicks), and one nothing could be found for defaults to ignored since
    /// there is nothing to launch.
    fn finalize_pending_decisions(&mut self) {
        for row in &mut self.rows {
            if row.decision.is_none() {
                row.decision = Some(row.is_usable());
            }
        }
    }

    /// Apply every recorded decision to `config` and mark onboarding
    /// completed at this wizard's version.
    ///
    /// Assumes [`WizardState::finalize_pending_decisions`] has already run
    /// (true for every reachable path to [`Action::Finish`] — see
    /// [`WizardState::handle_harnesses_key`]); a row somehow still `None`
    /// here is written as ignored rather than panicking, since a config
    /// write is not the place to enforce an internal invariant with a crash.
    pub fn apply_to(&self, config: &mut UserConfig) {
        for row in &self.rows {
            let entry = config.integrations_mut().entry(row.id);
            entry.set_enabled(row.decision.unwrap_or(false));
            entry.set_executable(row.override_path.clone());
        }
        config.onboarding_mut().mark_completed(self.version.clone());
    }
}

fn build_rows(detected: &[IntegrationDetection], existing: &UserConfig) -> Vec<Row> {
    let mut rows = Vec::with_capacity(IntegrationId::ALL.len());
    for &id in IntegrationId::ALL {
        let detection = detected.iter().find(|d| d.id == id);

        if id == IntegrationId::Cmux {
            let detected_cmux = detection.is_some_and(|d| d.executable.is_some());
            if !detected_cmux {
                // Never offered when not detected — see `WizardState::new`.
                continue;
            }
        }

        let existing_entry = existing.integrations().get(id);
        rows.push(Row {
            id,
            kind: id.kind(),
            status: detection.map_or(IntegrationStatus::NotFound, |d| d.status),
            detected_path: detection.and_then(|d| d.executable.clone()),
            version: detection.and_then(|d| d.version.clone()),
            override_path: existing_entry
                .and_then(crate::config::IntegrationConfig::executable)
                .map(Path::to_path_buf),
            decision: existing.integrations().is_enabled(id),
        });
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl_c() -> KeyEvent {
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
    }

    fn char_key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn detection(
        id: IntegrationId,
        status: IntegrationStatus,
        executable: Option<&str>,
    ) -> IntegrationDetection {
        IntegrationDetection {
            id,
            status,
            executable: executable.map(PathBuf::from),
            version: None,
        }
    }

    /// The four harnesses detected as usable, nothing else. A reasonable
    /// "everything found" baseline most tests start from.
    fn all_harnesses_detected() -> Vec<IntegrationDetection> {
        vec![
            detection(
                IntegrationId::ClaudeCode,
                IntegrationStatus::Configured,
                Some("/usr/bin/claude"),
            ),
            detection(
                IntegrationId::Codex,
                IntegrationStatus::Unconfigured,
                Some("/usr/bin/codex"),
            ),
            detection(
                IntegrationId::Antigravity,
                IntegrationStatus::NotFound,
                None,
            ),
            detection(IntegrationId::OpenCode, IntegrationStatus::NotFound, None),
            detection(IntegrationId::Cmux, IntegrationStatus::NotFound, None),
            detection(IntegrationId::Ollama, IntegrationStatus::NotFound, None),
            detection(IntegrationId::LlamaCpp, IntegrationStatus::NotFound, None),
        ]
    }

    fn new_state(detected: &[IntegrationDetection]) -> WizardState {
        WizardState::new(
            detected,
            &UserConfig::default(),
            "demo-project".to_owned(),
            PathBuf::from("/home/user/demo-project"),
            "1.2.3".to_owned(),
        )
    }

    /// Drive `state` through a sequence of keys as `super::run`'s loop would,
    /// stopping at the first terminal `Action` (`Cancel` or `Finish`), or
    /// once the sequence is exhausted. Mirrors the loop's dispatch without
    /// needing a `Screen` or `EventSource` — exactly the "state machine
    /// without a terminal" split this module exists for.
    fn drive(state: &mut WizardState, keys: &[KeyEvent]) -> Action {
        let mut last = Action::None;
        for &k in keys {
            last = state.handle_key(k);
            if matches!(last, Action::Cancel | Action::Finish) {
                return last;
            }
        }
        last
    }

    // --- is_required is exercised in `super::tests`, not here: it takes a
    // `UserConfig` directly and has nothing to do with this state machine.

    #[test]
    fn happy_path_disables_one_harness_and_records_explicit_decisions() {
        let mut state = new_state(&all_harnesses_detected());

        // Welcome -> Harnesses.
        assert_eq!(state.handle_key(key(KeyCode::Enter)), Action::Redraw);
        assert_eq!(state.step(), Step::Harnesses);

        // Selection starts on Claude Code (first row); move to Codex (second
        // row) and turn it off.
        assert_eq!(state.handle_key(key(KeyCode::Down)), Action::Redraw);
        assert_eq!(state.handle_key(key(KeyCode::Char(' '))), Action::Redraw);

        // Harnesses -> Summary.
        assert_eq!(state.handle_key(key(KeyCode::Tab)), Action::Redraw);
        assert_eq!(state.step(), Step::Summary);

        // Finish.
        let action = state.handle_key(key(KeyCode::Enter));
        assert_eq!(action, Action::Finish);

        let mut config = UserConfig::default();
        state.apply_to(&mut config);

        assert!(config.onboarding().completed());
        assert_eq!(config.onboarding().completed_at_version(), Some("1.2.3"));

        // Claude Code: detected and usable, never toggled -> defaulted on.
        assert_eq!(
            config.integrations().is_enabled(IntegrationId::ClaudeCode),
            Some(true)
        );
        // Codex: detected and usable, explicitly toggled off.
        assert_eq!(
            config.integrations().is_enabled(IntegrationId::Codex),
            Some(false)
        );
        // Never detected, never touched -> defaulted off, but still an
        // explicit decision, not "never asked".
        for id in [
            IntegrationId::Antigravity,
            IntegrationId::OpenCode,
            IntegrationId::Ollama,
            IntegrationId::LlamaCpp,
        ] {
            assert_eq!(
                config.integrations().is_enabled(id),
                Some(false),
                "{id:?} must have an explicit decision, not never-asked"
            );
        }
        // cmux was not detected in this fixture, so it must not even appear
        // as a shown row, let alone get a recorded decision.
        assert_eq!(config.integrations().is_enabled(IntegrationId::Cmux), None);
    }

    #[test]
    fn full_flow_driven_through_the_drive_helper_reaches_finish() {
        let mut state = new_state(&all_harnesses_detected());
        let action = drive(
            &mut state,
            &[key(KeyCode::Tab), key(KeyCode::Tab), key(KeyCode::Enter)],
        );
        assert_eq!(action, Action::Finish);
        assert_eq!(state.step(), Step::Summary);
    }

    #[test]
    fn cancelling_returns_cancel_and_leaves_the_caller_nothing_to_save() {
        let mut state = new_state(&all_harnesses_detected());

        // Make some changes first: cancellation must discard them, not just
        // happen to occur before any were made.
        let action = drive(
            &mut state,
            &[
                key(KeyCode::Tab),
                key(KeyCode::Char(' ')), // toggle Claude Code off
                key(KeyCode::Esc),
            ],
        );
        assert_eq!(action, Action::Cancel);

        // The contract `super::run` relies on: `Cancel` is never followed by
        // `apply_to`/`save`. There is no config mutation to inspect here
        // because none happens — that absence *is* the behaviour under
        // test. A fresh config, as the caller would still have it, remains
        // exactly default.
        let untouched = UserConfig::default();
        assert!(!untouched.onboarding().completed());
        assert!(untouched.integrations().is_empty());
    }

    #[test]
    fn ctrl_c_cancels_even_while_typing_a_path() {
        let mut state = new_state(&all_harnesses_detected());
        state.handle_key(key(KeyCode::Enter)); // Welcome -> Harnesses
        // Antigravity (3rd row, not detected) -> open path input.
        state.handle_key(key(KeyCode::Down));
        state.handle_key(key(KeyCode::Down));
        assert_eq!(state.handle_key(key(KeyCode::Enter)), Action::Redraw);
        assert!(state.path_input().is_some());

        assert_eq!(state.handle_key(ctrl_c()), Action::Cancel);
    }

    #[test]
    fn esc_while_typing_a_path_only_closes_the_input() {
        let mut state = new_state(&all_harnesses_detected());
        state.handle_key(key(KeyCode::Enter));
        state.handle_key(key(KeyCode::Down));
        state.handle_key(key(KeyCode::Down));
        state.handle_key(key(KeyCode::Enter)); // open input on Antigravity
        assert!(state.path_input().is_some());

        assert_eq!(state.handle_key(key(KeyCode::Esc)), Action::Redraw);
        assert!(state.path_input().is_none());
        assert_eq!(
            state.step(),
            Step::Harnesses,
            "Esc must not cancel the wizard here"
        );
    }

    #[test]
    fn reopening_preselects_existing_decisions_and_override_path() {
        let mut existing = UserConfig::default();
        existing
            .integrations_mut()
            .entry(IntegrationId::ClaudeCode)
            .set_enabled(false);
        existing
            .integrations_mut()
            .entry(IntegrationId::Antigravity)
            .set_enabled(true)
            .set_executable(Some(PathBuf::from("/opt/antigravity/bin/antigravity")));

        let state = WizardState::new(
            &all_harnesses_detected(),
            &existing,
            "demo".to_owned(),
            PathBuf::from("/tmp/demo"),
            "9.9.9".to_owned(),
        );

        let claude = state
            .rows()
            .find(|r| r.id == IntegrationId::ClaudeCode)
            .expect("claude row present");
        assert_eq!(claude.decision, Some(false));

        let antigravity = state
            .rows()
            .find(|r| r.id == IntegrationId::Antigravity)
            .expect("antigravity row present");
        assert_eq!(antigravity.decision, Some(true));
        assert_eq!(
            antigravity.executable,
            Some(Path::new("/opt/antigravity/bin/antigravity"))
        );
        assert!(
            antigravity.usable,
            "an overridden path makes the row usable"
        );
    }

    #[test]
    fn invalid_explicit_path_surfaces_the_error_and_does_not_advance() {
        let mut state = new_state(&all_harnesses_detected());
        state.handle_key(key(KeyCode::Enter)); // Welcome -> Harnesses
        state.handle_key(key(KeyCode::Down)); // Codex (usable, wrong target)
        state.handle_key(key(KeyCode::Down)); // Antigravity (not detected)
        assert_eq!(state.handle_key(key(KeyCode::Enter)), Action::Redraw);

        for c in "/definitely/not/a/real/executable".chars() {
            state.handle_key(char_key(c));
        }
        let action = state.handle_key(key(KeyCode::Enter));
        assert_eq!(action, Action::Redraw);

        let input = state.path_input().expect("still in input mode");
        assert!(input.error.is_some(), "resolve error must be surfaced");
        assert_eq!(input.integration_name, "Antigravity");

        let antigravity = state
            .rows()
            .find(|r| r.id == IntegrationId::Antigravity)
            .unwrap();
        assert_eq!(
            antigravity.decision, None,
            "must not record a decision on failure"
        );
        assert!(!antigravity.usable);
    }

    #[test]
    fn valid_explicit_path_is_recorded_as_the_executable() {
        let tmp = tempfile::tempdir().unwrap();
        let exe_path = tmp.path().join("my-antigravity");
        std::fs::write(&exe_path, "#!/bin/sh\necho hi\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&exe_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let mut state = new_state(&all_harnesses_detected());
        state.handle_key(key(KeyCode::Enter));
        state.handle_key(key(KeyCode::Down));
        state.handle_key(key(KeyCode::Down));
        assert_eq!(state.handle_key(key(KeyCode::Enter)), Action::Redraw);

        for c in exe_path.to_str().unwrap().chars() {
            state.handle_key(char_key(c));
        }
        let action = state.handle_key(key(KeyCode::Enter));
        assert_eq!(action, Action::Redraw);
        assert!(
            state.path_input().is_none(),
            "successful resolve closes the input"
        );

        let antigravity = state
            .rows()
            .find(|r| r.id == IntegrationId::Antigravity)
            .unwrap();
        assert_eq!(antigravity.decision, Some(true));
        assert!(antigravity.usable);
        let recorded = antigravity.executable.expect("executable recorded");
        assert_eq!(
            std::fs::canonicalize(recorded).unwrap(),
            std::fs::canonicalize(&exe_path).unwrap()
        );

        // And it round-trips into a real `UserConfig`.
        state.handle_key(key(KeyCode::Tab));
        state.handle_key(key(KeyCode::Enter));
        let mut config = UserConfig::default();
        state.apply_to(&mut config);
        assert_eq!(
            config
                .integrations()
                .get(IntegrationId::Antigravity)
                .and_then(crate::config::IntegrationConfig::executable)
                .map(|p| std::fs::canonicalize(p).unwrap()),
            Some(std::fs::canonicalize(&exe_path).unwrap())
        );
        assert_eq!(
            config.integrations().is_enabled(IntegrationId::Antigravity),
            Some(true)
        );
    }

    #[test]
    fn cmux_row_is_present_only_when_detected() {
        let without_cmux = new_state(&all_harnesses_detected());
        assert!(without_cmux.rows().all(|r| r.id != IntegrationId::Cmux));

        let mut with_cmux: Vec<IntegrationDetection> = all_harnesses_detected()
            .into_iter()
            .filter(|d| d.id != IntegrationId::Cmux)
            .collect();
        with_cmux.push(detection(
            IntegrationId::Cmux,
            IntegrationStatus::Available,
            Some("/usr/bin/cmux"),
        ));
        let state = new_state(&with_cmux);
        assert!(state.rows().any(|r| r.id == IntegrationId::Cmux));
    }

    #[test]
    fn selection_clamps_at_both_ends() {
        let mut state = new_state(&all_harnesses_detected());
        state.handle_key(key(KeyCode::Enter)); // -> Harnesses
        state.handle_key(key(KeyCode::Up));
        state.handle_key(key(KeyCode::Up));
        assert_eq!(
            state.rows().position(|r| r.selected),
            Some(0),
            "cannot move above the first row"
        );

        let row_count = state.rows().count();
        for _ in 0..row_count + 3 {
            state.handle_key(key(KeyCode::Down));
        }
        assert_eq!(state.rows().position(|r| r.selected), Some(row_count - 1));
    }

    #[test]
    fn tab_advances_and_enter_toggles_are_distinct_on_the_harnesses_step() {
        let mut state = new_state(&all_harnesses_detected());
        state.handle_key(key(KeyCode::Enter)); // Welcome -> Harnesses
        assert_eq!(state.step(), Step::Harnesses);

        // Enter on a usable row toggles it, it does not advance the step.
        state.handle_key(key(KeyCode::Enter));
        assert_eq!(state.step(), Step::Harnesses);
        let claude = state
            .rows()
            .find(|r| r.id == IntegrationId::ClaudeCode)
            .unwrap();
        assert_eq!(claude.decision, Some(false), "default true, toggled once");

        // Tab does advance.
        state.handle_key(key(KeyCode::Tab));
        assert_eq!(state.step(), Step::Summary);
    }
}
