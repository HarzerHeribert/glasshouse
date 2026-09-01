//! Routing policy: which backend serves which work, and why.
//!
//! [`classify`] is a third, independent thing: not a policy that picks a
//! backend, but the lightweight, model-optional classification of a request
//! (Phase 35) that a future policy — Phase 34F/35B, neither built yet — would
//! read before picking one. Nothing in this module or its siblings consumes
//! a [`classify::TaskClassification`] today; see that module's doc comment.
//!
//! # Two policy classes, and the reason they are two types
//!
//! Phase 9I line 533 asks Glasshouse to *"keep interactive harness routing
//! and disposable-support-job routing as separate policy classes"*. That
//! sentence is easy to satisfy on paper and easy to lose: one router with a
//! `disposable: bool` parameter would read as compliance and would be one
//! careless call site away from routing a live coding session the way a
//! throwaway classification job is routed.
//!
//! So the separation here is **structural**, in three independent ways:
//!
//! 1. [`interactive::InteractiveRouting`] and
//!    [`disposable::DisposableRouting`] are distinct types with distinct
//!    result types — [`interactive::Assignment`] and
//!    [`disposable::DisposableChoice`]. Neither result converts into the
//!    other: there is no `From`, no `Into`, no shared trait, and no public
//!    field on either, so a caller holding one cannot produce the other
//!    without going through the policy that mints it.
//! 2. Neither module names the other. `interactive.rs` contains no mention
//!    of `disposable`, and `disposable.rs` none of `interactive`;
//!    `tests::the_two_policy_classes_do_not_name_each_other` scans both
//!    sources to keep it that way, the same move
//!    `gateway::mod`'s import scan already makes.
//! 3. They **decide differently on identical input**, which is the part that
//!    matters: given one catalogue in which a free model and a paid model
//!    both serve, the disposable class picks the free one and the
//!    interactive class keeps the backend the session started on. A test
//!    that only checked the type separation would pass for a router that had
//!    quietly become one policy.
//!
//! # What this module refuses to do
//!
//! Nothing here opens a socket, resolves a credential, or reads the clock.
//! Every function is a pure function of values the caller supplies —
//! including `now`, which is a parameter and never [`std::time::Instant::now`]
//! called inside a policy. That is not tidiness:
//!
//! - a policy that could probe would eventually probe, and Phase 9I line 534
//!   says free requests must not be spent on health probes (see
//!   [`free::FreePool`], whose only mutator is fed by real workload);
//! - a policy that read its own clock could not be tested for a cooldown
//!   boundary without waiting for one.
//!
//! # Credentials appear here only as names
//!
//! [`CredentialId`] holds a [`SecretRef`] — an environment variable name, or
//! a store service and account. Never a value. Phase 9I lines 537 and 538
//! require quota state to be tracked *per credential*, which means a
//! credential has to be a map key, and a map key is a thing that gets
//! printed. `SecretRef` is already the one shape in Glasshouse that is safe
//! to write into a tracked configuration file, so it is the one used here.

pub mod capability;
pub mod classify;
pub mod disposable;
pub mod domain;
pub mod evidence;
pub mod free;
pub mod interactive;
pub mod pressure;
pub mod request;
pub mod session;

use crate::provider::quota::CapacityBand;
use crate::routing::evidence::SubscriptionHeadroomEstimate;
use crate::secret::SecretRef;

/// Which credential, by name — never by value.
///
/// A credential is identified by the provider it belongs to **and** the
/// reference it is resolved through, because those two together are what
/// Phase 9I line 538 calls "two separate allowances": two keys for the same
/// router are two entries here, and exhausting one says nothing about the
/// other.
///
/// The provider name is part of the identity rather than a label beside it.
/// Without it, two providers that happened to read the same environment
/// variable would share one allowance, which is the same defect in the
/// opposite direction.
/// Deliberately **not** `Hash` or `Ord`. [`SecretRef`] derives neither, and
/// widening a type in `crate::secret` so that a routing map could be a
/// `HashMap` would be this module reaching into the one module whose surface
/// is kept deliberately narrow. The pools this keys are a handful of entries
/// long — see [`free::FreePool`], which searches a slice and sorts by
/// [`CredentialId::label`] when an order is needed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialId {
    provider: String,
    reference: SecretRef,
}

impl CredentialId {
    pub fn new(provider: impl Into<String>, reference: SecretRef) -> Self {
        Self {
            provider: provider.into(),
            reference,
        }
    }

    pub fn provider(&self) -> &str {
        &self.provider
    }

    pub fn reference(&self) -> &SecretRef {
        &self.reference
    }

    /// A short name for a diagnostic: the provider and the reference's own
    /// name, which is a variable name or a service/account pair.
    ///
    /// Safe to render for exactly the reason [`SecretRef`]'s own
    /// documentation gives — both variants hold names and nothing else.
    pub fn label(&self) -> String {
        match &self.reference {
            SecretRef::Environment { var } => format!("{}/{var}", self.provider),
            SecretRef::OsCredential { service, account } => {
                format!("{}/{service}:{account}", self.provider)
            }
        }
    }
}

/// Whether using a model costs the user anything at the margin.
///
/// Phase 9I line 527 — "mark selected models as free-tier or zero-marginal-cost
/// resources". Two states and no third: "probably free" is not a thing a
/// policy may act on, and a model nobody marked is [`Cost::Metered`], which
/// is the fail-closed direction. A router that guessed a model was free and
/// was wrong spends the user's money.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cost {
    /// Marked free-tier or zero-marginal-cost by the user's own
    /// configuration.
    Free,
    /// Everything else, including anything nobody has marked.
    Metered,
}

impl Cost {
    pub fn is_free(self) -> bool {
        matches!(self, Self::Free)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Free => "free",
            Self::Metered => "metered",
        }
    }
}

/// What is established about a backend's tool-call behaviour.
///
/// Three states, not two, and the third is why this type exists rather than a
/// `bool`. Phase 9H line 517 forbids failing over to a backend that "cannot
/// preserve the harness's required protocol or tool semantics", and answering
/// that needs "known not to" told apart from "nobody checked" — the same
/// distinction [`crate::harness::Declared`] draws, narrowed to the one
/// question routing asks. `crate::profile` builds these from the provider's
/// own `Declared<bool>`; this module never sees a `Declared` because it never
/// needs the evidence string, only the verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolSemantics {
    /// Established to carry tool calls on this protocol.
    Verified,
    /// Nobody established it either way. Not a "no".
    Unverified,
    /// Established **not** to carry them.
    KnownAbsent,
}

/// One destination a request could be sent to: a provider, over a protocol,
/// with one credential, at a marginal cost.
///
/// This is deliberately not [`crate::provider::Provider`]. A `Provider` is
/// configuration — several protocols, several credential variables, no notion
/// of which model is in play. A `Backend` is one already-resolved choice, and
/// a routing policy that took the configuration shape would have to make the
/// same narrowing decision at every call site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backend {
    provider: String,
    /// `WireProtocol::slug`. A name; this module never parses it back.
    protocol: String,
    model: AssignedModel,
    credential: CredentialId,
    cost: Cost,
    tools: ToolSemantics,
}

impl Backend {
    pub fn new(
        provider: impl Into<String>,
        protocol: impl Into<String>,
        model: AssignedModel,
        credential: CredentialId,
        cost: Cost,
        tools: ToolSemantics,
    ) -> Self {
        Self {
            provider: provider.into(),
            protocol: protocol.into(),
            model,
            credential,
            cost,
            tools,
        }
    }

    pub fn provider(&self) -> &str {
        &self.provider
    }

    pub fn protocol(&self) -> &str {
        &self.protocol
    }

    pub fn model(&self) -> &AssignedModel {
        &self.model
    }

    pub fn credential(&self) -> &CredentialId {
        &self.credential
    }

