//! Phase 37 — the basic session-aware router: which *destination* a piece of
//! work goes to, and why.
//!
//! # What makes this a different router from the two beside it
//!
//! [`super::interactive`] answers "which backend serves this session", and
//! [`super::disposable`] answers "which resource serves this throwaway job".
//! Both rank **backends**. This module ranks **destinations**, and a
//! destination is a strictly larger thing: an existing session that could be
//! continued, or a fresh session that would have to be started. Map lines
//! 1593 and 1594 are that comparison and nothing else — *"prefer an existing
//! relevant session"* against *"prefer a fresh session"* — and neither can
//! be expressed by a policy whose candidates are all backends.
//!
//! That difference is also what makes the six `Consider X` lines (1595–1600)
//! answerable here when their equivalents were not answerable one layer down.
//! `docs/product/evidence/phase-9j.md`'s last entry records why: a signal
//! that is **constant across the candidate set** cannot change a ranking, and
//! every candidate set `crate::gateway::upstream::Upstream` can build varies
//! only by route, because [`Backend`] carries a provider, a credential and a
//! cost and no harness and no model of its own. A [`Destination`] carries its
//! own [`IntegrationId`] and its own [`Continuation`], so a candidate set here
//! genuinely varies along harness, warmth, cache locality, credential and
//! bootstrap cost — the six axes the six lines name. Every contribution below
//! has a test that holds two destinations differing **only** in that axis and
//! asserts they resolve differently; a contribution that could not do that
//! would be dead weight, and saying so is the finding rather than the failure.
//!
//! # The one thing this module deliberately does not weigh
//!
//! The native-pairing prior (line 566). It is constant across every candidate
//! set the shipped binary builds — see the entry cited above — and adding a
//! term that separates nothing to an explanation a person reads is worse than
//! leaving it out. [`harness_capability_fit`] reads `classify`'s *capability*
//! axes (protocol fit, model-behaviour fit, tool semantics), which do vary
//! with the harness, and not its vendor class, which does not vary with
//! anything this router can change.
//!
//! # Purity
//!
//! Same rule as the rest of `routing`: no socket, no credential resolution,
//! no clock. `now` is an argument. Warmth, capacity and checkpoint quality
//! are values the **caller looked up** — this module names neither
//! `crate::session` nor `crate::checkpoint`, for the reason
//! [`crate::config::pairing::ContinuitySource`] gives.

use std::time::Instant;

use crate::config::pairing::{ContinuitySource, WarmSession};
use crate::harness::WireProtocol;
use crate::harness::pairing::{self, ModelBehaviourFit, ProtocolFit};
use crate::integrations::IntegrationId;
use crate::provider::quota::RemainingCapacityScore;

use super::free::{FreePool, FreeResource};
use super::{
    Backend, CacheLocality, Contribution, HardConstraint, RoutingExplanation, ToolSemantics,
    apply_hard_constraints,
};

// ---------------------------------------------------------------------------
// Line 1592 — when a routing decision may be taken at all.
// ---------------------------------------------------------------------------

/// Map line 1592: *"route at task or session boundaries rather than switching
/// providers blindly on every conversational turn"*.
///
/// A value the caller must state, never a flag this module infers, and
/// deliberately the same shape as
/// [`crate::routing::interactive::SessionActivity`] rather than a reuse of
/// it: that type answers "may a *migration* be taken", which is a question
/// about one already-running session, and this one answers "may a
/// *destination* be chosen", which is a question about where work goes next.
/// Collapsing them would make a mid-turn migration refusal and a mid-turn
/// routing refusal the same decision, and line 1592's own wording separates
/// them.
///
/// [`Self::MidTurn`] is not an error. The router still answers — it answers
/// with the destination the work is already on, and an explanation that says
/// it did not re-decide. A router that returned `None` mid-turn would push
/// the "so what do I do now" decision back to a caller that has less to go
/// on than it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingMoment {
    /// No session exists yet for this work. Every destination is a candidate.
    SessionStart,
    /// One task finished and another is beginning. The work may move.
    TaskBoundary,
    /// A turn is in flight. Routing is **not** taken here.
    MidTurn,
}

impl RoutingMoment {
    /// Whether a routing decision may be taken at this moment — line 1592 in
    /// one function, so there is exactly one place the rule lives.
    pub fn permits_routing(self) -> bool {
        matches!(self, Self::SessionStart | Self::TaskBoundary)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::SessionStart => "session start",
            Self::TaskBoundary => "task boundary",
            Self::MidTurn => "mid-turn",
        }
    }
}

impl std::fmt::Display for RoutingMoment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Destinations.
// ---------------------------------------------------------------------------

/// Line 1594's *"a good checkpoint exists"*, as the two facts a caller can
/// read off a `crate::checkpoint::Checkpoint` without this module reading
/// one.
///
/// Deliberately **not** a quality score. A checkpoint's objective and
/// implementation state are required by the format, so their presence says
/// nothing; what varies is whether it says what to do next, and whether
/// anything had to be dropped to fit the size bound
/// (`Checkpoint::trimmed`). Those two are observable, and a third field for
/// "is the objective any good" would be a number a caller could only invent
/// — the same refusal [`WarmSession`] already makes about accumulated
/// context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckpointQuality {
    has_next_actions: bool,
    complete: bool,
}

impl CheckpointQuality {
    /// `has_next_actions` is `!checkpoint.handoff.next_actions.is_empty()`;
    /// `complete` is `!checkpoint.trimmed`.
    pub fn new(has_next_actions: bool, complete: bool) -> Self {
        Self {
            has_next_actions,
            complete,
        }
    }

    /// Line 1594's own adjective. Both facts, not either: a trimmed
    /// checkpoint that lists next actions has lost content the fresh session
    /// would have to rediscover, and a complete one that says nothing about
    /// what to do next hands a fresh session an objective and no route into
    /// it.
    pub fn is_good(&self) -> bool {
        self.has_next_actions && self.complete
    }

