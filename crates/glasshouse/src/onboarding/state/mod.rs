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

use crate::config::{ProviderConfig, RoutingModelChoice, RoutingModelResolution, UserConfig};
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
/// bypass-acknowledgement step, the optional provider/gateway step, the
/// optional routing-model step, and a confirmation summary.
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
    /// Optional: which model classifies requests before premium capacity is
    /// spent — Automatic, a pinned model, or not yet. See `RoutingMode`.
    ///
    /// **After [`Step::Provider`], never before.** Phase 2C line 1 asks for
    /// this step "after providers have been detected or configured", and the
    /// order is load-bearing rather than cosmetic: pinning a model means
    /// naming a configured provider, so a routing step the user reaches
    /// before configuring one has nothing to offer but Automatic and Do
    /// later.
    Routing,
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

/// Which of the three top-level offers is highlighted on [`Step::Routing`].
///
/// Exactly three, matching Phase 2C lines 2, 3 and 4 one for one — and
/// matching Phase 2D's Routing settings section, which asks for the same
/// three ("Automatic, a specific configured model, or deterministic-only
/// classification").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingChoice {
    /// Line 2: Glasshouse picks the cheapest sufficiently fast configured
    /// resource, at the moment it needs one.
    Automatic,
    /// Line 3: pin classification to one specific model.
    ChooseModel,
    /// Line 4: decide later; deterministic heuristics classify until then.
    /// The default — see [`WizardState::new`].
    DoLater,
}

/// Sub-mode of the optional routing-model step ([`Step::Routing`]).
///
/// The same shape as [`ProviderMode`], driven the same way: `Esc` steps back
/// one level rather than cancelling the wizard (see
/// [`WizardState::routing_step_back`]), and the deepest screen is a text
/// field. "Choose model" needs two answers — which configured provider, then
/// which model — because a model name alone would not say who to ask.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RoutingMode {
    /// Choosing among the three offers. `notice` explains a press that did
    /// nothing, and is cleared by the next keystroke that moves.
    Choice {
        choice: RoutingChoice,
        notice: Option<String>,
    },
    /// Picking which configured provider the pinned model belongs to. Index
    /// into [`WizardState::configured_providers`]'s own order.
    PickProvider { selected: usize },
    /// Typing the model name to pin.
    ModelInput {
        provider: String,
        /// The row `provider` was picked from, so `Esc` returns there
        /// instead of resetting to the top.
        provider_index: usize,
        buffer: String,
        /// Set when the last `Enter` was rejected (an empty name); cleared
        /// on the next keystroke or successful confirmation.
        error: Option<String>,
    },
}

/// One configured provider, as offered on the routing step's provider
/// picker.
#[derive(Debug, Clone)]
pub struct RoutingProviderRow {
    pub name: String,
    pub template: String,
    pub selected: bool,
}

/// What the wizard currently has recorded for the routing model, in the
/// shape the Choice and Summary screens render.
///
/// [`Self::PinnedUnavailable`] is the degrade Phase 2C's behavioural
/// contract requires: a configuration naming a model whose provider is no
/// longer configured must fall back to deterministic heuristics *and say
/// so*. Its `message` is not written here — it comes from
/// [`crate::config::RoutingFallback`]'s own `Display`, the same string
/// [`crate::config::EffectiveConfig::routing_model_resolution`] produces, so
/// the wizard and the rest of Glasshouse cannot drift into explaining the
/// same degrade two different ways.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutingSelectionView {
    /// Nothing recorded: deterministic heuristics classify.
    NotConfigured,
    /// Recorded as deterministic-only on purpose. Only reachable by seeding
    /// from a configuration that says so — Phase 2D writes it, this wizard
    /// does not.
    Deterministic,
    /// Glasshouse chooses at use time.
    Automatic,
    /// Pinned, and the provider it names is configured.
    Pinned { provider: String, model: String },
    /// Pinned, but the provider it names is not configured.
    PinnedUnavailable {
        provider: String,
        model: String,
        message: String,
    },
}

