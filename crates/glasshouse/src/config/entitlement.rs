//! Entitlement resolution: which plan an integration/harness runs under, and the tiers, job kinds and headroom it allows.
//!

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::integrations::IntegrationId;
use crate::secret::SecretRef;

use super::*;

// ---------------------------------------------------------------------------
// Phase 56/56A — `[entitlements.<name>]`: an entitlement — a specific
// subscription or API-credit account — as the configured unit of capacity,
// with rules of its own (map lines 1946, 1947, 1954, 1962, 1963, 1973).
// ---------------------------------------------------------------------------
/// Which plan an entitlement is — map line 1946's four: *"a Claude,
/// ChatGPT/Codex, or Gemini plan, or an API key"*.
///
/// Descriptive, and read by exactly one consumer: the launch announcement
/// that says which entitlement will serve a session. No rule depends on it —
/// [`EntitlementConfig`]'s rules are about harnesses, tiers and job kinds,
/// never about what kind of plan is paying — so a wrong `kind` misdescribes
/// an entitlement and never misroutes one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EntitlementKind {
    Claude,
    #[serde(rename = "chatgpt")]
    ChatGpt,
    Gemini,
    ApiKey,
}
impl EntitlementKind {
    /// The spelling a configuration file uses.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::ChatGpt => "chatgpt",
            Self::Gemini => "gemini",
            Self::ApiKey => "api-key",
        }
    }

    /// How the announcement names the plan.
    pub fn describe(self) -> &'static str {
        match self {
            Self::Claude => "Claude plan",
            Self::ChatGpt => "ChatGPT plan",
            Self::Gemini => "Gemini plan",
            Self::ApiKey => "API key",
        }
    }
}
/// The billing vendor behind an entitlement — map line 1962's *"distinct
/// from the vendor"*: the account that pays is one fact, who bills it is
/// another, and two entitlements of one vendor are still two accounts.
///
/// Descriptive, like [`EntitlementKind`], and read by the same one consumer:
/// the launch announcement ([`ResolvedEntitlement::describe`]). **No rule and
/// no resolution step keys on it** — map line 1963's coexistence is the point,
/// and nothing anywhere dedupes entitlements by vendor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EntitlementVendor {
    Claude,
    #[serde(rename = "openai")]
    OpenAi,
    Google,
    #[serde(rename = "openrouter")]
    OpenRouter,
    /// Any vendor the four names above do not cover — a self-hosted router,
    /// a reseller, an employer's own gateway.
    Custom,
}
impl EntitlementVendor {
    /// The spelling a configuration file uses.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::OpenAi => "openai",
            Self::Google => "google",
            Self::OpenRouter => "openrouter",
            Self::Custom => "custom",
        }
    }
}
/// An entitlement's own authentication — map lines 1962 and 1973: **a
/// reference, never a value**, in exactly [`crate::secret::SecretRef`]'s two
/// shapes.
///
/// ```toml
/// credential = { env = "CLAUDE_A_OAUTH_TOKEN" }            # an environment variable NAME
/// credential = { service = "glasshouse", account = "a" }   # an OS-credential reference
/// ```
///
/// The `Deserialize` impl is manual so that nothing else can ever parse: a
/// bare string is refused with a sentence naming the rule — and deliberately
/// **without echoing what was written**, because the one thing a value-shaped
/// mistake must not do is copy the value into an error message — and a map
/// carrying any other key (`value`, `token`, `key`, …) is refused by that
/// key's name. This is the config-file side of Phase 9E's boundary; the
/// serde impls live here and not on [`SecretRef`] itself because
/// `crate::secret`'s own tests hold that module to naming no serde at all.
#[derive(Clone, PartialEq, Eq)]
pub struct EntitlementCredential(SecretRef);
impl EntitlementCredential {
    pub fn environment(var: impl Into<String>) -> Self {
        Self(SecretRef::Environment { var: var.into() })
    }

    pub fn os_credential(service: impl Into<String>, account: impl Into<String>) -> Self {
        Self(SecretRef::OsCredential {
            service: service.into(),
            account: account.into(),
        })
    }

