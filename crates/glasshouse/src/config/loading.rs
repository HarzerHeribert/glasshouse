//! `UserConfig` and `ProjectConfig`: the two on-disk layers, their TOML load/save, and the shared `[guardrails]`/`[memory]` tables.
//!

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::guardrails::{BlockingCategory, GuardrailMode};
use crate::paths::RuntimePaths;
use crate::project::Project;

use super::*;

/// Configuration schema version this build of Glasshouse writes and fully
/// understands. Bump this only when the schema changes in a way that
/// matters for [`UserConfig::save`]'s forward-compatibility check below.
pub(super) const CURRENT_SCHEMA_VERSION: u32 = 1;
fn current_schema_version() -> u32 {
    CURRENT_SCHEMA_VERSION
}
/// Relative path of the optional project-level configuration file, inside
/// the project root.
pub(super) const PROJECT_CONFIG_RELATIVE_PATH: &str = ".glasshouse/config.toml";
/// Onboarding progress, persisted so the first-run wizard runs at most once
/// per user (Phase 2C: "Persist onboarding choices in user-level Glasshouse
/// configuration").
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnboardingState {
    #[serde(default)]
    completed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    completed_at_version: Option<String>,
}
impl OnboardingState {
    pub fn completed(&self) -> bool {
        self.completed
    }

    /// The Glasshouse version that was running when onboarding was last
    /// completed, if known. Informational only (e.g. for deciding whether a
    /// changelog-driven "what's new" prompt applies) — nothing in this
    /// module gates behavior on it.
    pub fn completed_at_version(&self) -> Option<&str> {
        self.completed_at_version.as_deref()
    }

    pub fn mark_completed(&mut self, version: impl Into<String>) {
        self.completed = true;
        self.completed_at_version = Some(version.into());
    }

    /// Reset onboarding so the wizard runs again. Phase 2C: "Allow the
    /// onboarding wizard to be reopened later from settings."
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}
/// The `[guardrails]` table — Phase 21K, capability map lines 1008, 1052.
///
/// Two preferences, both optional so that a project can override one
/// without restating the other, and both `None` for "this layer never
/// decided" — the same three-state reasoning [`RoutingConfig::model`] gives.
/// The vocabularies are [`crate::guardrails`]' own, so a value this file
/// accepts is a value the gate understands, and the shipped defaults live
/// with the gate (`Policy::default_policy`) rather than here.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardrailsConfig {
    /// `off`, `advisory` (the default) or `risk_gated`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mode: Option<GuardrailMode>,
    /// Which categories may answer `gated` under `risk_gated`. Only the four
    /// [`BlockingCategory`] names parse; anything else is a load error that
    /// names the vocabulary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    blocking: Option<Vec<BlockingCategory>>,
}
impl GuardrailsConfig {
    /// Whether this layer recorded nothing at all — the `skip_serializing_if`
    /// predicate, so a user who never touched the guardrails has no
    /// `[guardrails]` table in their file.
    pub fn is_unset(&self) -> bool {
        self.mode.is_none() && self.blocking.is_none()
    }

    pub fn mode(&self) -> Option<GuardrailMode> {
        self.mode
    }

    pub fn set_mode(&mut self, mode: Option<GuardrailMode>) -> &mut Self {
        self.mode = mode;
        self
    }

    pub fn blocking(&self) -> Option<&[BlockingCategory]> {
        self.blocking.as_deref()
    }

