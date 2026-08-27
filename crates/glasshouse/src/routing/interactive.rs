//! Sticky routing for one live harness-backed gateway session (Phase 9H).
//!
//! # The session owns the assignment; the assignment is not a session
//!
//! Phase 9H line 507 asks Glasshouse to *"treat the gateway assignment as
//! backend state belonging to the harness-backed session rather than as an
//! independent agent session"*. [`Assignment`] is therefore a value with no
//! identity of its own: no session id, no lifecycle, no start or end, nothing
//! that could be listed beside the user's real sessions. It is held by the
//! gateway a session started and dies with it.
//!
//! That is structural, not promised. Nothing in this file names
//! `crate::session`, and `tests::the_assignment_is_not_a_session_of_its_own`
//! scans for it — the same move `gateway::mod` already makes for the same
//! reason, and for the same product principle: the harness stays the harness.
//!
//! # Sticky means *nothing on a normal turn asks the question*
//!
//! Lines 508 and 509 are two halves of one behaviour, and the second is the
//! one that is easy to lose. It is not enough that a normal turn happens to
//! keep the same backend; a normal turn must keep it **even when a cheaper
//! free model is sitting right there**. So [`InteractiveRouting::next_turn`]
//! takes the alternatives as an argument. A version of this function that
//! could not see them would satisfy the line by accident, and the first
//! optimisation someone added would break it silently.
//!
//! # A failover is not a migration, and the difference is decidable today
//!
//! Lines 513 and 514 ask for same-family failover to be preferred and a
//! *material* model-family change to be treated as a migration decision.
//! Rather than invent a taxonomy by pattern-matching model names, this module
//! uses the conservative rule the available facts support:
//!
//! - **the same model identifier served by a different provider** is a
//!   same-family move — it is literally the same model, which is the common
//!   real case (one model offered by two routers) — so it is a
//!   [`FailureResponse::FailOver`];
//! - **any different model identifier** is treated as material, so it is
//!   offered as a migration and never taken transparently.
//!
//! Erring this way costs an automatic recovery that a family table would have
//! allowed. Erring the other way silently changes the model under a live
//! coding session, which is exactly what line 514 forbids.
//!
//! # Phase 9J and Phase 33A rank the survivors; they do not choose the group
//!
//! [`InteractiveRouting::on_provider_failure`] used to take the first
//! candidate in each group (same-model, then different-model) and return
//! immediately — the ordering above is unaffected by anything below this
//! paragraph. What changed is *which* candidate wins **within** a group,
//! once more than one survives `compatible`: `score_candidate` classifies
//! each one against the harness the failing session was serving
//! ([`pairing::classify`], Phase 9J) and weighs it against local observed
//! evidence for that exact `(provider, model, route, harness)` combination
//! (`crate::routing::evidence::ObservedEvidenceSource`, Phase 33A), and
//! `best` picks the highest-scoring survivor, the caller's own order
//! breaking a tie. A candidate can never be *excluded* this way — only
//! `compatible` refuses one — so this is design decision 1's "additive,
//! never a filter" made literal for this policy's own decision.

use crate::config::pairing::{
    ObservationSource, PairingPreference, native_pairing_prior_contribution,
};
use crate::harness::Declared;
use crate::harness::pairing;
use crate::integrations::IntegrationId;
use crate::routing::apply_hard_constraints;

use super::{Backend, CacheLocality, Contribution, RoutingExplanation, ToolSemantics};

/// The backend serving one live gateway-backed session, and the harness it is
/// serving.
///
/// The harness is part of the assignment because of line 506: *"keep the
/// harness identity and native session semantics explicit even when the
/// backend is routed through a Glasshouse gateway"*. A record of a routing
/// decision that did not say which harness it was made for would leave the
/// harness implicit exactly where the gateway makes it easiest to forget.
///
/// Carried as an integration **slug** rather than an `IntegrationId`, so that
/// `crate::gateway` — which may not name `crate::harness` or
/// `crate::integrations` — can hold one. `crate::profile` mints it from the
/// real identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assignment {
    harness: String,
    backend: Backend,
}

impl Assignment {
    pub fn new(harness: impl Into<String>, backend: Backend) -> Self {
        Self {
            harness: harness.into(),
            backend,
        }
    }

    /// The harness this backend is serving, as an integration slug.
    pub fn harness(&self) -> &str {
        &self.harness
    }

    pub fn backend(&self) -> &Backend {
        &self.backend
    }

    pub fn provider(&self) -> &str {
        self.backend.provider()
    }

    pub fn protocol(&self) -> &str {
        self.backend.protocol()
    }

    /// A one-line description for a diagnostic or a settings row. Names only.
    pub fn label(&self) -> String {
        format!(
            "{} on {} ({} over {})",
            self.backend.model().label(),
            self.backend.provider(),
            self.backend.credential().label(),
            self.backend.protocol()
        )
    }
}

/// Whether the user has pinned this session to one provider.
///
/// Phase 9H line 518. A pin is the user's statement that this session stays
/// where it is; it turns automatic failover off and it also refuses an
/// explicit migration away from the pinned provider, because a migration
/// under a live pin is the user contradicting an instruction they can simply
/// lift.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Pin {
    #[default]
    None,
    ToProvider(String),
}

