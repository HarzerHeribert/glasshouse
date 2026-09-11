//! Entitlement **policy**: which harnesses and job kinds an account is for,
//! how Glasshouse layers the `[entitlements]` tables into one resolved list,
//! and the facets it derives from its own evidence ledger.
//!
//! The client-neutral half — what an account *is*, how it authenticates, what
//! it can serve — lives in `super::inference_gateway::entitlement` and is re-exported
//! below, so every path that named one of those items here before the split
//! still resolves. The dependency runs one way: policy reads catalogue, and
//! the catalogue names nothing in this file.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::integrations::IntegrationId;
use crate::secret::SecretRef;

use super::*;

pub use inference_gateway::entitlement::{
    AccountEntry, EntitlementCredential, EntitlementKind, EntitlementModels,
    EntitlementSpendReading, EntitlementThrottleReading, EntitlementVendor, ResolvedAccount,
    SubscriptionBroker, TelemetryScope,
};
// Not public, and re-exported at exactly the visibility it had before the
// move: `config::provider`'s `credential_env` names it through `use super::*`.
pub(super) use inference_gateway::entitlement::deserialize_credential_env_names;

// ---------------------------------------------------------------------------
// Phase 56/56A — `[entitlements.<name>]`: an entitlement — a specific
// subscription or API-credit account — as the configured unit of capacity,
// with rules of its own (map lines 1946, 1947, 1954, 1962, 1963, 1973).
// ---------------------------------------------------------------------------
/// A harness as it is written in a `[entitlements]` rule — the
/// [`IntegrationId::slug`], parsed against the **harnesses** this build
/// knows. A local inference runtime (`ollama`, `llama-cpp`) or the terminal
/// multiplexer is refused by the loader: an entitlement serves a harness,
/// and a rule naming something that is not one would be a rule nothing can
/// ever match — the silent kind of wrong this project keeps finding.
///
/// The same newtype-over-a-routing-type shape as [`ConfiguredWorkloadTier`],
/// for the same reason: `IntegrationId` has no serialised form of its own,
/// and this is the config file's side of that boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfiguredHarness(IntegrationId);
impl ConfiguredHarness {
    pub fn new(id: IntegrationId) -> Self {
        Self(id)
    }

    pub fn id(self) -> IntegrationId {
        self.0
    }

    pub fn as_str(self) -> &'static str {
        self.0.slug()
    }

    /// Every integration an entitlement can serve, in presentation order.
    fn harnesses() -> impl Iterator<Item = IntegrationId> {
        IntegrationId::ALL
            .iter()
            .copied()
            .filter(|id| id.kind() == crate::integrations::IntegrationKind::Harness)
    }

    /// The harness a slug names, or `None` for one that is not a harness.
    /// Exact, like [`ConfiguredWorkloadTier::parse`].
    pub fn parse(text: &str) -> Option<Self> {
        Self::harnesses().find(|id| id.slug() == text).map(Self)
    }
}
impl Serialize for ConfiguredHarness {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}
impl<'de> Deserialize<'de> for ConfiguredHarness {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).ok_or_else(|| {
            let known = Self::harnesses()
                .map(IntegrationId::slug)
                .collect::<Vec<_>>()
                .join(", ");
            serde::de::Error::custom(format!(
                "unknown harness `{text}` — an entitlement rule names one of: {known}"
            ))
        })
    }
}
/// A [`crate::routing::disposable::JobKind`] as it is written in a
/// `[entitlements]` rule — the spelling is the kind's own `as_str`, and
/// `JOB_KIND_SPELLINGS` is kept complete by `job_kind_ordinal`'s
/// exhaustive `match`, exactly as [`ConfiguredWorkloadTier`] is kept honest
/// by `workload_tier_ordinal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfiguredJobKind(crate::routing::disposable::JobKind);
/// Every [`crate::routing::disposable::JobKind`], in the type's own order.
/// Kept complete by `job_kind_ordinal`.
pub(super) const JOB_KIND_SPELLINGS: [crate::routing::disposable::JobKind; 5] = {
    use crate::routing::disposable::JobKind as J;
    [
        J::Classification,
        J::MemoryExtraction,
        J::Reranking,
        J::Evaluation,
        J::ContextReduction,
    ]
};
/// The compile-time guard that `JOB_KIND_SPELLINGS` still lists every
/// variant — see `workload_tier_ordinal` for why this is `#[cfg(test)]`
/// and still a real gate.
#[cfg(test)]
pub(super) fn job_kind_ordinal(kind: crate::routing::disposable::JobKind) -> usize {
    use crate::routing::disposable::JobKind as J;
    match kind {
        J::Classification => 0,
        J::MemoryExtraction => 1,
        J::Reranking => 2,
        J::Evaluation => 3,
        J::ContextReduction => 4,
    }
}
impl ConfiguredJobKind {
    pub fn new(kind: crate::routing::disposable::JobKind) -> Self {
        Self(kind)
    }

    pub fn kind(self) -> crate::routing::disposable::JobKind {
        self.0
    }

    pub fn as_str(self) -> &'static str {
        self.0.as_str()
    }

    /// Exact, like [`ConfiguredWorkloadTier::parse`].
    pub fn parse(text: &str) -> Option<Self> {
        JOB_KIND_SPELLINGS
            .into_iter()
            .find(|kind| kind.as_str() == text)
            .map(Self)
    }
}
impl Serialize for ConfiguredJobKind {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}
impl<'de> Deserialize<'de> for ConfiguredJobKind {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).ok_or_else(|| {
            let known = JOB_KIND_SPELLINGS
                .into_iter()
                .map(|kind| kind.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            serde::de::Error::custom(format!(
                "unknown job kind `{text}` — expected one of: {known}"
            ))
        })
    }
}
/// A [`crate::routing::evidence::HeadroomBand`] as it is written in a
/// configuration file — map line 1252's override. Same shape and same
/// reason as [`ConfiguredWorkloadTier`] just above: `HeadroomBand` is a
/// routing type this crate derives from evidence it reads itself, and
/// giving it a `Deserialize` impl directly would make that derived value and
/// a user's typed-in correction the same surface. This newtype is the
/// config file's side of that boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfiguredHeadroomBand(crate::routing::evidence::HeadroomBand);
/// Every [`crate::routing::evidence::HeadroomBand`], in the type's own
/// presentation order — the same order `main.rs::entitlement_facets` renders
/// them in.
const HEADROOM_BAND_SPELLINGS: [crate::routing::evidence::HeadroomBand; 4] = {
    use crate::routing::evidence::HeadroomBand as B;
    [B::Exhausted, B::Low, B::Moderate, B::Ample]
};
fn headroom_band_spelling(band: crate::routing::evidence::HeadroomBand) -> &'static str {
    use crate::routing::evidence::HeadroomBand as B;
    match band {
        B::Exhausted => "exhausted",
        B::Low => "low",
        B::Moderate => "moderate",
        B::Ample => "ample",
    }
}
impl ConfiguredHeadroomBand {
    pub fn new(band: crate::routing::evidence::HeadroomBand) -> Self {
        Self(band)
    }

    pub fn band(self) -> crate::routing::evidence::HeadroomBand {
        self.0
    }

