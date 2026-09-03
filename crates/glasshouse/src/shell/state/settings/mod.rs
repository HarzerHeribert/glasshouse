use super::*;

mod keys;

impl ShellState {
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
}

impl ShellState {
    /// Answer one key while the Settings overlay is open. Everything is
    /// handled here rather than falling through to the bindings above: Tab,
    /// the arrows, and Enter all mean something different inside Settings.
    pub(super) fn handle_settings_key(&mut self, key: KeyEvent) -> Action {
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
pub(super) fn probe_endpoint(intent: &ProviderProbeIntent) -> String {
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
