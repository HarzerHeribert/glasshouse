//! `EffectiveConfig`: the layered reader over `UserConfig` and an optional `ProjectConfig`, one accessor per configuration decision.
//!

use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::guardrails::{self, BlockingCategory, GuardrailMode};
use crate::integrations::IntegrationId;
use crate::secret::SecretRef;

use super::*;

/// User configuration layered with an optional project-level override.
///
/// Kept intentionally small — a couple of lookup methods, not a generic
/// layering framework — because today only per-integration `enabled` and
/// `executable` need layering. Project always wins when it has recorded a
/// value; otherwise the user-level value applies; otherwise a caller
/// supplied default applies. Each lookup reports which of those three
/// happened via [`Layer`].
#[derive(Debug, Clone, Copy)]
pub struct EffectiveConfig<'a> {
    pub(super) user: &'a UserConfig,
    pub(super) project: Option<&'a ProjectConfig>,
}
impl<'a> EffectiveConfig<'a> {
    pub fn new(user: &'a UserConfig, project: Option<&'a ProjectConfig>) -> Self {
        Self { user, project }
    }

    /// Resolve whether `id` is enabled, reporting which layer decided it.
    /// Falls back to `default_enabled` (reported as [`Layer::Default`]) when
    /// neither layer has ever recorded a decision.
    pub fn enabled(&self, id: IntegrationId, default_enabled: bool) -> Layered<bool> {
        if let Some(enabled) = self.project.and_then(|p| p.integrations().is_enabled(id)) {
            return Layered::new(enabled, Layer::Project);
        }
        if let Some(enabled) = self.user.integrations().is_enabled(id) {
            return Layered::new(enabled, Layer::User);
        }
        Layered::new(default_enabled, Layer::Default)
    }

    /// Resolve whether `id` has the user's consent to write its lifecycle
    /// hooks inside the project itself, reporting which layer decided it.
    /// Falls back to `false` (reported as [`Layer::Default`]) when neither
    /// layer has ever recorded a decision — unlike
    /// [`EffectiveConfig::enabled`], callers never get to choose that
    /// default, because a session with no consent on record must run without
    /// project-local hooks rather than assume the answer either way.
    pub fn project_hooks(&self, id: IntegrationId) -> Layered<bool> {
        if let Some(consent) = self
            .project
            .and_then(|p| p.integrations().get(id))
            .and_then(IntegrationConfig::project_hooks)
        {
            return Layered::new(consent, Layer::Project);
        }
        if let Some(consent) = self
            .user
            .integrations()
            .get(id)
            .and_then(IntegrationConfig::project_hooks)
        {
            return Layered::new(consent, Layer::User);
        }
        Layered::new(false, Layer::Default)
    }

    /// Resolve whether the user has acknowledged `id`'s blanket approval
    /// bypass, reporting which layer decided it. Falls back to `false`
    /// (reported as [`Layer::Default`]) when the user layer has never
    /// recorded a decision.
    ///
    /// **This deliberately consults `self.user` only — never
    /// `self.project`.** Every other lookup on this type checks the project
    /// layer first; this one must not, because a repository that could
    /// pre-acknowledge a blanket permission bypass would be acknowledging it
    /// on behalf of whoever cloned it, who has been shown nothing. The
    /// acknowledgement this field records is a statement by a person about a
    /// harness on *their own machine*, not a property of the project, so a
    /// project-level `bypass_acknowledged = true` checked into a repository
    /// must have no effect at all. Say this plainly in code, because the
    /// deviation from every other lookup here reads as an oversight
    /// otherwise — it is not one.
    pub fn bypass_acknowledged(&self, id: IntegrationId) -> Layered<bool> {
        if let Some(acknowledged) = self
            .user
            .integrations()
            .get(id)
            .and_then(IntegrationConfig::bypass_acknowledged)
        {
            return Layered::new(acknowledged, Layer::User);
        }
        Layered::new(false, Layer::Default)
    }

    /// Resolve the explicit executable override for `id`, if any layer has
    /// recorded one. `None` means neither layer has an override, i.e. normal
    /// `PATH` discovery applies — there is no "default" executable path to
    /// report here, so unlike [`EffectiveConfig::enabled`] this has no
    /// [`Layer::Default`] case.
    pub fn executable(&self, id: IntegrationId) -> Option<Layered<PathBuf>> {
        if let Some(exe) = self
            .project
            .and_then(|p| p.integrations().get(id))
            .and_then(IntegrationConfig::executable)
        {
            return Some(Layered::new(exe.to_path_buf(), Layer::Project));
        }
        if let Some(exe) = self
            .user
            .integrations()
            .get(id)
            .and_then(IntegrationConfig::executable)
        {
            return Some(Layered::new(exe.to_path_buf(), Layer::User));
        }
        None
    }

    /// Every launch profile name available: the implied
    /// [`crate::profile::NATIVE_PROFILE_NAME`], plus every name either layer
    /// has configured. Where both layers configure the same name, this still
    /// lists it once — see [`EffectiveConfig::launch_profile`] for which
    /// layer's definition wins.
    pub fn profile_names(&self) -> Vec<String> {
        let mut names: BTreeSet<String> = BTreeSet::new();
        names.insert(crate::profile::NATIVE_PROFILE_NAME.to_owned());
        names.extend(self.user.profiles().names().map(str::to_owned));
        if let Some(project) = self.project {
            names.extend(project.profiles().names().map(str::to_owned));
        }
        names.into_iter().collect()
    }

    /// Whether the launch profile `name` may be *started*, reporting which
    /// layer decided it — project first, then user, then [`Layer::Default`],
    /// matching every other lookup on this type except
    /// [`EffectiveConfig::bypass_acknowledged`], and matching the layer
    /// [`EffectiveConfig::launch_profile`] picks: `launch_profile` takes the
    /// winning layer's [`ProfileConfig`] whole, so resolving `enabled` from a
    /// different layer could report a profile neither layer ever wrote.
    ///
    /// [`crate::profile::NATIVE_PROFILE_NAME`] answers `true` at
    /// [`Layer::Default`] without consulting either table: it exists for
    /// every harness by construction, never stored in a [`ProfileTable`], so
    /// there is nothing to disable and the enabled candidate set is never
    /// empty. An unknown name also answers `true` at [`Layer::Default`]:
    /// "disabled" and "never configured" are different facts, and
    /// [`EffectiveConfig::launch_profile`] already reports the second one as
    /// [`ProfileLookupError::Unknown`].
    ///
    /// History: design-decisions.md, "Trims: config, checkpoint, evaluation and codex module docs", effective.rs `profile_enabled`.
    pub fn profile_enabled(&self, name: &str) -> Layered<bool> {
        if name == crate::profile::NATIVE_PROFILE_NAME {
            return Layered::new(true, Layer::Default);
        }
        if let Some(config) = self.project.and_then(|p| p.profiles().get(name)) {
            return Layered::new(config.enabled(), Layer::Project);
        }
        if let Some(config) = self.user.profiles().get(name) {
            return Layered::new(config.enabled(), Layer::User);
        }
        Layered::new(true, Layer::Default)
    }

    /// Resolve `name` to a [`crate::profile::LaunchProfile`] for `harness`,
    /// reporting which layer supplied it.
    ///
    /// The implied Native profile is available for every harness regardless
    /// of either layer — by construction rather than a configuration entry,
    /// so adding gateway profiles can never remove it — and is built
    /// directly for `harness` without a table lookup. For any other name,
    /// the project layer's definition wins over the user layer's, matching
    /// every other lookup on this type; and a profile that names a harness
    /// other than `harness` is refused rather than silently substituted,
    /// because that harness is what the caller has already selected (or the
    /// user explicitly typed on the command line).
    pub fn launch_profile(
        &self,
        name: &str,
        harness: IntegrationId,
    ) -> Result<Layered<crate::profile::LaunchProfile>, ProfileLookupError> {
        if name == crate::profile::NATIVE_PROFILE_NAME {
            return Ok(Layered::new(
                crate::profile::LaunchProfile::native(harness),
                Layer::Default,
            ));
        }

        let found = if let Some(config) = self.project.and_then(|p| p.profiles().get(name)) {
            Some((config, Layer::Project))
        } else {
            self.user
                .profiles()
                .get(name)
                .map(|config| (config, Layer::User))
        };

        let Some((config, layer)) = found else {
            return Err(ProfileLookupError::Unknown {
                name: name.to_owned(),
                known: self.profile_names(),
            });
        };

        let profile = config.to_launch_profile(name)?;
        if profile.harness != harness {
            return Err(ProfileLookupError::HarnessMismatch {
                name: name.to_owned(),
                profile_harness: profile.harness,
                requested_harness: harness,
            });
        }
        Ok(Layered::new(profile, layer))
    }