/// Read-only view of the optional routing-model step, for rendering.
#[derive(Debug, Clone)]
pub enum RoutingStepView {
    Choice {
        selected: RoutingChoice,
        /// What is recorded right now — seeded from configuration on a
        /// reopen, or set by this run.
        recorded: RoutingSelectionView,
        /// Whether "Choose model" can be entered at all. `false` when no
        /// provider is configured: there would be nothing to pin to.
        can_choose_model: bool,
        notice: Option<String>,
    },
    PickProvider {
        options: Vec<RoutingProviderRow>,
    },
    ModelInput {
        provider: String,
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
    routing_mode: RoutingMode,
    /// The routing-model choice this run will write, or `None` for "no
    /// routing model configured".
    ///
    /// Seeded from `existing` so a reopen shows what is already recorded,
    /// and written verbatim by [`WizardState::apply_to`] — including the
    /// `None`. That assignment is the one place this step deviates from the
    /// provider step, which only ever *adds*: a provider table can hold many
    /// entries and dropping one silently would lose work, whereas the
    /// routing model is a single value the wizard always displays, so
    /// pressing Enter on "Do later" has to be able to mean "not configured"
    /// on a reopen too. Tabbing past the step changes nothing either way.
    pending_routing: Option<RoutingModelChoice>,
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
        let pending_routing = existing.routing().model().cloned();
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
            // Highlight whatever is already recorded, so a reopen does not
            // silently re-offer a different answer than the one on disk. On
            // a genuine first run nothing is recorded, so this is "Do later"
            // — the default Phase 2C line 4 requires, and what a user who
            // tabs straight through therefore gets.
            routing_mode: RoutingMode::Choice {
                choice: match pending_routing {
                    Some(RoutingModelChoice::Automatic) => RoutingChoice::Automatic,
                    Some(RoutingModelChoice::Pinned { .. }) => RoutingChoice::ChooseModel,
                    Some(RoutingModelChoice::Deterministic) | None => RoutingChoice::DoLater,
                },
                notice: None,
            },
            pending_routing,
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
                if self.step == Step::Routing && self.routing_step_back() {
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
            Step::Routing => self.handle_routing_key(key),
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
                    // provider and no credential. It still goes on to the
                    // routing step: declining a provider does not decline
                    // Automatic or deterministic-only, both of which are
                    // meaningful with no provider configured at all.
                    ProviderChoice::DoLater => self.step = Step::Routing,
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
                self.step = Step::Routing;
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

    /// Every configured provider's name, in the order the picker offers
    /// them — the same list [`WizardState::configured_providers`] renders,
    /// so a provider configured on the previous step is immediately
    /// pinnable. This is what Phase 2C line 1's "after providers have been
    /// detected or configured" buys.
    fn configured_provider_names(&self) -> Vec<String> {
        self.configured_providers()
            .into_iter()
            .map(|provider| provider.name)
            .collect()
    }

    /// What the wizard has recorded for the routing model right now,
    /// resolved against the providers that actually exist.
    ///
    /// Used by both the routing step and the Summary, so the two can never
    /// disagree about what was chosen. A pinned model whose provider is not
    /// configured comes back as `RoutingSelectionView::PinnedUnavailable`
    /// carrying the shared explanation from
    /// [`crate::config::RoutingFallback`] — the wizard never composes that
    /// sentence itself.
    ///
    /// (`RoutingSelectionView` is deliberately not re-exported from
    /// `super`, matching every other per-step view type in this module, so
    /// it is named here in a code span rather than an intra-doc link.)
    pub fn routing_selection(&self) -> RoutingSelectionView {
        let Some(choice) = &self.pending_routing else {
            return RoutingSelectionView::NotConfigured;
        };
        match choice {
            RoutingModelChoice::Deterministic => RoutingSelectionView::Deterministic,
            RoutingModelChoice::Automatic => RoutingSelectionView::Automatic,
            RoutingModelChoice::Pinned { provider, model } => {
                match choice.resolve(&self.configured_provider_names()) {
                    RoutingModelResolution::Heuristics(reason) => {
                        RoutingSelectionView::PinnedUnavailable {
                            provider: provider.clone(),
                            model: model.clone(),
                            message: reason.to_string(),
                        }
                    }
                    _ => RoutingSelectionView::Pinned {
                        provider: provider.clone(),
                        model: model.clone(),
                    },
                }
            }
        }
    }

    /// A read-only view of the current sub-screen of [`Step::Routing`], for
    /// rendering.
    pub fn routing_step(&self) -> RoutingStepView {
        match &self.routing_mode {
            RoutingMode::Choice { choice, notice } => RoutingStepView::Choice {
                selected: *choice,
                recorded: self.routing_selection(),
                can_choose_model: !self.existing_providers.is_empty()
                    || self.pending_provider.is_some(),
                notice: notice.clone(),
            },
            RoutingMode::PickProvider { selected } => RoutingStepView::PickProvider {
                options: self
                    .configured_providers()
                    .into_iter()
                    .enumerate()
                    .map(|(index, provider)| RoutingProviderRow {
                        name: provider.name,
                        template: provider.template,
                        selected: index == *selected,
                    })
                    .collect(),
            },
            RoutingMode::ModelInput {
                provider,
                buffer,
                error,
                ..
            } => RoutingStepView::ModelInput {
                provider: provider.clone(),
                buffer: buffer.clone(),
                error: error.clone(),
            },
        }
    }

    /// Step back one level inside [`Step::Routing`]'s sub-modes without
    /// cancelling the wizard, exactly as
    /// [`WizardState::provider_step_back`] does for the provider step.
    /// Returns whether it consumed the key — `false` from the top-level
    /// Choice screen.
    fn routing_step_back(&mut self) -> bool {
        match &self.routing_mode {
            RoutingMode::Choice { .. } => false,
            RoutingMode::PickProvider { .. } => {
                self.routing_mode = RoutingMode::Choice {
                    choice: RoutingChoice::ChooseModel,
                    notice: None,
                };
                true
            }
            RoutingMode::ModelInput { provider_index, .. } => {
                self.routing_mode = RoutingMode::PickProvider {
                    selected: *provider_index,
                };
                true
            }
        }
    }

    fn handle_routing_key(&mut self, key: KeyEvent) -> Action {
        match self.routing_mode {
            RoutingMode::Choice { .. } => self.handle_routing_choice_key(key),
            RoutingMode::PickProvider { .. } => self.handle_routing_provider_key(key),
            RoutingMode::ModelInput { .. } => self.handle_routing_model_key(key),
        }
    }

    fn handle_routing_choice_key(&mut self, key: KeyEvent) -> Action {
        let RoutingMode::Choice { choice, .. } = &self.routing_mode else {
            return Action::None;
        };
        let choice = *choice;
        const ORDER: [RoutingChoice; 3] = [
            RoutingChoice::Automatic,
            RoutingChoice::ChooseModel,
            RoutingChoice::DoLater,
        ];
        let index = ORDER.iter().position(|c| *c == choice).unwrap_or(0);
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.routing_mode = RoutingMode::Choice {
                    choice: ORDER[index.saturating_sub(1)],
                    notice: None,
                };
                Action::Redraw
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.routing_mode = RoutingMode::Choice {
                    choice: ORDER[(index + 1).min(ORDER.len() - 1)],
                    notice: None,
                };
                Action::Redraw
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                match choice {
                    // Line 2: the *intent*, never a resolved model. Which
                    // resource is cheapest and fast enough depends on
                    // conditions that change after this wizard exits, so
                    // Phase 34C decides it at use time.
                    RoutingChoice::Automatic => {
                        self.pending_routing = Some(RoutingModelChoice::Automatic);
                        self.step = Step::Summary;
                    }
                    RoutingChoice::ChooseModel => {
                        if self.configured_provider_names().is_empty() {
                            // Saying nothing would read as a broken key.
                            // There is genuinely nothing to pin to, and the
                            // other two choices still work.
                            self.routing_mode = RoutingMode::Choice {
                                choice,
                                notice: Some(
                                    "Choose model needs a configured provider, and none is \
                                     configured yet. Go back with Esc to add one, or pick \
                                     Automatic or Do later."
                                        .to_owned(),
                                ),
                            };
                        } else {
                            self.routing_mode = RoutingMode::PickProvider { selected: 0 };
                        }
                    }
                    // Line 4: declining records nothing at all, and the
                    // system stays on deterministic heuristics.
                    RoutingChoice::DoLater => {
                        self.pending_routing = None;
                        self.step = Step::Summary;
                    }
                }
                Action::Redraw
            }
            // `Tab` always means "continue", exactly as on every other
            // optional step — and it leaves `pending_routing` alone, so
            // tabbing past this screen on a first run records nothing and
            // tabbing past it on a reopen preserves what is already there.
            KeyCode::Tab => {
                self.step = Step::Summary;
                Action::Redraw
            }
            _ => Action::None,
        }
    }