    pub fn set_blocking(&mut self, blocking: Option<Vec<BlockingCategory>>) -> &mut Self {
        self.blocking = blocking;
        self
    }
}
/// The `[memory]` table — `GH-LAUNCH-BRIEFING`, the launch-path opt-out named
/// by the design ruling *"Memory is the project's, not the launch path's"*
/// (`docs/product/design-decisions.md`).
///
/// One field today, kept in its own table rather than as a bare top-level key
/// like [`UserConfig::memory_extraction`]: unlike that flag, this is a
/// user-facing product setting a person is expected to reach for by name
/// (`[memory] inject_at_launch = false`), not an internal automatic-behaviour
/// toggle.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// Whether `glasshouse launch` briefs a new session with this project's
    /// memory through the harness's own additive mechanism. `None` means
    /// "never decided" and resolves to enabled — see
    /// [`EffectiveConfig::inject_memory_at_launch`]. Opt-out, not opt-in: the
    /// ruling is that a plain launch briefs by default, the same way a
    /// door-spawned session already does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    inject_at_launch: Option<bool>,
    /// Which model may rerank the top lexical memory candidates before
    /// selection — map line 1089. `None` means no consent was ever given and
    /// resolves to "no model is ever called", the same shape
    /// [`UserConfig::memory_extraction_model`] uses for the same reason: this
    /// is the whole of the consent, and nothing else turns reranking into an
    /// outbound request. See [`EffectiveConfig::memory_rerank_model`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rerank_model: Option<ExtractionModelRef>,
    /// Whether a briefing appends one JSON line describing its retrieval and
    /// rerank decision to `<state_dir>/memory-retrieval.jsonl` — map line
    /// 1094. `None` resolves to `false`: diagnostics are off by default, and
    /// turning them on is an explicit ask. See
    /// [`EffectiveConfig::memory_retrieval_diagnostics`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    retrieval_diagnostics: Option<bool>,
    /// Whether an extraction run appends one JSON line describing its
    /// inputs and outputs to `<state_dir>/memory-extraction.jsonl` — map
    /// line 1769. `None` resolves to `false`, the same off-by-default
    /// direction [`Self::retrieval_diagnostics`] takes and for the same
    /// reason: this is a debugging surface, not something a project should
    /// pay to write on every extraction run unless it asked. See
    /// [`EffectiveConfig::memory_extraction_diagnostics`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    extraction_diagnostics: Option<bool>,
}
impl MemoryConfig {
    /// Whether this layer recorded nothing at all — the `skip_serializing_if`
    /// predicate, so a user who never touched this has no `[memory]` table in
    /// their file.
    pub fn is_unset(&self) -> bool {
        self.inject_at_launch.is_none()
            && self.rerank_model.is_none()
            && self.retrieval_diagnostics.is_none()
            && self.extraction_diagnostics.is_none()
    }

    pub fn inject_at_launch(&self) -> Option<bool> {
        self.inject_at_launch
    }

    pub fn set_inject_at_launch(&mut self, enabled: Option<bool>) -> &mut Self {
        self.inject_at_launch = enabled;
        self
    }

    pub fn rerank_model(&self) -> Option<&ExtractionModelRef> {
        self.rerank_model.as_ref()
    }

    pub fn set_rerank_model(&mut self, model: Option<ExtractionModelRef>) -> &mut Self {
        self.rerank_model = model;
        self
    }

    pub fn retrieval_diagnostics(&self) -> Option<bool> {
        self.retrieval_diagnostics
    }

    pub fn set_retrieval_diagnostics(&mut self, enabled: Option<bool>) -> &mut Self {
        self.retrieval_diagnostics = enabled;
        self
    }

    pub fn extraction_diagnostics(&self) -> Option<bool> {
        self.extraction_diagnostics
    }