    /// Every configured provider name available, from either layer.
    ///
    /// Unlike [`EffectiveConfig::profile_names`], there is no implied entry:
    /// a provider only exists here because a user or project explicitly
    /// configured one.
    pub fn provider_names(&self) -> Vec<String> {
        let mut names: BTreeSet<String> = BTreeSet::new();
        names.extend(self.user.providers().names().map(str::to_owned));
        if let Some(project) = self.project {
            names.extend(project.providers().names().map(str::to_owned));
        }
        names.into_iter().collect()
    }

    /// Whether Glasshouse's automatic post-turn memory-extraction trigger
    /// (Phase 21) may run, reporting which layer decided it. Project first,
    /// then user, then [`Layer::Default`], matching every other lookup on
    /// this type except [`EffectiveConfig::bypass_acknowledged`].
    ///
    /// Deliberately independent of [`EffectiveConfig::routing_model`] and
    /// response-profile injection: each reads its own field, so disabling one
    /// automatic behaviour never disables another. See
    /// `tests::the_three_automatic_behaviours_disable_independently`.
    pub fn memory_extraction_enabled(&self) -> Layered<bool> {
        if let Some(value) = self.project.and_then(ProjectConfig::memory_extraction) {
            return Layered::new(value, Layer::Project);
        }
        if let Some(value) = self.user.memory_extraction() {
            return Layered::new(value, Layer::User);
        }
        Layered::new(true, Layer::Default)
    }

    /// Whether Glasshouse delivers its own implementation policy
    /// (`crate::policy`, capability map lines 955-990) to an agent it briefs,
    /// reporting which layer decided it. Project first, then user, then
    /// [`Layer::Default`], matching [`Self::memory_extraction_enabled`]'s own
    /// layering.
    ///
    /// [`Layer::Default`] carries `true`: the policy is what Glasshouse has
    /// to say about how work is implemented here, and a default of off would
    /// ship the opposite of the decision that put it in the product.
    ///
    /// Deliberately independent of every other automatic behaviour: each
    /// reads its own field, so turning memory extraction off never turns this
    /// off and vice versa — the property
    /// `config::response::tests::the_three_automatic_behaviours_disable_independently`
    /// asserts for the three that came before it, and
    /// `implementation_policy::the_policy_is_not_delivered_when_turned_off_and_never_repeated_to_the_same_session`
    /// asserts end to end for this one.
    pub fn implementation_policy_enabled(&self) -> Layered<bool> {
        if let Some(value) = self.project.and_then(ProjectConfig::implementation_policy) {
            return Layered::new(value, Layer::Project);
        }
        if let Some(value) = self.user.implementation_policy() {
            return Layered::new(value, Layer::User);
        }
        Layered::new(true, Layer::Default)
    }

    /// Which model may perform memory extraction, and which layer chose it —
    /// Phase 21 line 834. Project first, then user, then [`Layer::Default`],
    /// matching [`Self::memory_extraction_enabled`]'s own layering.
    ///
    /// [`Layer::Default`] carries `None`, and `None` is the answer that keeps
    /// today's behaviour: no model is called. **A project naming a model
    /// overrides a user who named none** — the same direction every other
    /// lookup on this type resolves, and what lets one repository use a local
    /// runner without the user turning it on everywhere.
    ///
    /// Deliberately independent of [`Self::memory_extraction_enabled`]: that
    /// one is whether the trigger may fire at all, this one is what it asks
    /// when it does, and a user who turns the trigger off has turned the
    /// model off with it whatever this says.
    pub fn memory_extraction_model(&self) -> Layered<Option<ExtractionModelRef>> {
        if let Some(value) = self
            .project
            .and_then(ProjectConfig::memory_extraction_model)
        {
            return Layered::new(Some(value.clone()), Layer::Project);
        }
        if let Some(value) = self.user.memory_extraction_model() {
            return Layered::new(Some(value.clone()), Layer::User);
        }
        Layered::new(None, Layer::Default)
    }

    /// Which model may rerank the top lexical memory candidates before
    /// selection — map line 1089, the reranking seat's consent. Project
    /// first, then user, then [`Layer::Default`] carrying `None`, matching
    /// [`Self::memory_extraction_model`]'s own layering and for the same
    /// reason: **`None` is the default and means no model is ever called.**
    ///
    /// Lives on [`crate::config::loading::MemoryConfig`] (the `[memory]` table) rather than as a
    /// bare top-level key the way [`Self::memory_extraction_model`] does,
    /// because this is a user-facing product setting reached for by name
    /// (`[memory] rerank_model`), matching [`Self::inject_memory_at_launch`]'s
    /// own table.
    pub fn memory_rerank_model(&self) -> Layered<Option<ExtractionModelRef>> {
        if let Some(value) = self.project.and_then(|p| p.memory().rerank_model()) {
            return Layered::new(Some(value.clone()), Layer::Project);
        }
        if let Some(value) = self.user.memory().rerank_model() {
            return Layered::new(Some(value.clone()), Layer::User);
        }
        Layered::new(None, Layer::Default)
    }

    /// Whether a briefing records one JSON line describing its retrieval and
    /// rerank decision — map line 1094. Project first, then user, then
    /// [`Layer::Default`] carrying `false`: diagnostics are off unless
    /// explicitly turned on, matching every other automatic-behaviour
    /// default's direction of least surprise.
    pub fn memory_retrieval_diagnostics(&self) -> Layered<bool> {
        if let Some(value) = self
            .project
            .and_then(|p| p.memory().retrieval_diagnostics())
        {
            return Layered::new(value, Layer::Project);
        }
        if let Some(value) = self.user.memory().retrieval_diagnostics() {
            return Layered::new(value, Layer::User);
        }
        Layered::new(false, Layer::Default)
    }

    /// Whether an extraction run records one JSON line describing its
    /// inputs and outputs — map line 1769. Project first, then user, then
    /// [`Layer::Default`] carrying `false`, matching
    /// [`Self::memory_retrieval_diagnostics`]'s own layering and its own
    /// off-by-default direction.
    pub fn memory_extraction_diagnostics(&self) -> Layered<bool> {
        if let Some(value) = self
            .project
            .and_then(|p| p.memory().extraction_diagnostics())
        {
            return Layered::new(value, Layer::Project);
        }
        if let Some(value) = self.user.memory().extraction_diagnostics() {
            return Layered::new(value, Layer::User);
        }
        Layered::new(false, Layer::Default)
    }

    /// Whether Glasshouse may take a checkpoint automatically at a task
    /// boundary (Phase 19 line 802), reporting which layer decided it.
    /// Project first, then user, then [`Layer::Default`], matching
    /// [`Self::memory_extraction_enabled`]'s own layering.
    ///
    /// Deliberately independent of [`Self::memory_extraction_enabled`] and
    /// every other automatic behaviour: each reads its own field, so
    /// disabling one never disables another.
    pub fn automatic_checkpoint_enabled(&self) -> Layered<bool> {
        if let Some(value) = self.project.and_then(ProjectConfig::automatic_checkpoint) {
            return Layered::new(value, Layer::Project);
        }
        if let Some(value) = self.user.automatic_checkpoint() {
            return Layered::new(value, Layer::User);
        }
        Layered::new(true, Layer::Default)
    }

    /// Whether `glasshouse launch` briefs a new session with this project's
    /// memory (`GH-LAUNCH-BRIEFING`), reporting which layer decided it.
    /// Project first, then user, then [`Layer::Default`] carrying `true` —
    /// opt-out, not opt-in, matching the design ruling's own wording.
    ///
    /// Deliberately independent of every other automatic behaviour: it reads
    /// its own table, so turning this off never turns off memory extraction,
    /// checkpoints or the implementation policy, and vice versa.
    pub fn inject_memory_at_launch(&self) -> Layered<bool> {
        if let Some(value) = self.project.and_then(|p| p.memory().inject_at_launch()) {
            return Layered::new(value, Layer::Project);
        }
        if let Some(value) = self.user.memory().inject_at_launch() {
            return Layered::new(value, Layer::User);
        }
        Layered::new(true, Layer::Default)
    }