    /// The spelling a user writes.
    pub fn as_str(self) -> &'static str {
        headroom_band_spelling(self.0)
    }

    /// The band a spelling names, or `None` for one no variant answers to.
    /// Case-sensitive and untrimmed, the same discipline
    /// [`ConfiguredWorkloadTier::parse`] applies.
    pub fn parse(text: &str) -> Option<Self> {
        HEADROOM_BAND_SPELLINGS
            .into_iter()
            .find(|band| headroom_band_spelling(*band) == text)
            .map(Self)
    }
}
impl Serialize for ConfiguredHeadroomBand {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}
impl<'de> Deserialize<'de> for ConfiguredHeadroomBand {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).ok_or_else(|| {
            let known = HEADROOM_BAND_SPELLINGS
                .into_iter()
                .map(headroom_band_spelling)
                .collect::<Vec<_>>()
                .join(", ");
            serde::de::Error::custom(format!(
                "unknown headroom band `{text}` — expected one of: {known}"
            ))
        })
    }
}
/// Glasshouse's **policy overlay** on one gateway account — what an account
/// may be used *for*, keyed by the name the gateway files it under.
///
/// User ruling, 2026-09-11: the account itself — its plan, its billing
/// vendor, its credential reference, its broker and its provider — is the
/// gateway's, and lives in `gateway.toml`'s `[accounts.<name>]`
/// ([`AccountEntry`]). Any of those five keys written here is **refused**
/// with the name of the command that moves them; see this type's
/// `Deserialize`. What stays is the half a gateway has no opinion about:
/// which harnesses, tiers and job kinds the account may serve, the spend
/// ceiling, the headroom correction, and the context-firewall choice.
///
/// `native_harness` stays too, and is not an exception to that rule: it says
/// that *a harness's own sign-in is a route to this account*, which is a
/// statement about Glasshouse's harnesses and about nothing the gateway
/// serves. It is still one of the three backings
/// ([`EntitlementBacking`]) — paired with the account's
/// `subscription_broker` because those are two routes to one subscription,
/// and mutually exclusive with its `provider`
/// ([`EntitlementLookupError::TwoBackings`]).
///
/// An overlay naming an account the gateway does not have is refused by
/// [`ConfigError::UnknownGatewayAccount`] rather than dropped: a rule the
/// user believes is in force and that matches nothing is the silent kind of
/// wrong this project keeps paying for.
///
/// Rules resolve through [`crate::routing::EntitlementRules`] and nowhere
/// else — deny wins over allow, and `deny_unknown_fields` keeps an
/// unrecognised rule from being silently read as "no rule".
///
/// History: design-decisions.md, "Trims: config, checkpoint, evaluation and codex module docs", entitlement.rs module doc `EntitlementConfig`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EntitlementConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    native_harness: Option<ConfiguredHarness>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    allow_harnesses: Vec<ConfiguredHarness>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    deny_harnesses: Vec<ConfiguredHarness>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    allow_tiers: Vec<ConfiguredWorkloadTier>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    deny_tiers: Vec<ConfiguredWorkloadTier>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    allow_job_kinds: Vec<ConfiguredJobKind>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    deny_job_kinds: Vec<ConfiguredJobKind>,
    /// Map line 1971's fourth axis: the cumulative token spend past which
    /// this entitlement may not be charged. Absent means *the user stated
    /// no ceiling*, never *zero*, exactly as every other absent field in
    /// this table does.
    ///
    /// **Tokens, not money, and that is not this field's own decision.**
    /// `routing_observations.cost_micro_usd` has one producer now — map line
    /// 1307, `main.rs::record_entitlement_fallback` — but it writes only on
    /// an entitlement-fallback event, so a ceiling stated in money could
    /// almost never be reached and the broker could almost never be held to
    /// it — see [`crate::routing::evidence::CredentialSpend`], and map line
    /// 1465's reader, which already answers the same question the same way
    /// in production. `[providers.<name>.quota] budget` remains the money
    /// ceiling (map line 1203) and remains, by its own documentation,
    /// uncounted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    spend_ceiling_tokens: Option<u64>,
    /// Map line 1252: a user's own correction of an obviously incorrect
    /// subscription-headroom estimate. Authoritative over the derived
    /// band the moment it is set — that is the whole point of the line —
    /// but [`main.rs::entitlement_facets`] renders it in its own distinct
    /// vocabulary ("your reading", never the confidence-and-basis phrasing
    /// the derived estimate uses) so a substitution is never silent.
    /// Expressed as a [`crate::routing::evidence::HeadroomBand`], the same
    /// vocabulary the estimate itself uses — never a percentage or a token
    /// figure, so 1250/1251's honesty rules are not weakened by the one
    /// value a person, not evidence, supplies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    headroom_override: Option<ConfiguredHeadroomBand>,
    /// Map line 1255: skip the subscription-headroom estimator for this
    /// entitlement entirely, for a user who wants only authoritative usage
    /// data. `false` on a file written before this field existed, the same
    /// "absent reads as off" contract [`ProfileConfig::pin_gateway_backend`]
    /// already keeps. Per-entitlement rather than global: two entitlements
    /// in one config can disagree, which is exactly what
    /// `tests/subscription_estimator.rs`'s 1255 acceptance test proves side
    /// by side. Disabling touches nothing else this entry renders —
    /// `capacity`, `reset`, `throttling` and `models` are populated earlier
    /// in [`ResolvedEntitlement::populate_provider_facets`] and this field
    /// is read only afterward, to skip the estimator call alone.
    #[serde(default, skip_serializing_if = "is_false")]
    disable_headroom_estimate: bool,
    /// `[entitlements.<name>.context_firewall]` — map line 2024's explicit
    /// override: what this account's own reduction policy should be,
    /// outranking its kind's sub-table and the flat `[context_firewall]`
    /// table, and itself outranked by the launch profile's own override —
    /// the entitlement is what pays, the profile is the more specific
    /// choice. `None` here, on a file written before this field existed,
    /// loads as "this entitlement states no override".
    #[serde(
        default,
        skip_serializing_if = "firewall::ContextFirewallOverride::is_unset"
    )]
    context_firewall: firewall::ContextFirewallOverride,
}
impl EntitlementConfig {
    pub fn native_harness(&self) -> Option<IntegrationId> {
        self.native_harness.map(ConfiguredHarness::id)
    }

    pub fn set_native_harness(&mut self, value: Option<IntegrationId>) -> &mut Self {
        self.native_harness = value.map(ConfiguredHarness::new);
        self
    }

    pub fn set_allow_harnesses(
        &mut self,
        value: impl IntoIterator<Item = IntegrationId>,
    ) -> &mut Self {
        self.allow_harnesses = value.into_iter().map(ConfiguredHarness::new).collect();
        self
    }

    pub fn set_deny_harnesses(
        &mut self,
        value: impl IntoIterator<Item = IntegrationId>,
    ) -> &mut Self {
        self.deny_harnesses = value.into_iter().map(ConfiguredHarness::new).collect();
        self
    }

    pub fn set_allow_tiers(
        &mut self,
        value: impl IntoIterator<Item = crate::routing::classify::WorkloadTier>,
    ) -> &mut Self {
        self.allow_tiers = value.into_iter().map(ConfiguredWorkloadTier::new).collect();
        self
    }

    pub fn set_deny_tiers(
        &mut self,
        value: impl IntoIterator<Item = crate::routing::classify::WorkloadTier>,
    ) -> &mut Self {
        self.deny_tiers = value.into_iter().map(ConfiguredWorkloadTier::new).collect();
        self
    }

    pub fn set_allow_job_kinds(
        &mut self,
        value: impl IntoIterator<Item = crate::routing::disposable::JobKind>,
    ) -> &mut Self {
        self.allow_job_kinds = value.into_iter().map(ConfiguredJobKind::new).collect();
        self
    }

    pub fn set_deny_job_kinds(
        &mut self,
        value: impl IntoIterator<Item = crate::routing::disposable::JobKind>,
    ) -> &mut Self {
        self.deny_job_kinds = value.into_iter().map(ConfiguredJobKind::new).collect();
        self
    }

    /// Map line 1971's spend ceiling, in tokens, or `None` for *none
    /// stated*.
    pub fn spend_ceiling_tokens(&self) -> Option<u64> {
        self.spend_ceiling_tokens
    }

    pub fn set_spend_ceiling_tokens(&mut self, value: Option<u64>) -> &mut Self {
        self.spend_ceiling_tokens = value;
        self
    }

    /// Map line 1252's user override, or `None` for *the estimator's own
    /// reading stands*.
    pub fn headroom_override(&self) -> Option<crate::routing::evidence::HeadroomBand> {
        self.headroom_override.map(ConfiguredHeadroomBand::band)
    }

    pub fn set_headroom_override(
        &mut self,
        value: Option<crate::routing::evidence::HeadroomBand>,
    ) -> &mut Self {
        self.headroom_override = value.map(ConfiguredHeadroomBand::new);
        self
    }

