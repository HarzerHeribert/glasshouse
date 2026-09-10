//! The entitlement **catalogue**: what an inference account *is* and what it
//! can serve — its plan, its billing vendor, its authentication reference, the
//! provider or broker behind it, its spend ceiling, and the telemetry facets
//! read back against it.
//!
//! Client-neutral by construction. Nothing here names a harness, a job kind, a
//! Glasshouse session, an integration, a launch profile or an event: those are
//! the *policy* half — which harness an account is for, which job kinds it may
//! serve, how Glasshouse layers and applies it — and they live in
//! `crate::config::entitlement`, which consumes this module. The dependency
//! points one way only, in code and in these doc comments: nothing below links
//! to a policy item, so the arrow cannot be reversed by accident.
//!
//! The import list is the invariant, and it is checkable by reading: `serde`,
//! `crate::secret` (a credential is a *reference*, never a value) and
//! `crate::provider::quota` (an account's own capacity reading). No
//! `crate::integrations`, `crate::routing`, `crate::harness`, `crate::profile`,
//! `crate::session` or `crate::events`.

use serde::{Deserialize, Serialize};

use crate::provider::quota::RemainingCapacityScore;
use crate::secret::SecretRef;

/// Which plan an entitlement is — map line 1946's four: *"a Claude,
/// ChatGPT/Codex, or Gemini plan, or an API key"*.
///
/// Descriptive, and read by exactly one consumer: the launch announcement
/// that says which entitlement will serve a session. No rule depends on it —
/// `EntitlementConfig`'s rules are about harnesses, tiers and job kinds,
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
/// the launch announcement (`ResolvedEntitlement::describe`). **No rule and
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
/// A local broker that turns one subscription account into a loopback
/// inference endpoint. The value is a broker *kind*, never a URL, token, auth
/// directory, command, or credential. Those runtime details remain owned by
/// the broker process boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SubscriptionBroker {
    /// The pinned CLIProxyAPI sidecar supported by the first implementation.
    #[serde(rename = "cliproxyapi")]
    CliProxyApi,
}
impl SubscriptionBroker {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CliProxyApi => "cliproxyapi",
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
/// Deserializes `ProviderConfig::credential_env`, refusing any entry that
/// cannot be an environment variable name — the same shape check
/// [`EntitlementCredential`]'s `env` applies, and the same hole: this field
/// is documented as "names only — never a value" but nothing enforced it, so
/// a pasted key would be stored verbatim and later copied wherever this list
/// is rendered.
pub fn deserialize_credential_env_names<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
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

/// The catalogue half of a configured entitlement: the facts that say what
/// this account **is** and what it can serve, and nothing about who may use
/// it.
///
/// This is the shape a gateway reads from its own configuration file. The
/// Glasshouse-side `[entitlements.<name>]` table (`EntitlementConfig`) is a
/// superset — these six keys plus the harness, tier and job-kind rules, the
/// headroom override and the context-firewall override, none of which mean
/// anything to a gateway — and it projects into this type through
/// `EntitlementConfig::account`. The projection runs one way: nothing here
/// reconstructs the policy half.
///
/// Every key is spelled and typed exactly as the Glasshouse table spells it,
/// and the table is `deny_unknown_fields` on both sides, so the same six
/// lines of TOML parse to the same values whichever file they were written
/// in.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kind: Option<EntitlementKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    vendor: Option<EntitlementVendor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    credential: Option<EntitlementCredential>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    subscription_broker: Option<SubscriptionBroker>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    spend_ceiling_tokens: Option<u64>,
}
impl AccountEntry {
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

    /// The cumulative token spend past which this account may not be
    /// charged, or `None` for *no ceiling stated* — never zero.
    pub fn spend_ceiling_tokens(&self) -> Option<u64> {
        self.spend_ceiling_tokens
    }