    pub fn cost(&self) -> Cost {
        self.cost
    }

    pub fn tools(&self) -> ToolSemantics {
        self.tools
    }
}

/// Which model Glasshouse assigned, including the honest case where it
/// assigned none.
///
/// Phase 9H line 505 asks for "a provider **and model**" at session start. A
/// gateway-backed launch profile need not name one, and when it does not the
/// harness sends whatever model it decided on. Recording that as
/// `model: None` would leave a reader unable to tell "no model" from "we
/// forgot"; this type says which happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssignedModel {
    /// Glasshouse named this model for the session, from the launch
    /// profile's own `model` field.
    Named(String),
    /// The launch profile named no model, so the harness's own default
    /// serves the session and Glasshouse assigned none. Not a failure.
    HarnessDefault,
}

impl AssignedModel {
    pub fn named(model: impl Into<String>) -> Self {
        Self::Named(model.into())
    }

    /// The model's name, or `None` when the harness chose it.
    pub fn name(&self) -> Option<&str> {
        match self {
            Self::Named(model) => Some(model),
            Self::HarnessDefault => None,
        }
    }

    /// For a diagnostic. Never `""`: an empty column reads as missing data.
    pub fn label(&self) -> &str {
        match self {
            Self::Named(model) => model,
            Self::HarnessDefault => "the harness's own default",
        }
    }
}

/// Whether a change of backend leaves provider-side prompt caching usable.
///
/// # Pinning down "likely", because Phase 9H line 516 uses that word
///
/// The line is *"warn when failover is likely to invalidate provider-side
/// prompt caching"*, and a capability whose trigger is a feeling is not a
/// capability. So the rule is written down once, here, and every warning in
/// Glasshouse comes from it:
///
/// - **Different provider.** The cache is held by the provider. A request
///   that goes to a different service reaches a cache that never saw this
///   conversation. Certain, so [`CacheLocality::Lost`].
/// - **Different model.** A provider-side cache is keyed by the model as well
///   as the prefix — a cached prefix for one model is not a cached prefix for
///   another. Certain, so [`CacheLocality::Lost`].
/// - **Same provider and model, different credential.** Provider-side caches
///   are commonly scoped to the account a key belongs to, and Glasshouse has
///   established that for **no** configured provider — every template in
///   [`crate::provider::templates`] declares its capabilities `Unverified`.
///   So this is the case the map's "likely" is actually about, and it is
///   [`CacheLocality::LikelyLost`]: warned, and said as a likelihood rather
///   than as a fact.
/// - **Nothing moved.** [`CacheLocality::Preserved`].
///
/// The consequence worth stating: rotating a credential (Phase 9I line 537)
/// is a cache event too, which is not obvious and is why the rule is a
/// function rather than a comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheLocality {
    /// The request still goes to the same provider, model and credential.
    Preserved,
    /// A provider-side cache cannot survive this change.
    Lost(CacheLossReason),
    /// It probably cannot, and nothing has established that it can.
    LikelyLost(CacheLossReason),
}

/// What moved, for a warning that names the cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheLossReason {
    ProviderChanged,
    ModelChanged,
    /// Both, which is worth distinguishing so the warning does not have to
    /// pick one.
    ProviderAndModelChanged,
    CredentialChanged,
}

impl CacheLossReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProviderChanged => "the provider changed",
            Self::ModelChanged => "the model changed",
            Self::ProviderAndModelChanged => "the provider and the model both changed",
            Self::CredentialChanged => "the credential changed",
        }
    }
}

impl CacheLocality {
    /// Compare two backends for cache locality, by the rule in this type's
    /// documentation.
    ///
    /// The one place the rule exists. Every warning, every stickiness
    /// justification and every migration note reads it from here, so there is
    /// no second copy to drift.
    pub fn between(from: &Backend, to: &Backend) -> Self {
        let provider_changed = from.provider() != to.provider();
        let model_changed = from.model() != to.model();
        match (provider_changed, model_changed) {
            (true, true) => Self::Lost(CacheLossReason::ProviderAndModelChanged),
            (true, false) => Self::Lost(CacheLossReason::ProviderChanged),
            (false, true) => Self::Lost(CacheLossReason::ModelChanged),
            (false, false) => {
                if from.credential() == to.credential() {
                    Self::Preserved
                } else {
                    Self::LikelyLost(CacheLossReason::CredentialChanged)
                }
            }
        }
    }

    /// Whether this change is worth warning the user about at all.
    pub fn warrants_a_warning(&self) -> bool {
        !matches!(self, Self::Preserved)
    }

    /// The reason, when there is one.
    pub fn reason(&self) -> Option<CacheLossReason> {
        match self {
            Self::Preserved => None,
            Self::Lost(reason) | Self::LikelyLost(reason) => Some(*reason),
        }
    }
}

impl std::fmt::Display for CacheLocality {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Preserved => f.write_str("provider-side prompt caching is unaffected"),
            Self::Lost(reason) => write!(
                f,
                "provider-side prompt caching is invalidated: {}",
                reason.as_str()
            ),
            Self::LikelyLost(reason) => write!(
                f,
                "provider-side prompt caching is likely to be invalidated: {} — provider caches \
                 are commonly scoped to the account a key belongs to, and Glasshouse has not \
                 established otherwise for this provider",
                reason.as_str()
            ),
        }
    }
}

/// Why a free resource is the one being used.
///
/// Phase 9I line 540 — "show whether a free resource is being used because of
/// user preference, quota preservation, or fallback". Three reasons, exactly
/// the three the line names, produced by the policy that made the choice
/// rather than reconstructed afterwards by a view that would have to guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UseReason {
    /// The user asked for free resources to be preferred.
    UserPreference,
    /// A metered resource was available and was left alone on purpose.
    QuotaPreservation,
    /// The preferred resource could not serve, and this one could.
    Fallback,
}

impl UseReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UserPreference => "user preference",
            Self::QuotaPreservation => "quota preservation",
            Self::Fallback => "fallback",
        }
    }
}

impl std::fmt::Display for UseReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

/// One named contribution to a routing decision, with the magnitude it added
/// and the evidence behind it.
///
/// Phase 9J line 575 asks for "the contribution of the pairing prior in
/// routing explanations"; this type is deliberately not named after pairing.
/// `phase-32d`'s protected-quota reserve needs the identical shape for a
/// completely different contribution, and a type only pairing could populate
/// would have to be rebuilt for it. A magnitude of `0.0` is a legitimate
/// contribution — an informational line (which class a pairing is, how much
/// evidence exists) that adds nothing to the total but still belongs in the
/// explanation.
#[derive(Debug, Clone, PartialEq)]
pub struct Contribution {
    name: String,
    magnitude: f64,
    evidence: String,
}

impl Contribution {
    pub fn new(name: impl Into<String>, magnitude: f64, evidence: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            magnitude,
            evidence: evidence.into(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn magnitude(&self) -> f64 {
        self.magnitude
    }

    pub fn evidence(&self) -> &str {
        &self.evidence
    }
}

/// An ordered list of named contributions behind one routing decision, and
/// their sum.
///
/// Ordered because a reader compares a decision to its reasons top to bottom,
/// and because the caller that builds one (a scoring policy) is the only
/// party that knows which contribution logically comes first. Nothing here
/// deduplicates or reorders by name: two contributions with the same name are
/// two lines, and that is a policy's own affair, not this type's.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RoutingExplanation {
    contributions: Vec<Contribution>,
}

impl RoutingExplanation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, contribution: Contribution) -> &mut Self {
        self.contributions.push(contribution);
        self
    }

    pub fn contributions(&self) -> &[Contribution] {
        &self.contributions
    }

    /// The sum of every contribution's magnitude — the score a policy would
    /// rank candidates by, not a value this type interprets on its own.
    pub fn total(&self) -> f64 {
        self.contributions.iter().map(Contribution::magnitude).sum()
    }

    /// One line per contribution, signed magnitude first, for a diagnostic.
    pub fn render(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        for contribution in &self.contributions {
            let _ = writeln!(
                out,
                "  {:+.3}  {} — {}",
                contribution.magnitude(),
                contribution.name(),
                contribution.evidence()
            );
        }
        out
    }
}

