//! Provider configuration: credentials, quota, entitlement plans, and declared model facts and ceilings.
//!

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::*;

/// A [`crate::routing::classify::WorkloadTier`] as it is written in a
/// configuration file, and the only place this crate turns a spelling back
/// into that type.
///
/// A newtype, not `serde` on `WorkloadTier` itself: that enum has no
/// serialised form of its own — `routing::request` parses one out of a
/// routing model's untrusted JSON answer, and giving it `Deserialize` would
/// make that answer and a user's config file the same surface, which they
/// are not.
///
/// Spellings come from `as_str`, not a second list: `WORKLOAD_TIER_SPELLINGS`
/// holds every variant, and `workload_tier_ordinal`'s exhaustive `match`
/// makes adding a sixth variant a compile error rather than a silently
/// unparseable spelling, so a renamed tier renames its config spelling with
/// it and cannot drift.
// History: design-decisions.md, "Trims: api, events, harness and config module docs, second packet", crates/glasshouse/src/config/provider.rs `ConfiguredWorkloadTier`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfiguredWorkloadTier(crate::routing::classify::WorkloadTier);
/// Every [`crate::routing::classify::WorkloadTier`], in the type's own order.
/// Kept complete by [`workload_tier_ordinal`].
pub(super) const WORKLOAD_TIER_SPELLINGS: [crate::routing::classify::WorkloadTier; 5] = {
    use crate::routing::classify::WorkloadTier as T;
    [
        T::Deterministic,
        T::Leaf,
        T::Standard,
        T::Heavy,
        T::Frontier,
    ]
};
/// Where a tier sits in [`WORKLOAD_TIER_SPELLINGS`]. The `match` is
/// exhaustive on purpose: it is the compile-time guard that the array above
/// still lists every variant, and `every_workload_tier_spelling_round_trips`
/// is the run-time half that checks the two agree.
///
/// `#[cfg(test)]` because the guard is the `match` itself and nothing in the
/// shipped binary needs an ordinal. It is still a real gate: the local gate
/// and `cargo clippy --all-targets` both compile this module's tests, so a
/// sixth [`crate::routing::classify::WorkloadTier`] variant fails the build
/// there rather than becoming a spelling no configuration file can name.
#[cfg(test)]
pub(super) fn workload_tier_ordinal(tier: crate::routing::classify::WorkloadTier) -> usize {
    use crate::routing::classify::WorkloadTier as T;
    match tier {
        T::Deterministic => 0,
        T::Leaf => 1,
        T::Standard => 2,
        T::Heavy => 3,
        T::Frontier => 4,
    }
}
impl ConfiguredWorkloadTier {
    pub fn new(tier: crate::routing::classify::WorkloadTier) -> Self {
        Self(tier)
    }

    pub fn tier(self) -> crate::routing::classify::WorkloadTier {
        self.0
    }