    /// `guardrails.mode`, and which layer set it — Phase 21K line 1052.
    /// Project first, then user, then [`Layer::Default`] carrying
    /// `advisory`, matching every other lookup on this type.
    ///
    /// Deliberately independent of every other automatic behaviour: it
    /// reads its own table, so turning the guardrail off never turns off
    /// memory extraction or checkpoints, and vice versa.
    pub fn guardrail_mode(&self) -> Layered<GuardrailMode> {
        if let Some(value) = self.project.and_then(|p| p.guardrails().mode()) {
            return Layered::new(value, Layer::Project);
        }
        if let Some(value) = self.user.guardrails().mode() {
            return Layered::new(value, Layer::User);
        }
        Layered::new(GuardrailMode::Advisory, Layer::Default)
    }

    /// `guardrails.blocking`, and which layer set it. [`Layer::Default`]
    /// carries the design ruling's list, [`guardrails::DEFAULT_BLOCKING`].
    /// A layer that records an explicit empty list means *nothing may
    /// block*, which is a different fact from recording nothing.
    pub fn guardrail_blocking(&self) -> Layered<Vec<BlockingCategory>> {
        if let Some(value) = self.project.and_then(|p| p.guardrails().blocking()) {
            return Layered::new(value.to_vec(), Layer::Project);
        }
        if let Some(value) = self.user.guardrails().blocking() {
            return Layered::new(value.to_vec(), Layer::User);
        }
        Layered::new(guardrails::DEFAULT_BLOCKING.to_vec(), Layer::Default)
    }

    /// Both guardrail preferences as the gate reads them, each with the
    /// phrase naming its layer, and no per-task override — the door adds
    /// that from the session's own ledger.
    pub fn guardrail_policy(&self) -> guardrails::Policy {
        let mode = self.guardrail_mode();
        let blocking = self.guardrail_blocking();
        guardrails::Policy {
            mode: mode.value,
            mode_source: mode.layer.describe_source(),
            blocking: blocking.value,
            blocking_source: blocking.layer.describe_source(),
            override_: None,
        }
    }

    /// `context_firewall.mode`, and which layer set it — Phase 57 map line
    /// 1991. Project first, then user, then [`Layer::Default`] carrying
    /// [`firewall::FirewallMode::Off`], matching every other lookup on this
    /// type except [`EffectiveConfig::bypass_acknowledged`].
    ///
    /// [`Layer::Default`] carrying `off` is the decision, not merely the
    /// safe choice: a session nobody configured must launch with a command
    /// line byte-identical to one built before this phase existed, and that
    /// is only true if the missing case never registers a hook.
    pub fn context_firewall_mode(&self) -> Layered<firewall::FirewallMode> {
        if let Some(value) = self.project.and_then(|p| p.context_firewall().mode()) {
            return Layered::new(value, Layer::Project);
        }
        if let Some(value) = self.user.context_firewall().mode() {
            return Layered::new(value, Layer::User);
        }
        Layered::new(firewall::FirewallMode::Off, Layer::Default)
    }

    /// Phase 60 map line 2405: whether a launched Claude Code session gets
    /// the edit-intent coordination hook, and which layer decided.
    ///
    /// Project over user over default, exactly like
    /// [`Self::context_firewall_mode`] — and unlike it, the default is
    /// [`firewall::EditIntentMode::DEFAULT`] (`on`) rather than `off`. That
    /// constant's own doc carries the reasoning.
    pub fn edit_intent_mode(&self) -> Layered<firewall::EditIntentMode> {
        if let Some(value) = self.project.and_then(|p| p.edit_intent().mode()) {
            return Layered::new(value, Layer::Project);
        }
        if let Some(value) = self.user.edit_intent().mode() {
            return Layered::new(value, Layer::User);
        }
        Layered::new(firewall::EditIntentMode::DEFAULT, Layer::Default)
    }

    /// The passthrough-token threshold for `mode`, and which layer set it.
    ///
    /// Reads a different field per mode — `aggressive_passthrough_tokens`
    /// under [`firewall::FirewallMode::Aggressive`],
    /// `passthrough_tokens` under every other mode — so aggressive's own
    /// threshold can move without safe's changing underneath it (map line
    /// 1991's one permitted difference between the two). `off` and `shadow`
    /// still resolve a value here because `shadow` runs the full pipeline
    /// and needs a real threshold even though it never emits reduced text.
    pub fn context_firewall_passthrough_tokens(
        &self,
        mode: firewall::FirewallMode,
    ) -> Layered<u64> {
        let (project_value, user_value, default_value) =
            if mode == firewall::FirewallMode::Aggressive {
                (
                    self.project
                        .and_then(|p| p.context_firewall().aggressive_passthrough_tokens()),
                    self.user.context_firewall().aggressive_passthrough_tokens(),
                    firewall::DEFAULT_AGGRESSIVE_PASSTHROUGH_TOKENS,
                )
            } else {
                (
                    self.project
                        .and_then(|p| p.context_firewall().passthrough_tokens()),
                    self.user.context_firewall().passthrough_tokens(),
                    firewall::DEFAULT_PASSTHROUGH_TOKENS,
                )
            };
        if let Some(value) = project_value {
            return Layered::new(value, Layer::Project);
        }
        if let Some(value) = user_value {
            return Layered::new(value, Layer::User);
        }
        Layered::new(default_value, Layer::Default)
    }

    /// `context_firewall.reducer` — map line 1997's opt-in: `None` (the
    /// [`Layer::Default`] case, and the only state a user who never
    /// configured a reducer has) disables semantic reduction in every mode,
    /// per map line 1992's guarantee.
    pub fn context_firewall_reducer(&self) -> Layered<Option<String>> {
        if let Some(value) = self.project.and_then(|p| p.context_firewall().reducer()) {
            return Layered::new(Some(value.to_owned()), Layer::Project);
        }
        if let Some(value) = self.user.context_firewall().reducer() {
            return Layered::new(Some(value.to_owned()), Layer::User);
        }
        Layered::new(None, Layer::Default)
    }

    /// `context_firewall.reducer_model` — map line 2002's pin. `None` lets
    /// `DisposableRouting` choose among whatever
    /// [`EffectiveConfig::context_firewall_reducer`] names.
    pub fn context_firewall_reducer_model(&self) -> Layered<Option<String>> {
        if let Some(value) = self
            .project
            .and_then(|p| p.context_firewall().reducer_model())
        {
            return Layered::new(Some(value.to_owned()), Layer::Project);
        }
        if let Some(value) = self.user.context_firewall().reducer_model() {
            return Layered::new(Some(value.to_owned()), Layer::User);
        }
        Layered::new(None, Layer::Default)
    }

    /// `context_firewall.min_semantic_tokens` — map line 1997's gate,
    /// defaulting to [`firewall::DEFAULT_MIN_SEMANTIC_TOKENS`].
    pub fn context_firewall_min_semantic_tokens(&self) -> Layered<u64> {
        if let Some(value) = self
            .project
            .and_then(|p| p.context_firewall().min_semantic_tokens())
        {
            return Layered::new(value, Layer::Project);
        }
        if let Some(value) = self.user.context_firewall().min_semantic_tokens() {
            return Layered::new(value, Layer::User);
        }
        Layered::new(firewall::DEFAULT_MIN_SEMANTIC_TOKENS, Layer::Default)
    }

    /// `context_firewall.aggressive_drops_uncertain` — map line 2000's
    /// explicit opt-in for aggressive mode to drop `uncertain` candidates.
    /// Defaults to `false`: bias to inclusion is the state nobody had to ask
    /// for.
    pub fn context_firewall_aggressive_drops_uncertain(&self) -> Layered<bool> {
        if let Some(value) = self
            .project
            .and_then(|p| p.context_firewall().aggressive_drops_uncertain())
        {
            return Layered::new(value, Layer::Project);
        }
        if let Some(value) = self.user.context_firewall().aggressive_drops_uncertain() {
            return Layered::new(value, Layer::User);
        }
        Layered::new(false, Layer::Default)
    }