impl Pin {
    pub fn provider(&self) -> Option<&str> {
        match self {
            Self::None => None,
            Self::ToProvider(provider) => Some(provider),
        }
    }

    fn permits(&self, provider: &str) -> bool {
        match self {
            Self::None => true,
            Self::ToProvider(pinned) => pinned == provider,
        }
    }
}

/// A failure that is the **provider's**, and therefore the only kind that may
/// move a session.
///
/// Phase 9H line 512 says *"after a real provider failure"*, and the word
/// real is doing work. Two things that look like failures are not this:
///
/// - a `4xx` that is not `429` is the harness's own request being wrong, and
///   moving to another provider would send the same wrong request there;
/// - a `401` or `403` is about the **credential**, which Phase 9I line 537
///   handles by rotating keys within the provider. Treating it as a provider
///   failure would abandon a working provider over one bad key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderFailure {
    /// The provider could not be reached at all.
    Unreachable,
    /// It answered, and the answer was a refusal it owns: `429`, or `5xx`.
    Refused { status: u16 },
}

impl ProviderFailure {
    /// Whether a status the provider returned is a provider failure.
    ///
    /// The one place the classification lives, so a caller cannot invent a
    /// second reading of the same number.
    pub fn from_status(status: u16) -> Option<Self> {
        match status {
            429 => Some(Self::Refused { status }),
            500..=599 => Some(Self::Refused { status }),
            _ => None,
        }
    }

    pub fn describe(self) -> String {
        match self {
            Self::Unreachable => "the provider could not be reached".to_owned(),
            Self::Refused { status } => format!("the provider answered {status}"),
        }
    }
}

/// Why a candidate backend may not serve this session.
///
/// Phase 9H line 517 — *"never fail over to a backend that cannot preserve
/// the harness's required protocol or tool semantics"*.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Incompatibility {
    /// It does not speak the protocol this session is being served over. A
    /// hard fact: the provider declared a base URL for some protocols and not
    /// for this one.
    Protocol {
        provider: String,
        speaks: String,
        needed: String,
    },
    /// What is established about its tool-call behaviour is weaker than what
    /// is established about the backend serving now.
    ///
    /// Weaker rather than absent, deliberately. The ordering is
    /// `KnownAbsent < Unverified < Verified`, and a candidate must be at
    /// least where the current backend already is. That refuses the obvious
    /// case — a backend known not to carry tool calls — and also the quieter
    /// one, where a session running on an established backend would be moved
    /// onto one nobody has checked. It costs a recovery that might have
    /// worked; the alternative costs a coding session its tools mid-task.
    ToolSemantics {
        provider: String,
        has: ToolSemantics,
        needs_at_least: ToolSemantics,
    },
}

impl Incompatibility {
    pub fn provider(&self) -> &str {
        match self {
            Self::Protocol { provider, .. } | Self::ToolSemantics { provider, .. } => provider,
        }
    }
}

impl std::fmt::Display for Incompatibility {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Protocol {
                provider,
                speaks,
                needed,
            } => write!(
                f,
                "`{provider}` serves {speaks} and this session is being served over {needed}"
            ),
            Self::ToolSemantics { provider, has, .. } => write!(
                f,
                "`{provider}`'s tool-call behaviour is {}, which is weaker than what the backend \
                 serving this session has established",
                describe_tools(*has)
            ),
        }
    }
}

fn describe_tools(tools: ToolSemantics) -> &'static str {
    match tools {
        ToolSemantics::Verified => "established",
        ToolSemantics::Unverified => "unestablished",
        ToolSemantics::KnownAbsent => "established to be absent",
    }
}

/// `KnownAbsent < Unverified < Verified`. See [`Incompatibility::ToolSemantics`].
fn tool_rank(tools: ToolSemantics) -> u8 {
    match tools {
        ToolSemantics::KnownAbsent => 0,
        ToolSemantics::Unverified => 1,
        ToolSemantics::Verified => 2,
    }
}

/// What a normal turn resolves to.
///
/// It carries the [`CacheLocality`] of the answer, which on a normal turn is
/// always [`CacheLocality::Preserved`] — line 510's "preserve prompt-cache
/// locality as a routing objective", said by the value rather than promised
/// by a comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnRouting {
    assignment: Assignment,
    cache: CacheLocality,
}

impl TurnRouting {
    pub fn assignment(&self) -> &Assignment {
        &self.assignment
    }

    pub fn cache(&self) -> &CacheLocality {
        &self.cache
    }
}

