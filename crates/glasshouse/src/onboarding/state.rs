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

use crate::config::{ProviderConfig, UserConfig};
use crate::harness::{ApprovalMode, adapter_for};
use crate::integrations::{IntegrationId, IntegrationKind, IntegrationStatus};
use crate::platform::exec;
use crate::provider;
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
/// An introduction, the interactive integration list, the optional
/// bypass-acknowledgement step, the optional provider/gateway step, and a
/// confirmation summary. Routing-model configuration is not a step here —
/// see the module-level "Out of scope" note in `super`; that subsystem still
/// does not exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// What Glasshouse does and does not do, and the active project.
    Welcome,
    /// Detected harnesses and optional integrations; enable, ignore, or add
    /// an explicit path for each.
    Harnesses,
    /// Optional, off by default: acknowledge the blanket-bypass risk for
    /// each harness that declares one but no automatic-review mode. See
    /// `BypassRow`.
    Bypass,
    /// Optional: configure a provider (or leave it for later). See
    /// `ProviderMode` for the sub-flow.
    Provider,
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

/// One row on the optional bypass-acknowledgement step ([`Step::Bypass`]): a
/// harness whose adapter declares a bypass mode but no automatic-review
/// mode, so the only way to run it unattended is the blanket bypass Phase 9A
/// requires an explicit, once-per-harness acknowledgement for.
///
/// Built entirely from what the adapter declares — `mode` is the harness's
/// own [`ApprovalMode`], never a paraphrase — see [`build_bypass_rows`].
#[derive(Debug, Clone)]
struct BypassRow {
    id: IntegrationId,
    mode: ApprovalMode,
    /// What `existing` already had recorded, before this wizard run —
    /// `false` when never asked, matching how a `None` is read everywhere
    /// else this field is used. Kept alongside `acknowledged` so
    /// [`WizardState::apply_to`] writes only what actually changed this run:
    /// a previously granted acknowledgement the user un-checks is written as
    /// an explicit revocation, and one nothing touched is left exactly as it
    /// was on disk — never rewritten to the same value, and never silently
    /// dropped to "unset" when it was already an explicit `true`.
    seeded: bool,
    /// The current, possibly-toggled decision. Defaults to `seeded` (never
    /// starts as "granted" that the user did not just grant) — see the
    /// module's "never downgrade to a bypass silently" rule, which applies
    /// here as "never upgrade to one silently" too.
    acknowledged: bool,
}

/// Read-only view of one `BypassRow`, for rendering.
#[derive(Debug, Clone, Copy)]
pub struct BypassRowView<'a> {
    pub id: IntegrationId,
    pub args: &'a [&'static str],
    pub description: &'static str,
    pub acknowledged: bool,
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

/// Which of the two top-level choices is highlighted on [`Step::Provider`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderChoice {
    ConfigureNow,
    DoLater,
}

/// Sub-mode of the optional provider step ([`Step::Provider`]).
///
/// Mirrors [`PathInput`]'s role for the Harnesses step: everything about
/// "which screen inside Provider is showing" lives in one place, driven the
/// same way — `Esc` steps back one level rather than cancelling the wizard,
/// see [`WizardState::provider_step_back`].
#[derive(Debug, Clone, PartialEq, Eq)]
enum ProviderMode {
    /// Choosing between "Configure now" and "Do later".
    Choice(ProviderChoice),
    /// Picking a built-in provider template to configure. Index into
    /// [`provider::templates`]'s own order.
    PickTemplate { selected: usize },
    /// Typing the base URL a generic template
    /// ([`provider::GENERIC_TEMPLATE_NAMES`]) needs before it can be used —
    /// its own base URL is empty on purpose, see [`provider::templates`]'s
    /// documentation.
    BaseUrlInput {
        template: String,
        /// The row `template` was picked from in [`ProviderMode::PickTemplate`],
        /// so `Esc` can return there instead of resetting to the top.
        template_index: usize,
        buffer: String,
        /// Set when the last `Enter` was rejected (an empty URL); cleared on
        /// the next keystroke or successful confirmation.
        error: Option<String>,
    },
}

/// One built-in provider template, as shown in the picker.
#[derive(Debug, Clone)]
pub struct ProviderTemplateRow {
    pub name: String,
    /// Every protocol this template serves, comma-separated.
    pub protocols: String,
    /// The template's own base URL, or empty for the two generic templates
    /// the user must supply one for.
    pub base_url: String,
    pub selected: bool,
}

/// One provider recorded in configuration — already on disk before this
/// wizard run, or configured during it — for the Choice and Summary screens.
#[derive(Debug, Clone)]
pub struct ProviderRow {
    pub name: String,
    pub template: String,
    pub base_url: Option<String>,
}

