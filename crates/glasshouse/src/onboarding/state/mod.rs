//! The wizard's state machine, kept deliberately free of the terminal.
//!
//! [`WizardState`] owns every piece of mutable wizard data and is driven
//! entirely through [`WizardState::handle_key`], which takes a `crossterm`
//! [`KeyEvent`] and returns an [`Action`]; it never touches a
//! [`crate::tui::Screen`], never reads the clock, and never consults
//! process-global state. The one exception is
//! [`crate::platform::exec::resolve_explicit`], a synchronous, local
//! filesystem check — validating a user-typed path is the whole point of
//! the explicit-path step — and it is deterministic given real files on disk.
//! This split is what makes the wizard testable at all: [`super::run`] pumps
//! a real terminal's events into `handle_key` and paints [`super::view`],
//! but none of the actual decision logic lives there.
//!
//! History: design-decisions.md, "Trims: the remaining module docs, second
//! packet", onboarding/state/mod.rs module doc.

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
/// machine running the tests. `super::detections_from` maps a real
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
/// confirmation summary. There is no routing-model step any more
/// (2026-09-16 ruling — Glasshouse never decides which model is used).
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
    /// Rows skipped from the top of the Summary body, in terminal rows.
    /// Reset to `0` every time [`Step::Summary`] becomes current — see the
    /// three assignment sites of `self.step = Step::Summary` — so the
    /// Summary always opens at the top regardless of where a previous visit
    /// left it. The view clamps this to the body's actual maximum scroll,
    /// since this alone does not know the terminal's height.
    summary_scroll: u16,
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
            summary_scroll: 0,
        }
    }

    pub fn step(&self) -> Step {
        self.step
    }

    /// Rows skipped from the top of the Summary body. `0` on a fresh
    /// wizard and every time the Summary step is (re-)entered; the view
    /// clamps this to its own computed maximum before rendering.
    pub fn summary_scroll(&self) -> u16 {
        self.summary_scroll
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
            Step::Summary => match key.code {
                KeyCode::Enter | KeyCode::Tab => Action::Finish,
                KeyCode::Down | KeyCode::Char('j') => {
                    self.summary_scroll = self.summary_scroll.saturating_add(1);
                    Action::Redraw
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.summary_scroll = self.summary_scroll.saturating_sub(1);
                    Action::Redraw
                }
                KeyCode::PageDown => {
                    self.summary_scroll = self.summary_scroll.saturating_add(10);
                    Action::Redraw
                }
                KeyCode::PageUp => {
                    self.summary_scroll = self.summary_scroll.saturating_sub(10);
                    Action::Redraw
                }
                // The view clamps this to the real maximum — this alone
                // does not know the terminal's height.
                KeyCode::End => {
                    self.summary_scroll = u16::MAX;
                    Action::Redraw
                }
                KeyCode::Home => {
                    self.summary_scroll = 0;
                    Action::Redraw
                }
                _ => Action::None,
            },
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
    /// Assumes `WizardState::finalize_pending_decisions` has already run; a
    /// row somehow still `None` here is written as ignored rather than
    /// panicking, since a config write is not the place to enforce an
    /// internal invariant with a crash. `pending_provider` is only ever
    /// `Some` after a completed "Configure now", so "Do later" leaves
    /// `config.providers()` untouched (Phase 2C lines 3 and 6, one code
    /// path). A `BypassRow` is written only when this run actually changed
    /// it (`acknowledged != seeded`), so declining leaves
    /// `bypass_acknowledged` genuinely unset rather than an explicit
    /// `false`, and the same rule lets a reopen revoke a previously granted
    /// acknowledgement.
    ///
    /// History: design-decisions.md, "Trims: the remaining module docs,
    /// second packet", `apply_to`.
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
mod tests;