    pub fn set_extraction_diagnostics(&mut self, enabled: Option<bool>) -> &mut Self {
        self.extraction_diagnostics = enabled;
        self
    }
}
/// User-level Glasshouse configuration: `<config_dir>/config.toml`.
///
/// Unknown top-level keys and unknown fields inside known tables are
/// ignored on load rather than rejected, so a file written by a newer
/// Glasshouse still loads here (see [`ConfigError::UnsupportedVersion`] for
/// what still gets refused: writing it back).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserConfig {
    #[serde(default = "current_schema_version")]
    version: u32,
    #[serde(default)]
    onboarding: OnboardingState,
    #[serde(default)]
    integrations: IntegrationTable,
    #[serde(default)]
    profiles: ProfileTable,
    #[serde(default)]
    providers: ProviderTable,
    /// `[entitlements.<name>]` — Phase 56. Skipped when empty: a user who
    /// configured none has no table, and every harness's own sign-in still
    /// resolves to a default entry through
    /// [`EffectiveConfig::entitlements`].
    #[serde(default, skip_serializing_if = "EntitlementTable::is_empty")]
    entitlements: EntitlementTable,
    /// Skipped when empty so a first run that declines the routing-model
    /// step writes no `[routing]` table at all — see [`RoutingConfig::model`].
    #[serde(default, skip_serializing_if = "RoutingConfig::is_unset")]
    routing: RoutingConfig,
    /// Pairing metadata corrections — Phase 9J line 561. Skipped when empty
    /// for the same reason `routing` is: a user who never corrected a
    /// pairing has no `[pairing]` table in their file at all.
    #[serde(default, skip_serializing_if = "pairing::PairingConfig::is_unset")]
    pairing: pairing::PairingConfig,
    /// Response-profile configuration — Phase 9K lines 593–597. Skipped when
    /// empty for the same reason `pairing` is: a user who never chose a
    /// response profile has no `[response]` table, and Glasshouse applies
    /// nothing to their harness.
    #[serde(default, skip_serializing_if = "response::ResponseConfig::is_unset")]
    response: response::ResponseConfig,
    /// Whether Glasshouse's automatic post-turn memory-extraction trigger
    /// (Phase 21) may run in this project. `None` means "never decided" and
    /// resolves to enabled — the same reasoning [`RoutingConfig::model`]
    /// documents for why this stays an `Option` rather than a plain `bool`:
    /// a project that wants to record an explicit "off" over a user-level
    /// "on" needs a third state to override, not just two.
    ///
    /// Independent of [`RoutingConfig::model`] and
    /// [`response::ResponseConfig`]'s own `enabled` field by construction —
    /// each lives in its own table/field and is read by
    /// [`EffectiveConfig::memory_extraction_enabled`] alone, so setting one
    /// never touches another. See
    /// `tests::the_three_automatic_behaviours_disable_independently`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    memory_extraction: Option<bool>,
    /// Whether Glasshouse may take a checkpoint automatically at a task
    /// boundary (Phase 19 line 802), without being asked. `None` means
    /// "never decided" and resolves to enabled, for the same reason
    /// [`Self::memory_extraction`] stays an `Option` rather than a plain
    /// `bool`.
    ///
    /// Independent of [`Self::memory_extraction`] and every other automatic
    /// behaviour by construction — this is its own field, read by
    /// [`EffectiveConfig::automatic_checkpoint_enabled`] alone, so disabling
    /// memory extraction never disables this and vice versa.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    automatic_checkpoint: Option<bool>,
    /// Whether Glasshouse delivers its own project implementation policy
    /// (`crate::policy`) to an agent it briefs. `None` means "never decided"
    /// and resolves to enabled, for the same reason
    /// [`Self::memory_extraction`] stays an `Option` rather than a plain
    /// `bool`.
    ///
    /// **The default is on, and that is the decision rather than the safe
    /// choice**: a policy nobody receives is not a policy, and an off default
    /// silently wins every comparison nobody runs. A team that does not want
    /// Glasshouse speaking into its agents' context turns it off here, and
    /// what remains is coherent — the briefing and the task arrive exactly as
    /// they did before the policy existed.
    ///
    /// Independent of [`Self::memory_extraction`] and every other automatic
    /// behaviour by construction: its own field, read by
    /// [`EffectiveConfig::implementation_policy_enabled`] alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    implementation_policy: Option<bool>,
    /// Which model performs memory extraction, or `None` for "the user has
    /// not chosen one" — Phase 21 line 834. See [`ExtractionModelRef`] for
    /// why this is a field of its own rather than a reading of the routing
    /// preferences, and [`EffectiveConfig::memory_extraction_model`] for how
    /// a project may override it.
    ///
    /// **`None` is the default and means no model is ever called.** This is
    /// the whole of the consent: nothing else in this file, and nothing in
    /// the provider table, turns memory extraction into an outbound request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    memory_extraction_model: Option<ExtractionModelRef>,
    /// The assumption guardrail's mode and blocking list — Phase 21K. Skipped
    /// when empty for the same reason `routing` is. Read by
    /// [`EffectiveConfig::guardrail_mode`] and
    /// [`EffectiveConfig::guardrail_blocking`] alone, so setting it never
    /// touches another automatic behaviour.
    #[serde(default, skip_serializing_if = "GuardrailsConfig::is_unset")]
    guardrails: GuardrailsConfig,
    /// The context firewall's mode and thresholds — Phase 57 map lines
    /// 1991-1996. Skipped when empty for the same reason `guardrails` is: a
    /// user who never touched it has no `[context_firewall]` table, and
    /// [`EffectiveConfig::context_firewall_mode`] resolves the missing case
    /// to `off`.
    #[serde(
        default,
        skip_serializing_if = "firewall::ContextFirewallConfig::is_unset"
    )]
    context_firewall: firewall::ContextFirewallConfig,
    /// The `[memory]` table — see [`MemoryConfig`] and
    /// [`EffectiveConfig::inject_memory_at_launch`].
    #[serde(default, skip_serializing_if = "MemoryConfig::is_unset")]
    memory: MemoryConfig,
}
impl Default for UserConfig {
    fn default() -> Self {
        Self {
            version: CURRENT_SCHEMA_VERSION,
            onboarding: OnboardingState::default(),
            integrations: IntegrationTable::default(),
            profiles: ProfileTable::default(),
            providers: ProviderTable::default(),
            entitlements: EntitlementTable::default(),
            routing: RoutingConfig::default(),
            pairing: pairing::PairingConfig::default(),
            response: response::ResponseConfig::default(),
            memory_extraction: None,
            automatic_checkpoint: None,
            implementation_policy: None,
            memory_extraction_model: None,
            memory: MemoryConfig::default(),
            guardrails: GuardrailsConfig::default(),
            context_firewall: firewall::ContextFirewallConfig::default(),
        }
    }
}
impl UserConfig {
    pub fn version(&self) -> u32 {
        self.version
    }