    pub fn has_next_actions(&self) -> bool {
        self.has_next_actions
    }

    pub fn complete(&self) -> bool {
        self.complete
    }
}

/// Whether a destination continues something or starts something.
///
/// The axis lines 1593 and 1594 are about, as a type rather than a `bool`,
/// because the two arms carry different evidence: an existing session carries
/// what is known about its warmth, and a fresh one carries what is known
/// about the checkpoint it would boot from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Continuation {
    /// An existing session, with the warmth the caller read off its
    /// `crate::session::store::SessionRecord`.
    Existing(WarmSession),
    /// A session that does not exist yet. `Some` when a checkpoint is
    /// available to boot it from; `None` when it would start from nothing.
    Fresh(Option<CheckpointQuality>),
}

impl Continuation {
    pub fn is_fresh(self) -> bool {
        matches!(self, Self::Fresh(_))
    }
}

/// One place a piece of work could go.
///
/// Carries its own harness and its own continuation, which is the whole
/// difference from [`Backend`] and the reason the six `Consider X` lines are
/// answerable here — see this module's header.
#[derive(Debug, Clone, PartialEq)]
pub struct Destination {
    id: String,
    harness: IntegrationId,
    launch_profile: String,
    backend: Backend,
    continuation: Continuation,
    /// Line 1598: what the caller has read about this destination's
    /// credential's remaining quota. `None` when nothing has been read —
    /// never a zero and never a full, for the reason `glasshouse resources`
    /// prints `unknown` rather than either.
    capacity: Option<RemainingCapacityScore>,
    /// Every wire protocol the serving provider offers, not only the one
    /// [`Backend::protocol`] names.
    ///
    /// Load-bearing for line 1595, and the reason it is a field rather than
    /// being derived from the backend: `ProtocolFit::Compatible` — *"not this
    /// protocol, but the provider serves another one the harness does
    /// speak"* — is **unreachable** from a backend alone, because a backend
    /// carries exactly one protocol and `protocol_fit` would then only ever
    /// answer `Native` or `Incompatible`. A capability signal with two
    /// reachable states where the model has five is a signal that has quietly
    /// lost most of its resolution. The caller reads this off
    /// `crate::provider::Provider`, which already knows every base URL it has.
    provider_protocols: Vec<WireProtocol>,
}

impl Destination {
    /// An existing session that could be continued.
    pub fn existing(
        id: impl Into<String>,
        harness: IntegrationId,
        launch_profile: impl Into<String>,
        backend: Backend,
        warm: WarmSession,
    ) -> Self {
        Self::new(
            id,
            harness,
            launch_profile,
            backend,
            Continuation::Existing(warm),
        )
    }

    /// A session that would be started for this work.
    pub fn fresh(
        id: impl Into<String>,
        harness: IntegrationId,
        launch_profile: impl Into<String>,
        backend: Backend,
        checkpoint: Option<CheckpointQuality>,
    ) -> Self {
        Self::new(
            id,
            harness,
            launch_profile,
            backend,
            Continuation::Fresh(checkpoint),
        )
    }

    fn new(
        id: impl Into<String>,
        harness: IntegrationId,
        launch_profile: impl Into<String>,
        backend: Backend,
        continuation: Continuation,
    ) -> Self {
        let protocol = backend.protocol().to_owned();
        Self {
            id: id.into(),
            harness,
            launch_profile: launch_profile.into(),
            backend,
            continuation,
            capacity: None,
            provider_protocols: pairing::wire_protocol_from_slug(&protocol)
                .into_iter()
                .collect(),
        }
    }

    /// Attach a real quota reading — line 1598. The caller resolves it from
    /// `crate::provider::quota::CapacityState::remaining_capacity_score`;
    /// this module reads no telemetry of its own.
    pub fn with_capacity(mut self, capacity: Option<RemainingCapacityScore>) -> Self {
        self.capacity = capacity;
        self
    }

    /// Declare every protocol the serving provider offers — line 1595. The
    /// default is the one the backend names, which is the honest floor for a
    /// caller that has not looked the provider up.
    pub fn with_provider_protocols(mut self, protocols: Vec<WireProtocol>) -> Self {
        self.provider_protocols = protocols;
        self
    }

    pub fn provider_protocols(&self) -> &[WireProtocol] {
        &self.provider_protocols
    }

    /// The stable identifier a user names in an override, and the one a
    /// routing overview prints.
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn harness(&self) -> IntegrationId {
        self.harness
    }

    pub fn launch_profile(&self) -> &str {
        &self.launch_profile
    }

    pub fn backend(&self) -> &Backend {
        &self.backend
    }

    pub fn continuation(&self) -> Continuation {
        self.continuation
    }

    pub fn is_fresh(&self) -> bool {
        self.continuation.is_fresh()
    }

    pub fn capacity(&self) -> Option<&RemainingCapacityScore> {
        self.capacity.as_ref()
    }

    /// A short name for a diagnostic. Never carries a credential value — the
    /// credential appears only through [`super::CredentialId::label`], which
    /// is a name.
    pub fn label(&self) -> String {
        format!(
            "{} on {} via {} ({})",
            self.id,
            self.harness.slug(),
            self.backend.provider(),
            if self.is_fresh() { "fresh" } else { "existing" }
        )
    }
}

/// What the work itself requires, as facts a caller states rather than
/// preferences a router guesses.
///
/// One field today, and it is the one that can actually **reject** a
/// destination: a task that needs tool calls cannot go somewhere tool calls
/// are established not to work. Anything a router would only *prefer* belongs
/// in a contribution, not here — that is design decision 1 ("additive, never
/// a filter") carried into this phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TaskRequirements {
    /// Whether the work needs the harness's tool-call protocol to work.
    pub needs_tool_calls: bool,
}

// ---------------------------------------------------------------------------
// Line 1602 — the user's override.
// ---------------------------------------------------------------------------