    /// The spelling a user writes, which is the tier's own `as_str`.
    pub fn as_str(self) -> &'static str {
        self.0.as_str()
    }

    /// The tier a spelling names, or `None` for one no variant answers to.
    /// Case-sensitive and untrimmed: a configuration value is compared
    /// exactly as written, the same way `ProviderConfig::cost_of` compares a
    /// model name, so a value that does not parse is reported rather than
    /// guessed at.
    pub fn parse(text: &str) -> Option<Self> {
        WORKLOAD_TIER_SPELLINGS
            .into_iter()
            .find(|tier| tier.as_str() == text)
            .map(Self)
    }
}
impl Serialize for ConfiguredWorkloadTier {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}
impl<'de> Deserialize<'de> for ConfiguredWorkloadTier {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).ok_or_else(|| {
            let known = WORKLOAD_TIER_SPELLINGS
                .into_iter()
                .map(|tier| tier.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            serde::de::Error::custom(format!(
                "unknown workload tier `{text}` — expected one of: {known}"
            ))
        })
    }
}
/// One configured provider, as stored in a `[providers.<name>]` table.
///
/// The provider's *name* is its key in [`ProviderTable`], not a field here —
/// the same relationship [`ProfileConfig`] has to its name in [`ProfileTable`].
///
/// Deliberately holds only a template slug, an optional base-URL override,
/// and credential variable *names* — see the module-level "No secrets here"
/// section. [`ProviderConfig::to_provider`] is the only thing that turns this
/// into a [`crate::provider::Provider`], and it never reads an environment
/// variable's value while doing so.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// The built-in template this provider is based on — a
    /// [`crate::provider::Provider::name`] from [`crate::provider::templates`].
    /// Required: the two generic templates (`openai-compatible`,
    /// `anthropic-compatible`) are exactly how a fully custom provider gets
    /// configured, so there is no separate template-less shape to support.
    template: String,
    /// Override for the template's base URL. Required, in practice, for the
    /// two generic templates — their own base URL is empty because it is
    /// user-supplied — and optional for the rest, where it lets a configured
    /// provider point at a mirror or self-hosted instance of a known router
    /// (line 423).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    base_url: Option<String>,
    /// Environment variable names this provider's credential may come from.
    /// **Names only — never a value.** Non-empty here replaces the
    /// template's own default credential names entirely, which is what lets
    /// a user hold several keys for the same router.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "deserialize_credential_env_names"
    )]
    credential_env: Vec<String>,
    /// Where this provider's credential is kept, when the user has put it in
    /// the operating system's own secure store. **A reference — a service
    /// and an account name — never a value.**
    ///
    /// The serialised shape of [`crate::secret::SecretRef::OsCredential`]:
    /// the two names here are as safe to write into a tracked project file
    /// as [`ProviderConfig::credential_env`]'s variable names already are.
    ///
    /// Records intent; it is not what makes resolution work:
    /// [`crate::secret::native::PreferNativeSecretStore`] finds a stored
    /// credential by the variable name a harness expects it in, whether or
    /// not this field was ever saved, so a configuration file drifted out of
    /// step with the keychain cannot cause a wrong launch — this field is
    /// for telling the *user* where their key is, and giving deletion
    /// something to remove.
    // History: design-decisions.md, "Trims: api, events, harness and config module docs, second packet", crates/glasshouse/src/config/provider.rs `ProviderConfig::credential_store` (`SecretRef::OsCredential` doc).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    credential_store: Option<StoredCredentialRef>,
    /// Extra HTTP headers this provider needs, as name/value pairs — see
    /// [`crate::provider::Provider::headers`]. Configuration, not a
    /// credential: a header value here is written by the user into their own
    /// config file and never resolved through `SecretStore`. Non-empty here
    /// replaces the template's own headers entirely, matching
    /// [`ProviderConfig::credential_env`]'s own replace-not-merge rule.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    headers: Vec<(String, String)>,
    /// Whether this provider is currently enabled. Disabling is not
    /// removal — see [`ProviderConfig::set_enabled`] and the "disable is not
    /// delete" rule Phase 2D's Settings behavioural contract requires: every
    /// other field stays exactly as configured, and re-enabling needs no
    /// retyping. Deciding whether routing may actually use a disabled
    /// provider is a later phase's job; [`ProviderConfig::to_provider`]
    /// never consults this field.
    #[serde(
        default = "enabled_by_default",
        skip_serializing_if = "is_enabled_by_default"
    )]
    enabled: bool,
    /// Model identifiers on this provider the user has marked free-tier or
    /// zero-marginal-cost — Phase 9I lines 527 and 531. **Names only,
    /// exactly like [`ProviderConfig::credential_env`]'s variable names.**
    ///
    /// Unmarked means metered. There is no inference here — no prefix match
    /// on `:free`, no heuristic — because [`crate::routing::Cost::Metered`]
    /// is the fail-closed default: a router that guessed a model was free
    /// and was wrong spends the user's money. See
    /// [`ProviderConfig::cost_of`], the one place that answer is computed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    free_models: Vec<String>,
    /// Model identifiers on this provider the user has explicitly named as
    /// eligible for metered (paid) fallback on Glasshouse's own bounded
    /// support work — the decision recorded in
    /// `docs/product/design-decisions.md`, *"Metered capacity for background
    /// jobs"*: ordinary support work may spend quota as a last resort when no
    /// free resource can serve.
    ///
    /// **This list is the control, not a switch beside it.** Empty (the
    /// default) means no metered fallback for this provider — the coherent
    /// off state, and the only honest default: Glasshouse never invents a
    /// model name, so a provider nobody named a paid model for contributes
    /// nothing metered. Naming one here is the user's decision already made;
    /// nothing above this list asks permission again. **Names only**, exactly
    /// like [`ProviderConfig::free_models`] — a model in both lists resolves
    /// through [`ProviderConfig::cost_of`], which still answers `Free` for
    /// it, because [`ProviderConfig::free_models`] is checked first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    metered_models: Vec<String>,
    /// Whether this provider's protocols carry tool calls — capability map
    /// line 1513's tool-semantics half, and the producer
    /// [`ProviderConfig::to_provider`] applies to every
    /// [`crate::provider::ProtocolSupport`] this provider serves.
    ///
    /// `None` (the default) leaves every protocol's `tool_calls` exactly as
    /// its own template declares — every built-in template's own answer is
    /// [`crate::harness::Declared::Unverified`] — so a provider nobody has
    /// said anything about excludes nothing new. `Some(value)` is the
    /// user's own word: [`ProviderConfig::to_provider`] turns it into
    /// [`crate::harness::Declared::verified`], citing the `[providers.<name>]`
    /// table it came from — the user, having read their own provider's
    /// documentation, is the verifier here, exactly as a `--help` line is
    /// the verifier for an adapter's own declarations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool_calls: Option<bool>,
    /// Per-model resource facts the user has declared for this provider —
    /// capability map line 1517's producer, keyed by the same model
    /// identifier [`ProviderConfig::model_ceilings`] uses, and read through
    /// [`ProviderConfig::resource_facts_of`].
    ///
    /// A model absent from this map, or an axis absent from its own table,
    /// stays [`crate::harness::Declared::Unverified`] — the same "nobody has
    /// said" rule [`ProviderConfig::model_ceilings`]'s own doc states, and
    /// for the same reason: an empty declaration must never read as an
    /// established absence.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    model_facts: BTreeMap<String, ConfiguredModelFacts>,
    /// The highest workload tier the user is willing to trust an individual
    /// model on this provider with — capability map line 1796, and the one
    /// production producer of [`crate::routing::session::Destination::with_tier_ceiling`].
    ///
    /// Keyed by the same model identifier [`ProviderConfig::free_models`] and
    /// [`ProviderConfig::metered_models`] name, and valued by
    /// [`crate::routing::classify::WorkloadTier`]'s own spellings —
    /// `deterministic`, `leaf`, `standard`, `heavy`, `frontier` — parsed
    /// through [`ConfiguredWorkloadTier`], which refuses an unknown spelling
    /// at load rather than silently reading it as no ceiling at all. A
    /// misspelt ceiling that read as absent would be exactly the failure this
    /// project keeps paying for: an empty result indistinguishable from
    /// success (practice §68).
    ///
    /// **A model absent from this map has no ceiling, and that is not a low
    /// one.** `super::routing::session::hard_constraint` rejects only a
    /// destination whose ceiling is *established* below the task's required
    /// tier; "nobody has said" is never "cannot". So the empty default — every
    /// project that has not configured this — leaves every destination exactly
    /// as eligible as it was before this field existed.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    model_ceilings: BTreeMap<String, ConfiguredWorkloadTier>,
    /// Calibrated model-capability records — Phase 34F, capability map
    /// lines 1475–1479 and 1482–1485, widening [`ProviderConfig::model_ceilings`]
    /// to the rest of that neighbourhood rather than duplicating it. Keyed
    /// by the same model identifier `model_ceilings` uses; this provider
    /// entry is the `backend` axis capability map line 1482 asks calibration
    /// to stay local to. See [`capability::ModelCapabilityRecord`] for the
    /// record's other narrowing fields and [`capability::resolve_ceiling`]
    /// for how its ceiling and this field's own override reconcile.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    model_capabilities: BTreeMap<String, capability::ModelCapabilityRecord>,
    /// A prompt transformation this backend performs on the way through —
    /// Phase 9K line 609.
    ///
    /// **Backend metadata a person records, never something Glasshouse does.**
    /// Line 608 forbids gateway-side system-prompt rewriting from being the
    /// default way a response profile is applied, and nothing in Glasshouse
    /// writes this field: `crate::harness::response::apply` cannot reach a
    /// gateway at all. What this exists for is the case where a user's own
    /// gateway or router already rewrites prompts, and a session's
    /// instructions therefore arrive at the model altered by something
    /// Glasshouse did not do. `glasshouse response` surfaces it with that
    /// warning attached, which is the whole of line 609: an unsurfaced
    /// transformation is exactly how a harness's own instructions get
    /// silently rewritten and nobody can tell.
    ///
    /// Free text, because it describes somebody else's software.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prompt_transform: Option<String>,
    /// What the user told Glasshouse about this provider's quota when the
    /// provider's own telemetry does not say — capability map lines 1233,
    /// 1203 and 1237.
    ///
    /// As safe to write into a tracked project file as
    /// [`ProviderConfig::credential_env`]'s variable names: a plan name, an
    /// integer number of microdollars, and an integer number of seconds.
    /// Nothing here is resolved through [`crate::secret`] and nothing here
    /// names a credential.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    quota: Option<QuotaOverride>,
}
/// How long one provider's quota telemetry stays current — capability map
/// line 1237's "provider-specific configurable age".
///
/// Seconds, as a human-editable integer, matching
/// [`RouterCostMicroUsd`]'s own reasoning about exactness in policy. There is
/// no one right value and that is the point of the line: a credit balance
/// changes when somebody pays a bill and a requests-per-minute ceiling is a
/// contract that changes when a plan does, so the same age would be wrong for
/// both on the same provider, let alone across providers.
///
/// The default is fifteen minutes. It is a *default*, not a claim about any
/// provider — long enough that a resource view opened twice in a row does not
/// call a reading from a minute ago stale, short enough that a balance read
/// before lunch is not still presented as current after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct QuotaStaleAfterSeconds(u32);
impl QuotaStaleAfterSeconds {
    /// Thirty days. A ceiling rather than a policy: an age limit longer than
    /// this describes a reading nobody should be routing on under any
    /// definition, and accepting `u32::MAX` silently would let a typo disable
    /// staleness entirely without saying so.
    pub const MAX: u32 = 30 * 24 * 60 * 60;
    pub const DEFAULT: Self = Self(15 * 60);

