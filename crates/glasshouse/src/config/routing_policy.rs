//! Routing policy configuration: score weights, capacity-band thresholds, reserve policies, and the routing-model choice.
//!

use serde::{Deserialize, Serialize};

use super::*;

/// The raw, on-disk shape [`CapacityBandThresholdsConfig`] validates itself
/// out of. A private intermediate rather than a public one: nothing outside
/// `serde`'s `try_from` machinery should ever hold an unvalidated set of
/// thresholds.
#[derive(Debug, Clone, Copy, Deserialize)]
struct RawCapacityBandThresholds {
    exhausted_percent: u8,
    reserve_percent: u8,
    tight_percent: u8,
    healthy_percent: u8,
}
/// User-configurable capacity-band thresholds — capability map line 1270.
///
/// Four ascending percentages, validated as one unit at deserialization time
/// via `#[serde(try_from = "RawCapacityBandThresholds")]` — the same
/// fail-closed idiom [`QuotaStaleAfterSeconds`], [`RouterCostMicroUsd`] and
/// [`PremiumReservePercent`] already use for a single field, applied here
/// across four so a non-monotonic set is refused at `UserConfig::load` /
/// `load_project_config` time rather than sorted silently or discovered only
/// when a band is computed. See
/// [`crate::provider::quota::CapacityBandThresholds`], the domain type this
/// converts to via [`CapacityBandThresholdsConfig::to_domain`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawCapacityBandThresholds")]
pub struct CapacityBandThresholdsConfig {
    exhausted_percent: u8,
    reserve_percent: u8,
    tight_percent: u8,
    healthy_percent: u8,
}
impl TryFrom<RawCapacityBandThresholds> for CapacityBandThresholdsConfig {
    type Error = QuotaValueError;

    fn try_from(raw: RawCapacityBandThresholds) -> Result<Self, Self::Error> {
        crate::provider::quota::CapacityBandThresholds::new(
            raw.exhausted_percent,
            raw.reserve_percent,
            raw.tight_percent,
            raw.healthy_percent,
        )?;
        Ok(Self {
            exhausted_percent: raw.exhausted_percent,
            reserve_percent: raw.reserve_percent,
            tight_percent: raw.tight_percent,
            healthy_percent: raw.healthy_percent,
        })
    }
}
impl From<crate::provider::quota::CapacityBandThresholds> for CapacityBandThresholdsConfig {
    /// A domain value is already known-monotonic by its own constructor, so
    /// this is a plain field copy rather than a second validation pass.
    fn from(domain: crate::provider::quota::CapacityBandThresholds) -> Self {
        Self {
            exhausted_percent: domain.exhausted_percent(),
            reserve_percent: domain.reserve_percent(),
            tight_percent: domain.tight_percent(),
            healthy_percent: domain.healthy_percent(),
        }
    }
}
impl CapacityBandThresholdsConfig {
    /// The validated domain value — see
    /// [`crate::provider::quota::CapacityBandThresholds::band_for_percent`].
    pub fn to_domain(self) -> crate::provider::quota::CapacityBandThresholds {
        crate::provider::quota::CapacityBandThresholds::new(
            self.exhausted_percent,
            self.reserve_percent,
            self.tight_percent,
            self.healthy_percent,
        )
        .expect("validated once already at deserialization")
    }
}
/// A `[routing.score_weights]` value with a non-finite field — capability map
/// lines 1357/1358's own "fail closed" requirement, stated in code rather
/// than clamped around, the same shape
/// [`crate::provider::quota::CapacityBandThresholdsError`] states for its
/// four fields.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
#[error(
    "routing score weights must be finite numbers (quota_pressure_weight {quota_pressure_weight}, \
     health_failure_penalty {health_failure_penalty}, health_penalty_floor {health_penalty_floor}, \
     health_unavailable_penalty {health_unavailable_penalty}); refusing rather than substituting a \
     value nobody wrote"
)]
pub struct ScoreWeightsError {
    pub quota_pressure_weight: f64,
    pub health_failure_penalty: f64,
    pub health_penalty_floor: f64,
    pub health_unavailable_penalty: f64,
}
/// The raw, on-disk shape [`ScoreWeightsConfig`] validates itself out of. A
/// private intermediate rather than a public one, for
/// [`RawCapacityBandThresholds`]'s own reason: nothing outside `serde`'s
/// `try_from` machinery should ever hold an unvalidated set of weights.
#[derive(Debug, Clone, Copy, Deserialize)]
struct RawScoreWeights {
    quota_pressure_weight: f64,
    health_failure_penalty: f64,
    health_penalty_floor: f64,
    health_unavailable_penalty: f64,
}
/// User-configurable routing score weights — capability map lines 1357/1358:
/// the four weights [`crate::routing::session::ScoreWeights`]'s doc comment
/// names are an observed starting policy, not a universal constant, and this
/// is where a user overrides them.
///
/// Validated as one unit at deserialization time via
/// `#[serde(try_from = "RawScoreWeights")]` — the same fail-closed idiom
/// [`CapacityBandThresholdsConfig`] uses, refusing a non-finite field (`NaN`
/// or infinite) outright rather than substituting a default silently, so a
/// malformed config file is refused at `UserConfig::load` /
/// `load_project_config` time rather than producing a routing decision
/// nobody could predict. Unlike the capacity-band thresholds, there is no
/// ordering between these four fields to enforce: each prices an independent
/// term (line 1598's quota weight and the three line-1599 health terms), so
/// finiteness is the whole of what "fail closed" means here.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawScoreWeights")]
pub struct ScoreWeightsConfig {
    quota_pressure_weight: f64,
    health_failure_penalty: f64,
    health_penalty_floor: f64,
    health_unavailable_penalty: f64,
}
/// Sound because `TryFrom<RawScoreWeights>` is the only way to build one and
/// refuses every non-finite field: `f64::eq`'s single deviation from an
/// equivalence relation is `NaN != NaN`, and no `ScoreWeightsConfig` can ever
/// hold one. `RoutingConfig` derives `Eq` across every field it carries,
/// [`CapacityBandThresholdsConfig`] among them, and this is what lets a
/// `[routing.score_weights]` value sit beside it rather than forcing that
/// derive apart.
impl Eq for ScoreWeightsConfig {}
impl TryFrom<RawScoreWeights> for ScoreWeightsConfig {
    type Error = ScoreWeightsError;