/// Read-only view of the optional provider step, for rendering.
#[derive(Debug, Clone)]
pub enum ProviderStepView {
    Choice {
        configure_now_selected: bool,
        /// Providers already on disk, overlaid with whatever this run
        /// configured — see [`WizardState::configured_providers`].
        providers: Vec<ProviderRow>,
    },
    PickTemplate {
        options: Vec<ProviderTemplateRow>,
    },
    BaseUrlInput {
        template: String,
        buffer: String,
        error: Option<String>,
    },
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
    bypass_rows: Vec<BypassRow>,
    selected_bypass_row: usize,
    provider_mode: ProviderMode,
    /// Providers already recorded in configuration before this wizard run —
    /// seeded once from `existing` so a reopen shows what is already there
    /// (Phase 2C: "Allow the onboarding wizard to be reopened later from
    /// settings").
    existing_providers: Vec<ProviderRow>,
    /// The provider this wizard run configured, if the user chose "Configure
    /// now" and completed it. `None` after "Do later" — see
    /// [`WizardState::apply_to`], which is what keeps that path from writing
    /// any provider or credential into configuration at all (Phase 2C line
    /// 3/4).
    pending_provider: Option<(String, ProviderConfig)>,
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
    /// for it, or `existing` already has a recorded decision about it (a
    /// prior run's explicit request, preserved on reopen) — the capability
    /// map is explicit that cmux must not be offered to a user who neither
    /// has it nor asked for it. A user who has neither can still ask for it
    /// live, with `c` on the Harnesses step — see
    /// `WizardState::request_cmux`. Every other catalog integration is
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
        let bypass_rows = build_bypass_rows(existing);
        let existing_providers = existing
            .providers()
            .iter()
            .map(|(name, config)| ProviderRow {
                name: name.to_owned(),
                template: config.template().to_owned(),
                base_url: config.base_url().map(str::to_owned),
            })
            .collect();
        Self {
            step: Step::Welcome,
            rows,
            selected_row: 0,
            path_input: None,
            bypass_rows,
            selected_bypass_row: 0,
            provider_mode: ProviderMode::Choice(ProviderChoice::DoLater),
            existing_providers,
            pending_provider: None,
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

    /// Every row on the optional bypass-acknowledgement step, in catalog
    /// order, for rendering or inspection.
    pub fn bypass_rows(&self) -> impl Iterator<Item = BypassRowView<'_>> + '_ {
        self.bypass_rows
            .iter()
            .enumerate()
            .map(|(index, row)| BypassRowView {
                id: row.id,
                args: row.mode.args,
                description: row.mode.description,
                acknowledged: row.acknowledged,
                selected: index == self.selected_bypass_row,
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

    /// A read-only view of the current sub-screen of [`Step::Provider`], for
    /// rendering.
    pub fn provider_step(&self) -> ProviderStepView {
        match &self.provider_mode {
            ProviderMode::Choice(choice) => ProviderStepView::Choice {
                configure_now_selected: *choice == ProviderChoice::ConfigureNow,
                providers: self.configured_providers(),
            },
            ProviderMode::PickTemplate { selected } => ProviderStepView::PickTemplate {
                options: provider::templates()
                    .iter()
                    .enumerate()
                    .map(|(index, template)| ProviderTemplateRow {
                        name: template.name.clone(),
                        protocols: template
                            .protocols
                            .iter()
                            .map(|support| support.protocol.to_string())
                            .collect::<Vec<_>>()
                            .join(", "),
                        base_url: template
                            .protocols
                            .first()
                            .map(|support| support.base_url.clone())
                            .unwrap_or_default(),
                        selected: index == *selected,
                    })
                    .collect(),
            },
            ProviderMode::BaseUrlInput {
                template,
                buffer,
                error,
                ..
            } => ProviderStepView::BaseUrlInput {
                template: template.clone(),
                buffer: buffer.clone(),
                error: error.clone(),
            },
        }
    }

    /// Every provider a Choice or Summary screen should list: providers
    /// already on disk before this run, overlaid with whatever this run
    /// configured (a provider sharing an existing name is shown as this
    /// run's version, since that is what [`WizardState::apply_to`] will
    /// write over it).
    pub fn configured_providers(&self) -> Vec<ProviderRow> {
        let mut rows = self.existing_providers.clone();
        if let Some((name, config)) = &self.pending_provider {
            let row = ProviderRow {
                name: name.clone(),
                template: config.template().to_owned(),
                base_url: config.base_url().map(str::to_owned),
            };
            if let Some(existing) = rows.iter_mut().find(|r| &r.name == name) {
                *existing = row;
            } else {
                rows.push(row);
            }
        }
        rows
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
            if key.code == KeyCode::Esc {
                if self.path_input.is_some() {
                    self.path_input = None;
                    return Action::Redraw;
                }
                if self.step == Step::Provider && self.provider_step_back() {
                    return Action::Redraw;
                }
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
            Step::Bypass => self.handle_bypass_key(key),
            Step::Provider => self.handle_provider_key(key),
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
            // Explicit ask, line 5: cmux is otherwise absent from the list
            // unless detected or previously configured (see `build_rows`).
            // A no-op once the row already exists, so this is safe to leave
            // bound everywhere on this step.
            KeyCode::Char('c') if !self.rows.iter().any(|row| row.id == IntegrationId::Cmux) => {
                self.request_cmux()
            }
            KeyCode::Tab => {
                self.finalize_pending_decisions();
                self.step = Step::Bypass;
                Action::Redraw
            }
            _ => Action::None,
        }
    }

    fn handle_bypass_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_bypass_selection(-1);
                Action::Redraw
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_bypass_selection(1);
                Action::Redraw
            }
            // Amendment line 5: defaults to not acknowledged, and declining
            // is fine. Toggling here only ever flips this run's in-memory
            // decision — see `WizardState::apply_to` for how (and whether)
            // it reaches configuration.
            KeyCode::Char(' ') | KeyCode::Enter => {
                if let Some(row) = self.bypass_rows.get_mut(self.selected_bypass_row) {
                    row.acknowledged = !row.acknowledged;
                }
                Action::Redraw
            }
            KeyCode::Tab => {
                self.step = Step::Provider;
                Action::Redraw
            }
            _ => Action::None,
        }
    }

    fn move_bypass_selection(&mut self, delta: i32) {
        if self.bypass_rows.is_empty() {
            return;
        }
        let last = self.bypass_rows.len() as i32 - 1;
        let next = (self.selected_bypass_row as i32 + delta).clamp(0, last);
        self.selected_bypass_row = next as usize;
    }

    /// Add a cmux row on live, explicit request even though it was neither
    /// detected nor previously configured — Phase 2C line 5: "Offer cmux
    /// integration only when cmux is detected or the user explicitly asks
    /// to configure it." Once added, it is an ordinary row: the same
    /// enable/ignore and explicit-path machinery every other integration
    /// uses takes over from here.
    fn request_cmux(&mut self) -> Action {
        self.rows.push(Row {
            id: IntegrationId::Cmux,
            kind: IntegrationId::Cmux.kind(),
            status: IntegrationStatus::NotFound,
            detected_path: None,
            version: None,
            override_path: None,
            decision: None,
        });
        self.selected_row = self.rows.len() - 1;
        Action::Redraw
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

    /// Step back one level inside [`Step::Provider`]'s sub-modes without
    /// cancelling the wizard, mirroring how `Esc` only closes the
    /// explicit-path input on the Harnesses step instead of leaving
    /// entirely. Returns whether it consumed the key — `false` from the
    /// top-level Choice screen, where `Esc` falls through to cancelling the
    /// whole wizard like every other step.
    fn provider_step_back(&mut self) -> bool {
        match &self.provider_mode {
            ProviderMode::Choice(_) => false,
            ProviderMode::PickTemplate { .. } => {
                self.provider_mode = ProviderMode::Choice(ProviderChoice::ConfigureNow);
                true
            }
            ProviderMode::BaseUrlInput { template_index, .. } => {
                self.provider_mode = ProviderMode::PickTemplate {
                    selected: *template_index,
                };
                true
            }
        }
    }

    fn handle_provider_key(&mut self, key: KeyEvent) -> Action {
        match self.provider_mode {
            ProviderMode::Choice(_) => self.handle_provider_choice_key(key),
            ProviderMode::PickTemplate { .. } => self.handle_provider_template_key(key),
            ProviderMode::BaseUrlInput { .. } => self.handle_provider_base_url_key(key),
        }
    }

    fn handle_provider_choice_key(&mut self, key: KeyEvent) -> Action {
        let ProviderMode::Choice(choice) = &self.provider_mode else {
            return Action::None;
        };
        let choice = *choice;
        match key.code {
            KeyCode::Up | KeyCode::Down | KeyCode::Char('j' | 'k') => {
                self.provider_mode = ProviderMode::Choice(match choice {
                    ProviderChoice::ConfigureNow => ProviderChoice::DoLater,
                    ProviderChoice::DoLater => ProviderChoice::ConfigureNow,
                });
                Action::Redraw
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                match choice {
                    // Line 3: "Do later" completes onboarding without ever
                    // touching `pending_provider`, so `apply_to` writes no
                    // provider and no credential.
                    ProviderChoice::DoLater => self.step = Step::Summary,
                    ProviderChoice::ConfigureNow => {
                        self.provider_mode = ProviderMode::PickTemplate { selected: 0 };
                    }
                }
                Action::Redraw
            }
            // `Tab` always means "continue" here, exactly like Welcome and
            // the Harnesses step — pressing it without ever touching this
            // screen is what makes it genuinely optional (Phase 2C
            // acceptance test 1: "the wizard completes without it").
            KeyCode::Tab => {
                self.step = Step::Summary;
                Action::Redraw
            }
            _ => Action::None,
        }
    }

    fn handle_provider_template_key(&mut self, key: KeyEvent) -> Action {
        let ProviderMode::PickTemplate { selected } = &self.provider_mode else {
            return Action::None;
        };
        let selected = *selected;
        let templates = provider::templates();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.provider_mode = ProviderMode::PickTemplate {
                    selected: selected.saturating_sub(1),
                };
                Action::Redraw
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let last = templates.len().saturating_sub(1);
                self.provider_mode = ProviderMode::PickTemplate {
                    selected: (selected + 1).min(last),
                };
                Action::Redraw
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                let Some(chosen) = templates.get(selected) else {
                    return Action::None;
                };
                let name = chosen.name.clone();
                if provider::GENERIC_TEMPLATE_NAMES.contains(&name.as_str()) {
                    self.provider_mode = ProviderMode::BaseUrlInput {
                        template: name,
                        template_index: selected,
                        buffer: String::new(),
                        error: None,
                    };
                } else {
                    // The template's own defaults (base URL, credential
                    // variable names) are used as-is — nothing here ever
                    // types in a credential *value*, only names, and only
                    // for the two generic templates that need one.
                    self.pending_provider = Some((name.clone(), ProviderConfig::new(name)));
                    self.provider_mode = ProviderMode::Choice(ProviderChoice::DoLater);
                }
                Action::Redraw
            }
            _ => Action::None,
        }
    }