/// Which destination the user named, when they named one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DestinationChoice {
    /// This destination by [`Destination::id`], whatever the ranking says.
    To(String),
    /// Whichever fresh destination ranks highest — "start a new session".
    Fresh,
    /// Whatever the work is already on — "leave it where it is".
    Hold,
}

/// Map line 1602: *"allow the user to override every automatic routing
/// choice"*.
///
/// This router makes exactly **two** automatic choices, and both are
/// overridable here, which is what makes the line's word "every" checkable
/// rather than aspirational:
///
/// 1. *whether to route at this moment* — [`RoutingMoment::permits_routing`],
///    overridden by [`Self::and_route_now`];
/// 2. *which destination wins* — the ranking, overridden by
///    [`DestinationChoice`].
///
/// An override never overrules a **hard constraint**, and that is not a gap.
/// A constraint is a fact about what can serve (a protocol the harness does
/// not speak, tool calls established not to work), not a choice the router
/// made; honouring an override into one would produce a session that cannot
/// run and an explanation that said it was asked for. The refusal is reported
/// in the explanation rather than silently swallowed — see
/// [`Routed::override_refused`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoutingOverride {
    destination: Option<DestinationChoice>,
    route_now: bool,
}

impl RoutingOverride {
    /// No override — the shape a caller with nothing configured passes, and
    /// the one that reproduces the router's automatic answer exactly.
    pub fn none() -> Self {
        Self::default()
    }

    pub fn to(id: impl Into<String>) -> Self {
        Self {
            destination: Some(DestinationChoice::To(id.into())),
            route_now: false,
        }
    }

    pub fn fresh() -> Self {
        Self {
            destination: Some(DestinationChoice::Fresh),
            route_now: false,
        }
    }

    pub fn hold() -> Self {
        Self {
            destination: Some(DestinationChoice::Hold),
            route_now: false,
        }
    }

    /// Also override the boundary gate: decide now, even mid-turn.
    ///
    /// Not a contradiction of line 1592. That line forbids switching
    /// *blindly* on every turn — an automatic re-decision nobody asked for.
    /// A person asking for one is the opposite of blind, and it is recorded
    /// in the explanation as their doing.
    pub fn and_route_now(mut self) -> Self {
        self.route_now = true;
        self
    }

    /// An override that only lifts the boundary gate and leaves the ranking
    /// alone.
    pub fn route_now() -> Self {
        Self {
            destination: None,
            route_now: true,
        }
    }

    pub fn destination(&self) -> Option<&DestinationChoice> {
        self.destination.as_ref()
    }

    pub fn routes_now(&self) -> bool {
        self.route_now
    }

    pub fn is_empty(&self) -> bool {
        self.destination.is_none() && !self.route_now
    }
}

// ---------------------------------------------------------------------------
// Weights. Every one of them is on the scale `crate::config::pairing` already
// established: `PriorStrength::Strong` peaks at 1.0 and a live warm session
// at zero idle is worth 1.5, so a term written at 1.0 here is "as much as the
// strongest configured preference" and not an arbitrary unit.
// ---------------------------------------------------------------------------

/// Line 1595. What a destination whose harness speaks the route's own wire
/// protocol is worth against one that reaches it only through a second
/// protocol the provider happens to serve.
///
/// Positive rather than "not penalised" because protocol nativeness is an
/// established fact — the adapter declares the protocols it speaks — and this
/// module's rule is that only established facts earn a candidate anything.
const PROTOCOL_NATIVE_FIT: f64 = 1.0;
const PROTOCOL_COMPATIBLE_FIT: f64 = 0.4;

/// A model established **not** to behave the way the harness needs. Large and
/// negative: this is a "known no", not an absence of evidence, and it is the
/// one capability signal that should be able to lose a destination the
/// ranking outright without being a hard constraint.
const MODEL_BEHAVIOUR_KNOWN_ABSENT: f64 = -1.0;

/// Line 1597. What an intact provider-side prompt cache is worth.
///
/// Below [`PROTOCOL_NATIVE_FIT`] on purpose: a cold cache costs latency and
/// tokens on the first turn, and a harness that cannot speak the protocol
/// costs the whole session.
const CACHE_PRESERVED: f64 = 0.6;

/// The *likely* case — same provider and model, different credential. Worth
/// something, because nothing has established that account-scoped caches
/// really do miss, and worth less than a certainty, for the same reason
/// [`CacheLocality::LikelyLost`] exists as a separate variant at all.
const CACHE_LIKELY_LOST: f64 = 0.2;

/// Line 1598. The full weight of a destination's remaining quota, multiplied
/// by [`RemainingCapacityScore::routing_fraction`] — so a resource at 100%
/// contributes this and one at 10% contributes a tenth of it.
const QUOTA_PRESSURE_WEIGHT: f64 = 0.8;

/// Line 1599. What one consecutive observed failure costs a destination, and
/// the floor the accumulation stops at.
///
/// Small per failure and bounded, because [`super::free::ResourceHealth`]
/// already answers the *hard* question ("may this be chosen right now")
/// through its cooldown; this term is the soft residue — a resource that has
/// been flaky lately but is not currently cooling down.
const HEALTH_FAILURE_PENALTY: f64 = -0.3;
const HEALTH_PENALTY_FLOOR: f64 = -0.9;

/// A destination whose credential the provider refused, or which is still
/// cooling down. Larger than [`HEALTH_PENALTY_FLOOR`] because these are
/// present-tense facts rather than a history, and deliberately still a
/// contribution rather than a hard constraint: line 1599 says *consider*
/// provider health, and a build where every destination is unhealthy must
/// still choose one and say why.
const HEALTH_UNAVAILABLE_PENALTY: f64 = -1.5;

/// Line 1600. What starting a session from nothing costs: the first turn is
/// spent re-establishing what a warm session already knows.
const BOOTSTRAP_COST: f64 = -1.0;