    /// Map line 1255: `true` when this entitlement asked to skip the
    /// subscription-headroom estimator entirely.
    pub fn disable_headroom_estimate(&self) -> bool {
        self.disable_headroom_estimate
    }

    pub fn set_disable_headroom_estimate(&mut self, value: bool) -> &mut Self {
        self.disable_headroom_estimate = value;
        self
    }

    /// This entitlement's `[entitlements.<name>.context_firewall]`
    /// override — see the field's own doc.
    pub fn context_firewall(&self) -> &firewall::ContextFirewallOverride {
        &self.context_firewall
    }

    pub fn context_firewall_mut(&mut self) -> &mut firewall::ContextFirewallOverride {
        &mut self.context_firewall
    }

    /// This entry's six lists and its spend ceiling as the router's one
    /// rules value.
    pub fn rules(&self) -> crate::routing::EntitlementRules {
        crate::routing::EntitlementRules::UNRESTRICTED
            .allow_harnesses(self.allow_harnesses.iter().map(|h| h.id()))
            .deny_harnesses(self.deny_harnesses.iter().map(|h| h.id()))
            .allow_tiers(self.allow_tiers.iter().map(|t| t.tier()))
            .deny_tiers(self.deny_tiers.iter().map(|t| t.tier()))
            .allow_job_kinds(self.allow_job_kinds.iter().map(|k| k.kind()))
            .deny_job_kinds(self.deny_job_kinds.iter().map(|k| k.kind()))
            .with_spend_ceiling_tokens(self.spend_ceiling_tokens)
    }

    /// [`Self::rules`] with the gateway account's own spend ceiling standing
    /// in when this overlay states none.
    ///
    /// Both files can carry `spend_ceiling_tokens`, and they mean the same
    /// thing: the gateway's is what the account itself is held to, and an
    /// overlay's is the narrower ceiling a user set on top. The overlay wins
    /// when it states one, because it is the more specific statement; the
    /// account's applies otherwise, so a ceiling written only in
    /// `gateway.toml` is not quietly dropped on the way through.
    pub fn rules_over(&self, account: &AccountEntry) -> crate::routing::EntitlementRules {
        let ceiling = self.spend_ceiling_tokens.or(account.spend_ceiling_tokens());
        self.rules().with_spend_ceiling_tokens(ceiling)
    }

    /// The rules a gateway account with **no** overlay resolves under —
    /// unrestricted, carrying only the account's own ceiling. The default
    /// half of "rules from the overlay or the defaults".
    pub fn default_rules_for(account: &AccountEntry) -> crate::routing::EntitlementRules {
        crate::routing::EntitlementRules::UNRESTRICTED
            .with_spend_ceiling_tokens(account.spend_ceiling_tokens())
    }

    /// The resolved value for the gateway account `account`, named `name` —
    /// the key the gateway files it under — and attributed to `layer`.
    ///
    /// **The account comes from the gateway, never from this table.** That
    /// is the 2026-09-11 ruling in one signature: Glasshouse cannot resolve
    /// an entitlement it was not handed an account for, so it cannot invent
    /// one.
    pub fn to_resolved(
        &self,
        name: &str,
        account: &AccountEntry,
        layer: Layer,
    ) -> Result<ResolvedEntitlement, EntitlementLookupError> {
        // The backing is the one decision that reads both halves: the
        // gateway's account says which provider or broker serves it, and
        // `native_harness` — a harness identity, and so policy — says
        // whether the harness's own sign-in is a route to it.
        let backing = match (
            self.native_harness,
            account.subscription_broker(),
            account.provider().map(str::to_owned),
        ) {
            (Some(_), _, Some(_)) | (None, Some(_), Some(_)) => {
                return Err(EntitlementLookupError::TwoBackings {
                    name: name.to_owned(),
                });
            }
            (Some(harness), Some(broker), None) => EntitlementBacking::SubscriptionBroker {
                broker,
                native_harness: Some(harness.id()),
            },
            (None, Some(broker), None) => EntitlementBacking::SubscriptionBroker {
                broker,
                native_harness: None,
            },
            (Some(harness), None, None) => EntitlementBacking::NativeHarness(harness.id()),
            (None, None, Some(provider)) => EntitlementBacking::Provider(provider),
            (None, None, None) => EntitlementBacking::Unstated,
        };
        // A harness's own sign-in authenticates through the harness itself;
        // an entry claiming to be one while naming its own credential would
        // be two accounts wearing one name — map line 1973's isolation,
        // refused rather than resolved by guessing which authentication
        // counts.
        if matches!(backing, EntitlementBacking::NativeHarness(_)) && account.credential().is_some()
        {
            return Err(EntitlementLookupError::NativeSignInWithOwnCredential {
                name: name.to_owned(),
            });
        }
        if matches!(backing, EntitlementBacking::SubscriptionBroker { .. })
            && account.credential().is_some()
        {
            return Err(
                EntitlementLookupError::SubscriptionBrokerWithOwnCredential {
                    name: name.to_owned(),
                },
            );
        }
        Ok(ResolvedEntitlement {
            account: ResolvedAccount::resolve(name, account),
            backing,
            rules: self.rules_over(account),
            layer,
            headroom_estimate: None,
            headroom_override: self.headroom_override(),
            disable_headroom_estimate: self.disable_headroom_estimate,
            context_firewall: self.context_firewall.clone(),
        })
    }
}
/// The five `[entitlements.<name>]` keys the gateway owns since the
/// 2026-09-11 ruling, in the order a refusal checks them.
///
/// Spelled once, here, and read by three things that must agree: the
/// `Deserialize` refusal below, [`LegacyEntitlementConfig`] (which is the
/// only thing that still *reads* them, for the migration), and
/// `glasshouse doctor`'s legacy report.
pub const GATEWAY_ACCOUNT_KEYS: [&str; 5] = [
    "kind",
    "vendor",
    "credential",
    "subscription_broker",
    "provider",
];

/// The sentence a legacy account key gets. Names the key and the command
/// that moves it, and **never what was written** — one of these keys is a
/// credential reference, and a refusal that echoed its table would be the
/// one thing `EntitlementCredential`'s own refusal exists to avoid.
fn account_key_is_the_gateways(key: &str) -> String {
    format!(
        "`{key}` under `[entitlements]` is the gateway's: an account's plan, billing vendor, \
         credential reference, subscription broker and provider live in the gateway's \
         `gateway.toml` under `[accounts.<name>]`, and Glasshouse's `[entitlements.<name>]` \
         table states only policy about an account the gateway already has. Run `{}` to move \
         them; it writes `gateway.toml` and rewrites this file in place after taking a backup",
        crate::config::MIGRATE_COMMAND
    )
}

/// The wire shape `[entitlements.<name>]` is read through: every policy key,
/// plus the five the gateway owns — present **only so that they can be
/// refused by name**. `serde::de::IgnoredAny` because presence is the whole
/// of what this needs; the value is never built, so it can never be echoed.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EntitlementOverlayWire {
    #[serde(default)]
    kind: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    vendor: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    credential: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    subscription_broker: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    provider: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    native_harness: Option<ConfiguredHarness>,
    #[serde(default)]
    allow_harnesses: Vec<ConfiguredHarness>,
    #[serde(default)]
    deny_harnesses: Vec<ConfiguredHarness>,
    #[serde(default)]
    allow_tiers: Vec<ConfiguredWorkloadTier>,
    #[serde(default)]
    deny_tiers: Vec<ConfiguredWorkloadTier>,
    #[serde(default)]
    allow_job_kinds: Vec<ConfiguredJobKind>,
    #[serde(default)]
    deny_job_kinds: Vec<ConfiguredJobKind>,
    #[serde(default)]
    spend_ceiling_tokens: Option<u64>,
    #[serde(default)]
    headroom_override: Option<ConfiguredHeadroomBand>,
    #[serde(default)]
    disable_headroom_estimate: bool,
    #[serde(default)]
    context_firewall: firewall::ContextFirewallOverride,
}
impl<'de> Deserialize<'de> for EntitlementConfig {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;
        let wire = EntitlementOverlayWire::deserialize(deserializer)?;
        for (present, key) in [
            (wire.kind.is_some(), GATEWAY_ACCOUNT_KEYS[0]),
            (wire.vendor.is_some(), GATEWAY_ACCOUNT_KEYS[1]),
            (wire.credential.is_some(), GATEWAY_ACCOUNT_KEYS[2]),
            (wire.subscription_broker.is_some(), GATEWAY_ACCOUNT_KEYS[3]),
            (wire.provider.is_some(), GATEWAY_ACCOUNT_KEYS[4]),
        ] {
            if present {
                return Err(D::Error::custom(account_key_is_the_gateways(key)));
            }
        }
        Ok(Self {
            native_harness: wire.native_harness,
            allow_harnesses: wire.allow_harnesses,
            deny_harnesses: wire.deny_harnesses,
            allow_tiers: wire.allow_tiers,
            deny_tiers: wire.deny_tiers,
            allow_job_kinds: wire.allow_job_kinds,
            deny_job_kinds: wire.deny_job_kinds,
            spend_ceiling_tokens: wire.spend_ceiling_tokens,
            headroom_override: wire.headroom_override,
            disable_headroom_estimate: wire.disable_headroom_estimate,
            context_firewall: wire.context_firewall,
        })
    }
}