    fn try_from(raw: RawScoreWeights) -> Result<Self, Self::Error> {
        if raw.quota_pressure_weight.is_finite()
            && raw.health_failure_penalty.is_finite()
            && raw.health_penalty_floor.is_finite()
            && raw.health_unavailable_penalty.is_finite()
        {
            Ok(Self {
                quota_pressure_weight: raw.quota_pressure_weight,
                health_failure_penalty: raw.health_failure_penalty,
                health_penalty_floor: raw.health_penalty_floor,
                health_unavailable_penalty: raw.health_unavailable_penalty,
            })
        } else {
            Err(ScoreWeightsError {
                quota_pressure_weight: raw.quota_pressure_weight,
                health_failure_penalty: raw.health_failure_penalty,
                health_penalty_floor: raw.health_penalty_floor,
                health_unavailable_penalty: raw.health_unavailable_penalty,
            })
        }
    }
}
impl From<crate::routing::session::ScoreWeights> for ScoreWeightsConfig {
    /// A domain value is already known-finite by its own constructor (or is
    /// [`crate::routing::session::ScoreWeights::default`], which is a
    /// compile-time constant), so this is a plain field copy rather than a
    /// second validation pass — the same reasoning
    /// [`CapacityBandThresholdsConfig`]'s own `From` impl states.
    fn from(domain: crate::routing::session::ScoreWeights) -> Self {
        Self {
            quota_pressure_weight: domain.quota_pressure_weight,
            health_failure_penalty: domain.health_failure_penalty,
            health_penalty_floor: domain.health_penalty_floor,
            health_unavailable_penalty: domain.health_unavailable_penalty,
        }
    }
}
impl ScoreWeightsConfig {
    /// The validated domain value — see
    /// [`crate::routing::session::ScoreWeights`].
    pub fn to_domain(self) -> crate::routing::session::ScoreWeights {
        crate::routing::session::ScoreWeights {
            quota_pressure_weight: self.quota_pressure_weight,
            health_failure_penalty: self.health_failure_penalty,
            health_penalty_floor: self.health_penalty_floor,
            health_unavailable_penalty: self.health_unavailable_penalty,
        }
    }
}
/// `[routing.reserve]` — capability map line 1577: the reserve policy for
/// interactive work and the one for background support jobs, recorded
/// separately.
///
/// Each field is `None` for "this layer never recorded one", matching
/// [`RoutingConfig::model`]'s reasoning, and the two resolve **per field**
/// through [`EffectiveConfig::reserve_policy`] — a project that sets only
/// `background` leaves `interactive` to the user layer and then the default,
/// exactly as [`CapacityBandThresholdsConfig`]'s siblings do. The values are
/// [`crate::routing::pressure::ReservePolicy`]'s own, deserialized directly,
/// so there is no second spelling of `protect`/`spend` to keep in step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ReservePoliciesConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    interactive: Option<crate::routing::pressure::ReservePolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    background: Option<crate::routing::pressure::ReservePolicy>,
}
impl ReservePoliciesConfig {
    pub fn interactive(&self) -> Option<crate::routing::pressure::ReservePolicy> {
        self.interactive
    }

