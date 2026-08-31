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

use std::collections::BTreeSet;
use std::time::Instant;

use crate::config::pairing::{ContinuitySource, WarmSession};
use crate::harness::pairing::{self, ModelBehaviourFit, ProtocolFit};
use crate::harness::{Capabilities as HarnessCapabilities, Declared, WireProtocol};
use crate::integrations::IntegrationId;
use crate::provider::quota::{CapacityBand, RemainingCapacityScore};

use super::capability::{self, ResourceFacts};
use super::classify::{HardCapability, WorkloadTier};
use super::free::{FreePool, FreeResource};
use super::pressure::{
    self, Alternatives, CapacityFacts, PressureInputs, ReservePolicies, ReserveScope,
};
use super::request::RouterAnswer;
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
    /// Lines 1570–1574: the band that score falls in against the user's
    /// thresholds and the provider's own reserve percentage, and how far off
    /// the next reset is — both read by the caller, exactly as `capacity` is,
    /// and both `None` when nothing has been read. Carried beside `capacity`
    /// rather than derived from it here because the thresholds that turn a
    /// score into a band are configuration this module does not read, and
    /// the reset is not in the score at all.
    capacity_facts: CapacityFacts,
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
    /// Map line 1382: model/resource facts a harness adapter does not
    /// declare, attached via [`Self::with_resource_facts`]. Defaults to
    /// [`ResourceFacts::UNVERIFIED`] — the honest floor for a caller that has
    /// not looked a model's own facts up, matching every other unattached
    /// `Declared` value in this struct.
    resource_facts: ResourceFacts,
    /// Map line 1516: the highest workload tier this destination is
    /// **established** to serve, attached via [`Self::with_tier_ceiling`].
    /// `None` — the default, and what a destination whose model the user
    /// named no ceiling for carries — means nobody has established one, and
    /// the tier gate then does nothing to it: an unknown ceiling is not a low
    /// one, the same rule `capability`'s `Unverified` and line 1434's
    /// absent-headroom reading both follow.
    tier_ceiling: Option<WorkloadTier>,
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
            capacity_facts: CapacityFacts::UNREAD,
            provider_protocols: pairing::wire_protocol_from_slug(&protocol)
                .into_iter()
                .collect(),
            resource_facts: ResourceFacts::UNVERIFIED,
            tier_ceiling: None,
        }
    }

    /// Attach a real quota reading — line 1598. The caller resolves it from
    /// `crate::provider::quota::CapacityState::remaining_capacity_score`;
    /// this module reads no telemetry of its own.
    pub fn with_capacity(mut self, capacity: Option<RemainingCapacityScore>) -> Self {
        self.capacity = capacity;
        self
    }

    /// Attach what was read about this destination's capacity band and reset
    /// — lines 1570–1574. The caller resolves both from the same
    /// `crate::provider::quota::CapacityState` it took `capacity` from; the
    /// default is [`CapacityFacts::UNREAD`], on which every pressure term is
    /// inert and says so.
    pub fn with_capacity_facts(mut self, facts: CapacityFacts) -> Self {
        self.capacity_facts = facts;
        self
    }

    pub fn capacity_facts(&self) -> CapacityFacts {
        self.capacity_facts
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

    /// Attach model/resource facts — map line 1382. The default is
    /// [`ResourceFacts::UNVERIFIED`], the honest floor for a caller that has
    /// not looked a model's own facts up; see `capability`'s module
    /// documentation for how this combines with the harness's own
    /// declaration.
    pub fn with_resource_facts(mut self, facts: ResourceFacts) -> Self {
        self.resource_facts = facts;
        self
    }

    pub fn resource_facts(&self) -> ResourceFacts {
        self.resource_facts
    }

    /// State the highest workload tier this destination is established to
    /// serve — map line 1516's input. `None` withdraws the fact.
    ///
    /// **The production caller is `main.rs::routing_destinations`**, which
    /// attaches every destination the shipped binary builds with
    /// `destination_tier_ceiling`'s reading of the user's own
    /// `providers.<p>.model_ceilings` (map line 1796) — so the gate in
    /// `hard_constraint` and the term in [`workload_tier_fit`] act on the
    /// binary's path, not only on the library's.
    ///
    /// It is still `None` for most destinations, and that is the design
    /// rather than a gap: a ceiling exists only where the user stated one for
    /// that specific model on that specific provider. Every destination in a
    /// project that has configured none carries `None` and is treated exactly
    /// as it was before the producer existed — "nobody has said" is not
    /// "cannot".
    pub fn with_tier_ceiling(mut self, ceiling: Option<WorkloadTier>) -> Self {
        self.tier_ceiling = ceiling;
        self
    }

    pub fn tier_ceiling(&self) -> Option<WorkloadTier> {
        self.tier_ceiling
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
/// `needs_tool_calls` is the one field that can actually **reject** a
/// destination: a task that needs tool calls cannot go somewhere tool calls
/// are established not to work. Anything a router would only *prefer* belongs
/// in a contribution, not here — that is design decision 1 ("additive, never
/// a filter") carried into this phase.
///
/// `hard_capabilities` carries `TaskClassification::hard_capabilities()`'s own
/// output (`super::classify`) so [`capability_fit`] has something to compare
/// a destination's registry entry against. It is additive only — ruling 4 of
/// the `GH-ROUTING-CAPABILITY` packet gives capability mismatch exactly one
/// rejecting exception (a hard capability the resource is *established* to
/// lack), and that exception is not wired here: nothing in this package
/// constructs a `HardConstraint::Capability`, so an established-absent axis
/// still only costs a candidate a contribution, never a rejection.
///
/// `minimum_tier` is the second field that can reject (map line 1516), and
/// it rejects only a destination whose ceiling is *established* below it —
/// see [`Destination::with_tier_ceiling`]. `classification` is the answer
/// the requirements were built from, carried so the explanation can say who
/// classified the work and whether line 1459's conservative rules fired;
/// `None` for a caller with no task in hand, which is every launch that
/// states none and therefore reproduces the pre-classification explanation
/// byte for byte.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TaskRequirements {
    /// Whether the work needs the harness's tool-call protocol to work.
    pub needs_tool_calls: bool,
    /// The hard capability requirements this task implies — see
    /// `super::classify::TaskClassification::hard_capabilities`.
    pub hard_capabilities: Vec<HardCapability>,
    /// Line 1516: the lowest workload tier that may serve this work.
    pub minimum_tier: Option<WorkloadTier>,
    /// The classification these requirements came from, for the explanation.
    pub classification: Option<RouterAnswer>,
}

impl TaskRequirements {
    /// The required tier, or `None` when none was established.
    pub fn tier(&self) -> Option<WorkloadTier> {
        self.minimum_tier
    }
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

/// `GH-ROUTING-CAPABILITY`, box 1391. A resource established to have a task's
/// required capability — the case a router should prefer.
const CAPABILITY_ESTABLISHED_PRESENT: f64 = 0.4;

/// The mirror case: a resource established **not** to have a required
/// capability. Negative, and worse than [`CAPABILITY_UNVERIFIED`] — but this
/// is additive (ruling 4), so a mismatch costs a candidate something and does
/// not remove it from consideration. (Ruling 4's one rejecting exception —
/// a hard capability the resource is *established* to lack — is not wired in
/// this package; see [`TaskRequirements`]'s own doc comment.)
const CAPABILITY_ESTABLISHED_ABSENT: f64 = -0.4;

/// Ruling 3's tri-state: nothing established either way scores `0.0`, the
/// same precedent [`harness_capability_fit`]'s own `ProtocolFit::Unknown` arm
/// sets — *"declares no protocols, or the route named none — not a `no`"*.
/// Strictly greater than [`CAPABILITY_ESTABLISHED_ABSENT`], which is exactly
/// what acceptance test 2 checks.
const CAPABILITY_UNVERIFIED: f64 = 0.0;

/// Map line 1531. A destination whose established ceiling is exactly the
/// tier the work needs — the fit a router should prefer, on the same scale
/// as [`CAPABILITY_ESTABLISHED_PRESENT`] because it is the same kind of
/// established fact.
const TIER_FIT_EXACT: f64 = 0.4;

/// The same, for a destination established to serve *above* the required
/// tier. Positive — it can do the work — and less than an exact fit, because
/// sending routine work to the strongest resource spends what the map calls
/// a scarce premium session on something a cheaper one could do.
const TIER_FIT_HEADROOM: f64 = 0.2;

/// Nothing established about the destination's tier scores `0.0` — the same
/// tri-state as [`CAPABILITY_UNVERIFIED`], and for the same reason. A
/// destination *below* the tier never reaches this term: line 1516's gate
/// removed it first.
const TIER_FIT_UNVERIFIED: f64 = 0.0;

/// Map line 1558: what a destination that costs the user money contributes,
/// against one that costs nothing, when both are otherwise adequate.
///
/// **Deliberately the smallest magnitude in this module** — smaller than
/// every other differentiating constant here, [`TIER_FIT_HEADROOM`] and
/// [`CACHE_LIKELY_LOST`]'s `0.2` included — because 1558 asks for the
/// cheapest candidate *that satisfies* the tier and the hard capabilities,
/// not for the cheapest candidate. Adequacy, health and warmth are priced by
/// their own terms and must keep outranking price; this one decides only
/// between candidates those terms could not separate, which is exactly what
/// "prefer the cheapest **among** them" means.
const METERED_COST_PREFERENCE: f64 = -0.1;

// ---------------------------------------------------------------------------
// The contributions. One public function each, so a mutation can zero
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

/// Map line 1382, joined to a task's hard capability requirements —
/// `GH-ROUTING-CAPABILITY`'s package, and `capability::axis_for`'s own
/// comparison function is what makes this ruling-1-safe: this function never
/// compares a task's tier to a resource's tier, only a resource's registry
/// entry to the specific axis a requirement names.
///
/// This is `TaskClassification::hard_capabilities`' real production
/// consumer: nothing else in the shipped binary reads the value that
/// function returns for anything other than the diagnostic `writeln!` in
/// `classify::describe`. `requirements.hard_capabilities` is where a caller
/// of [`SessionRouter::choose`] attaches it (`main.rs` passes
/// `TaskRequirements::default()` today, which is an empty list and therefore
/// a `0.0` contribution — this package wires the mechanism; a follow-up
/// package is what will have `main.rs` actually call
/// `TaskClassification::hard_capabilities` and populate the field).
///
/// Reads `destination.harness()` the same way [`harness_capability_fit`]
/// does — the identity is already in hand at the point this term is
/// computed — and combines it with [`Destination::resource_facts`] through
/// [`capability::ResourceCapabilities::describe`]. No capability value and no
/// resource identity is matched here: this function only asks the registry a
/// question and applies the three named constants above, which is 1390's
/// answer — a new resource, a new harness, or a corrected axis changes
/// nothing in this function's body.
pub fn capability_fit(destination: &Destination, requirements: &TaskRequirements) -> Contribution {
    if requirements.hard_capabilities.is_empty() {
        return Contribution::new(
            "capability fit",
            0.0,
            "the task named no hard capability requirement, so this resource's capability \
             description contributes nothing",
        );
    }

    let harness_caps = crate::harness::adapter_for(destination.harness())
        .map(|adapter| adapter.describe().capabilities)
        .unwrap_or(HarnessCapabilities::UNVERIFIED);
    let resource =
        capability::ResourceCapabilities::describe(&harness_caps, destination.resource_facts());

    let mut magnitude = 0.0;
    let mut notes = Vec::with_capacity(requirements.hard_capabilities.len());
    for requirement in &requirements.hard_capabilities {
        let axis = capability::axis_for(*requirement);
        let (term, note) = match resource.axis(axis) {
            Declared::Verified {
                value: true,
                evidence,
            } => (
                CAPABILITY_ESTABLISHED_PRESENT,
                format!(
                    "`{}` needs {} and this resource's `{}` is established present ({evidence})",
                    destination.harness().slug(),
                    requirement.as_str(),
                    axis.name(),
                ),
            ),
            Declared::Verified {
                value: false,
                evidence,
            } => (
                CAPABILITY_ESTABLISHED_ABSENT,
                format!(
                    "`{}` needs {} and this resource's `{}` is established **absent** \
                     ({evidence})",
                    destination.harness().slug(),
                    requirement.as_str(),
                    axis.name(),
                ),
            ),
            Declared::Unverified => (
                CAPABILITY_UNVERIFIED,
                format!(
                    "`{}` needs {} and this resource's `{}` is not established — not a `no`",
                    destination.harness().slug(),
                    requirement.as_str(),
                    axis.name(),
                ),
            ),
        };
        magnitude += term;
        notes.push(note);
    }

    Contribution::new("capability fit", magnitude, notes.join("; "))
}

/// Map line 1531: how well this destination's established workload ceiling
/// fits the tier the work needs.
///
/// Only called when a tier is stated (`score` skips it otherwise, so a
/// launch that states no task renders exactly the explanation it always
/// has). A destination below the required tier is never scored here — line
/// 1516's gate in `hard_constraint` removed it — so the three cases are
/// exact, headroom, and not established.
pub fn workload_tier_fit(destination: &Destination, required: WorkloadTier) -> Contribution {
    match destination.tier_ceiling() {
        Some(ceiling) if ceiling == required => Contribution::new(
            "workload tier fit",
            TIER_FIT_EXACT,
            format!(
                "the task needs the `{required}` tier and `{}` is established to serve exactly \
                 that",
                destination.id()
            ),
        ),
        Some(ceiling) => Contribution::new(
            "workload tier fit",
            TIER_FIT_HEADROOM,
            format!(
                "the task needs the `{required}` tier and `{}` is established to serve up to \
                 `{ceiling}` — it can, with headroom a cheaper resource would not spend",
                destination.id()
            ),
        ),
        None => Contribution::new(
            "workload tier fit",
            TIER_FIT_UNVERIFIED,
            format!(
                "the task needs the `{required}` tier and nothing has established `{}`'s \
                 ceiling — not a `no`",
                destination.id()
            ),
        ),
    }
}

/// Map line 1558: *"prefer the cheapest healthy candidate that satisfies the
/// required workload tier and hard capabilities."*
///
/// # What this term is, and what the three words before it already decide
///
/// The line names four properties, and three of them are already decided
/// before this function is reached, which is why it prices only the fourth:
///
/// - *satisfies the required workload tier* — `hard_constraint` has already
///   **removed** a destination whose established ceiling is below the
///   requirement, and [`workload_tier_fit`] prices the fit of what is left;
/// - *satisfies the hard capabilities* — [`capability_fit`] prices an
///   established-absent axis at `CAPABILITY_ESTABLISHED_ABSENT`, four times
///   this term's magnitude, so a cheap resource established to lack what the
///   task needs can never win on price;
/// - *healthy* — [`provider_health`] prices a refused credential and a
///   cooling-down provider, and its penalties are larger again.
///
/// So what is left for this term is the comparison the line is actually
/// about: two candidates the terms above could not separate, one of which
/// spends the user's money. It is `METERED_COST_PREFERENCE` for a metered
/// destination and `0.0` for a free one — a preference for the free
/// resource expressed as a cost on the paid one, so that a project with no
/// free resource configured is not scored as though every destination it has
/// were somehow deficient.
///
/// # Why it is only pushed when a tier was established
///
/// `score` pushes this term exactly where it pushes [`workload_tier_fit`]:
/// under `if let Some(required)`. The line's own subject is *"a candidate
/// that satisfies the required workload tier"*, and there is no required tier
/// until a task has been classified — so a launch or a `glasshouse route`
/// that states no task renders precisely the explanation it rendered before
/// this term existed, byte for byte. The same rule, and the same reason, as
/// the tier term beside it.
///
/// `Cost` is [`super::Backend::cost`], which `main.rs::destination_backend`
/// resolves through `ProviderConfig::cost_of` — the user's own `free_models`
/// list. Nothing here infers a price from a model's name.
pub fn cost_preference(destination: &Destination) -> Contribution {
    if destination.backend().cost().is_free() {
        return Contribution::new(
            "cost preference",
            0.0,
            format!(
                "`{}` is a zero-cost resource for this work — nothing is spent by preferring it",
                destination.id()
            ),
        );
    }
    Contribution::new(
        "cost preference",
        METERED_COST_PREFERENCE,
        format!(
            "`{}` is metered, so it is preferred only over candidates this decision's other \
             terms could not already separate it from (line 1558)",
            destination.id()
        ),
    )
}

/// The classification a decision acted on, as a zero-weight line in every
/// candidate's explanation — so a reader of `glasshouse route --task` or of
/// a launch sees who classified the work, what it was classed as, and
/// whether line 1459's conservative rules changed the answer, beside the
/// terms that answer then drove.
fn classification_note(answer: &RouterAnswer) -> Contribution {
    Contribution::new("task classification", 0.0, answer.explain())
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
                    "  {} — hard {constraint} constraint{}",
                    destination.label(),
                    constraint
                        .reason()
                        .map(|reason| format!(" — {reason}"))
                        .unwrap_or_default()
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
    /// Line 1577: both scopes' reserve policies, as configuration resolved
    /// them. This router applies the **interactive** one — see
    /// [`ReserveScope`] for why that is a fact about its callers and not a
    /// guess — and carries both so the explanation can name which applied.
    reserve_policies: ReservePolicies,
    /// Line 1290: the sessions the user named as allowed to spend protected
    /// reserve, as `crate::config::EffectiveConfig::reserve_override_sessions`
    /// resolved them. Read at exactly one place,
    /// [`SessionRouter::reserve_overridden`], which is true only of an
    /// existing session the user named — the same scope rule the throwaway-job
    /// router's own override type enforces, restated here in one line rather
    /// than imported, because this module may not reach that module
    /// (`super::tests::the_session_router_cannot_reach_the_disposable_policy_class`).
    reserve_override_sessions: BTreeSet<String>,
}

impl SessionRouter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Line 1602.
    pub fn with_override(user_override: RoutingOverride) -> Self {
        Self {
            user_override,
            ..Self::default()
        }
    }

    /// Line 1577. Without this the default applies — `protect` for both
    /// scopes, the fail-closed direction for a spending protection.
    #[must_use]
    pub fn with_reserve_policies(mut self, policies: ReservePolicies) -> Self {
        self.reserve_policies = policies;
        self
    }

    /// Line 1290. Naming none is the same as not calling this.
    #[must_use]
    pub fn with_reserve_override_sessions<S: Into<String>>(
        mut self,
        sessions: impl IntoIterator<Item = S>,
    ) -> Self {
        self.reserve_override_sessions = sessions.into_iter().map(Into::into).collect();
        self
    }

    pub fn user_override(&self) -> &RoutingOverride {
        &self.user_override
    }

    pub fn reserve_policies(&self) -> ReservePolicies {
        self.reserve_policies
    }

    /// Whether the user overrode reserve protection for `destination` — true
    /// only for an existing session the user named. A fresh destination has
    /// no session for the user to have named, and there is deliberately no
    /// spelling of this set that means "every session" (line 1290 says *for
    /// a specific task or session*).
    fn reserve_overridden(&self, destination: &Destination) -> bool {
        !destination.is_fresh() && self.reserve_override_sessions.contains(destination.id())
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
    ///    to work, a harness that cannot speak the route's protocol at all
    ///    cannot serve it, and a destination established to serve below the
    ///    classified minimum tier cannot serve the work (line 1516).
    /// 3. **The soft contributions** (lines 1595–1600, the capability fit,
    ///    and — when a tier is stated — line 1531's tier fit), summed by
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

        // The set-level facts the pressure terms need (`super::pressure`)
        // are computed against the *eligible* set, after step 2: a candidate
        // a hard constraint removed is not an alternative anything can be
        // routed to instead.
        let candidates: Vec<Destination> = eligible
            .into_iter()
            .map(super::EligibleCandidate::into_inner)
            .collect();
        let mut scored: Vec<(Destination, RoutingExplanation)> = candidates
            .iter()
            .enumerate()
            .map(|(index, destination)| {
                let alternatives = alternatives_for(index, &candidates, inputs);
                let pressure = PressureInputs {
                    premium: !destination.backend().cost().is_free(),
                    facts: destination.capacity_facts(),
                    // `None` is "not established": `super::pressure` takes it
                    // conservatively at the reserve gate (line 1459) and is
                    // inert on it for the low-tier term.
                    tier: inputs.requirements.minimum_tier,
                    existing: !destination.is_fresh(),
                    alternatives: &alternatives,
                    policies: self.reserve_policies,
                    scope: ReserveScope::Interactive,
                    user_override: self.reserve_overridden(destination),
                };
                let explanation = score(destination, current, inputs, &pressure);
                (destination.clone(), explanation)
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
    pressure: &PressureInputs<'_>,
) -> RoutingExplanation {
    let mut explanation = RoutingExplanation::new();
    if let Some(answer) = &inputs.requirements.classification {
        explanation.push(classification_note(answer));
    }
    explanation.push(harness_capability_fit(destination, inputs.overrides));
    explanation.push(capability_fit(destination, &inputs.requirements));
    if let Some(required) = inputs.requirements.minimum_tier {
        explanation.push(workload_tier_fit(destination, required));
        // Line 1558, pushed under the same condition and for the same
        // reason: "the cheapest candidate that satisfies the required
        // workload tier" has no subject until a tier has been required.
        explanation.push(cost_preference(destination));
    }
    explanation.push(session_affinity(destination));
    explanation.push(prompt_cache_state(destination, current));
    explanation.push(quota_pressure(destination));
    // Phase 35D, lines 1570–1577: the band the quota reading falls in, and
    // what the scope's reserve policy makes of it — placed right after the
    // reading it qualifies, so a reader sees the percentage and the band
    // together.
    explanation.push(pressure::capacity_band_pressure(pressure));
    explanation.push(pressure::low_tier_spend(pressure));
    explanation.push(provider_health(destination, inputs.health, inputs.now));
    explanation.push(switching_and_bootstrap_cost(destination, current));
    explanation
}

/// What the other eligible candidates offer `candidates[index]` as an
/// alternative — the two set-level facts `super::pressure` reads, computed
/// here because only the router holds the set.
///
/// "Adequate" is [`is_adequate`]: no required hard capability established
/// absent, the same fact [`capability_fit`] prices. "Available" is the
/// provider's observed health, the same fact [`provider_health`] prices.
/// Neither is re-decided here; both are read off the destination the way the
/// pricing terms read them, so the alternative an explanation names is one
/// those terms would also have scored well.
fn alternatives_for(
    index: usize,
    candidates: &[Destination],
    inputs: &RouterInputs<'_>,
) -> Alternatives {
    let mut alternatives = Alternatives::none();
    for (other_index, other) in candidates.iter().enumerate() {
        // A candidate that cannot serve right now — refused by its provider,
        // or cooling down — is not an alternative anything can be routed to
        // instead, whatever its band. Without this, a reserve-band
        // destination would be denied in favour of a provider that
        // `provider_health` is about to score as unavailable, and the work
        // would go to the one place it cannot run.
        if other_index == index
            || !is_adequate(other, &inputs.requirements)
            || !provider_available(other, inputs.health, inputs.now)
        {
            continue;
        }
        let free = other.backend().cost().is_free();
        let band = other.capacity_facts().band();
        if alternatives.healthy_free_adequate().is_none()
            && free
            && band.is_none_or(|band| band >= CapacityBand::Healthy)
        {
            alternatives = alternatives.with_healthy_free_adequate(other.id());
        }
        if alternatives.cheaper_adequate().is_none()
            && (free || band.is_some_and(|band| band > CapacityBand::Reserve))
        {
            alternatives = alternatives.with_cheaper_adequate(other.id());
        }
    }
    alternatives
}

/// Whether `destination` is established to lack none of the task's required
/// hard capabilities — the negative half of [`capability_fit`]'s reading,
/// as a fact rather than a price. Unverified is not a `no`, here as there.
fn is_adequate(destination: &Destination, requirements: &TaskRequirements) -> bool {
    if requirements.hard_capabilities.is_empty() {
        return true;
    }
    let harness_caps = crate::harness::adapter_for(destination.harness())
        .map(|adapter| adapter.describe().capabilities)
        .unwrap_or(HarnessCapabilities::UNVERIFIED);
    let resource =
        capability::ResourceCapabilities::describe(&harness_caps, destination.resource_facts());
    requirements.hard_capabilities.iter().all(|requirement| {
        !matches!(
            resource.axis(capability::axis_for(*requirement)),
            Declared::Verified { value: false, .. }
        )
    })
}

/// Whether the provider behind `destination` is currently usable by its
/// observed health — not refused, not cooling down. The same two facts
/// [`provider_health`] prices at [`HEALTH_UNAVAILABLE_PENALTY`].
fn provider_available(destination: &Destination, pool: &FreePool, now: Instant) -> bool {
    let health = pool.health(&FreeResource::new(
        destination.backend().credential().clone(),
        destination.backend().model().label(),
    ));
    !health.credential_was_rejected() && health.is_available(now)
}

/// The gate step 2 runs. Three constraints and no others, for the same
/// reason [`crate::routing::interactive`]'s `compatible` has two: each is a
/// fact about whether the destination *can* serve, not a preference about
/// whether it *should*.
///
/// The third — map line 1516 — fires only on an **established** ceiling
/// strictly below the required tier. A destination with no ceiling stated
/// passes, because "nobody has said" is not "cannot"; the same rule the two
/// constraints above already follow for `Unverified` tool semantics and an
/// unknown protocol.
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
    if let (Some(required), Some(offered)) =
        (inputs.requirements.minimum_tier, destination.tier_ceiling())
        && offered < required
    {
        return Err(HardConstraint::WorkloadTier { required, offered });
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