/// A hard constraint a candidate failed, named so a routing explanation can
/// say *which* one rather than only that the candidate was refused.
///
/// Phase 9J line 568 — "apply hard protocol, tool, capability, privacy, and
/// user constraints before applying the pairing prior" — names the first
/// five, and map line 1516 — *"exclude candidates below the classified
/// minimum workload tier"* — names the sixth. Each is a line of the map, not
/// this module's guess at what a hard constraint could be.
///
/// [`Self::WorkloadTier`] carries the two tiers it compared, because "below
/// the minimum tier" is only readable next to which tier was required and
/// which was offered — see [`Self::reason`].
///
/// [`Self::Entitlement`] — Phase 56 line 1954 — carries the entitlement's
/// **name** and what it refused, because *"never charge a task to a
/// subscription the user's rules did not allow"* is only inspectable when the
/// explanation says which entitlement and which rule. A name is a `String`,
/// which is why this type is no longer `Copy`: the one caller that copied a
/// constraint (`session::SessionRouter::apply_override`) clones it instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HardConstraint {
    Protocol,
    ToolSemantics,
    Capability,
    Privacy,
    UserConstraint,
    /// Line 1516. `offered` is the destination's established ceiling, which
    /// is strictly below `required`; a destination whose ceiling nobody has
    /// established is never given this constraint (`session::hard_constraint`).
    WorkloadTier {
        required: classify::WorkloadTier,
        offered: classify::WorkloadTier,
    },
    /// Line 1954. The entitlement that would be charged for this destination
    /// has a rule — [`EntitlementRules`] — that does not admit the harness
    /// or the tier this work would run as. Raised by
    /// `session::hard_constraint` from the [`Entitlement`] the caller
    /// attached to the destination; a destination with no entitlement
    /// attached is never given this constraint, for the same reason an
    /// unknown ceiling is never given [`Self::WorkloadTier`].
    Entitlement {
        /// The `[entitlements.<name>]` key, as the user wrote it.
        entitlement: String,
        refused: EntitlementRefusal,
    },
}

impl HardConstraint {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Protocol => "protocol",
            Self::ToolSemantics => "tool semantics",
            Self::Capability => "capability",
            Self::Privacy => "privacy",
            Self::UserConstraint => "user constraint",
            Self::WorkloadTier { .. } => "workload tier",
            Self::Entitlement { .. } => "entitlement",
        }
    }

    /// The sentence a person reads beside a rejection, for the constraints
    /// that carry enough to write one. `None` for the five that name only
    /// their kind — their explanations live at the site that raised them.
    pub fn reason(&self) -> Option<String> {
        match self {
            Self::WorkloadTier { required, offered } => Some(format!(
                "the task needs at least the `{required}` tier and this destination is \
                 established to offer at most `{offered}`"
            )),
            Self::Entitlement {
                entitlement,
                refused,
            } => Some(format!(
                "entitlement `{entitlement}` does not serve {refused}"
            )),
            Self::Protocol
            | Self::ToolSemantics
            | Self::Capability
            | Self::Privacy
            | Self::UserConstraint => None,
        }
    }
}

impl std::fmt::Display for HardConstraint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Phase 56/56A, lines 1946, 1947, 1954 and 1962 — an entitlement as a routing
// resource with rules of its own.
// ---------------------------------------------------------------------------

/// Which part of an entitlement's rules refused a destination or a candidate —
/// the thing a [`HardConstraint::Entitlement`] names after the entitlement
/// itself.
///
/// Three parts, one per rule axis, and two distinct askers: the session
/// router raises [`Self::Harness`] and [`Self::Tier`] through
/// `session::hard_constraint`, because a session has a harness and may have a
/// classified tier but never a job kind; `disposable::DisposableRouting`
/// raises [`Self::JobKind`] through [`Entitlement::job_constraint`], because
/// a disposable job has a [`disposable::JobKind`] and neither a harness nor
/// a tier of its own. No caller can raise the wrong part: each asks only the
/// question its work actually poses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntitlementRefusal {
    /// The rules do not admit this harness.
    Harness(crate::integrations::IntegrationId),
    /// The rules do not admit the tier this work would run as.
    Tier(classify::WorkloadTier),
    /// The rules do not admit this kind of bounded support job — the third
    /// clause of map line 1947, consumed by `disposable::DisposableRouting`.
    JobKind(disposable::JobKind),
    /// Map line 1953's model half: the entitlement's **declared** model list
    /// is known and does not name the model this destination would serve.
    /// Raised only from a [`EntitlementModelsFacet::Declared`] facet — a
    /// harness-decided or unknown facet constrains nothing, exactly as an
    /// unknown tier ceiling never raises [`HardConstraint::WorkloadTier`].
    /// Carries the model's name (which is why this enum is no longer
    /// `Copy`), so the refusal a person reads says which model.
    Model(String),
    /// Map line 1971's fourth axis: the user stated a spend ceiling for this
    /// entitlement and the spend **observed** against it has reached or
    /// passed the ceiling. Carries both numbers, because "over its ceiling"
    /// is only inspectable next to which ceiling and how much was seen —
    /// the same reason [`HardConstraint::WorkloadTier`] carries its pair.
    /// Raised only from an established reading: an entitlement whose spend
    /// nothing measured is never refused by this, exactly as an unknown
    /// tier ceiling never raises [`HardConstraint::WorkloadTier`].
    SpendCeiling {
        /// The ceiling the user wrote, in tokens.
        ceiling_tokens: u64,
        /// Input plus output tokens observed in the evidence window.
        observed_tokens: u64,
    },
}

impl std::fmt::Display for EntitlementRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Harness(harness) => write!(f, "harness `{}`", harness.slug()),
            Self::Tier(tier) => write!(f, "the `{tier}` tier"),
            Self::JobKind(kind) => write!(f, "the `{kind}` job kind"),
            Self::Model(model) => write!(f, "the `{model}` model"),
            Self::SpendCeiling {
                ceiling_tokens,
                observed_tokens,
            } => write!(
                f,
                "any more work — its spend ceiling of {ceiling_tokens} tokens is reached \
                 ({observed_tokens} observed)"
            ),
        }
    }
}

/// What an entitlement may and may never be charged for — map line 1947.
///
/// Six lists in three pairs, each pair an allow-list and a deny-list over one
/// axis: harnesses, workload tiers, job kinds. The resolution rule is the same
/// for every axis and lives in exactly one private function, `admits`:
///
/// - **deny wins over allow** — a value on both lists is refused;
/// - **an empty allow-list means everything not denied** — an entitlement
///   nobody restricted serves whatever asks, which is what makes the default
///   entry for a harness's own sign-in ([`Self::UNRESTRICTED`]) change
///   nothing for a user who configured nothing;
/// - **a non-empty allow-list admits only what it names.**
///
/// A rules value carries no name and no knowledge of what an entitlement *is*
/// — a Claude plan, an API key — because the router never decides on those:
/// [`Entitlement`] carries the name, and the kind stays in configuration,
/// where the announcement that reads it lives. This module never reads
/// configuration; `crate::config::EffectiveConfig::entitlement_for` resolves
/// one of these from the user's `[entitlements.<name>]` tables and the caller
/// attaches it to a `session::Destination`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EntitlementRules {
    allow_harnesses: Vec<crate::integrations::IntegrationId>,
    deny_harnesses: Vec<crate::integrations::IntegrationId>,
    allow_tiers: Vec<classify::WorkloadTier>,
    deny_tiers: Vec<classify::WorkloadTier>,
    allow_job_kinds: Vec<disposable::JobKind>,
    deny_job_kinds: Vec<disposable::JobKind>,
    /// Map line 1971's fourth axis: the cumulative spend, in tokens, past
    /// which this entitlement may not be charged. `None` — the default and
    /// the shape [`Self::UNRESTRICTED`] carries — is *the user stated no
    /// ceiling*, never *the ceiling is zero*.
    ///
    /// The other three axes are lists over a closed vocabulary and are
    /// decided by [`Self::admits`]. This one is a threshold against a
    /// *reading*, so it does not live in `admits` and is not asked by
    /// [`Self::refusal`]: it needs telemetry the rules value does not hold,
    /// and it is asked by [`Entitlement::spend_constraint`], where the
    /// observed spend is.
    spend_ceiling_tokens: Option<u64>,
}

