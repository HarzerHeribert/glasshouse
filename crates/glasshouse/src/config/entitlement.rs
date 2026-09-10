//! Entitlement **policy**: which harnesses and job kinds an account is for,
//! how Glasshouse layers the `[entitlements]` tables into one resolved list,
//! and the facets it derives from its own evidence ledger.
//!
//! The client-neutral half — what an account *is*, how it authenticates, what
//! it can serve — lives in [`super::entitlement_catalogue`] and is re-exported
//! below, so every path that named one of those items here before the split
//! still resolves. The dependency runs one way: policy reads catalogue, and
//! the catalogue names nothing in this file.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::integrations::IntegrationId;
use crate::secret::SecretRef;

use super::*;

pub use super::entitlement_catalogue::{
    AccountEntry, EntitlementCredential, EntitlementKind, EntitlementModels,
    EntitlementSpendReading, EntitlementThrottleReading, EntitlementVendor, ResolvedAccount,
    SubscriptionBroker, TelemetryScope,
};
// Not public, and re-exported at exactly the visibility it had before the
// move: `config::provider`'s `credential_env` names it through `use super::*`.
pub(super) use super::entitlement_catalogue::deserialize_credential_env_names;

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
/// One configured entitlement — a specific subscription or API-credit
/// account, the unit of capacity — as stored in an `[entitlements.<name>]`
/// table. Map lines 1946, 1947, 1962 and 1963. Backed by exactly one of
/// `native_harness` (the harness's own sign-in, no `credential` of its own)
/// or `provider` (the account behind a configured provider); naming both is
/// refused ([`EntitlementLookupError::TwoBackings`]), naming neither makes a
/// pool member ([`EffectiveConfig::entitlement_resources`]) no launch
/// profile charges yet. A `subscription_broker` is a third backing, and may
/// be paired with `native_harness` because those are two routes to one
/// subscription account. A provider remains mutually exclusive with both.
///
/// Sits in a stack of five separately replaceable layers — harness, protocol
/// adapter, authentication (`credential`), this entry, and inference model —
/// owning only authentication and itself, so replacing any other layer
/// leaves this entitlement's capacity where it was: the entitlement, not the
/// vendor or harness, is the unit of capacity.
///
/// Rules resolve through [`crate::routing::EntitlementRules`] and nowhere
/// else — deny wins over allow, and `deny_unknown_fields` keeps an
/// unrecognised rule from being silently read as "no rule".
///
/// History: design-decisions.md, "Trims: config, checkpoint, evaluation and codex module docs", entitlement.rs module doc `EntitlementConfig`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntitlementConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kind: Option<EntitlementKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    vendor: Option<EntitlementVendor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    credential: Option<EntitlementCredential>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    native_harness: Option<ConfiguredHarness>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    subscription_broker: Option<SubscriptionBroker>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider: Option<String>,
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
    pub fn kind(&self) -> Option<EntitlementKind> {
        self.kind
    }

    pub fn set_kind(&mut self, value: Option<EntitlementKind>) -> &mut Self {
        self.kind = value;
        self
    }

    pub fn vendor(&self) -> Option<EntitlementVendor> {
        self.vendor
    }

    pub fn set_vendor(&mut self, value: Option<EntitlementVendor>) -> &mut Self {
        self.vendor = value;
        self
    }

    pub fn credential(&self) -> Option<&EntitlementCredential> {
        self.credential.as_ref()
    }

    pub fn set_credential(&mut self, value: Option<EntitlementCredential>) -> &mut Self {
        self.credential = value;
        self
    }

    pub fn native_harness(&self) -> Option<IntegrationId> {
        self.native_harness.map(ConfiguredHarness::id)
    }

    pub fn set_native_harness(&mut self, value: Option<IntegrationId>) -> &mut Self {
        self.native_harness = value.map(ConfiguredHarness::new);
        self
    }

    pub fn subscription_broker(&self) -> Option<SubscriptionBroker> {
        self.subscription_broker
    }

    pub fn set_subscription_broker(&mut self, value: Option<SubscriptionBroker>) -> &mut Self {
        self.subscription_broker = value;
        self
    }

    pub fn provider(&self) -> Option<&str> {
        self.provider.as_deref()
    }

    pub fn set_provider(&mut self, value: Option<String>) -> &mut Self {
        self.provider = value;
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

    /// This entry's catalogue half — the six keys that say what the account
    /// is and what it can serve, with nothing about who may use it. The
    /// shape a gateway reads from its own configuration file; see
    /// [`AccountEntry`].
    ///
    /// A projection, not a view: the policy table above stays the one thing
    /// `[entitlements.<name>]` deserialises into, so the file's parse
    /// behaviour — `deny_unknown_fields` included — is untouched by the
    /// split.
    pub fn account(&self) -> AccountEntry {
        let mut entry = AccountEntry::default();
        entry
            .set_kind(self.kind)
            .set_vendor(self.vendor)
            .set_credential(self.credential.clone())
            .set_subscription_broker(self.subscription_broker)
            .set_provider(self.provider.clone())
            .set_spend_ceiling_tokens(self.spend_ceiling_tokens);
        entry
    }

    /// The resolved value, named `name` — the key this entry was stored
    /// under — and attributed to `layer`.
    pub fn to_resolved(
        &self,
        name: &str,
        layer: Layer,
    ) -> Result<ResolvedEntitlement, EntitlementLookupError> {
        // The backing is the one decision that reads both halves: the
        // catalogue says which provider or broker serves the account, and
        // `native_harness` — a harness identity, and so policy — says
        // whether the harness's own sign-in is a route to it.
        let account = self.account();
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
        if matches!(backing, EntitlementBacking::NativeHarness(_)) && self.credential.is_some() {
            return Err(EntitlementLookupError::NativeSignInWithOwnCredential {
                name: name.to_owned(),
            });
        }
        if matches!(backing, EntitlementBacking::SubscriptionBroker { .. })
            && self.credential.is_some()
        {
            return Err(
                EntitlementLookupError::SubscriptionBrokerWithOwnCredential {
                    name: name.to_owned(),
                },
            );
        }
        Ok(ResolvedEntitlement {
            account: ResolvedAccount::resolve(name, &account),
            backing,
            rules: self.rules(),
            layer,
            headroom_estimate: None,
            headroom_override: self.headroom_override(),
            disable_headroom_estimate: self.disable_headroom_estimate,
            context_firewall: self.context_firewall.clone(),
        })
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
mod subscription_broker_tests {
    use super::*;

    #[test]
    fn broker_configuration_is_a_typed_reference_and_serializes_no_credentials() {
        let config: EntitlementConfig =
            toml::from_str("subscription_broker = \"cliproxyapi\"\nnative_harness = \"codex\"\n")
                .expect("the supported broker reference parses");
        assert_eq!(
            config.subscription_broker(),
            Some(SubscriptionBroker::CliProxyApi)
        );
        assert_eq!(config.native_harness(), Some(IntegrationId::Codex));

        let written = toml::to_string(&config).expect("broker config serializes");
        assert!(written.contains("subscription_broker = \"cliproxyapi\""));
        assert!(written.contains("native_harness = \"codex\""));
        assert!(!written.contains("credential"), "{written}");
        assert!(!written.contains("token"), "{written}");

        toml::from_str::<EntitlementConfig>("subscription_broker = \"other\"\n")
            .expect_err("unknown broker names are refused at the typed boundary");
        toml::from_str::<EntitlementConfig>(
            "subscription_broker = { name = \"cliproxyapi\", token = \"planted\" }\n",
        )
        .expect_err("a broker reference cannot contain credentials");
    }

    #[test]
    fn broker_and_native_are_one_subscription_while_provider_or_credentials_are_refused() {
        let dual: EntitlementConfig =
            toml::from_str("subscription_broker = \"cliproxyapi\"\nnative_harness = \"codex\"\n")
                .unwrap();
        let resolved = dual.to_resolved("chatgpt-a", Layer::User).unwrap();
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

        for invalid in [
            "subscription_broker = \"cliproxyapi\"\nprovider = \"openai\"\n",
            "subscription_broker = \"cliproxyapi\"\nnative_harness = \"codex\"\nprovider = \"openai\"\n",
        ] {
            let config: EntitlementConfig = toml::from_str(invalid).unwrap();
            assert!(matches!(
                config.to_resolved("mixed", Layer::User),
                Err(EntitlementLookupError::TwoBackings { .. })
            ));
        }

        let with_credential: EntitlementConfig = toml::from_str(
            "subscription_broker = \"cliproxyapi\"\ncredential = { env = \"BROKER_TOKEN\" }\n",
        )
        .unwrap();
        assert!(matches!(
            with_credential.to_resolved("mixed", Layer::User),
            Err(EntitlementLookupError::SubscriptionBrokerWithOwnCredential { .. })
        ));
    }

    #[test]
    fn broker_entitlements_keep_layering_rules_and_account_scoped_telemetry_facets() {
        use crate::provider::cache::{ModelCache, ModelCatalogue, ModelEntry};
        use crate::provider::telemetry::{GatewayQuotaCache, RateLimitHeaders};

        let user: UserConfig = toml::from_str(
            "version = 1\n\n[entitlements.chatgpt-a]\n\
             subscription_broker = \"cliproxyapi\"\ndeny_harnesses = [\"pane\"]\n",
        )
        .unwrap();
        let project: ProjectConfig = toml::from_str(
            "version = 1\n\n[entitlements.chatgpt-a]\n\
             subscription_broker = \"cliproxyapi\"\nnative_harness = \"codex\"\n\
             allow_harnesses = [\"pane\"]\nspend_ceiling_tokens = 42\n",
        )
        .unwrap();
        let effective = EffectiveConfig::new(&user, Some(&project));

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
}

#[cfg(test)]
mod catalogue_projection_tests {
    use super::*;

    /// **The catalogue projection carries every serving fact and no policy.**
    /// [`EntitlementConfig::account`] is hand-written, so a field silently
    /// dropped from it would leave the gateway-side view of an account
    /// missing a key that the Glasshouse-side table plainly states. Written
    /// against a table that sets all six catalogue keys *and* four policy
    /// ones, so the test fails both ways: on a lost serving fact, and on a
    /// harness rule leaking across the boundary.
    #[test]
    fn account_carries_every_serving_fact_and_no_policy() {
        let config: EntitlementConfig = toml::from_str(
            "kind = \"claude\"\nvendor = \"claude\"\n\
             credential = { env = \"CLAUDE_A_TOKEN\" }\n\
             provider = \"alpha-probe\"\nspend_ceiling_tokens = 250000\n\
             allow_harnesses = [\"codex\"]\ndeny_job_kinds = [\"reranking\"]\n\
             headroom_override = \"low\"\ndisable_headroom_estimate = true\n",
        )
        .expect("a table stating both halves parses");

        let account = config.account();
        assert_eq!(account.kind(), Some(EntitlementKind::Claude));
        assert_eq!(account.vendor(), Some(EntitlementVendor::Claude));
        assert_eq!(
            account.credential(),
            Some(&EntitlementCredential::environment("CLAUDE_A_TOKEN"))
        );
        assert_eq!(account.provider(), Some("alpha-probe"));
        assert_eq!(account.subscription_broker(), None);
        assert_eq!(account.spend_ceiling_tokens(), Some(250_000));

        // The written form is the gateway's own file: the six serving keys,
        // and not one of the four policy keys the same table stated.
        let written = toml::to_string(&account).expect("the catalogue entry serialises");
        for serving in [
            "kind",
            "vendor",
            "credential",
            "provider",
            "spend_ceiling_tokens",
        ] {
            assert!(
                written.contains(serving),
                "{serving} missing from:\n{written}"
            );
        }
        for policy in [
            "allow_harnesses",
            "deny_job_kinds",
            "headroom_override",
            "disable_headroom_estimate",
        ] {
            assert!(
                !written.contains(policy),
                "{policy} is policy and must not cross into the catalogue:\n{written}"
            );
        }

        // And the resolved catalogue value is that entry under its name,
        // with every telemetry facet still unknown.
        let resolved = config
            .to_resolved("claude-a", Layer::User)
            .expect("a provider-backed entry with its own credential resolves");
        assert_eq!(resolved.account().name(), "claude-a");
        assert_eq!(resolved.account().kind(), Some(EntitlementKind::Claude));
        assert!(resolved.account().credential().is_some());
        assert!(resolved.account().remaining_capacity().is_none());
        assert!(resolved.account().models().is_none());
        assert!(resolved.account().spend().is_none());
    }
}