/// What a real provider failure does to a live session.
///
/// `PartialEq` only, not `Eq`: [`RoutingExplanation`] carries `f64`
/// magnitudes, which cannot be `Eq`, and this type composes it rather than
/// dropping it for the sake of a derive it does not otherwise need — nothing
/// here compares a `FailureResponse` for a `HashSet`/`BTreeSet` key.
#[derive(Debug, Clone, PartialEq)]
pub enum FailureResponse {
    /// Nothing moves; the harness sees the provider's own error.
    Stay { reason: StayReason },
    /// Move to a compatible backend serving the same model. Line 512 and 513.
    FailOver {
        to: Assignment,
        cache: CacheLocality,
        /// Line 575: why `to` won among every same-model survivor — the
        /// native-pairing prior and local evidence `score_candidate`
        /// computed for it, in the same shape
        /// [`crate::routing::disposable::DisposableChoice::explanation`]
        /// already surfaces for the other policy class.
        explanation: RoutingExplanation,
    },
    /// A compatible backend exists, but it serves a **different model**, so
    /// taking it would be a migration rather than a transparent failover.
    /// Line 514: offered, never taken.
    OfferMigration {
        to: Assignment,
        cache: CacheLocality,
        /// The same explanation [`Self::FailOver`]'s own field carries.
        explanation: RoutingExplanation,
    },
}

/// Why a session stayed where it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StayReason {
    /// Line 518: the user pinned this session and turned automatic failover
    /// off.
    SessionPinned { provider: String },
    /// Nothing compatible was configured. Every candidate and the reason it
    /// was refused, because "there was nowhere to go" is only useful when it
    /// says where it looked.
    NoCompatibleBackend { rejected: Vec<Incompatibility> },
}

/// Why an explicit migration was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationRefusal {
    /// Line 511 says *"at a task boundary"*. Mid-turn is not one: the harness
    /// has a request in flight and the conversation prefix it was built from.
    MidTurn,
    /// Line 518 again: lifting the pin is the user's own move, and doing it
    /// for them would make the pin advisory.
    SessionPinned { provider: String },
    /// Line 517 applies to a migration as much as to a failover.
    Incompatible(Incompatibility),
}

impl std::fmt::Display for MigrationRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MidTurn => f.write_str(
                "a session migration changes the backend a conversation is served by, so it is \
                 taken at a task boundary and not while a turn is in flight",
            ),
            Self::SessionPinned { provider } => write!(
                f,
                "this session is pinned to `{provider}`; lift the pin to migrate it"
            ),
            Self::Incompatible(why) => write!(f, "{why}"),
        }
    }
}

/// Whether the session is between tasks.
///
/// Line 511's "task boundary", as a value the caller must state rather than a
/// comment asking it to be careful.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionActivity {
    /// Between turns. A migration may be taken here.
    Idle,
    /// A turn is in flight.
    MidTurn,
}

/// The routing policy for one live harness-backed gateway session.
///
/// Holds the user's pin and nothing else. Everything it decides is a function
/// of its arguments, so the same policy value answers the same way every
/// time — which is what makes stickiness checkable rather than a property of
/// when you happened to ask.
#[derive(Debug, Clone, Default)]
pub struct InteractiveRouting {
    pin: Pin,
}

impl InteractiveRouting {
    pub fn new() -> Self {
        Self::default()
    }

    /// Line 518: pin this session to one provider and turn automatic failover
    /// off.
    pub fn pinned_to(provider: impl Into<String>) -> Self {
        Self {
            pin: Pin::ToProvider(provider.into()),
        }
    }

    pub fn pin(&self) -> &Pin {
        &self.pin
    }

    /// Line 505: the assignment a session is given when it starts.
    ///
    /// The harness identity comes in from the caller, which knows it; this
    /// function does not derive it from the backend, because a provider says
    /// nothing about which harness is talking to it.
    pub fn assign(&self, harness: &str, backend: Backend) -> Assignment {
        Assignment::new(harness, backend)
    }

    /// Lines 508, 509 and 510: what a normal turn is served by.
    ///
    /// `alternatives` is every other backend that could serve right now,
    /// **including free ones**. It is taken and deliberately not used to
    /// change the answer: that is the whole of line 509, and a signature
    /// without this argument could not express it.
    pub fn next_turn(&self, current: &Assignment, alternatives: &[Backend]) -> TurnRouting {
        let _ = alternatives;
        TurnRouting {
            assignment: current.clone(),
            cache: CacheLocality::between(current.backend(), current.backend()),
        }
    }