    pub fn set_interactive(
        &mut self,
        value: Option<crate::routing::pressure::ReservePolicy>,
    ) -> &mut Self {
        self.interactive = value;
        self
    }

    pub fn background(&self) -> Option<crate::routing::pressure::ReservePolicy> {
        self.background
    }

    pub fn set_background(
        &mut self,
        value: Option<crate::routing::pressure::ReservePolicy>,
    ) -> &mut Self {
        self.background = value;
        self
    }

    /// This layer's recorded policy for `scope`, or `None` for "never
    /// decided" — the one place the scope selects a field.
    pub fn for_scope(
        &self,
        scope: crate::routing::pressure::ReserveScope,
    ) -> Option<crate::routing::pressure::ReservePolicy> {
        match scope {
            crate::routing::pressure::ReserveScope::Interactive => self.interactive,
            crate::routing::pressure::ReserveScope::Background => self.background,
        }
    }
}
/// Which routing model classifies a request, as recorded in configuration.
///
/// The routing model is the cheap, fast, replaceable component the capability
/// map describes: before spending premium agent capacity, Glasshouse may ask
/// it to classify a request and estimate the capability tier the work needs.
/// This type only records *which* of three answers the user gave — actually
/// asking a model is Phase 34B, choosing one for
/// [`RoutingModelChoice::Automatic`] is Phase 34C, and neither is built here.
///
/// `Automatic` stores an intent, not a model: the choice depends on provider
/// health, rate-limit headroom, latency and price at the moment of use, so
/// resolving it once during a wizard would freeze a decision the map wants
/// re-evaluated. It carries no payload — the user saying "you pick", not a
/// cached answer.
/// A reference, never a credential: [`RoutingModelChoice::Pinned`] holds a
/// provider *name* and a model *name*, as safe to write into a tracked
/// project file as [`ProviderConfig::credential_env`]'s variable names —
/// resolving the named provider to a credential stays `SecretStore`'s job
/// (guarded by `tests::serialized_form_has_no_secret_capable_field`).
// History: design-decisions.md, "Trims: api, events, harness and config module docs, second packet", crates/glasshouse/src/config/routing_policy.rs `RoutingModelChoice`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RoutingModelChoice {
    /// No model classifies anything: deterministic routing heuristics do.
    ///
    /// This is the default, and it is a first-class outcome rather than an
    /// absence — Phase 2C line 4 ("use deterministic routing heuristics
    /// until configured") and Phase 2D's "deterministic-only classification"
    /// are the same state, reached from opposite directions: the wizard's
    /// "Do later" simply records nothing at all (see
    /// [`RoutingConfig::model`]), and a settings screen that wants to say
    /// "deterministic, on purpose, do not ask again" writes this variant
    /// explicitly. Both resolve identically — see
    /// [`RoutingModelChoice::resolve`].
    #[default]
    Deterministic,
    /// Let Glasshouse choose among the configured resources at the moment it
    /// needs one. Stored as an intent; Phase 34C does the choosing.
    Automatic,
    /// Classify with exactly this model, from exactly this configured
    /// provider. Two names, no credential.
    Pinned { provider: String, model: String },
}
impl RoutingModelChoice {
    /// What this choice actually resolves to, given the provider names that
    /// are configured right now.
    ///
    /// A vanished provider is not a startup failure: unlike
    /// [`EffectiveConfig::configured_provider`], which answers an unknown
    /// name with [`ProviderLookupError::Unknown`] because a user who typed
    /// `--provider nope` asked for something specific, a routing model is an
    /// optimisation nobody asked for this run, and providers legitimately
    /// come and go. A [`RoutingModelChoice::Pinned`] naming a provider no
    /// longer configured degrades to [`RoutingModelResolution::Heuristics`]
    /// with a [`RoutingFallback`] naming which provider went missing, rather
    /// than failing to start.
    ///
    /// `configured` is provider *names*
    /// ([`EffectiveConfig::provider_names`] in production);
    /// [`ProviderConfig::enabled`] is deliberately not consulted here, since
    /// that is a later phase's job.
    // History: design-decisions.md, "Trims: api, events, harness and config module docs, second packet", crates/glasshouse/src/config/routing_policy.rs `RoutingModelChoice::resolve`.
    pub fn resolve(&self, configured: &[String]) -> RoutingModelResolution {
        match self {
            Self::Deterministic => {
                RoutingModelResolution::Heuristics(RoutingFallback::DeterministicChosen)
            }
            Self::Automatic => RoutingModelResolution::Automatic,
            Self::Pinned { provider, model } => {
                if configured.iter().any(|name| name == provider) {
                    RoutingModelResolution::Pinned {
                        provider: provider.clone(),
                        model: model.clone(),
                    }
                } else {
                    RoutingModelResolution::Heuristics(RoutingFallback::ProviderNotConfigured {
                        provider: provider.clone(),
                        model: model.clone(),
                    })
                }
            }
        }
    }
}
/// What will actually classify a request, after a recorded
/// [`RoutingModelChoice`] has been checked against the providers that exist.
///
/// Three outcomes, and only one of them names a model. [`Self::Automatic`]
/// is passed through unresolved on purpose — picking the cheapest
/// sufficiently fast resource is Phase 34C's whole job, and this type
/// refuses to guess at it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutingModelResolution {
    /// Deterministic routing heuristics classify the request. The
    /// [`RoutingFallback`] says why, and is worth showing to the user when
    /// it is not simply "you never configured one".
    Heuristics(RoutingFallback),
    /// Glasshouse picks a resource when it needs one (Phase 34C).
    Automatic,
    /// This exact provider and model, both still configured.
    Pinned { provider: String, model: String },
}
impl RoutingModelResolution {
    /// The reason deterministic heuristics are answering, or `None` when
    /// they are not.
    pub fn fallback(&self) -> Option<&RoutingFallback> {
        match self {
            Self::Heuristics(reason) => Some(reason),
            Self::Automatic | Self::Pinned { .. } => None,
        }
    }
}
/// Why deterministic routing heuristics are classifying requests instead of
/// a model.
///
/// Not an error type, deliberately: two of these three are ordinary,
/// expected, fully working states, and giving them `std::error::Error` would
/// invite a caller to treat "the user has not configured a routing model"
/// as a failure. It carries a [`std::fmt::Display`] because the degrade must
/// be *sayable* — Phase 2C's behavioural contract requires a configuration
/// naming a model that has disappeared to degrade "and say so".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutingFallback {
    /// Nothing has ever been recorded — the wizard's "Do later", or a
    /// configuration written before this field existed.
    NotConfigured,
    /// [`RoutingModelChoice::Deterministic`] was recorded explicitly.
    DeterministicChosen,
    /// A [`RoutingModelChoice::Pinned`] model names a provider that is not
    /// in configuration any more.
    ProviderNotConfigured { provider: String, model: String },
}
/// A routing-policy scalar outside the range Glasshouse can use honestly.
///
/// These bounds are intentionally generous. They reject values that are
/// almost certainly unit mistakes while leaving policy, including a
/// zero-cost/free-only ceiling and a disabled zero-percent reserve, under the
/// user's control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RoutingValueError {
    #[error("router latency must be between {min_ms}ms and {max_ms}ms, not {value_ms}ms")]
    Latency {
        value_ms: u32,
        min_ms: u32,
        max_ms: u32,
    },
    #[error(
        "router marginal cost must be at most {max_micro_usd} micro-USD per decision, not {value_micro_usd} micro-USD"
    )]
    Cost {
        value_micro_usd: u32,
        max_micro_usd: u32,
    },
    #[error("premium reserve must be between 0% and 100%, not {value}%")]
    Reserve { value: u16 },
}
/// Maximum acceptable routing-model latency, in milliseconds.
///
/// Ten milliseconds is below any realistic end-to-end model decision but
/// still permits a very fast local classifier. Sixty seconds is already far
/// beyond interactive routing; a larger value is almost certainly seconds
/// entered as milliseconds (or a policy that should disable model routing).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct RouterLatencyMs(u32);
impl RouterLatencyMs {
    pub const MIN: u32 = 10;
    pub const MAX: u32 = 60_000;
    pub const DEFAULT: Self = Self(2_000);