    pub fn onboarding(&self) -> &OnboardingState {
        &self.onboarding
    }

    pub fn onboarding_mut(&mut self) -> &mut OnboardingState {
        &mut self.onboarding
    }

    pub fn integrations(&self) -> &IntegrationTable {
        &self.integrations
    }

    pub fn integrations_mut(&mut self) -> &mut IntegrationTable {
        &mut self.integrations
    }

    pub fn profiles(&self) -> &ProfileTable {
        &self.profiles
    }

    pub fn profiles_mut(&mut self) -> &mut ProfileTable {
        &mut self.profiles
    }

    pub fn providers(&self) -> &ProviderTable {
        &self.providers
    }

    pub fn providers_mut(&mut self) -> &mut ProviderTable {
        &mut self.providers
    }

    pub fn entitlements(&self) -> &EntitlementTable {
        &self.entitlements
    }

    pub fn entitlements_mut(&mut self) -> &mut EntitlementTable {
        &mut self.entitlements
    }

    pub fn routing(&self) -> &RoutingConfig {
        &self.routing
    }

    pub fn routing_mut(&mut self) -> &mut RoutingConfig {
        &mut self.routing
    }

    pub fn pairing(&self) -> &pairing::PairingConfig {
        &self.pairing
    }

    pub fn pairing_mut(&mut self) -> &mut pairing::PairingConfig {
        &mut self.pairing
    }

    pub fn response(&self) -> &response::ResponseConfig {
        &self.response
    }

    pub fn response_mut(&mut self) -> &mut response::ResponseConfig {
        &mut self.response
    }

    /// This layer's recorded decision on the automatic memory-extraction
    /// trigger, or `None` for "never decided". See the field's own doc.
    pub fn memory_extraction(&self) -> Option<bool> {
        self.memory_extraction
    }

    /// The user's decision on delivering the implementation policy, or `None`
    /// for "never decided". See [`Self::implementation_policy`].
    pub fn implementation_policy(&self) -> Option<bool> {
        self.implementation_policy
    }

    pub fn set_implementation_policy(&mut self, enabled: Option<bool>) -> &mut Self {
        self.implementation_policy = enabled;
        self
    }

    pub fn set_memory_extraction(&mut self, enabled: Option<bool>) -> &mut Self {
        self.memory_extraction = enabled;
        self
    }
    /// The model this user has chosen to perform memory extraction, or
    /// `None` — see [`UserConfig::memory_extraction_model`].
    pub fn memory_extraction_model(&self) -> Option<&ExtractionModelRef> {
        self.memory_extraction_model.as_ref()
    }

    pub fn set_memory_extraction_model(&mut self, model: Option<ExtractionModelRef>) -> &mut Self {
        self.memory_extraction_model = model;
        self
    }

    /// This layer's `[guardrails]` table — see [`GuardrailsConfig`].
    pub fn guardrails(&self) -> &GuardrailsConfig {
        &self.guardrails
    }

    pub fn guardrails_mut(&mut self) -> &mut GuardrailsConfig {
        &mut self.guardrails
    }

    /// This layer's `[context_firewall]` table — see
    /// [`firewall::ContextFirewallConfig`].
    pub fn context_firewall(&self) -> &firewall::ContextFirewallConfig {
        &self.context_firewall
    }

    pub fn context_firewall_mut(&mut self) -> &mut firewall::ContextFirewallConfig {
        &mut self.context_firewall
    }

    /// This layer's `[memory]` table — see [`MemoryConfig`].
    pub fn memory(&self) -> &MemoryConfig {
        &self.memory
    }

    pub fn memory_mut(&mut self) -> &mut MemoryConfig {
        &mut self.memory
    }