    /// Lines 512, 513, 514, 517 and 518: what a real provider failure does.
    ///
    /// `candidates` are the other backends configured for this session's
    /// protocol. The order is the caller's — the user's own ordering is the
    /// tiebreaker, exactly as it is in the free pool — but every candidate
    /// that survives `compatible` is now scored by Phase 9J's native-pairing
    /// prior and Phase 33A's local evidence (`score_candidate`), and the
    /// best-scoring one wins rather than simply the first one found. With no
    /// evidence at all (`evidence` answers `None` for everything, as
    /// [`crate::config::pairing::NoObservations`] always does) every
    /// candidate scores `0.0` and this reproduces "first compatible
    /// candidate" exactly, which is what every test in this module that
    /// passes `NoObservations` is checking.
    ///
    /// `evidence` is [`crate::config::pairing::ObservationSource`] rather
    /// than a concrete store for the same reason
    /// `native_pairing_prior_contribution` itself takes one: this function
    /// stays a pure function of its arguments (see this module's own header)
    /// with no knowledge of `crate::routing::evidence::EvidenceLedger` or how
    /// its caller reached it.
    pub fn on_provider_failure(
        &self,
        current: &Assignment,
        failure: ProviderFailure,
        candidates: &[Backend],
        evidence: &dyn ObservationSource,
    ) -> FailureResponse {
        let _ = failure;

        if let Pin::ToProvider(provider) = &self.pin {
            return FailureResponse::Stay {
                reason: StayReason::SessionPinned {
                    provider: provider.clone(),
                },
            };
        }

        let harness = resolve_harness(current.harness());
        let mut rejected = Vec::new();
        let mut same_model: Vec<(Assignment, RoutingExplanation)> = Vec::new();
        let mut migration: Vec<(Assignment, RoutingExplanation)> = Vec::new();

        for candidate in candidates {
            if candidate.provider() == current.provider()
                && candidate.model() == current.backend().model()
                && candidate.credential() == current.backend().credential()
            {
                // The backend that just failed. Not a candidate for its own
                // replacement.
                continue;
            }
            match compatible(current.backend(), candidate) {
                Err(why) => rejected.push(why),
                Ok(()) => {
                    let to = Assignment::new(current.harness(), candidate.clone());
                    let explanation = match harness {
                        Some(harness) => score_candidate(harness, candidate, evidence),
                        None => {
                            let mut explanation = RoutingExplanation::new();
                            explanation.push(Contribution::new(
                                "native-pairing prior",
                                0.0,
                                format!(
                                    "`{}` is not a harness this build recognises, so no pairing \
                                     could be classified for it",
                                    current.harness()
                                ),
                            ));
                            explanation
                        }
                    };
                    if candidate.model() == current.backend().model() {
                        // Line 513: the same model, served elsewhere. Every
                        // one found is kept; the best-scoring one is what
                        // gets returned below.
                        same_model.push((to, explanation));
                    } else {
                        // Line 514: a different model is material. Every one
                        // found is kept, and the best-scoring one is what
                        // gets offered — never taken transparently.
                        migration.push((to, explanation));
                    }
                }
            }
        }

        if !same_model.is_empty() {
            let (to, explanation) = best(same_model);
            let cache = CacheLocality::between(current.backend(), to.backend());
            return FailureResponse::FailOver {
                to,
                cache,
                explanation,
            };
        }

        if !migration.is_empty() {
            let (to, explanation) = best(migration);
            let cache = CacheLocality::between(current.backend(), to.backend());
            return FailureResponse::OfferMigration {
                to,
                cache,
                explanation,
            };
        }

        FailureResponse::Stay {
            reason: StayReason::NoCompatibleBackend { rejected },
        }
    }

    /// Line 511: an explicit migration, taken at a task boundary.
    ///
    /// Explicit means the caller asked for this exact backend. Nothing here
    /// searches, ranks or falls back — a migration that quietly landed
    /// somewhere else would be the transparent re-routing line 514 forbids,
    /// wearing the word "migration".
    pub fn migrate(
        &self,
        current: &Assignment,
        to: Backend,
        activity: SessionActivity,
    ) -> Result<Assignment, MigrationRefusal> {
        if activity == SessionActivity::MidTurn {
            return Err(MigrationRefusal::MidTurn);
        }
        if !self.pin.permits(to.provider()) {
            return Err(MigrationRefusal::SessionPinned {
                provider: self
                    .pin
                    .provider()
                    .expect("a pin that refuses a provider names one")
                    .to_owned(),
            });
        }
        compatible(current.backend(), &to).map_err(MigrationRefusal::Incompatible)?;
        Ok(Assignment::new(current.harness(), to))
    }
}

/// The evidence window [`InteractiveRouting::on_provider_failure`] reads
/// local observations from — wide enough that a session which only fails
/// over occasionally still has something to compare a fresh pairing prior
/// against, and bounded so a very old incident cannot outweigh how a pairing
/// has behaved lately.
pub const FAILOVER_EVIDENCE_WINDOW_SECONDS: i64 = 7 * 24 * 60 * 60;

/// [`Assignment::harness`], resolved to the strongly-typed identifier
/// [`pairing::classify`] needs. [`Assignment`] carries it as a slug rather
/// than an [`IntegrationId`] — see that type's own doc comment — so this is
/// the same reverse lookup [`crate::config::pairing::report`] already does
/// for a `--harness` argument, not a new mechanism. `None` for a slug this
/// build does not know, which only this function's own caller degrades for
/// (see [`InteractiveRouting::on_provider_failure`]).
fn resolve_harness(slug: &str) -> Option<IntegrationId> {
    IntegrationId::ALL
        .iter()
        .copied()
        .find(|id| id.slug() == slug)
}

