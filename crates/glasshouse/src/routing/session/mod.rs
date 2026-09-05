//! Phase 37 — the basic session-aware router: which *destination* a piece of
//! work goes to, and why.
//!
//! [`super::interactive`] and [`super::disposable`] both rank **backends**;
//! this module ranks **destinations**, a strictly larger thing (an existing
//! session that could be continued, or a fresh one to start) — map lines
//! 1593/1594's *"prefer an existing relevant session"* against *"prefer a
//! fresh session"*, unexpressable by a policy whose candidates are all
//! backends. That difference is what makes the six `Consider X` lines
//! (1595–1600) answerable here: [`Destination`] varies along harness,
//! warmth, cache locality, credential and bootstrap cost, where a
//! `crate::gateway::upstream::Upstream`-built candidate set varies only by
//! route. Every contribution below has a test holding two destinations
//! differing **only** in that axis and asserting they resolve differently.
//!
//! [`pairing_prior`] reads `classify`'s *vendor* axis, unlike
//! [`harness_capability_fit`]'s capability axes, because a [`Destination`]'s
//! [`Backend`] carries a model resolved **per launch profile**, so a
//! candidate set built from two enabled profiles of one harness genuinely
//! varies in `PairingClass` — unlike [`super::interactive`]'s
//! `UpstreamBackend`, which takes one model for the whole set.
//!
//! Same purity as the rest of `routing`: no socket, credential resolution
//! or clock; `now` is an argument, and this module names neither
//! `crate::session` nor `crate::checkpoint`, taking warmth, capacity and
//! checkpoint quality as values the caller looked up.
// History: design-decisions.md, "Trims: routing module docs", routing/session/mod.rs module doc.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use crate::config::pairing::{ContinuitySource, WarmSession};
use crate::evaluation::{HarnessTierOutcome, RoutingTier, TierOutcomeVerdict};
use crate::harness::pairing::{self, ModelBehaviourFit, ProtocolFit};
use crate::harness::{Capabilities as HarnessCapabilities, Declared, WireProtocol};
use crate::integrations::IntegrationId;
use crate::provider::pricing::PriceTable;
use crate::provider::quota::{CapacityBand, RemainingCapacityScore};

use super::burn::ClassOutput;
use super::capability::{self, ResourceFacts};
use super::classify::{DurationClass, HardCapability, TaskClassification, WorkloadTier};
use super::evidence::{CostConfidence, FailureClass, MIN_SAMPLE_FOR_SUMMARY, ObservedCost};
use super::free::{Allowance, FreePool, FreeResource, Window};
use super::pressure::{
    self, Alternatives, CapacityFacts, PressureInputs, ReservePolicies, ReserveScope,
};
use super::request::{RouterAnswer, TaskClass};
use super::{
    Backend, CacheLocality, Contribution, EntitlementSource, HardConstraint,
    ProviderUnavailableCause, RoutingExplanation, TierRelation, ToolSemantics,
    apply_hard_constraints, same_capability_tier,
};

mod discovery;
mod reserve;
mod scoring;
#[cfg(test)]
mod tests;

pub use discovery::{
    AffinityBreakdown, AffinityFacet, affinity_breakdown, paths_named_in, session_affinity,
};
use discovery::{alternatives_for, hard_constraint};
use reserve::decide_tier_movement;
pub use reserve::{
    EntitlementFallback, EntitlementPoolView, EscalationTrigger, FallbackReason, FallbackStep,
    HoldReason, OverrideRefusal, TierMovement, cadence_availability, entitlement_capacity,
    entitlement_fallback, entitlement_model_availability, entitlement_reset_boundary,
    entitlement_throttling, provider_health, quota_pressure, switching_and_bootstrap_cost,
};
pub use scoring::{
    HarnessEfficiencySummary, capability_fit, cost_preference, harness_capability_fit,
    pairing_prior, prompt_cache_state, request_pool_cost, workload_tier_fit,
};
use scoring::{estimated_cost, score};

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

/// Phase 36 (lines 1581–1588): what the caller read about an existing
/// session's native context, beyond the warmth a [`WarmSession`] carries.
/// Every field is a value the **caller looked up** — this module names
/// neither `crate::session` nor `crate::checkpoint` — and the production
/// caller is `main.rs::routing_destinations`.
///
/// `None` everywhere means **unknown**, never zero: `Some(0)` compactions
/// is a counted clean history, `None` is a row nobody counted.
/// `task_named_paths` is a fact about the *task*, carried here because the
/// router holds no task text of its own; `None` is "no task was stated",
/// `Some(vec![])` is "a task was stated and names no path".
// History: design-decisions.md, "Trims: routing module docs, second packet", routing/session/mod.rs `SessionContextFacts`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionContextFacts {
    observed_compactions: Option<i64>,
    last_task: Option<TaskClassification>,
    touched_files: Option<Vec<String>>,
    task_named_paths: Option<Vec<String>>,
    estimated_context_tokens: Option<i64>,
}

impl SessionContextFacts {
    /// Nothing read — the honest floor, and what every destination carries
    /// until `main.rs::routing_destinations` attaches what it looked up.
    pub const UNREAD: Self = Self {
        observed_compactions: None,
        last_task: None,
        touched_files: None,
        task_named_paths: None,
        estimated_context_tokens: None,
    };

    /// Lines 1584 and 1586: how many compactions a harness has said it was
    /// about to perform on this session — `SessionRecord::observed_compactions`
    /// verbatim, `None` when nobody was counting.
    pub fn with_observed_compactions(mut self, count: Option<i64>) -> Self {
        self.observed_compactions = count;
        self
    }

    /// Line 1582: the classification the sticky classification cache
    /// recorded as the last task classified onto **this** session. `None`
    /// when the cache names another session, or nothing.
    pub fn with_last_task(mut self, classification: Option<TaskClassification>) -> Self {
        self.last_task = classification;
        self
    }

    /// Line 1583: the repo-relative paths this session's latest checkpoint
    /// lists — its handoff's files and its working tree's changed files.
    /// `None` when the session has no checkpoint at all.
    pub fn with_touched_files(mut self, files: Option<Vec<String>>) -> Self {
        self.touched_files = files;
        self
    }

    /// Line 1583's other operand: [`paths_named_in`] the task text, or `None`
    /// when no task was stated.
    pub fn with_task_named_paths(mut self, paths: Option<Vec<String>>) -> Self {
        self.task_named_paths = paths;
        self
    }

    /// Line 1158: [`crate::routing::evidence::estimated_context_tokens`]'s
    /// reading for this session, or `None` when it never relayed an exchange
    /// with a known input-token count.
    pub fn with_estimated_context_tokens(mut self, tokens: Option<i64>) -> Self {
        self.estimated_context_tokens = tokens;
        self
    }

    pub fn observed_compactions(&self) -> Option<i64> {
        self.observed_compactions
    }

    pub fn last_task(&self) -> Option<&TaskClassification> {
        self.last_task.as_ref()
    }

    pub fn touched_files(&self) -> Option<&[String]> {
        self.touched_files.as_deref()
    }