/// `[entitlements.<name>]` as it was written **before** the 2026-09-11
/// ruling, read by exactly two callers: `glasshouse migrate-gateway-state`,
/// which moves the five account keys into `gateway.toml`, and `glasshouse
/// doctor`, which reports that they are still there.
///
/// Nothing resolves through this type and nothing routes on it. It exists so
/// that the migration can read a file the loader now refuses — which is the
/// only way a migration can ever work.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LegacyEntitlementConfig {
    #[serde(default)]
    pub kind: Option<EntitlementKind>,
    #[serde(default)]
    pub vendor: Option<EntitlementVendor>,
    #[serde(default)]
    pub credential: Option<EntitlementCredential>,
    #[serde(default)]
    pub subscription_broker: Option<SubscriptionBroker>,
    #[serde(default)]
    pub provider: Option<String>,
}
impl LegacyEntitlementConfig {
    /// Whether this entry carries any of the five keys — the question both
    /// the migration's plan and the doctor's report ask.
    pub fn has_account_keys(&self) -> bool {
        self.kind.is_some()
            || self.vendor.is_some()
            || self.credential.is_some()
            || self.subscription_broker.is_some()
            || self.provider.is_some()
    }

    /// Which of the five are present, by name, in
    /// [`GATEWAY_ACCOUNT_KEYS`] order.
    pub fn account_keys_present(&self) -> Vec<&'static str> {
        let mut present = Vec::new();
        for (there, key) in [
            (self.kind.is_some(), GATEWAY_ACCOUNT_KEYS[0]),
            (self.vendor.is_some(), GATEWAY_ACCOUNT_KEYS[1]),
            (self.credential.is_some(), GATEWAY_ACCOUNT_KEYS[2]),
            (self.subscription_broker.is_some(), GATEWAY_ACCOUNT_KEYS[3]),
            (self.provider.is_some(), GATEWAY_ACCOUNT_KEYS[4]),
        ] {
            if there {
                present.push(key);
            }
        }
        present
    }

    /// This entry as the gateway's own `[accounts.<name>]` table.
    pub fn to_account(&self) -> AccountEntry {
        let mut entry = AccountEntry::default();
        entry
            .set_kind(self.kind)
            .set_vendor(self.vendor)
            .set_credential(self.credential.clone())
            .set_subscription_broker(self.subscription_broker)
            .set_provider(self.provider.clone());
        entry
    }
}

/// A map of configured entitlements, keyed by name — `[entitlements.<name>]`.
///
/// Configuration, never a credential store: see [`EntitlementConfig`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntitlementTable(BTreeMap<String, EntitlementConfig>);
impl EntitlementTable {
    pub fn get(&self, name: &str) -> Option<&EntitlementConfig> {
        self.0.get(name)
    }

    pub fn set(&mut self, name: impl Into<String>, config: EntitlementConfig) {
        self.0.insert(name.into(), config);
    }

    pub fn remove(&mut self, name: &str) -> Option<EntitlementConfig> {
        self.0.remove(name)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &EntitlementConfig)> {
        self.0.iter().map(|(name, cfg)| (name.as_str(), cfg))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}
/// What a resolved entitlement is backed by — the resource it stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntitlementBacking {
    /// A harness's own first-party sign-in
    /// ([`crate::profile::BackendResource::Native`] on that harness).
    NativeHarness(IntegrationId),
    /// The account behind a configured `[providers.<name>]` entry
    /// ([`crate::profile::BackendResource::DirectProvider`] naming it).
    Provider(String),
    /// A subscription account served through a Glasshouse-managed local
    /// broker. `native_harness` is an additional route to this same account,
    /// not a second backing or a second entitlement.
    SubscriptionBroker {
        broker: SubscriptionBroker,
        native_harness: Option<IntegrationId>,
    },
    /// The entry names neither. Listed, never matched, never charged.
    Unstated,
}
impl EntitlementBacking {
    pub fn subscription_broker(&self) -> Option<SubscriptionBroker> {
        match self {
            Self::SubscriptionBroker { broker, .. } => Some(*broker),
            _ => None,
        }
    }

    pub fn native_harness(&self) -> Option<IntegrationId> {
        match self {
            Self::NativeHarness(harness) => Some(*harness),
            Self::SubscriptionBroker { native_harness, .. } => *native_harness,
            _ => None,
        }
    }

    pub fn matches_native_harness(&self, harness: IntegrationId) -> bool {
        self.native_harness() == Some(harness)
    }

    /// What pays for this account, as the **router** may branch on it — map
    /// line 1970's *"subscription to subscription to API credits"*, and the
    /// user's ruling of 2026-08-31: *"A api key or a subscription isn't that
    /// the distinction?"* It is, and the distinction is already structural
    /// here rather than a field somebody typed:
    /// [`Self::NativeHarness`] authenticates **through the harness**, and
    /// [`Self::SubscriptionBroker`] through its private local sidecar; both
    /// are subscriptions. [`Self::Provider`] carries a credential of its own,
    /// which is an API key. The loader **enforces** the separation —
    /// an entry that is both is refused as
    /// [`EntitlementLookupError::NativeSignInWithOwnCredential`], map line
    /// 1973's isolation rule — so nothing here is a guess.
    ///
    /// This is why [`EntitlementKind`]'s invariant survives Phase 56A step
    /// 5 intact: routing branches on the *backing*, never on the *kind*.
    pub fn source(&self) -> crate::routing::EntitlementSource {
        match self {
            Self::NativeHarness(_) | Self::SubscriptionBroker { .. } => {
                crate::routing::EntitlementSource::Subscription
            }
            Self::Provider(_) => crate::routing::EntitlementSource::ApiCredits,
            Self::Unstated => crate::routing::EntitlementSource::Unstated,
        }
    }
}
/// An entitlement as configuration resolved it — [`EntitlementConfig`] with
/// its name, its layer, and its rules already turned into the router's
/// [`crate::routing::EntitlementRules`].
///
/// [`Self::to_routing`] is the bridge to the value a
/// `crate::routing::session::Destination` carries, and it drops everything
/// the router does not decide on: the kind, the vendor, the credential
/// reference, the backing and the layer stay here, where the announcement
/// and the launch path that read them live.
///
/// `PartialEq` without `Eq`: the remaining-capacity slot is a score over an
/// `f64` once 56A package 2 populates it.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedEntitlement {
    /// The catalogue half — the account's name, plan, billing vendor and
    /// credential reference, and the telemetry facets read back against it.
    /// Every one of those is a fact about the account itself, so they live
    /// in [`ResolvedAccount`], which names nothing Glasshouse-side; this
    /// struct adds the policy the account is used *under*.
    ///
    /// [`Self::with_telemetry`] is the resolver that fills the facets in,
    /// and it stays here rather than moving with them because what it reads
    /// — the routing evidence ledger — is Glasshouse's own.
    pub(super) account: ResolvedAccount,
    pub(super) backing: EntitlementBacking,
    pub(super) rules: crate::routing::EntitlementRules,
    pub(super) layer: Layer,
    /// Map lines 1244/1245/1246/1250/1251/1254's subscription-headroom
    /// estimate — [`Self::populate_provider_facets`]'s own producer,
    /// [`crate::routing::evidence::estimate_subscription_headroom`]. `None`
    /// is unknown, the same rule as every facet above; **also** `None` once
    /// [`Self::capacity_scope`] is [`TelemetryScope::PerAccount`] — an
    /// authoritative per-account reading is never displaced by an estimate
    /// (56A-3+'s own ground; this build's own gateway cache can never
    /// produce that scope, so the estimate populates in every reachable case
    /// today).
    pub(super) headroom_estimate: Option<crate::routing::evidence::SubscriptionHeadroomEstimate>,
    /// Map line 1252 — the user's own stated correction, read straight from
    /// `[entitlements.<name>] headroom_override` at load time, not touched
    /// by [`Self::populate_provider_facets`]. `None` is "no correction
    /// stated", never "the estimate is confirmed correct".
    pub(super) headroom_override: Option<crate::routing::evidence::HeadroomBand>,
    /// Map line 1255 — `true` when this entry's config asked the
    /// subscription-headroom estimator to stay off. Read only inside
    /// [`Self::populate_provider_facets`], after `capacity`/`reset` are
    /// already populated, so disabling never touches those facets.
    pub(super) disable_headroom_estimate: bool,
    /// Map line 2024 — this entry's own `context_firewall` override, read
    /// straight from `[entitlements.<name>.context_firewall]` at load time.
    /// Carried here rather than looked up again by name later, the same
    /// choice [`Self::headroom_override`] already makes.
    pub(super) context_firewall: firewall::ContextFirewallOverride,
}
impl ResolvedEntitlement {
    /// The catalogue half on its own — what this account is and what
    /// telemetry has read back against it, with none of the policy below.
    /// The value a gateway would be handed.
    pub fn account(&self) -> &ResolvedAccount {
        &self.account
    }