/// What a fresh session's affinity is worth: nothing, and **not a penalty**.
///
/// Named rather than written as a literal in the early return for the same
/// reason every other magnitude in this module is named: the claim "the
/// absence of a warm session says nothing against a candidate" is a policy
/// decision — it is the one
/// [`crate::config::pairing::session_continuity_contribution`] makes too —
/// and a policy decision buried as a bare `0.0` inside a `return` cannot be
/// argued with, moved, or mutated to find out what watches it.
const FRESH_SESSION_AFFINITY: f64 = 0.0;

/// The same, when a good checkpoint exists to boot from — line 1594's own
/// clause. Reduced, never removed: a checkpoint carries the objective, the
/// state and the next actions, and it does not carry the conversation.
const BOOTSTRAP_COST_WITH_CHECKPOINT: f64 = -0.25;

/// Line 1600's other half: moving work to a different harness mid-task costs
/// more than moving it to a different provider, because the harness is what
/// holds the tools, the permissions and the transcript.
const SWITCH_HARNESS_COST: f64 = -0.8;
const SWITCH_PROVIDER_COST: f64 = -0.3;

// ---------------------------------------------------------------------------
// The six contributions. One public function each, so a mutation can zero
// exactly one of them.
// ---------------------------------------------------------------------------

/// Line 1595: what the harness's own capability fit for this destination
/// contributes.
///
/// Reads `classify`'s three **capability** axes and not its vendor class —
/// see this module's header for why the vendor class is deliberately absent.
/// The axes vary with the harness, which is exactly what makes this term able
/// to separate a candidate set: `crate::harness::adapter_for` returns a
/// different adapter per [`IntegrationId`], and each declares its own
/// protocols.
pub fn harness_capability_fit(
    destination: &Destination,
    overrides: &pairing::PairingOverrides,
) -> Contribution {
    let pairing = classify_destination(destination, overrides);

    let (protocol_score, protocol_note) = match pairing.protocol_fit() {
        ProtocolFit::Native => (
            PROTOCOL_NATIVE_FIT,
            "speaks this route's wire protocol itself",
        ),
        ProtocolFit::Compatible => (
            PROTOCOL_COMPATIBLE_FIT,
            "does not speak this route's protocol, but the provider serves another one it does",
        ),
        ProtocolFit::Translated => (0.0, "reaches this route only through a translation adapter"),
        ProtocolFit::Incompatible => (0.0, "declares protocols and none of them is served here"),
        ProtocolFit::Unknown => (
            0.0,
            "declares no protocols, or the route named none — not a `no`",
        ),
    };

    let (behaviour_score, behaviour_note) = match pairing.model_behaviour() {
        ModelBehaviourFit::Verified => (0.0, "model behaviour established for this harness"),
        ModelBehaviourFit::Unverified => (
            0.0,
            "nobody established whether this model behaves the way this harness needs",
        ),
        ModelBehaviourFit::KnownAbsent => (
            MODEL_BEHAVIOUR_KNOWN_ABSENT,
            "this model is established **not** to behave the way this harness needs",
        ),
    };

    Contribution::new(
        "harness capability fit",
        protocol_score + behaviour_score,
        format!(
            "{}: protocol fit `{}` — {protocol_note}; model behaviour `{}` — {behaviour_note}",
            destination.harness().slug(),
            pairing.protocol_fit(),
            pairing.model_behaviour(),
        ),
    )
}

/// Line 1596: what an existing session's affinity contributes.
///
/// The affinity Glasshouse can actually compute today is **warmth**: whether
/// the session is live or merely resumable, and how long it has been idle.
/// The arithmetic is
/// [`crate::config::pairing::session_continuity_contribution`]'s own, reused
/// rather than copied so that the decay window and the live/resumable ratio
/// have one definition; only the name changes, because line 569 and line 1596
/// ask for the same quantity at two different decisions.
///
/// **What this is not.** Phase 36 (lines 1581–1588) asks for an affinity
/// score that rises with same-task work, recently touched files and
/// semantically useful context, and falls with noise. None of those three has
/// a producer in this build — the session store records no turn count, no
/// touched-file set and no task identity — so this term is warmth alone and
/// says so in its own evidence string rather than implying a richer signal.
pub fn session_affinity(destination: &Destination) -> Contribution {
    let Continuation::Existing(warm) = destination.continuation() else {
        return Contribution::new(
            "session affinity",
            FRESH_SESSION_AFFINITY,
            "a fresh session has no accumulated context to be affine to — not a penalty, only \
             the absence of the term (the bootstrap cost is where starting from nothing is \
             priced)",
        );
    };

    let reused = crate::config::pairing::session_continuity_contribution(
        &evidence_key_for(destination),
        &OneWarmSession(warm),
    );

    Contribution::new(
        "session affinity",
        reused.magnitude(),
        format!(
            "`{}` is a {} session, idle {}s — warmth is the whole of the affinity this build can \
             compute: Phase 36's same-task, touched-file and semantic-quality signals have no \
             producer here",
            destination.id(),
            warm.state,
            warm.idle_seconds.max(0)
        ),
    )
}