    pub fn task_named_paths(&self) -> Option<&[String]> {
        self.task_named_paths.as_deref()
    }

    pub fn estimated_context_tokens(&self) -> Option<i64> {
        self.estimated_context_tokens
    }
}

/// Map lines 1298, 1299 and 1304: the components of one decision's own
/// input-size estimate, named rather than folded into a single number.
/// Every component is `Some(tokens)` when actually measured and `None`
/// when not — never a zero standing in for "nobody looked".
/// [`Self::total_tokens`] is `None` only when every component is `None`;
/// otherwise it sums what was measured, so an unread component never
/// understates the total as a zero would.
///
/// The production caller is `main.rs::routing_destinations`: a fresh
/// destination carries the project's own memory and checkpoint (line
/// 1304), an existing cold session carries its own latest checkpoint
/// (line 1299), and a live session stays [`Self::UNESTIMATED`] entirely —
/// `WarmSession` already refuses to guess at accumulated context.
// History: design-decisions.md, "Trims: routing module docs, second packet", routing/session/mod.rs `EstimatedInputSize`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EstimatedInputSize {
    project_memory_tokens: Option<u64>,
    checkpoint_tokens: Option<u64>,
    bootstrap_context_tokens: Option<u64>,
}

impl EstimatedInputSize {
    /// Nothing measured — the honest floor, and what every destination
    /// carries until `main.rs::routing_destinations` attaches what it read.
    pub const UNESTIMATED: Self = Self {
        project_memory_tokens: None,
        checkpoint_tokens: None,
        bootstrap_context_tokens: None,
    };

    /// Map line 1304's "project memory": [`crate::firewall::estimate::estimate_tokens`]
    /// of the real text [`crate::memory::inject::briefing`] would inject for
    /// this task — a measurement of the actual injection, not a model of it.
    /// `None` when the store could not be opened, `briefing` itself failed,
    /// or `briefing` matched nothing to inject — all three read as "this
    /// component was not counted" rather than "this component counts as
    /// zero", because none of them is the certain fact [`super::Cost::is_free`]
    /// is.
    pub fn with_project_memory_tokens(mut self, tokens: Option<u64>) -> Self {
        self.project_memory_tokens = tokens;
        self
    }

    /// Map lines 1299 and 1304's checkpoint component: the rendered size of
    /// the checkpoint document this destination would actually read — the
    /// project's latest for a fresh session's bootstrap half, or the cold
    /// session's own latest for a resume. `None` when there is no checkpoint
    /// to measure.
    pub fn with_checkpoint_tokens(mut self, tokens: Option<u64>) -> Self {
        self.checkpoint_tokens = tokens;
        self
    }

    /// Map line 1304's "bootstrap context": a fixed session document
    /// installed at launch, distinct from the checkpoint above, when this
    /// build has one reachable before routing measures it. Always `None`
    /// today — see this field's producer in `main.rs` for why it is
    /// deliberately never set rather than modeled.
    pub fn with_bootstrap_context_tokens(mut self, tokens: Option<u64>) -> Self {
        self.bootstrap_context_tokens = tokens;
        self
    }

    pub fn project_memory_tokens(&self) -> Option<u64> {
        self.project_memory_tokens
    }

    pub fn checkpoint_tokens(&self) -> Option<u64> {
        self.checkpoint_tokens
    }

    pub fn bootstrap_context_tokens(&self) -> Option<u64> {
        self.bootstrap_context_tokens
    }

    /// The components actually measured, summed. `None` when none were —
    /// never `Some(0)` for an estimate nobody could build any part of, which
    /// is what keeps a destination nobody can size from becoming the
    /// cheapest candidate by default.
    pub fn total_tokens(&self) -> Option<u64> {
        let known: Vec<u64> = [
            self.project_memory_tokens,
            self.checkpoint_tokens,
            self.bootstrap_context_tokens,
        ]
        .into_iter()
        .flatten()
        .collect();
        if known.is_empty() {
            None
        } else {
            Some(known.into_iter().sum())
        }
    }