    /// The reference this credential names. A caller resolves it through a
    /// [`crate::secret::SecretStore`] at the moment of use, never earlier.
    pub fn secret_ref(&self) -> &SecretRef {
        &self.0
    }
}
/// Names only — the variable's, the service's, the account's — exactly what
/// [`SecretRef`]'s own `Debug` prints. Manual so the shape is pinned by
/// `tests/entitlement_pool.rs` rather than drifting with a derive.
impl std::fmt::Debug for EntitlementCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            SecretRef::Environment { var } => write!(f, "environment variable `{var}`"),
            SecretRef::OsCredential { service, account } => {
                write!(f, "OS credential `{service}`/`{account}`")
            }
        }
    }
}
/// The sentence every value-shaped mistake gets. One spelling, no echo.
const CREDENTIAL_IS_A_REFERENCE: &str = "an entitlement credential is a reference, never a value: \
     write `credential = { env = \"VAR_NAME\" }` for an environment variable, or `credential = \
     { service = \"...\", account = \"...\" }` for the operating system's credential store. A \
     secret does not belong in a configuration file, so what was written here is not repeated";
/// The sentence a value pasted into the `env` slot gets — the same mistake
/// as a bare string, one nesting level deeper, and refused the same way:
/// by the rule's name, never by repeating what was written.
const ENV_NAME_IS_NOT_A_VALUE: &str = "an entitlement credential's `env` is the NAME of an \
     environment variable, not its value: a name may use letters, digits and `_` only and may \
     not start with a digit, and what was written here is neither a name nor repeated";
/// Whether `name` can be an environment variable name at all.
///
/// The portable (POSIX) character set, deliberately narrower than what
/// `std::env::var_os` would accept: every credential shape
/// [`crate::secret::redact`] knows about — `sk-`, `sk-or-v1-`, `ghp_` with
/// its dots, a JWT's `.` and `=` — carries a character this refuses, so a
/// value pasted where a name belongs is caught by shape rather than by
/// guessing at prefixes.
fn is_environment_variable_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}
/// Deserializes [`ProviderConfig::credential_env`], refusing any entry that
/// cannot be an environment variable name — the same shape check
/// [`EntitlementCredential`]'s `env` applies, and the same hole: this field
/// is documented as "names only — never a value" but nothing enforced it, so
/// a pasted key would be stored verbatim and later copied wherever this list
/// is rendered.
pub(super) fn deserialize_credential_env_names<'de, D>(
    deserializer: D,
) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error as _;
    let names: Vec<String> = Vec::deserialize(deserializer)?;
    for name in &names {
        if !is_environment_variable_name(name) {
            return Err(D::Error::custom(ENV_NAME_IS_NOT_A_VALUE));
        }
    }
    Ok(names)
}
impl Serialize for EntitlementCredential {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        match &self.0 {
            SecretRef::Environment { var } => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("env", var)?;
                map.end()
            }
            SecretRef::OsCredential { service, account } => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("service", service)?;
                map.serialize_entry("account", account)?;
                map.end()
            }
        }
    }
}
impl<'de> Deserialize<'de> for EntitlementCredential {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ReferenceOnly;

        impl<'de> serde::de::Visitor<'de> for ReferenceOnly {
            type Value = EntitlementCredential;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(CREDENTIAL_IS_A_REFERENCE)
            }