/// Line 1597: what this destination does to provider-side prompt caching.
///
/// Two different questions, and the answer is the *worse* of them:
///
/// - a **fresh** session has no cached prefix at all, whatever its backend
///   is, because the cache is keyed by the conversation and there is no
///   conversation yet;
/// - an **existing** session's cached prefix survives only if the work stays
///   on the backend that built it, which is [`CacheLocality::between`]'s own
///   question and is answered by the one rule in Glasshouse that answers it.
///
/// `current` is what is serving the work now. `None` at a session start,
/// where there is nothing to move away from.
pub fn prompt_cache_state(
    destination: &Destination,
    current: Option<&Destination>,
) -> Contribution {
    if destination.is_fresh() {
        return Contribution::new(
            "prompt-cache state",
            0.0,
            "a fresh session starts with no cached prefix anywhere — there is no conversation \
             for a provider-side cache to have seen",
        );
    }

    let Some(current) = current else {
        // Deliberately `0.0`, and this is the correction that came out of
        // line 1594's own test failing.
        //
        // [`CacheLocality`] is defined as a comparison — `between(from, to)`
        // — and a session start has no `from`. Crediting an existing
        // destination with `Preserved` here would assert that a cached
        // prefix survived a move that was never made, and Glasshouse
        // observes neither a provider cache's presence nor its TTL (see
        // `WARM_SESSION_RELEVANCE_WINDOW_SECONDS`' own doc, which says those
        // expire in minutes). It would also double-count warmth, which
        // [`session_affinity`] already prices once.
        //
        // The consequence, stated rather than hidden: **line 1597's
        // contribution is inert at a session start** and live at a task
        // boundary, which is where the line's word "state" has a state to
        // compare against.
        return Contribution::new(
            "prompt-cache state",
            0.0,
            format!(
                "`{}` is not being moved from anything — a session start has no prior backend \
                 for a cached prefix to have survived, and Glasshouse observes neither a \
                 provider cache's presence nor its lifetime",
                destination.id()
            ),
        );
    };

    let locality = CacheLocality::between(current.backend(), destination.backend());
    let magnitude = match &locality {
        CacheLocality::Preserved => CACHE_PRESERVED,
        CacheLocality::LikelyLost(_) => CACHE_LIKELY_LOST,
        CacheLocality::Lost(_) => 0.0,
    };
    Contribution::new("prompt-cache state", magnitude, locality.to_string())
}

/// Line 1598: what is known about this destination's remaining quota.
///
/// `None` contributes exactly `0.0` and says so. That is not "assume full"
/// and not "assume empty": an unread resource is neither preferred nor
/// withheld, which is the same stance `glasshouse resources` takes when it
/// prints `unknown` rather than a number nobody read.
pub fn quota_pressure(destination: &Destination) -> Contribution {
    match destination.capacity() {
        Some(score) => Contribution::new(
            "known quota pressure",
            score.routing_fraction() * QUOTA_PRESSURE_WEIGHT,
            format!(
                "{} remaining on `{}`, bound by {}",
                score.percent().render(),
                destination.backend().credential().label(),
                score.dimension()
            ),
        ),
        None => Contribution::new(
            "known quota pressure",
            0.0,
            format!(
                "nothing has been read about `{}`'s remaining quota — an unread resource is \
                 neither preferred nor withheld",
                destination.backend().credential().label()
            ),
        ),
    }
}

/// Line 1599: what has actually been observed about this destination's
/// provider.
///
/// Read from [`FreePool`], whose health half is keyed by credential **and**
/// model and is cost-agnostic — [`super::free::ResourceHealth`] counts
/// consecutive failures, cooldowns and credential rejections, none of which
/// is a statement about price. `crate::gateway::session`'s `observe_exchange`
/// is what puts real outcomes into it, from work that was going to happen
/// anyway, which is line 534's constraint and the reason nothing here probes.
pub fn provider_health(destination: &Destination, pool: &FreePool, now: Instant) -> Contribution {
    let resource = FreeResource::new(
        destination.backend().credential().clone(),
        destination.backend().model().label(),
    );
    let health = pool.health(&resource);

    if health.credential_was_rejected() {
        return Contribution::new(
            "provider health",
            HEALTH_UNAVAILABLE_PENALTY,
            format!(
                "`{}` was refused by its provider — waiting does not fix a revoked key",
                destination.backend().credential().label()
            ),
        );
    }
    if !health.is_available(now) {
        return Contribution::new(
            "provider health",
            HEALTH_UNAVAILABLE_PENALTY,
            format!(
                "`{}` is still cooling down after {} consecutive observed failures",
                destination.backend().credential().label(),
                health.consecutive_failures()
            ),
        );
    }
    let failures = health.consecutive_failures();
    if failures == 0 {
        return Contribution::new(
            "provider health",
            0.0,
            format!(
                "nothing has been observed against `{}` on `{}` — not a health claim, the \
                 absence of one",
                destination.backend().model().label(),
                destination.backend().credential().label()
            ),
        );
    }
    Contribution::new(
        "provider health",
        (f64::from(failures) * HEALTH_FAILURE_PENALTY).max(HEALTH_PENALTY_FLOOR),
        format!(
            "{failures} consecutive observed failures on `{}` that have not yet earned a \
             cooldown",
            destination.backend().credential().label()
        ),
    )
}

/// Line 1600: what it would cost to send the work here.
///
/// Two costs, added, because they are genuinely separate and a destination
/// can carry both:
///
/// - **bootstrap** — a fresh session must re-establish what a warm one
///   already holds. Line 1594's *"a good checkpoint exists"* is priced right
///   here: a good checkpoint carries the objective, the state and the next
///   actions, so it cuts the cost rather than removing it, because it does
///   not carry the conversation.
/// - **switching** — moving work off the harness or provider it is on now.
///   Harness costs more than provider: the harness holds the tools, the
///   permissions and the transcript.
///
/// Deliberately distinct from [`prompt_cache_state`], which prices the
/// *provider's* cache. This one prices what Glasshouse and the person have to
/// redo, and the two are different quantities that a single "switching cost"
/// term would have averaged into one unreadable number.
pub fn switching_and_bootstrap_cost(
    destination: &Destination,
    current: Option<&Destination>,
) -> Contribution {
    let mut magnitude = 0.0;
    let mut notes: Vec<String> = Vec::new();

    match destination.continuation() {
        Continuation::Fresh(Some(checkpoint)) if checkpoint.is_good() => {
            magnitude += BOOTSTRAP_COST_WITH_CHECKPOINT;
            notes.push(
                "a fresh session, booting from a good checkpoint — it carries the objective, the \
                 state and the next actions, and not the conversation"
                    .to_owned(),
            );
        }
        Continuation::Fresh(Some(checkpoint)) => {
            magnitude += BOOTSTRAP_COST;
            notes.push(format!(
                "a fresh session, and the available checkpoint is not a good one ({}{})",
                if checkpoint.has_next_actions() {
                    "next actions present"
                } else {
                    "no next actions"
                },
                if checkpoint.complete() {
                    ""
                } else {
                    ", and it was trimmed to fit"
                }
            ));
        }
        Continuation::Fresh(None) => {
            magnitude += BOOTSTRAP_COST;
            notes.push(
                "a fresh session with no checkpoint to boot from — its first turn is spent \
                 re-establishing context"
                    .to_owned(),
            );
        }
        Continuation::Existing(_) => {
            notes.push("an existing session pays no bootstrap cost".to_owned());
        }
    }

    if let Some(current) = current {
        if current.harness() != destination.harness() {
            magnitude += SWITCH_HARNESS_COST;
            notes.push(format!(
                "moving from `{}` to `{}` changes the harness, which holds the tools, the \
                 permissions and the transcript",
                current.harness().slug(),
                destination.harness().slug()
            ));
        } else if current.backend().provider() != destination.backend().provider() {
            magnitude += SWITCH_PROVIDER_COST;
            notes.push(format!(
                "moving from `{}` to `{}` changes the provider",
                current.backend().provider(),
                destination.backend().provider()
            ));
        } else {
            notes.push("no harness or provider change".to_owned());
        }
    }

    Contribution::new("switching and bootstrap cost", magnitude, notes.join("; "))
}