    pub fn get(self) -> u32 {
        self.0
    }
}
impl TryFrom<u32> for RouterLatencyMs {
    type Error = RoutingValueError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        if (Self::MIN..=Self::MAX).contains(&value) {
            Ok(Self(value))
        } else {
            Err(RoutingValueError::Latency {
                value_ms: value,
                min_ms: Self::MIN,
                max_ms: Self::MAX,
            })
        }
    }
}
impl From<RouterLatencyMs> for u32 {
    fn from(value: RouterLatencyMs) -> Self {
        value.0
    }
}
/// Maximum marginal price of one routing decision, in millionths of a US
/// dollar.
///
/// Fixed-point microdollars keep a human-editable TOML integer exact and
/// avoid floating-point comparisons in policy. One dollar is a deliberately
/// high ceiling for a bounded classification call; larger values are treated
/// as a unit mistake rather than accepted silently. Zero is valid and means
/// that only zero-marginal-cost candidates satisfy the price policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct RouterCostMicroUsd(u32);
impl RouterCostMicroUsd {
    pub const MAX: u32 = 1_000_000;
    pub const DEFAULT: Self = Self(1_000);

    pub fn get(self) -> u32 {
        self.0
    }
}
impl TryFrom<u32> for RouterCostMicroUsd {
    type Error = RoutingValueError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        if value <= Self::MAX {
            Ok(Self(value))
        } else {
            Err(RoutingValueError::Cost {
                value_micro_usd: value,
                max_micro_usd: Self::MAX,
            })
        }
    }
}
impl From<RouterCostMicroUsd> for u32 {
    fn from(value: RouterCostMicroUsd) -> Self {
        value.0
    }
}
/// Remaining premium capacity below which routing protects the subscription.
/// Zero disables the reserve and one hundred protects all remaining premium
/// capacity from lower-priority routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub struct PremiumReservePercent(u8);
impl PremiumReservePercent {
    pub const DEFAULT: Self = Self(20);