/// Phase 9J and Phase 33A's one production consumer: what the native-pairing
/// prior and local observed evidence contribute to routing `candidate`,
/// given the harness the failing session was serving.
///
/// Two things this consumer does not have, and degrades honestly rather than
/// inventing:
///
/// - **Configured pairing corrections and the user's native-pairing
///   preference**, both of which live in configuration `crate::profile`'s
///   gateway path resolves once and which never reaches
///   [`crate::gateway::session::SessionRouting::bind`] today — see this
///   package's report for the exact patch. Until that is threaded
///   through, every candidate is scored against
///   [`crate::harness::pairing::PairingOverrides::default`] (no corrections
///   applied) and [`PairingPreference::Strong`] — the same out-of-the-box
///   default `EffectiveConfig::native_pairing_preference` itself falls back
///   to when nothing is configured.
/// - **The candidate's protocol as a [`crate::harness::WireProtocol`]** —
///   [`Backend::protocol`] is deliberately kept as an opaque slug (see that
///   method's own doc comment); [`pairing::wire_protocol_from_slug`] is the
///   reverse lookup, and it answers `None` for a slug it does not recognise
///   rather than guessing, which only weakens `Pairing::protocol_fit`, a
///   field `native_pairing_prior_contribution` never reads.
fn score_candidate(
    harness: IntegrationId,
    candidate: &Backend,
    evidence: &dyn ObservationSource,
) -> RoutingExplanation {
    let route = pairing::ServingRoute {
        provider: Some(candidate.provider().to_owned()),
        gateway: None,
        protocol: pairing::wire_protocol_from_slug(candidate.protocol()),
    };
    let query = pairing::PairingQuery {
        harness,
        model: candidate.model().clone(),
        route: route.clone(),
        // Not the `Declared<bool>` evidence string `crate::routing::Backend`
        // was built from — `routing` never keeps it, see `Backend::tools`'
        // own doc comment — and `classify` uses this only for
        // `Pairing::tool_semantics`, which `native_pairing_prior_contribution`
        // never reads either.
        tool_calls: Declared::Unverified,
        provider_protocols: Vec::new(),
    };
    let overrides = pairing::PairingOverrides::default();
    let pairing_value = pairing::classify(&query, &overrides);

    // `compatible()` already ran `candidate` through every hard constraint
    // `on_provider_failure` enforces (protocol, tool semantics) before this
    // function is ever called. This is that check's type-level receipt
    // (design decision 2), not a second, independent gate — the closure
    // always succeeds because the gate already ran.
    let (eligible, _) = apply_hard_constraints(vec![pairing_value], |_| Ok(()));
    let Some(eligible) = eligible.into_iter().next() else {
        unreachable!(
            "apply_hard_constraints keeps every input its own check accepts, and this check \
             accepts everything"
        );
    };

    // `EvidenceKey::launch_profile` is not part of this query:
    // `ObservedEvidenceSource` never reads it (see `routing::evidence`'s own
    // header for why), and a bound gateway assignment does not carry a
    // launch profile name to supply one honestly.
    let key = pairing::EvidenceKey::new(harness, String::new(), candidate.model().clone(), route);

    native_pairing_prior_contribution(&eligible, &key, PairingPreference::Strong, evidence)
}

/// The best-scoring `(Assignment, RoutingExplanation)` in `candidates`,
/// preferring the first one seen on a tie — the caller's own order. A build
/// with no evidence source reproduces the pre-batch-46 "first compatible
/// candidate" behaviour exactly this way: every contribution is `0.0` with
/// nothing to weigh, so every candidate ties and the first stands.
///
/// Panics on an empty `candidates` — both call sites only reach this after
/// checking `!candidates.is_empty()`.
fn best(mut candidates: Vec<(Assignment, RoutingExplanation)>) -> (Assignment, RoutingExplanation) {
    let mut best_index = 0;
    let mut best_total = candidates[0].1.total();
    for (index, (_, explanation)) in candidates.iter().enumerate().skip(1) {
        let total = explanation.total();
        if total > best_total {
            best_total = total;
            best_index = index;
        }
    }
    candidates.swap_remove(best_index)
}

/// Line 517, in one function: may `candidate` take over from `current`?
///
/// Two constraints and no others. The protocol must be the same one — not a
/// compatible-looking one, the same one, because a session's harness is
/// already speaking it and translation is not part of this architecture. And
/// what is established about tool calls must not go backwards.
fn compatible(current: &Backend, candidate: &Backend) -> Result<(), Incompatibility> {
    if candidate.protocol() != current.protocol() {
        return Err(Incompatibility::Protocol {
            provider: candidate.provider().to_owned(),
            speaks: candidate.protocol().to_owned(),
            needed: current.protocol().to_owned(),
        });
    }
    if tool_rank(candidate.tools()) < tool_rank(current.tools()) {
        return Err(Incompatibility::ToolSemantics {
            provider: candidate.provider().to_owned(),
            has: candidate.tools(),
            needs_at_least: current.tools(),
        });
    }
    Ok(())
}

/// Why the backend serving a session changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeCause {
    /// A real provider failure moved it — line 512.
    Failover(ProviderFailure),
    /// The user migrated it — line 511.
    Migration,
    /// One credential could not serve and another of the same provider's
    /// could — Phase 9I line 537.
    CredentialRotation,
}

impl ChangeCause {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Failover(_) => "failover",
            Self::Migration => "migration",
            Self::CredentialRotation => "credential rotation",
        }
    }
}

/// One recorded change of the backend serving a live session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignmentChange {
    pub from: Assignment,
    pub to: Assignment,
    pub cause: ChangeCause,
    pub cache: CacheLocality,
}