// ---------------------------------------------------------------------------
// The router.
// ---------------------------------------------------------------------------

/// Everything [`SessionRouter::choose`] weighs that is not a destination.
///
/// One struct rather than four arguments for
/// [`crate::routing::interactive::SessionStartInputs`]' own reason: these
/// travel together and are resolved together, and a caller assembling them at
/// the call site is a caller that can pass last decision's health with this
/// decision's overrides.
pub struct RouterInputs<'a> {
    /// The user's corrections to pairing metadata — line 561, and what
    /// `classify` reads for [`harness_capability_fit`].
    pub overrides: &'a pairing::PairingOverrides,
    /// Observed provider health — line 1599. `&FreePool::new()` is the honest
    /// shape for a build with no gateway running.
    pub health: &'a FreePool,
    /// The clock, as an argument. See this module's header.
    pub now: Instant,
    /// What the work requires, for the hard-constraint gate.
    pub requirements: TaskRequirements,
}

impl std::fmt::Debug for RouterInputs<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RouterInputs")
            .field("overrides", self.overrides)
            .field("requirements", &self.requirements)
            .finish_non_exhaustive()
    }
}

/// Why an override could not be honoured. Reported rather than swallowed —
/// a user who asked for a destination and silently got another one has been
/// lied to by a router whose whole product is an explanation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverrideRefusal {
    /// The named id is not among the destinations offered.
    NoSuchDestination(String),
    /// The named destination exists and a hard constraint rejected it.
    Ineligible(String, HardConstraint),
    /// `Fresh` was asked for and every fresh destination was rejected, or
    /// none was offered.
    NoFreshDestination,
    /// `Hold` was asked for and the caller named no current destination.
    NothingToHold,
}

impl std::fmt::Display for OverrideRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSuchDestination(id) => write!(
                f,
                "the override named `{id}`, which is not one of the destinations offered"
            ),
            Self::Ineligible(id, constraint) => write!(
                f,
                "the override named `{id}`, which a hard {constraint} constraint rejected — an \
                 override may overrule a ranking and not a fact about what can serve"
            ),
            Self::NoFreshDestination => f.write_str(
                "the override asked for a fresh session and no eligible fresh destination was \
                 offered",
            ),
            Self::NothingToHold => f.write_str(
                "the override asked to hold the current destination and no current destination \
                 was named",
            ),
        }
    }
}

/// What [`SessionRouter::choose`] decided, and everything behind it — map
/// line 1601.
///
/// Holds every candidate's explanation and not only the winner's, because
/// line 1601 asks for an *inspectable* explanation in an overview mode and
/// "why this one" is unanswerable without "and what the others scored". The
/// rejected destinations are kept separately with the constraint that removed
/// them, so a reader can tell "scored badly" from "could not serve".
#[derive(Debug, Clone, PartialEq)]
pub struct Routed {
    moment: RoutingMoment,
    re_decided: bool,
    chosen: Destination,
    explanation: RoutingExplanation,
    considered: Vec<(Destination, RoutingExplanation)>,
    rejected: Vec<(Destination, HardConstraint)>,
    automatic: Option<String>,
    override_refused: Option<OverrideRefusal>,
}

impl Routed {
    /// Where the work goes.
    pub fn chosen(&self) -> &Destination {
        &self.chosen
    }

    /// The winning destination's own contributions, in the order they were
    /// weighed.
    pub fn explanation(&self) -> &RoutingExplanation {
        &self.explanation
    }

    /// Every eligible destination and its explanation, best first.
    pub fn considered(&self) -> &[(Destination, RoutingExplanation)] {
        &self.considered
    }

    /// Every destination a hard constraint removed, and which constraint.
    pub fn rejected(&self) -> &[(Destination, HardConstraint)] {
        &self.rejected
    }

    /// Whether the router actually ranked anything — `false` when line 1592's
    /// boundary gate held the work where it was.
    pub fn re_decided(&self) -> bool {
        self.re_decided
    }

    pub fn moment(&self) -> RoutingMoment {
        self.moment
    }

    /// The [`Destination::id`] the ranking would have chosen, when a user
    /// override changed the answer. `None` when the automatic answer stands.
    pub fn overrode(&self) -> Option<&str> {
        self.automatic.as_deref()
    }

    /// Why an override was not honoured, when one was not.
    pub fn override_refused(&self) -> Option<&OverrideRefusal> {
        self.override_refused.as_ref()
    }