    pub fn get(self) -> u32 {
        self.0
    }

    /// As the `i64` [`crate::provider::quota::Reading::freshness`] takes.
    pub fn seconds(self) -> i64 {
        i64::from(self.0)
    }
}
impl TryFrom<u32> for QuotaStaleAfterSeconds {
    type Error = QuotaValueError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        if value <= Self::MAX {
            Ok(Self(value))
        } else {
            Err(QuotaValueError::StaleAfter {
                seconds: value,
                max_seconds: Self::MAX,
            })
        }
    }
}
impl From<QuotaStaleAfterSeconds> for u32 {
    fn from(value: QuotaStaleAfterSeconds) -> Self {
        value.0
    }
}
/// The period a monetary budget covers — capability map line 1203's
/// "monthly **or rolling**".
///
/// Two answers because they are genuinely different promises: a calendar
/// month's budget is spent and forgiven on the first, and a rolling window's
/// is never fully forgiven at all. Nothing in Glasshouse counts spend against
/// either yet — see [`MonetaryBudget`] — but a ceiling recorded without its
/// period is a number that cannot be checked, and this project does not store
/// those.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BudgetPeriod {
    /// Resets at the start of each calendar month.
    CalendarMonth,
    /// A trailing thirty days that never fully resets.
    RollingThirtyDays,
}
impl BudgetPeriod {
    pub fn as_str(self) -> &'static str {
        match self {
            BudgetPeriod::CalendarMonth => "calendar month",
            BudgetPeriod::RollingThirtyDays => "rolling thirty days",
        }
    }
}
/// A spending ceiling the user set for one metered provider — capability map
/// line 1203.
///
/// # Not [`RouterCostMicroUsd`], and the difference is the whole line
///
/// [`RouterCostMicroUsd`] caps the price of **one routing decision**. This
/// caps **cumulative spend over a period**. A user who set the first to a
/// tenth of a cent has said nothing at all about how many such calls they are
/// willing to pay for in a month, which is why Phase 32A recorded that the
/// existing field does not satisfy this line.
///
/// # What it does not do, stated rather than implied
///
/// Nothing in Glasshouse counts money spent. This is the ceiling half of
/// capability map line 1209 and the ceiling half only: it reaches
/// [`crate::provider::quota::CapacityState::user_budget`] as the pool's
/// *limit*, with the remaining half left unmeasured, so a resource view can
/// honestly say "you set a ten dollar monthly ceiling and Glasshouse does not
/// know what you have spent against it" instead of implying a balance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonetaryBudget {
    #[serde(rename = "amount_micro_usd")]
    amount_micro_usd: u64,
    period: BudgetPeriod,
}
impl MonetaryBudget {
    /// Ten thousand dollars. A unit-mistake guard in the same spirit as
    /// [`RouterCostMicroUsd::MAX`]: somebody who writes `1000` meaning ten
    /// dollars has made an error this cannot catch, but somebody who writes
    /// a dollar figure where microdollars belong is off by a million and this
    /// does.
    pub const MAX_MICRO_USD: u64 = 10_000 * 1_000_000;