    /// This layer's recorded decision on automatic task-boundary
    /// checkpoints, or `None` for "never decided". See the field's own doc.
    pub fn automatic_checkpoint(&self) -> Option<bool> {
        self.automatic_checkpoint
    }

    pub fn set_automatic_checkpoint(&mut self, enabled: Option<bool>) -> &mut Self {
        self.automatic_checkpoint = enabled;
        self
    }

    /// Load the user-level configuration file named by `paths`.
    ///
    /// A missing file is not an error: it returns [`UserConfig::default`]
    /// (onboarding not completed, no integration decisions recorded). This
    /// is what makes "no initialization command required" true for a normal
    /// first run. A malformed file *is* an error — see [`ConfigError::Parse`].
    pub fn load(paths: &RuntimePaths) -> Result<Self, ConfigError> {
        load_toml_or_default(&paths.user_config_file())
    }

    /// Atomically write this configuration to the user-level configuration
    /// file named by `paths`, creating the configuration directory
    /// (owner-only on Unix) if it does not exist yet.
    ///
    /// Refuses to write, without touching the file, if `version` is newer
    /// than this build understands — see [`ConfigError::UnsupportedVersion`].
    /// That situation only arises by loading a newer file and saving it back
    /// unmodified or by constructing a [`UserConfig`] with an inflated
    /// version by hand; a config this build created itself always carries
    /// `CURRENT_SCHEMA_VERSION` and never hits it.
    pub fn save(&self, paths: &RuntimePaths) -> Result<(), ConfigError> {
        let path = paths.user_config_file();
        if self.version > CURRENT_SCHEMA_VERSION {
            return Err(ConfigError::UnsupportedVersion {
                path,
                found: self.version,
                supported: CURRENT_SCHEMA_VERSION,
            });
        }
        write_atomic_toml(paths.config_dir(), &path, self)
    }
}
/// Optional project-level Glasshouse configuration:
/// `<project root>/.glasshouse/config.toml`.
///
/// Same overridable shape as [`UserConfig`]'s integrations — see
/// [`EffectiveConfig`] for how the two are layered together.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectConfig {
    #[serde(default = "current_schema_version")]
    version: u32,
    #[serde(default)]
    integrations: IntegrationTable,
    #[serde(default)]
    profiles: ProfileTable,
    #[serde(default)]
    providers: ProviderTable,
    /// A project's `[entitlements.<name>]` entries replace the user's
    /// **by name**, whole — the rule [`ProviderTable`] follows — see
    /// [`EffectiveConfig::entitlements`].
    #[serde(default, skip_serializing_if = "EntitlementTable::is_empty")]
    entitlements: EntitlementTable,
    /// A project may override the routing-model choice, unlike
    /// [`IntegrationConfig::bypass_acknowledged`] — see
    /// [`EffectiveConfig::routing_model`] for why this is a preference and
    /// that one is not.
    #[serde(default, skip_serializing_if = "RoutingConfig::is_unset")]
    routing: RoutingConfig,
    /// A project may correct pairing metadata for the models its own work
    /// uses, and its corrections win per key over the user's — see
    /// [`EffectiveConfig::pairing_overrides`].
    #[serde(default, skip_serializing_if = "pairing::PairingConfig::is_unset")]
    pairing: pairing::PairingConfig,
    /// A project may set its own response profile, and it reaches no other
    /// project — line 597. See `crate::config::response`'s own header.
    #[serde(default, skip_serializing_if = "response::ResponseConfig::is_unset")]
    response: response::ResponseConfig,
    /// A project may override the user's decision on the automatic
    /// memory-extraction trigger — see [`UserConfig::memory_extraction`] for
    /// the field this mirrors and [`EffectiveConfig::memory_extraction_enabled`]
    /// for how the two layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    memory_extraction: Option<bool>,
    /// A project may override the user's decision on automatic
    /// task-boundary checkpoints — see [`UserConfig::automatic_checkpoint`]
    /// for the field this mirrors and
    /// [`EffectiveConfig::automatic_checkpoint_enabled`] for how the two
    /// layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    automatic_checkpoint: Option<bool>,
    /// A project may override the user's decision on delivering the
    /// implementation policy — see [`UserConfig::implementation_policy`] for
    /// the field this mirrors and
    /// [`EffectiveConfig::implementation_policy_enabled`] for how the two
    /// layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    implementation_policy: Option<bool>,
    /// A project may name its own extraction model — see
    /// [`UserConfig::memory_extraction_model`] for the field this mirrors and
    /// [`EffectiveConfig::memory_extraction_model`] for how the two layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    memory_extraction_model: Option<ExtractionModelRef>,
    /// A project may set its own guardrail mode and blocking list — see
    /// [`UserConfig::guardrails`] for the table this mirrors and
    /// [`EffectiveConfig::guardrail_mode`] for how the two layer.
    #[serde(default, skip_serializing_if = "GuardrailsConfig::is_unset")]
    guardrails: GuardrailsConfig,
    /// A project may set its own context-firewall mode and thresholds — see
    /// [`UserConfig::context_firewall`] for the table this mirrors and
    /// [`EffectiveConfig::context_firewall_mode`] for how the two layer.
    #[serde(
        default,
        skip_serializing_if = "firewall::ContextFirewallConfig::is_unset"
    )]
    context_firewall: firewall::ContextFirewallConfig,
    /// A project may override the user's decision on briefing a launch with
    /// this project's memory — see [`UserConfig::memory`] for the table this
    /// mirrors and [`EffectiveConfig::inject_memory_at_launch`] for how the
    /// two layer.
    #[serde(default, skip_serializing_if = "MemoryConfig::is_unset")]
    memory: MemoryConfig,
}
impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            version: CURRENT_SCHEMA_VERSION,
            integrations: IntegrationTable::default(),
            profiles: ProfileTable::default(),
            providers: ProviderTable::default(),
            entitlements: EntitlementTable::default(),
            routing: RoutingConfig::default(),
            pairing: pairing::PairingConfig::default(),
            response: response::ResponseConfig::default(),
            memory_extraction: None,
            automatic_checkpoint: None,
            implementation_policy: None,
            memory_extraction_model: None,
            guardrails: GuardrailsConfig::default(),
            context_firewall: firewall::ContextFirewallConfig::default(),
            memory: MemoryConfig::default(),
        }
    }
}
impl ProjectConfig {
    pub fn version(&self) -> u32 {
        self.version
    }