    /// What the estimate is made of, for a routing explanation — names which
    /// components were counted and which were not, and says outright that
    /// likely repository reads are never counted at all. Reports counts and
    /// component names only, never memory or checkpoint content: this
    /// module counts tokens, it does not quote text.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        match self.project_memory_tokens {
            Some(tokens) => parts.push(format!("project memory ~{tokens} tokens")),
            None => parts.push("project memory not measured".to_owned()),
        }
        match self.checkpoint_tokens {
            Some(tokens) => parts.push(format!("checkpoint ~{tokens} tokens")),
            None => parts.push("checkpoint not measured".to_owned()),
        }
        match self.bootstrap_context_tokens {
            Some(tokens) => parts.push(format!("bootstrap context ~{tokens} tokens")),
            None => parts.push("bootstrap context not measured".to_owned()),
        }
        parts.push("likely repository reads always omitted (unpredictable)".to_owned());
        parts.join("; ")
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
    /// Phase 32E line 1280: what `super::burn::forecast` made of this
    /// destination's resource, resolved by the caller from the evidence
    /// ledger's own rows and the same `CapacityState` `capacity_facts` came
    /// from. `None` — the default, and the value on every build that reads
    /// no ledger — makes `super::pressure::exhaustion_forecast_pressure`
    /// inert.
    burn_forecast: Option<super::burn::ExhaustionForecast>,
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
    /// Map line 1970's axis: the model's own user-assigned capability tier,
    /// attached via [`Self::with_capability_tier`]. `None` — the default —
    /// means nobody assigned this model a tier, and [`super::same_capability_tier`]
    /// reads that as *unknown*, never as a match.
    ///
    /// Deliberately its own field rather than a read of [`Self::tier_ceiling`]
    /// at the fallback call site: `tier_ceiling` is a hard constraint
    /// [`hard_constraint`] and [`workload_tier_fit`] gate on, and keeping the
    /// fallback's identity axis separate means the two can diverge later
    /// without either call site re-deriving the other's meaning — the
    /// accepted wiring shape from `docs/product/design-decisions.md`'s
    /// Phase 56A "Step 5" addendum. Populated by the same caller that
    /// attaches `tier_ceiling`, from the same resolved value: `main.rs`'s
    /// `routing_destinations`, at the point `destination_tier_ceiling`
    /// already calls `resolved_ceiling`.
    capability_tier: Option<WorkloadTier>,
    /// Phase 36: what was read about this session's native context, attached
    /// via [`Self::with_session_context`]. [`SessionContextFacts::UNREAD`] for
    /// a fresh destination and for any caller that did not look.
    context: SessionContextFacts,
    /// Phase 56 line 1946: the entitlement that would be charged for work
    /// on this destination, attached via [`Self::with_entitlement`] by the
    /// caller that resolved it from configuration. `None` — the default —
    /// means no entitlement describes this destination's resource (a
    /// gateway-backed profile, whose upstream is assigned when the session
    /// starts; a direct provider no `[entitlements]` entry names), and the
    /// entitlement constraint then does nothing to it: nobody's rule can
    /// refuse a resource nobody's rule describes. A harness's own sign-in
    /// always arrives with one, because configuration supplies a default
    /// entry for it.
    entitlement: Option<super::Entitlement>,
    /// Map lines 1298, 1299 and 1304: what the caller measured about this
    /// decision's own input size, attached via
    /// [`Self::with_estimated_input_size`]. [`EstimatedInputSize::UNESTIMATED`]
    /// for any caller that did not measure — including every live (warm)
    /// session, on purpose.
    estimated_input_size: EstimatedInputSize,
    /// Line 1923's decay: how many local observations exist for this
    /// destination's own harness-model-route pairing, attached via
    /// [`Self::with_pairing_prior_evidence`]. `0` — the default, and what
    /// every destination carries until a caller populates it — is the honest
    /// floor [`pairing_prior`] reads as "little local evidence", the same
    /// wiring-now/populate-later shape [`Self::capability_tier`] and
    /// [`EstimatedInputSize`] already carry: `main.rs` does not call the
    /// builder below yet, so every destination it constructs keeps the
    /// starting prior until a follow-up package reads
    /// `crate::routing::evidence`'s own counts and attaches them here.
    pairing_prior_evidence: u32,
    /// Map lines 1351/1352/1542/1543/1544: this destination's own
    /// responsiveness and reliability reading, attached via
    /// [`Self::with_route_responsiveness`]. `None` for a destination whose
    /// caller read no ledger, or whose backend names no configured provider
    /// — the honest floor on which `responsiveness`, `tool_round_rate` and
    /// `observed_pairing_reliability` are all inert.
    route_responsiveness: Option<super::evidence::RouteResponsiveness>,
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
            burn_forecast: None,
            provider_protocols: pairing::wire_protocol_from_slug(&protocol)
                .into_iter()
                .collect(),
            resource_facts: ResourceFacts::UNVERIFIED,
            tier_ceiling: None,
            capability_tier: None,
            context: SessionContextFacts::UNREAD,
            entitlement: None,
            estimated_input_size: EstimatedInputSize::UNESTIMATED,
            pairing_prior_evidence: 0,
            route_responsiveness: None,
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

    /// Attach the entitlement this destination would charge — Phase 56
    /// lines 1946 and 1954. The caller resolves it from
    /// `crate::config::EffectiveConfig::entitlement_for`; this module reads
    /// no configuration of its own. `None` is "no entitlement describes this
    /// resource", on which the entitlement constraint is inert.
    #[must_use]
    pub fn with_entitlement(mut self, entitlement: Option<super::Entitlement>) -> Self {
        self.entitlement = entitlement;
        self
    }

    pub fn entitlement(&self) -> Option<&super::Entitlement> {
        self.entitlement.as_ref()
    }

    pub fn capacity_facts(&self) -> CapacityFacts {
        self.capacity_facts
    }

    /// Attach the exhaustion forecast for this destination's resource — line
    /// 1280. The caller resolves it from `super::burn::forecast` over the
    /// rows `crate::routing::evidence::EvidenceLedger::consumption_in_window`
    /// returned; this module reads no ledger of its own, exactly as it reads
    /// no telemetry of its own for `capacity`.
    #[must_use]
    pub fn with_burn_forecast(mut self, forecast: Option<super::burn::ExhaustionForecast>) -> Self {
        self.burn_forecast = forecast;
        self
    }

    pub fn burn_forecast(&self) -> Option<super::burn::ExhaustionForecast> {
        self.burn_forecast
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
    /// **The production caller is `main.rs::routing_destinations`**, via
    /// `destination_tier_ceiling`'s reading of
    /// [`crate::config::EffectiveConfig::model_ceiling`], which (Phase 34F)
    /// reads `providers.<p>.model_ceilings` (map line 1796) *or* a matching
    /// `providers.<p>.model_capabilities` ceiling through
    /// [`crate::config::capability::CeilingResolution::hard_ceiling`] — only
    /// a record the user assigned themselves, never a benchmark-provenance
    /// prior (map line 1484).
    ///
    /// `None` for most destinations by design: a ceiling exists only where
    /// the user stated one for that specific model on that provider —
    /// "nobody has said" is not "cannot".
    // History: design-decisions.md, "Trims: routing module docs, second packet", routing/session/mod.rs `fn with_tier_ceiling`.
    pub fn with_tier_ceiling(mut self, ceiling: Option<WorkloadTier>) -> Self {
        self.tier_ceiling = ceiling;
        self
    }

    pub fn tier_ceiling(&self) -> Option<WorkloadTier> {
        self.tier_ceiling
    }

    /// Attach the model's own user-assigned capability tier — map line
    /// 1970's axis, and the seam [`super::same_capability_tier`] reads.
    /// `None` withdraws the fact, which the tier-preserving fallback steps
    /// then read as unknown rather than as a match — unknown never widens
    /// the fallback (capability map line 1970's ruling).
    ///
    /// **The production caller is `main.rs::routing_destinations`**, which
    /// attaches every destination the shipped binary builds from the same
    /// resolved value [`Self::with_tier_ceiling`] carries, at the same call
    /// site — the two fields answer different questions about that one
    /// resolution rather than being derived from each other.
    pub fn with_capability_tier(mut self, tier: Option<WorkloadTier>) -> Self {
        self.capability_tier = tier;
        self
    }

    pub fn capability_tier(&self) -> Option<WorkloadTier> {
        self.capability_tier
    }

    /// Attach what the caller read about this session's native context —
    /// Phase 36's producers, lines 1582–1586. Meaningful only on an existing
    /// session: [`session_affinity`] never reads it off a fresh destination,
    /// which has no context to have facts about.
    ///
    /// **The production caller is `main.rs::routing_destinations`**, which
    /// attaches the session record's compaction count, the sticky
    /// classification cache's last task when it names this session, and the
    /// session's own latest checkpoint's file list.
    pub fn with_session_context(mut self, facts: SessionContextFacts) -> Self {
        self.context = facts;
        self
    }

    pub fn session_context(&self) -> &SessionContextFacts {
        &self.context
    }

    /// Attach what the caller measured about this decision's own input
    /// size — map lines 1298, 1299 and 1304. The default is
    /// [`EstimatedInputSize::UNESTIMATED`], on which this decision's own
    /// expected-marginal-cost term can state a rate but not a cost.
    ///
    /// **The production caller is `main.rs::routing_destinations`.**
    pub fn with_estimated_input_size(mut self, size: EstimatedInputSize) -> Self {
        self.estimated_input_size = size;
        self
    }

    pub fn estimated_input_size(&self) -> &EstimatedInputSize {
        &self.estimated_input_size
    }

    /// Attach how many local observations exist for this destination's own
    /// harness-model-route pairing — line 1923's decay for [`pairing_prior`].
    /// The default is `0`, on which the term reads as a fresh session with
    /// little local evidence, whatever [`Self::pairing_prior_evidence`]
    /// returns until a caller attaches a real count.
    ///
    /// No production caller attaches this today — see the field's own doc.
    pub fn with_pairing_prior_evidence(mut self, count: u32) -> Self {
        self.pairing_prior_evidence = count;
        self
    }

    pub fn pairing_prior_evidence(&self) -> u32 {
        self.pairing_prior_evidence
    }

    /// Attach this destination's own responsiveness and reliability reading
    /// — map lines 1351/1352/1542/1543/1544, computed by the caller from
    /// `crate::routing::evidence::RouteResponsiveness::from_observations`
    /// over the launch's own `consumption` slice, filtered to this
    /// destination's `(provider, model)`. `None` — the default — is what
    /// every destination carries until a caller attaches a real reading, on
    /// which every one of the three terms this feeds is inert and says so.
    #[must_use]
    pub fn with_route_responsiveness(
        mut self,
        route_responsiveness: Option<super::evidence::RouteResponsiveness>,
    ) -> Self {
        self.route_responsiveness = route_responsiveness;
        self
    }

    pub fn route_responsiveness(&self) -> Option<&super::evidence::RouteResponsiveness> {
        self.route_responsiveness.as_ref()
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
/// `needs_tool_calls` can **reject** a destination — anything a router
/// would only *prefer* belongs in a contribution instead (design decision
/// 1, "additive, never a filter"). `hard_capabilities` carries
/// `TaskClassification::hard_capabilities()`'s own output, feeding both
/// [`capability_fit`] and the hard-constraint gate (map line 1517) — an
/// unverified axis is "nobody has said", not "cannot", and only costs a
/// `capability_fit` contribution.
///
/// `minimum_tier` also rejects (map line 1516), only when a destination's
/// ceiling is *established* below it — see
/// [`Destination::with_tier_ceiling`]. `classification` is `None` for a
/// caller with no task in hand, reproducing the pre-classification
/// explanation byte for byte.
// History: design-decisions.md, "Trims: routing module docs, second packet", routing/session/mod.rs `TaskRequirements`.
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

/// Lines 566, 1540, 1923. What a compatible vendor-native harness-model
/// pairing is worth for a fresh session with little local evidence — a
/// starting assumption, never a quality claim (`PairingClass::is_vendor_native`'s
/// own doc, and the map's first fixed architectural requirement).
///
/// Small on purpose, and bounded two ways: strictly below
/// [`FreePool`]-observed evidence's own weight — one consecutive observed
/// failure ([`HEALTH_FAILURE_PENALTY`], `0.3` in magnitude) already outweighs
/// it, so a single bad exchange on the native candidate settles a tie this
/// term made — and strictly below warmth's `1.5` ceiling (line 569), so a
/// relevant warm session on a non-native candidate always outranks it. `0.2`
/// keeps [`METERED_COST_PREFERENCE`]'s own claim about the smallest magnitude
/// in this module true, sitting instead beside [`CACHE_LIKELY_LOST`] and
/// [`TIER_FIT_HEADROOM`].
const PAIRING_PRIOR: f64 = 0.2;

/// Line 1923's decay: how many local observations [`Destination::pairing_prior_evidence`]
/// must carry before the starting assumption above is considered replaced by
/// what was actually observed. `5`, matching
/// [`crate::routing::evidence::MIN_SAMPLE_FOR_SUMMARY`] and
/// `crate::config::pairing::CONFIDENT_AT_OBSERVATIONS`'s own choice — not
/// because the numbers must agree, but because all three answer the same
/// underlying question ("how many local observations before this project
/// trusts them at all"), and picking a fourth number with no evidence either
/// way would be exactly the unearned precision line 1234 forbids on the quota
/// side.
const PAIRING_PRIOR_EVIDENCE_THRESHOLD: u32 = 5;

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

/// Map lines 1535/1545. What a destination's own measured prompt-cache read
/// history is worth, at either end of
/// [`crate::routing::evidence::RouteResponsiveness::cache_read_ratio`].
///
/// Strictly below [`CACHE_LIKELY_LOST`] — the *weaker* of [`prompt_cache_state`]'s
/// two magnitudes ([`CACHE_PRESERVED`] is the other) — on purpose:
/// [`prompt_cache_state`] establishes a locality **fact** about this specific
/// move (same backend, same credential, or neither), where this term is a
/// historical **average** over past exchanges against this destination's
/// `(provider, model)`, which says nothing about whether *this* move
/// preserves a prefix. A measured signal must never outrank the weakest
/// structural one it sits beside, so even a destination with a perfect
/// observed warm-cache record scores less than a destination merely
/// *likely* to have kept its cache from the last move.
///
/// [`prompt_cache_state`]: scoring::prompt_cache_state
const MEASURED_CACHE_TEMPERATURE_MAGNITUDE_CEILING: f64 = 0.1;

/// Map line 1534. Equal to
/// [`MEASURED_CACHE_TEMPERATURE_MAGNITUDE_CEILING`] and, like it, strictly
/// below [`CACHE_LIKELY_LOST`] in `scoring.rs` — a size reading never
/// outweighs a structural fact about the move in front of it. See
/// `design-decisions.md`, *"Context size is read off the gateway's own
/// exchange, never guessed"*, for the full reasoning. This is the **warm**
/// ceiling; see [`CONTEXT_QUALITY_MAGNITUDE_CEILING_COLD`] for the other one.
const CONTEXT_QUALITY_MAGNITUDE_CEILING: f64 = 0.1;

/// Map line 1594. A cold session has no cache left to lose, no affinity to
/// outrank and nothing to resume but its size, so size may weigh what
/// carrying it actually costs. See `design-decisions.md`, *"A fresh session
/// over a cold and bloated one"*, for the crossover this produces against
/// [`BOOTSTRAP_COST_WITH_CHECKPOINT`].
const CONTEXT_QUALITY_MAGNITUDE_CEILING_COLD: f64 = 0.4;

/// Map line 1534. The size a working context normally sits at — under this,
/// [`crate::routing::session::scoring::context_quality`] contributes exactly
/// `0.0`.
const CONTEXT_LEAN_TOKENS: i64 = 32_000;

/// Map line 1534. The span, added to [`CONTEXT_LEAN_TOKENS`], at which the
/// penalty reaches [`CONTEXT_QUALITY_MAGNITUDE_CEILING`] — 160,000 tokens,
/// where every shipped frontier window has either compacted or is about to.
const CONTEXT_BLOAT_SPAN_TOKENS: i64 = 128_000;

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

/// [`quota_pressure`] and [`provider_health`]'s four weights, resolved as one
/// value — capability map lines 1357/1358: the four constants above are an
/// observed starting policy, not a universal one, and this is what a user's
/// `[routing.score_weights]` overrides.
///
/// [`Default`] reproduces `QUOTA_PRESSURE_WEIGHT`, `HEALTH_FAILURE_PENALTY`,
/// `HEALTH_PENALTY_FLOOR` and `HEALTH_UNAVAILABLE_PENALTY` exactly, so a
/// caller that never resolves configuration — every caller before this type
/// existed, and every [`SessionRouter`] nobody calls
/// [`SessionRouter::with_score_weights`] on — scores byte-identically to
/// before this package. See `crate::config::ScoreWeightsConfig`, where a user
/// overrides these, and `crate::config::EffectiveConfig::score_weights`,
/// which resolves them the same project-over-user-over-default way as every
/// other `[routing]` value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScoreWeights {
    pub quota_pressure_weight: f64,
    pub health_failure_penalty: f64,
    pub health_penalty_floor: f64,
    pub health_unavailable_penalty: f64,
}

impl Default for ScoreWeights {
    fn default() -> Self {
        Self {
            quota_pressure_weight: QUOTA_PRESSURE_WEIGHT,
            health_failure_penalty: HEALTH_FAILURE_PENALTY,
            health_penalty_floor: HEALTH_PENALTY_FLOOR,
            health_unavailable_penalty: HEALTH_UNAVAILABLE_PENALTY,
        }
    }
}

/// Line 1546. What being inside a wait a provider itself declared costs a
/// destination — a fact [`provider_health`] does not price, because an
/// invented cooldown scores there too and this term must not agree with that
/// one for a reason it was never told.
const CADENCE_DECLARED_WAIT_PENALTY: f64 = -1.5;

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

// ---------------------------------------------------------------------------
// Phase 36, lines 1582–1587: the facets of the affinity score. Each is a
// named constant so that a mutation can move exactly one, and each is small
// against warmth's `1.5` ceiling on purpose — warmth is the one signal here
// that is measured rather than inferred, and no inferred facet may outweigh
// it on its own.
// ---------------------------------------------------------------------------

/// Line 1582. What it is worth that the last task classified onto a session
/// was classed the same way as this one — the nearest thing to task identity
/// this build stores (the sticky classification cache keeps a classification,
/// never task text).
const SAME_TASK_AFFINITY: f64 = 0.5;

/// Line 1583. The full value of a session whose latest checkpoint lists every
/// path the task names; scaled by the fraction it lists.
const TOUCHED_FILES_AFFINITY: f64 = 0.6;

/// Line 1584. A session whose native context was never compacted and is
/// still inside the warm-session relevance window holds exactly what was
/// said to it, which is the only sense of "semantically useful" this build
/// can observe.
const NATIVE_CONTEXT_INTACT: f64 = 0.3;

/// Line 1585. What a prompt cache that is *likely* still hot is worth. Below
/// [`CACHE_PRESERVED`] because that term prices a locality Glasshouse can
/// establish and this one prices a lifetime it only reasons about.
const PROMPT_CACHE_HOT: f64 = 0.4;

/// Line 1585's lifetime: five minutes. The shortest published default among
/// the providers in scope — Anthropic documents its prompt-cache lifetime as
/// five minutes by default, refreshed on each use
/// (`docs.anthropic.com/en/docs/build-with-claude/prompt-caching`) — and the
/// same figure the session store's advisory cache state reasons from. A
/// reasoned constant, not a reading: no provider reports a cache hit.
const PROMPT_CACHE_TTL_SECONDS: i64 = 5 * 60;

/// Line 1586. From how many observed compactions a session's context is
/// priced as noisy: each compaction replaces context with a summary of it,
/// and by the third the session is mostly summaries of summaries.
const NOISY_COMPACTION_COUNT: i64 = 3;
/// Per observed compaction at or past that count, bounded.
const COMPACTION_NOISE_PENALTY: f64 = -0.2;
const COMPACTION_NOISE_FLOOR: f64 = -0.6;

/// Line 1586. A session whose last classified task was classed differently
/// from this one, or whose checkpoint lists files and none the task names.
const UNRELATED_TASK_PENALTY: f64 = -0.3;
const UNRELATED_FILES_PENALTY: f64 = -0.3;

/// Line 1587. What significant pressure on the session's quota resource
/// takes off its affinity. Read from the **same** capacity reading
/// [`quota_pressure`] prices — the band the caller derived from it with the
/// user's thresholds — never a second reading; "significant" is the reserve
/// band or below, the same threshold `super::pressure` gates spending at.
const QUOTA_PRESSURE_AFFINITY_PENALTY: f64 = -0.4;

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

/// Map line 1538. What a metered destination costs against a free one,
/// independent of any workload tier — see [`expected_marginal_cost`] for why
/// this is a separate term from [`METERED_COST_PREFERENCE`] rather than a
/// second use of it. Same magnitude as [`METERED_COST_PREFERENCE`]: the two
/// never price the same candidate at once, so there is no compounding to
/// bound against.
const EXPECTED_MARGINAL_COST_PENALTY: f64 = -0.1;

/// Phase 32G line 1302: the name every `request_pool_cost` contribution
/// carries, named once so a reader and a mutation both spell it the same way.
const REQUEST_POOL_COST_TERM: &str = "request-pool cost";

/// Line 1302's ceiling magnitude — strictly below
/// [`super::pressure::EXHAUSTION_FORECAST_PENALTY`]'s `-0.7`. That term owns
/// the case this one is inert for (a forecast that will not survive to its
/// reset — `phase-32g.md`'s 1302 entry: "one forecast is priced once"), so
/// this term prices the milder case beside it — a pool spending fast but
/// still expected to make its reset — and must never outweigh the term that
/// fires when the outlook is actually worse. Also strictly below warm
/// affinity's `1.5` ceiling, per the packet.
const REQUEST_POOL_COST_PENALTY: f64 = -0.5;

/// The reference horizon [`request_pool_cost`]'s curve treats as
/// "comfortably plenty" — the point at which the magnitude has fallen to half
/// [`REQUEST_POOL_COST_PENALTY`]. Not a claim about any provider's reset:
/// that reasoning belongs to [`super::burn::WELL_BEFORE_RESET_FRACTION`]
/// alone. Twelve hours, chosen against a working day: a pool projected to
/// last a full session's length is barely priced, and one projected to run
/// dry within an hour or two is priced near the ceiling.
const REQUEST_POOL_COST_HALF_LIFE_HOURS: f64 = 12.0;

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
    /// Lines 1559–1565: what this decision did to the tier it prefers.
    /// `None` when no tier was stated — no task, nothing to move — and
    /// `Some(Held { .. })` when one was and nothing moved, with the reason.
    movement: Option<TierMovement>,
    /// Map line 1970: the fallback across the pool this decision made, when
    /// it made one. `None` is *no fallback happened* — the ordinary case,
    /// and the one in which this decision is byte-identical to the one this
    /// router made before line 1970 existed. There is no `Held` shape here,
    /// unlike [`TierMovement`]: a tier is decided on every classified
    /// launch and so has something to say when it stands, while a fallback
    /// is an event that either occurred or did not.
    fallback: Option<EntitlementFallback>,
    /// Map line 1307: the marginal input cost this decision actually used
    /// for [`Self::chosen`], computed once by [`estimated_cost`] and carried
    /// here rather than recomputed by whatever records it. `None` when
    /// either half of the multiplication — the price, or this decision's
    /// own input-size estimate — was unknown; never a fabricated zero.
    cost: Option<ObservedCost>,
}