    /// `context_firewall.reducer_local_only` — map line 2003's local-only
    /// operation. Defaults to `false`, matching every reducer field's
    /// "nobody configured anything" state.
    pub fn context_firewall_reducer_local_only(&self) -> Layered<bool> {
        if let Some(value) = self
            .project
            .and_then(|p| p.context_firewall().reducer_local_only())
        {
            return Layered::new(value, Layer::Project);
        }
        if let Some(value) = self.user.context_firewall().reducer_local_only() {
            return Layered::new(value, Layer::User);
        }
        Layered::new(false, Layer::Default)
    }

    /// `[profiles.<name>.context_firewall]`, project before user, matching
    /// [`Self::launch_profile`]'s own "project's definition wins over the
    /// user's, by name" rule — `None` for the implied Native profile (no
    /// configured entry can name it) and for a name neither layer defines.
    fn context_firewall_profile_override(
        &self,
        name: &str,
    ) -> Option<&firewall::ContextFirewallOverride> {
        if name == crate::profile::NATIVE_PROFILE_NAME {
            return None;
        }
        if let Some(config) = self.project.and_then(|p| p.profiles().get(name)) {
            return Some(config.context_firewall());
        }
        self.user
            .profiles()
            .get(name)
            .map(ProfileConfig::context_firewall)
    }

    /// Map lines 2023/2024's mode resolution: `[profiles.<name>.context_firewall]`,
    /// then the serving entitlement's own `[entitlements.<name>.context_firewall]`
    /// (via [`ResolvedEntitlement::context_firewall`] — resolved once at
    /// entitlement-lookup time, never re-looked-up by name here), then
    /// `kind`'s `[context_firewall.<kind>]` sub-table (project before user),
    /// then [`Self::context_firewall_mode`]'s flat value. A profile name of
    /// `None` or an entitlement of `None` simply skips that layer — the
    /// caller passes `None` for a resource neither names, exactly as
    /// [`Self::entitlement_for`] already returns `None` for one.
    pub fn context_firewall_policy_mode(
        &self,
        kind: Option<firewall::ReductionPolicyKind>,
        profile_name: Option<&str>,
        entitlement: Option<&ResolvedEntitlement>,
    ) -> firewall::FirewallMode {
        if let Some(value) = profile_name
            .and_then(|name| self.context_firewall_profile_override(name))
            .and_then(firewall::ContextFirewallOverride::mode)
        {
            return value;
        }
        if let Some(value) = entitlement.and_then(|e| e.context_firewall().mode()) {
            return value;
        }
        if let Some(kind) = kind {
            if let Some(value) = self
                .project
                .and_then(|p| p.context_firewall().kind_override(kind).mode())
            {
                return value;
            }
            if let Some(value) = self.user.context_firewall().kind_override(kind).mode() {
                return value;
            }
        }
        self.context_firewall_mode().value
    }

    /// Map lines 2023/2024's passthrough-threshold resolution, same layer
    /// order as [`Self::context_firewall_policy_mode`]. `mode` (the already
    /// version-floor-adjusted effective mode) picks between the aggressive
    /// and ordinary field at every layer, exactly like
    /// [`Self::context_firewall_passthrough_tokens`] does for the flat
    /// fallback it ends on.
    pub fn context_firewall_policy_passthrough_tokens(
        &self,
        mode: firewall::FirewallMode,
        kind: Option<firewall::ReductionPolicyKind>,
        profile_name: Option<&str>,
        entitlement: Option<&ResolvedEntitlement>,
    ) -> u64 {
        if let Some(value) = profile_name
            .and_then(|name| self.context_firewall_profile_override(name))
            .and_then(|over| over.passthrough_tokens_for(mode))
        {
            return value;
        }
        if let Some(value) =
            entitlement.and_then(|e| e.context_firewall().passthrough_tokens_for(mode))
        {
            return value;
        }
        if let Some(kind) = kind {
            if let Some(value) = self.project.and_then(|p| {
                p.context_firewall()
                    .kind_override(kind)
                    .passthrough_tokens_for(mode)
            }) {
                return value;
            }
            if let Some(value) = self
                .user
                .context_firewall()
                .kind_override(kind)
                .passthrough_tokens_for(mode)
            {
                return value;
            }
        }
        self.context_firewall_passthrough_tokens(mode).value
    }

    /// Map lines 2023/2024's `--min-semantic-tokens` resolution, same layer
    /// order as [`Self::context_firewall_policy_mode`] — but, unlike mode
    /// and passthrough tokens, `None` when **no layer at all** set a value,
    /// flat field and constant included. This is what lets
    /// `install_context_firewall_hook` omit the flag entirely for a session
    /// nobody configured: before this resolver existed the flag was never
    /// baked, so REQUIRED BEHAVIOR's byte-identical regression pin needs the
    /// unconfigured case to keep producing no flag, not the constant's own
    /// value spelled out for the first time.
    pub fn context_firewall_policy_min_semantic_tokens(
        &self,
        kind: Option<firewall::ReductionPolicyKind>,
        profile_name: Option<&str>,
        entitlement: Option<&ResolvedEntitlement>,
    ) -> Option<u64> {
        if let Some(value) = profile_name
            .and_then(|name| self.context_firewall_profile_override(name))
            .and_then(firewall::ContextFirewallOverride::min_semantic_tokens)
        {
            return Some(value);
        }
        if let Some(value) = entitlement.and_then(|e| e.context_firewall().min_semantic_tokens()) {
            return Some(value);
        }
        if let Some(kind) = kind {
            if let Some(value) = self.project.and_then(|p| {
                p.context_firewall()
                    .kind_override(kind)
                    .min_semantic_tokens()
            }) {
                return Some(value);
            }
            if let Some(value) = self
                .user
                .context_firewall()
                .kind_override(kind)
                .min_semantic_tokens()
            {
                return Some(value);
            }
        }
        if let Some(value) = self
            .project
            .and_then(|p| p.context_firewall().min_semantic_tokens())
        {
            return Some(value);
        }
        self.user.context_firewall().min_semantic_tokens()
    }

    /// Resolve which routing model classifies requests, reporting which
    /// layer decided it.
    ///
    /// Project first, then user, then [`Layer::Default`] — the ordinary
    /// layering every lookup on this type uses except
    /// [`EffectiveConfig::bypass_acknowledged`], which consults the user
    /// layer alone. This is deliberately *not* that exception. A bypass
    /// acknowledgement is a safety attestation a person makes about their
    /// own machine, so a repository must not be able to pre-acknowledge one
    /// on behalf of whoever cloned it. A routing-model choice is a
    /// preference about which cheap classifier to ask, it grants nothing and
    /// attests to nothing, and a project that wants its own — deterministic
    /// only in a repository whose work is uniform, say — is making an
    /// ordinary configuration statement. So the normal rule applies.
    ///
    /// The [`Layer::Default`] case is [`RoutingModelChoice::Deterministic`]:
    /// with nothing recorded anywhere, deterministic heuristics classify,
    /// which is exactly Phase 2C line 4.
    pub fn routing_model(&self) -> Layered<RoutingModelChoice> {
        if let Some(choice) = self.project.and_then(|p| p.routing().model()) {
            return Layered::new(choice.clone(), Layer::Project);
        }
        if let Some(choice) = self.user.routing().model() {
            return Layered::new(choice.clone(), Layer::User);
        }
        Layered::new(RoutingModelChoice::Deterministic, Layer::Default)
    }

    /// Maximum router latency, resolved per field so a project can override
    /// this limit without copying any other routing preference.
    pub fn max_router_latency(&self) -> Layered<RouterLatencyMs> {
        if let Some(value) = self.project.and_then(|p| p.routing().max_router_latency()) {
            return Layered::new(value, Layer::Project);
        }
        if let Some(value) = self.user.routing().max_router_latency() {
            return Layered::new(value, Layer::User);
        }
        Layered::new(RouterLatencyMs::DEFAULT, Layer::Default)
    }

    /// Maximum marginal cost of one routing decision, resolved per field.
    pub fn max_router_cost(&self) -> Layered<RouterCostMicroUsd> {
        if let Some(value) = self.project.and_then(|p| p.routing().max_marginal_cost()) {
            return Layered::new(value, Layer::Project);
        }
        if let Some(value) = self.user.routing().max_marginal_cost() {
            return Layered::new(value, Layer::User);
        }
        Layered::new(RouterCostMicroUsd::DEFAULT, Layer::Default)
    }