    pub fn integrations(&self) -> &IntegrationTable {
        &self.integrations
    }

    pub fn integrations_mut(&mut self) -> &mut IntegrationTable {
        &mut self.integrations
    }

    pub fn profiles(&self) -> &ProfileTable {
        &self.profiles
    }

    pub fn profiles_mut(&mut self) -> &mut ProfileTable {
        &mut self.profiles
    }

    pub fn providers(&self) -> &ProviderTable {
        &self.providers
    }

    pub fn providers_mut(&mut self) -> &mut ProviderTable {
        &mut self.providers
    }

    pub fn entitlements(&self) -> &EntitlementTable {
        &self.entitlements
    }

    pub fn entitlements_mut(&mut self) -> &mut EntitlementTable {
        &mut self.entitlements
    }

    pub fn routing(&self) -> &RoutingConfig {
        &self.routing
    }

    pub fn routing_mut(&mut self) -> &mut RoutingConfig {
        &mut self.routing
    }

    pub fn pairing(&self) -> &pairing::PairingConfig {
        &self.pairing
    }

    pub fn pairing_mut(&mut self) -> &mut pairing::PairingConfig {
        &mut self.pairing
    }

    pub fn response(&self) -> &response::ResponseConfig {
        &self.response
    }

    pub fn response_mut(&mut self) -> &mut response::ResponseConfig {
        &mut self.response
    }

    /// This layer's recorded decision on the automatic memory-extraction
    /// trigger, or `None` for "never decided". See [`UserConfig::memory_extraction`].
    pub fn memory_extraction(&self) -> Option<bool> {
        self.memory_extraction
    }

    /// The project's decision on delivering the implementation policy, or
    /// `None` for "never decided". See [`UserConfig::implementation_policy`].
    pub fn implementation_policy(&self) -> Option<bool> {
        self.implementation_policy
    }

    pub fn set_implementation_policy(&mut self, enabled: Option<bool>) -> &mut Self {
        self.implementation_policy = enabled;
        self
    }

    pub fn set_memory_extraction(&mut self, enabled: Option<bool>) -> &mut Self {
        self.memory_extraction = enabled;
        self
    }
    /// The model this project has chosen to perform memory extraction, or
    /// `None` — see [`ProjectConfig::memory_extraction_model`].
    pub fn memory_extraction_model(&self) -> Option<&ExtractionModelRef> {
        self.memory_extraction_model.as_ref()
    }

    pub fn set_memory_extraction_model(&mut self, model: Option<ExtractionModelRef>) -> &mut Self {
        self.memory_extraction_model = model;
        self
    }

