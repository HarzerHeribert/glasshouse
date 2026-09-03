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
//!
//! Every candidate also carries a **failure-domain diversity** contribution
//! (Phase 33C, `failure_domain_contribution`): a candidate sharing the
//! failed backend's provider is penalised, because `Backend` carries no base
//! URL and the provider is the only honest proxy this build has for "lands
//! on the same infrastructure" (see [`super::domain::FailureDomain`]). A
//! different provider scores `0.0` rather than a bonus — line 1378 forbids
//! rewarding a candidate for independence nothing has established.

use crate::config::pairing::{
    ContinuitySource, ObservationSource, PairingPreference, native_pairing_prior_contribution,
    session_continuity_contribution,
};
use crate::harness::Declared;
use crate::harness::pairing;
use crate::integrations::IntegrationId;
use crate::routing::{HardConstraint, apply_hard_constraints};

use super::domain::FailureDomain;
use super::evidence::{CorrelationVerdict, RouteCorrelations, RouteIdentity};
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
        /// Map line 1851: what the failure-domain term did to the ranking
        /// that produced `to` — see [`FailureDomainEffect`].
        domain_effect: FailureDomainEffect,
    },
    /// A compatible backend exists, but it serves a **different model**, so
    /// taking it would be a migration rather than a transparent failover.
    /// Line 514: offered, never taken.
    OfferMigration {
        to: Assignment,
        cache: CacheLocality,
        /// The same explanation [`Self::FailOver`]'s own field carries.
        explanation: RoutingExplanation,
        /// The same effect [`Self::FailOver`]'s own field carries, over the
        /// migration candidates. Computed identically and **not recorded**:
        /// line 1851 counts failovers, and a migration is offered rather
        /// than taken, so a row here would put a move nobody made into the
        /// denominator of how often a move was steered.
        domain_effect: FailureDomainEffect,
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
/// Everything [`InteractiveRouting::start`] weighs that is not a candidate:
/// the user's resolved pairing configuration and the two sources of local
/// knowledge about a pairing.
///
/// One struct rather than four arguments because these four always travel
/// together and are resolved together — `crate::gateway::session::SessionRouting`
/// already holds the first two on its own `State` (see
/// `SessionRouting::set_pairing_preference`), and the day a session-start
/// caller exists it will hold all four. A caller assembling them one at a
/// time at the call site is a caller that can silently pass last session's
/// preference with this session's evidence.
pub struct SessionStartInputs<'a> {
    /// Line 576: the native-pairing preference the user configured, resolved
    /// by `crate::config::EffectiveConfig` and carried here by the caller.
    pub preference: PairingPreference,
    /// Line 561: the user's own corrections to pairing metadata.
    pub overrides: &'a pairing::PairingOverrides,
    /// Phase 33A: what has actually been observed about each candidate.
    pub evidence: &'a dyn ObservationSource,
    /// Line 569: which candidates a relevant warm session already exists for.
    pub continuity: &'a dyn ContinuitySource,
}

impl std::fmt::Debug for SessionStartInputs<'_> {
    /// Hand-written because neither source is [`Debug`]: a trait object is
    /// whatever the caller implemented, and requiring `Debug` of it would
    /// push a derive onto every future session store and ledger for the sake
    /// of one diagnostic line.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionStartInputs")
            .field("preference", &self.preference)
            .field("overrides", self.overrides)
            .finish_non_exhaustive()
    }
}

/// What [`InteractiveRouting::start`] decided for a session that is starting,
/// and why.
///
/// The same shape as [`TurnRouting`] — an assignment plus the one thing that
/// makes it inspectable — and deliberately not an [`Assignment`] on its own.
/// Map line 575 asks for the pairing class, the evidence strength and the
/// prior's contribution to be *surfaced in routing explanations*; a session
/// start that returned only its answer would have computed all three and
/// thrown them away at the one moment a person is most likely to ask "why
/// this backend?".
#[derive(Debug, Clone, PartialEq)]
pub struct SessionStart {
    assignment: Assignment,
    explanation: RoutingExplanation,
}

impl SessionStart {
    pub fn assignment(&self) -> &Assignment {
        &self.assignment
    }

    /// Every named contribution behind this choice, in the order they were
    /// weighed. [`RoutingExplanation::render`] is what a diagnostic prints.
    pub fn explanation(&self) -> &RoutingExplanation {
        &self.explanation
    }