    pub fn name(&self) -> &str {
        self.account.name()
    }

    pub fn kind(&self) -> Option<EntitlementKind> {
        self.account.kind()
    }

    pub fn vendor(&self) -> Option<EntitlementVendor> {
        self.account.vendor()
    }

    /// Map line 2024's explicit override — see the field's own doc.
    pub fn context_firewall(&self) -> &firewall::ContextFirewallOverride {
        &self.context_firewall
    }

    /// The credential reference this account authenticates with, when the
    /// entry states one. Resolved to a value only through a
    /// [`crate::secret::SecretStore`], at the moment of use, by whatever
    /// launches against this account — never here.
    pub fn credential(&self) -> Option<&SecretRef> {
        self.account.credential()
    }

    /// Remaining capacity, when telemetry has read one — `None` until
    /// [`Self::with_telemetry`] runs, and `None` thereafter for an
    /// entitlement whose provider exposes nothing. Unknown, never
    /// fabricated.
    pub fn remaining_capacity(&self) -> Option<&crate::provider::quota::RemainingCapacityScore> {
        self.account.remaining_capacity()
    }

    /// Seconds until this account's allowance resets, when telemetry has
    /// read one — the same contract as [`Self::remaining_capacity`].
    pub fn seconds_until_reset(&self) -> Option<i64> {
        self.account.seconds_until_reset()
    }

    /// Whose reading the capacity and reset slots carry — `Some` exactly
    /// when either slot is populated, and [`TelemetryScope::ProviderWide`]
    /// for every reading this build can take: the gateway's quota cache is
    /// keyed by provider, so both entitlements of one provider share it.
    pub fn capacity_scope(&self) -> Option<TelemetryScope> {
        self.account.capacity_scope()
    }

    /// Map line 1965's recent-throttling facet — `None` means *unknown*
    /// (nothing consulted the ledger for this entry), never "none observed".
    pub fn throttling(&self) -> Option<&EntitlementThrottleReading> {
        self.account.throttling()
    }

    /// Map line 1965's models facet — `None` means *unknown*.
    pub fn models(&self) -> Option<&EntitlementModels> {
        self.account.models()
    }

    /// Map line 1971's observed-spend facet — `None` means *unknown*
    /// (nothing consulted the ledger, or no row it holds carried a token
    /// count), never "nothing spent".
    pub fn spend(&self) -> Option<&EntitlementSpendReading> {
        self.account.spend()
    }

    /// Map lines 1244/1245/1246/1250/1251/1254's subscription-headroom
    /// estimate — `None` is unknown (nothing consulted the ledger, or
    /// nothing at all was available to estimate from), and also `None`
    /// whenever [`Self::capacity_scope`] already reads
    /// [`TelemetryScope::PerAccount`]: an authoritative per-account reading
    /// is never displaced by an estimate. See
    /// [`crate::routing::evidence::estimate_subscription_headroom`] for what
    /// this reads and [`crate::routing::evidence::SubscriptionHeadroomEstimate`]
    /// for why it is never a bare number.
    pub fn headroom_estimate(
        &self,
    ) -> Option<&crate::routing::evidence::SubscriptionHeadroomEstimate> {
        self.headroom_estimate.as_ref()
    }

    /// Map line 1252's user override — `[entitlements.<name>]
    /// headroom_override`, read at load time. Authoritative over
    /// [`Self::headroom_estimate`] at the one consumer,
    /// `main.rs::entitlement_facets`, but this accessor hands both back
    /// unmixed so a caller decides how to combine them rather than this
    /// type silently doing it.
    pub fn headroom_override(&self) -> Option<crate::routing::evidence::HeadroomBand> {
        self.headroom_override
    }

    /// This account's key in the ledger's `quota_context` column — the
    /// [`crate::routing::CredentialId::label`] the gateway stamps on every
    /// exchange it forwards for this credential. `None` for an entry with no
    /// credential of its own or no provider backing: such an account has no
    /// per-account rows to be narrowed to.
    pub fn credential_label(&self) -> Option<String> {
        let EntitlementBacking::Provider(provider) = &self.backing else {
            return None;
        };
        self.account
            .credential
            .as_ref()
            .map(|reference| crate::routing::CredentialId::new(provider, reference.clone()).label())
    }

    /// Map line 1965's producer — populate the four telemetry facets from
    /// what `telemetry` actually holds, each reading carrying its scope.
    /// Capacity and reset are [`TelemetryScope::ProviderWide`] for a
    /// remote-provider backing (the gateway's cache is keyed by provider, so
    /// a reading cannot be narrowed to one credential) and skipped outright
    /// for local inference, which has no account allowance to read.
    /// Throttling narrows to this account when every ledger row names one,
    /// provider-wide otherwise. Models come from the provider's own declared
    /// catalogue, except a native sign-in is
    /// [`EntitlementModels::HarnessDecided`] — Glasshouse does not know the
    /// plan's models, so none are invented.
    ///
    /// Every facet a source cannot answer stays `None` — unknown, never
    /// full, never empty, never zero-observed.
    ///
    /// History: design-decisions.md, "Trims: config, checkpoint, evaluation and codex module docs", entitlement.rs `with_telemetry`.
    pub fn with_telemetry(mut self, telemetry: &EntitlementTelemetry<'_>) -> Self {
        match self.backing.clone() {
            EntitlementBacking::Provider(provider) => {
                self.populate_provider_facets(&provider, telemetry);
            }
            EntitlementBacking::NativeHarness(_) => {
                self.account.models = Some(EntitlementModels::HarnessDecided);
            }
            EntitlementBacking::SubscriptionBroker { native_harness, .. } => {
                // Broker telemetry is account-scoped by entitlement name. A
                // native route can additionally let the harness decide its
                // model when no broker catalogue has been observed yet.
                let telemetry_key = self.account.name.clone();
                self.populate_provider_facets(&telemetry_key, telemetry);
                if self.account.models.is_none() && native_harness.is_some() {
                    self.account.models = Some(EntitlementModels::HarnessDecided);
                }
            }
            EntitlementBacking::Unstated => {}
        }
        self
    }