    /// Whether zero-marginal-cost resources are preferred after capability,
    /// health, rate-limit, and latency requirements are satisfied.
    pub fn prefer_free_routing(&self) -> Layered<bool> {
        if let Some(value) = self.project.and_then(|p| p.routing().prefer_free()) {
            return Layered::new(value, Layer::Project);
        }
        if let Some(value) = self.user.routing().prefer_free() {
            return Layered::new(value, Layer::User);
        }
        Layered::new(true, Layer::Default)
    }

    /// Whether `glasshouse launch` may rank destinations at all — capability
    /// map line 1712.
    ///
    /// Project over user over a default of `true`, like every other routing
    /// preference: the ranking is what Glasshouse has always done, so the
    /// default has to be the behaviour that existed before this switch did.
    ///
    /// `false` does not mean *"route badly"* — it means the launch path takes
    /// no routing decision. What the person's own flags say still happens:
    /// see `main.rs::launch_session`, which reads this before it opens
    /// anything the ranking would have needed.
    pub fn automatic_routing(&self) -> Layered<bool> {
        if let Some(value) = self.project.and_then(|p| p.routing().automatic()) {
            return Layered::new(value, Layer::Project);
        }
        if let Some(value) = self.user.routing().automatic() {
            return Layered::new(value, Layer::User);
        }
        Layered::new(true, Layer::Default)
    }

    /// The sessions the user overrode reserve protection for — capability
    /// map line 1290 — resolved per field, project over user over the empty
    /// default, exactly like [`Self::free_resource_disabled`].
    ///
    /// First layer wins outright rather than the two being unioned. A union
    /// would mean a project could add sessions a user's own configuration
    /// never named and the user had no single place to look to see what was
    /// overridden; every other list on this type resolves the same way, and
    /// this one is a *spending* control, which is the last place to invent a
    /// second resolution rule.
    pub fn reserve_override_sessions(&self) -> Layered<Vec<String>> {
        if let Some(value) = self
            .project
            .and_then(|p| p.routing().reserve_override_sessions())
        {
            return Layered::new(value.to_vec(), Layer::Project);
        }
        if let Some(value) = self.user.routing().reserve_override_sessions() {
            return Layered::new(value.to_vec(), Layer::User);
        }
        Layered::new(Vec::new(), Layer::Default)
    }

    /// The routing-model fallback chain — capability map lines 1423 and
    /// 1795 — resolved per field, project over user over the empty default,
    /// exactly like [`Self::free_resource_order`]. First layer wins outright
    /// rather than the two being concatenated, for
    /// [`Self::reserve_override_sessions`]'s reason: a chain is a list of
    /// models Glasshouse may *call on the user's behalf*, and a project must
    /// not be able to append one the user's own configuration never named.
    pub fn routing_model_fallback(&self) -> Layered<Vec<FreeResourceRef>> {
        if let Some(value) = self.project.and_then(|p| p.routing().model_fallback()) {
            return Layered::new(value.to_vec(), Layer::Project);
        }
        if let Some(value) = self.user.routing().model_fallback() {
            return Layered::new(value.to_vec(), Layer::User);
        }
        Layered::new(Vec::new(), Layer::Default)
    }

    /// Whether classification is confined to local inference — capability
    /// map line 1427 — resolved per field; `false` when neither layer
    /// decided.
    pub fn classification_local_only(&self) -> Layered<bool> {
        if let Some(value) = self
            .project
            .and_then(|p| p.routing().classification_local_only())
        {
            return Layered::new(value, Layer::Project);
        }
        if let Some(value) = self.user.routing().classification_local_only() {
            return Layered::new(value, Layer::User);
        }
        Layered::new(false, Layer::Default)
    }

    /// Premium remaining-capacity threshold below which reserve protection
    /// applies, resolved per field.
    pub fn premium_reserve(&self) -> Layered<PremiumReservePercent> {
        if let Some(value) = self.project.and_then(|p| p.routing().premium_reserve()) {
            return Layered::new(value, Layer::Project);
        }
        if let Some(value) = self.user.routing().premium_reserve() {
            return Layered::new(value, Layer::User);
        }
        Layered::new(PremiumReservePercent::DEFAULT, Layer::Default)
    }

    /// Capacity-band thresholds, resolved per field — capability map line
    /// 1270. [`crate::provider::quota::CapacityBandThresholds::DEFAULT`] when
    /// neither layer recorded any, converted through
    /// [`CapacityBandThresholdsConfig::to_domain`] rather than re-validated
    /// here: it was already validated once, at deserialization.
    pub fn capacity_band_thresholds(
        &self,
    ) -> Layered<crate::provider::quota::CapacityBandThresholds> {
        if let Some(value) = self
            .project
            .and_then(|p| p.routing().capacity_band_thresholds())
        {
            return Layered::new(value.to_domain(), Layer::Project);
        }
        if let Some(value) = self.user.routing().capacity_band_thresholds() {
            return Layered::new(value.to_domain(), Layer::User);
        }
        Layered::new(
            crate::provider::quota::CapacityBandThresholds::DEFAULT,
            Layer::Default,
        )
    }

    /// Routing score weights, resolved per field — capability map lines
    /// 1357/1358. [`crate::routing::session::ScoreWeights::default`] when
    /// neither layer recorded any — today's compile-time constants,
    /// unchanged — converted through [`ScoreWeightsConfig::to_domain`] rather
    /// than re-validated here: it was already validated once, at
    /// deserialization.
    pub fn score_weights(&self) -> Layered<crate::routing::session::ScoreWeights> {
        if let Some(value) = self.project.and_then(|p| p.routing().score_weights()) {
            return Layered::new(value.to_domain(), Layer::Project);
        }
        if let Some(value) = self.user.routing().score_weights() {
            return Layered::new(value.to_domain(), Layer::User);
        }
        Layered::new(
            crate::routing::session::ScoreWeights::default(),
            Layer::Default,
        )
    }

    /// One resource's own protected reserve percentage — capability map line
    /// 1288 — or the global [`EffectiveConfig::premium_reserve`] preference
    /// when the provider has not stated one of its own.
    pub fn reserve_percent(&self, name: &str) -> Layered<PremiumReservePercent> {
        let configured = self.quota_override(name);
        match configured.value.reserve_percent() {
            Some(value) => Layered::new(value, configured.layer),
            None => self.premium_reserve(),
        }
    }

    /// The reserve policy for `scope` — capability map line 1577 — resolved
    /// per field, project over user over
    /// [`crate::routing::pressure::ReservePolicy::Protect`], the fail-closed
    /// default for a spending protection. Per field, so a project that
    /// records only the background policy inherits the user's interactive
    /// one rather than resetting it.
    pub fn reserve_policy(
        &self,
        scope: crate::routing::pressure::ReserveScope,
    ) -> Layered<crate::routing::pressure::ReservePolicy> {
        if let Some(value) = self
            .project
            .and_then(|p| p.routing().reserve())
            .and_then(|reserve| reserve.for_scope(scope))
        {
            return Layered::new(value, Layer::Project);
        }
        if let Some(value) = self
            .user
            .routing()
            .reserve()
            .and_then(|reserve| reserve.for_scope(scope))
        {
            return Layered::new(value, Layer::User);
        }
        Layered::new(
            crate::routing::pressure::ReservePolicy::default(),
            Layer::Default,
        )
    }

    /// Both scopes' policies at once, for a router that carries them whole.
    pub fn reserve_policies(&self) -> crate::routing::pressure::ReservePolicies {
        crate::routing::pressure::ReservePolicies {
            interactive: self
                .reserve_policy(crate::routing::pressure::ReserveScope::Interactive)
                .value,
            background: self
                .reserve_policy(crate::routing::pressure::ReserveScope::Background)
                .value,
        }
    }