    pub fn new(amount_micro_usd: u64, period: BudgetPeriod) -> Result<Self, QuotaValueError> {
        if amount_micro_usd > Self::MAX_MICRO_USD {
            return Err(QuotaValueError::Budget {
                micro_usd: amount_micro_usd,
                max_micro_usd: Self::MAX_MICRO_USD,
            });
        }
        Ok(Self {
            amount_micro_usd,
            period,
        })
    }

    pub fn amount_micro_usd(self) -> u64 {
        self.amount_micro_usd
    }

    pub fn period(self) -> BudgetPeriod {
        self.period
    }
}
/// What the user told Glasshouse about one provider's quota when its own
/// telemetry does not say — capability map lines 1233, 1203 and 1237.
///
/// Every field is optional and absent means "the user said nothing", never
/// "the user said none": a provider with no `[providers.x.quota]` table at
/// all and one with an empty table are the same, and neither asserts that the
/// provider has no plan.
///
/// This is configuration, so everything here is [`Layer`]-resolved the same
/// way every other provider field is, and everything here becomes a
/// [`crate::provider::quota::ReadingSource::UserConfiguration`] reading —
/// which [`crate::provider::quota::Capacity::prefer`] then ranks *below* a
/// provider's or a harness's own word, per capability map line 1228. A user's
/// manual entry fills a gap; it does not override a measurement.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaOverride {
    /// The plan this account is on, as the provider names it — `"max"`,
    /// `"pro"`, `"team"`. Capability map line 1233's "known plan".
    ///
    /// Free text, for [`crate::provider::quota::KnownPlan`]'s own reason: every
    /// vendor names its own tiers and a closed enumeration here would be
    /// wrong within a quarter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    plan: Option<String>,
    /// A cumulative spending ceiling for this provider — capability map
    /// line 1203.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    budget: Option<MonetaryBudget>,
    /// How long this provider's telemetry stays current — capability map
    /// line 1237. `None` means [`QuotaStaleAfterSeconds::DEFAULT`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stale_after_seconds: Option<QuotaStaleAfterSeconds>,
    /// This provider's own protected reserve percentage — capability map
    /// line 1288, Phase 32F's *"allow each premium resource to define a
    /// protected reserve percentage"*. Reuses [`PremiumReservePercent`]
    /// rather than a second 0–100 type: it is the same question
    /// (`crate::provider::quota::CapacityBandThresholds::with_resource_reserve`
    /// is where this reaches the band the packet's design decision 6 asks
    /// for), asked per provider instead of once globally.
    /// [`EffectiveConfig::reserve_percent`]'s own default is
    /// [`EffectiveConfig::premium_reserve`] — the existing global routing
    /// preference — when a provider states none of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reserve_percent: Option<PremiumReservePercent>,
}
impl QuotaOverride {
    pub fn plan(&self) -> Option<&str> {
        self.plan.as_deref()
    }

    pub fn set_plan(&mut self, plan: Option<String>) -> &mut Self {
        self.plan = plan;
        self
    }

    pub fn budget(&self) -> Option<MonetaryBudget> {
        self.budget
    }

    pub fn set_budget(&mut self, budget: Option<MonetaryBudget>) -> &mut Self {
        self.budget = budget;
        self
    }

    /// This layer's configured age, or `None` for "never decided".
    pub fn stale_after(&self) -> Option<QuotaStaleAfterSeconds> {
        self.stale_after_seconds
    }

    pub fn set_stale_after(&mut self, value: Option<QuotaStaleAfterSeconds>) -> &mut Self {
        self.stale_after_seconds = value;
        self
    }

    /// This provider's own protected reserve percentage, or `None` for
    /// "never decided" — capability map line 1288.
    pub fn reserve_percent(&self) -> Option<PremiumReservePercent> {
        self.reserve_percent
    }

    pub fn set_reserve_percent(&mut self, value: Option<PremiumReservePercent>) -> &mut Self {
        self.reserve_percent = value;
        self
    }