impl AssignmentChange {
    /// Whether this change is one line 515 asks to be recorded — *"when
    /// failover changes the provider or model serving a live session"*.
    ///
    /// A credential rotation within one provider and model changes neither,
    /// and is recorded anyway because the record is cheap and its absence
    /// would make a later cache warning unexplainable. The distinction is
    /// kept so a reader can tell which is which.
    pub fn changed_provider_or_model(&self) -> bool {
        self.from.provider() != self.to.provider()
            || self.from.backend().model() != self.to.backend().model()
    }

    /// The warning line 516 asks for, or `None` when there is nothing to warn
    /// about. See [`CacheLocality`] for what makes it decidable.
    pub fn cache_warning(&self) -> Option<String> {
        self.cache
            .warrants_a_warning()
            .then(|| format!("{}", self.cache))
    }
}

/// Every change of backend one live session has made, in order.
///
/// Line 515's *"record when failover changes the provider or model serving a
/// live session"*. In-process and ordered: it belongs to the session's
/// gateway and dies with it, exactly like [`Assignment`] and for the same
/// reason (line 507). Each entry is also emitted at `info` through
/// `tracing`, which is Glasshouse's existing opt-in log rather than a second
/// switch invented here.
#[derive(Debug, Clone, Default)]
pub struct RoutingRecord {
    entries: Vec<AssignmentChange>,
}

impl RoutingRecord {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one change, and say so in the log.
    ///
    /// Every value in the log line is a name, a status or a rendered
    /// explanation — there is nowhere here to put a credential, and
    /// `Assignment::label` is built from [`super::CredentialId::label`],
    /// which is two names.
    pub fn note(&mut self, change: AssignmentChange) {
        tracing::info!(
            harness = %change.to.harness(),
            cause = change.cause.as_str(),
            from = %change.from.label(),
            to = %change.to.label(),
            changed_provider_or_model = change.changed_provider_or_model(),
            cache = %change.cache,
            "the backend serving a Glasshouse gateway session changed"
        );
        self.entries.push(change);
    }

    pub fn entries(&self) -> &[AssignmentChange] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::pairing::NoObservations;
    use crate::routing::{AssignedModel, Cost, CredentialId};
    use crate::secret::SecretRef;

    fn backend(provider: &str, model: &str) -> Backend {
        backend_with(
            provider,
            model,
            "anthropic-messages",
            ToolSemantics::Unverified,
        )
    }

    fn backend_with(provider: &str, model: &str, protocol: &str, tools: ToolSemantics) -> Backend {
        Backend::new(
            provider,
            protocol,
            AssignedModel::named(model),
            CredentialId::new(
                provider,
                SecretRef::Environment {
                    var: format!("{}_API_KEY", provider.to_uppercase()),
                },
            ),
            Cost::Metered,
            tools,
        )
    }

    fn session() -> Assignment {
        Assignment::new("claude-code", backend("openrouter", "the-model"))
    }

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

    /// Line 507, structurally: a value that cannot see the session model
    /// cannot become a session.
    #[test]
    fn the_assignment_is_not_a_session_of_its_own() {
        let code = production_code(include_str!("interactive.rs"));
        assert!(
            !code.contains("crate::session"),
            "routing/interactive.rs names `crate::session`: the gateway assignment has started \
             to look like a session in its own right, which Phase 9H line 507 forbids"
        );
    }

    /// Line 506: the harness is part of the assignment, not implied by it.
    #[test]
    fn an_assignment_says_which_harness_it_serves() {
        let assignment = session();
        assert_eq!(assignment.harness(), "claude-code");
        assert!(assignment.label().contains("the-model"));
        assert!(assignment.label().contains("openrouter"));
    }

    /// Line 509, the one that needs the alternatives to be visible: a free
    /// model is sitting right there and the session does not move.
    #[test]
    fn a_normal_turn_keeps_its_backend_even_when_a_free_model_is_available() {
        let routing = InteractiveRouting::new();
        let current = session();
        let free = Backend::new(
            "nous",
            "anthropic-messages",
            AssignedModel::named("something-free"),
            CredentialId::new(
                "nous",
                SecretRef::Environment {
                    var: "NOUS_API_KEY".to_owned(),
                },
            ),
            Cost::Free,
            ToolSemantics::Verified,
        );

        let turn = routing.next_turn(&current, &[free]);
        assert_eq!(turn.assignment(), &current);
        assert_eq!(turn.cache(), &CacheLocality::Preserved);
    }

    /// Line 513: the same model on another router is a failover.
    #[test]
    fn failover_prefers_the_same_model_on_another_provider() {
        let routing = InteractiveRouting::new();
        let current = session();
        let other_model_first = backend("kilo", "a-different-model");
        let same_model = backend("nous", "the-model");

        let response = routing.on_provider_failure(
            &current,
            ProviderFailure::Unreachable,
            &[other_model_first, same_model],
            &NoObservations,
        );

        match response {
            FailureResponse::FailOver { to, cache, .. } => {
                assert_eq!(to.provider(), "nous");
                assert_eq!(to.backend().model(), &AssignedModel::named("the-model"));
                assert_eq!(
                    cache,
                    CacheLocality::Lost(crate::routing::CacheLossReason::ProviderChanged)
                );
            }
            other => panic!("expected a same-model failover, got {other:?}"),
        }
    }