    pub fn set_spend_ceiling_tokens(&mut self, value: Option<u64>) -> &mut Self {
        self.spend_ceiling_tokens = value;
        self
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
    /// A count taken by a resolver that actually looked, and whose reading
    /// it is. The fields stay private so the pair can never be assembled
    /// without stating a scope — the one mistake this facet exists to
    /// prevent.
    pub fn new(throttled: usize, scope: TelemetryScope) -> Self {
        Self { throttled, scope }
    }

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
/// `EntitlementConfig::spend_ceiling_tokens` and
/// `crate::routing::evidence::CredentialSpend` for why the only currency
/// this ledger holds is the one a ceiling can be checked against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntitlementSpendReading {
    tokens: u64,
    scope: TelemetryScope,
}
impl EntitlementSpendReading {
    /// A token count taken by a resolver that actually looked, and whose
    /// reading it is. Private fields for the same reason as
    /// [`EntitlementThrottleReading::new`]: a count without a scope is a
    /// claim nothing measured.
    pub fn new(tokens: u64, scope: TelemetryScope) -> Self {
        Self { tokens, scope }
    }

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

/// An account the catalogue has resolved: an [`AccountEntry`] under the name
/// it was stored as, plus whatever telemetry has since been read back
/// against it.
///
/// Every facet is `Option` and `None` means **unknown** — never full, never
/// empty, never "none observed". Only a resolver that actually looked may
/// fill one in, so the fields are written by that caller rather than given a
/// number here.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedAccount {
    pub name: String,
    pub kind: Option<EntitlementKind>,
    pub vendor: Option<EntitlementVendor>,
    /// This account's own authentication — a **reference**, never a value.
    /// Safe to hold and to `Debug` because every field of a [`SecretRef`] is
    /// a name.
    pub credential: Option<SecretRef>,
    pub remaining_capacity: Option<RemainingCapacityScore>,
    pub seconds_until_reset: Option<i64>,
    /// Whose reading [`Self::remaining_capacity`] and
    /// [`Self::seconds_until_reset`] are — `Some` exactly when either slot
    /// is populated, because both come from one provider reading.
    pub capacity_scope: Option<TelemetryScope>,
    pub throttling: Option<EntitlementThrottleReading>,
    pub models: Option<EntitlementModels>,
    pub spend: Option<EntitlementSpendReading>,
}
impl ResolvedAccount {
    /// The catalogue's own resolution step: an entry, under the name it was
    /// stored as, with every telemetry facet still unknown.
    ///
    /// Copies the three facts that describe the account itself. The entry's
    /// `provider`, `subscription_broker` and `spend_ceiling_tokens` are read
    /// by whatever decides *backing* and *limits* — in this build the policy
    /// half — so they are deliberately not carried here a second time.
    pub fn resolve(name: impl Into<String>, entry: &AccountEntry) -> Self {
        Self {
            name: name.into(),
            kind: entry.kind(),
            vendor: entry.vendor(),
            credential: entry
                .credential()
                .map(|credential| credential.secret_ref().clone()),
            remaining_capacity: None,
            seconds_until_reset: None,
            capacity_scope: None,
            throttling: None,
            models: None,
            spend: None,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn kind(&self) -> Option<EntitlementKind> {
        self.kind
    }

    pub fn vendor(&self) -> Option<EntitlementVendor> {
        self.vendor
    }

    /// The credential reference this account authenticates with, when it
    /// states one. Resolved to a value only through a
    /// [`crate::secret::SecretStore`], at the moment of use, by whatever
    /// launches against this account — never here.
    pub fn credential(&self) -> Option<&SecretRef> {
        self.credential.as_ref()
    }

    /// Remaining capacity, when a resolver has read one. `None` is unknown.
    pub fn remaining_capacity(&self) -> Option<&RemainingCapacityScore> {
        self.remaining_capacity.as_ref()
    }

    /// Seconds until this account's allowance resets, when a resolver has
    /// read one — the same contract as [`Self::remaining_capacity`].
    pub fn seconds_until_reset(&self) -> Option<i64> {
        self.seconds_until_reset
    }

    /// Whose reading the capacity and reset slots carry — `Some` exactly
    /// when either slot is populated.
    pub fn capacity_scope(&self) -> Option<TelemetryScope> {
        self.capacity_scope
    }

    /// The recent-throttling facet — `None` means *unknown* (nothing
    /// looked), never "none observed".
    pub fn throttling(&self) -> Option<&EntitlementThrottleReading> {
        self.throttling.as_ref()
    }

    /// The models facet — `None` means *unknown*.
    pub fn models(&self) -> Option<&EntitlementModels> {
        self.models.as_ref()
    }

    /// The observed-spend facet — `None` means *unknown* (nothing looked, or
    /// no row carried a token count), never "nothing spent".
    pub fn spend(&self) -> Option<&EntitlementSpendReading> {
        self.spend.as_ref()
    }
}