impl EntitlementRules {
    /// No rule on any axis: serves every harness, every tier, every job kind.
    /// The rules a harness's own sign-in carries when the user configured no
    /// `[entitlements]` entry for it.
    pub const UNRESTRICTED: Self = Self {
        allow_harnesses: Vec::new(),
        deny_harnesses: Vec::new(),
        allow_tiers: Vec::new(),
        deny_tiers: Vec::new(),
        allow_job_kinds: Vec::new(),
        deny_job_kinds: Vec::new(),
        spend_ceiling_tokens: None,
    };

    #[must_use]
    pub fn allow_harnesses(
        mut self,
        harnesses: impl IntoIterator<Item = crate::integrations::IntegrationId>,
    ) -> Self {
        self.allow_harnesses = harnesses.into_iter().collect();
        self
    }

    #[must_use]
    pub fn deny_harnesses(
        mut self,
        harnesses: impl IntoIterator<Item = crate::integrations::IntegrationId>,
    ) -> Self {
        self.deny_harnesses = harnesses.into_iter().collect();
        self
    }

    #[must_use]
    pub fn allow_tiers(mut self, tiers: impl IntoIterator<Item = classify::WorkloadTier>) -> Self {
        self.allow_tiers = tiers.into_iter().collect();
        self
    }

    #[must_use]
    pub fn deny_tiers(mut self, tiers: impl IntoIterator<Item = classify::WorkloadTier>) -> Self {
        self.deny_tiers = tiers.into_iter().collect();
        self
    }

    #[must_use]
    pub fn allow_job_kinds(mut self, kinds: impl IntoIterator<Item = disposable::JobKind>) -> Self {
        self.allow_job_kinds = kinds.into_iter().collect();
        self
    }

    #[must_use]
    pub fn deny_job_kinds(mut self, kinds: impl IntoIterator<Item = disposable::JobKind>) -> Self {
        self.deny_job_kinds = kinds.into_iter().collect();
        self
    }

    /// Map line 1971's spend ceiling, in tokens. Stating none is the same
    /// as not calling this.
    #[must_use]
    pub fn with_spend_ceiling_tokens(mut self, tokens: Option<u64>) -> Self {
        self.spend_ceiling_tokens = tokens;
        self
    }

    /// The ceiling the user wrote, or `None` for *no ceiling was stated*.
    pub fn spend_ceiling_tokens(&self) -> Option<u64> {
        self.spend_ceiling_tokens
    }

    /// Whether no axis carries a rule — the [`Self::UNRESTRICTED`] shape,
    /// however it was built.
    pub fn is_unrestricted(&self) -> bool {
        *self == Self::UNRESTRICTED
    }

    /// The one resolution rule, over one axis: deny wins, an empty allow-list
    /// admits everything not denied, a non-empty one admits only its members.
    fn admits<T: PartialEq>(allow: &[T], deny: &[T], value: &T) -> bool {
        let denied = deny.contains(value);
        let allowed = allow.is_empty() || allow.contains(value);
        allowed && !denied
    }

    pub fn serves_harness(&self, harness: crate::integrations::IntegrationId) -> bool {
        Self::admits(&self.allow_harnesses, &self.deny_harnesses, &harness)
    }

    pub fn serves_tier(&self, tier: classify::WorkloadTier) -> bool {
        Self::admits(&self.allow_tiers, &self.deny_tiers, &tier)
    }

    /// The job-kind axis of line 1947, resolved by the same rule as the other
    /// two. A session has no job kind, so no *session* router asks this; the
    /// router for Glasshouse's own bounded support jobs,
    /// `disposable::DisposableRouting`, asks it through
    /// [`Entitlement::job_constraint`] for every candidate that carries an
    /// entitlement, and a candidate whose entitlement does not serve the job's
    /// kind is never a candidate at all.
    pub fn serves_job_kind(&self, kind: disposable::JobKind) -> bool {
        Self::admits(&self.allow_job_kinds, &self.deny_job_kinds, &kind)
    }

    /// Line 1954's question for one destination: why these rules refuse to
    /// serve `harness` at `tier`, or `None` when they admit it.
    ///
    /// The harness half is always asked. The tier half is asked only when a
    /// tier is *established* — `None` is "no task was stated", and a rule
    /// about tiers has nothing to compare against then, exactly as
    /// [`HardConstraint::WorkloadTier`] is never raised against an unknown
    /// ceiling. An allow-list of tiers therefore does not refuse a launch
    /// that stated no task; it refuses a task whose tier it does not name.
    pub fn refusal(
        &self,
        harness: crate::integrations::IntegrationId,
        tier: Option<classify::WorkloadTier>,
    ) -> Option<EntitlementRefusal> {
        if !self.serves_harness(harness) {
            return Some(EntitlementRefusal::Harness(harness));
        }
        if let Some(tier) = tier
            && !self.serves_tier(tier)
        {
            return Some(EntitlementRefusal::Tier(tier));
        }
        None
    }
}

/// Map line 1965's recent-throttling facet, as the router carries it: how
/// many informative throttles the evidence window recorded against this
/// entitlement, and whether that count could honestly be narrowed to this
/// account's own credential — 56A-2's `AccountSpecific` narrowing, reduced
/// to the one bit the score's evidence sentence needs. The caller resolves
/// it (`crate::config`'s telemetry resolver); this module reads no ledger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntitlementThrottleFacet {
    throttled: usize,
    account_scoped: bool,
}

impl EntitlementThrottleFacet {
    pub fn new(throttled: usize, account_scoped: bool) -> Self {
        Self {
            throttled,
            account_scoped,
        }
    }

    /// Informative throttles in the window — this account's own when
    /// [`Self::is_account_scoped`], the provider's shared total otherwise.
    pub fn throttled(&self) -> usize {
        self.throttled
    }

    pub fn is_account_scoped(&self) -> bool {
        self.account_scoped
    }

    /// The scope word an explanation renders — the same two words
    /// `glasshouse status` prints for the same facet, so a reading cannot
    /// claim per-account knowledge nothing measured.
    pub fn scope_word(&self) -> &'static str {
        if self.account_scoped {
            "this account"
        } else {
            "provider-wide"
        }
    }
}

/// Map line 1965's models facet, as the router carries it: which models this
/// entitlement can serve, from what its backing actually declares — never an
/// invented list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntitlementModelsFacet {
    /// The provider's own declared model list — the fetched catalogue,
    /// carried by name.
    Declared(Vec<String>),
    /// A native sign-in: the harness picks its own models, and Glasshouse
    /// does not know the plan's list — an answer, not an absence, and one
    /// that constrains nothing.
    HarnessDecided,
}

impl EntitlementModelsFacet {
    /// Whether these models admit `model`. Only a declared list that does
    /// not name it answers `false`; [`Self::HarnessDecided`] constrains
    /// nothing, because the harness's own choice is not Glasshouse's to
    /// second-guess.
    pub fn serves(&self, model: &str) -> bool {
        match self {
            Self::Declared(models) => models.iter().any(|declared| declared == model),
            Self::HarnessDecided => true,
        }
    }
}