    fn populate_provider_facets(&mut self, provider: &str, telemetry: &EntitlementTelemetry<'_>) {
        use crate::provider::registry::{Locality, ResourceKind};

        let kind = ResourceKind::from_direct_provider(provider);
        if kind.locality() == Locality::Remote
            && let Some(cache) = telemetry.gateway_quota
            && let Some((headers, observed_at_unix)) = cache.load(provider)
        {
            let state = headers.apply_to(
                crate::provider::quota::CapacityState::for_resource(&kind),
                observed_at_unix,
            );
            self.account.remaining_capacity = state.remaining_capacity_score();
            self.account.seconds_until_reset = state.seconds_until_reset(telemetry.now_unix);
            if self.account.remaining_capacity.is_some()
                || self.account.seconds_until_reset.is_some()
            {
                self.account.capacity_scope = Some(TelemetryScope::ProviderWide);
            }
        }

        if let Some(observations) = telemetry.observations {
            let label = self.credential_label();
            let counted = crate::routing::evidence::recent_credential_throttles(
                observations,
                provider,
                label.as_deref(),
            );
            self.account.throttling = Some(EntitlementThrottleReading::new(
                counted.throttled,
                if counted.account_narrowed {
                    TelemetryScope::PerAccount
                } else {
                    TelemetryScope::ProviderWide
                },
            ));
        }

        if let Some(observations) = telemetry.observations {
            // Map line 1971's spend half, read from the same rows and
            // narrowed by the same rule as the throttle facet above — one
            // ledger pass' worth of arithmetic, and `None` when no row
            // carried a count, which is what keeps a stated ceiling from
            // being judged reached by a build that measured nothing.
            let label = self.credential_label();
            let counted = crate::routing::evidence::recent_credential_spend(
                observations,
                provider,
                label.as_deref(),
            );
            self.account.spend = counted.tokens.map(|tokens| {
                EntitlementSpendReading::new(
                    tokens,
                    if counted.account_narrowed {
                        TelemetryScope::PerAccount
                    } else {
                        TelemetryScope::ProviderWide
                    },
                )
            });
        }

        // Map lines 1244/1245/1246/1250/1251/1254 — the subscription
        // headroom estimator. Guarded on `capacity_scope`, not skipped
        // outright: this build's gateway cache can only ever narrow capacity
        // to `TelemetryScope::ProviderWide` (56A-2's own recorded limit), so
        // this populates in every reachable case today, exactly the
        // "resolver populates the per-account capacity facet from the
        // estimator where the provider-wide reading is all headers gave"
        // the packet asks for — and the moment a future per-account reading
        // exists (56A-3+), this stays inert rather than displacing it.
        //
        // Map line 1255 sits in front of that guard, not behind it: a
        // disabled entitlement leaves `headroom_estimate` at its default
        // `None` and never calls the estimator at all — `capacity`, `reset`,
        // `throttling` and `models` above are already populated by the time
        // this runs, so disabling touches nothing but this one facet.
        if self.disable_headroom_estimate {
            self.headroom_estimate = None;
        } else if self.account.capacity_scope != Some(TelemetryScope::PerAccount) {
            let label = self.credential_label();
            let session_count = telemetry
                .session_counts
                .and_then(|counts| counts.get(self.account.name.as_str()))
                .copied();
            // Map line 1247's reachable half: re-calibrating the estimator
            // when the quota regime changes is one floor at this, its only
            // caller. `regime_changed_at` reads the same on-disk reading
            // `capacity_scope` above already loaded through `cache.load`;
            // `None` means no change has ever been recorded, in which case
            // every row in the window is still evidence and the filter
            // below is a no-op — `filter`, not `filter_map`, so a `None`
            // floor keeps every row rather than dropping them all.
            let regime_changed_at = telemetry
                .gateway_quota
                .and_then(|cache| cache.regime_changed_at(provider));
            let floored: Vec<crate::routing::evidence::RoutingObservation>;
            let scoped_observations: &[crate::routing::evidence::RoutingObservation] =
                match (telemetry.observations, regime_changed_at) {
                    (Some(observations), Some(floor)) => {
                        floored = observations
                            .iter()
                            .filter(|row| row.observed_at_unix >= floor)
                            .cloned()
                            .collect();
                        &floored
                    }
                    (Some(observations), None) => observations,
                    (None, _) => &[],
                };
            self.headroom_estimate = crate::routing::evidence::estimate_subscription_headroom(
                scoped_observations,
                provider,
                label.as_deref(),
                telemetry.now_unix,
                self.account.seconds_until_reset,
                session_count,
            )
            .map(|mut estimate| {
                estimate.since_unix = regime_changed_at;
                estimate
            });
        }

        if let Some(catalogues) = telemetry.model_catalogues {
            self.account.models =
                catalogues
                    .load(provider)
                    .map(|catalogue| EntitlementModels::Declared {
                        models: catalogue
                            .models()
                            .iter()
                            .map(|model| model.id().to_owned())
                            .collect(),
                        scope: TelemetryScope::ProviderWide,
                    });
        }
    }

    pub fn backing(&self) -> &EntitlementBacking {
        &self.backing
    }

    pub fn rules(&self) -> &crate::routing::EntitlementRules {
        &self.rules
    }

    pub fn layer(&self) -> Layer {
        self.layer
    }

    /// The router's view: name, rules, and whether a user or project
    /// actually wrote the entry — the synthesised harness-default carries
    /// `configured = false`, which is what keeps the router's pool terms
    /// inert for a user who configured nothing (56A step 3's preservation
    /// clause). The 56A-2 telemetry facets are attached by the caller that
    /// resolved them (`main.rs::routing_entitlement`), because the band they
    /// carry is derived against the user's own thresholds, which this
    /// method does not hold.
    pub fn to_routing(&self) -> crate::routing::Entitlement {
        crate::routing::Entitlement::new(self.account.name.clone(), self.rules.clone())
            .with_configured(self.layer != Layer::Default)
            // Map line 1970's work item 1, and the two facets that need no
            // threshold to derive, so they are carried **here** rather than
            // by the caller: the backing discriminant is structural (see
            // [`EntitlementBacking::source`]) and the spend reading is a
            // raw token count. The four 56A-2 facets stay with the caller
            // for the reason above — a band is derived against the user's
            // own thresholds, which this method does not hold.
            .with_source(self.backing.source())
            .with_spend(self.account.spend.map(|reading| {
                crate::routing::EntitlementSpendFacet::new(
                    reading.tokens(),
                    reading.scope() == TelemetryScope::PerAccount,
                )
            }))
            // The headroom estimate needs no threshold either — unlike the
            // capacity band, [`Self::headroom_estimate`] is already a
            // finished value the moment telemetry ran.
            .with_headroom_estimate(self.headroom_estimate)
    }