    /// Every entitlement this configuration describes (Phase 56 line 1946),
    /// rules already resolved (line 1947): the configured entries by name,
    /// project over user whole (not per field), plus a default entry for
    /// every harness's own sign-in that no configured entry claims through
    /// `native_harness` — [`crate::routing::EntitlementRules::UNRESTRICTED`],
    /// so a user who configured nothing keeps every native launch announcing
    /// an entitlement with no rule.
    ///
    /// Refused rather than resolved by guessing when the two layers together
    /// contradict — see [`EntitlementLookupError`].
    ///
    /// History: design-decisions.md, "Trims: config, checkpoint, evaluation and codex module docs", effective.rs `entitlements`.
    pub fn entitlements(&self) -> Result<Vec<ResolvedEntitlement>, EntitlementLookupError> {
        let mut names: BTreeSet<&str> = self.user.entitlements().names().collect();
        if let Some(project) = self.project {
            names.extend(project.entitlements().names());
        }
        let mut resolved = Vec::with_capacity(names.len());
        for name in names {
            let (config, layer) = match self.project.and_then(|p| p.entitlements().get(name)) {
                Some(config) => (config, Layer::Project),
                None => (
                    self.user
                        .entitlements()
                        .get(name)
                        .expect("a name collected from the user table is in the user table"),
                    Layer::User,
                ),
            };
            resolved.push(config.to_resolved(name, layer)?);
        }

        // Map line 1973: one credential is one account. Two entries naming
        // the same reference would be two names drawing on one account —
        // exactly the mixing the line forbids — so the contradiction is
        // refused by name, like every other one here. The comparison is over
        // references (names), never values: nothing is resolved.
        for (index, entry) in resolved.iter().enumerate() {
            let Some(reference) = entry.credential() else {
                continue;
            };
            let sharers: Vec<String> = resolved[index..]
                .iter()
                .filter(|other| other.credential() == Some(reference))
                .map(|other| other.name.clone())
                .collect();
            if sharers.len() > 1 {
                return Err(EntitlementLookupError::SharedCredential {
                    names: sharers,
                    reference: match reference {
                        SecretRef::Environment { var } => format!("environment variable `{var}`"),
                        SecretRef::OsCredential { service, account } => {
                            format!("OS credential `{service}`/`{account}`")
                        }
                    },
                });
            }
        }

        for harness in IntegrationId::ALL
            .iter()
            .copied()
            .filter(|id| id.kind() == crate::integrations::IntegrationKind::Harness)
        {
            let claimed = resolved
                .iter()
                .any(|entry| entry.backing == EntitlementBacking::NativeHarness(harness));
            if claimed {
                continue;
            }
            if let Some(taken) = resolved.iter().find(|entry| entry.name == harness.slug()) {
                return Err(EntitlementLookupError::NameReservedForHarness {
                    name: taken.name.clone(),
                    harness,
                });
            }
            resolved.push(ResolvedEntitlement {
                name: harness.slug().to_owned(),
                kind: None,
                vendor: None,
                credential: None,
                backing: EntitlementBacking::NativeHarness(harness),
                rules: crate::routing::EntitlementRules::UNRESTRICTED,
                layer: Layer::Default,
                remaining_capacity: None,
                seconds_until_reset: None,
                capacity_scope: None,
                throttling: None,
                models: None,
                spend: None,
                headroom_estimate: None,
                headroom_override: None,
                disable_headroom_estimate: false,
                context_firewall: firewall::ContextFirewallOverride::default(),
            });
        }
        Ok(resolved)
    }

    /// Every entitlement the *user or project actually wrote* — the resolved
    /// list without the per-harness defaults [`Self::entitlements`]
    /// synthesises. The defaults exist so an unconfigured launch has an
    /// entitlement to announce; a listing of the user's configured accounts
    /// that included eight synthetic entries would bury the two real ones.
    pub fn configured_entitlements(
        &self,
    ) -> Result<Vec<ResolvedEntitlement>, EntitlementLookupError> {
        Ok(self
            .entitlements()?
            .into_iter()
            .filter(|entry| entry.layer() != Layer::Default)
            .collect())
    }

    /// Map line 1965's resolver: the configured pool with every telemetry
    /// facet populated from what `telemetry` actually holds — see
    /// [`ResolvedEntitlement::with_telemetry`] for what each facet reads and
    /// the scope every reading carries. One resolver for all entries, so two
    /// entitlements of one provider cannot be handed different provider-wide
    /// readings.
    pub fn configured_entitlements_with_telemetry(
        &self,
        telemetry: &EntitlementTelemetry<'_>,
    ) -> Result<Vec<ResolvedEntitlement>, EntitlementLookupError> {
        Ok(self
            .configured_entitlements()?
            .into_iter()
            .map(|entry| entry.with_telemetry(telemetry))
            .collect())
    }

    /// Every configured entitlement as a resource the registry can name —
    /// map line 1963's *"several entitlements of the same vendor and plan
    /// coexist in one pool"*, as the enumeration `glasshouse status` prints.
    ///
    /// One [`crate::provider::registry::ResourceKind::Entitlement`] per
    /// configured **entry, keyed by name and by nothing else** — never
    /// deduplicated by vendor, kind or backing, because two accounts of one
    /// vendor being two resources is the entire point of the pool.
    pub fn entitlement_resources(
        &self,
    ) -> Result<Vec<crate::provider::registry::ResourceKind>, EntitlementLookupError> {
        Ok(self
            .configured_entitlements()?
            .into_iter()
            .map(|entry| crate::provider::registry::ResourceKind::Entitlement { name: entry.name })
            .collect())
    }

    /// The entitlement a session on `harness` over `backend` would be charged
    /// to — line 1954's *which subscription*, resolved once here for the
    /// router (`main.rs::routing_destinations` attaches it to every
    /// destination) and for the announcement, so the two cannot disagree.
    ///
    /// `Ok(None)` is a real answer and means *no entitlement describes this
    /// resource*: a direct provider no entry names, or the Glasshouse gateway,
    /// whose upstream is assigned when the session starts. No rule can refuse
    /// a resource no rule describes, and the announcement says so rather than
    /// naming one. A harness's own sign-in is never `None`, because
    /// [`Self::entitlements`] supplies its default.
    pub fn entitlement_for(
        &self,
        harness: IntegrationId,
        backend: &crate::profile::BackendResource,
    ) -> Result<Option<ResolvedEntitlement>, EntitlementLookupError> {
        use crate::profile::BackendResource;

        let wanted = match backend {
            BackendResource::Native => EntitlementBacking::NativeHarness(harness),
            BackendResource::DirectProvider { provider } => {
                EntitlementBacking::Provider(provider.clone())
            }
            BackendResource::GlasshouseGateway => return Ok(None),
        };
        let mut matching: Vec<ResolvedEntitlement> = self
            .entitlements()?
            .into_iter()
            .filter(|entry| entry.backing == wanted)
            .collect();
        match matching.len() {
            0 => Ok(None),
            1 => Ok(matching.pop()),
            _ => {
                let names = matching.into_iter().map(|entry| entry.name).collect();
                Err(match wanted {
                    EntitlementBacking::NativeHarness(harness) => {
                        EntitlementLookupError::AmbiguousNativeHarness { harness, names }
                    }
                    EntitlementBacking::Provider(provider) => {
                        EntitlementLookupError::AmbiguousProvider { provider, names }
                    }
                    EntitlementBacking::Unstated => {
                        unreachable!("`wanted` is built from a backend and is never Unstated")
                    }
                })
            }
        }
    }

    /// Map line 1973's isolation, on the path that actually leaks: the child
    /// process inherits this whole process's environment, so every *other*
    /// entitlement's environment-variable credential would ride along into a
    /// session charged to one account — two accounts' keys mixed in one
    /// child, which is exactly what the line forbids. This names the
    /// variables to remove from a launch: the environment-shaped credential
    /// reference of every entitlement that is not the one serving this
    /// session. The serving entitlement's own variable stays; an
    /// OS-credential reference has no variable to leak; and a user with no
    /// `[entitlements]` entries gets an empty list, so nothing about an
    /// unconfigured launch changes.
    ///
    /// Called with the launch's resolved entitlement name, or `None` for a
    /// session no entitlement describes — which scrubs every entitlement's
    /// variable, because a session charged to no account has no business
    /// carrying any account's key. Tables that do not resolve scrub nothing:
    /// the launch path has already refused or reported that error before any
    /// process exists.
    pub fn foreign_entitlement_credential_vars(&self, serving: Option<&str>) -> Vec<String> {
        let Ok(entitlements) = self.entitlements() else {
            return Vec::new();
        };
        entitlements
            .iter()
            .filter(|entry| serving != Some(entry.name()))
            .filter_map(|entry| match entry.credential() {
                Some(SecretRef::Environment { var }) => Some(var.clone()),
                _ => None,
            })
            .collect()
    }