/// What pays for an entitlement, as the **router** may branch on it — map
/// line 1970's *"subscription to subscription to API credits"*, and the
/// user's ruling of 2026-08-31 (`design-decisions.md` §Phase 56A, "Step 4's
/// fallback order"): *"Determining model if they are api or subscription is
/// just which entitlement brings them. A api key or a subscription isn't
/// that the distinction?"*
///
/// # Why this exists and [`crate::config::EntitlementKind`] still does not
///
/// `EntitlementKind` carries its own invariant — *"No rule depends on it —
/// so a wrong `kind` misdescribes an entitlement and never misroutes one"* —
/// and it is optional and absent by default. This value is neither: it is
/// derived structurally from `crate::config::EntitlementBacking`, which the
/// loader already **enforces** (an entry that is a native sign-in *and*
/// names its own credential is refused outright, map line 1973). So the
/// distinction the router branches on is one the data model guarantees
/// rather than one a person typed, and `EntitlementKind`'s invariant
/// survives untouched — it is the *backing*, not the *kind*, that routing
/// reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EntitlementSource {
    /// A harness's own first-party sign-in — a plan, authenticated through
    /// the harness itself
    /// (`crate::config::EntitlementBacking::NativeHarness`).
    Subscription,
    /// The account behind a configured `[providers.<name>]` entry, which
    /// carries a credential of its own — an API key, billed per call
    /// (`crate::config::EntitlementBacking::Provider`).
    ApiCredits,
    /// The entry names neither backing. *Listed, never matched, never
    /// charged* — and therefore never a fallback target either: an order
    /// over subscription and API credits has no step this belongs to.
    #[default]
    Unstated,
}

impl EntitlementSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Subscription => "subscription",
            Self::ApiCredits => "API credits",
            Self::Unstated => "no backing stated",
        }
    }
}

impl std::fmt::Display for EntitlementSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Whether two models sit in the same **user-assigned capability tier** —
/// the axis Phase 34F (`GH-TIER-AXIS-34F`, "Model capability and tier
/// calibration") builds, consumed here through one function and nothing
/// else.
///
/// [`Self::Unknown`] is not [`Self::Different`] and is not
/// [`Self::Same`]: it is *nobody has said*, which everywhere else in this
/// module contributes nothing and refuses nothing. Here it does one thing
/// more, because the ruling is explicit — *"You can't put a fable 5 task and
/// switch it to a nemotron v3"* — so an unknown relation **narrows** the
/// fallback rather than widening it: see [`same_capability_tier`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TierRelation {
    /// Established to be the same capability tier.
    Same,
    /// Established to be different capability tiers.
    Different,
    /// No axis has ranked one of these models. Never read as `Same`.
    Unknown,
}

/// The seam map line 1970's tier-preserving fallback consumes: are these two
/// destinations' models of the same user-assigned capability tier?
///
/// # The axis, now landed
///
/// `classify::WorkloadTier` ranks *how hard the task is*, and
/// `capability::CapabilityAxis` answers *can it do this at all* — neither is
/// "how capable is this model, relative to others". Phase 34F's answer to
/// that question is the resolved ceiling a user assigns a model (an
/// override, or a capability record's own `ceiling`): the same `WorkloadTier`
/// vocabulary, read as *the tier this model is trusted to serve*. The caller
/// resolves that once per destination — `main.rs::destination_tier_ceiling`,
/// beside where it attaches [`session::Destination::with_tier_ceiling`] — and
/// attaches it via [`session::Destination::with_capability_tier`]; this
/// function compares the two attached values and reads no configuration of
/// its own, matching every other free function in this module.
///
/// [`TierRelation::Unknown`] is not [`TierRelation::Same`]: a destination
/// whose model nobody assigned a tier answers unknown, and the fallback's
/// tier steps never fire on it — the ruling's own direction, *"You can't put
/// a fable 5 task and switch it to a nemotron v3"*, and a fallback that
/// silently downgrades the model *"is worse than a refusal, because the work
/// continues and looks fine"*.
pub fn same_capability_tier(
    from: Option<crate::routing::classify::WorkloadTier>,
    to: Option<crate::routing::classify::WorkloadTier>,
) -> TierRelation {
    match (from, to) {
        (Some(from), Some(to)) if from == to => TierRelation::Same,
        (Some(_), Some(_)) => TierRelation::Different,
        _ => TierRelation::Unknown,
    }
}

/// Map line 1971's spend half, as the router carries it: how much this
/// entitlement is **observed** to have spent inside the evidence window, and
/// whether that reading could honestly be narrowed to this account's own
/// credential — the same two-part shape [`EntitlementThrottleFacet`] carries,
/// resolved by the same caller from the same rows.
///
/// # "Spend" is tokens, and that is this ledger's own ruling
///
/// `routing_observations.cost_micro_usd` has **no producer in this build**
/// (see [`evidence::NewObservation::with_tokens`]), and map line 1465's
/// reader already settled the consequence in production:
/// *"'Spend' is tokens, input plus output as the provider reported them,
/// because that is the only currency this ledger holds."* A ceiling stated
/// in money that nothing counts could never refuse anything, which is the
/// one outcome map line 1971 — *"never let the broker exceed them"* — cannot
/// have. Cached input tokens are left out for line 1465's reason: providers
/// disagree on whether they are already inside `input_tokens`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntitlementSpendFacet {
    tokens: u64,
    account_scoped: bool,
}

impl EntitlementSpendFacet {
    pub fn new(tokens: u64, account_scoped: bool) -> Self {
        Self {
            tokens,
            account_scoped,
        }
    }

    /// Input plus output tokens observed in the window — this account's own
    /// when [`Self::is_account_scoped`], the provider's shared total
    /// otherwise.
    pub fn tokens(&self) -> u64 {
        self.tokens
    }

    pub fn is_account_scoped(&self) -> bool {
        self.account_scoped
    }

    /// The scope word an explanation renders — the same two words
    /// [`EntitlementThrottleFacet::scope_word`] prints, so one reading
    /// cannot claim per-account knowledge nothing measured while its sibling
    /// does not.
    pub fn scope_word(&self) -> &'static str {
        if self.account_scoped {
            "this account"
        } else {
            "provider-wide"
        }
    }
}

/// An entitlement as the router sees it — map lines 1946 and 1962: a named
/// resource with rules, separate from the harness that consumes it.
///
/// Two facts the router acts on structurally — the name, so a refusal and an
/// announcement can say which entitlement; and the rules — plus, since 56A
/// step 3 (lines 1953/1966–1969), the **telemetry facets the score reads**:
/// capacity band, seconds until reset, recent throttling, and the models the
/// backing declares. Every facet is a value the *caller resolved* from
/// 56A-2's telemetry (`crate::config`'s
/// `EffectiveConfig::configured_entitlements_with_telemetry` reads the
/// sources; `main.rs` derives the band against the user's own thresholds)
/// and `None` means **unknown** — a facet nothing measured contributes
/// nothing to the score and says so, never a guessed number. Which plan it
/// is, which vendor bills it and which credential authenticates it stay in
/// `crate::config` (`ResolvedEntitlement`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entitlement {
    name: String,
    rules: EntitlementRules,
    /// Whether a user or project actually wrote this entry, as opposed to
    /// the default a harness's own sign-in is synthesised with. The score's
    /// pool gate counts configured entries only: a user with zero or one
    /// configured entitlement has no pool to choose across, and the five
    /// entitlement terms stay inert for them by this bit.
    configured: bool,
    capacity_band: Option<CapacityBand>,
    seconds_until_reset: Option<i64>,
    throttling: Option<EntitlementThrottleFacet>,
    models: Option<EntitlementModelsFacet>,
    /// What pays for this account — map line 1970's order is over exactly
    /// this. Derived structurally from the backing by
    /// `crate::config::ResolvedEntitlement::to_routing`, never typed by a
    /// person; [`EntitlementSource::Unstated`] is the default a caller
    /// building one by hand gets, and it belongs to no step of the order.
    source: EntitlementSource,
    /// Map line 1971's spend reading, against which
    /// [`EntitlementRules::spend_ceiling_tokens`] is compared. `None` is
    /// unknown — nothing consulted the ledger — never "nothing spent".
    spend: Option<EntitlementSpendFacet>,
    /// Map lines 1244/1245/1246/1250/1251/1254's subscription-headroom
    /// estimate — carried here so a routing explanation or a pool view can
    /// render it, never so scoring is wired to it: this package attaches the
    /// facet, and does not change what any score term reads. `None` is
    /// unknown, the same rule as every facet above.
    headroom_estimate: Option<SubscriptionHeadroomEstimate>,
}