    /// Line 1601's debug mode: the decision and the contributions behind it.
    pub fn render(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        let _ = writeln!(out, "destination  {}", self.chosen.label());
        let _ = writeln!(
            out,
            "moment       {} — {}",
            self.moment,
            if self.re_decided {
                "routing was taken here"
            } else {
                "routing is not taken here; the work stays where it is (line 1592)"
            }
        );
        let _ = writeln!(out, "score        {:+.3}", self.explanation.total());
        if let Some(automatic) = self.overrode() {
            let _ = writeln!(
                out,
                "override     the user chose this; the ranking would have chosen `{automatic}`"
            );
        }
        if let Some(refusal) = self.override_refused() {
            let _ = writeln!(out, "override     not applied — {refusal}");
        }
        out.push_str("why\n");
        out.push_str(&self.explanation.render());
        out
    }

    /// Line 1601's overview mode: every candidate, ranked, with what each was
    /// rejected or scored for.
    pub fn render_overview(&self) -> String {
        use std::fmt::Write as _;
        let mut out = self.render();
        if self.considered.len() > 1 {
            out.push_str("\nalternatives\n");
            for (destination, explanation) in self.considered.iter().skip(1) {
                let _ = writeln!(
                    out,
                    "  {:+.3}  {}",
                    explanation.total(),
                    destination.label()
                );
                for line in explanation.render().lines() {
                    let _ = writeln!(out, "  {line}");
                }
            }
        }
        if !self.rejected.is_empty() {
            out.push_str("\nrejected\n");
            for (destination, constraint) in &self.rejected {
                let _ = writeln!(
                    out,
                    "  {} — hard {constraint} constraint",
                    destination.label()
                );
            }
        }
        out
    }
}

/// The session-aware router — map lines 1592 to 1602.
///
/// Holds the user's override and nothing else, exactly as
/// [`crate::routing::interactive::InteractiveRouting`] holds their pin and
/// nothing else: everything it decides is a function of its arguments, so the
/// same router answers the same way every time and a decision can be
/// reproduced from a log rather than from when it happened to be asked.
#[derive(Debug, Clone, Default)]
pub struct SessionRouter {
    user_override: RoutingOverride,
}

impl SessionRouter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Line 1602.
    pub fn with_override(user_override: RoutingOverride) -> Self {
        Self { user_override }
    }

    pub fn user_override(&self) -> &RoutingOverride {
        &self.user_override
    }

    /// Choose where this work goes — the router's one production entry point.
    ///
    /// `current` is where the work is now: `None` at a session start.
    /// `destinations` is every place it could go, in the caller's own order,
    /// which is the tiebreaker exactly as it is everywhere else in this
    /// module's siblings.
    ///
    /// `None` only when `destinations` is empty **and** there is no current
    /// destination to hold — there is nowhere for the work to go, and
    /// inventing one would be worse than saying so.
    ///
    /// # Order
    ///
    /// 1. **Line 1592's boundary gate.** Mid-turn, nothing is ranked and the
    ///    current destination is returned with an explanation saying why.
    ///    [`RoutingOverride::and_route_now`] is the one thing that lifts it.
    /// 2. **Hard constraints**, through
    ///    [`super::apply_hard_constraints`] and therefore structurally: a
    ///    task that needs tool calls cannot go where they are established not
    ///    to work, and a harness that cannot speak the route's protocol at
    ///    all cannot serve it.
    /// 3. **The six soft contributions** (lines 1595–1600), summed by
    ///    [`RoutingExplanation::total`]. None of them can exclude a
    ///    destination; only step 2 can.
    /// 4. **The user's override** (line 1602), applied over the ranking and
    ///    never over step 2, with the automatic answer recorded so a reader
    ///    can see what was overruled.
    pub fn choose(
        &self,
        moment: RoutingMoment,
        current: Option<&Destination>,
        destinations: &[Destination],
        inputs: &RouterInputs<'_>,
    ) -> Option<Routed> {
        if !moment.permits_routing() && !self.user_override.routes_now() {
            let held = current.cloned()?;
            let mut explanation = RoutingExplanation::new();
            explanation.push(Contribution::new(
                "routing boundary",
                0.0,
                format!(
                    "this is a {moment} moment, and routing is taken at task or session \
                     boundaries only — the work stays on `{}` rather than being re-decided \
                     between turns",
                    held.id()
                ),
            ));
            return Some(Routed {
                moment,
                re_decided: false,
                chosen: held,
                explanation,
                considered: Vec::new(),
                rejected: Vec::new(),
                automatic: None,
                override_refused: None,
            });
        }

        if destinations.is_empty() {
            return None;
        }

        let (eligible, rejected) =
            apply_hard_constraints(destinations.to_vec(), |destination: &Destination| {
                hard_constraint(destination, inputs)
            });

        if eligible.is_empty() {
            // Every destination failed a hard constraint. Holding the work
            // where it is beats refusing to answer, and beats sending it
            // somewhere established not to serve.
            let held = current.cloned()?;
            let mut explanation = RoutingExplanation::new();
            explanation.push(Contribution::new(
                "hard constraints",
                0.0,
                format!(
                    "every destination offered failed a hard constraint, so the work stays on \
                     `{}` — a hard constraint is a fact about what can serve and is not \
                     outranked by a score",
                    held.id()
                ),
            ));
            return Some(Routed {
                moment,
                re_decided: true,
                chosen: held,
                explanation,
                considered: Vec::new(),
                rejected,
                automatic: None,
                override_refused: None,
            });
        }

        let mut scored: Vec<(Destination, RoutingExplanation)> = eligible
            .into_iter()
            .map(super::EligibleCandidate::into_inner)
            .map(|destination| {
                let explanation = score(&destination, current, inputs);
                (destination, explanation)
            })
            .collect();

        // Stable, best first: `sort_by` keeps the caller's own order on a
        // tie, which is the tiebreaker every other policy in this module's
        // siblings uses.
        scored.sort_by(|a, b| {
            b.1.total()
                .partial_cmp(&a.1.total())
                .expect("a contribution magnitude is never NaN")
        });

        let automatic_id = scored[0].0.id().to_owned();
        let (index, refusal) = self.apply_override(&scored, current, &rejected);
        let overrode = (scored[index].0.id() != automatic_id).then(|| automatic_id.clone());

        let (chosen, mut explanation) = scored[index].clone();
        if let Some(automatic) = &overrode {
            explanation.push(Contribution::new(
                "user override",
                0.0,
                format!(
                    "the user routed this work to `{}` explicitly; without the override the \
                     ranking would have chosen `{automatic}`",
                    chosen.id()
                ),
            ));
        }
        if let Some(refusal) = &refusal {
            explanation.push(Contribution::new(
                "user override",
                0.0,
                format!("the override was not applied: {refusal}"),
            ));
        }

        Some(Routed {
            moment,
            re_decided: true,
            chosen,
            explanation,
            considered: scored,
            rejected,
            automatic: overrode,
            override_refused: refusal,
        })
    }

    /// Line 1602, over the ranking: which index of `scored` the user's
    /// override selects, and why it could not be honoured when it could not.
    fn apply_override(
        &self,
        scored: &[(Destination, RoutingExplanation)],
        current: Option<&Destination>,
        rejected: &[(Destination, HardConstraint)],
    ) -> (usize, Option<OverrideRefusal>) {
        let Some(choice) = self.user_override.destination() else {
            return (0, None);
        };
        match choice {
            DestinationChoice::To(id) => match scored.iter().position(|(d, _)| d.id() == id) {
                Some(index) => (index, None),
                None => match rejected.iter().find(|(d, _)| d.id() == id) {
                    Some((_, constraint)) => (
                        0,
                        Some(OverrideRefusal::Ineligible(id.clone(), *constraint)),
                    ),
                    None => (0, Some(OverrideRefusal::NoSuchDestination(id.clone()))),
                },
            },
            DestinationChoice::Fresh => match scored.iter().position(|(d, _)| d.is_fresh()) {
                Some(index) => (index, None),
                None => (0, Some(OverrideRefusal::NoFreshDestination)),
            },
            DestinationChoice::Hold => {
                let Some(current) = current else {
                    return (0, Some(OverrideRefusal::NothingToHold));
                };
                match scored.iter().position(|(d, _)| d.id() == current.id()) {
                    Some(index) => (index, None),
                    None => (
                        0,
                        Some(OverrideRefusal::NoSuchDestination(current.id().to_owned())),
                    ),
                }
            }
        }
    }
}

