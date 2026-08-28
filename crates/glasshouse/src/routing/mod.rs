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

pub mod classify;
pub mod disposable;
pub mod domain;
pub mod evidence;
pub mod free;
pub mod interactive;

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
/// user constraints before applying the pairing prior" — names exactly these
/// five, and no others, on purpose: it is the map's own list, not this
/// module's guess at what a hard constraint could be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HardConstraint {
    Protocol,
    ToolSemantics,
    Capability,
    Privacy,
    UserConstraint,
}

impl HardConstraint {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Protocol => "protocol",
            Self::ToolSemantics => "tool semantics",
            Self::Capability => "capability",
            Self::Privacy => "privacy",
            Self::UserConstraint => "user constraint",
        }
    }
}

impl std::fmt::Display for HardConstraint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
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