    fn handle_routing_provider_key(&mut self, key: KeyEvent) -> Action {
        let RoutingMode::PickProvider { selected } = &self.routing_mode else {
            return Action::None;
        };
        let selected = *selected;
        let providers = self.configured_provider_names();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.routing_mode = RoutingMode::PickProvider {
                    selected: selected.saturating_sub(1),
                };
                Action::Redraw
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let last = providers.len().saturating_sub(1);
                self.routing_mode = RoutingMode::PickProvider {
                    selected: (selected + 1).min(last),
                };
                Action::Redraw
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                let Some(provider) = providers.get(selected) else {
                    return Action::None;
                };
                // Pre-fill the model already pinned to this same provider,
                // so a reopen that only wants to change the provider does
                // not make the user retype a name Glasshouse already knows.
                let buffer = match &self.pending_routing {
                    Some(RoutingModelChoice::Pinned {
                        provider: pinned,
                        model,
                    }) if pinned == provider => model.clone(),
                    _ => String::new(),
                };
                self.routing_mode = RoutingMode::ModelInput {
                    provider: provider.clone(),
                    provider_index: selected,
                    buffer,
                    error: None,
                };
                Action::Redraw
            }
            _ => Action::None,
        }
    }

    fn handle_routing_model_key(&mut self, key: KeyEvent) -> Action {
        if !matches!(self.routing_mode, RoutingMode::ModelInput { .. }) {
            return Action::None;
        }
        match key.code {
            KeyCode::Enter => {
                let RoutingMode::ModelInput {
                    provider, buffer, ..
                } = &self.routing_mode
                else {
                    return Action::None;
                };
                let trimmed = buffer.trim();
                if trimmed.is_empty() {
                    if let RoutingMode::ModelInput { error, .. } = &mut self.routing_mode {
                        *error = Some("a model name is required to pin routing".to_owned());
                    }
                    return Action::Redraw;
                }
                // Two names and nothing else: this is a reference, in the
                // same sense `StoredCredentialRef` is one. Nothing typed on
                // this screen is ever a credential, and the provider named
                // here is where the credential question already lives.
                self.pending_routing = Some(RoutingModelChoice::Pinned {
                    provider: provider.clone(),
                    model: trimmed.to_owned(),
                });
                self.routing_mode = RoutingMode::Choice {
                    choice: RoutingChoice::ChooseModel,
                    notice: None,
                };
                Action::Redraw
            }
            KeyCode::Backspace => {
                if let RoutingMode::ModelInput { buffer, error, .. } = &mut self.routing_mode {
                    buffer.pop();
                    *error = None;
                }
                Action::Redraw
            }
            KeyCode::Char(c) => {
                if let RoutingMode::ModelInput { buffer, error, .. } = &mut self.routing_mode {
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
    /// Assumes `WizardState::finalize_pending_decisions` has already run
    /// (true for every reachable path to [`Action::Finish`] — see
    /// `WizardState::handle_harnesses_key`); a row somehow still `None`
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
    /// `pending_routing` is written verbatim, `None` included — see the
    /// field's own documentation for why this one assigns where
    /// `pending_provider` only ever adds. On a first run that declined the
    /// routing step it is `None`, so the saved file has no `[routing]` table
    /// at all and deterministic heuristics classify (Phase 2C line 4).
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
        config.routing_mut().set_model(self.pending_routing.clone());
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