            // Every non-map shape lands in one of these, and none of them
            // repeats what it was handed.
            fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<Self::Value, E> {
                Err(E::custom(CREDENTIAL_IS_A_REFERENCE))
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut access: A,
            ) -> Result<Self::Value, A::Error> {
                use serde::de::Error as _;
                let mut env: Option<String> = None;
                let mut service: Option<String> = None;
                let mut account: Option<String> = None;
                while let Some(key) = access.next_key::<String>()? {
                    match key.as_str() {
                        "env" => env = Some(access.next_value()?),
                        "service" => service = Some(access.next_value()?),
                        "account" => account = Some(access.next_value()?),
                        other => {
                            return Err(A::Error::custom(format!(
                                "an entitlement credential does not take a key named \
                                 `{other}` — {CREDENTIAL_IS_A_REFERENCE}"
                            )));
                        }
                    }
                }
                match (env, service, account) {
                    (Some(var), None, None) => {
                        if !is_environment_variable_name(&var) {
                            return Err(A::Error::custom(ENV_NAME_IS_NOT_A_VALUE));
                        }
                        Ok(EntitlementCredential::environment(var))
                    }
                    (None, Some(service), Some(account)) => {
                        Ok(EntitlementCredential::os_credential(service, account))
                    }
                    (Some(_), _, _) => Err(A::Error::custom(
                        "an entitlement credential names `env` alone, or `service` and \
                         `account` together — not both shapes at once",
                    )),
                    (None, _, _) => Err(A::Error::custom(
                        "an entitlement credential names `env` alone, or `service` and \
                         `account` together",
                    )),
                }
            }
        }

        deserializer.deserialize_any(ReferenceOnly)
    }
}
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
/// table. Map lines 1946, 1947, 1962 and 1963.
///
/// The entitlement's *name* is its key in [`EntitlementTable`], as a
/// provider's is in [`ProviderTable`]. What it is (`kind`), who bills it
/// (`vendor`), what backs it (`native_harness` **or** `provider`, never
/// both), its own authentication (`credential` — a **reference**, never a
/// value), and six rule lists in three allow/deny pairs.
///
/// # The five layers, and which field is which (map line 1964)
///
/// An entitlement sits in a stack of five separately replaceable layers,
/// and this entry deliberately owns only its own two:
///
/// 1. **harness** — [`IntegrationId`], chosen per launch profile
///    ([`crate::profile::LaunchProfile::harness`]); an entitlement's rules
///    may refuse one, but the choice is the user's.
/// 2. **protocol adapter** — [`crate::harness::WireProtocol`], declared by
///    the provider template a backing names, never by this entry.
/// 3. **authentication** — the `credential` reference on this entry: which
///    key or token proves the account. Two entries of one vendor differ
///    here and nowhere else, and that is enough to make them two accounts.
/// 4. **entitlement** — this entry itself: the named account whose capacity
///    is spent.
/// 5. **inference model** — [`crate::profile::LaunchProfile::model`], again
///    per profile.
///
/// Replacing any one layer leaves the other four standing: the same
/// entitlement can serve two harnesses, the same harness can run under two
/// entitlements, the same entitlement can serve two models, and one vendor
/// and protocol can stand behind two credentials — which is what makes the
/// entitlement, not the vendor or the harness, the unit of capacity.
///
/// # Backing
///
/// `native_harness = "claude-code"` says *this entry is Claude Code's own
/// sign-in* — the resource `crate::provider::registry::ResourceKind::
/// NativeSubscription` describes — and replaces the default entry
/// [`EffectiveConfig::entitlements`] would otherwise supply for that
/// harness. Such an entry carries **no `credential` of its own**
/// ([`EntitlementLookupError::NativeSignInWithOwnCredential`]): the harness
/// authenticates itself, and what the registry's `NativeSubscription` names
/// is exactly *one shape of an entitlement, not the shape*. `provider =
/// "<name>"` says *this entry is the account behind that configured
/// provider*, which is how an API key becomes an entitlement with rules.
/// Naming both is refused when resolved
/// ([`EntitlementLookupError::TwoBackings`]). Naming neither is allowed: an
/// account with its own `credential` and no backing is a pool member —
/// listed by [`EffectiveConfig::entitlement_resources`], carrying its own
/// capacity and reset slots — that no launch profile charges yet; the 56A
/// broker packages are what will place work on it.
///
/// # Rules
///
/// Resolved by [`crate::routing::EntitlementRules`] and nowhere else: deny
/// wins over allow, an empty allow-list admits everything not denied. The
/// spellings are the routing types' own — [`IntegrationId::slug`],
/// [`crate::routing::classify::WorkloadTier::as_str`],
/// [`crate::routing::disposable::JobKind::as_str`] — through the three
/// `Configured*` newtypes above, so an unknown spelling is refused by the
/// loader rather than read as "no rule".
///
/// `deny_unknown_fields` is load-bearing for the same reason those newtypes
/// are, one level up: the fields are plural, `deny_harness` for
/// `deny_harnesses` is the natural typo, and a rule that silently does not
/// exist is not a cosmetic default — an empty deny-list *admits*. The
/// forward-compatibility story `ConfigError::UnsupportedVersion` tells is
/// about `version`, and it already refuses to *write* a file it does not
/// understand; refusing to read a rule it does not understand is the same
/// fail-closed choice applied to the one table that grants capacity.
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

    /// The resolved value, named `name` — the key this entry was stored
    /// under — and attributed to `layer`.
    pub fn to_resolved(
        &self,
        name: &str,
        layer: Layer,
    ) -> Result<ResolvedEntitlement, EntitlementLookupError> {
        let backing = match (self.native_harness, &self.provider) {
            (Some(_), Some(_)) => {
                return Err(EntitlementLookupError::TwoBackings {
                    name: name.to_owned(),
                });
            }
            (Some(harness), None) => EntitlementBacking::NativeHarness(harness.id()),
            (None, Some(provider)) => EntitlementBacking::Provider(provider.clone()),
            (None, None) => EntitlementBacking::Unstated,
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
        Ok(ResolvedEntitlement {
            name: name.to_owned(),
            kind: self.kind,
            vendor: self.vendor,
            credential: self.credential.as_ref().map(|c| c.secret_ref().clone()),
            backing,
            rules: self.rules(),
            layer,
            remaining_capacity: None,
            seconds_until_reset: None,
            capacity_scope: None,
            throttling: None,
            models: None,
            spend: None,
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
    /// The entry names neither. Listed, never matched, never charged.
    Unstated,
}
impl EntitlementBacking {
    /// What pays for this account, as the **router** may branch on it — map
    /// line 1970's *"subscription to subscription to API credits"*, and the
    /// user's ruling of 2026-08-31: *"A api key or a subscription isn't that
    /// the distinction?"* It is, and the distinction is already structural
    /// here rather than a field somebody typed:
    /// [`Self::NativeHarness`] authenticates **through the harness**, which
    /// is a subscription, and [`Self::Provider`] carries a credential of its
    /// own, which is an API key. The loader **enforces** the separation —
    /// an entry that is both is refused as
    /// [`EntitlementLookupError::NativeSignInWithOwnCredential`], map line
    /// 1973's isolation rule — so nothing here is a guess.
    ///
    /// This is why [`EntitlementKind`]'s invariant survives Phase 56A step
    /// 5 intact: routing branches on the *backing*, never on the *kind*.
    pub fn source(&self) -> crate::routing::EntitlementSource {
        match self {
            Self::NativeHarness(_) => crate::routing::EntitlementSource::Subscription,
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
    pub(super) name: String,
    pub(super) kind: Option<EntitlementKind>,
    pub(super) vendor: Option<EntitlementVendor>,
    /// This account's own authentication — a **reference**, never a value.
    /// Safe to hold and to `Debug` because every field of a [`SecretRef`] is
    /// a name.
    pub(super) credential: Option<SecretRef>,
    pub(super) backing: EntitlementBacking,
    pub(super) rules: crate::routing::EntitlementRules,
    pub(super) layer: Layer,
    /// Map line 1963's remaining-capacity slot — map line 1965's producer is
    /// [`Self::with_telemetry`], which populates it from a gateway-captured
    /// per-provider reading. `None` until that resolver runs, and `None`
    /// thereafter for an entitlement whose provider exposes nothing: an
    /// entitlement nothing has read is *unknown*, never full and never
    /// empty.
    pub(super) remaining_capacity: Option<crate::provider::quota::RemainingCapacityScore>,
    /// Map line 1963's reset-time slot, in seconds. Same contract as
    /// `remaining_capacity`: [`Self::with_telemetry`] populates it, `None`
    /// is unknown.
    pub(super) seconds_until_reset: Option<i64>,
    /// Whose reading `remaining_capacity` and `seconds_until_reset` are —
    /// `Some` exactly when either slot is populated. One scope for the pair,
    /// because both come from the same cached provider reading.
    pub(super) capacity_scope: Option<TelemetryScope>,
    /// Map line 1965's recent-throttling facet. `None` until
    /// [`Self::with_telemetry`] runs with the ledger's rows in hand —
    /// *unknown*, never "none observed": an absence may only be reported by
    /// a resolver that actually looked.
    pub(super) throttling: Option<EntitlementThrottleReading>,
    /// Map line 1965's models facet. `None` is unknown, same rule as above.
    pub(super) models: Option<EntitlementModels>,
    /// Map line 1971's observed-spend facet, against which this entry's own
    /// [`crate::routing::EntitlementRules::spend_ceiling_tokens`] is
    /// compared. Same contract as the four facets above: `None` until
    /// [`Self::with_telemetry`] runs with the ledger's rows in hand, and
    /// `None` thereafter when no row carried a token count — *unknown*,
    /// never "nothing spent".
    pub(super) spend: Option<EntitlementSpendReading>,
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
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn kind(&self) -> Option<EntitlementKind> {
        self.kind
    }

    pub fn vendor(&self) -> Option<EntitlementVendor> {
        self.vendor
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
        self.credential.as_ref()
    }

    /// Remaining capacity, when telemetry has read one — `None` until
    /// [`Self::with_telemetry`] runs, and `None` thereafter for an
    /// entitlement whose provider exposes nothing. Unknown, never
    /// fabricated.
    pub fn remaining_capacity(&self) -> Option<&crate::provider::quota::RemainingCapacityScore> {
        self.remaining_capacity.as_ref()
    }

    /// Seconds until this account's allowance resets, when telemetry has
    /// read one — the same contract as [`Self::remaining_capacity`].
    pub fn seconds_until_reset(&self) -> Option<i64> {
        self.seconds_until_reset
    }

    /// Whose reading the capacity and reset slots carry — `Some` exactly
    /// when either slot is populated, and [`TelemetryScope::ProviderWide`]
    /// for every reading this build can take: the gateway's quota cache is
    /// keyed by provider, so both entitlements of one provider share it.
    pub fn capacity_scope(&self) -> Option<TelemetryScope> {
        self.capacity_scope
    }

    /// Map line 1965's recent-throttling facet — `None` means *unknown*
    /// (nothing consulted the ledger for this entry), never "none observed".
    pub fn throttling(&self) -> Option<&EntitlementThrottleReading> {
        self.throttling.as_ref()
    }

    /// Map line 1965's models facet — `None` means *unknown*.
    pub fn models(&self) -> Option<&EntitlementModels> {
        self.models.as_ref()
    }

    /// Map line 1971's observed-spend facet — `None` means *unknown*
    /// (nothing consulted the ledger, or no row it holds carried a token
    /// count), never "nothing spent".
    pub fn spend(&self) -> Option<&EntitlementSpendReading> {
        self.spend.as_ref()
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
        self.credential
            .as_ref()
            .map(|reference| crate::routing::CredentialId::new(provider, reference.clone()).label())
    }

    /// Map line 1965's producer — populate the four telemetry facets from
    /// what `telemetry` actually holds, each reading carrying its scope.
    ///
    /// - **Capacity and reset**, for a remote-provider backing: the
    ///   gateway-captured per-provider reading
    ///   ([`crate::provider::telemetry::GatewayQuotaCache`]) folded into the
    ///   provider's own capacity shape. The cache is keyed by provider and
    ///   the gateway's write is settled, so the reading cannot be narrowed
    ///   to one credential: it is [`TelemetryScope::ProviderWide`], shared
    ///   verbatim by every entitlement of that provider. A local-inference
    ///   provider is skipped outright — a local server has no account
    ///   allowance, and the capacity model's local-inference estimate is not
    ///   a reading about *this account*.
    /// - **Recent throttling**: the ledger rows' informative throttles for
    ///   the provider, narrowed to this account's own
    ///   ([`crate::routing::evidence::recent_credential_throttles`]) when
    ///   every throttle row names its account, provider-wide otherwise.
    /// - **Models**: the provider's own declared catalogue
    ///   ([`crate::provider::cache::ModelCache`]), when one was ever
    ///   fetched; a native sign-in is [`EntitlementModels::HarnessDecided`]
    ///   — the harness picks, and Glasshouse does not know the plan's
    ///   models, so no list is ever invented for one.
    ///
    /// Every facet a source cannot answer stays `None` — unknown, never
    /// full, never empty, never zero-observed.
    pub fn with_telemetry(mut self, telemetry: &EntitlementTelemetry<'_>) -> Self {
        match self.backing.clone() {
            EntitlementBacking::Provider(provider) => {
                self.populate_provider_facets(&provider, telemetry);
            }
            EntitlementBacking::NativeHarness(_) => {
                self.models = Some(EntitlementModels::HarnessDecided);
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
            self.remaining_capacity = state.remaining_capacity_score();
            self.seconds_until_reset = state.seconds_until_reset(telemetry.now_unix);
            if self.remaining_capacity.is_some() || self.seconds_until_reset.is_some() {
                self.capacity_scope = Some(TelemetryScope::ProviderWide);
            }
        }

        if let Some(observations) = telemetry.observations {
            let label = self.credential_label();
            let counted = crate::routing::evidence::recent_credential_throttles(
                observations,
                provider,
                label.as_deref(),
            );
            self.throttling = Some(EntitlementThrottleReading {
                throttled: counted.throttled,
                scope: if counted.account_narrowed {
                    TelemetryScope::PerAccount
                } else {
                    TelemetryScope::ProviderWide
                },
            });
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
            self.spend = counted.tokens.map(|tokens| EntitlementSpendReading {
                tokens,
                scope: if counted.account_narrowed {
                    TelemetryScope::PerAccount
                } else {
                    TelemetryScope::ProviderWide
                },
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
        } else if self.capacity_scope != Some(TelemetryScope::PerAccount) {
            let label = self.credential_label();
            let session_count = telemetry
                .session_counts
                .and_then(|counts| counts.get(self.name.as_str()))
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
                self.seconds_until_reset,
                session_count,
            )
            .map(|mut estimate| {
                estimate.since_unix = regime_changed_at;
                estimate
            });
        }

        if let Some(catalogues) = telemetry.model_catalogues {
            self.models = catalogues
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
        crate::routing::Entitlement::new(self.name.clone(), self.rules.clone())
            .with_configured(self.layer != Layer::Default)
            // Map line 1970's work item 1, and the two facets that need no
            // threshold to derive, so they are carried **here** rather than
            // by the caller: the backing discriminant is structural (see
            // [`EntitlementBacking::source`]) and the spend reading is a
            // raw token count. The four 56A-2 facets stay with the caller
            // for the reason above — a band is derived against the user's
            // own thresholds, which this method does not hold.
            .with_source(self.backing.source())
            .with_spend(self.spend.map(|reading| {
                crate::routing::EntitlementSpendFacet::new(
                    reading.tokens,
                    reading.scope == TelemetryScope::PerAccount,
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
            EntitlementBacking::Unstated => "no backing stated".to_owned(),
        };
        let mut parts = Vec::new();
        if let Some(kind) = self.kind {
            parts.push(kind.describe().to_owned());
        }
        if let Some(vendor) = self.vendor {
            parts.push(format!("vendor `{}`", vendor.as_str()));
        }
        parts.push(backing);
        parts.join(", ")
    }
}
/// Whose reading a telemetry facet is — map line 1965's scope discipline:
/// telemetry keyed by this account's own credential is one thing, telemetry
/// the whole provider shares is another, and a display that showed the
/// second as the first would be claiming per-account knowledge nothing
/// measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelemetryScope {
    /// Keyed by this entitlement's own credential — the reading is about
    /// this account and no other.
    PerAccount,
    /// Keyed by the provider — every entitlement of that provider shares
    /// this same reading.
    ProviderWide,
}
impl TelemetryScope {
    /// The display's scope word.
    pub fn as_str(self) -> &'static str {
        match self {
            TelemetryScope::PerAccount => "this account",
            TelemetryScope::ProviderWide => "provider-wide",
        }
    }
}
/// Map line 1965's recent-throttling facet: how many informative throttles
/// the evidence window records against this entitlement, and whose count it
/// is. A count of zero from a resolver that looked is "none observed" — a
/// different fact from the `None` an unresolved entry carries, which is
/// *unknown*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntitlementThrottleReading {
    throttled: usize,
    scope: TelemetryScope,
}
impl EntitlementThrottleReading {
    /// Informative throttles in the window — this account's own when
    /// [`Self::scope`] is [`TelemetryScope::PerAccount`], the provider's
    /// total otherwise.
    pub fn throttled(&self) -> usize {
        self.throttled
    }

    pub fn scope(&self) -> TelemetryScope {
        self.scope
    }
}
/// Map line 1971's observed-spend facet: how many tokens the evidence
/// window recorded against this entitlement, and whose reading that is.
///
/// **Tokens, not money** — see
/// [`EntitlementConfig::spend_ceiling_tokens`] and
/// [`crate::routing::evidence::CredentialSpend`] for why the only currency
/// this ledger holds is the one a ceiling can be checked against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntitlementSpendReading {
    tokens: u64,
    scope: TelemetryScope,
}
impl EntitlementSpendReading {
    /// Input plus output tokens in the window — this account's own when
    /// [`Self::scope`] is [`TelemetryScope::PerAccount`], the provider's
    /// total otherwise.
    pub fn tokens(&self) -> u64 {
        self.tokens
    }

    pub fn scope(&self) -> TelemetryScope {
        self.scope
    }
}
/// Map line 1965's models facet: which models this entitlement can serve,
/// from what its backing actually declares — never an invented list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntitlementModels {
    /// The provider's own declared model list — the fetched
    /// [`crate::provider::cache::ModelCatalogue`], which is per provider,
    /// so the scope is stated on the value.
    Declared {
        models: Vec<String>,
        scope: TelemetryScope,
    },
    /// A native sign-in: the harness picks its own models, and Glasshouse
    /// does not know the plan's list — an answer, not an absence.
    HarnessDecided,
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
        "entitlement `{name}` names both `native_harness` and `provider`; an entitlement is \
         a harness's own sign-in or the account behind a provider, not both"
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