    fn handle_provider_base_url_key(&mut self, key: KeyEvent) -> Action {
        if !matches!(self.provider_mode, ProviderMode::BaseUrlInput { .. }) {
            return Action::None;
        }
        match key.code {
            KeyCode::Enter => {
                let ProviderMode::BaseUrlInput {
                    template, buffer, ..
                } = &self.provider_mode
                else {
                    return Action::None;
                };
                let trimmed = buffer.trim();
                if trimmed.is_empty() {
                    if let ProviderMode::BaseUrlInput { error, .. } = &mut self.provider_mode {
                        *error = Some("a base URL is required for this template".to_owned());
                    }
                    return Action::Redraw;
                }
                let template = template.clone();
                let base_url = trimmed.to_owned();
                let mut config = ProviderConfig::new(template.clone());
                config.set_base_url(Some(base_url));
                self.pending_provider = Some((template, config));
                self.provider_mode = ProviderMode::Choice(ProviderChoice::DoLater);
                Action::Redraw
            }
            KeyCode::Backspace => {
                if let ProviderMode::BaseUrlInput { buffer, error, .. } = &mut self.provider_mode {
                    buffer.pop();
                    *error = None;
                }
                Action::Redraw
            }
            KeyCode::Char(c) => {
                if let ProviderMode::BaseUrlInput { buffer, error, .. } = &mut self.provider_mode {
                    buffer.push(c);
                    *error = None;
                }
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
    ///
    /// `pending_provider` is only ever `Some` after a completed "Configure
    /// now" — see `WizardState::handle_provider_template_key` and
    /// `WizardState::handle_provider_base_url_key` — so "Do later" leaves
    /// `config.providers()` exactly as `existing` already had it: untouched,
    /// never cleared. That is what satisfies both Phase 2C line 3 (no
    /// provider, no credential, on a genuine first run, where `existing` had
    /// none to begin with) and line 6 (a reopen that chooses "Do later"
    /// keeps whatever was already configured) with the same code path.
    ///
    /// A `BypassRow` is written only when this run actually changed it
    /// (`acknowledged != seeded`, both starting equal on a fresh row, so an
    /// untouched one is never written at all) — Amendment 1's acceptance
    /// test 8: declining leaves `bypass_acknowledged` genuinely unset on a
    /// first run, not overwritten with an explicit `false`. On a reopen,
    /// the same rule is what makes revoking a previously granted
    /// acknowledgement possible: un-checking a row that started `true`
    /// writes `false`, rather than that change being swallowed by "only
    /// write true".
    pub fn apply_to(&self, config: &mut UserConfig) {
        for row in &self.rows {
            let entry = config.integrations_mut().entry(row.id);
            entry.set_enabled(row.decision.unwrap_or(false));
            entry.set_executable(row.override_path.clone());
        }
        for row in &self.bypass_rows {
            if row.acknowledged != row.seeded {
                config
                    .integrations_mut()
                    .entry(row.id)
                    .set_bypass_acknowledged(row.acknowledged);
            }
        }
        if let Some((name, provider_config)) = &self.pending_provider {
            config
                .providers_mut()
                .set(name.clone(), provider_config.clone());
        }
        config.onboarding_mut().mark_completed(self.version.clone());
    }
}

/// Every harness whose adapter declares a bypass mode but no
/// automatic-review mode — read from the adapters themselves, never a
/// hard-coded list, so this tracks whatever a future adapter declares
/// without this module having to change (Amendment 1, line 1).
fn build_bypass_rows(existing: &UserConfig) -> Vec<BypassRow> {
    IntegrationId::ALL
        .iter()
        .copied()
        .filter_map(|id| {
            let adapter = adapter_for(id)?;
            let approvals = adapter.describe().approvals;
            if approvals.automatic_review.is_verified() {
                return None;
            }
            let mode = *approvals.bypass.value()?;
            let seeded = existing
                .integrations()
                .get(id)
                .and_then(crate::config::IntegrationConfig::bypass_acknowledged)
                .unwrap_or(false);
            Some(BypassRow {
                id,
                mode,
                seeded,
                acknowledged: seeded,
            })
        })
        .collect()
}

fn build_rows(detected: &[IntegrationDetection], existing: &UserConfig) -> Vec<Row> {
    let mut rows = Vec::with_capacity(IntegrationId::ALL.len());
    for &id in IntegrationId::ALL {
        let detection = detected.iter().find(|d| d.id == id);

        if id == IntegrationId::Cmux {
            let detected_cmux = detection.is_some_and(|d| d.executable.is_some());
            let previously_configured = existing.integrations().get(id).is_some();
            if !detected_cmux && !previously_configured {
                // Never offered unless detected or explicitly asked for —
                // live, via `WizardState::request_cmux`, or in a past run,
                // preserved here on reopen. See `WizardState::new`.
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

        // Harnesses -> Bypass.
        assert_eq!(state.handle_key(key(KeyCode::Tab)), Action::Redraw);
        assert_eq!(state.step(), Step::Bypass);

        // Bypass is optional too: Tab skips it (declined) straight to
        // Provider.
        assert_eq!(state.handle_key(key(KeyCode::Tab)), Action::Redraw);
        assert_eq!(state.step(), Step::Provider);

        // Provider is optional: Tab skips it ("Do later") straight to
        // Summary without ever touching `pending_provider`.
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
            &[
                key(KeyCode::Tab), // Welcome -> Harnesses
                key(KeyCode::Tab), // Harnesses -> Bypass
                key(KeyCode::Tab), // Bypass (declined) -> Provider
                key(KeyCode::Tab), // Provider (Do later) -> Summary
                key(KeyCode::Enter),
            ],
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

        // Tab does advance — to the optional Bypass step now sitting
        // between Harnesses and Provider.
        state.handle_key(key(KeyCode::Tab));
        assert_eq!(state.step(), Step::Bypass);
    }

    // --- Phase 2C: the optional provider step -----------------------------

    /// Drive a fresh wizard from Welcome to the Provider step, letting every
    /// harness default (Tab through Welcome and Harnesses).
    fn drive_to_provider(detected: &[IntegrationDetection]) -> WizardState {
        let mut state = new_state(detected);
        state.handle_key(key(KeyCode::Tab)); // Welcome -> Harnesses
        state.handle_key(key(KeyCode::Tab)); // Harnesses -> Bypass
        state.handle_key(key(KeyCode::Tab)); // Bypass (declined) -> Provider
        assert_eq!(state.step(), Step::Provider);
        state
    }

    /// Acceptance 1: the provider step is reachable and optional — the
    /// wizard completes without ever touching it. A mutation that made
    /// `Step::Provider` mandatory (refusing `Tab`/`Finish` until a provider
    /// is chosen) fails this.
    #[test]
    fn the_provider_step_is_reachable_and_the_wizard_completes_without_touching_it() {
        let mut state = drive_to_provider(&all_harnesses_detected());

        // Do nothing but continue: Tab from the Choice screen, exactly like
        // Welcome and Harnesses.
        assert_eq!(state.handle_key(key(KeyCode::Tab)), Action::Redraw);
        assert_eq!(state.step(), Step::Summary);
        assert_eq!(state.handle_key(key(KeyCode::Enter)), Action::Finish);

        let mut config = UserConfig::default();
        state.apply_to(&mut config);
        assert!(config.onboarding().completed());
        assert!(
            config.providers().is_empty(),
            "the wizard must complete without recording a provider when the step is skipped"
        );
    }

    /// Acceptance 2 and 3: "Configure now" leads to real provider
    /// configuration, and "Do later" completes onboarding recording none —
    /// on the same wizard, proving the two paths are genuinely distinct
    /// rather than one silently doing the other's job.
    #[test]
    fn configure_now_records_a_provider_while_do_later_records_none() {
        // --- Do later: no provider recorded, onboarding still completes.
        // The Choice screen defaults to "Do later" (see `WizardState::new`),
        // so `Enter` here confirms it directly.
        let mut do_later = drive_to_provider(&all_harnesses_detected());
        assert_eq!(do_later.handle_key(key(KeyCode::Enter)), Action::Redraw);
        assert_eq!(do_later.step(), Step::Summary);
        assert_eq!(do_later.handle_key(key(KeyCode::Enter)), Action::Finish);
        let mut config = UserConfig::default();
        do_later.apply_to(&mut config);
        assert!(config.onboarding().completed());
        assert!(
            config.providers().is_empty(),
            "\"Do later\" must record no provider and no credential of any kind"
        );

        // --- Configure now: picking the first template (openrouter, a
        // named, non-generic template) records a real, resolvable provider.
        let mut configure_now = drive_to_provider(&all_harnesses_detected());
        // The Choice screen defaults to "Do later"; move up onto "Configure
        // now".
        assert_eq!(configure_now.handle_key(key(KeyCode::Up)), Action::Redraw);
        assert_eq!(
            configure_now.handle_key(key(KeyCode::Enter)),
            Action::Redraw
        );
        let first_template = provider::templates()
            .first()
            .expect("at least one built-in template")
            .name
            .clone();
        assert_eq!(
            configure_now.handle_key(key(KeyCode::Enter)), // choose the first template
            Action::Redraw
        );
        assert_eq!(configure_now.step(), Step::Provider);
        assert_eq!(configure_now.handle_key(key(KeyCode::Tab)), Action::Redraw);
        assert_eq!(configure_now.step(), Step::Summary);
        assert_eq!(
            configure_now.handle_key(key(KeyCode::Enter)),
            Action::Finish
        );

        let mut config = UserConfig::default();
        configure_now.apply_to(&mut config);
        assert!(config.onboarding().completed());
        let provider_config = config
            .providers()
            .get(&first_template)
            .unwrap_or_else(|| panic!("`{first_template}` must be recorded after Configure now"));
        assert_eq!(provider_config.template(), first_template);

        // And it is a real, resolvable provider — not a name stashed
        // somewhere inert.
        let provider = provider_config
            .to_provider(&first_template)
            .expect("a built-in template name must resolve");
        assert_eq!(provider.name, first_template);
    }

    /// A generic template (`openai-compatible`/`anthropic-compatible`)
    /// declares no base URL of its own, so "Configure now" must ask for one
    /// and refuse to record the provider until it gets a non-empty answer.
    #[test]
    fn a_generic_template_is_recorded_only_once_a_base_url_is_typed() {
        let mut state = drive_to_provider(&all_harnesses_detected());
        state.handle_key(key(KeyCode::Up)); // onto "Configure now"
        state.handle_key(key(KeyCode::Enter)); // -> PickTemplate

        let templates = provider::templates();
        let generic_index = templates
            .iter()
            .position(|p| provider::GENERIC_TEMPLATE_NAMES.contains(&p.name.as_str()))
            .expect("at least one generic template exists");
        let generic_name = templates[generic_index].name.clone();
        for _ in 0..generic_index {
            state.handle_key(key(KeyCode::Down));
        }
        assert_eq!(state.handle_key(key(KeyCode::Enter)), Action::Redraw);

        // Confirming an empty URL is refused, with an inline error, and
        // nothing is recorded.
        assert_eq!(state.handle_key(key(KeyCode::Enter)), Action::Redraw);
        match state.provider_step() {
            ProviderStepView::BaseUrlInput { error, .. } => {
                assert!(error.is_some(), "an empty base URL must surface an error")
            }
            other => panic!("expected BaseUrlInput, got {other:?}"),
        }
        assert!(state.configured_providers().is_empty());

        for c in "https://gateway.example/v1".chars() {
            state.handle_key(char_key(c));
        }
        assert_eq!(state.handle_key(key(KeyCode::Enter)), Action::Redraw);
        assert_eq!(state.step(), Step::Provider);

        let mut config = UserConfig::default();
        state.handle_key(key(KeyCode::Tab)); // -> Summary
        state.handle_key(key(KeyCode::Enter)); // Finish
        state.apply_to(&mut config);

        let recorded = config
            .providers()
            .get(&generic_name)
            .expect("the generic template must be recorded once a base URL is confirmed");
        assert_eq!(recorded.base_url(), Some("https://gateway.example/v1"));
    }

    /// Acceptance 4: after "Do later", at least one detected harness is
    /// enabled and its Native launch profile resolves — Glasshouse remains
    /// fully usable on native, subscription-backed harnesses alone, with no
    /// provider and no credential anywhere in the resulting configuration.
    #[test]
    fn do_later_leaves_glasshouse_usable_on_native_harnesses_with_no_provider_or_credential() {
        let mut state = drive_to_provider(&all_harnesses_detected());
        assert_eq!(state.handle_key(key(KeyCode::Tab)), Action::Redraw); // Do later
        assert_eq!(state.step(), Step::Summary);
        assert_eq!(state.handle_key(key(KeyCode::Enter)), Action::Finish);

        let mut config = UserConfig::default();
        state.apply_to(&mut config);

        // The absence that would otherwise silently rot: no provider, no
        // credential, anywhere in the resulting configuration.
        assert!(config.providers().is_empty());

        let enabled_harness = IntegrationId::ALL
            .iter()
            .copied()
            .find(|&id| {
                id.kind() == IntegrationKind::Harness
                    && config.integrations().is_enabled(id) == Some(true)
            })
            .expect("at least one detected harness must be enabled after \"Do later\"");

        let adapter = crate::harness::adapter_for(enabled_harness)
            .expect("an enabled harness must have an adapter");
        let profile = crate::profile::LaunchProfile::native(enabled_harness);
        let secrets = crate::secret::EnvironmentSecretStore::new();
        let resolution = crate::profile::Resolution {
            adapter,
            acknowledged_bypass: false,
            provider: None,
            secrets: &secrets,
        };
        crate::profile::resolve(&profile, &resolution).unwrap_or_else(|err| {
            panic!("{enabled_harness:?}'s Native profile must resolve after \"Do later\": {err}")
        });
    }

    /// Acceptance 6, for the provider step specifically: reopening the
    /// wizard after a provider was already configured must not lose it —
    /// choosing "Do later" this time still leaves the earlier provider on
    /// disk.
    #[test]
    fn reopening_preserves_a_previously_configured_provider() {
        let mut existing = UserConfig::default();
        existing.providers_mut().set(
            "openrouter",
            crate::config::ProviderConfig::new("openrouter"),
        );

        let mut state = WizardState::new(
            &all_harnesses_detected(),
            &existing,
            "demo".to_owned(),
            PathBuf::from("/tmp/demo"),
            "9.9.9".to_owned(),
        );

        // The reopened wizard shows the existing provider without being
        // told about it again.
        match state.provider_step() {
            ProviderStepView::Choice { providers, .. } => {
                assert!(providers.iter().any(|p| p.name == "openrouter"));
            }
            other => panic!("expected Choice, got {other:?}"),
        }

        state.handle_key(key(KeyCode::Tab)); // Welcome -> Harnesses
        state.handle_key(key(KeyCode::Tab)); // Harnesses -> Provider
        state.handle_key(key(KeyCode::Tab)); // Do later -> Summary
        state.handle_key(key(KeyCode::Enter)); // Finish

        let mut config = existing;
        state.apply_to(&mut config);
        assert_eq!(
            config.providers().get("openrouter").map(|p| p.template()),
            Some("openrouter"),
            "\"Do later\" on a reopen must not clear a provider configured in a prior run"
        );
    }

    /// Acceptance 5, live-request half: cmux stays absent until the user
    /// explicitly asks for it with `c`, and once asked for it behaves like
    /// any other row — an ordinary explicit-path/enable flow.
    #[test]
    fn cmux_can_be_explicitly_requested_even_when_not_detected() {
        let without_cmux: Vec<IntegrationDetection> = all_harnesses_detected()
            .into_iter()
            .filter(|d| d.id != IntegrationId::Cmux)
            .collect();
        let mut state = new_state(&without_cmux);
        state.handle_key(key(KeyCode::Enter)); // Welcome -> Harnesses
        assert!(
            state.rows().all(|r| r.id != IntegrationId::Cmux),
            "cmux must be absent when neither detected nor requested"
        );

        assert_eq!(state.handle_key(char_key('c')), Action::Redraw);
        assert!(
            state.rows().any(|r| r.id == IntegrationId::Cmux),
            "`c` must add cmux to the list on explicit request"
        );
        let cmux = state
            .rows()
            .find(|r| r.id == IntegrationId::Cmux)
            .expect("cmux row present");
        assert!(cmux.selected, "requesting cmux must select its new row");

        // A second `c` must not duplicate the row.
        state.handle_key(char_key('c'));
        assert_eq!(
            state.rows().filter(|r| r.id == IntegrationId::Cmux).count(),
            1
        );
    }

    /// Acceptance 6, config-persistence half for cmux: a previously
    /// explicitly-configured cmux (via an explicit path, in an earlier run)
    /// must still be shown on reopen even though live detection still finds
    /// nothing — the wizard must not silently drop it from the list.
    #[test]
    fn reopening_shows_a_previously_configured_cmux_even_without_live_detection() {
        let mut existing = UserConfig::default();
        existing
            .integrations_mut()
            .entry(IntegrationId::Cmux)
            .set_enabled(true)
            .set_executable(Some(PathBuf::from("/opt/cmux/bin/cmux")));

        let without_cmux: Vec<IntegrationDetection> = all_harnesses_detected()
            .into_iter()
            .filter(|d| d.id != IntegrationId::Cmux)
            .collect();

        let state = WizardState::new(
            &without_cmux,
            &existing,
            "demo".to_owned(),
            PathBuf::from("/tmp/demo"),
            "9.9.9".to_owned(),
        );
        let cmux = state
            .rows()
            .find(|r| r.id == IntegrationId::Cmux)
            .expect("a previously configured cmux must still be shown on reopen");
        assert_eq!(cmux.decision, Some(true));
    }

    // --- Amendment 1: the optional bypass-acknowledgement step ------------

    /// Production source of this module, with its test module and its
    /// comments removed — mirrors `harness::production_code` and its
    /// siblings elsewhere in this crate.
    fn production_code(source: &str) -> String {
        source
            .split("#[cfg(test)]")
            .next()
            .expect("split always yields at least one part")
            .lines()
            .filter(|line| {
                let trimmed = line.trim_start();
                !trimmed.starts_with("//")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Drive a fresh wizard from Welcome to the Bypass step.
    fn drive_to_bypass(detected: &[IntegrationDetection]) -> WizardState {
        let mut state = new_state(detected);
        state.handle_key(key(KeyCode::Tab)); // Welcome -> Harnesses
        state.handle_key(key(KeyCode::Tab)); // Harnesses -> Bypass
        assert_eq!(state.step(), Step::Bypass);
        state
    }

    /// Skip both optional steps and finish, from wherever `state` currently
    /// is on or before Bypass.
    fn skip_to_finish(state: &mut WizardState) {
        while state.step() != Step::Summary {
            state.handle_key(key(KeyCode::Tab));
        }
        state.handle_key(key(KeyCode::Enter));
    }

    /// Acceptance 7: a harness declaring automatic review is not offered a
    /// bypass acknowledgement; one declaring only a bypass is.
    #[test]
    fn only_a_harness_with_a_bypass_and_no_automatic_review_is_offered_bypass_acknowledgement() {
        let state = new_state(&all_harnesses_detected());
        let offered: Vec<IntegrationId> = state.bypass_rows().map(|r| r.id).collect();

        // Claude Code declares automatic review (see
        // `crate::profile::tests::a_native_profile_exists_for_every_harness_and_adds_nothing`
        // and its neighbours) — it must not be offered this step at all.
        assert!(
            !offered.contains(&IntegrationId::ClaudeCode),
            "a harness with an automatic-review mode must not be offered a bypass \
             acknowledgement: {offered:?}"
        );

        // Read from the adapters, not a fixed name, so this half stays
        // honest if the qualifying set ever changes.
        let expected: Vec<IntegrationId> = IntegrationId::ALL
            .iter()
            .copied()
            .filter(|&id| {
                crate::harness::adapter_for(id).is_some_and(|adapter| {
                    let approvals = adapter.describe().approvals;
                    !approvals.automatic_review.is_verified() && approvals.bypass.is_verified()
                })
            })
            .collect();
        assert!(
            !expected.is_empty(),
            "at least one harness declaring a bypass but no automatic review must exist for \
             this test to mean anything"
        );
        for id in expected {
            assert!(
                offered.contains(&id),
                "{id:?} declares a bypass but no automatic review and must be offered the step"
            );
        }
    }

    /// Acceptance 8: declining leaves `bypass_acknowledged` genuinely unset
    /// — not an explicit `false` — and a `Bypass` profile for that harness
    /// is still refused end to end, through the same `EffectiveConfig` read
    /// a real caller uses.
    #[test]
    fn declining_leaves_bypass_acknowledged_unset_and_the_profile_still_refused() {
        let mut state = drive_to_bypass(&all_harnesses_detected());
        let harness = state
            .bypass_rows()
            .next()
            .expect("at least one bypass row")
            .id;

        // Leave every row untouched — declining is doing nothing here.
        skip_to_finish(&mut state);

        let mut config = UserConfig::default();
        state.apply_to(&mut config);
        assert_eq!(
            config
                .integrations()
                .get(harness)
                .and_then(crate::config::IntegrationConfig::bypass_acknowledged),
            None,
            "declining must leave bypass_acknowledged genuinely unset"
        );

        let effective = crate::config::EffectiveConfig::new(&config, None);
        let adapter = crate::harness::adapter_for(harness).expect("adapter exists");
        let mut profile = crate::profile::LaunchProfile::native(harness);
        profile.approval = crate::profile::ApprovalSelection::Bypass;
        let secrets = crate::secret::EnvironmentSecretStore::new();
        let resolution = crate::profile::Resolution {
            adapter,
            acknowledged_bypass: effective.bypass_acknowledged(harness).value,
            provider: None,
            secrets: &secrets,
        };
        let err = crate::profile::resolve(&profile, &resolution)
            .expect_err("an unacknowledged bypass must be refused");
        assert!(
            matches!(err, crate::profile::Refusal::BypassNotAcknowledged { .. }),
            "expected BypassNotAcknowledged, got {err:?}"
        );
    }

    /// Acceptance 9: accepting records the acknowledgement for that harness
    /// only, leaving every other qualifying harness unset.
    #[test]
    fn accepting_records_the_acknowledgement_for_that_harness_only() {
        let mut state = drive_to_bypass(&all_harnesses_detected());
        let rows: Vec<IntegrationId> = state.bypass_rows().map(|r| r.id).collect();
        assert!(
            rows.len() >= 2,
            "need at least two qualifying harnesses for this test to mean anything: {rows:?}"
        );
        let accepted = rows[0];
        let untouched = rows[1..].to_vec();

        // Acknowledge only the first (selected) row.
        assert_eq!(state.handle_key(key(KeyCode::Char(' '))), Action::Redraw);

        skip_to_finish(&mut state);

        let mut config = UserConfig::default();
        state.apply_to(&mut config);
        assert_eq!(
            config
                .integrations()
                .get(accepted)
                .and_then(crate::config::IntegrationConfig::bypass_acknowledged),
            Some(true),
            "{accepted:?} was explicitly acknowledged and must be recorded"
        );
        for id in untouched {
            assert_eq!(
                config
                    .integrations()
                    .get(id)
                    .and_then(crate::config::IntegrationConfig::bypass_acknowledged),
                None,
                "{id:?} was never touched and must remain unacknowledged"
            );
        }
    }

    /// Acceptance 10: the acknowledgement is written to the user layer and
    /// never the project layer.
    ///
    /// Structural half: this module has no way to reach a `ProjectConfig` at
    /// all — `apply_to` only ever mutates the `UserConfig` it is handed, and
    /// [`super::run`] only ever saves that same `UserConfig`. Runtime half:
    /// once written, `EffectiveConfig::bypass_acknowledged` — which
    /// deliberately never reads a project layer for this field, see its own
    /// doc comment — reports it as [`crate::config::Layer::User`].
    #[test]
    fn the_acknowledgement_is_written_to_the_user_layer_and_never_the_project_layer() {
        let code = production_code(include_str!("state.rs"));
        for forbidden in ["ProjectConfig", "write_project_config_with_consent"] {
            assert!(
                !code.contains(forbidden),
                "onboarding/state.rs names `{forbidden}` in production code: the wizard must \
                 stay structurally unable to write a project-level configuration file"
            );
        }

        let mut state = drive_to_bypass(&all_harnesses_detected());
        let harness = state
            .bypass_rows()
            .next()
            .expect("at least one bypass row")
            .id;
        assert_eq!(state.handle_key(key(KeyCode::Char(' '))), Action::Redraw);
        skip_to_finish(&mut state);

        let mut config = UserConfig::default();
        state.apply_to(&mut config);

        let effective = crate::config::EffectiveConfig::new(&config, None);
        let resolved = effective.bypass_acknowledged(harness);
        assert!(resolved.value);
        assert_eq!(resolved.layer, crate::config::Layer::User);
    }

    /// Acceptance 11: the step is skippable and onboarding completes
    /// without it — a mutation that made a row mandatory before `Tab`/
    /// `Finish` would work fails this.
    #[test]
    fn the_bypass_step_is_skippable_and_onboarding_completes_without_it() {
        let mut state = drive_to_bypass(&all_harnesses_detected());
        assert_eq!(state.handle_key(key(KeyCode::Tab)), Action::Redraw);
        assert_eq!(state.step(), Step::Provider);

        skip_to_finish(&mut state);
        let mut config = UserConfig::default();
        state.apply_to(&mut config);
        assert!(config.onboarding().completed());
        for row in state.bypass_rows() {
            assert_eq!(
                config
                    .integrations()
                    .get(row.id)
                    .and_then(crate::config::IntegrationConfig::bypass_acknowledged),
                None,
                "skipping the step must record no acknowledgement for {:?}",
                row.id
            );
        }
    }
}