impl Routed {
    /// Where the work goes.
    pub fn chosen(&self) -> &Destination {
        &self.chosen
    }

    /// Map line 1307: the estimated cost this decision actually used, when
    /// both halves of the multiplication — the price, and this decision's
    /// own input-size estimate — were known. A caller recording this writes
    /// at most one row per decision, and none at all when it is `None`.
    pub fn cost(&self) -> Option<ObservedCost> {
        self.cost
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

    /// Lines 1559–1565: the tier movement this decision made, when a tier
    /// was stated at all. [`TierMovement::fired`] tells a moved preference
    /// from one that was held with a reason.
    pub fn movement(&self) -> Option<&TierMovement> {
        self.movement.as_ref()
    }

    /// Map line 1970: the pool fallback this decision made, or `None` when
    /// it made none. **Zero fallbacks is `None` and never an empty record**
    /// — a caller recording these writes one row per `Some` and nothing at
    /// all otherwise.
    pub fn fallback(&self) -> Option<&EntitlementFallback> {
        self.fallback.as_ref()
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
        // Line 1565's visibility half: a moved tier is never only a term
        // inside a candidate's block — it is a heading a person reads first.
        if let Some(movement) = self.movement.as_ref().filter(|m| m.fired()) {
            let _ = writeln!(out, "tier         {}", movement.describe());
        }
        // Line 1970's visibility half, on the same terms as the moved tier
        // above: a fallback moved the work to another account, which is a
        // heading a person reads first rather than a term inside one
        // candidate's block.
        if let Some(fallback) = self.fallback.as_ref() {
            let _ = writeln!(out, "fallback     {}", fallback.describe());
        }
        // Map line 1307's visibility half: the figure a later evaluation
        // will compare against actual usage is worth a heading, not only a
        // buried term — `cost` is `None` whenever either half of the
        // multiplication was unknown, in which case nothing is printed
        // rather than a fabricated zero.
        if let Some(cost) = self.cost {
            let _ = writeln!(
                out,
                "cost         ${:.4} estimated ({})",
                cost.micro_usd as f64 / 1_000_000.0,
                cost.confidence.as_str()
            );
        }
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

/// What step 2 of [`SessionRouter::choose`] settled: who survived every hard
/// constraint, who did not and why, and the tier movement decided between the
/// two halves. Private — the only readers are `choose` and
/// [`SessionRouter::refused`].
struct Gate {
    eligible: Vec<super::EligibleCandidate<Destination>>,
    rejected: Vec<(Destination, HardConstraint)>,
    movement: Option<TierMovement>,
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
    /// Lines 1294 and 1610: the sessions whose operators declared their
    /// current task nearly complete, as
    /// `crate::session::SessionStore::active_task_progress` resolved them —
    /// which reports no declaration that has expired or whose session is no
    /// longer live, so a stale statement never reaches this set. Read at
    /// exactly one place, [`SessionRouter::task_nearly_complete`], which is
    /// true only of an existing session somebody declared.
    ///
    /// **Declared, never inferred.** Nothing here derives this from a turn
    /// count, an elapsed time or any other observable; the set is what
    /// somebody said on purpose. The reserve policy takes this as its first
    /// branch, outranking every other signal including the override above,
    /// so a value Glasshouse had worked out for itself would invert the
    /// protection rather than approximate it — see
    /// [`crate::provider::quota::ReserveDecisionInputs::task_nearly_complete`].
    ///
    /// Restated here as a set rather than imported, for
    /// `reserve_override_sessions`' reason: this module may not reach the
    /// throwaway-job policy class
    /// (`super::tests::the_session_router_cannot_reach_the_disposable_policy_class`).
    declared_task_progress_sessions: BTreeSet<String>,
    /// Line 1564: the failure class the most recent exchange on the
    /// *current* destination's backend recorded, when the caller looked one
    /// up — see [`Self::with_retry_after`].
    retry_after: Option<FailureClass>,
    /// Lines 1305/1306: provider price metadata, as the caller resolved and
    /// read it — see [`Self::with_price_table`]. Defaults to
    /// [`PriceTable::empty`], which is what every candidate saw before this
    /// package and what every candidate with no metadata file still sees:
    /// [`expected_marginal_cost`] renders that as an honest unknown, never
    /// as a free zero.
    prices: PriceTable,
    /// Capability map lines 1357/1358: the resolved score weights, as the
    /// caller's configuration decided them — see [`Self::with_score_weights`].
    /// `ScoreWeights::default()` reproduces today's compile-time constants,
    /// so not calling this builder scores exactly as before this field
    /// existed.
    score_weights: ScoreWeights,
    /// Map line 1951's producer, as the caller resolved it — see
    /// [`Self::with_harness_efficiency`]. `HarnessEfficiencySummary::empty()`
    /// is what every candidate scored before this field existed, and is what
    /// `harness_efficiency` reads as inert.
    harness_efficiency: HarnessEfficiencySummary,
    /// Map line 1301's other half, as the caller resolved it — see
    /// [`Self::with_comparable_output_tokens`]. Empty is what every candidate
    /// saw before this field existed: [`expected_marginal_cost`] renders that
    /// as an honest *unmeasured*, never as a fabricated size.
    comparable_output: Vec<ClassOutput>,
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

    /// Lines 1294 and 1610: the sessions whose operators declared their
    /// current task nearly complete. Declaring none is the same as not
    /// calling this, which is what every caller predating those lines does
    /// and what keeps their rankings byte-identical.
    #[must_use]
    pub fn with_declared_task_progress<S: Into<String>>(
        mut self,
        sessions: impl IntoIterator<Item = S>,
    ) -> Self {
        self.declared_task_progress_sessions = sessions.into_iter().map(Into::into).collect();
        self
    }

    /// Line 1564: tell the router how the last attempt at this work ended.
    ///
    /// `Some(class)` is what the evidence ledger recorded for the most recent
    /// exchange on the current destination's provider and model —
    /// `main.rs`'s task-boundary `route` path reads it through
    /// `EvidenceLedger::latest_failure_class_for_model`. Only a
    /// [`FailureClass::RequestIncompatibility`] or
    /// [`FailureClass::EmptyCompletion`] promotes (the model could not do the
    /// work); every other class is a provider-health or quota fact that
    /// [`provider_health`] and the pressure terms already price, and the
    /// explanation says so rather than promoting on it.
    ///
    /// A builder on the router rather than a field on [`RouterInputs`]: that
    /// struct is written as a literal at every caller in and out of this
    /// crate, and a per-decision fact the caller may not have looked up
    /// belongs beside the other caller-resolved state this router carries.
    #[must_use]
    pub fn with_retry_after(mut self, class: Option<FailureClass>) -> Self {
        self.retry_after = class;
        self
    }

    /// Lines 1305/1306: the provider price metadata the caller resolved —
    /// normally `PriceTable::load_from_dir(paths.config_dir())`, read once
    /// per decision the same way every other caller-resolved fact on this
    /// router is. Not calling this at all keeps `PriceTable::empty()`, which
    /// reproduces this router's behaviour before this package existed
    /// byte-for-byte: this is a builder rather than a required constructor
    /// argument for exactly that reason, the same one [`Self::with_retry_after`]
    /// gives for its own field.
    #[must_use]
    pub fn with_price_table(mut self, prices: PriceTable) -> Self {
        self.prices = prices;
        self
    }

    /// Capability map lines 1357/1358 — normally
    /// `effective.score_weights().value`, read once per decision the same way
    /// every other caller-resolved fact on this router is. Not calling this
    /// at all keeps `ScoreWeights::default()`, which reproduces this router's
    /// behaviour before this field existed byte-for-byte: this is a builder
    /// rather than a required constructor argument for the same reason
    /// [`Self::with_price_table`] is.
    #[must_use]
    pub fn with_score_weights(mut self, weights: ScoreWeights) -> Self {
        self.score_weights = weights;
        self
    }

    /// Map line 1952: the per-(harness, task class) efficiency summary, as
    /// the caller resolved it — normally
    /// `EvaluationObservations::outcomes_by_tier_and_harness` over the same
    /// window `glasshouse route`'s report reads (map line 1951), reduced
    /// with [`HarnessEfficiencySummary::from_outcomes`]. Not calling this at
    /// all keeps [`HarnessEfficiencySummary::empty()`], which reproduces
    /// this router's behaviour before this field existed byte-for-byte —
    /// the same reason [`Self::with_retry_after`] gives for its own field: a
    /// per-decision fact the caller may not have looked up belongs beside
    /// the other caller-resolved state this router carries, not folded into
    /// [`RouterInputs`], which is written as a literal at every caller in
    /// and out of this crate.
    #[must_use]
    pub fn with_harness_efficiency(mut self, summary: HarnessEfficiencySummary) -> Self {
        self.harness_efficiency = summary;
        self
    }

    /// Map line 1301: the recent comparable-task output-token sizes, as the
    /// caller resolved them — normally
    /// `super::burn::output_tokens_by_class` over the same window the price
    /// table's own caller reads, read once per decision the same way every
    /// other caller-resolved fact on this router is. Not calling this at all
    /// keeps an empty `Vec`, which reproduces this router's behaviour before
    /// this field existed byte-for-byte: `expected_marginal_cost` renders
    /// every class as *unmeasured*, the same words a class below the
    /// standing floor gets.
    #[must_use]
    pub fn with_comparable_output_tokens(mut self, comparable_output: Vec<ClassOutput>) -> Self {
        self.comparable_output = comparable_output;
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

    /// Whether `destination`'s session had its current task declared nearly
    /// complete — lines 1294 and 1610, true only for an existing session
    /// somebody declared.
    ///
    /// A fresh destination has no session anyone could have declared, which
    /// is also why the guard cannot make the router keep work it has not
    /// started: the branch it feeds protects work already in flight, and a
    /// fresh candidate has none.
    fn task_nearly_complete(&self, destination: &Destination) -> bool {
        !destination.is_fresh()
            && self
                .declared_task_progress_sessions
                .contains(destination.id())
    }

    /// Step 2 of [`Self::choose`], in two halves — the one place the hard
    /// constraints run, so the path that acts and the path that reports a
    /// refusal cannot disagree about what was refused.
    ///
    /// The tier-movement decision (lines 1559–1565) has to see the candidate
    /// set — "every candidate at the classified tier is struggling" is a fact
    /// about the set — and it has to be settled *before* the tier gate,
    /// because a downgrade admits a resource the classified tier would have
    /// refused. So the capability constraints run first, the movement is
    /// decided over what survived them, and the tier gate then reads the tier
    /// the movement settled. An escalation never changes the gate: it moves a
    /// preference, and a preference does not remove a candidate (design
    /// decision 1, "additive, never a filter").
    fn gate(&self, destinations: &[Destination], inputs: &RouterInputs<'_>) -> Gate {
        // Line 1953's model half is part of the entitlement AXIS: it fires
        // only when the offered set actually carries a pool (two or more
        // configured entitlements). A user with zero or one entitlement
        // keeps today's launches — their possibly-stale model catalogue may
        // not refuse the only account they have — which is the same
        // preservation clause the score's pool gate enforces, read off the
        // raw offered set because a refusal must not depend on what some
        // *other* constraint already removed.
        let entitlement_axis = EntitlementPoolView::of(destinations).offers_a_choice();
        let capability_survivors: Vec<&Destination> = destinations
            .iter()
            .filter(|destination| {
                hard_constraint(destination, inputs, None, entitlement_axis).is_ok()
            })
            .collect();
        let movement = decide_tier_movement(&capability_survivors, inputs, self.retry_after);
        let gate_tier = match &movement {
            Some(movement) => Some(movement.gate_tier()),
            None => inputs.requirements.minimum_tier,
        };
        let (eligible, rejected) =
            apply_hard_constraints(destinations.to_vec(), |destination: &Destination| {
                hard_constraint(destination, inputs, gate_tier, entitlement_axis)
            });
        Gate {
            eligible,
            rejected,
            movement,
        }
    }

    /// Every destination a hard constraint would remove for this work, and
    /// which constraint — the same gate [`Self::choose`] runs, exposed for the
    /// one case `choose` cannot report: **every** destination refused and no
    /// current destination to hold, where it answers `None` and the rejections
    /// would otherwise be lost.
    ///
    /// Phase 56 line 1954 is why that case matters. Before this method, a
    /// launch whose only destination was refused read `None` as "nowhere to
    /// go" and proceeded on that destination anyway — the silence Phase 35D's
    /// decision 3 recorded, and the one outcome *never charge a task to a
    /// subscription the user's rules did not allow* cannot tolerate. The
    /// launch path asks this when `choose` answers `None` for a non-empty set,
    /// and refuses by name.
    pub fn refused(
        &self,
        destinations: &[Destination],
        inputs: &RouterInputs<'_>,
    ) -> Vec<(Destination, HardConstraint)> {
        self.gate(destinations, inputs).rejected
    }

    /// Choose where this work goes — the router's one production entry
    /// point. `current` is where the work is now (`None` at session
    /// start); `destinations` is every place it could go, in the caller's
    /// order, the tiebreaker throughout this module's siblings. `None`
    /// only when `destinations` is empty **and** there is no current
    /// destination to hold.
    ///
    /// Order: (1) line 1592's boundary gate — mid-turn, nothing is ranked
    /// and the current destination returns with an explanation, unless
    /// [`RoutingOverride::and_route_now`] lifts it; (2) hard constraints
    /// via [`super::apply_hard_constraints`], structurally excluding a
    /// destination (tool calls, protocol, minimum tier — line 1516); (3)
    /// soft contributions (lines 1595–1600, capability fit, tier fit),
    /// summed by [`RoutingExplanation::total`] — none of these can
    /// exclude; (4) the user's override (line 1602), applied over the
    /// ranking and never over step 2, recorded so a reader can see what
    /// was overruled.
    // History: design-decisions.md, "Trims: routing module docs, second packet", routing/session/mod.rs `fn choose`.
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
            let cost = estimated_cost(&held, &self.prices);
            return Some(Routed {
                moment,
                re_decided: false,
                chosen: held,
                explanation,
                considered: Vec::new(),
                rejected: Vec::new(),
                automatic: None,
                override_refused: None,
                movement: None,
                fallback: None,
                cost,
            });
        }

        if destinations.is_empty() {
            return None;
        }

        // Step 2, in two halves. The tier-movement decision (lines
        // 1559–1565) has to see the candidate set — "every candidate at the
        // classified tier is struggling" is a fact about the set — and it
        // has to be settled *before* the tier gate, because a downgrade
        // admits a resource the classified tier would have refused. So the
        // two capability constraints run first, the movement is decided over
        // what survived them, and the tier gate then reads the tier the
        // movement settled. An escalation never changes the gate: it moves
        // a preference, and a preference does not remove a candidate
        // (design decision 1, "additive, never a filter").
        let Gate {
            eligible,
            rejected,
            movement,
        } = self.gate(destinations, inputs);

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
            let cost = estimated_cost(&held, &self.prices);
            return Some(Routed {
                moment,
                re_decided: true,
                chosen: held,
                explanation,
                considered: Vec::new(),
                rejected,
                automatic: None,
                override_refused: None,
                movement,
                fallback: None,
                cost,
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
        // The entitlement axis of the eligible set (lines 1953/1966–1969),
        // computed once like the alternatives below: a candidate a hard
        // constraint removed is not a pool member anything can be spread to.
        let pool = EntitlementPoolView::of(&candidates);
        // Map line 1952's own axis: which harnesses this decision is
        // actually choosing among. A set `launch_session` already scoped to
        // one assigned harness collapses this to a single entry, which is
        // what makes `harness_efficiency`'s "no other harness to compare
        // against" case the assertion the packet asks for, rather than code
        // that rebuilds the scoping `launch_session` already did.
        let candidate_harnesses: BTreeSet<&str> = candidates
            .iter()
            .map(|destination| destination.harness().slug())
            .collect();
        // Line 1352's own comparison: the lowest (best) effective TTFC
        // among the candidates this decision is actually choosing between —
        // computed once here, the same shape `candidate_harnesses` above
        // already takes, so `responsiveness` scales every candidate against
        // the field it is being compared within rather than an absolute
        // constant.
        let best_effective_ttfc_ms: Option<f64> = candidates
            .iter()
            .filter_map(Destination::route_responsiveness)
            .filter_map(super::evidence::RouteResponsiveness::effective_ttfc_ms)
            .fold(None, |best: Option<f64>, ttfc| {
                Some(best.map_or(ttfc, |current_best: f64| current_best.min(ttfc)))
            });
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
                    // inert on it for the low-tier term. A moved tier reaches
                    // the pressure terms only downward — see
                    // [`TierMovement::pressure_tier`].
                    tier: movement.as_ref().map(TierMovement::pressure_tier),
                    existing: !destination.is_fresh(),
                    alternatives: &alternatives,
                    policies: self.reserve_policies,
                    scope: ReserveScope::Interactive,
                    user_override: self.reserve_overridden(destination),
                    task_nearly_complete: self.task_nearly_complete(destination),
                    forecast: destination.burn_forecast(),
                };
                let explanation = score(
                    destination,
                    current,
                    inputs,
                    &pressure,
                    movement.as_ref(),
                    &pool,
                    &self.harness_efficiency,
                    &candidate_harnesses,
                    &self.prices,
                    &self.score_weights,
                    &self.comparable_output,
                    best_effective_ttfc_ms,
                );
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
        let (ranked_index, refusal) = self.apply_override(&scored, current, &rejected);
        let overrode = (scored[ranked_index].0.id() != automatic_id).then(|| automatic_id.clone());

        // Step 5, map line 1970: the post-ranking reselection. It runs
        // after the override, over the same already-gated list, so it can
        // only move the work to a candidate every hard constraint already
        // admitted (line 1971).
        //
        // It never moves an account the user named: naming an exact id
        // (`@<account>`) or `--hold` takes the account out of Glasshouse's
        // hands, while a bare `fresh:<harness>:<profile>` or `--fresh`
        // names only the profile, leaving the account — and the fallback
        // — to Glasshouse (line 1969). An override that was refused leaves
        // the ranking's own answer standing, and the fallback applies to
        // that.
        // History: design-decisions.md, "Trims: routing module docs, second packet", routing/session/mod.rs post-ranking reselection comment in `fn choose`.
        let user_chose = refusal.is_none()
            && match self.user_override.destination() {
                Some(DestinationChoice::To(id)) => id == scored[ranked_index].0.id(),
                Some(DestinationChoice::Hold) => true,
                Some(DestinationChoice::Fresh) | None => false,
            };
        let moved = if user_chose {
            None
        } else {
            let ranked: Vec<&Destination> = scored.iter().map(|(candidate, _)| candidate).collect();
            entitlement_fallback(&ranked, ranked_index, &pool)
        };
        let index = moved.as_ref().map_or(ranked_index, |(to, _)| *to);
        let fallback = moved.map(|(_, record)| record);

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
        // A contribution of zero magnitude, like the override's own: the
        // fallback did not out-score anything, it moved the winner after the
        // scoring was over. Its evidence is the whole record, so the block a
        // person reads under `why` says which account was left, which was
        // taken, and under which step of the order.
        if let Some(fallback) = &fallback {
            explanation.push(Contribution::new(
                "entitlement fallback",
                0.0,
                fallback.describe(),
            ));
        }

        let cost = estimated_cost(&chosen, &self.prices);
        Some(Routed {
            moment,
            re_decided: true,
            chosen,
            explanation,
            considered: scored,
            rejected,
            automatic: overrode,
            override_refused: refusal,
            movement,
            fallback,
            cost,
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
            DestinationChoice::To(id) => match scored
                .iter()
                .position(|(d, _)| destination_answers_to(d.id(), id))
            {
                Some(index) => (index, None),
                None => match rejected
                    .iter()
                    .find(|(d, _)| destination_answers_to(d.id(), id))
                {
                    Some((_, constraint)) => (
                        0,
                        Some(OverrideRefusal::Ineligible(id.clone(), constraint.clone())),
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

/// Whether a destination's id answers to the name an override used: its
/// exact id, or — for a fresh candidate the entitlement axis split into
/// several (`fresh:<harness>:<profile>@<entitlement>`, line 1953) — the
/// shared `fresh:<harness>:<profile>` prefix. So `--to` naming the profile
/// still works on a pool: the person chose the profile, and the ranking
/// still chooses the account among that profile's candidates (line 1969's
/// "bound to the pool's choice, not to one account"), because
/// `apply_override` scans `scored` best-first. An exact id, `@` suffix
/// included, pins the account too.
fn destination_answers_to(destination_id: &str, named: &str) -> bool {
    destination_id == named
        || (destination_id.len() > named.len()
            && destination_id.starts_with(named)
            && destination_id.as_bytes()[named.len()] == b'@')
}