    /// This layer's `[guardrails]` table — see [`GuardrailsConfig`].
    pub fn guardrails(&self) -> &GuardrailsConfig {
        &self.guardrails
    }

    pub fn guardrails_mut(&mut self) -> &mut GuardrailsConfig {
        &mut self.guardrails
    }

    /// This layer's `[context_firewall]` table — see
    /// [`UserConfig::context_firewall`] for the table this mirrors and
    /// [`EffectiveConfig::context_firewall_mode`] for how the two layer.
    pub fn context_firewall(&self) -> &firewall::ContextFirewallConfig {
        &self.context_firewall
    }

    pub fn context_firewall_mut(&mut self) -> &mut firewall::ContextFirewallConfig {
        &mut self.context_firewall
    }

    /// This layer's `[memory]` table — see [`UserConfig::memory`] for the
    /// table this mirrors.
    pub fn memory(&self) -> &MemoryConfig {
        &self.memory
    }

    pub fn memory_mut(&mut self) -> &mut MemoryConfig {
        &mut self.memory
    }

    /// This layer's recorded decision on automatic task-boundary
    /// checkpoints, or `None` for "never decided". See
    /// [`UserConfig::automatic_checkpoint`].
    pub fn automatic_checkpoint(&self) -> Option<bool> {
        self.automatic_checkpoint
    }