    pub fn get(self) -> u8 {
        self.0
    }
}
impl TryFrom<u16> for PremiumReservePercent {
    type Error = RoutingValueError;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        if value <= 100 {
            Ok(Self(value as u8))
        } else {
            Err(RoutingValueError::Reserve { value })
        }
    }
}
impl From<PremiumReservePercent> for u16 {
    fn from(value: PremiumReservePercent) -> Self {
        value.0.into()
    }
}
impl std::fmt::Display for RoutingFallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotConfigured => f.write_str(
                "no routing model is configured; requests are classified by deterministic \
                 routing heuristics",
            ),
            Self::DeterministicChosen => f.write_str(
                "routing is set to deterministic-only; requests are classified by \
                 deterministic routing heuristics",
            ),
            Self::ProviderNotConfigured { provider, model } => write!(
                f,
                "routing model `{model}` names provider `{provider}`, which is not \
                 configured; requests are classified by deterministic routing heuristics \
                 until that provider is configured again"
            ),
        }
    }
}
/// The `[routing]` table: how requests get classified.
///
/// The model choice and four bounded policy preferences belong together here:
/// they describe how routing should classify, not live observations about any
/// provider. Every field is optional so project and user layers can override
/// one preference without copying the rest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingConfig {
    /// The recorded routing-model choice, or `None` for "never decided".
    ///
    /// `None` is what the wizard's "Do later" writes, and it is why that
    /// choice leaves *no routing model configured* in the literal sense:
    /// `skip_serializing_if` means a first run that tabs straight through
    /// produces a configuration file with no `[routing]` table in it at all.
    /// It resolves exactly like [`RoutingModelChoice::Deterministic`] — see
    /// [`EffectiveConfig::routing_model`] — so the two are behaviourally one
    /// state with two spellings, and only the *reason* string tells them
    /// apart.
    ///
    /// Keeping it an `Option` rather than collapsing it into
    /// [`RoutingModelChoice::Deterministic`] is what makes layering work:
    /// [`EffectiveConfig`] needs three states per layer — "this layer says
    /// automatic", "this layer says deterministic", and "this layer says
    /// nothing, ask the next one" — exactly like
    /// [`IntegrationConfig::executable`]. A project that wants
    /// deterministic-only classification *over* a user-level `automatic` can
    /// then say so, which a collapsed shape could not express.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<RoutingModelChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_router_latency_ms: Option<RouterLatencyMs>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_marginal_cost_micro_usd: Option<RouterCostMicroUsd>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prefer_free: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    premium_reserve_percent: Option<PremiumReservePercent>,
    /// The user's preferred order over free resources — Phase 9I line 536.
    /// `None` is "this layer never recorded one", matching
    /// [`RoutingConfig::model`]'s own reasoning, not "the user cleared it to
    /// empty"; a project layer that wants to record an explicit empty order
    /// writes `Some(Vec::new())`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    free_resource_order: Option<Vec<FreeResourceRef>>,
    /// Free resources the user has disabled — Phase 9I line 536. Same
    /// `None`-means-undecided reasoning as
    /// [`RoutingConfig::free_resource_order`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    free_resource_disabled: Option<Vec<FreeResourceRef>>,
    /// The user's pinned free resource, if any — Phase 9I line 536.
    ///
    /// A pin naming a provider that is no longer configured must not stop
    /// Glasshouse from starting. This module never validates the name
    /// against configured providers when loading or saving it — the same
    /// reasoning [`RoutingModelChoice::resolve`] documents for
    /// [`RoutingModelChoice::Pinned`]: providers legitimately come and go as
    /// keys are rotated, and a stale pin degrades visibly, through
    /// [`crate::routing::disposable::NoResource::PinnedResourceUnavailable`],
    /// rather than refusing to load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    free_resource_pin: Option<FreeResourceRef>,
    /// User-overridden capacity-band thresholds — capability map line 1270.
    /// `None` means the defaults in
    /// [`crate::provider::quota::CapacityBandThresholds::DEFAULT`] apply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    capacity_band_thresholds: Option<CapacityBandThresholdsConfig>,
    /// User-overridden routing score weights — capability map lines
    /// 1357/1358. `None` means the defaults in
    /// [`crate::routing::session::ScoreWeights::default`] apply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    score_weights: Option<ScoreWeightsConfig>,
    /// The sessions whose work may spend protected quota reserve whatever the
    /// reserve policy would otherwise decide — capability map line 1290.
    ///
    /// A list of session identifiers and **not a boolean**, which is the
    /// whole of the design: line 1290 asks for an override *"for a specific
    /// task or session"*, so there is no spelling of this setting that means
    /// "always". The empty list and `None` both mean no override, and
    /// [`crate::routing::disposable::ReserveOverride`] documents why the
    /// scope travels with the value rather than being checked once at a call
    /// site.
    ///
    /// Same `None`-means-undecided reasoning as
    /// [`RoutingConfig::free_resource_order`]: a layer that has recorded
    /// nothing defers to the next one, and a layer that wants to record an
    /// explicit "no sessions, ignore the layer below" writes `Some(vec![])`.
    ///
    /// Not validated against the session store when loading or saving, for
    /// [`RoutingConfig::free_resource_pin`]'s reason: a session that has since
    /// been closed must not stop Glasshouse from starting, and a stale entry
    /// here is inert — it can only ever match a session with that identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reserve_override_sessions: Option<Vec<String>>,
    /// The routing-model fallback chain — capability map lines 1423 and
    /// 1795: the resources `glasshouse classify` tries next, in this order,
    /// when the model it chose (automatic or pinned) cannot be reached or
    /// does not answer in the schema. Each is tried at most once per
    /// classification, and an entry naming the model that already failed is
    /// skipped rather than retried.
    ///
    /// Two names per entry — exactly [`FreeResourceRef`]'s on-disk shape —
    /// because a chain entry is a reference to a provider and model
    /// configured elsewhere, never a base URL or a credential. An entry may
    /// name a metered model: like a pin, it is the user's own explicit
    /// instruction. Same `None`-means-undecided reasoning as
    /// [`RoutingConfig::free_resource_order`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model_fallback: Option<Vec<FreeResourceRef>>,
    /// Capability map line 1427: confine classification to local inference.
    /// When `true`, automatic selection admits only candidates the provider
    /// registry knows to be local, the fallback chain skips remote entries,
    /// and a pinned remote model is not called — nothing about a request
    /// leaves the machine for classification, and deterministic heuristics
    /// answer when no local model can. `None` defers to the next layer;
    /// the default is `false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    classification_local_only: Option<bool>,
    /// `[routing.reserve]` — capability map line 1577. `None` means this
    /// layer recorded neither scope's policy; see [`ReservePoliciesConfig`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reserve: Option<ReservePoliciesConfig>,
    /// Whether the session router decides where a launch's work goes —
    /// capability map line 1712, *"allow the user to disable automatic
    /// routing for the current Glasshouse instance."*
    ///
    /// `None` means this layer never said; `Some(false)` is a person saying
    /// *stop deciding for me*, resolved project over user over a default of
    /// `true` by [`EffectiveConfig::automatic_routing`].
    ///
    /// Not [`RoutingModelChoice::Deterministic`], easy to confuse:
    /// [`RoutingConfig::model`] chooses **what classifies a request** and a
    /// launch is still ranked either way; this field turns the **ranking on
    /// the launch path** off altogether, so `glasshouse launch` starts the
    /// session the person's own flags describe without asking whether this
    /// project has one worth continuing.
    ///
    /// Off means off, including the diagnosis: see `main.rs::launch_session`
    /// for why a disabled launch does not compute the ranking to report what
    /// it would have chosen. `glasshouse route` still answers on demand.
    // History: design-decisions.md, "Trims: api, events, harness and config module docs, second packet", crates/glasshouse/src/config/routing_policy.rs `RoutingConfig::automatic`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    automatic: Option<bool>,
}
impl RoutingConfig {
    /// The recorded choice, or `None` when this layer has never recorded one.
    pub fn model(&self) -> Option<&RoutingModelChoice> {
        self.model.as_ref()
    }