    /// What the announcement says inside the parentheses after the name:
    /// the plan when one was stated, the billing vendor when one was stated,
    /// and what backs it. Never a credential — a harness's display name, a
    /// provider's *name*, a vendor's spelling.
    pub fn describe(&self) -> String {
        let backing = match &self.backing {
            EntitlementBacking::NativeHarness(harness) => {
                format!("{}'s own sign-in", harness.display_name())
            }
            EntitlementBacking::Provider(provider) => format!("behind provider `{provider}`"),
            EntitlementBacking::SubscriptionBroker {
                broker,
                native_harness,
            } => match native_harness {
                Some(harness) => format!(
                    "through subscription broker `{}` or {}'s own sign-in",
                    broker.as_str(),
                    harness.display_name()
                ),
                None => format!("through subscription broker `{}`", broker.as_str()),
            },
            EntitlementBacking::Unstated => "no backing stated".to_owned(),
        };
        let mut parts = Vec::new();
        if let Some(kind) = self.account.kind {
            parts.push(kind.describe().to_owned());
        }
        if let Some(vendor) = self.account.vendor {
            parts.push(format!("vendor `{}`", vendor.as_str()));
        }
        parts.push(backing);
        parts.join(", ")
    }
}
/// The telemetry sources [`ResolvedEntitlement::with_telemetry`] reads —
/// each one optional and each one already opened or loaded by the caller,
/// so this resolver performs no I/O beyond the caches' own fail-soft file
/// reads: never a probe, never a network call, never a database write
/// (design-decisions §56A step 2's Cluster E discipline). A source left
/// unset leaves its facets `None` — unknown — rather than fabricating an
/// observation nothing took.
pub struct EntitlementTelemetry<'a> {
    gateway_quota: Option<&'a crate::provider::telemetry::GatewayQuotaCache>,
    model_catalogues: Option<&'a crate::provider::cache::ModelCache>,
    /// The evidence window's rows, when the caller read them —
    /// `None` keeps the throttling facet unknown, because "none observed"
    /// may only be said by a resolver that actually looked.
    observations: Option<&'a [crate::routing::evidence::RoutingObservation]>,
    /// Map line 1245's "historical sessions" input to the headroom
    /// estimator: how many of this project's own sessions
    /// (`sessions.entitlement`, migration 22) were charged to each
    /// entitlement, keyed by entitlement **name** — not by provider, unlike
    /// every other source here, because a session names the account that
    /// served it directly. `None` leaves the estimator without this input,
    /// exactly like every other absent source.
    session_counts: Option<&'a std::collections::BTreeMap<String, usize>>,
    now_unix: i64,
}
impl<'a> EntitlementTelemetry<'a> {
    /// No sources at all — every facet stays unknown until a `with_*`
    /// supplies one.
    pub fn new(now_unix: i64) -> Self {
        Self {
            gateway_quota: None,
            model_catalogues: None,
            observations: None,
            session_counts: None,
            now_unix,
        }
    }

    /// The gateway-captured per-provider rate-limit readings.
    pub fn with_gateway_quota(
        mut self,
        cache: &'a crate::provider::telemetry::GatewayQuotaCache,
    ) -> Self {
        self.gateway_quota = Some(cache);
        self
    }

    /// The fetched provider model catalogues.
    pub fn with_model_catalogues(mut self, cache: &'a crate::provider::cache::ModelCache) -> Self {
        self.model_catalogues = Some(cache);
        self
    }

    /// The evidence window's observation rows, read from the project's
    /// ledger by the caller.
    pub fn with_observations(
        mut self,
        observations: &'a [crate::routing::evidence::RoutingObservation],
    ) -> Self {
        self.observations = Some(observations);
        self
    }

    /// How many of this project's own sessions were charged to each
    /// entitlement, keyed by entitlement name — map line 1245's "historical
    /// sessions" input to the subscription-headroom estimator.
    pub fn with_session_counts(
        mut self,
        counts: &'a std::collections::BTreeMap<String, usize>,
    ) -> Self {
        self.session_counts = Some(counts);
        self
    }
}
/// Why the `[entitlements]` tables could not be resolved. Each is a
/// contradiction only the two layers together can show, so none is a
/// deserialisation error; each is refused rather than resolved by guessing.
#[derive(Debug, thiserror::Error)]
pub enum EntitlementLookupError {
    #[error(
        "entitlement `{name}` names `provider` together with a subscription path; an entitlement \
         is a subscription account or the API-credit account behind a provider, not both"
    )]
    TwoBackings { name: String },
    #[error(
        "entitlement `{name}` takes the name reserved for {}'s own sign-in without being it; \
         set `native_harness = \"{name}\"` on it or rename it",
        .harness.display_name()
    )]
    NameReservedForHarness {
        name: String,
        harness: IntegrationId,
    },
    #[error(
        "entitlements {} all claim to be {}'s own sign-in; a harness signs in to one account, \
         so keep one",
        .names.join(", "), .harness.display_name()
    )]
    AmbiguousNativeHarness {
        harness: IntegrationId,
        names: Vec<String>,
    },
    #[error(
        "entitlements {} all claim provider `{provider}`; a configured provider is one account, \
         so keep one",
        .names.join(", ")
    )]
    AmbiguousProvider {
        provider: String,
        names: Vec<String>,
    },
    #[error(
        "entitlement `{name}` claims to be a harness's own sign-in and names its own \
         `credential`; a harness authenticates its own sign-in itself, and an entitlement with \
         its own credential is a separate account — drop one of the two"
    )]
    NativeSignInWithOwnCredential { name: String },
    #[error(
        "entitlement `{name}` names `subscription_broker` and its own `credential`; broker \
         authentication belongs in the broker's private account store, and configuration may \
         contain only the broker reference"
    )]
    SubscriptionBrokerWithOwnCredential { name: String },
    /// A `[entitlements.<name>]` overlay states policy for an account the
    /// gateway's catalogue does not hold, and does not name a harness's own
    /// sign-in either — so there is nothing for it to be policy *about*.
    ///
    /// Refused rather than dropped: a rule the user believes is in force and
    /// that matches nothing is the silent kind of wrong this project keeps
    /// paying for. The message names both files, because the fix is in one
    /// of them and the reader cannot tell which without being told what the
    /// other holds.
    #[error(
        "`[entitlements.{name}]` states policy for an account the gateway does not have. \
         Accounts live in `{gateway_path}`, which configures {}; an overlay may also stand \
         alone when it names `native_harness`, which this one does not. Connect the account \
         with `glasshouse subscriptions connect`, add it to that file, or remove the overlay",
        if .known.is_empty() { "no accounts".to_owned() } else { .known.join(", ") }
    )]
    UnknownAccount {
        name: String,
        /// The gateway configuration file this catalogue was read from.
        gateway_path: String,
        /// The account names the gateway does have.
        known: Vec<String>,
    },

    #[error(
        "entitlements {} all name the same credential ({reference}); one credential is one \
         account, and map line 1963 gives each entitlement its own — give each entry its own \
         reference",
        .names.join(", ")
    )]
    SharedCredential {
        names: Vec<String>,
        /// The reference's *names* — a variable name, or a service and
        /// account — never a value.
        reference: String,
    },
}

#[cfg(test)]
mod overlay_tests {
    use super::*;

    /// An account's own keys parse as the gateway's.
    fn account(text: &str) -> AccountEntry {
        toml::from_str(text).expect("the gateway's `[accounts.<name>]` shape parses")
    }

    /// **Each of the five account keys is refused by name, and the refusal
    /// names the command that moves it.** The user ruling of 2026-09-11 in
    /// one test: a `[entitlements.<name>]` table may state policy and
    /// nothing about the account itself.
    #[test]
    fn every_account_key_in_an_entitlements_table_is_refused_by_name() {
        for (key, line) in [
            ("kind", "kind = \"claude\"\n"),
            ("vendor", "vendor = \"claude\"\n"),
            ("credential", "credential = { env = \"CLAUDE_A_TOKEN\" }\n"),
            (
                "subscription_broker",
                "subscription_broker = \"cliproxyapi\"\n",
            ),
            ("provider", "provider = \"openrouter\"\n"),
        ] {
            let error = toml::from_str::<EntitlementConfig>(line)
                .expect_err("an account key is the gateway's");
            let rendered = error.message().to_owned();
            assert!(rendered.contains(key), "the key is named: {rendered}");
            assert!(
                rendered.contains(crate::config::MIGRATE_COMMAND),
                "the refusal names the command that moves it: {rendered}"
            );
            assert!(
                rendered.contains("gateway.toml"),
                "and where it goes: {rendered}"
            );
        }
        // And the refusal never repeats what was written — a credential
        // reference is one of the five.
        let error = toml::from_str::<EntitlementConfig>(
            "credential = { env = \"A_VERY_DISTINCTIVE_NAME\" }\n",
        )
        .expect_err("refused");
        assert!(
            !error.message().contains("A_VERY_DISTINCTIVE_NAME"),
            "{}",
            error.message()
        );
    }