    /// The credential **references** of every entitlement that is not the
    /// one serving — [`Self::foreign_entitlement_credential_vars`]'s twin
    /// for the resolution side (56A line 1969): a launch bound to the
    /// pool's chosen account must not *resolve* a sibling account's
    /// credential either, or the overlay's "first reference that resolves"
    /// rule would quietly re-bind the process to whichever account is
    /// listed first. Same filter, same `None`-scrubs-everything contract,
    /// and both reference shapes rather than only the environment one — an
    /// OS-credential reference has no variable to scrub from a child, but
    /// it can absolutely be resolved, so the resolution side must know it.
    pub fn foreign_entitlement_credential_refs(&self, serving: Option<&str>) -> Vec<SecretRef> {
        let Ok(entitlements) = self.entitlements() else {
            return Vec::new();
        };
        entitlements
            .iter()
            .filter(|entry| serving != Some(entry.name()))
            .filter_map(|entry| entry.credential().cloned())
            .collect()
    }

    /// The entitlement charged for work sent to `provider` — map line 1947's
    /// job-kind clause, for the disposable router: a bounded support job has
    /// no harness and no launch profile, only the provider its candidate
    /// names, so this is [`Self::entitlement_for`]'s provider arm with no
    /// harness in the question. `Ok(None)` means no entry names the
    /// provider, and no rule can refuse a resource no rule describes.
    pub fn entitlement_for_provider(
        &self,
        provider: &str,
    ) -> Result<Option<ResolvedEntitlement>, EntitlementLookupError> {
        self.entitlement_for(
            // Any harness id serves: the provider arm below never reads it.
            IntegrationId::ClaudeCode,
            &crate::profile::BackendResource::DirectProvider {
                provider: provider.to_owned(),
            },
        )
    }

    /// Whether `model` on `provider` costs the user anything at the margin —
    /// [`ProviderConfig::cost_of`], read from the layer that configures the
    /// provider (project over user), and
    /// [`crate::routing::Cost::Metered`] when neither does: a provider nobody
    /// configured is not one anybody marked free.
    pub fn model_cost(&self, provider: &str, model: &str) -> Layered<crate::routing::Cost> {
        if let Some(config) = self.project.and_then(|p| p.providers().get(provider)) {
            return Layered::new(config.cost_of(model), Layer::Project);
        }
        if let Some(config) = self.user.providers().get(provider) {
            return Layered::new(config.cost_of(model), Layer::User);
        }
        Layered::new(crate::routing::Cost::Metered, Layer::Default)
    }

    /// `model`'s declared resource facts on `provider` — map line 1517's
    /// producer, read from the layer that configures the provider (project
    /// over user), exactly as [`EffectiveConfig::model_cost`] and
    /// [`EffectiveConfig::model_ceiling`] read beside it.
    ///
    /// [`crate::routing::capability::ResourceFacts::UNVERIFIED`] when
    /// neither layer configures the provider, or when the configuring layer
    /// declares no facts for this model — both are *not established*, the
    /// same "nobody has said" reading [`EffectiveConfig::model_ceiling`]'s
    /// own doc gives a `None` ceiling.
    pub fn model_facts(
        &self,
        provider: &str,
        model: &str,
    ) -> Layered<crate::routing::capability::ResourceFacts> {
        if let Some(config) = self.project.and_then(|p| p.providers().get(provider)) {
            return Layered::new(
                config.resource_facts_of(model, Layer::Project),
                Layer::Project,
            );
        }
        if let Some(config) = self.user.providers().get(provider) {
            return Layered::new(config.resource_facts_of(model, Layer::User), Layer::User);
        }
        Layered::new(
            crate::routing::capability::ResourceFacts::UNVERIFIED,
            Layer::Default,
        )
    }

    /// The highest workload tier `model` on `provider` is established to
    /// serve — map line 1796, read from the layer that configures the
    /// provider (project over user). `None` when the configuring layer states
    /// no ceiling, and `None` when no layer configures the provider at all —
    /// both are *not established*, and the tier gate does nothing to a
    /// destination carrying one. The layer is still reported, so a reader can
    /// tell "no ceiling stated" from "nothing configures this provider".
    ///
    /// Reads through [`ProviderConfig::resolved_ceiling`], which also honours
    /// a Phase 34F capability-record ceiling once no override states one —
    /// [`capability::CeilingResolution::hard_ceiling`] keeps a
    /// benchmark-provenance record out of this value: only the user's own
    /// word, override or capability record, may narrow what a destination is
    /// established to serve.
    ///
    /// History: design-decisions.md, "Trims: config, checkpoint, evaluation and codex module docs", effective.rs `model_ceiling`.
    pub fn model_ceiling(
        &self,
        provider: &str,
        model: &str,
    ) -> Layered<Option<crate::routing::classify::WorkloadTier>> {
        if let Some(config) = self.project.and_then(|p| p.providers().get(provider)) {
            return Layered::new(
                config.resolved_ceiling(model).hard_ceiling(),
                Layer::Project,
            );
        }
        if let Some(config) = self.user.providers().get(provider) {
            return Layered::new(config.resolved_ceiling(model).hard_ceiling(), Layer::User);
        }
        Layered::new(None, Layer::Default)
    }

    /// [`Self::model_ceiling`], through [`ProviderConfig::resolved_ceiling_for`]
    /// rather than [`ProviderConfig::resolved_ceiling`] — capability map line
    /// 1482's closing half, for a caller that has harness, launch-profile, or
    /// protocol context to narrow a capability record by. Layer precedence
    /// and every other rule match [`Self::model_ceiling`] exactly; only the
    /// record filter differs.
    pub fn model_ceiling_for(
        &self,
        provider: &str,
        model: &str,
        query: &capability::CapabilityQuery<'_>,
    ) -> Layered<Option<crate::routing::classify::WorkloadTier>> {
        if let Some(config) = self.project.and_then(|p| p.providers().get(provider)) {
            return Layered::new(
                config.resolved_ceiling_for(model, query).hard_ceiling(),
                Layer::Project,
            );
        }
        if let Some(config) = self.user.providers().get(provider) {
            return Layered::new(
                config.resolved_ceiling_for(model, query).hard_ceiling(),
                Layer::User,
            );
        }
        Layered::new(None, Layer::Default)
    }

    /// Every `(provider, model)` pair a calibrated [`capability::ModelCapabilityRecord`]
    /// actually decides the ceiling for, from the layer that configures that
    /// provider — project over user, matching every other lookup on this
    /// type. Map line 1481's own enumeration: the calibrated data a
    /// suggestion compares observed outcomes against.
    ///
    /// A pair whose resolution is [`capability::CeilingResolution::UserOverride`]
    /// is left out: [`ProviderConfig::model_ceilings`]'s own override
    /// is what actually governs that destination, so a capability record
    /// sitting unused beside it is not this line's to suggest changes to.
    /// Context-blind on purpose — a project-wide report has no one
    /// destination's harness or launch profile to narrow by, the same
    /// "genuinely no context" case [`ProviderConfig::resolved_ceiling`]
    /// documents for its own caller.
    pub fn calibrated_model_ceilings(
        &self,
    ) -> Vec<(String, String, capability::CeilingResolution)> {
        let mut out = Vec::new();
        for name in self.provider_names() {
            let config = self
                .project
                .and_then(|p| p.providers().get(&name))
                .or_else(|| self.user.providers().get(&name));
            let Some(config) = config else { continue };
            for model in config.model_capabilities().keys() {
                let resolution = config.resolved_ceiling(model);
                if matches!(
                    resolution,
                    capability::CeilingResolution::UserCapabilityRecord(_)
                        | capability::CeilingResolution::Prior(_)
                ) {
                    out.push((name.clone(), model.clone(), resolution));
                }
            }
        }
        out
    }