    /// Whether the user recorded anything at all here.
    pub fn is_empty(&self) -> bool {
        self.plan.is_none()
            && self.budget.is_none()
            && self.stale_after_seconds.is_none()
            && self.reserve_percent.is_none()
    }
}
/// A quota configuration value outside the range its field accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum QuotaValueError {
    #[error(
        "a quota staleness age of {seconds}s is longer than the maximum {max_seconds}s; a          reading older than that should not be routed on under any policy"
    )]
    StaleAfter { seconds: u32, max_seconds: u32 },

    #[error(
        "a monetary budget of {micro_usd} microdollars is above the maximum {max_micro_usd};          this field is in millionths of a US dollar, so a plain dollar figure here is off by a          million"
    )]
    Budget { micro_usd: u64, max_micro_usd: u64 },

    /// Capability map line 1270: capacity-band thresholds must be ascending
    /// and are refused outright rather than sorted into shape. Wraps
    /// [`crate::provider::quota::CapacityBandThresholdsError`] rather than
    /// repeating its fields, so the two can never say different things about
    /// the same refusal.
    #[error("{0}")]
    BandThresholds(#[from] crate::provider::quota::CapacityBandThresholdsError),
}
/// Why a stored [`ProviderConfig`] could not be turned into a
/// [`crate::provider::Provider`].
#[derive(Debug, thiserror::Error)]
pub enum ProviderConfigError {
    #[error(
        "provider `{name}` names template `{template}`, which Glasshouse does not know; fix \
         the provider's `template` key or remove the entry"
    )]
    UnknownTemplate { name: String, template: String },

    /// A header *name* would reach an `ANTHROPIC_CUSTOM_HEADERS` line or a
    /// Codex `-c model_providers.<id>.http_headers=…` TOML literal carrying
    /// a character neither would parse as part of the name itself.
    #[error(
        "provider `{name}` names a header {header_name:?} that contains {offending:?}; a \
         header name may use letters, digits and `-` only"
    )]
    InvalidHeaderName {
        name: String,
        header_name: String,
        offending: char,
    },

    /// A header *value* containing a control character — most importantly
    /// `\r` or `\n`, which would let a header value inject a second header of
    /// its own choosing into every request this provider's child process
    /// sends. Refused, never escaped.
    #[error(
        "provider `{name}`'s header {header_name:?} has a value that contains {offending:?}, a \
         control character; a header value must not contain one"
    )]
    InvalidHeaderValue {
        name: String,
        header_name: String,
        offending: char,
    },
}
/// The on-disk shape of a [`crate::secret::SecretRef::OsCredential`].
///
/// Two names and nothing else, which is why it may be serialized at all —
/// see [`ProviderConfig::credential_store`]. It lives here rather than in
/// [`mod@crate::secret`] on purpose: that module's own tests forbid the word
/// `Serialize` anywhere in its production code, so the shape a *reference*
/// takes on disk is a configuration-schema decision and the value handling
/// stays somewhere that can never be serialized. The two halves cannot
/// drift, because [`StoredCredentialRef::to_secret_ref`] is the only bridge
/// between them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredCredentialRef {
    service: String,
    account: String,
}
impl StoredCredentialRef {
    pub fn new(service: impl Into<String>, account: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            account: account.into(),
        }
    }

    pub fn service(&self) -> &str {
        &self.service
    }

    pub fn account(&self) -> &str {
        &self.account
    }

    /// The reference a [`crate::secret::SecretStore`] can be asked with.
    pub fn to_secret_ref(&self) -> crate::secret::SecretRef {
        crate::secret::SecretRef::OsCredential {
            service: self.service.clone(),
            account: self.account.clone(),
        }
    }

    /// The stored shape of `reference`, or `None` for one that names no OS
    /// credential.
    ///
    /// `None` rather than an invented service name: an
    /// [`crate::secret::SecretRef::Environment`] reference is not a stored
    /// credential, and writing one here as though it were would claim
    /// something about where a key lives that nobody established.
    pub fn from_secret_ref(reference: &crate::secret::SecretRef) -> Option<Self> {
        match reference {
            crate::secret::SecretRef::OsCredential { service, account } => {
                Some(Self::new(service.clone(), account.clone()))
            }
            crate::secret::SecretRef::Environment { .. } => None,
        }
    }
}
/// One model's declared resource facts, as stored under
/// `[providers.<name>.model_facts.<model>]` — the serialisable, per-axis
/// mirror of [`crate::routing::capability::ResourceFacts`]'s seven axes.
///
/// Every field optional and independent: `None` on an axis means the user
/// has not declared it, exactly as an absent model in
/// [`ProviderConfig::model_facts`] means the same thing one level up. See
/// [`ProviderConfig::resource_facts_of`], the only place these become a
/// [`crate::routing::capability::ResourceFacts`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfiguredModelFacts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_edit: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_tool_use: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser_use: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub large_context: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast_cheap_analysis: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_review: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp: Option<bool>,
}
/// Which configuration table a user-declared fact came from — the two
/// places a fact can be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DeclaredIn {
    /// `[providers.<name>] tool_calls = …`
    ProviderToolCalls,
    /// `[providers.<name>.model_facts.<model>] <axis> = …`
    ModelFacts,
}
/// Turn a user's config-time declaration into the `'static` evidence
/// [`crate::harness::Declared::verified`] requires: four literals, one per
/// (layer, table) pair, and **no allocation**. The provider and model names
/// are deliberately not in the text. `Declared`'s evidence is `&'static str`
/// everywhere it is constructed, and the alternative — leaking a formatted
/// string per resolved fact — is unbounded in `glasshouse api serve`, which
/// answers `RecommendRoute` requests for as long as it runs and resolves
/// configuration for each one. The destination a reason is printed beside
/// already names its provider and model, so the text stays re-checkable
/// without repeating them.
pub(super) fn declared_from_config(layer: Layer, table: DeclaredIn) -> &'static str {
    match (layer, table) {
        (Layer::Project, DeclaredIn::ProviderToolCalls) => {
            "declared as tool_calls in the project config's [providers] table"
        }
        (Layer::User | Layer::Default, DeclaredIn::ProviderToolCalls) => {
            "declared as tool_calls in the user config's [providers] table"
        }
        (Layer::Project, DeclaredIn::ModelFacts) => {
            "declared in the project config's [providers.*.model_facts] table"
        }
        (Layer::User | Layer::Default, DeclaredIn::ModelFacts) => {
            "declared in the user config's [providers.*.model_facts] table"
        }
    }
}
impl ProviderConfig {
    pub fn new(template: impl Into<String>) -> Self {
        Self {
            template: template.into(),
            base_url: None,
            credential_env: Vec::new(),
            credential_store: None,
            headers: Vec::new(),
            enabled: true,
            free_models: Vec::new(),
            metered_models: Vec::new(),
            tool_calls: None,
            model_facts: BTreeMap::new(),
            model_ceilings: BTreeMap::new(),
            model_capabilities: BTreeMap::new(),
            prompt_transform: None,
            quota: None,
        }
    }

    pub fn template(&self) -> &str {
        &self.template
    }

    pub fn set_template(&mut self, template: impl Into<String>) -> &mut Self {
        self.template = template.into();
        self
    }

    /// This layer's quota overrides, or `None` when this layer recorded
    /// none — capability map lines 1233, 1203 and 1237.
    pub fn quota(&self) -> Option<&QuotaOverride> {
        self.quota.as_ref()
    }

    pub fn set_quota(&mut self, quota: Option<QuotaOverride>) -> &mut Self {
        self.quota = quota;
        self
    }

    pub fn base_url(&self) -> Option<&str> {
        self.base_url.as_deref()
    }

    pub fn set_base_url(&mut self, base_url: Option<String>) -> &mut Self {
        self.base_url = base_url;
        self
    }

    pub fn credential_env(&self) -> &[String] {
        &self.credential_env
    }

    pub fn set_credential_env(&mut self, names: Vec<String>) -> &mut Self {
        self.credential_env = names;
        self
    }