impl Entitlement {
    /// A named entitlement with rules and no telemetry read — every facet
    /// unknown, and `configured` true: a caller building one by hand is
    /// describing a real entry (the synthesised harness-default arrives
    /// through [`Self::with_configured`], from the one resolver that
    /// synthesises it).
    pub fn new(name: impl Into<String>, rules: EntitlementRules) -> Self {
        Self {
            name: name.into(),
            rules,
            configured: true,
            capacity_band: None,
            seconds_until_reset: None,
            throttling: None,
            models: None,
            source: EntitlementSource::Unstated,
            spend: None,
            headroom_estimate: None,
        }
    }

    /// See the `configured` field. `false` marks the default entry a
    /// harness's own sign-in gets when the user configured none.
    #[must_use]
    pub fn with_configured(mut self, configured: bool) -> Self {
        self.configured = configured;
        self
    }

    /// Attach what 56A-2's telemetry read about this account's remaining
    /// capacity: the band the caller derived against the user's own
    /// thresholds, and the seconds until the allowance resets. `None` is
    /// unknown, never full and never empty.
    #[must_use]
    pub fn with_capacity(
        mut self,
        band: Option<CapacityBand>,
        seconds_until_reset: Option<i64>,
    ) -> Self {
        self.capacity_band = band;
        self.seconds_until_reset = seconds_until_reset;
        self
    }

    /// Attach the recent-throttling facet. `None` is unknown — nothing
    /// consulted the ledger — never "none observed".
    #[must_use]
    pub fn with_throttling(mut self, throttling: Option<EntitlementThrottleFacet>) -> Self {
        self.throttling = throttling;
        self
    }

    /// Attach the models facet. `None` is unknown — no catalogue was ever
    /// read — which constrains nothing.
    #[must_use]
    pub fn with_models(mut self, models: Option<EntitlementModelsFacet>) -> Self {
        self.models = models;
        self
    }

    /// State what pays for this account — map line 1970's work item 1. The
    /// one production caller is
    /// `crate::config::ResolvedEntitlement::to_routing`, which reads it off
    /// the loader-enforced backing rather than off anything a person wrote.
    #[must_use]
    pub fn with_source(mut self, source: EntitlementSource) -> Self {
        self.source = source;
        self
    }

    /// Attach the observed-spend facet — map line 1971. `None` is unknown,
    /// never "nothing spent": a ceiling may only be judged reached by a
    /// resolver that actually looked.
    #[must_use]
    pub fn with_spend(mut self, spend: Option<EntitlementSpendFacet>) -> Self {
        self.spend = spend;
        self
    }

    /// Attach the subscription-headroom estimate — map lines
    /// 1244/1245/1246/1250/1251/1254. `None` is unknown, the same rule as
    /// every facet above; the one production caller is
    /// `crate::config::ResolvedEntitlement::to_routing`, which never sets
    /// this alongside a `capacity_band` that already reads a true
    /// per-account reading.
    #[must_use]
    pub fn with_headroom_estimate(
        mut self,
        headroom_estimate: Option<SubscriptionHeadroomEstimate>,
    ) -> Self {
        self.headroom_estimate = headroom_estimate;
        self
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn rules(&self) -> &EntitlementRules {
        &self.rules
    }

    pub fn is_configured(&self) -> bool {
        self.configured
    }

    pub fn capacity_band(&self) -> Option<CapacityBand> {
        self.capacity_band
    }

    pub fn seconds_until_reset(&self) -> Option<i64> {
        self.seconds_until_reset
    }

    pub fn throttling(&self) -> Option<&EntitlementThrottleFacet> {
        self.throttling.as_ref()
    }

    pub fn models(&self) -> Option<&EntitlementModelsFacet> {
        self.models.as_ref()
    }

    /// What pays for this account — map line 1970's order is over this and
    /// never over `crate::config::EntitlementKind`.
    pub fn source(&self) -> EntitlementSource {
        self.source
    }

    pub fn spend(&self) -> Option<&EntitlementSpendFacet> {
        self.spend.as_ref()
    }

    pub fn headroom_estimate(&self) -> Option<&SubscriptionHeadroomEstimate> {
        self.headroom_estimate.as_ref()
    }

    /// Map line 1971's spend half, as the hard constraint the router raises:
    /// an entitlement whose **observed** spend has reached the ceiling the
    /// user stated for it is refused by name.
    ///
    /// Both halves must be established. A ceiling nobody wrote refuses
    /// nothing, and a ceiling whose spend nothing measured refuses nothing
    /// either — *"nobody has said" is not "cannot"*, the rule every other
    /// gate in `session::hard_constraint` follows, and the alternative here
    /// would refuse every entitlement forever on a build whose ledger is
    /// empty. What the rule does guarantee is the direction map line 1971
    /// asks for: once the spend **is** known and has reached the ceiling,
    /// nothing admits the entitlement again — not a better score, and not
    /// map line 1970's fallback, which reselects only over candidates this
    /// gate already passed.
    pub fn spend_constraint(&self) -> Result<(), HardConstraint> {
        let (Some(ceiling_tokens), Some(spend)) = (self.rules.spend_ceiling_tokens, self.spend)
        else {
            return Ok(());
        };
        if spend.tokens() < ceiling_tokens {
            return Ok(());
        }
        Err(HardConstraint::Entitlement {
            entitlement: self.name.clone(),
            refused: EntitlementRefusal::SpendCeiling {
                ceiling_tokens,
                observed_tokens: spend.tokens(),
            },
        })
    }

    /// Line 1953's model half, as the hard constraint the session router
    /// raises: a candidate whose entitlement **declares** its models and
    /// does not declare this destination's is refused by name. A
    /// harness-decided facet, an unknown facet, and a destination whose
    /// model the harness picks itself ([`AssignedModel::HarnessDefault`])
    /// all constrain nothing — "nobody has said" is not "cannot", the same
    /// rule every other gate in `session::hard_constraint` follows.
    pub fn model_constraint(&self, model: &AssignedModel) -> Result<(), HardConstraint> {
        if let (Some(models), Some(name)) = (&self.models, model.name())
            && !models.serves(name)
        {
            return Err(HardConstraint::Entitlement {
                entitlement: self.name.clone(),
                refused: EntitlementRefusal::Model(name.to_owned()),
            });
        }
        Ok(())
    }

    /// [`EntitlementRules::refusal`], as the hard constraint the router
    /// raises — the one place a session-side rule becomes a
    /// [`HardConstraint`].
    pub fn constraint(
        &self,
        harness: crate::integrations::IntegrationId,
        tier: Option<classify::WorkloadTier>,
    ) -> Result<(), HardConstraint> {
        match self.rules.refusal(harness, tier) {
            Some(refused) => Err(HardConstraint::Entitlement {
                entitlement: self.name.clone(),
                refused,
            }),
            None => Ok(()),
        }
    }

    /// The job-kind axis as the hard constraint the disposable router raises
    /// — map line 1947's third clause, mirrored on [`Self::constraint`] so
    /// the refusal a support job reports names the entitlement and the job
    /// kind exactly as the session router's names the entitlement and the
    /// harness or tier.
    pub fn job_constraint(&self, kind: disposable::JobKind) -> Result<(), HardConstraint> {
        if self.rules.serves_job_kind(kind) {
            Ok(())
        } else {
            Err(HardConstraint::Entitlement {
                entitlement: self.name.clone(),
                refused: EntitlementRefusal::JobKind(kind),
            })
        }
    }
}

/// A candidate that has survived every hard constraint, and therefore the
/// only thing a scoring policy — a pairing prior among them — may be asked to
/// rank.
///
/// Phase 9J's design settled this as a structural requirement rather than a
/// convention (design decision 2): a policy function that scores a bare `T`
/// could be called before hard constraints ever ran, and nothing would say
/// so. A policy that scores `EligibleCandidate<T>` cannot be called that way,
/// because the only way to produce one is [`apply_hard_constraints`] actually
/// running the check. The private field is the whole mechanism — there is no
/// public constructor here to bypass it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EligibleCandidate<T> {
    value: T,
}