    /// The user's preferred order over free resources, resolved per field —
    /// Phase 9I line 536.
    pub fn free_resource_order(&self) -> Layered<Vec<FreeResourceRef>> {
        if let Some(value) = self.project.and_then(|p| p.routing().free_resource_order()) {
            return Layered::new(value.to_vec(), Layer::Project);
        }
        if let Some(value) = self.user.routing().free_resource_order() {
            return Layered::new(value.to_vec(), Layer::User);
        }
        Layered::new(Vec::new(), Layer::Default)
    }

    /// Free resources the user has disabled, resolved per field.
    pub fn free_resource_disabled(&self) -> Layered<Vec<FreeResourceRef>> {
        if let Some(value) = self
            .project
            .and_then(|p| p.routing().free_resource_disabled())
        {
            return Layered::new(value.to_vec(), Layer::Project);
        }
        if let Some(value) = self.user.routing().free_resource_disabled() {
            return Layered::new(value.to_vec(), Layer::User);
        }
        Layered::new(Vec::new(), Layer::Default)
    }

    /// The user's pinned free resource, resolved per field.
    pub fn free_resource_pin(&self) -> Layered<Option<FreeResourceRef>> {
        if let Some(value) = self.project.and_then(|p| p.routing().free_resource_pin()) {
            return Layered::new(Some(value.clone()), Layer::Project);
        }
        if let Some(value) = self.user.routing().free_resource_pin() {
            return Layered::new(Some(value.clone()), Layer::User);
        }
        Layered::new(None, Layer::Default)
    }

    /// What will actually classify a request: the recorded choice from
    /// [`EffectiveConfig::routing_model`], checked against the providers
    /// that are configured right now.
    ///
    /// This never fails. A pinned model whose provider has since been
    /// removed degrades to [`RoutingModelResolution::Heuristics`] carrying a
    /// [`RoutingFallback`] that says which one went missing — see
    /// [`RoutingModelChoice::resolve`] for why this is the one lookup here
    /// that will not return an error. The [`Layer`] reported is the layer the
    /// *choice* came from, not a claim about where the degrade was decided.
    ///
    /// A choice nothing was ever recorded for reports
    /// [`RoutingFallback::NotConfigured`] rather than
    /// [`RoutingFallback::DeterministicChosen`], so a user who declined the
    /// wizard's routing step and a user who deliberately picked
    /// deterministic-only are told different, accurate things.
    pub fn routing_model_resolution(&self) -> Layered<RoutingModelResolution> {
        let Layered { value, layer } = self.routing_model();
        let mut resolution = value.resolve(&self.provider_names());
        if layer == Layer::Default
            && let RoutingModelResolution::Heuristics(reason) = &mut resolution
        {
            *reason = RoutingFallback::NotConfigured;
        }
        Layered::new(resolution, layer)
    }

    /// Each layer's `[pairing]` table, in the order corrections are applied
    /// — user first, project second, so a report reads in the order
    /// [`EffectiveConfig::pairing_overrides`] merges them.
    ///
    /// The [`EffectiveConfig`] fields are private and this type is `Copy`,
    /// so a caller outside this module cannot reach a layer's table any
    /// other way; a report that wants to show *which file* a correction came
    /// from needs exactly this.
    pub fn pairing_layers(&self) -> Vec<(Layer, &pairing::PairingConfig)> {
        let mut layers = vec![(Layer::User, self.user.pairing())];
        if let Some(project) = self.project {
            layers.push((Layer::Project, project.pairing()));
        }
        layers
    }

    /// Resolve `name` to a [`crate::provider::Provider`], reporting which
    /// layer supplied it. The project layer's definition wins over the user
    /// layer's, matching every other lookup on this type.
    pub fn configured_provider(
        &self,
        name: &str,
    ) -> Result<Layered<crate::provider::Provider>, ProviderLookupError> {
        let found = if let Some(config) = self.project.and_then(|p| p.providers().get(name)) {
            Some((config, Layer::Project))
        } else {
            self.user
                .providers()
                .get(name)
                .map(|config| (config, Layer::User))
        };

        let Some((config, layer)) = found else {
            return Err(ProviderLookupError::Unknown {
                name: name.to_owned(),
                known: self.provider_names(),
            });
        };

        let mut provider = config.to_provider(name)?;
        config.declare_tool_calls(&mut provider, layer);
        Ok(Layered::new(provider, layer))
    }

    /// What the user configured about `name`'s quota, reporting which layer
    /// supplied it — capability map lines 1233, 1203 and 1237.
    ///
    /// # Whole-table precedence, deliberately
    ///
    /// The project layer's `[providers.<name>.quota]` table replaces the
    /// user layer's rather than merging field by field, matching
    /// [`ProviderConfig::credential_env`]'s and
    /// [`ProviderConfig::headers`]'s own replace-not-merge rule and the
    /// [`EffectiveConfig::configured_provider`] lookup this sits beside — a
    /// project that states a quota table has stated the whole of what it
    /// believes about that provider's quota. A per-field merge would let a
    /// project's `plan` silently inherit a user's `budget`, which is a
    /// spending ceiling arriving somewhere nobody wrote it.
    ///
    /// [`Layer::Default`] with an empty override when neither layer says
    /// anything, which is not the same as the provider being unknown: this
    /// answers a question about configuration and an unconfigured provider
    /// has, correctly, no overrides.
    pub fn quota_override(&self, name: &str) -> Layered<QuotaOverride> {
        if let Some(quota) = self
            .project
            .and_then(|p| p.providers().get(name))
            .and_then(ProviderConfig::quota)
        {
            return Layered::new(quota.clone(), Layer::Project);
        }
        if let Some(quota) = self
            .user
            .providers()
            .get(name)
            .and_then(ProviderConfig::quota)
        {
            return Layered::new(quota.clone(), Layer::User);
        }
        Layered::new(QuotaOverride::default(), Layer::Default)
    }

    /// How long `name`'s quota telemetry stays current — capability map
    /// line 1237.
    ///
    /// Resolved through [`EffectiveConfig::quota_override`], so a project
    /// layer's age wins over a user layer's, and
    /// [`QuotaStaleAfterSeconds::DEFAULT`] when neither said. **The value is
    /// per provider**, which is the whole of the line's "provider-specific":
    /// asking this for two providers may legitimately give two answers, and a
    /// caller that read it once and reused it would have flattened exactly
    /// the distinction the line asks for.
    pub fn quota_stale_after(&self, name: &str) -> Layered<QuotaStaleAfterSeconds> {
        let configured = self.quota_override(name);
        match configured.value.stale_after() {
            Some(value) => Layered::new(value, configured.layer),
            None => Layered::new(QuotaStaleAfterSeconds::DEFAULT, Layer::Default),
        }
    }
}
/// Why a launch profile named on the command line could not be resolved.
#[derive(Debug, thiserror::Error)]
pub enum ProfileLookupError {
    #[error("`{name}` is not a known launch profile; valid names are: {}", .known.join(", "))]
    Unknown { name: String, known: Vec<String> },
    #[error(
        "launch profile `{name}` is for {}, not {}; name the harness the profile itself \
         belongs to, or choose a different profile",
        .profile_harness.display_name(), .requested_harness.display_name()
    )]
    HarnessMismatch {
        name: String,
        profile_harness: IntegrationId,
        requested_harness: IntegrationId,
    },
    #[error(transparent)]
    Invalid(#[from] ProfileConfigError),
}
/// A launch profile a person named explicitly is disabled.
///
/// Deliberately **not** a [`ProfileLookupError`] variant.
/// [`EffectiveConfig::launch_profile`] never returns this and must not: it is
/// the resolver *every* path uses, and a session already running under a
/// profile since disabled has to stay resumable — `enabled` decides what may
/// be **started**, not what may be continued, which is the same reading
/// `resume`'s own profile fallback already took. So this is a separate
/// refusal raised at the one place a session is started from a name a person
/// typed.
///
/// The message carries the profile name and the two ways to undo it, and no
/// path: see [`Layer::describe_source`].
#[derive(Debug, thiserror::Error)]
#[error(
    "launch profile `{name}` is disabled {}; re-enable it in the Settings screen's \
     Launch Profiles section, or set `enabled = true` under `[profiles.{name}]`",
    .layer.describe_source()
)]
pub struct ProfileDisabled {
    pub name: String,
    pub layer: Layer,
}
impl ProfileDisabled {
    pub fn new(name: impl Into<String>, layer: Layer) -> Self {
        Self {
            name: name.into(),
            layer,
        }
    }
}