    /// Where this provider's credential is stored, when the user put it in
    /// the OS's own secure store — see the field's own doc.
    pub fn credential_store(&self) -> Option<&StoredCredentialRef> {
        self.credential_store.as_ref()
    }

    /// `None` clears the record, which is the configuration half of
    /// deleting a stored credential.
    pub fn set_credential_store(&mut self, reference: Option<StoredCredentialRef>) -> &mut Self {
        self.credential_store = reference;
        self
    }

    pub fn headers(&self) -> &[(String, String)] {
        &self.headers
    }

    pub fn set_headers(&mut self, headers: Vec<(String, String)>) -> &mut Self {
        self.headers = headers;
        self
    }

    /// Whether this provider is currently enabled. `true` for a provider no
    /// one has ever disabled, including one loaded from a file written
    /// before this field existed.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Disabling keeps every other field untouched and is fully reversible —
    /// see the field's own doc comment.
    pub fn set_enabled(&mut self, enabled: bool) -> &mut Self {
        self.enabled = enabled;
        self
    }

    /// The model identifiers on this provider marked free-tier or
    /// zero-marginal-cost. See the field's own doc.
    pub fn free_models(&self) -> &[String] {
        &self.free_models
    }

    pub fn set_free_models(&mut self, models: Vec<String>) -> &mut Self {
        self.free_models = models;
        self
    }

    /// The model identifiers the user has explicitly named as eligible for
    /// metered disposable-job fallback. See the field's own doc for why an
    /// empty list is the off state rather than a separate flag.
    pub fn metered_models(&self) -> &[String] {
        &self.metered_models
    }

    pub fn set_metered_models(&mut self, models: Vec<String>) -> &mut Self {
        self.metered_models = models;
        self
    }

    /// Whether this provider's protocols carry tool calls, as the user
    /// declared it — `None` when nobody has. See the field's own doc.
    pub fn tool_calls(&self) -> Option<bool> {
        self.tool_calls
    }

    pub fn set_tool_calls(&mut self, tool_calls: Option<bool>) -> &mut Self {
        self.tool_calls = tool_calls;
        self
    }

    /// Overlay [`ProviderConfig::tool_calls`] onto every protocol `provider`
    /// declares — [`ProviderConfig::to_provider`]'s own doc explains why
    /// this is a step its caller takes rather than something that method
    /// does itself: the evidence string needs `layer`, which a `Provider`
    /// resolved from a bare template has no way to supply.
    ///
    /// `None` (nobody declared) touches nothing, leaving whatever
    /// `to_provider` already produced — the template's own `Unverified` on
    /// every real provider today. `Some(value)` overrides every protocol's
    /// `tool_calls` with [`crate::harness::Declared::verified`], citing
    /// `layer` and the `[providers]` table exactly as
    /// [`ProviderConfig::resource_facts_of`]'s reason does one level down.
    pub fn declare_tool_calls(&self, provider: &mut crate::provider::Provider, layer: Layer) {
        if let Some(declared) = self.tool_calls {
            let reason = declared_from_config(layer, DeclaredIn::ProviderToolCalls);
            for protocol in &mut provider.protocols {
                protocol.tool_calls = crate::harness::Declared::verified(declared, reason);
            }
        }
    }

    /// The per-model resource facts the user configured — map line 1517.
    /// See [`ProviderConfig::resource_facts_of`] for how a lookup by model
    /// name reads these.
    pub fn model_facts(&self) -> &BTreeMap<String, ConfiguredModelFacts> {
        &self.model_facts
    }

    pub fn set_model_facts(&mut self, facts: BTreeMap<String, ConfiguredModelFacts>) -> &mut Self {
        self.model_facts = facts;
        self
    }

    /// `model`'s declared resource facts on this provider — map line 1517's
    /// producer, turning [`ProviderConfig::model_facts`]'s per-axis
    /// `Option<bool>`s into [`crate::routing::capability::ResourceFacts`]'s
    /// `Declared<bool>`s. `layer` is which configuration layer is asking,
    /// for the evidence string's own `[providers.*.model_facts]` table.
    ///
    /// A model absent from [`ProviderConfig::model_facts`] answers
    /// [`crate::routing::capability::ResourceFacts::UNVERIFIED`] outright.
    /// An axis absent from a present model's table stays
    /// [`crate::harness::Declared::Unverified`] on that axis alone — a
    /// missing key never upgrades to `Verified`.
    pub fn resource_facts_of(
        &self,
        model: &str,
        layer: Layer,
    ) -> crate::routing::capability::ResourceFacts {
        use crate::routing::capability::ResourceFacts;

        let Some(config) = self.model_facts.get(model) else {
            return ResourceFacts::UNVERIFIED;
        };
        let reason = declared_from_config(layer, DeclaredIn::ModelFacts);
        let axis = |value: Option<bool>| match value {
            Some(v) => crate::harness::Declared::verified(v, reason),
            None => crate::harness::Declared::Unverified,
        };
        ResourceFacts {
            code_edit: axis(config.code_edit),
            shell_tool_use: axis(config.shell_tool_use),
            browser_use: axis(config.browser_use),
            large_context: axis(config.large_context),
            fast_cheap_analysis: axis(config.fast_cheap_analysis),
            repository_review: axis(config.repository_review),
            mcp: axis(config.mcp),
        }
    }

    /// The per-model workload-tier ceilings the user configured — map line
    /// 1796. See the field's own doc for why an absent model is not a low
    /// ceiling.
    pub fn model_ceilings(&self) -> &BTreeMap<String, ConfiguredWorkloadTier> {
        &self.model_ceilings
    }

    pub fn set_model_ceilings(
        &mut self,
        ceilings: BTreeMap<String, ConfiguredWorkloadTier>,
    ) -> &mut Self {
        self.model_ceilings = ceilings;
        self
    }