impl<T> EligibleCandidate<T> {
    pub fn value(&self) -> &T {
        &self.value
    }

    pub fn into_inner(self) -> T {
        self.value
    }
}

/// Filter `candidates` by `check`, in order, into what survives every hard
/// constraint and what was rejected and why.
///
/// This is the one function in Glasshouse that can produce an
/// [`EligibleCandidate`]. `check` is supplied by the caller rather than fixed
/// here, because "capability" and "privacy" are decided by configuration this
/// module does not read (line 568 names them; it does not define them) — this
/// function's job is only to make the *ordering* structural, not to invent
/// what a capability or a privacy constraint is.
pub fn apply_hard_constraints<T>(
    candidates: Vec<T>,
    check: impl Fn(&T) -> Result<(), HardConstraint>,
) -> (Vec<EligibleCandidate<T>>, Vec<(T, HardConstraint)>) {
    let mut eligible = Vec::new();
    let mut rejected = Vec::new();
    for candidate in candidates {
        match check(&candidate) {
            Ok(()) => eligible.push(EligibleCandidate { value: candidate }),
            Err(reason) => rejected.push((candidate, reason)),
        }
    }
    (eligible, rejected)
}

/// Map lines 1970 and 1971 — the seam Phase 34F plugs into, and the spend
/// gate.
#[cfg(test)]
mod entitlement_fallback_seam_tests {
    use super::*;

    /// Phase 34F's axis, landed: two attached tiers agree, two attached
    /// tiers disagree, and either side unattached reads as unknown rather
    /// than as a guess in either direction.
    #[test]
    fn the_tier_seam_compares_two_attached_values() {
        use crate::routing::classify::WorkloadTier;

        assert_eq!(
            same_capability_tier(Some(WorkloadTier::Frontier), Some(WorkloadTier::Frontier)),
            TierRelation::Same
        );
        assert_eq!(
            same_capability_tier(Some(WorkloadTier::Frontier), Some(WorkloadTier::Leaf)),
            TierRelation::Different,
            "two established, differing tiers must not read as unknown — a build that folded \
             this into `Unknown` would silently widen the fallback rather than narrow it"
        );
        assert_eq!(
            same_capability_tier(None, Some(WorkloadTier::Frontier)),
            TierRelation::Unknown,
            "a model nobody assigned a tier must not read as different, or the fallback would \
             have grounds to refuse a step it has no evidence about"
        );
        assert_eq!(same_capability_tier(None, None), TierRelation::Unknown);
    }

    fn entitlement(ceiling: Option<u64>, spend: Option<u64>) -> Entitlement {
        Entitlement::new(
            "claude-a",
            EntitlementRules::UNRESTRICTED.with_spend_ceiling_tokens(ceiling),
        )
        .with_spend(spend.map(|tokens| EntitlementSpendFacet::new(tokens, true)))
    }

    /// Both halves must be established, and once they are the refusal
    /// carries both numbers. At the ceiling exactly is *reached*, which is
    /// the fail-closed direction a spending protection takes everywhere else
    /// in this crate.
    #[test]
    fn a_spend_ceiling_refuses_only_when_the_ceiling_and_the_spend_are_both_known() {
        assert_eq!(entitlement(None, Some(9_999)).spend_constraint(), Ok(()));
        assert_eq!(entitlement(Some(1_000), None).spend_constraint(), Ok(()));
        assert_eq!(
            entitlement(Some(1_000), Some(999)).spend_constraint(),
            Ok(())
        );
        assert_eq!(
            entitlement(Some(1_000), Some(1_000)).spend_constraint(),
            Err(HardConstraint::Entitlement {
                entitlement: "claude-a".to_owned(),
                refused: EntitlementRefusal::SpendCeiling {
                    ceiling_tokens: 1_000,
                    observed_tokens: 1_000,
                },
            }),
            "at the ceiling is reached — a ceiling that admitted one more turn is not a ceiling"
        );
    }