/// Lines 1595 to 1600, in the order a reader compares them: what the harness
/// can do, what the session already holds, what the provider has cached, what
/// is left of the quota, how the provider has behaved, and what the move
/// costs.
fn score(
    destination: &Destination,
    current: Option<&Destination>,
    inputs: &RouterInputs<'_>,
) -> RoutingExplanation {
    let mut explanation = RoutingExplanation::new();
    explanation.push(harness_capability_fit(destination, inputs.overrides));
    explanation.push(session_affinity(destination));
    explanation.push(prompt_cache_state(destination, current));
    explanation.push(quota_pressure(destination));
    explanation.push(provider_health(destination, inputs.health, inputs.now));
    explanation.push(switching_and_bootstrap_cost(destination, current));
    explanation
}

/// The gate step 2 runs. Two constraints and no others, for the same reason
/// [`crate::routing::interactive`]'s `compatible` has two: each is a fact
/// about whether the destination *can* serve, not a preference about whether
/// it *should*.
fn hard_constraint(
    destination: &Destination,
    inputs: &RouterInputs<'_>,
) -> Result<(), HardConstraint> {
    if inputs.requirements.needs_tool_calls
        && destination.backend().tools() == ToolSemantics::KnownAbsent
    {
        return Err(HardConstraint::ToolSemantics);
    }
    if classify_destination(destination, inputs.overrides).protocol_fit()
        == ProtocolFit::Incompatible
    {
        return Err(HardConstraint::Protocol);
    }
    Ok(())
}

/// One place the pairing query is built, so every consumer asks the same
/// question — the reason `interactive`'s `evidence_key_for` is one function
/// too.
fn classify_destination(
    destination: &Destination,
    overrides: &pairing::PairingOverrides,
) -> pairing::Pairing {
    let query = pairing::PairingQuery {
        harness: destination.harness(),
        model: destination.backend().model().clone(),
        route: serving_route(destination.backend()),
        // `crate::harness::Declared` carries a `&'static str` evidence
        // string that `crate::routing::Backend` deliberately does not keep
        // (see `Backend::tools`' own doc comment), so there is nothing
        // honest to reconstruct one from. `classify` reads this field only
        // for `Pairing::tool_semantics`, which neither
        // `harness_capability_fit` nor `hard_constraint` looks at — the
        // hard constraint reads `Backend::tools()` directly, which is the
        // fact rather than a round trip through a type that would have to
        // invent its provenance. Same degradation, same reason, as
        // `crate::routing::interactive`'s own `score_candidate`.
        tool_calls: crate::harness::Declared::Unverified,
        provider_protocols: destination.provider_protocols().to_vec(),
    };
    pairing::classify(&query, overrides)
}

fn serving_route(backend: &Backend) -> pairing::ServingRoute {
    pairing::ServingRoute {
        provider: Some(backend.provider().to_owned()),
        gateway: None,
        protocol: pairing::wire_protocol_from_slug(backend.protocol()),
    }
}

fn evidence_key_for(destination: &Destination) -> pairing::EvidenceKey {
    pairing::EvidenceKey::new(
        destination.harness(),
        destination.launch_profile(),
        destination.backend().model().clone(),
        serving_route(destination.backend()),
    )
}

/// A [`ContinuitySource`] answering with the one warm session the caller
/// already attached to this destination.
///
/// The adapter exists so the decay window and the live/resumable ratio have
/// exactly one definition — `crate::config::pairing`'s — rather than a second
/// copy here that could drift from it. It answers the same for every key
/// because it is only ever asked about the destination it was built from.
struct OneWarmSession(WarmSession);

impl ContinuitySource for OneWarmSession {
    fn warm_session(&self, _key: &pairing::EvidenceKey) -> Option<WarmSession> {
        Some(self.0)
    }
}