    /// Record `choice`, or `None` to return this layer to "never decided".
    pub fn set_model(&mut self, choice: Option<RoutingModelChoice>) -> &mut Self {
        self.model = choice;
        self
    }

    pub fn max_router_latency(&self) -> Option<RouterLatencyMs> {
        self.max_router_latency_ms
    }

    pub fn set_max_router_latency(&mut self, value: Option<RouterLatencyMs>) -> &mut Self {
        self.max_router_latency_ms = value;
        self
    }

    pub fn max_marginal_cost(&self) -> Option<RouterCostMicroUsd> {
        self.max_marginal_cost_micro_usd
    }

    pub fn set_max_marginal_cost(&mut self, value: Option<RouterCostMicroUsd>) -> &mut Self {
        self.max_marginal_cost_micro_usd = value;
        self
    }

    pub fn prefer_free(&self) -> Option<bool> {
        self.prefer_free
    }

    pub fn set_prefer_free(&mut self, value: Option<bool>) -> &mut Self {
        self.prefer_free = value;
        self
    }

    /// Whether this layer recorded an answer to *"may the router decide where
    /// a launch goes"* — map line 1712.
    pub fn automatic(&self) -> Option<bool> {
        self.automatic
    }

    pub fn set_automatic(&mut self, value: Option<bool>) -> &mut Self {
        self.automatic = value;
        self
    }