    /// The highest workload tier `model` on this provider is established to
    /// serve, or `None` when nobody has stated one.
    ///
    /// The one place this lookup lives, so no call site re-implements it —
    /// [`ProviderConfig::cost_of`]'s own rule. There is no inference and no
    /// default: a model nobody named here answers `None`, which the router
    /// reads as *not established* rather than as a refusal.
    pub fn ceiling_of(&self, model: &str) -> Option<crate::routing::classify::WorkloadTier> {
        self.model_ceilings
            .get(model)
            .map(|configured| configured.tier())
    }

    /// The calibrated capability records the user configured for this
    /// provider — Phase 34F, capability map line 1475. See the field's own
    /// doc for why `model` and `backend` are not fields of the record
    /// itself.
    pub fn model_capabilities(&self) -> &BTreeMap<String, capability::ModelCapabilityRecord> {
        &self.model_capabilities
    }

    pub fn set_model_capabilities(
        &mut self,
        records: BTreeMap<String, capability::ModelCapabilityRecord>,
    ) -> &mut Self {
        self.model_capabilities = records;
        self
    }

    /// The calibrated record for `model` on this provider, or `None` when
    /// nobody has recorded one — capability map line 1475's "configurable
    /// data" read back.
    pub fn model_capability(&self, model: &str) -> Option<&capability::ModelCapabilityRecord> {
        self.model_capabilities.get(model)
    }

    /// The one lookup capability map lines 1476, 1478, 1479, and 1484 share:
    /// [`ProviderConfig::ceiling_of`]'s own override always wins; failing
    /// that, `model`'s capability record contributes its initial ceiling
    /// (capped by its task-kind suitability) when the user assigned it
    /// themselves, or only a non-binding prior when a benchmark seeded it.
    /// See [`capability::resolve_ceiling`] for why a benchmark-derived
    /// record can rank but never refuse.
    ///
    /// Context-blind: it knows only `model` and the provider `self` is, so
    /// only a record with no harness/launch-profile/protocol narrowing at
    /// all (`ModelCapabilityRecord::is_context_general`) is eligible — a
    /// narrowed record is filtered out rather than applied unchecked,
    /// because this path has no harness/profile/protocol to check
    /// [`capability::ModelCapabilityRecord::applies_to`] against (that
    /// context exists only in `main.rs`'s destination construction), so
    /// honouring it here would leak calibration onto every destination
    /// sharing this provider and model. Capability map line 1482.
    // History: design-decisions.md, "Trims: api, events, harness and config module docs, second packet", crates/glasshouse/src/config/provider.rs `ProviderConfig::resolved_ceiling`.
    pub fn resolved_ceiling(&self, model: &str) -> capability::CeilingResolution {
        let record = self
            .model_capability(model)
            .filter(|record| record.is_context_general());
        capability::resolve_ceiling(self.ceiling_of(model), record)
    }

    /// [`Self::resolved_ceiling`], for a caller that actually has the
    /// harness/launch-profile/protocol context a destination is built with —
    /// capability map line 1482's closing half. Where [`Self::resolved_ceiling`]
    /// may only trust a record that narrows nothing at all, this checks each
    /// record's own [`capability::ModelCapabilityRecord::applies_to`] against
    /// `query` instead: a record scoped to a harness, profile, or protocol the
    /// query does not name is filtered out exactly as before, and a record
    /// that narrows to axes `query` *does* state is now honoured rather than
    /// blanket-excluded.
    ///
    /// **The production caller is `main.rs::destination_tier_ceiling`**,
    /// which builds `query` from the same launch context — harness, launch
    /// profile, protocol — that `main.rs::routing_destinations` already has
    /// in hand for every destination it constructs.
    pub fn resolved_ceiling_for(
        &self,
        model: &str,
        query: &capability::CapabilityQuery<'_>,
    ) -> capability::CeilingResolution {
        let record = self
            .model_capability(model)
            .filter(|record| record.applies_to(query));
        capability::resolve_ceiling(self.ceiling_of(model), record)
    }

    /// What this backend does to a prompt on the way through, as the user
    /// described it — Phase 9K line 609. `None` means nothing was declared,
    /// which is not the same as "nothing happens".
    pub fn prompt_transform(&self) -> Option<&str> {
        self.prompt_transform.as_deref()
    }

    pub fn set_prompt_transform(&mut self, transform: Option<String>) -> &mut Self {
        self.prompt_transform = transform;
        self
    }

    /// Whether `model` costs the user anything at the margin.
    ///
    /// The one place this lookup lives, so no call site re-implements it. A
    /// model this provider has not named in [`ProviderConfig::free_models`]
    /// answers [`crate::routing::Cost::Metered`] — including a model whose
    /// name happens to end in `:free` or look free by any other convention.
    /// There is no inference here; see the field's own doc for why.
    pub fn cost_of(&self, model: &str) -> crate::routing::Cost {
        if self.free_models.iter().any(|marked| marked == model) {
            crate::routing::Cost::Free
        } else {
            crate::routing::Cost::Metered
        }
    }