    /// An entitlement with no rule and no reading is the value every
    /// unconfigured launch carries, and nothing about it changed.
    #[test]
    fn an_unrestricted_entitlement_is_unrestricted_and_says_so() {
        let bare = Entitlement::new("default", EntitlementRules::UNRESTRICTED);
        assert!(bare.rules().is_unrestricted());
        assert_eq!(bare.spend_constraint(), Ok(()));
        assert_eq!(bare.source(), EntitlementSource::Unstated);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A source file's production code: everything before the first
    /// `#[cfg(test)]`, with `//` comments stripped — the idiom
    /// `gateway/mod.rs`, `harness/mod.rs`, `main.rs`, `shim.rs` and
    /// `secret/mod.rs` each keep their own copy of.
    ///
    /// Comment lines go because this module's own documentation names both
    /// policy classes in one breath while explaining why they do not name
    /// each other.
    fn production_code(source: &str) -> String {
        source
            .split("#[cfg(test)]")
            .next()
            .expect("split always yields at least one part")
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn backend(provider: &str, model: &str, var: &str) -> Backend {
        Backend::new(
            provider,
            "anthropic-messages",
            AssignedModel::named(model),
            CredentialId::new(
                provider,
                SecretRef::Environment {
                    var: var.to_owned(),
                },
            ),
            Cost::Metered,
            ToolSemantics::Verified,
        )
    }

    /// Phase 9I line 533's structural half: neither policy module can reach
    /// the other, so nothing can quietly become one router with a flag.
    #[test]
    fn the_two_policy_classes_do_not_name_each_other() {
        let interactive = production_code(include_str!("interactive.rs"));
        assert!(
            !interactive.contains("disposable"),
            "routing/interactive.rs names the disposable policy class: the two policy classes \
             Phase 9I line 533 requires to stay separate have started to share code"
        );
        let disposable = production_code(include_str!("disposable.rs"));
        assert!(
            !disposable.contains("interactive"),
            "routing/disposable.rs names the interactive policy class: the two policy classes \
             Phase 9I line 533 requires to stay separate have started to share code"
        );
    }

    /// Phase 9I line 533's third case, and the one the original scan could
    /// not have anticipated: [`session`] is a policy class too.
    ///
    /// It ranks destinations rather than backends, so it legitimately names
    /// `interactive` — a destination's current backend is an interactive
    /// concern and the two are layers, not peers. It must never name
    /// `disposable`. A session router that could reach the throwaway-job
    /// policy is one careless call site away from sending a person's live
    /// coding session wherever a classification job would have gone, which is
    /// the exact failure line 533 exists to prevent.
    #[test]
    fn the_session_router_cannot_reach_the_disposable_policy_class() {
        let session = production_code(include_str!("session.rs"));
        assert!(
            !session.contains("disposable"),
            "routing/session.rs names the disposable policy class: a router that chooses where a \
             person's live session goes has reached the policy for throwaway jobs"
        );
    }

    /// The session router's project-isolation guarantee, structurally.
    ///
    /// Map lines 1593 and 1594 make it *rank sessions*, which is the first
    /// routing policy in Glasshouse with a reason to want to look one up —
    /// and a policy that could enumerate sessions would be one query away
    /// from ranking another project's. It cannot: warmth arrives as a
    /// [`crate::config::pairing::WarmSession`] the caller read, and
    /// checkpoint quality as two booleans the caller read, exactly as
    /// continuity already arrives at `interactive`. This is the same move
    /// `ContinuitySource`'s own doc comment describes, kept honest by a scan
    /// rather than by a convention.
    #[test]
    fn the_session_router_cannot_look_a_session_or_a_checkpoint_up() {
        let session = production_code(include_str!("session.rs"));
        for forbidden in ["crate::session", "crate::checkpoint", "SessionStore"] {
            assert!(
                !session.contains(forbidden),
                "routing/session.rs names `{forbidden}`: a router that can enumerate sessions \
                 can enumerate another project's, and project scoping would become a habit \
                 rather than a structure"
            );
        }
    }

    /// Phase 9I line 534's structural half. A health checker that spent the
    /// quota it protects would need a way to make a request; there is none in
    /// this module, and that absence is the capability.
    #[test]
    fn no_routing_policy_can_make_a_request() {
        for (name, source) in [
            ("routing/mod.rs", include_str!("mod.rs")),
            ("routing/interactive.rs", include_str!("interactive.rs")),
            ("routing/free.rs", include_str!("free.rs")),
            ("routing/disposable.rs", include_str!("disposable.rs")),
            ("routing/session.rs", include_str!("session.rs")),
        ] {
            let code = production_code(source);
            for forbidden in ["ureq", "TcpStream", "reqwest", "std::net"] {
                assert!(
                    !code.contains(forbidden),
                    "{name} names `{forbidden}`: a routing policy that can open a connection can \
                     spend the free requests Phase 9I line 534 exists to protect"
                );
            }
        }
    }

    #[test]
    fn nothing_moved_preserves_the_cache() {
        let one = backend("openrouter", "z-ai/glm-4.5-air:free", "OPENROUTER_API_KEY");
        assert_eq!(CacheLocality::between(&one, &one), CacheLocality::Preserved);
        assert!(!CacheLocality::between(&one, &one).warrants_a_warning());
    }

    #[test]
    fn a_different_provider_or_model_loses_the_cache_certainly() {
        let from = backend("openrouter", "model-a", "OPENROUTER_API_KEY");

        let other_provider = backend("nous", "model-a", "NOUS_API_KEY");
        assert_eq!(
            CacheLocality::between(&from, &other_provider),
            CacheLocality::Lost(CacheLossReason::ProviderChanged)
        );

        let other_model = backend("openrouter", "model-b", "OPENROUTER_API_KEY");
        assert_eq!(
            CacheLocality::between(&from, &other_model),
            CacheLocality::Lost(CacheLossReason::ModelChanged)
        );

        let both = backend("nous", "model-b", "NOUS_API_KEY");
        assert_eq!(
            CacheLocality::between(&from, &both),
            CacheLocality::Lost(CacheLossReason::ProviderAndModelChanged)
        );
    }

    /// The case the map's word "likely" is about, and the one a rule written
    /// as "did the provider change" would miss entirely.
    #[test]
    fn rotating_a_credential_is_only_likely_to_lose_the_cache() {
        let from = backend("openrouter", "model-a", "OPENROUTER_API_KEY");
        let rotated = backend("openrouter", "model-a", "OPENROUTER_API_KEY_2");
        let locality = CacheLocality::between(&from, &rotated);
        assert_eq!(
            locality,
            CacheLocality::LikelyLost(CacheLossReason::CredentialChanged)
        );
        assert!(locality.warrants_a_warning());
        assert!(
            locality.to_string().contains("likely"),
            "a likelihood must be said as one, not asserted as a fact: {locality}"
        );
    }

    /// Two keys for the same router are two identities; the same variable
    /// name under two providers is also two identities.
    #[test]
    fn a_credential_identity_is_the_provider_and_the_reference_together() {
        let env = |var: &str| SecretRef::Environment {
            var: var.to_owned(),
        };
        assert_ne!(
            CredentialId::new("openrouter", env("OPENROUTER_API_KEY")),
            CredentialId::new("openrouter", env("OPENROUTER_API_KEY_2"))
        );
        assert_ne!(
            CredentialId::new("openrouter", env("SHARED")),
            CredentialId::new("nous", env("SHARED"))
        );
    }

    /// A label is a diagnostic, so it must carry names and nothing else.
    #[test]
    fn a_credential_label_is_two_names() {
        let id = CredentialId::new(
            "openrouter",
            SecretRef::Environment {
                var: "OPENROUTER_API_KEY".to_owned(),
            },
        );
        assert_eq!(id.label(), "openrouter/OPENROUTER_API_KEY");
    }

    /// Anything nobody marked is metered. The fail-closed direction: a router
    /// that guessed "free" and was wrong spends the user's money.
    #[test]
    fn cost_has_no_third_state() {
        assert!(Cost::Free.is_free());
        assert!(!Cost::Metered.is_free());
    }

    /// A routing explanation is a plain ordered sum, and nothing here filters
    /// a contribution out for being zero or negative — the general surface
    /// must never itself become a hard rule.
    #[test]
    fn a_routing_explanation_sums_every_contribution_in_order() {
        let mut explanation = RoutingExplanation::new();
        explanation.push(Contribution::new("a", 1.0, "first"));
        explanation.push(Contribution::new("b", -0.25, "second"));
        explanation.push(Contribution::new("c", 0.0, "informational only"));

        assert_eq!(explanation.contributions().len(), 3);
        assert_eq!(explanation.contributions()[0].name(), "a");
        assert!((explanation.total() - 0.75).abs() < 1e-9);
        assert!(explanation.render().contains("informational only"));
    }

    /// The one function that can build an `EligibleCandidate`: candidates
    /// that fail `check` are rejected with a reason, and the rest come back
    /// wrapped, in the same order they went in.
    #[test]
    fn apply_hard_constraints_actually_filters_and_names_the_reason() {
        let candidates = vec![1, 2, 3, 4];
        let (eligible, rejected) = apply_hard_constraints(candidates, |n| {
            if *n % 2 == 0 {
                Ok(())
            } else {
                Err(HardConstraint::Protocol)
            }
        });

        assert_eq!(
            eligible
                .iter()
                .map(EligibleCandidate::value)
                .collect::<Vec<_>>(),
            vec![&2, &4]
        );
        assert_eq!(
            rejected,
            vec![(1, HardConstraint::Protocol), (3, HardConstraint::Protocol)]
        );
    }

    /// The structural half of design decision 2: nothing outside this module
    /// can construct an `EligibleCandidate` directly — its field is private
    /// and no `pub fn new`/`pub value` exists, so the only source is
    /// `apply_hard_constraints` actually running the check. A mutation that
    /// makes the field public or adds a bypass constructor is exactly what
    /// this guards.
    #[test]
    fn eligible_candidate_has_no_public_constructor_other_than_the_filter() {
        let code = production_code(include_str!("mod.rs"));
        assert!(
            !code.contains("pub value: T"),
            "EligibleCandidate's field must stay private, or a caller could build one without \
             passing through apply_hard_constraints"
        );
        assert!(
            !code.contains("impl<T> EligibleCandidate<T> {\n    pub fn new"),
            "a public constructor on EligibleCandidate would let a caller skip \
             apply_hard_constraints entirely"
        );
    }
}