    pub fn premium_reserve(&self) -> Option<PremiumReservePercent> {
        self.premium_reserve_percent
    }

    pub fn set_premium_reserve(&mut self, value: Option<PremiumReservePercent>) -> &mut Self {
        self.premium_reserve_percent = value;
        self
    }

    /// This layer's recorded reserve overrides, or `None` for "never
    /// decided" — capability map line 1290.
    pub fn reserve_override_sessions(&self) -> Option<&[String]> {
        self.reserve_override_sessions.as_deref()
    }

    pub fn set_reserve_override_sessions(&mut self, value: Option<Vec<String>>) -> &mut Self {
        self.reserve_override_sessions = value;
        self
    }

    /// This layer's recorded fallback chain, or `None` for "never decided"
    /// — capability map lines 1423 and 1795.
    pub fn model_fallback(&self) -> Option<&[FreeResourceRef]> {
        self.model_fallback.as_deref()
    }

    pub fn set_model_fallback(&mut self, value: Option<Vec<FreeResourceRef>>) -> &mut Self {
        self.model_fallback = value;
        self
    }

    /// This layer's recorded local-only confinement, or `None` for "never
    /// decided" — capability map line 1427.
    pub fn classification_local_only(&self) -> Option<bool> {
        self.classification_local_only
    }