    /// The policy half still parses and still serialises, and what it
    /// serialises is policy only.
    #[test]
    fn the_overlay_parses_and_serialises_policy_only() {
        let config: EntitlementConfig = toml::from_str(
            "native_harness = \"codex\"\ndeny_harnesses = [\"pane\"]\n\
             spend_ceiling_tokens = 42\nheadroom_override = \"low\"\n",
        )
        .expect("the policy shape parses");
        assert_eq!(config.native_harness(), Some(IntegrationId::Codex));
        assert_eq!(config.spend_ceiling_tokens(), Some(42));

        let written = toml::to_string(&config).expect("the overlay serialises");
        assert!(written.contains("native_harness = \"codex\""), "{written}");
        for account_key in GATEWAY_ACCOUNT_KEYS {
            assert!(
                !written.contains(account_key),
                "`{account_key}` is the gateway's and must not be written here:\n{written}"
            );
        }
    }

    /// A broker account and a native route are one subscription; a broker
    /// account that also names a provider, or a credential, is refused —
    /// and every one of those facts now comes from the **gateway's**
    /// account, not from Glasshouse's table.
    #[test]
    fn broker_and_native_are_one_subscription_while_provider_or_credentials_are_refused() {
        let broker = account("subscription_broker = \"cliproxyapi\"\n");
        let dual: EntitlementConfig = toml::from_str("native_harness = \"codex\"\n").unwrap();
        let resolved = dual.to_resolved("chatgpt-a", &broker, Layer::User).unwrap();
        assert_eq!(
            resolved.backing().source(),
            crate::routing::EntitlementSource::Subscription
        );
        assert!(
            resolved
                .backing()
                .matches_native_harness(IntegrationId::Codex)
        );
        assert_eq!(
            resolved.backing().subscription_broker(),
            Some(SubscriptionBroker::CliProxyApi)
        );

        let both = account("subscription_broker = \"cliproxyapi\"\nprovider = \"openai\"\n");
        for overlay in ["", "native_harness = \"codex\"\n"] {
            let config: EntitlementConfig = toml::from_str(overlay).unwrap();
            assert!(matches!(
                config.to_resolved("mixed", &both, Layer::User),
                Err(EntitlementLookupError::TwoBackings { .. })
            ));
        }

        let with_credential = account(
            "subscription_broker = \"cliproxyapi\"\ncredential = { env = \"BROKER_TOKEN\" }\n",
        );
        let bare: EntitlementConfig = toml::from_str("").unwrap();
        assert!(matches!(
            bare.to_resolved("mixed", &with_credential, Layer::User),
            Err(EntitlementLookupError::SubscriptionBrokerWithOwnCredential { .. })
        ));
    }

    /// Layering, rules and account-scoped telemetry all still work — with
    /// the account read from the gateway's catalogue and the rules from
    /// Glasshouse's two layers.
    #[test]
    fn broker_entitlements_keep_layering_rules_and_account_scoped_telemetry_facets() {
        use crate::provider::cache::{ModelCache, ModelCatalogue, ModelEntry};
        use crate::provider::telemetry::{GatewayQuotaCache, RateLimitHeaders};

        let gateway = GatewayCatalogue::from_toml(
            "[accounts.chatgpt-a]\nsubscription_broker = \"cliproxyapi\"\n",
        )
        .expect("the gateway's catalogue parses");
        let user: UserConfig = toml::from_str(
            "version = 1\n\n[entitlements.chatgpt-a]\ndeny_harnesses = [\"pane\"]\n",
        )
        .unwrap();
        let project: ProjectConfig = toml::from_str(
            "version = 1\n\n[entitlements.chatgpt-a]\nnative_harness = \"codex\"\n\
             allow_harnesses = [\"pane\"]\nspend_ceiling_tokens = 42\n",
        )
        .unwrap();
        let effective = EffectiveConfig::with_gateway(&user, Some(&project), &gateway);

        let native = effective
            .entitlement_for(
                IntegrationId::Codex,
                &crate::profile::BackendResource::Native,
            )
            .unwrap()
            .expect("the broker entitlement also owns the native route");
        assert_eq!(native.name(), "chatgpt-a");
        assert_eq!(native.layer(), Layer::Project);
        assert!(native.rules().serves_harness(IntegrationId::Pane));
        assert_eq!(native.rules().spend_ceiling_tokens(), Some(42));

        let temp = tempfile::tempdir().unwrap();
        let quota = GatewayQuotaCache::at(temp.path().join("quota"));
        quota.store(
            "chatgpt-a",
            &RateLimitHeaders::read(vec![
                ("ratelimit-limit", "100"),
                ("ratelimit-remaining", "40"),
                ("ratelimit-reset", "60"),
            ]),
            1_800_000_000,
        );
        let models = ModelCache::at(temp.path().join("models"));
        models
            .store(&ModelCatalogue::new(
                "chatgpt-a",
                "http://127.0.0.1",
                "http://127.0.0.1/v1/models",
                1_800_000_000,
                vec![ModelEntry::new("gpt-account-model")],
            ))
            .unwrap();
        let telemetry = EntitlementTelemetry::new(1_800_000_010)
            .with_gateway_quota(&quota)
            .with_model_catalogues(&models);
        let configured = effective
            .configured_entitlements_with_telemetry(&telemetry)
            .unwrap();
        let account = configured
            .iter()
            .find(|entry| entry.name() == "chatgpt-a")
            .unwrap();
        assert!(account.remaining_capacity().is_some());
        assert_eq!(account.seconds_until_reset(), Some(50));
        assert!(matches!(
            account.models(),
            Some(EntitlementModels::Declared { models, .. })
                if models == &["gpt-account-model".to_owned()]
        ));
    }

    /// **An account the gateway has resolves with no overlay at all**, at
    /// [`Layer::User`] and under the defaults — and an overlay for an
    /// account the gateway does *not* have is refused, naming both files.
    #[test]
    fn an_account_resolves_without_an_overlay_and_an_orphan_overlay_is_refused() {
        let gateway = GatewayCatalogue::from_toml(
            "[accounts.claude-a]\nkind = \"claude\"\nsubscription_broker = \"cliproxyapi\"\n\
             spend_ceiling_tokens = 900\n",
        )
        .expect("parses");
        let empty = UserConfig::default();
        let effective = EffectiveConfig::with_gateway(&empty, None, &gateway);
        let configured = effective.configured_entitlements().expect("resolves");
        let entry = configured
            .iter()
            .find(|entry| entry.name() == "claude-a")
            .expect("a gateway account is an entitlement even with no overlay");
        assert_eq!(entry.layer(), Layer::User);
        assert_eq!(entry.kind(), Some(EntitlementKind::Claude));
        assert_eq!(
            entry.rules().spend_ceiling_tokens(),
            Some(900),
            "the account's own ceiling applies when no overlay states one"
        );

        let orphan: UserConfig = toml::from_str(
            "version = 1\n\n[entitlements.not-an-account]\ndeny_harnesses = [\"pane\"]\n",
        )
        .unwrap();
        let error = EffectiveConfig::with_gateway(&orphan, None, &gateway)
            .entitlements()
            .expect_err("an overlay with no account behind it describes nothing");
        let rendered = error.to_string();
        assert!(rendered.contains("not-an-account"), "{rendered}");
        assert!(rendered.contains("claude-a"), "{rendered}");
        assert!(
            rendered.contains("gateway.toml"),
            "the refusal names the gateway's file: {rendered}"
        );
    }

    /// The one overlay that stands alone: a harness's own sign-in is not an
    /// account the gateway serves, so an entry naming `native_harness`
    /// needs no `[accounts.<name>]` at all.
    #[test]
    fn a_native_sign_in_overlay_needs_no_gateway_account() {
        let gateway = GatewayCatalogue::from_toml("").expect("an empty catalogue");
        let user: UserConfig = toml::from_str(
            "version = 1\n\n[entitlements.my-claude]\nnative_harness = \"claude-code\"\n\
             deny_tiers = [\"frontier\"]\n",
        )
        .unwrap();
        let effective = EffectiveConfig::with_gateway(&user, None, &gateway);
        let entry = effective
            .configured_entitlements()
            .expect("resolves")
            .into_iter()
            .find(|entry| entry.name() == "my-claude")
            .expect("a native sign-in stands alone");
        assert!(
            entry
                .backing()
                .matches_native_harness(IntegrationId::ClaudeCode)
        );
        assert!(entry.credential().is_none());
    }
}