    /// Line 514: a different model is offered, never taken.
    #[test]
    fn a_different_model_is_offered_as_a_migration_rather_than_taken() {
        let routing = InteractiveRouting::new();
        let current = session();
        let response = routing.on_provider_failure(
            &current,
            ProviderFailure::Refused { status: 503 },
            &[backend("kilo", "a-different-model")],
            &NoObservations,
        );
        match response {
            FailureResponse::OfferMigration { to, .. } => {
                assert_eq!(
                    to.backend().model(),
                    &AssignedModel::named("a-different-model")
                );
            }
            other => panic!("a material model change must not be taken transparently: {other:?}"),
        }
    }

    /// Line 517: a different protocol is never a failover target.
    #[test]
    fn failover_never_crosses_a_protocol() {
        let routing = InteractiveRouting::new();
        let current = session();
        let wrong_protocol = backend_with(
            "nous",
            "the-model",
            "openai-chat",
            ToolSemantics::Unverified,
        );

        let response = routing.on_provider_failure(
            &current,
            ProviderFailure::Unreachable,
            &[wrong_protocol],
            &NoObservations,
        );

        match response {
            FailureResponse::Stay {
                reason: StayReason::NoCompatibleBackend { rejected },
            } => {
                assert_eq!(rejected.len(), 1);
                assert!(matches!(rejected[0], Incompatibility::Protocol { .. }));
            }
            other => panic!("a protocol mismatch must not be failed over to: {other:?}"),
        }
    }

    /// Line 517's quieter half: tool semantics must not go backwards.
    #[test]
    fn failover_never_weakens_what_is_established_about_tool_calls() {
        let routing = InteractiveRouting::new();
        let current = Assignment::new(
            "claude-code",
            backend_with(
                "openrouter",
                "the-model",
                "anthropic-messages",
                ToolSemantics::Verified,
            ),
        );
        let known_absent = backend_with(
            "nous",
            "the-model",
            "anthropic-messages",
            ToolSemantics::KnownAbsent,
        );
        let unverified = backend_with(
            "kilo",
            "the-model",
            "anthropic-messages",
            ToolSemantics::Unverified,
        );

        let response = routing.on_provider_failure(
            &current,
            ProviderFailure::Unreachable,
            &[known_absent, unverified],
            &NoObservations,
        );

        match response {
            FailureResponse::Stay {
                reason: StayReason::NoCompatibleBackend { rejected },
            } => {
                assert_eq!(rejected.len(), 2);
                assert!(
                    rejected
                        .iter()
                        .all(|why| matches!(why, Incompatibility::ToolSemantics { .. }))
                );
            }
            other => panic!("tool semantics must not be weakened by a failover: {other:?}"),
        }
    }

    /// Line 518: a pin turns automatic failover off.
    #[test]
    fn a_pinned_session_does_not_fail_over_even_when_a_perfect_candidate_exists() {
        let routing = InteractiveRouting::pinned_to("openrouter");
        let current = session();
        let perfect = backend("nous", "the-model");

        let response = routing.on_provider_failure(
            &current,
            ProviderFailure::Unreachable,
            &[perfect],
            &NoObservations,
        );

        assert_eq!(
            response,
            FailureResponse::Stay {
                reason: StayReason::SessionPinned {
                    provider: "openrouter".to_owned()
                }
            }
        );
    }

    /// A fixed [`ObservationSource`] test double that answers strong evidence
    /// for one named provider and poor evidence for every other, so a test
    /// can assert *which* candidate a ranking picked rather than only that
    /// ranking ran at all.
    struct FakeEvidence {
        good_provider: &'static str,
    }

    impl ObservationSource for FakeEvidence {
        fn observed(
            &self,
            key: &pairing::EvidenceKey,
        ) -> Option<crate::config::pairing::ObservedEvidence> {
            let provider = key.route().provider.as_deref()?;
            let mut evidence = crate::config::pairing::ObservedEvidence::none();
            evidence.reliable_observation_count = 20;
            if provider == self.good_provider {
                evidence.task_success_rate = Some(1.0);
                evidence.reliability = Some(1.0);
            } else {
                evidence.task_success_rate = Some(0.0);
                evidence.reliability = Some(0.0);
            }
            Some(evidence)
        }
    }

    /// Phase 33A's own consumer, proven decisively: the candidate with real
    /// local evidence behind it wins even though it is not first in the
    /// caller's order — the §35 proof that ranking, not merely "first
    /// compatible candidate", drives this decision. Mutating [`best`] to
    /// return `candidates.remove(0)` unconditionally fails this test.
    #[test]
    fn on_provider_failure_ranks_same_model_survivors_by_local_evidence_not_order() {
        let routing = InteractiveRouting::new();
        let current = session();
        let poor_evidence_first = backend("kilo", "the-model");
        let good_evidence_second = backend("nous", "the-model");

        let response = routing.on_provider_failure(
            &current,
            ProviderFailure::Unreachable,
            &[poor_evidence_first, good_evidence_second],
            &FakeEvidence {
                good_provider: "nous",
            },
        );

        match response {
            FailureResponse::FailOver { to, .. } => {
                assert_eq!(
                    to.provider(),
                    "nous",
                    "the candidate with strong local evidence must win even though it was not \
                     first in the caller's own order"
                );
            }
            other => panic!("expected a same-model failover: {other:?}"),
        }
    }