    /// Turn this stored configuration into the resolvable domain type,
    /// naming it `name` — the key this entry was stored under.
    ///
    /// The template's own base URL is overridden, on every protocol it
    /// declares, when [`ProviderConfig::base_url`] is set — see the field's
    /// own doc for why there is exactly one override rather than one per
    /// protocol: every built-in template today declares exactly one
    /// protocol. Likewise, a non-empty [`ProviderConfig::credential_env`]
    /// replaces the template's own credential names rather than adding to
    /// them.
    ///
    /// **Does not apply [`ProviderConfig::tool_calls`].** That declaration
    /// needs the configuration *layer* this entry was read from, for its
    /// evidence string, and this method's callers outside this module have
    /// no layer to give it — so the override is applied one layer up, in
    /// [`EffectiveConfig::configured_provider`], the one caller that reads
    /// this entry with a layer already in hand. Every other caller of this
    /// method sees exactly what it saw before [`ProviderConfig::tool_calls`]
    /// existed.
    pub fn to_provider(
        &self,
        name: &str,
    ) -> Result<crate::provider::Provider, ProviderConfigError> {
        let mut provider = crate::provider::template(&self.template).ok_or_else(|| {
            ProviderConfigError::UnknownTemplate {
                name: name.to_owned(),
                template: self.template.clone(),
            }
        })?;

        provider.name = name.to_owned();
        if let Some(base_url) = &self.base_url {
            for protocol in &mut provider.protocols {
                protocol.base_url = base_url.clone();
            }
        }
        if !self.credential_env.is_empty() {
            provider.credential_env = self.credential_env.clone();
        }
        if !self.headers.is_empty() {
            for (header_name, value) in &self.headers {
                if let Some(offending) = unsafe_header_name_char(header_name) {
                    return Err(ProviderConfigError::InvalidHeaderName {
                        name: name.to_owned(),
                        header_name: header_name.clone(),
                        offending,
                    });
                }
                if let Some(offending) = unsafe_header_value_char(value) {
                    return Err(ProviderConfigError::InvalidHeaderValue {
                        name: name.to_owned(),
                        header_name: header_name.clone(),
                        offending,
                    });
                }
            }
            provider.headers = self.headers.clone();
        }

        Ok(provider)
    }
}
/// The first character of `name` that must not reach a header rendered into
/// an `ANTHROPIC_CUSTOM_HEADERS` line or a Codex `-c` TOML literal, or `None`
/// when every character is safe.
///
/// A header field-name is narrower than [`crate::shim`]'s `check_name`:
/// letters, digits and `-` only — no `_` and no `.`, neither of which an HTTP
/// header name uses. See that function for the shape this one follows.
fn unsafe_header_name_char(name: &str) -> Option<char> {
    name.chars()
        .find(|c| !(c.is_ascii_alphanumeric() || *c == '-'))
}
/// The first character of `value` that must not reach a header value line,
/// or `None` when every character is safe.
///
/// Any control character is refused, which already covers `\r` and `\n`:
/// a header *value* carrying either would let it inject a second header of
/// the attacker's own choosing into every request this provider's child
/// process sends — the exact class [`crate::shim`]'s `check_name` already
/// refuses for profile names, applied here to a header value instead of a
/// name. Refused, never escaped.
fn unsafe_header_value_char(value: &str) -> Option<char> {
    value.chars().find(|c| c.is_control())
}
/// A free resource by name, as configuration stores it — the serialisable
/// counterpart to [`crate::routing::free::FreeResourceKey`].
///
/// That type is frozen in `crate::routing` and derives neither `Serialize`
/// nor `Deserialize`, the same reason [`StoredCredentialRef`] exists beside
/// [`crate::secret::SecretRef`] rather than that type growing serde impls of
/// its own: the shape a *reference* takes on disk is a configuration-schema
/// decision, kept separate from the domain type routing policy reasons
/// about. [`FreeResourceRef::to_key`] and [`FreeResourceRef::from_key`] are
/// the only bridge between the two, so they cannot drift.
///
/// Two names — a provider and a model — and nothing else, for the same
/// reason [`RoutingModelChoice::Pinned`] holds only names: both are as safe
/// to write into a tracked configuration file as
/// [`ProviderConfig::credential_env`]'s variable names already are.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreeResourceRef {
    provider: String,
    model: String,
}
impl FreeResourceRef {
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
        }
    }

    pub fn provider(&self) -> &str {
        &self.provider
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn to_key(&self) -> crate::routing::free::FreeResourceKey {
        crate::routing::free::FreeResourceKey::new(self.provider.clone(), self.model.clone())
    }

    pub fn from_key(key: &crate::routing::free::FreeResourceKey) -> Self {
        Self::new(key.provider.clone(), key.model.clone())
    }
}
/// The provider and model a user has chosen to perform memory extraction —
/// Phase 21's *"allow a configurable cheap or local model to perform memory
/// extraction."*
///
/// # Why this is its own field and not a reuse of the routing preferences
///
/// [`FreeResourceRef`] is the same two strings, and reusing it was the
/// tempting move. It would have been wrong: the free-routing preferences say
/// *which resource to prefer when Glasshouse routes*, and a user who has
/// written them has not thereby asked Glasshouse to start making outbound
/// requests from a hook that runs inside their coding session. This field is
/// that request, made once and explicitly, and `None` — the default — is
/// exactly today's behaviour.
///
/// # Names only, exactly like every other provider field here
///
/// A provider name and a model identifier. The base URL, the credential
/// variable names and any extra headers all come from the
/// [`ProviderConfig`] this names, which is where they already live.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractionModelRef {
    provider: String,
    model: String,
}
impl ExtractionModelRef {
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
        }
    }

    pub fn provider(&self) -> &str {
        &self.provider
    }

    pub fn model(&self) -> &str {
        &self.model
    }
}
/// A map of configured providers, keyed by provider name.
///
/// Providers are configuration, never a credential store: every value this
/// table can hold is a template slug, a base-URL override, or credential
/// variable *names* — see [`ProviderConfig`] and the module-level "No
/// secrets here" section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderTable(BTreeMap<String, ProviderConfig>);
impl ProviderTable {
    pub fn get(&self, name: &str) -> Option<&ProviderConfig> {
        self.0.get(name)
    }

    pub fn set(&mut self, name: impl Into<String>, config: ProviderConfig) {
        self.0.insert(name.into(), config);
    }

    pub fn remove(&mut self, name: &str) -> Option<ProviderConfig> {
        self.0.remove(name)
    }

    /// Every configured provider name in this table.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &ProviderConfig)> {
        self.0.iter().map(|(name, cfg)| (name.as_str(), cfg))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}
/// Why a provider named on the command line, or looked up for `glasshouse
/// doctor`, could not be resolved.
#[derive(Debug, thiserror::Error)]
pub enum ProviderLookupError {
    #[error("`{name}` is not a configured provider; valid names are: {}", .known.join(", "))]
    Unknown { name: String, known: Vec<String> },
    #[error(transparent)]
    Invalid(#[from] ProviderConfigError),
}