    pub fn set_classification_local_only(&mut self, value: Option<bool>) -> &mut Self {
        self.classification_local_only = value;
        self
    }

    /// This layer's recorded reserve policies, or `None` for "never
    /// decided" — capability map line 1577.
    pub fn reserve(&self) -> Option<ReservePoliciesConfig> {
        self.reserve
    }

    pub fn set_reserve(&mut self, value: Option<ReservePoliciesConfig>) -> &mut Self {
        self.reserve = value;
        self
    }

    /// This layer's recorded free-resource order, or `None` for "never
    /// decided".
    pub fn free_resource_order(&self) -> Option<&[FreeResourceRef]> {
        self.free_resource_order.as_deref()
    }

    pub fn set_free_resource_order(&mut self, value: Option<Vec<FreeResourceRef>>) -> &mut Self {
        self.free_resource_order = value;
        self
    }

    /// This layer's recorded disabled list, or `None` for "never decided".
    pub fn free_resource_disabled(&self) -> Option<&[FreeResourceRef]> {
        self.free_resource_disabled.as_deref()
    }

    pub fn set_free_resource_disabled(&mut self, value: Option<Vec<FreeResourceRef>>) -> &mut Self {
        self.free_resource_disabled = value;
        self
    }

    /// This layer's recorded pin, or `None` — either because no pin was
    /// recorded, or because none was ever chosen. See the field's own doc.
    pub fn free_resource_pin(&self) -> Option<&FreeResourceRef> {
        self.free_resource_pin.as_ref()
    }

    pub fn set_free_resource_pin(&mut self, value: Option<FreeResourceRef>) -> &mut Self {
        self.free_resource_pin = value;
        self
    }

    /// This layer's recorded capacity-band thresholds, or `None` for "never
    /// decided" — capability map line 1270.
    pub fn capacity_band_thresholds(&self) -> Option<CapacityBandThresholdsConfig> {
        self.capacity_band_thresholds
    }

    pub fn set_capacity_band_thresholds(
        &mut self,
        value: Option<CapacityBandThresholdsConfig>,
    ) -> &mut Self {
        self.capacity_band_thresholds = value;
        self
    }

    /// This layer's recorded routing score weights, or `None` for "never
    /// decided" — capability map lines 1357/1358.
    pub fn score_weights(&self) -> Option<ScoreWeightsConfig> {
        self.score_weights
    }

    pub fn set_score_weights(&mut self, value: Option<ScoreWeightsConfig>) -> &mut Self {
        self.score_weights = value;
        self
    }

    /// This layer's three free-resource preferences, folded into the shape
    /// [`crate::routing::disposable::DisposableRouting`] actually consumes.
    /// A layer that recorded nothing produces
    /// [`crate::routing::free::FreePreferences::new`]'s empty default for
    /// whichever of the three it never decided.
    pub fn free_preferences(&self) -> crate::routing::free::FreePreferences {
        crate::routing::free::FreePreferences::new()
            .with_order(
                self.free_resource_order
                    .as_ref()
                    .map(|order| order.iter().map(FreeResourceRef::to_key).collect())
                    .unwrap_or_default(),
            )
            .with_disabled(
                self.free_resource_disabled
                    .as_ref()
                    .map(|disabled| disabled.iter().map(FreeResourceRef::to_key).collect())
                    .unwrap_or_default(),
            )
            .with_pin(self.free_resource_pin.as_ref().map(FreeResourceRef::to_key))
    }

    /// Whether this table would serialize to nothing at all.
    pub(super) fn is_unset(&self) -> bool {
        self.model.is_none()
            && self.max_router_latency_ms.is_none()
            && self.max_marginal_cost_micro_usd.is_none()
            && self.prefer_free.is_none()
            && self.premium_reserve_percent.is_none()
            && self.free_resource_order.is_none()
            && self.free_resource_disabled.is_none()
            && self.free_resource_pin.is_none()
            && self.capacity_band_thresholds.is_none()
            && self.score_weights.is_none()
            && self.reserve_override_sessions.is_none()
            && self.model_fallback.is_none()
            && self.classification_local_only.is_none()
            && self.reserve.is_none()
            && self.automatic.is_none()
    }
}