    pub fn into_assignment(self) -> Assignment {
        self.assignment
    }
}

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

    /// Map lines 566 and 569: which of several eligible backends a **fresh**
    /// session starts on, and the full explanation of why.
    ///
    /// [`Self::assign`] above is the older, narrower entry point: the caller
    /// had already chosen, and `assign` recorded the choice. This one is the
    /// caller *asking*. It exists because line 566 asks for a positive
    /// **initial** routing prior — "initial" is a moment, and until this
    /// function there was no moment in Glasshouse where a starting session
    /// compared two backends. `crate::gateway::session::SessionRouting::bind`
    /// took `Upstream::serving()`, the first configured backend, and its own
    /// doc said so: *"Nothing here chooses; the choice was made when the
    /// upstream was built."*
    ///
    /// # What is weighed, and in what order
    ///
    /// 1. **Hard constraints first** (line 568), through
    ///    [`apply_hard_constraints`] and therefore structurally rather than
    ///    by convention: a session pin is a
    ///    [`HardConstraint::UserConstraint`] and removes a candidate outright.
    ///    Unlike `score_candidate`'s own receipt-shaped call, this check can
    ///    actually reject, so the [`EligibleCandidate`](crate::routing::EligibleCandidate)s
    ///    below are a filter's output and not a formality.
    /// 2. **The native-pairing prior** (line 566) and **local observed
    ///    evidence** (Phase 33A), from `score_candidate` — the same function,
    ///    unchanged, that [`Self::on_provider_failure`] already scores
    ///    failover survivors with.
    /// 3. **Session continuity** (line 569), from
    ///    [`session_continuity_contribution`] — bounded, never negative, and
    ///    on the prior's own scale so that `best` can weigh the two against
    ///    each other by simple sum. That is what "commensurable" has to mean
    ///    here: not that a warm session is compared to a prior by a special
    ///    rule, but that neither term knows the other exists and
    ///    [`RoutingExplanation::total`] adds them up.
    ///
    /// A candidate is never *excluded* by any of steps 2 or 3 — only step 1
    /// can remove one, and only for a constraint the user or the protocol
    /// imposed. That is design decision 1 ("additive, never a filter") at the
    /// one caller where a prior could most easily have been written as
    /// `if native { return it }`.
    ///
    /// # What a build with nothing configured does
    ///
    /// Exactly what it did before this function existed. With
    /// [`NoObservations`](crate::config::pairing::NoObservations),
    /// [`NoWarmSessions`](crate::config::pairing::NoWarmSessions) and no
    /// vendor-native candidate, every contribution is `0.0`, every candidate
    /// ties, and `best` keeps the first — which is `Upstream::backends()`'
    /// own order, which is the user's configuration order.
    ///
    /// # What this function cannot decide, and it is not a gap here
    ///
    /// **The native-pairing prior is constant across every candidate set the
    /// shipped binary can build**, so at this caller it contributes a real
    /// number to every explanation and separates nothing.
    /// [`pairing::classify`] reads `query.route` exactly once, and only to
    /// compute `Pairing::protocol_fit` — a field
    /// `native_pairing_prior_contribution` never looks at — so
    /// [`pairing::PairingClass`] is a function of the harness, the model and
    /// the user's corrections alone. A session start's candidates are
    /// `crate::gateway::upstream::Upstream::backends`, which carry a
    /// provider, a credential and a cost and **no model**: the one model
    /// comes from the launch profile and is the same for all of them. Same
    /// harness plus same model means same class means same prior.
    ///
    /// `tests::the_native_pairing_prior_is_constant_across_a_real_session_start_candidate_set`
    /// holds that as an executable fact rather than a comment. What *does*
    /// separate candidates here is local evidence and session continuity,
    /// both of which are keyed by [`pairing::EvidenceKey`] and therefore vary
    /// with the route.
    ///
    /// `None` only when `candidates` is empty: there is nothing to start on,
    /// and `best` may not be called with nothing.
    pub fn start(
        &self,
        harness: &str,
        launch_profile: &str,
        candidates: &[Backend],
        inputs: &SessionStartInputs<'_>,
    ) -> Option<SessionStart> {
        if candidates.is_empty() {
            return None;
        }

        // Line 568, before anything is scored. The pin is the only hard
        // constraint a *starting* session has that this policy can decide:
        // protocol and tool semantics are `compatible()`'s question, and
        // that compares a candidate against a current backend, which a
        // session that has not started yet does not have.
        let (eligible, rejected) =
            apply_hard_constraints(candidates.to_vec(), |candidate: &Backend| {
                if self.pin.permits(candidate.provider()) {
                    Ok(())
                } else {
                    Err(HardConstraint::UserConstraint)
                }
            });

        // A pin naming a provider none of the configured backends serve
        // would otherwise leave nothing to start on. Refusing the launch
        // over it would be worse than starting somewhere and saying so, and
        // silently dropping the pin would be worse than both — so the pin is
        // reported as unappliable, in the explanation, on every candidate.
        let pin_eliminated_everything = eligible.is_empty();
        let scored_candidates: Vec<Backend> = if pin_eliminated_everything {
            rejected
                .into_iter()
                .map(|(candidate, _)| candidate)
                .collect()
        } else {
            eligible
                .into_iter()
                .map(crate::routing::EligibleCandidate::into_inner)
                .collect()
        };

        let harness_id = resolve_harness(harness);
        let mut scored: Vec<(Assignment, RoutingExplanation)> =
            Vec::with_capacity(scored_candidates.len());
        for candidate in scored_candidates {
            let mut explanation = match harness_id {
                Some(id) => score_candidate(
                    id,
                    launch_profile,
                    &candidate,
                    inputs.preference,
                    inputs.overrides,
                    inputs.evidence,
                ),
                None => unrecognised_harness_explanation(harness),
            };
            if let Some(id) = harness_id {
                // Line 569. Pushed here rather than inside `score_candidate`
                // because `on_provider_failure` deliberately does not weigh
                // continuity: the backend that just failed is the one the
                // session was warm on, and crediting a *replacement* for a
                // warmth it does not have would be an invention. A fresh
                // session's candidates can each honestly hold one.
                explanation.push(session_continuity_contribution(
                    &evidence_key_for(id, launch_profile, &candidate),
                    inputs.continuity,
                ));
            }
            if pin_eliminated_everything {
                explanation.push(Contribution::new(
                    "session pin",
                    0.0,
                    format!(
                        "this session is pinned to `{}`, which none of the configured backends \
                         serve — the pin could not be applied, and every candidate was scored \
                         instead of the session being refused a backend",
                        self.pin.provider().unwrap_or("<unset>")
                    ),
                ));
            }
            scored.push((Assignment::new(harness, candidate), explanation));
        }

        // No candidate here carries a failure-domain term — nothing has
        // failed — so the second ranking `best` computes is the first one,
        // and the effect it returns is always *not prevented*. Discarded
        // rather than plumbed: line 1851 counts failovers.
        let (assignment, explanation, _) = best(scored);
        Some(SessionStart {
            assignment,
            explanation,
        })
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
    ///
    /// `preference` and `overrides` are Phase 9J line 576's own patch: the
    /// user's configured native-pairing preference and corrections, resolved
    /// once from configuration by `crate::profile`'s gateway path and carried
    /// here by `crate::gateway::session::SessionRouting`, which is why this
    /// method takes them as arguments rather than storing them on `self` —
    /// `self.pin` is session *policy* state that a pin or an unpin replaces
    /// wholesale, while a resolved preference must survive that replacement
    /// unchanged.
    ///
    /// `correlations` is Phase 33C lines 1370–1376's answer to *do these
    /// two front doors fail together* — read off the same ledger as
    /// `evidence`, by the same caller, and passed beside it for the same
    /// reason: this function stays pure. [`RouteCorrelations::default`]
    /// (every pair unmeasured) reproduces the ranking exactly as it was
    /// before that package, which is what every test here that passes it
    /// checks.
    #[allow(clippy::too_many_arguments)]
    pub fn on_provider_failure(
        &self,
        current: &Assignment,
        failure: ProviderFailure,
        candidates: &[Backend],
        preference: PairingPreference,
        overrides: &pairing::PairingOverrides,
        evidence: &dyn ObservationSource,
        correlations: &RouteCorrelations,
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
                    let mut explanation = match harness {
                        // A failover has no launch profile name to key
                        // evidence by — see `score_candidate`'s own doc
                        // comment — so it passes the empty one it has always
                        // effectively used.
                        Some(harness) => score_candidate(
                            harness,
                            NO_LAUNCH_PROFILE,
                            candidate,
                            preference,
                            overrides,
                            evidence,
                        ),
                        None => unrecognised_harness_explanation(current.harness()),
                    };
                    // Phase 33C lines 1375 and 1547: failure-domain
                    // diversity is a ranking signal in its own right, named
                    // and evidenced like every other contribution here — see
                    // `failure_domain_contribution`'s own doc comment.
                    explanation.push(failure_domain_contribution(current.backend(), candidate));
                    // Phase 33C lines 1370–1376: what the ledger has
                    // *measured* about this pair failing together, as its
                    // own term beside the provider-identity one — see
                    // `route_correlation_contribution`.
                    if let Some(contribution) =
                        route_correlation_contribution(current.backend(), candidate, correlations)
                    {
                        explanation.push(contribution);
                    }
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
            let (to, cache, explanation, domain_effect) = ranked_with_cache(current, same_model);
            return FailureResponse::FailOver {
                to,
                cache,
                explanation,
                domain_effect,
            };
        }

        if !migration.is_empty() {
            let (to, cache, explanation, domain_effect) = ranked_with_cache(current, migration);
            return FailureResponse::OfferMigration {
                to,
                cache,
                explanation,
                domain_effect,
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
/// `preference` and `overrides` are the caller's own resolved configuration —
/// Phase 9J line 576's patch. `on_provider_failure` receives them as
/// arguments and this function never looks them up itself, matching every
/// other value this module reads: it stays a pure function of what it is
/// given.
///
/// One thing this consumer still does not have, and degrades honestly rather
/// than inventing: **the candidate's protocol as a
/// [`crate::harness::WireProtocol`]** — [`Backend::protocol`] is deliberately
/// kept as an opaque slug (see that method's own doc comment);
/// [`pairing::wire_protocol_from_slug`] is the reverse lookup, and it answers
/// `None` for a slug it does not recognise rather than guessing, which only
/// weakens `Pairing::protocol_fit`, a field `native_pairing_prior_contribution`
/// never reads.
fn score_candidate(
    harness: IntegrationId,
    launch_profile: &str,
    candidate: &Backend,
    preference: PairingPreference,
    overrides: &pairing::PairingOverrides,
    evidence: &dyn ObservationSource,
) -> RoutingExplanation {
    let route = serving_route(candidate);
    let query = pairing::PairingQuery {
        harness,
        model: candidate.model().clone(),
        route,
        // Not the `Declared<bool>` evidence string `crate::routing::Backend`
        // was built from — `routing` never keeps it, see `Backend::tools`'
        // own doc comment — and `classify` uses this only for
        // `Pairing::tool_semantics`, which `native_pairing_prior_contribution`
        // never reads either.
        tool_calls: Declared::Unverified,
        provider_protocols: Vec::new(),
    };
    let pairing_value = pairing::classify(&query, overrides);

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

    let key = evidence_key_for(harness, launch_profile, candidate);

    native_pairing_prior_contribution(&eligible, &key, preference, evidence)
}

/// The launch profile name a caller that genuinely has none passes.
///
/// `crate::gateway::session::SessionRouting`'s failover path is that caller:
/// a bound assignment carries the harness, the protocol and the model, and no
/// profile name. Named rather than written as `""` at the call site so that
/// "this caller has no profile" and "this profile is called the empty string"
/// are not the same three characters. `ObservedEvidenceSource` does not read
/// the field at all (see `routing::evidence`'s own header for why), so this
/// costs that source nothing; a continuity source, which distinguishes
/// sessions, is handed a real name by [`InteractiveRouting::start`].
const NO_LAUNCH_PROFILE: &str = "";

/// The route a [`Backend`] describes, as the pairing model's own type.
///
/// `protocol` degrades to `None` for a slug this build does not recognise
/// rather than guessing — [`Backend::protocol`] is deliberately an opaque
/// slug, and [`pairing::wire_protocol_from_slug`] is the one reverse lookup.
fn serving_route(candidate: &Backend) -> pairing::ServingRoute {
    pairing::ServingRoute {
        provider: Some(candidate.provider().to_owned()),
        gateway: None,
        protocol: pairing::wire_protocol_from_slug(candidate.protocol()),
    }
}

/// The [`pairing::EvidenceKey`] naming exactly one harness, launch profile,
/// model and backend combination — map line 572's four axes, and the key both
/// [`ObservationSource`] and [`ContinuitySource`] are asked with.
///
/// One function so the two sources are always asked the *same* question. Two
/// call sites building the key independently is how a warm session for one
/// route ends up credited to another.
fn evidence_key_for(
    harness: IntegrationId,
    launch_profile: &str,
    candidate: &Backend,
) -> pairing::EvidenceKey {
    pairing::EvidenceKey::new(
        harness,
        launch_profile,
        candidate.model().clone(),
        serving_route(candidate),
    )
}

/// The explanation for a candidate whose harness slug this build does not
/// know: no pairing could be classified, so the prior is `0.0` and says why.
///
/// Shared by [`InteractiveRouting::start`] and
/// [`InteractiveRouting::on_provider_failure`] so that an unrecognised
/// harness degrades identically at both callers rather than in two places
/// that could drift.
fn unrecognised_harness_explanation(harness: &str) -> RoutingExplanation {
    let mut explanation = RoutingExplanation::new();
    explanation.push(Contribution::new(
        "native-pairing prior",
        0.0,
        format!(
            "`{harness}` is not a harness this build recognises, so no pairing could be \
             classified for it"
        ),
    ));
    explanation
}

/// Phase 33C lines 1375 and 1547: what failure-domain diversity contributes
/// to ranking `candidate` against the backend that just failed.
///
/// A magnitude comparable to the native-pairing prior's own scale
/// (`PriorStrength::Strong` peaks at `1.0` — see `crate::config::pairing`),
/// large enough to actually move [`best`]'s decision (acceptance test 1's
/// whole point) and never positive: sharing the failed backend's provider
/// can only ever cost a candidate something, never earn it one, because
/// "known shared" is the one thing this signal is ever certain about.
/// [`FailureDomain::Unknown`] scores exactly `0.0` — not a bonus for being
/// on a different provider, only the absence of the penalty, per line 1378.
const SHARED_FAILURE_DOMAIN_PENALTY: f64 = -1.0;

fn failure_domain_contribution(current: &Backend, candidate: &Backend) -> Contribution {
    match FailureDomain::between(current, candidate) {
        FailureDomain::Shared => Contribution::new(
            FAILURE_DOMAIN_TERM,
            SHARED_FAILURE_DOMAIN_PENALTY,
            format!(
                "`{}` shares its provider with the backend that just failed, which is the only \
                 failure-domain signal this build can observe — this candidate cannot be \
                 credited with resilience against the failure that just happened",
                candidate.provider()
            ),
        ),
        FailureDomain::Unknown | FailureDomain::Independent => Contribution::new(
            FAILURE_DOMAIN_TERM,
            0.0,
            format!(
                "`{}` is on a different provider than the backend that failed, but independence \
                 is not established — Glasshouse has no correlation evidence for this pair, and \
                 absent evidence is not treated as independence",
                candidate.provider()
            ),
        ),
    }
}

/// The name [`failure_domain_contribution`] gives its [`Contribution`], and
/// the key [`best`] removes to rank a second time.
///
/// Spelled once because two spellings would silently make the comparison
/// below a comparison of a ranking against itself, which always answers
/// *not prevented* and would look exactly like a correct measurement.
const FAILURE_DOMAIN_TERM: &str = "failure-domain diversity";

/// The name [`route_correlation_contribution`] gives its [`Contribution`],
/// and the key [`best`] removes to rank a third time — capability map line
/// 1852's derivation, with the same one-spelling rule as
/// [`FAILURE_DOMAIN_TERM`] and for the same reason.
const ROUTE_CORRELATION_TERM: &str = "route correlation";

/// The `(provider, model)` a backend is observed under in the evidence
/// ledger — the same two strings `gateway::session` writes on every row.
fn route_of(backend: &Backend) -> RouteIdentity {
    RouteIdentity::new(backend.provider(), backend.model().label())
}

/// Capability map lines 1370, 1373, 1374 and 1376 at the one place a
/// correlation changes a decision: what the ledger has **measured** about
/// `candidate` failing at the same moments as the backend that just failed.
///
/// # A sibling of `failure_domain_contribution`, not a change to it
///
/// [`FailureDomain::between`] is a certainty about provider identity and
/// stays exactly what it was; this term is evidence about behaviour, and it
/// is only ever consulted for a pair that identity calls
/// [`FailureDomain::Unknown`]. A same-provider candidate gets [`None`] here:
/// the provider term already carries the whole penalty, and a second term
/// for the same fact would count it twice. Keeping the two terms apart is
/// also what keeps line 1851's count meaning what `glasshouse route` prints
/// beside it — *steered off a candidate sharing the failed backend's
/// provider* — while line 1852 is derived from this term alone.
///
/// # What the magnitude is
///
/// [`RouteCorrelation::confidence`] scaled by [`SHARED_FAILURE_DOMAIN_PENALTY`]:
/// a pair observed failing together every time is penalised exactly as a
/// shared provider is, a pair that never did is penalised nothing, and a
/// pair between moves between — line 1374's "confidence-weighted", with the
/// weight recomputed from the rows on every failover rather than stored.
///
/// # Line 1376
///
/// Below [`super::evidence::MIN_CORRELATION_SAMPLE`] events the term is
/// `0.0` — indistinguishable in the ranking from no correlation at all —
/// and its detail says how many of how many, so the explanation `glasshouse
/// route` prints names the sample size before anything reads as meaningful.
fn route_correlation_contribution(
    current: &Backend,
    candidate: &Backend,
    correlations: &RouteCorrelations,
) -> Option<Contribution> {
    if FailureDomain::between(current, candidate) == FailureDomain::Shared {
        return None;
    }
    let failed = route_of(current);
    let route = route_of(candidate);
    let correlation = correlations.between(&failed, &route);
    Some(match correlation.verdict() {
        CorrelationVerdict::InsufficientEvidence {
            sample_size,
            required,
        } => Contribution::new(
            ROUTE_CORRELATION_TERM,
            0.0,
            format!(
                "`{route}` and `{failed}` have been observed at the same moment in {sample_size} \
                 of the {required} failures a correlation needs — insufficient evidence, \
                 treated as no correlation"
            ),
        ),
        CorrelationVerdict::Measured {
            confidence,
            sample_size,
        } => Contribution::new(
            ROUTE_CORRELATION_TERM,
            correlation_penalty(confidence),
            format!(
                "`{route}` failed the same way as `{failed}` at the same moment in {} of \
                 {sample_size} observed failures — correlation {confidence:.2}, weighed as that \
                 share of a shared provider's penalty",
                correlation.overlaps()
            ),
        ),
    })
}

/// [`SHARED_FAILURE_DOMAIN_PENALTY`] scaled by a confidence in `[0, 1]`,
/// with a zero confidence yielding `0.0` rather than IEEE's `-0.0` so an
/// explanation never prints a signed nothing.
fn correlation_penalty(confidence: f64) -> f64 {
    let penalty = SHARED_FAILURE_DOMAIN_PENALTY * confidence;
    if penalty == 0.0 { 0.0 } else { penalty }
}

/// What the failure-domain term did to one ranking — **capability map line
/// 1851**, derived rather than decided.
///
/// # Why this is a derivation and not a rejection
///
/// Design decision 1 makes failure-domain diversity *additive, never a
/// filter*: `failure_domain_contribution` is a `-1.0` term inside an
/// explanation, and nothing anywhere refuses a candidate for sharing the
/// failed backend's provider. So no production code path *decides* that a
/// failover was prevented, and inventing one would change the policy in
/// order to measure it.
///
/// What can be established honestly is a comparison: rank the survivors
/// once as production does, and once with that one term's magnitude removed.
/// If the winners differ, the term is what moved the decision.
///
/// # The displaced candidate always shares the failed provider
///
/// This is a property of the arithmetic rather than a claim. Every
/// candidate's score differs between the two rankings by its own
/// failure-domain magnitude, which is `0.0` for every candidate except one
/// on the failed backend's own provider, where it is
/// `SHARED_FAILURE_DOMAIN_PENALTY`. A candidate scoring `0.0` for that
/// term therefore has the same total in both rankings, while every other
/// candidate's total can only be lower in the production ranking — so a
/// `0.0` winner of the term-free ranking still wins the production one, with
/// `best`'s first-seen tie-break unchanged because both rankings walk the
/// same order. A winner that *changes* is therefore always a candidate that
/// shared the upstream, which is exactly the map line's *"failover onto the
/// same unhealthy upstream"*.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailureDomainEffect {
    displaced: Option<String>,
    /// Capability map line 1852, derived the same way one paragraph up:
    /// the route the **correlation** term displaced, when removing that one
    /// term alone would have made it win. Always a candidate on a
    /// different provider than the failed backend's — the only kind that
    /// term ever scores — which is exactly the line's *"nominally different
    /// routes"* that turned out to share failure resilience.
    correlation_displaced: Option<RouteIdentity>,
}

impl FailureDomainEffect {
    /// The term changed which candidate won, and this is the label of the one
    /// it displaced.
    pub fn prevented(&self) -> bool {
        self.displaced.is_some()
    }

    /// The candidate that would have won without the term — [`None`] when
    /// the term changed nothing.
    ///
    /// **`provider/model`, and deliberately not [`Assignment::label`].** That
    /// label names the credential *reference* too, which every log line here
    /// already carries and which this value must not: it travels into
    /// `crate::evaluation`'s durable ledger, where the rule is ids and
    /// vocabulary only. The provider and the model are the whole of what
    /// *"the same unhealthy upstream"* means, so the measurement loses
    /// nothing by leaving the rest out.
    pub fn displaced(&self) -> Option<&str> {
        self.displaced.as_deref()
    }

    /// Line 1852: the correlation term changed which candidate won.
    pub fn correlation_steered(&self) -> bool {
        self.correlation_displaced.is_some()
    }

    /// The route the correlation term steered this failover off — a route
    /// on a different provider whose observed failures overlap the failed
    /// backend's — or [`None`] when that term changed nothing.
    pub fn correlation_displaced(&self) -> Option<&RouteIdentity> {
        self.correlation_displaced.as_ref()
    }
}

/// The best-scoring `(Assignment, RoutingExplanation)` in `candidates`,
/// preferring the first one seen on a tie — the caller's own order. A build
/// with no evidence source reproduces the pre-batch-46 "first compatible
/// candidate" behaviour exactly this way: every contribution is `0.0` with
/// nothing to weigh, so every candidate ties and the first stands.
///
/// Returns the winner and, beside it, what the failure-domain term did to
/// this ranking — map line 1851. The second ranking is over the same vector
/// in the same order with only that term's magnitude subtracted, so the two
/// differ in exactly one input and nothing else.
///
/// Panics on an empty `candidates` — both call sites only reach this after
/// checking `!candidates.is_empty()`.
fn best(
    mut candidates: Vec<(Assignment, RoutingExplanation)>,
) -> (Assignment, RoutingExplanation, FailureDomainEffect) {
    let best_index = argmax(&candidates, |explanation| explanation.total());
    // The same ranking with the one term's magnitude taken back out. Not a
    // re-score: `score_candidate` is not called again, so nothing else about
    // the comparison can differ.
    let without_index = argmax(&candidates, |explanation| {
        explanation.total() - failure_domain_magnitude(explanation)
    });
    let displaced = (without_index != best_index).then(|| {
        let backend = candidates[without_index].0.backend();
        format!("{}/{}", backend.provider(), backend.model().label())
    });
    // Line 1852, by the same construction with the other term: the same
    // vector, the same order, only the correlation term's magnitude taken
    // back out. The provider-identity term stays in both rankings, so this
    // comparison isolates what the *measured* correlation did.
    let without_correlation_index = argmax(&candidates, |explanation| {
        explanation.total() - route_correlation_magnitude(explanation)
    });
    let correlation_displaced = (without_correlation_index != best_index)
        .then(|| route_of(candidates[without_correlation_index].0.backend()));
    let (assignment, explanation) = candidates.swap_remove(best_index);
    (
        assignment,
        explanation,
        FailureDomainEffect {
            displaced,
            correlation_displaced,
        },
    )
}

/// The setup both [`FailureResponse::FailOver`] and
/// [`FailureResponse::OfferMigration`] need: the best-ranked candidate from
/// [`best`], plus the cache locality of moving to it from `current`. Shared
/// because the two arms of [`InteractiveRouting::on_provider_failure`] built
/// this identically before this extraction — they differ only in which
/// variant wraps the result.
fn ranked_with_cache(
    current: &Assignment,
    candidates: Vec<(Assignment, RoutingExplanation)>,
) -> (
    Assignment,
    CacheLocality,
    RoutingExplanation,
    FailureDomainEffect,
) {
    let (to, explanation, domain_effect) = best(candidates);
    let cache = CacheLocality::between(current.backend(), to.backend());
    (to, cache, explanation, domain_effect)
}

/// The index of the highest `score`, preferring the first on a tie — the
/// caller's own order, which is what makes two rankings over one vector
/// comparable.
fn argmax(
    candidates: &[(Assignment, RoutingExplanation)],
    score: impl Fn(&RoutingExplanation) -> f64,
) -> usize {
    let mut best_index = 0;
    let mut best_total = score(&candidates[0].1);
    for (index, (_, explanation)) in candidates.iter().enumerate().skip(1) {
        let total = score(explanation);
        if total > best_total {
            best_total = total;
            best_index = index;
        }
    }
    best_index
}

/// What [`failure_domain_contribution`] put into this explanation, summed —
/// `0.0` for an explanation that carries no such term at all, which is every
/// explanation built anywhere but [`InteractiveRouting::on_provider_failure`].
fn failure_domain_magnitude(explanation: &RoutingExplanation) -> f64 {
    explanation
        .contributions()
        .iter()
        .filter(|contribution| contribution.name() == FAILURE_DOMAIN_TERM)
        .map(Contribution::magnitude)
        .sum()
}

/// What [`route_correlation_contribution`] put into this explanation,
/// summed — `0.0` when the pair was same-provider, unmeasured, or below the
/// minimum sample, and for every explanation built anywhere but
/// [`InteractiveRouting::on_provider_failure`].
fn route_correlation_magnitude(explanation: &RoutingExplanation) -> f64 {
    explanation
        .contributions()
        .iter()
        .filter(|contribution| contribution.name() == ROUTE_CORRELATION_TERM)
        .map(Contribution::magnitude)
        .sum()
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

/// What Phase 33C line 1377 asks every recorded [`AssignmentChange`] to
/// answer honestly: which domain(s) actually changed, computed from the two
/// backends the change is between — never invented from [`ChangeCause`]
/// alone, because a rotation and a failover can carry different causes and
/// still need the same honest answer about what they bought.
///
/// Two variants, not the map line's full four ("independent capacity,
/// independent quota, independent failure handling, or merely a different
/// queue onto the same upstream"): a quota-domain change is certain — two
/// [`super::CredentialId`]s are either the same allowance or they are not,
/// by construction — but this build has no producer for a *capacity* signal
/// (Phase 32G/33, both 0/N per `docs/product/evidence/phase-35b.md`'s own
/// missing-evidence list) and line 1378 forbids ever calling a cross-provider
/// move "independent failure handling" outright, proven or not. Reporting a
/// category this build cannot honestly support would be exactly the
/// "invent a source" mistake Phase 35B's own worker refused for the pairing
/// prior on a disposable candidate — see that phase's evidence entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingBenefit {
    /// The provider changed. The failure domain moved from
    /// [`FailureDomain::Shared`] (certain) to [`FailureDomain::Unknown`]
    /// (never claimed as [`FailureDomain::Independent`]) — and, since a
    /// different provider always means a different credential too, the
    /// quota domain changed as well.
    UnconfirmedFailureDomainChange,
    /// The provider did not change; the credential did. Line 1372's exact
    /// case: the quota domain changed — a real, certain gain — and the
    /// failure domain did not, so this is never resilience against the
    /// failure that just happened.
    DifferentQueueSameUpstream,
    /// Neither changed. Not reachable from any production caller today — an
    /// [`AssignmentChange`] is only ever recorded when something moved —
    /// kept so this type stays honest about what "nothing changed" would
    /// mean rather than making it unrepresentable.
    NoChange,
}

impl RoutingBenefit {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UnconfirmedFailureDomainChange => {
                "a different provider, and therefore an unconfirmed failure domain — no evidence \
                 establishes independence"
            }
            Self::DifferentQueueSameUpstream => {
                "the same provider's other credential: a different queue onto the same upstream, \
                 not independent failure handling"
            }
            Self::NoChange => "neither the provider nor the credential changed",
        }
    }
}

impl std::fmt::Display for RoutingBenefit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
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

    /// Line 1377: which domain(s) this change actually bought, computed from
    /// the two backends rather than from `cause` — see [`RoutingBenefit`]'s
    /// own doc comment for why `cause` alone cannot answer this honestly.
    pub fn benefit(&self) -> RoutingBenefit {
        let domain = FailureDomain::between(self.from.backend(), self.to.backend());
        let credential_changed = self.from.backend().credential() != self.to.backend().credential();
        match (domain, credential_changed) {
            (FailureDomain::Shared, true) => RoutingBenefit::DifferentQueueSameUpstream,
            (FailureDomain::Shared, false) => RoutingBenefit::NoChange,
            (FailureDomain::Unknown | FailureDomain::Independent, _) => {
                RoutingBenefit::UnconfirmedFailureDomainChange
            }
        }
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
            benefit = %change.benefit(),
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
mod tests;