    /// Line 575: a failover's explanation actually names the pairing class
    /// and cites the evidence behind it — not merely a value nobody reads.
    #[test]
    fn a_failover_explanation_names_the_pairing_class_it_scored() {
        let routing = InteractiveRouting::new();
        let current = Assignment::new("claude-code", backend("openrouter", "claude-fable-5"));
        let candidate = backend("nous", "claude-fable-5");

        let response = routing.on_provider_failure(
            &current,
            ProviderFailure::Unreachable,
            &[candidate],
            &NoObservations,
        );

        match response {
            FailureResponse::FailOver { explanation, .. } => {
                let rendered = explanation.render();
                assert!(
                    rendered.contains("vendor-native"),
                    "claude-code serving claude-fable-5 is a real vendor-native pairing and the \
                     explanation must say so: {rendered}"
                );
            }
            other => panic!("expected a failover: {other:?}"),
        }
    }

    /// A harness slug this build does not recognise degrades to a `0.0`
    /// contribution rather than panicking or silently dropping the
    /// candidate — the failover itself still happens.
    #[test]
    fn on_provider_failure_degrades_when_the_harness_slug_is_not_recognised() {
        let routing = InteractiveRouting::new();
        let current = Assignment::new("some-future-harness", backend("openrouter", "the-model"));
        let candidate = backend("nous", "the-model");

        let response = routing.on_provider_failure(
            &current,
            ProviderFailure::Unreachable,
            &[candidate],
            &NoObservations,
        );

        match response {
            FailureResponse::FailOver {
                to, explanation, ..
            } => {
                assert_eq!(to.provider(), "nous");
                assert!(
                    explanation
                        .render()
                        .contains("not a harness this build recognises"),
                    "{}",
                    explanation.render()
                );
            }
            other => panic!(
                "expected a same-model failover even with an unrecognised harness: {other:?}"
            ),
        }
    }

    /// Line 512: only a provider's own failure may move a session.
    #[test]
    fn a_bad_request_and_a_bad_credential_are_not_provider_failures() {
        assert_eq!(ProviderFailure::from_status(400), None);
        assert_eq!(ProviderFailure::from_status(401), None);
        assert_eq!(ProviderFailure::from_status(403), None);
        assert_eq!(ProviderFailure::from_status(404), None);
        assert_eq!(
            ProviderFailure::from_status(429),
            Some(ProviderFailure::Refused { status: 429 })
        );
        assert_eq!(
            ProviderFailure::from_status(503),
            Some(ProviderFailure::Refused { status: 503 })
        );
    }

    /// Line 511: a migration is taken at a task boundary and not mid-turn.
    #[test]
    fn a_migration_is_refused_mid_turn_and_allowed_between_tasks() {
        let routing = InteractiveRouting::new();
        let current = session();
        let to = backend("nous", "a-different-model");

        assert_eq!(
            routing.migrate(&current, to.clone(), SessionActivity::MidTurn),
            Err(MigrationRefusal::MidTurn)
        );

        let migrated = routing
            .migrate(&current, to, SessionActivity::Idle)
            .expect("a compatible backend at a task boundary");
        assert_eq!(migrated.provider(), "nous");
        assert_eq!(migrated.harness(), "claude-code");
    }

    /// A pin refuses an explicit migration away from it, and says so.
    #[test]
    fn a_pin_refuses_a_migration_rather_than_being_overridden_by_one() {
        let routing = InteractiveRouting::pinned_to("openrouter");
        let current = session();
        let err = routing
            .migrate(
                &current,
                backend("nous", "the-model"),
                SessionActivity::Idle,
            )
            .expect_err("a pinned session refuses a migration away from the pin");
        assert_eq!(
            err,
            MigrationRefusal::SessionPinned {
                provider: "openrouter".to_owned()
            }
        );
        assert!(err.to_string().contains("lift the pin"));
    }

    /// Line 515 and 516 together: the record says what moved, and carries the
    /// cache warning when there is one.
    #[test]
    fn a_recorded_failover_names_what_changed_and_warns_about_the_cache() {
        let mut record = RoutingRecord::new();
        let from = session();
        let to = Assignment::new("claude-code", backend("nous", "the-model"));
        let cache = CacheLocality::between(from.backend(), to.backend());

        record.note(AssignmentChange {
            from,
            to,
            cause: ChangeCause::Failover(ProviderFailure::Unreachable),
            cache,
        });

        let entry = &record.entries()[0];
        assert!(entry.changed_provider_or_model());
        let warning = entry.cache_warning().expect("a provider change warns");
        assert!(warning.contains("invalidated"));
    }
}