    pub fn set_automatic_checkpoint(&mut self, enabled: Option<bool>) -> &mut Self {
        self.automatic_checkpoint = enabled;
        self
    }
}
/// Resolve the project-level configuration file path inside `project`'s
/// scope.
///
/// Going through [`crate::project::ProjectScope::resolve`] rather than a
/// plain `project.root().join(...)` is the point: even though the relative
/// path here is a fixed constant we control, resolving it through the scope
/// guard means the write path can never end up outside the project root
/// through a symlink planted at `.glasshouse` (or anywhere along it), and it
/// keeps this module honest with every other component that touches a
/// project-relative path.
fn project_config_path(project: &Project) -> Result<PathBuf, ConfigError> {
    project
        .scope()
        .resolve(PROJECT_CONFIG_RELATIVE_PATH)
        .map_err(ConfigError::Scope)
}
/// Load the optional project-level configuration for `project`, if the user
/// has ever created one.
///
/// Returns `Ok(None)` when no such file exists. This function never creates
/// one — see [`write_project_config_with_consent`] for the only way this
/// file comes into existence.
pub fn load_project_config(project: &Project) -> Result<Option<ProjectConfig>, ConfigError> {
    let path = project_config_path(project)?;
    match std::fs::read_to_string(&path) {
        Ok(contents) => parse_toml(&path, &contents).map(Some),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ConfigError::Read { path, source }),
    }
}
/// Write `config` into `project`'s `.glasshouse/config.toml`, creating the
/// `.glasshouse` directory (owner-only on Unix) if needed.
///
/// # This requires the user's explicit consent
///
/// This writes inside the user's project tree — Phase 2D requires "explicit
/// confirmation before writing project-level configuration into the
/// repository," and this function performs none of that confirmation
/// itself; it is the caller's (the settings UI's) job to have obtained it
/// first. The `_with_consent` suffix exists so this is never reached for
/// unconditionally, by-default, or "just in case" writes — a project that
/// has not opted in must never grow a `.glasshouse/config.toml` on its own.
pub fn write_project_config_with_consent(
    project: &Project,
    config: &ProjectConfig,
) -> Result<(), ConfigError> {
    let path = project_config_path(project)?;
    if config.version > CURRENT_SCHEMA_VERSION {
        return Err(ConfigError::UnsupportedVersion {
            path,
            found: config.version,
            supported: CURRENT_SCHEMA_VERSION,
        });
    }
    // `path` is `<root>/.glasshouse/config.toml`, so it always has a parent;
    // the fallback only guards a `PROJECT_CONFIG_RELATIVE_PATH` that no
    // longer names a nested file, which would be a bug in this module, not
    // something a caller can trigger.
    let dir = path.parent().unwrap_or(&path).to_path_buf();
    write_atomic_toml(&dir, &path, config)
}
/// Which configuration layer supplied a resolved value. Surfaced so the
/// Phase 2D settings view can visibly distinguish a user-level default from
/// a project-level override, as required.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    /// Read from the project-level file.
    Project,
    /// Read from the user-level file.
    User,
    /// Neither layer had a recorded value; this is a hardcoded fallback.
    Default,
}
impl Layer {
    /// Where a value this layer supplied came from, as a phrase that reads
    /// inside a sentence — "disabled *in your configuration*". Deliberately
    /// says nothing about *where on disk*: a refusal a person reads on their
    /// terminal must not print an absolute path (`crate::secret`'s own rule
    /// about what a message may carry), and the file a layer means is
    /// already `glasshouse doctor`'s job to name.
    pub fn describe_source(self) -> &'static str {
        match self {
            Self::Project => "in this project's configuration",
            Self::User => "in your configuration",
            Self::Default => "by default",
        }
    }
}
/// A resolved value together with which layer produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layered<T> {
    pub value: T,
    pub layer: Layer,
}
impl<T> Layered<T> {
    pub fn new(value: T, layer: Layer) -> Self {
        Self { value, layer }
    }
}
/// Load a TOML-serialized `T` from `path`, or `T::default()` if the file
/// does not exist.
fn load_toml_or_default<T: Default + serde::de::DeserializeOwned>(
    path: &Path,
) -> Result<T, ConfigError> {
    match std::fs::read_to_string(path) {
        Ok(contents) => parse_toml(path, &contents),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(T::default()),
        Err(source) => Err(ConfigError::Read {
            path: path.to_path_buf(),
            source,
        }),
    }
}
fn parse_toml<T: serde::de::DeserializeOwned>(
    path: &Path,
    contents: &str,
) -> Result<T, ConfigError> {
    toml::from_str(contents).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source: Box::new(source),
    })
}
/// Monotonic counter mixed into temporary file names so that two saves
/// racing inside the same process (as can happen across test threads, which
/// share a process id) never pick the same temporary path.
static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);
/// Serialize `value` as pretty TOML and atomically replace `path` with it.
///
/// Writes to a fresh temporary file inside `dir` (which must be the same
/// directory as `path`, so the following rename stays on one filesystem)
/// with owner-only permissions, then `rename`s it over `path`. `rename` is
/// atomic on both POSIX and Windows when source and destination share a
/// filesystem, so a crash or power loss during the write can only ever
/// leave the previous file intact or the new file complete — never a
/// half-written config on disk. `dir` is created first (owner-only on Unix,
/// mirroring `create_state_dir` in `lib.rs`) if it does not exist yet.
fn write_atomic_toml<T: Serialize>(dir: &Path, path: &Path, value: &T) -> Result<(), ConfigError> {
    create_secure_dir(dir).map_err(|source| ConfigError::CreateDir {
        path: dir.to_path_buf(),
        source,
    })?;

    let contents = toml::to_string_pretty(value).map_err(|source| ConfigError::Serialize {
        path: path.to_path_buf(),
        source: Box::new(source),
    })?;

    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config.toml".to_owned());
    let unique = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp_path = dir.join(format!(".{file_name}.{}.{unique}.tmp", std::process::id()));

    write_secure_file(&tmp_path, contents.as_bytes()).map_err(|source| ConfigError::Write {
        path: tmp_path.clone(),
        source,
    })?;

    if let Err(source) = std::fs::rename(&tmp_path, path) {
        // Best-effort cleanup: leaving the temp file behind on a failed
        // rename is better than leaving nothing, but never mask the real
        // error with a cleanup failure.
        let _ = std::fs::remove_file(&tmp_path);
        return Err(ConfigError::Write {
            path: path.to_path_buf(),
            source,
        });
    }

    Ok(())
}
/// Create `dir` (and parents) restricted to its owner on Unix. Mirrors
/// `create_state_dir` in `lib.rs`: a config file can carry integration
/// executable paths and, later, other user-specific detail, so default
/// (typically world-readable) directory permissions are not appropriate.
/// When `dir` already exists as something else, this keeps whatever
/// permissions it already had — it neither widens nor narrows a directory
/// it did not create.
#[cfg(unix)]
fn create_secure_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}
#[cfg(not(unix))]
fn create_secure_dir(dir: &Path) -> io::Result<()> {
    std::fs::DirBuilder::new().recursive(true).create(dir)
}
/// Write `contents` to a new file at `path`, restricted to its owner on
/// Unix. `path` is always a fresh temporary file name (see
/// [`write_atomic_toml`]), so `create_new` semantics are not required here;
/// `create + truncate` is enough.
#[cfg(unix)]
fn write_secure_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(contents)?;
    file.sync_all()
}
#[cfg(not(unix))]
fn write_secure_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
    file.write_all(contents)?;
    file.sync_all()
}
