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
//! # The native-pairing prior, and why it belongs here and not one layer down
//!
//! `docs/product/evidence/phase-9j.md`'s 2026-09-02 entry corrects the
//! sentence this paragraph used to carry: the constancy proof it cites is
//! scoped to [`super::interactive`]'s `UpstreamBackend`, which has no model
//! field of its own and takes one model for the whole candidate set at
//! `SessionRouting::bind`. A [`Destination`]'s [`Backend`] carries a model
//! resolved **per launch profile** (`main.rs::destination_backend` →
//! `session_pairing`), so a candidate set built from two enabled profiles of
//! one harness genuinely varies in `PairingClass` — a fact
//! `docs/product/evidence/phase-56.md`'s "The question the orchestrator
//! added" section establishes from current production code. [`pairing_prior`]
//! reads `classify`'s *vendor* axis for exactly that reason, beside
//! [`harness_capability_fit`], which reads its *capability* axes (protocol
//! fit, model-behaviour fit, tool semantics) and does not.
//!
//! # Purity
//!
//! Same rule as the rest of `routing`: no socket, no credential resolution,
//! no clock. `now` is an argument. Warmth, capacity and checkpoint quality
//! are values the **caller looked up** — this module names neither
//! `crate::session` nor `crate::checkpoint`, for the reason
//! [`crate::config::pairing::ContinuitySource`] gives.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use crate::config::pairing::{ContinuitySource, WarmSession};
use crate::harness::pairing::{self, ModelBehaviourFit, ProtocolFit};
use crate::harness::{Capabilities as HarnessCapabilities, Declared, WireProtocol};
use crate::integrations::IntegrationId;
use crate::provider::pricing::PriceTable;
use crate::provider::quota::{CapacityBand, RemainingCapacityScore};

use super::capability::{self, ResourceFacts};
use super::classify::{DurationClass, HardCapability, TaskClassification, WorkloadTier};
use super::evidence::{CostConfidence, FailureClass, ObservedCost};
use super::free::{FreePool, FreeResource};
use super::pressure::{
    self, Alternatives, CapacityFacts, PressureInputs, ReservePolicies, ReserveScope,
};
use super::request::{RouterAnswer, TaskClass};
use super::{
    Backend, CacheLocality, Contribution, EntitlementSource, HardConstraint,
    ProviderUnavailableCause, RoutingExplanation, TierRelation, ToolSemantics,
    apply_hard_constraints, same_capability_tier,
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

/// Phase 36 (lines 1581–1588): what the caller read about an existing
/// session's native context, beyond the warmth a [`WarmSession`] carries.
///
/// Every field is a value the **caller looked up**, on the terms this
/// module's header sets — it names neither `crate::session` nor
/// `crate::checkpoint`, so the compaction count arrives as the integer the
/// session record holds, the last task as the classification the sticky
/// classification cache recorded against this session, and the touched
/// files as the paths this session's own latest checkpoint listed. The
/// production caller is `main.rs::routing_destinations`.
///
/// `None` everywhere means **unknown**, never zero: the facet an absent
/// field feeds contributes nothing and says so in [`AffinityBreakdown`],
/// exactly as `capacity: None` is neither full nor empty. `Some(0)`
/// compactions is a counted clean history; `None` is a row nobody counted.
///
/// `task_named_paths` is a fact about the *task* rather than the session,
/// carried here because it is the other half of line 1583's intersection
/// and the router holds no task text of its own: `main.rs` runs
/// [`paths_named_in`] once and attaches the same answer to every existing
/// destination. `None` is "no task was stated"; `Some(vec![])` is "a task
/// was stated and it names no path" — different facts, both unknown to
/// the facet, and both said in its evidence.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionContextFacts {
    observed_compactions: Option<i64>,
    last_task: Option<TaskClassification>,
    touched_files: Option<Vec<String>>,
    task_named_paths: Option<Vec<String>>,
}

impl SessionContextFacts {
    /// Nothing read — the honest floor, and what every destination carries
    /// until `main.rs::routing_destinations` attaches what it looked up.
    pub const UNREAD: Self = Self {
        observed_compactions: None,
        last_task: None,
        touched_files: None,
        task_named_paths: None,
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
}

/// Map lines 1298, 1299 and 1304: the components of one decision's own
/// input-size estimate, named rather than folded into a single number — a
/// reader of `glasshouse route --task` is owed which pieces were counted,
/// not just a total. Follows [`AffinityFacet`]'s `known`/`unknown` idiom at
/// the level of a whole estimate: every component is `Some(tokens)` when it
/// was actually measured and `None` when it was not — never a zero standing
/// in for "nobody looked" or "the read came back empty" (both degrade to
/// absent, by this package's own ruling — see `main.rs`'s producers).
///
/// [`Self::total_tokens`] is `None` only when every component is `None`;
/// otherwise it sums the components that were measured, which is the same
/// "absent, not zero" rule applied to a sum instead of one field — the
/// component this build could not read simply does not enter the total,
/// rather than entering it as a zero that would understate a real cost.
///
/// The production caller is `main.rs::routing_destinations`, which attaches
/// one of these per destination it builds: a fresh destination's carries the
/// project's own memory and checkpoint (line 1304, *"fresh-session cost
/// estimates"*), an existing session's carries that session's own latest
/// checkpoint only when the session is cold rather than live (line 1299),
/// and a live session's stays [`Self::UNESTIMATED`] entirely — `WarmSession`
/// already refuses to guess at accumulated context, and this estimate does
/// not overturn that refusal.
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
    /// **The production caller is `main.rs::routing_destinations`**, which
    /// attaches every destination the shipped binary builds with
    /// `destination_tier_ceiling`'s reading of
    /// [`crate::config::EffectiveConfig::model_ceiling`] — so the gate in
    /// `hard_constraint` and the term in [`workload_tier_fit`] act on the
    /// binary's path, not only on the library's.
    ///
    /// Phase 34F widened what that reading may establish without changing
    /// this method or its caller: `model_ceiling` now reads
    /// `providers.<p>.model_ceilings` (map line 1796) *or* a matching
    /// `providers.<p>.model_capabilities` record's own ceiling, through
    /// [`crate::config::capability::CeilingResolution::hard_ceiling`]. Only a
    /// record the user assigned themselves can reach here — a
    /// benchmark-provenance record's ceiling is a prior, never a hard
    /// constraint, and never arrives as a `Some` this method sees (capability
    /// map line 1484).
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
/// `needs_tool_calls` can **reject** a destination: a task that needs tool
/// calls cannot go somewhere tool calls are established not to work.
/// Anything a router would only *prefer* belongs in a contribution, not
/// here — that is design decision 1 ("additive, never a filter") carried
/// into this phase.
///
/// `hard_capabilities` carries `TaskClassification::hard_capabilities()`'s own
/// output (`super::classify`) so [`capability_fit`] has something to compare
/// a destination's registry entry against, and so does the hard-constraint
/// gate (map line 1517): ruling 4 of the `GH-ROUTING-CAPABILITY` packet gives
/// capability mismatch exactly one rejecting exception — a hard capability
/// the resource is *established* to lack — and `session::hard_constraint`
/// raises `HardConstraint::Capability` from exactly that reading
/// (`session::is_adequate`). An unverified axis is "nobody has said," not
/// "cannot," and still only costs a candidate a `capability_fit` contribution.
///
/// `minimum_tier` also rejects (map line 1516), and
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

// ---------------------------------------------------------------------------
// The contributions. One public function each, so a mutation can zero
// exactly one of them.
// ---------------------------------------------------------------------------

/// Line 1595: what the harness's own capability fit for this destination
/// contributes.
///
/// Reads `classify`'s three **capability** axes and not its vendor class —
/// [`pairing_prior`], right below, reads that one. The axes vary with the
/// harness, which is exactly what makes this term able to separate a
/// candidate set: `crate::harness::adapter_for` returns a different adapter
/// per [`IntegrationId`], and each declares its own protocols.
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

/// Lines 566, 1540, 1923: the vendor-native pairing's own soft starting
/// prior — see this module's header for why it is reachable here and not in
/// [`super::interactive`].
///
/// **Order of the two checks matters for the explanation a reader gets.** A
/// non-native pairing is inert regardless of how much local evidence exists
/// for it — it never had a prior to decay — so that check runs first and
/// reports plainly that this is not a vendor-native pairing. Only a
/// vendor-native pairing goes on to ask whether `PAIRING_PRIOR_EVIDENCE_THRESHOLD`
/// worth of local observations have replaced its starting assumption.
///
/// Never rejects: this term is a preference among candidates a hard
/// constraint has already admitted, and map line 1950's *"never rejecting
/// solely for being cross-vendor"* is a hard-constraint rule — no candidate
/// is refused on this axis anywhere in this module. `is_vendor_native`'s own
/// doc is the source of the "not a quality claim" wording repeated in both
/// branches below, and the map's first fixed architectural requirement is the
/// same claim at the product level.
pub fn pairing_prior(destination: &Destination, inputs: &RouterInputs<'_>) -> Contribution {
    let pairing = classify_destination(destination, inputs.overrides);
    let class = pairing.class();

    if !class.is_vendor_native() {
        return Contribution::new(
            "pairing prior",
            0.0,
            format!(
                "`{}` operating `{}` is a {class} pairing — inert: not a vendor-native pairing",
                destination.harness().slug(),
                destination.backend().model().label(),
            ),
        );
    }

    let observed = destination.pairing_prior_evidence();
    if observed >= PAIRING_PRIOR_EVIDENCE_THRESHOLD {
        return Contribution::new(
            "pairing prior",
            0.0,
            format!(
                "`{}` operating `{}` is a {class} pairing, but {observed} local observations \
                 have accumulated for it — observed evidence has replaced the starting prior",
                destination.harness().slug(),
                destination.backend().model().label(),
            ),
        );
    }

    Contribution::new(
        "pairing prior",
        PAIRING_PRIOR,
        format!(
            "`{}` operating `{}` is a {class} pairing — a starting assumption for a fresh \
             session with little local evidence, not a quality claim",
            destination.harness().slug(),
            destination.backend().model().label(),
        ),
    )
}

/// Map line 1382, joined to a task's hard capability requirements —
/// `GH-ROUTING-CAPABILITY`'s package, and `capability::axis_for`'s own
/// comparison function is what makes this ruling-1-safe: this function never
/// compares a task's tier to a resource's tier, only a resource's registry
/// entry to the specific axis a requirement names.
///
/// This is one of `TaskClassification::hard_capabilities`' two production
/// consumers — the other is `is_adequate`, which `session::hard_constraint`
/// asks the same question of to raise `HardConstraint::Capability` (map line
/// 1517). `requirements.hard_capabilities` is where a caller of
/// [`SessionRouter::choose`] attaches it: `main.rs`'s `launch_session` and
/// `route_recommendation` both build it from `classified.answer.requirements()`
/// on every classified launch; a caller with no task in hand still passes
/// `TaskRequirements::default()`, an empty list that contributes `0.0` here
/// and excludes nothing at the gate.
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
/// has). `required` is the tier this decision **prefers** — the classified
/// tier, or the one a [`TierMovement`] moved it to. A destination below the
/// *classified* tier is never scored here — line 1516's gate in
/// `hard_constraint` removed it — so the fourth arm is reachable only after
/// an escalation: a destination established at the classified tier, kept
/// eligible because it can serve the work, and not the fit preferred now.
pub fn workload_tier_fit(destination: &Destination, required: WorkloadTier) -> Contribution {
    match destination.tier_ceiling() {
        Some(ceiling) if ceiling < required => Contribution::new(
            "workload tier fit",
            TIER_FIT_BELOW_MOVED,
            format!(
                "this decision escalated its preference to the `{required}` tier and `{}` is \
                 established to serve up to `{ceiling}` — still eligible for the classified \
                 tier, and not the fit preferred now (line 1559)",
                destination.id()
            ),
        ),
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

/// Map line 1538: *"include expected marginal cost in candidate scoring."*
///
/// `Cost` — [`super::Cost`]'s own doc calls it *"whether using a model costs
/// the user anything at the margin"* — is still the only reading that can
/// ever make this term `0.0`: [`Cost::is_free`] returning `true` is a
/// **known** zero, never an unknown, and stays priced exactly as it always
/// was. Phase 32G's `PriceTable` (`crate::provider::pricing`) answers the
/// other half for a metered destination — a known per-million price, or an
/// honest unknown — but it changes only the **evidence**, never the
/// magnitude: there is still no per-call token estimate at this call site
/// (`SessionContextFacts` carries none), so a known price cannot yet be
/// converted into an actual expected dollar figure without inventing one.
/// Reporting the known rate is honest; reporting a dollar estimate from it
/// is map line 1298's job, once a size producer exists. A destination whose
/// price is unknown is priced identically to one whose price is known but
/// unconvertible — both metered, neither free — and the difference between
/// them is only ever textual, the same way [`AffinityFacet`]'s `known` and
/// `unknown` constructors both start every unattached facet as `0.0`.
///
/// **Pushed unconditionally**, unlike [`cost_preference`], because line 1538
/// names no workload-tier precondition the way line 1558 does. That is also
/// why it must stay inert exactly where [`cost_preference`] is active: once a
/// tier is established, [`cost_preference`] already prices the same `Cost`
/// reading as its own deliberately small tie-break (line 1558's own doc).
/// Pricing it again here would score the identical fact in the identical
/// direction a second time — the double-count this term exists to avoid, not
/// to add — so the two conditions partition rather than overlap: exactly one
/// of them ever prices a given candidate.
fn expected_marginal_cost(
    destination: &Destination,
    movement: Option<&TierMovement>,
    prices: &PriceTable,
) -> Contribution {
    if movement.is_some() {
        return Contribution::new(
            "expected marginal cost",
            0.0,
            "a workload tier is established for this decision, so `cost preference` (line \
             1558) already prices free versus metered here — pricing it twice would double- \
             count the same reading",
        );
    }
    if destination.backend().cost().is_free() {
        return Contribution::new(
            "expected marginal cost",
            0.0,
            format!(
                "`{}` is a zero-cost resource for this work — nothing is spent by preferring it",
                destination.id()
            ),
        );
    }
    let known_price = destination
        .backend()
        .model()
        .name()
        .and_then(|model| prices.price_for(destination.backend().provider(), model));
    let (magnitude, evidence) = match known_price {
        Some(price) => match destination.estimated_input_size().total_tokens() {
            // Map line 1298: the rate and this decision's own input-size
            // estimate together become an actual dollar figure. The
            // **magnitude** still does not move — see this function's own
            // doc comment on why pricing it a second time here would
            // double-count `cost_preference` — only the evidence gains the
            // conversion this package exists to make possible.
            Some(tokens) => {
                let cost_usd = tokens as f64 * price.input_per_million_usd / 1_000_000.0;
                (
                    EXPECTED_MARGINAL_COST_PENALTY,
                    format!(
                        "`{}` is metered; its price is known — ${:.2} per million input \
                         tokens, ${:.2} per million output tokens — and this decision's own \
                         input-size estimate ({}) puts the expected marginal cost at roughly \
                         ${:.4} for this call; no workload tier is established yet to price it \
                         another way (line 1558 would once one is)",
                        destination.id(),
                        price.input_per_million_usd,
                        price.output_per_million_usd,
                        destination.estimated_input_size().describe(),
                        cost_usd,
                    ),
                )
            }
            // Unknown SIZE makes the cost unknown even when the price is
            // known — the rule this package adds. Priced identically to an
            // unpriced metered destination, never collapsed toward zero.
            None => (
                EXPECTED_MARGINAL_COST_PENALTY,
                format!(
                    "`{}` is metered; its price is known — ${:.2} per million input tokens, \
                     ${:.2} per million output tokens — but this decision's own input-size \
                     estimate could not measure any component for it, so there is nothing to \
                     convert that rate with; it is priced the same as an unpriced metered \
                     destination and no workload tier is established yet to price it another \
                     way (line 1558 would once one is)",
                    destination.id(),
                    price.input_per_million_usd,
                    price.output_per_million_usd,
                ),
            ),
        },
        // The magnitude here must stay `EXPECTED_MARGINAL_COST_PENALTY`, not
        // `0.0`: an unknown price is unknown, not free, and collapsing this
        // branch to the free branch's zero is exactly the fake-zero map line
        // 1305 forbids (see this module's own mutation coverage).
        None => (
            EXPECTED_MARGINAL_COST_PENALTY,
            format!(
                "`{}` is metered and its price is unknown — no provider price metadata names \
                 this provider/model, and no workload tier is established yet to price it \
                 another way (line 1558 would once one is); an unknown price is priced as \
                 metered, never as free",
                destination.id()
            ),
        ),
    };
    Contribution::new("expected marginal cost", magnitude, evidence)
}

/// Map line 1307: the marginal input cost this decision actually used, as a
/// monetary reading with its required confidence — never recomputed once
/// carried. [`SessionRouter::choose`] calls this exactly once, for the
/// destination it settled on, and the result travels on [`Routed`] to
/// whatever records it (`main.rs::record_entitlement_fallback`), rather than
/// being derived a second time at the writer from a `PriceTable` that may
/// have changed on disk since the decision was made.
///
/// Free is a known zero, regardless of size — nothing is spent whatever the
/// input turns out to be, the same certainty [`expected_marginal_cost`]'s
/// free branch reads. A metered destination needs **both** a known price
/// and a known size; either half missing answers `None` — never a
/// fabricated zero, matching map line 1307's own rule that unknown size or
/// unknown price means no cost row at all.
///
/// [`CostConfidence::Estimated`], always: every cost this function can
/// produce is built from the user's own `pricing.toml` and Glasshouse's own
/// token measurement, never a figure a provider reported — migration 11's
/// `CHECK` requires a label to be chosen, and this is the one that says so.
fn estimated_cost(destination: &Destination, prices: &PriceTable) -> Option<ObservedCost> {
    if destination.backend().cost().is_free() {
        return Some(ObservedCost {
            micro_usd: 0,
            confidence: CostConfidence::Estimated,
        });
    }
    let price = destination
        .backend()
        .model()
        .name()
        .and_then(|model| prices.price_for(destination.backend().provider(), model))?;
    let tokens = destination.estimated_input_size().total_tokens()?;
    let micro_usd = (tokens as f64 * price.input_per_million_usd).round() as i64;
    Some(ObservedCost {
        micro_usd,
        confidence: CostConfidence::Estimated,
    })
}

/// The classification a decision acted on, as a zero-weight line in every
/// candidate's explanation — so a reader of `glasshouse route --task` or of
/// a launch sees who classified the work, what it was classed as, and
/// whether line 1459's conservative rules changed the answer, beside the
/// terms that answer then drove.
fn classification_note(answer: &RouterAnswer) -> Contribution {
    Contribution::new("task classification", 0.0, answer.explain())
}

/// One named term of the affinity score — Phase 36's unit of inspection.
///
/// `known` separates *"this facet read its signal and it is worth this"*
/// from *"this facet's signal did not arrive"*: both can be `0.0`, and a
/// reader of line 1588's explanation is owed the difference, because an
/// unread signal is a producer to go and look for and a read zero is not.
#[derive(Debug, Clone, PartialEq)]
pub struct AffinityFacet {
    name: &'static str,
    line: u16,
    magnitude: f64,
    known: bool,
    evidence: String,
}

impl AffinityFacet {
    fn known(name: &'static str, line: u16, magnitude: f64, evidence: String) -> Self {
        Self {
            name,
            line,
            magnitude,
            known: true,
            evidence,
        }
    }

    fn unknown(name: &'static str, line: u16, evidence: String) -> Self {
        Self {
            name,
            line,
            magnitude: 0.0,
            known: false,
            evidence,
        }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    /// The capability-map line this facet answers.
    pub fn line(&self) -> u16 {
        self.line
    }

    pub fn magnitude(&self) -> f64 {
        self.magnitude
    }

    /// `false` when the signal this facet reads did not arrive, in which case
    /// `magnitude` is `0.0` and `evidence` says what is missing.
    pub fn is_known(&self) -> bool {
        self.known
    }

    pub fn evidence(&self) -> &str {
        &self.evidence
    }
}

/// Line 1581: the session-affinity score of one existing session, as its
/// facets — **the struct is the score** ([`Self::total`]) **and its
/// `Display` is the explanation** (line 1588).
///
/// Seven named terms, one per map line, each with its own evidence
/// sentence, summed into the one `session affinity` contribution
/// [`session_affinity`] has always pushed. Nothing here is a filter: every
/// facet is additive, an unknown facet is `0.0` and says so, and the
/// bounded magnitudes above keep warmth — the only measured signal — the
/// largest single term.
///
/// A fresh destination has no breakdown: it has no context to be affine to,
/// and [`session_affinity`] prices it at `FRESH_SESSION_AFFINITY` with the
/// sentence it always used.
#[derive(Debug, Clone, PartialEq)]
pub struct AffinityBreakdown {
    /// Lines 569 and 1596, the term as it was: live or resumable, and how
    /// long idle, through `crate::config::pairing`'s one definition.
    pub warmth: AffinityFacet,
    /// Line 1582.
    pub same_task: AffinityFacet,
    /// Line 1583.
    pub touched_files: AffinityFacet,
    /// Line 1584.
    pub native_context: AffinityFacet,
    /// Line 1585.
    pub prompt_cache: AffinityFacet,
    /// Line 1586.
    pub noise: AffinityFacet,
    /// Line 1587.
    pub quota_pressure: AffinityFacet,
}

impl AffinityBreakdown {
    /// Every facet, in the order a reader compares them.
    pub fn facets(&self) -> [&AffinityFacet; 7] {
        [
            &self.warmth,
            &self.same_task,
            &self.touched_files,
            &self.native_context,
            &self.prompt_cache,
            &self.noise,
            &self.quota_pressure,
        ]
    }

    /// The facet answering `line`, if any.
    pub fn for_line(&self, line: u16) -> Option<&AffinityFacet> {
        self.facets().into_iter().find(|facet| facet.line() == line)
    }

    /// The score — the sum of the facets, and the magnitude of the
    /// `session affinity` contribution.
    pub fn total(&self) -> f64 {
        self.facets().iter().map(|facet| facet.magnitude()).sum()
    }

    /// How many facets read no signal.
    pub fn unknown_count(&self) -> usize {
        self.facets()
            .iter()
            .filter(|facet| !facet.is_known())
            .count()
    }
}

impl std::fmt::Display for AffinityBreakdown {
    /// Line 1588: one summary line, then one line per facet — signed
    /// magnitude, name, and its evidence — so the explanation a person
    /// reads carries every term the score was built from.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let unknown = self.unknown_count();
        write!(
            f,
            "the sum of {} facets, {} of which read no signal and weigh nothing:",
            self.facets().len(),
            unknown
        )?;
        for facet in self.facets() {
            write!(
                f,
                "\n    {:+.3}  {} (line {}{}) — {}",
                facet.magnitude(),
                facet.name(),
                facet.line(),
                if facet.is_known() { "" } else { ", unknown" },
                facet.evidence()
            )?;
        }
        Ok(())
    }
}

/// Lines 1581–1588: what an existing session's affinity contributes, and
/// every facet behind it.
///
/// One contribution, as before, so that the ranking, the overview and every
/// existing assertion on the `session affinity` term keep reading one
/// number; the number is now [`AffinityBreakdown::total`] and the evidence
/// is its `Display`. [`affinity_breakdown`] is the same computation with the
/// facets kept apart, for a caller or a test that wants one of them.
///
/// `current` is where the work is now — `None` at a session start — and is
/// read by exactly one facet, line 1585's, for the cache locality of the
/// move. `requirements` carries the classification of the work in hand,
/// read by lines 1582 and 1586; a launch that stated no task leaves both
/// facets unknown rather than inventing a task to compare against.
pub fn session_affinity(
    destination: &Destination,
    current: Option<&Destination>,
    requirements: &TaskRequirements,
) -> Contribution {
    match affinity_breakdown(destination, current, requirements) {
        Some(breakdown) => {
            Contribution::new("session affinity", breakdown.total(), breakdown.to_string())
        }
        None => Contribution::new(
            "session affinity",
            FRESH_SESSION_AFFINITY,
            "a fresh session has no accumulated context to be affine to — not a penalty, only \
             the absence of the term (the bootstrap cost is where starting from nothing is \
             priced)",
        ),
    }
}

/// [`session_affinity`] with the facets kept apart. `None` for a fresh
/// destination, which has no context and therefore no breakdown.
pub fn affinity_breakdown(
    destination: &Destination,
    current: Option<&Destination>,
    requirements: &TaskRequirements,
) -> Option<AffinityBreakdown> {
    let Continuation::Existing(warm) = destination.continuation() else {
        return None;
    };
    let id = destination.id();
    let facts = destination.session_context();
    let current_task = requirements
        .classification
        .as_ref()
        .map(RouterAnswer::classification);

    // Lines 569 and 1596 — warmth, exactly as the term has always computed
    // it: the decay window and the live/resumable ratio have one definition
    // and it is not here.
    let reused = crate::config::pairing::session_continuity_contribution(
        &evidence_key_for(destination),
        &OneWarmSession(warm),
    );
    let warmth = AffinityFacet::known(
        "warmth",
        1596,
        reused.magnitude(),
        format!(
            "`{id}` is a {} session, idle {}s — {}",
            warm.state,
            warm.idle_seconds.max(0),
            reused.evidence()
        ),
    );
    let stale = warmth.magnitude() <= 0.0;

    // Line 1582 — the same task, as far as the sticky classification cache
    // can say it.
    let same_task_verdict = match (facts.last_task(), current_task) {
        (Some(previous), Some(now)) => Some(same_work(previous, now)),
        _ => None,
    };
    let same_task = match (facts.last_task(), current_task) {
        (Some(previous), Some(now)) if same_work(previous, now) => AffinityFacet::known(
            "same task",
            1582,
            SAME_TASK_AFFINITY,
            format!(
                "the last task classified onto `{id}` was classed the way this one is — tier \
                 `{}`, {} — which is the nearest thing to task identity this build records; \
                 the sticky classification cache keeps a classification, never the task text",
                now.workload_tier(),
                describe_capabilities(now),
            ),
        ),
        (Some(previous), Some(now)) => AffinityFacet::known(
            "same task",
            1582,
            0.0,
            format!(
                "the last task classified onto `{id}` was classed differently from this one \
                 (tier `{}` then, `{}` now) — the noise facet prices that",
                previous.workload_tier(),
                now.workload_tier(),
            ),
        ),
        (Some(_), None) => AffinityFacet::unknown(
            "same task",
            1582,
            format!(
                "a last classified task is recorded against `{id}` and this launch stated no \
                 task — nothing to compare it with"
            ),
        ),
        (None, Some(_)) => AffinityFacet::unknown(
            "same task",
            1582,
            format!(
                "no classified task is recorded against `{id}` — the sticky classification \
                 cache names another session, or was never written"
            ),
        ),
        (None, None) => AffinityFacet::unknown(
            "same task",
            1582,
            format!("no task was stated and none is recorded against `{id}`"),
        ),
    };

    // Line 1583 — the files this session touched, against the paths the
    // task names.
    let hits: Option<Vec<&str>> = match (facts.task_named_paths(), facts.touched_files()) {
        (Some(named), Some(touched)) if !named.is_empty() && !touched.is_empty() => Some(
            named
                .iter()
                .filter(|name| touched.iter().any(|path| path_names(path, name)))
                .map(String::as_str)
                .collect(),
        ),
        _ => None,
    };
    let touched_files = match (facts.task_named_paths(), facts.touched_files(), &hits) {
        (Some(named), Some(_), Some(hits)) if hits.is_empty() => AffinityFacet::known(
            "touched files",
            1583,
            0.0,
            format!(
                "the task names {} path{} and `{id}`'s latest checkpoint lists none of them — \
                 the noise facet prices that",
                named.len(),
                if named.len() == 1 { "" } else { "s" },
            ),
        ),
        (Some(named), Some(_), Some(hits)) => AffinityFacet::known(
            "touched files",
            1583,
            TOUCHED_FILES_AFFINITY * hits.len() as f64 / named.len() as f64,
            format!(
                "`{id}`'s latest checkpoint lists {} of the {} path{} the task names ({})",
                hits.len(),
                named.len(),
                if named.len() == 1 { "" } else { "s" },
                hits.join(", "),
            ),
        ),
        (None, _, _) => AffinityFacet::unknown(
            "touched files",
            1583,
            "no task was stated, so there is nothing to intersect the session's files with"
                .to_owned(),
        ),
        (Some([]), _, _) => AffinityFacet::unknown(
            "touched files",
            1583,
            "the task text names no path, so there is nothing to intersect the session's \
             files with"
                .to_owned(),
        ),
        (Some(_), None, _) => AffinityFacet::unknown(
            "touched files",
            1583,
            format!("no checkpoint records which files `{id}` touched"),
        ),
        (Some(_), Some(_), None) => AffinityFacet::unknown(
            "touched files",
            1583,
            format!("`{id}`'s latest checkpoint lists no files"),
        ),
    };

    // Line 1584 — the native context, as compactions and staleness say it.
    let native_context = match facts.observed_compactions() {
        None => AffinityFacet::unknown(
            "native context",
            1584,
            format!(
                "nobody counted `{id}`'s compactions — a row from before the count existed — \
                 and an uncounted history is not a clean one"
            ),
        ),
        Some(_) if stale => AffinityFacet::known(
            "native context",
            1584,
            0.0,
            format!(
                "`{id}` is past the window a warm session stays relevant for, so whatever its \
                 context holds is not credited as still useful"
            ),
        ),
        Some(0) => AffinityFacet::known(
            "native context",
            1584,
            NATIVE_CONTEXT_INTACT,
            format!(
                "no compaction has been observed on `{id}` and it is inside the relevance \
                 window — its native context holds exactly what was said to it"
            ),
        ),
        Some(count) if count < NOISY_COMPACTION_COUNT => AffinityFacet::known(
            "native context",
            1584,
            NATIVE_CONTEXT_INTACT / 2.0,
            format!(
                "`{id}` has been compacted {count} time{} — a summary stands in for part of \
                 its context, so it is credited at half",
                if count == 1 { "" } else { "s" },
            ),
        ),
        Some(count) => AffinityFacet::known(
            "native context",
            1584,
            0.0,
            format!(
                "`{id}` has been compacted {count} times — what survives is mostly summaries \
                 of summaries, credited as neither intact nor useful (the noise facet prices \
                 the count)"
            ),
        ),
    };

    // Line 1585 — is the provider-side prefix likely still there.
    let locality =
        current.map(|current| CacheLocality::between(current.backend(), destination.backend()));
    let prompt_cache = match locality {
        Some(locality @ CacheLocality::Lost(_)) => AffinityFacet::known(
            "prompt cache",
            1585,
            0.0,
            format!("the work is moving off the backend that built `{id}`'s prefix: {locality}"),
        ),
        Some(locality @ CacheLocality::LikelyLost(_)) => AffinityFacet::known(
            "prompt cache",
            1585,
            0.0,
            format!("moving to `{id}` changes the credential: {locality}"),
        ),
        _ if warm.idle_seconds < 0 => AffinityFacet::unknown(
            "prompt cache",
            1585,
            format!(
                "`{id}`'s last activity is in the future — a clock moved backwards — and a \
                 cache lifetime cannot be measured against that"
            ),
        ),
        _ if warm.idle_seconds <= PROMPT_CACHE_TTL_SECONDS => AffinityFacet::known(
            "prompt cache",
            1585,
            PROMPT_CACHE_HOT,
            format!(
                "`{id}` was active {}s ago, inside the {PROMPT_CACHE_TTL_SECONDS}s a \
                 provider-side cached prefix is published to survive by default — likely \
                 hot, not observed: no provider reports a hit",
                warm.idle_seconds
            ),
        ),
        _ => AffinityFacet::known(
            "prompt cache",
            1585,
            0.0,
            format!(
                "`{id}` was active {}s ago, past the {PROMPT_CACHE_TTL_SECONDS}s default \
                 lifetime of a provider-side cached prefix — likely expired",
                warm.idle_seconds
            ),
        ),
    };

    // Line 1586 — the same signals, read for noise and unrelatedness.
    let mut noise_magnitude = 0.0;
    let mut noise_notes: Vec<String> = Vec::new();
    let mut noise_readable = false;
    if let Some(count) = facts.observed_compactions() {
        noise_readable = true;
        if count >= NOISY_COMPACTION_COUNT {
            noise_magnitude +=
                (count as f64 * COMPACTION_NOISE_PENALTY).max(COMPACTION_NOISE_FLOOR);
            noise_notes.push(format!(
                "compacted {count} times, and each compaction replaces context with a summary \
                 of it"
            ));
        }
    }
    if let Some(verdict) = same_task_verdict {
        noise_readable = true;
        if !verdict {
            noise_magnitude += UNRELATED_TASK_PENALTY;
            noise_notes.push(
                "the last task classified onto it was classed differently from this one".to_owned(),
            );
        }
    }
    if let (Some(named), Some(hits)) = (facts.task_named_paths(), &hits) {
        noise_readable = true;
        // A bare `foo.rs` in prose is a weaker claim than `src/foo.rs`; the
        // penalty needs the stronger spelling so a word that merely looks
        // like a file name cannot cost every session a third of a point.
        let names_a_directory_path = named.iter().any(|name| name.contains('/'));
        if hits.is_empty() && names_a_directory_path {
            noise_magnitude += UNRELATED_FILES_PENALTY;
            noise_notes.push(
                "the task names paths and its latest checkpoint lists none of them".to_owned(),
            );
        }
    }
    let noise = if !noise_readable {
        AffinityFacet::unknown(
            "noise",
            1586,
            format!(
                "no compaction count, no classified task to compare and no checkpoint file \
                 list — nothing to read `{id}`'s noise from"
            ),
        )
    } else if noise_notes.is_empty() {
        AffinityFacet::known(
            "noise",
            1586,
            0.0,
            format!(
                "nothing read says `{id}`'s context is noisy or unrelated — the absence of a \
                 signal, not a clean bill"
            ),
        )
    } else {
        AffinityFacet::known(
            "noise",
            1586,
            noise_magnitude,
            format!("`{id}`: {}", noise_notes.join("; ")),
        )
    };

    // Line 1587 — significant pressure on the resource this session spends,
    // from the band the caller derived from the same reading `quota_pressure`
    // prices.
    let credential = destination.backend().credential().label();
    let quota_pressure = match destination.capacity_facts().band() {
        Some(band) if band <= CapacityBand::Reserve => AffinityFacet::known(
            "quota pressure",
            1587,
            QUOTA_PRESSURE_AFFINITY_PENALTY,
            format!(
                "`{credential}` is in the `{}` band — significant pressure on the \
                 resource this session spends; the reading itself is priced once, by the \
                 `known quota pressure` term, and this is the map's own decrease in affinity",
                band.as_str()
            ),
        ),
        Some(band) => AffinityFacet::known(
            "quota pressure",
            1587,
            0.0,
            format!(
                "`{credential}` is in the `{}` band — not significant pressure",
                band.as_str()
            ),
        ),
        None => AffinityFacet::unknown(
            "quota pressure",
            1587,
            format!(
                "nothing has been read about `{credential}`'s remaining quota — an unread \
                 resource is neither preferred nor withheld"
            ),
        ),
    };

    Some(AffinityBreakdown {
        warmth,
        same_task,
        touched_files,
        native_context,
        prompt_cache,
        noise,
        quota_pressure,
    })
}

/// Line 1582's "same task or feature", as far as a stored classification can
/// say it: the same hard capabilities, the same workload tier, and the same
/// answer to whether the work touches the repository and modifies code.
/// Confidence and source are deliberately not compared — the same task
/// classed by heuristics one launch and by a model the next is one task.
fn same_work(previous: &TaskClassification, current: &TaskClassification) -> bool {
    previous.hard_capabilities() == current.hard_capabilities()
        && previous.workload_tier() == current.workload_tier()
        && previous.needs_repo_context() == current.needs_repo_context()
        && previous.needs_code_modification() == current.needs_code_modification()
}

fn describe_capabilities(classification: &TaskClassification) -> String {
    let capabilities = classification.hard_capabilities();
    if capabilities.is_empty() {
        "no hard capability".to_owned()
    } else {
        capabilities
            .iter()
            .map(|capability| capability.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Whether a checkpoint's `path` is the file the task's `name` names: the
/// same repo-relative path, or a bare file name the path ends in. A name is
/// never matched as a substring — `foo.rs` is not `barfoo.rs`.
fn path_names(path: &str, name: &str) -> bool {
    let name = name.trim_start_matches("./");
    path == name || path.ends_with(&format!("/{name}"))
}

/// Line 1583's "relevant": the path-shaped tokens in a task's text.
///
/// A spelling test and not a vocabulary — a token names a path when it
/// contains a `/` (and is not a URL), or ends in a dotted extension of one to
/// five lowercase ASCII alphanumerics with at least one letter, after a stem
/// of at least two characters. The stem rule is what keeps `e.g.` and `i.e.`
/// out; the lowercase rule keeps `Ph.D.` out; the letter rule keeps `v1.2`
/// out. `Node.js` gets in, which is the price of a spelling test, and the
/// reason [`affinity_breakdown`]'s unrelated-files penalty needs a `/`.
///
/// Surrounding punctuation and backticks are stripped, so `` `src/foo.rs`, ``
/// names `src/foo.rs`. Order is first mention, without repeats.
pub fn paths_named_in(task_text: &str) -> Vec<String> {
    let mut named: Vec<String> = Vec::new();
    for raw in task_text.split_whitespace() {
        let token = raw
            .trim_matches(|c: char| !(c.is_alphanumeric() || matches!(c, '/' | '.' | '_' | '-')));
        if token.is_empty() || token.contains("://") {
            continue;
        }
        let has_separator = token.contains('/') && token.trim_matches('/').len() > 1;
        if (has_separator || has_file_extension(token)) && !named.iter().any(|n| n == token) {
            named.push(token.to_owned());
        }
    }
    named
}

fn has_file_extension(token: &str) -> bool {
    let Some((stem, extension)) = token.rsplit_once('.') else {
        return false;
    };
    stem.chars().count() >= 2
        && !stem.ends_with('/')
        && (1..=5).contains(&extension.len())
        && extension
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && extension.chars().any(|c| c.is_ascii_lowercase())
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
pub fn quota_pressure(destination: &Destination, weights: &ScoreWeights) -> Contribution {
    match destination.capacity() {
        Some(score) => Contribution::new(
            "known quota pressure",
            score.routing_fraction() * weights.quota_pressure_weight,
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
pub fn provider_health(
    destination: &Destination,
    pool: &FreePool,
    now: Instant,
    weights: &ScoreWeights,
) -> Contribution {
    let resource = FreeResource::new(
        destination.backend().credential().clone(),
        destination.backend().model().label(),
    );
    let health = pool.health(&resource);

    if health.credential_was_rejected() {
        return Contribution::new(
            "provider health",
            weights.health_unavailable_penalty,
            format!(
                "`{}` was refused by its provider — waiting does not fix a revoked key",
                destination.backend().credential().label()
            ),
        );
    }
    if !health.is_available(now) {
        return Contribution::new(
            "provider health",
            weights.health_unavailable_penalty,
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
        (f64::from(failures) * weights.health_failure_penalty).max(weights.health_penalty_floor),
        format!(
            "{failures} consecutive observed failures on `{}` that have not yet earned a \
             cooldown",
            destination.backend().credential().label()
        ),
    )
}

/// Line 1546: current cadence availability, scored separately from
/// [`provider_health`]'s general route health.
///
/// `provider_health` folds together a provider-declared wait and a cooldown
/// Glasshouse invented after repeated ordinary failures (Phase 9I line 534
/// deliberately keeps the invented kind probeable by real work); this term
/// reads only [`super::free::ResourceHealth::declared_wait_remaining`], which
/// is `None` for every case except a wait the destination's own provider is
/// currently inside. An invented cooldown, or a resource nothing has ever
/// been observed about, scores exactly the same here: inert.
pub fn cadence_availability(
    destination: &Destination,
    pool: &FreePool,
    now: Instant,
) -> Contribution {
    let resource = FreeResource::new(
        destination.backend().credential().clone(),
        destination.backend().model().label(),
    );
    let health = pool.health(&resource);

    match health.declared_wait_remaining(now) {
        Some(remaining) => Contribution::new(
            "cadence availability",
            CADENCE_DECLARED_WAIT_PENALTY,
            format!(
                "`{}` is inside a {}s wait its own provider declared",
                destination.backend().credential().label(),
                remaining.as_secs()
            ),
        ),
        None => Contribution::new(
            "cadence availability",
            0.0,
            format!(
                "no provider-declared wait is in effect for `{}` — not a cadence claim, the \
                 absence of one",
                destination.backend().credential().label()
            ),
        ),
    }
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
// Phase 56A step 3, lines 1953 and 1966–1969 — the entitlement pool's own
// terms: the pool enters the candidate set (main.rs widens it, one candidate
// per entitlement allowed to serve the harness), and the score chooses.
//
// Five factors, map line 1966's own list: available capacity (band), time
// until reset, recent throttling, session affinity, and model availability.
// The affinity factor is deliberately NOT a new term — it **is**
// [`session_affinity`], because the entitlement holding a warm session's
// context scores exactly what that session's warmth already says, and a
// second number for the same fact would be the double-count this module
// refuses everywhere. Stickiness (line 1968) is therefore the affinity
// term's weight, not a second mechanism, and
// [`entitlement_stickiness_note`] says so in the explanation.
//
// Two rules every term below obeys:
//
// - **an unknown facet contributes NOTHING and says so** — never a guessed
//   number, the same stance `quota_pressure` takes for an unread quota;
// - **the terms are live only when the candidate set actually offers a
//   choice of configured entitlements** ([`EntitlementPoolView`], two or
//   more distinct configured names). A user with zero or one configured
//   entitlement has no pool for a score to choose across, and their ranking
//   must stay byte-for-byte what it was — the packet's own preservation
//   clause, enforced structurally rather than hoped for.
// ---------------------------------------------------------------------------

/// Line 1966's capacity factor, by band. Plenty earns the most, and the
/// magnitudes are graded so that one band's step (0.15) is a *slight* lead —
/// the lead the distribution rule (line 1968) must be able to overcome —
/// while the drop into the reserve band is priced like the protected thing
/// it is.
const ENTITLEMENT_CAPACITY_PLENTY: f64 = 0.3;
const ENTITLEMENT_CAPACITY_HEALTHY: f64 = 0.15;
const ENTITLEMENT_CAPACITY_TIGHT: f64 = -0.15;
const ENTITLEMENT_CAPACITY_RESERVE: f64 = -0.5;

/// An entitlement whose band reads **exhausted** — line 1968's one capacity
/// case that must move even a warm session. Sized against this module's own
/// scale: it must clear a live zero-idle session's warmth (`1.5`), the hot
/// prompt-cache and intact-context facets a short-idle session can carry
/// (`0.4 + 0.3`), and the cold bootstrap the fresh sibling pays (`1.0`) —
/// `3.2` in all — because an account with nothing left cannot serve the next
/// turn, and staying warm on a resource that cannot answer is not warmth. It
/// deliberately does **not** also clear the task-identity facets (same task
/// `0.5` + touched files `0.6`): a session demonstrably mid-task on the
/// named files is not yanked by a band reading alone.
const ENTITLEMENT_EXHAUSTED_PENALTY: f64 = -3.5;

/// Line 1968's distribution half: what each **live** session already charged
/// to the same entitlement (elsewhere in this candidate set) costs a
/// candidate. One step must outweigh a *slight* capacity lead — one band
/// (`0.15`) — so two simultaneous fresh choices do not both land on the
/// entitlement that was marginally ahead, and must not outweigh a full
/// plenty-to-tight gap, so real capacity still decides when the pool is
/// genuinely uneven.
const ENTITLEMENT_IN_FLIGHT_LOAD: f64 = -0.2;

/// Line 1967, the burn half: what a low-band entitlement whose reset is
/// inside [`RESET_BURN_HORIZON_SECONDS`] earns. The user's instruction of
/// record (design-decisions §56A): *"A at 12% resetting in 1h20m and B at
/// 61% resetting in 4d ⇒ burn A"* — capacity that would expire unused is the
/// cheapest capacity there is, so the full weight of the strongest
/// established preference on this module's scale (`1.0`), enough to overturn
/// the reserve-band's own capacity penalty (`-0.5`) plus a healthy sibling's
/// lead (`+0.15`).
const RESET_BURN: f64 = 1.0;

/// Line 1967, the preserve half: what a low-band entitlement whose reset is
/// at or past [`RESET_PRESERVE_HORIZON_SECONDS`] pays — *"A at 12% resetting
/// in 4d ⇒ preserve A, route B"*. Smaller than the burn half on purpose: the
/// capacity factor already penalises the low band once, and this term adds
/// the map's own extra reason to look elsewhere, not a second copy of the
/// band.
const RESET_PRESERVE: f64 = -0.4;

/// Line 1967's "near". Two hours — comfortably containing the user's own
/// 1h20m example, and, on the scale of the multi-hour subscription windows
/// the 56A instruction describes, a remainder with under two hours to live
/// cannot plausibly be banked for later work. Deliberately **not**
/// `pressure::RESET_RELIEF_HORIZON_SECONDS` (300s): that figure prices an
/// API window's imminence, and reusing it here would make the user's own
/// example read as "distant".
const RESET_BURN_HORIZON_SECONDS: i64 = 2 * 3600;

/// Line 1967's "far": a day or more away is fully "preserve" — the user's
/// four-day example with a wide margin — and between the two horizons the
/// term fades linearly, so a reset crossing either boundary does not jump.
const RESET_PRESERVE_HORIZON_SECONDS: i64 = 24 * 3600;

/// Line 1966's throttling factor: per informative throttle recorded against
/// this entitlement in the evidence window, bounded. The same shape and
/// scale as [`HEALTH_FAILURE_PENALTY`], because both read observed
/// misbehaviour of the serving resource; account-scoped where 56A-2's
/// narrowing could honestly narrow it, and the evidence sentence says whose
/// count it is either way.
const ENTITLEMENT_THROTTLE_PENALTY: f64 = -0.2;
const ENTITLEMENT_THROTTLE_FLOOR: f64 = -0.6;

/// Line 1966's model-availability factor: the entitlement's declared
/// catalogue names the model this destination would serve — an established
/// fact, on the same scale as [`TIER_FIT_HEADROOM`]. The negative case is
/// not priced here at all: a declared list that does *not* name the model is
/// [`super::Entitlement::model_constraint`]'s hard refusal, and such a
/// candidate never reaches the score.
const ENTITLEMENT_MODEL_AVAILABLE: f64 = 0.2;

/// What the candidate set offers along the entitlement axis — the two
/// set-level facts the pool terms read, computed once per `choose` from the
/// eligible candidates because only the router holds the set (the same
/// reason the private `alternatives_for` helper lives here).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EntitlementPoolView {
    /// Distinct **configured** entitlement names among the candidates — the
    /// pool gate. The synthesised harness-default entries do not count
    /// toward the gate (a user who configured nothing has no pool), but a
    /// candidate carrying one is still scored once the gate is open: the
    /// map's own pool names native sign-ins as members.
    configured: BTreeSet<String>,
    /// Live sessions per entitlement name, by destination id — line 1968's
    /// in-flight load, so a candidate can ask "who else is drawing on my
    /// account right now" without counting itself.
    live: BTreeMap<String, Vec<String>>,
}

impl EntitlementPoolView {
    /// Read the axis off a candidate set.
    pub fn of(destinations: &[Destination]) -> Self {
        let mut view = Self::default();
        for destination in destinations {
            let Some(entitlement) = destination.entitlement() else {
                continue;
            };
            if entitlement.is_configured() {
                view.configured.insert(entitlement.name().to_owned());
            }
            if let Continuation::Existing(warm) = destination.continuation()
                && warm.state == crate::config::pairing::WarmSessionState::Live
            {
                view.live
                    .entry(entitlement.name().to_owned())
                    .or_default()
                    .push(destination.id().to_owned());
            }
        }
        view
    }

    /// Whether the set carries two or more distinct configured entitlements
    /// — the gate every pool term checks first.
    pub fn offers_a_choice(&self) -> bool {
        self.configured.len() >= 2
    }

    pub fn configured_count(&self) -> usize {
        self.configured.len()
    }

    /// How many **live** sessions other than `excluding_id` are charged to
    /// `entitlement` in this candidate set.
    pub fn live_sessions_elsewhere(&self, entitlement: &str, excluding_id: &str) -> usize {
        self.live
            .get(entitlement)
            .map(|ids| ids.iter().filter(|id| *id != excluding_id).count())
            .unwrap_or(0)
    }

    /// The inert sentence every pool term renders when the gate is closed —
    /// one wording, so the explanation cannot say two different things about
    /// one fact.
    fn no_choice_evidence(&self) -> String {
        format!(
            "inert: the candidate set carries {} configured entitlement{} — the pool axis \
             separates nothing until there are two (lines 1953, 1966)",
            self.configured.len(),
            if self.configured.len() == 1 { "" } else { "s" },
        )
    }
}

/// The two gates every pool term shares: the set must offer a choice, and
/// the candidate must carry an entitlement at all. `Err` is the finished
/// inert contribution.
fn entitlement_axis<'a>(
    term: &'static str,
    destination: &'a Destination,
    pool: &EntitlementPoolView,
) -> Result<&'a super::Entitlement, Contribution> {
    if !pool.offers_a_choice() {
        return Err(Contribution::new(term, 0.0, pool.no_choice_evidence()));
    }
    match destination.entitlement() {
        Some(entitlement) => Ok(entitlement),
        None => Err(Contribution::new(
            term,
            0.0,
            "no entitlement describes this destination's resource — nothing to score on the \
             pool axis, and nothing is guessed",
        )),
    }
}

/// Line 1966's capacity factor, with line 1968's distribution built in: the
/// entitlement's own band, minus a load step per live session elsewhere in
/// the set already charged to the same account. The load half is what
/// spreads independent fresh choices across the pool — the second of two
/// simultaneous choices sees the first's live session and the marginally
/// leading account stops leading.
pub fn entitlement_capacity(destination: &Destination, pool: &EntitlementPoolView) -> Contribution {
    const TERM: &str = "entitlement capacity";
    let entitlement = match entitlement_axis(TERM, destination, pool) {
        Ok(entitlement) => entitlement,
        Err(inert) => return inert,
    };
    let Some(band) = entitlement.capacity_band() else {
        return Contribution::new(
            TERM,
            0.0,
            format!(
                "`{}`'s remaining capacity is unknown — nothing measured it, so it contributes \
                 nothing, never a guessed number",
                entitlement.name()
            ),
        );
    };
    if band == CapacityBand::Exhausted {
        return Contribution::new(
            TERM,
            ENTITLEMENT_EXHAUSTED_PENALTY,
            format!(
                "`{}` reads exhausted — an account with nothing left cannot serve the next \
                 turn, and this is the one band reading that outweighs a warm session's \
                 context (line 1968)",
                entitlement.name()
            ),
        );
    }
    let band_weight = match band {
        CapacityBand::Plenty => ENTITLEMENT_CAPACITY_PLENTY,
        CapacityBand::Healthy => ENTITLEMENT_CAPACITY_HEALTHY,
        CapacityBand::Tight => ENTITLEMENT_CAPACITY_TIGHT,
        CapacityBand::Reserve => ENTITLEMENT_CAPACITY_RESERVE,
        CapacityBand::Exhausted => unreachable!("returned above"),
    };
    let live_elsewhere = pool.live_sessions_elsewhere(entitlement.name(), destination.id());
    let magnitude = band_weight + live_elsewhere as f64 * ENTITLEMENT_IN_FLIGHT_LOAD;
    let mut evidence = format!(
        "`{}` is in the {band} band — the pool's capacity axis, read from 56A-2's telemetry",
        entitlement.name()
    );
    if live_elsewhere > 0 {
        use std::fmt::Write as _;
        let _ = write!(
            evidence,
            "; {live_elsewhere} live session{} in this set already draw{} on it, so independent \
             work spreads to a sibling account rather than piling on (line 1968)",
            if live_elsewhere == 1 { "" } else { "s" },
            if live_elsewhere == 1 { "s" } else { "" },
        );
    }
    Contribution::new(TERM, magnitude, evidence)
}

/// Line 1967 — the reset-boundary rule, as its own named term. Reads ONLY
/// the entitlement's reset facet and its capacity band: a **low** band
/// (tight or reserve) with a **near** reset is burned (its remainder would
/// otherwise expire), the same band with a **far** reset is preserved, and
/// the two fade into each other linearly between the horizons. A healthy
/// band has no scarce remainder to burn or preserve; an exhausted one has
/// nothing left to burn; an unknown reset contributes nothing.
pub fn entitlement_reset_boundary(
    destination: &Destination,
    pool: &EntitlementPoolView,
) -> Contribution {
    const TERM: &str = "reset boundary";
    let entitlement = match entitlement_axis(TERM, destination, pool) {
        Ok(entitlement) => entitlement,
        Err(inert) => return inert,
    };
    let Some(band) = entitlement.capacity_band() else {
        return Contribution::new(
            TERM,
            0.0,
            format!(
                "`{}`'s capacity band is unknown — this term reads the reset beside the band, \
                 and without the band nobody can say whether a remainder would expire",
                entitlement.name()
            ),
        );
    };
    let Some(seconds) = entitlement.seconds_until_reset() else {
        return Contribution::new(
            TERM,
            0.0,
            format!(
                "`{}`'s reset is unknown — an unknown reset contributes nothing (line 1967)",
                entitlement.name()
            ),
        );
    };
    if band == CapacityBand::Exhausted {
        return Contribution::new(
            TERM,
            0.0,
            format!(
                "`{}` reads exhausted — there is no remainder left to burn, and the capacity \
                 term already prices the band",
                entitlement.name()
            ),
        );
    }
    if band > CapacityBand::Tight {
        return Contribution::new(
            TERM,
            0.0,
            format!(
                "`{}` is in the {band} band — no scarce remainder to burn before a reset or to \
                 preserve past one",
                entitlement.name()
            ),
        );
    }
    let urgency = burn_urgency(seconds);
    let magnitude = RESET_BURN * urgency + RESET_PRESERVE * (1.0 - urgency);
    let evidence = if urgency >= 1.0 {
        format!(
            "`{}` is in the {band} band and resets in {seconds}s, inside the \
             {RESET_BURN_HORIZON_SECONDS}s burn horizon — its remainder would otherwise \
             expire, so it is burned aggressively (line 1967)",
            entitlement.name()
        )
    } else if urgency <= 0.0 {
        format!(
            "`{}` is in the {band} band and resets in {seconds}s, at or past the \
             {RESET_PRESERVE_HORIZON_SECONDS}s preserve horizon — a low remainder with a far \
             reset is preserved, and the work routes to a sibling (line 1967)",
            entitlement.name()
        )
    } else {
        format!(
            "`{}` is in the {band} band and resets in {seconds}s, between the burn and \
             preserve horizons — {:.0}% of the burn preference applies (line 1967)",
            entitlement.name(),
            urgency * 100.0
        )
    };
    Contribution::new(TERM, magnitude, evidence)
}

/// How much of the burn preference applies at `seconds` until reset: `1.0`
/// inside [`RESET_BURN_HORIZON_SECONDS`] (a reset already past is the
/// clearest reason of all to stop conserving), `0.0` at or past
/// [`RESET_PRESERVE_HORIZON_SECONDS`], linear between.
fn burn_urgency(seconds: i64) -> f64 {
    if seconds <= 0 {
        // A reset already reached — or a stale reading from the deliberately
        // un-staled capacity cache whose window has since closed — means the
        // remainder has rolled over: there is nothing left to burn before it
        // expires, so it is no more urgent than a distant reset. Without this
        // guard a negative `seconds` slips under the burn horizon and scores
        // the maximum +1.0, making the router prefer the *stalest* account
        // over a fresh healthy one (caught by the 2026-08-31 investigation
        // swarm; it inverts this line's own intent).
        0.0
    } else if seconds <= RESET_BURN_HORIZON_SECONDS {
        1.0
    } else if seconds >= RESET_PRESERVE_HORIZON_SECONDS {
        0.0
    } else {
        1.0 - (seconds - RESET_BURN_HORIZON_SECONDS) as f64
            / (RESET_PRESERVE_HORIZON_SECONDS - RESET_BURN_HORIZON_SECONDS) as f64
    }
}

/// Line 1966's throttling factor: what the evidence window's informative
/// throttles against this entitlement cost it, account-scoped where 56A-2's
/// narrowing could honestly narrow the count and provider-wide otherwise —
/// and the evidence sentence says which, because a shared reading shown as a
/// per-account one would claim knowledge nothing measured.
pub fn entitlement_throttling(
    destination: &Destination,
    pool: &EntitlementPoolView,
) -> Contribution {
    const TERM: &str = "entitlement throttling";
    let entitlement = match entitlement_axis(TERM, destination, pool) {
        Ok(entitlement) => entitlement,
        Err(inert) => return inert,
    };
    let Some(throttling) = entitlement.throttling() else {
        return Contribution::new(
            TERM,
            0.0,
            format!(
                "`{}`'s recent throttling is unknown — nothing consulted the ledger for it, \
                 and \"none observed\" may only be said by a resolver that looked",
                entitlement.name()
            ),
        );
    };
    if throttling.throttled() == 0 {
        return Contribution::new(
            TERM,
            0.0,
            format!(
                "no recent throttle is recorded against `{}` ({})",
                entitlement.name(),
                throttling.scope_word()
            ),
        );
    }
    let magnitude = (throttling.throttled() as f64 * ENTITLEMENT_THROTTLE_PENALTY)
        .max(ENTITLEMENT_THROTTLE_FLOOR);
    Contribution::new(
        TERM,
        magnitude,
        format!(
            "{} recent throttle{} recorded against `{}` ({})",
            throttling.throttled(),
            if throttling.throttled() == 1 { "" } else { "s" },
            entitlement.name(),
            throttling.scope_word()
        ),
    )
}

/// Line 1966's model-availability factor. Only the established-positive case
/// carries weight: a declared catalogue naming this destination's model. The
/// established-negative case is [`super::Entitlement::model_constraint`]'s
/// hard refusal and never reaches a score; harness-decided, an unknown
/// facet, and a harness-picked model all contribute nothing and say why.
pub fn entitlement_model_availability(
    destination: &Destination,
    pool: &EntitlementPoolView,
) -> Contribution {
    const TERM: &str = "entitlement model availability";
    let entitlement = match entitlement_axis(TERM, destination, pool) {
        Ok(entitlement) => entitlement,
        Err(inert) => return inert,
    };
    let Some(models) = entitlement.models() else {
        return Contribution::new(
            TERM,
            0.0,
            format!(
                "`{}`'s model list is unknown — no catalogue was ever read, which is not a \
                 `no`",
                entitlement.name()
            ),
        );
    };
    match models {
        super::EntitlementModelsFacet::HarnessDecided => Contribution::new(
            TERM,
            0.0,
            format!(
                "`{}` is a native sign-in whose models the harness decides — nothing to check \
                 a destination's model against, and no list is invented",
                entitlement.name()
            ),
        ),
        super::EntitlementModelsFacet::Declared(declared) => {
            let Some(model) = destination.backend().model().name() else {
                return Contribution::new(
                    TERM,
                    0.0,
                    format!(
                        "the harness picks this destination's model itself, so `{}`'s declared \
                         list of {} constrains and earns nothing",
                        entitlement.name(),
                        declared.len()
                    ),
                );
            };
            if models.serves(model) {
                Contribution::new(
                    TERM,
                    ENTITLEMENT_MODEL_AVAILABLE,
                    format!(
                        "`{}`'s declared catalogue names `{model}` — established to serve this \
                         destination's model",
                        entitlement.name()
                    ),
                )
            } else {
                // Unreachable through `choose` — the hard constraint removed
                // such a candidate — but this is a public function, and a
                // direct caller is owed the honest sentence rather than a
                // panic.
                Contribution::new(
                    TERM,
                    0.0,
                    format!(
                        "`{}`'s declared catalogue does not name `{model}` — a hard entitlement \
                         constraint removes such a candidate before any score is taken",
                        entitlement.name()
                    ),
                )
            }
        }
    }
}

/// Line 1968's stickiness, said where a reader looks for it: a zero-weight
/// note on an existing session's explanation stating that keeping the
/// session on the entitlement holding its context is the `session affinity`
/// term's weight — one mechanism, priced once — and naming the two things
/// that do move it (a rule, through 56A-1's hard constraint; an exhausted
/// band, through [`entitlement_capacity`]). `None` for a fresh destination,
/// which holds no context to be sticky about, and outside a live pool.
fn entitlement_stickiness_note(
    destination: &Destination,
    pool: &EntitlementPoolView,
    affinity_magnitude: f64,
) -> Option<Contribution> {
    if !pool.offers_a_choice() || destination.is_fresh() {
        return None;
    }
    let entitlement = destination.entitlement()?;
    Some(Contribution::new(
        "entitlement stickiness",
        0.0,
        format!(
            "`{}` holds this session's context, and keeping the session there is already \
             priced by the `session affinity` term ({affinity_magnitude:+.3}) — stickiness is \
             that term's weight, not a second mechanism; only a rule that now denies this work \
             (the hard entitlement constraint) or an exhausted capacity band moves it \
             (line 1968)",
            entitlement.name()
        ),
    ))
}

// ---------------------------------------------------------------------------
// Phase 35C, lines 1559–1565 — the tier-movement decision.
//
// Escalation and downgrade are one decision, made once per `choose` over the
// candidate set, and they act on the ranking through the terms that already
// exist: the tier the fit term prefers, the tier the gate admits, and the
// tier the pressure terms read. No second warmth signal, no second health
// signal, no second capacity signal — every input below is one the terms
// above already price, read the same way they read it. What is new is the
// *movement* itself: a named, explained, recordable change to which tier this
// decision prefers, and the rules for when it may happen.
//
// Every threshold is stated once, here, with its reason.
// ---------------------------------------------------------------------------

/// Line 1559's *"repeatedly fail"*: this many consecutive observed failures
/// on a candidate, short of the cooldown [`super::free::ResourceHealth`]
/// would already have imposed. Two, because once is an incident and the
/// health term prices it at [`HEALTH_FAILURE_PENALTY`]; twice is a pattern
/// the tier above should be preferred over.
const REPEATED_FAILURES: u32 = 2;

/// Line 1565's cap: how many tiers one routing decision may escalate by
/// itself, whatever the triggers. **One.** A malformed task that trips every
/// trigger at once still moves one step, the step is printed, and the
/// triggers that would have moved it further are named as capped — so the
/// premium resources it did not reach are reachable only by a person.
const MAX_ESCALATION_STEPS: usize = 1;

/// Line 1560's *"task failure would be expensive"*: a task stated at this
/// tier or above. [`WorkloadTier::Heavy`]'s own doc — difficult debugging,
/// architecture-sensitive changes, broad refactors — is where a wrong tier
/// costs a whole attempt rather than a turn.
const EXPENSIVE_FAILURE_TIER: WorkloadTier = WorkloadTier::Heavy;

/// Line 1562's *"routine support work"*, tier half: at or below this tier.
/// Above it the work is by definition not routine, and the map's own tier
/// descriptions say so.
const ROUTINE_SUPPORT_CEILING: WorkloadTier = WorkloadTier::Standard;

/// Line 1562's *"premium capacity is tight"*: every metered candidate with a
/// reading is in this band or worse. The same band `pressure::TIGHT_BAND_PENALTY`
/// starts at, so "tight" means one thing across the module.
const DOWNGRADE_PRESSURE_BAND: CapacityBand = CapacityBand::Tight;

/// Line 1563: the longest expected duration a downgrade may bet on. A task
/// that runs several turns below its tier and fails is redone **in full** on
/// the premium resource the downgrade meant to spare — the retry then costs
/// at least what was saved, plus every turn already spent. Only single-turn
/// work has a retry cheap enough to risk.
const DOWNGRADE_RETRY_TOLERANCE: DurationClass = DurationClass::SingleTurn;

/// Line 1561: the tier delta a warm session's affinity is weighed against —
/// what an exact tier fit is worth over headroom, the one step a downgrade
/// would trade a warm higher-tier session's context for. Not a new number:
/// the difference between two constants this module already prices the tier
/// with, so the comparison is on the module's own scale.
const WARM_CONTEXT_TIER_DELTA: f64 = TIER_FIT_EXACT - TIER_FIT_HEADROOM;

/// The fit of a destination established *below* the tier an escalation moved
/// the preference to — reachable only after an escalation, because the gate
/// still admits the classified tier. Zero: kept eligible (it can serve the
/// work), not preferred (the escalation exists to prefer something else),
/// and its own health terms say why it was passed over.
const TIER_FIT_BELOW_MOVED: f64 = 0.0;

/// What asked for an escalation. Ordered as evaluated: a failure that
/// already happened outranks the set's health, which outranks the
/// classifier's uncertainty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscalationTrigger {
    /// Line 1564: the last exchange on the current destination ended in a
    /// model-capability failure.
    AttributableFailure(FailureClass),
    /// Line 1559: every candidate established at the classified tier is
    /// refused, cooling down, exhausted, or repeatedly failing.
    TierStruggling,
    /// Line 1560: deterministic heuristics rated the work heavy and no model
    /// confirmed it.
    HeuristicHeavy,
}

impl std::fmt::Display for EscalationTrigger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AttributableFailure(class) => write!(
                f,
                "the last exchange on the current destination ended in `{class}`, a \
                 model-capability failure (line 1564)"
            ),
            Self::TierStruggling => f.write_str(
                "every candidate established at the classified tier is refused, cooling down, \
                 exhausted or repeatedly failing (line 1559)",
            ),
            Self::HeuristicHeavy => f.write_str(
                "deterministic heuristics rated the work heavy and no model confirmed it — the \
                 verdict most expensive to be wrong about (line 1560)",
            ),
        }
    }
}

/// Why a stated tier stood. Every arm is a sentence a person reads in the
/// explanation, because "nothing moved" is only useful when it says why.
#[derive(Debug, Clone, PartialEq)]
pub enum HoldReason {
    /// No trigger fired. `retry_after` is what the router was told about the
    /// last exchange, kept so the explanation can say a health failure was
    /// seen and deliberately not promoted on.
    NoTrigger { retry_after: Option<FailureClass> },
    /// Triggers fired and the classified tier is already the top.
    AtTop { triggers: Vec<EscalationTrigger> },
    /// Triggers fired and no healthy candidate is established at the tier
    /// above — the preference stays rather than pointing at nothing.
    NoTarget {
        to: WorkloadTier,
        triggers: Vec<EscalationTrigger>,
    },
    /// Line 1563: routine work under pressure, not downgraded, because a
    /// failed retry would cost more than the downgrade saves.
    RetryCost { duration: DurationClass },
    /// Line 1561: routine work under pressure, not downgraded, because an
    /// existing higher-tier session's context outweighs the tier delta.
    WarmContext { session: String, affinity: f64 },
    /// Line 1562 wanted to downgrade and no free candidate adequate for the
    /// work is available to take it.
    NoFreeResource { to: WorkloadTier },
}

/// What one routing decision did to the tier it prefers — lines 1559–1565.
#[derive(Debug, Clone, PartialEq)]
pub enum TierMovement {
    /// The classified tier stands, and `reason` says why.
    Held {
        tier: WorkloadTier,
        reason: HoldReason,
    },
    /// One tier up (line 1565: never more). `trigger` is what moved it;
    /// `capped` is every further trigger that fired and was not applied.
    Escalated {
        from: WorkloadTier,
        to: WorkloadTier,
        trigger: EscalationTrigger,
        capped: Vec<EscalationTrigger>,
    },
    /// One tier down (line 1562). `target` is the free destination that made
    /// the downgrade worth taking — the existence proof, not the winner.
    Downgraded {
        from: WorkloadTier,
        to: WorkloadTier,
        target: String,
    },
}

impl TierMovement {
    /// The tier the work was classified at.
    pub fn classified(&self) -> WorkloadTier {
        match self {
            Self::Held { tier, .. } => *tier,
            Self::Escalated { from, .. } | Self::Downgraded { from, .. } => *from,
        }
    }

    /// The tier this decision prefers — what [`workload_tier_fit`] reads.
    pub fn preferred_tier(&self) -> WorkloadTier {
        match self {
            Self::Held { tier, .. } => *tier,
            Self::Escalated { to, .. } | Self::Downgraded { to, .. } => *to,
        }
    }

    /// The tier the hard gate admits — what line 1516's arm reads. A
    /// downgrade lowers it, so a cheaper resource the classified tier would
    /// have refused becomes eligible; an escalation **never** raises it,
    /// because a preference may not remove a candidate that can serve the
    /// work (design decision 1).
    pub fn gate_tier(&self) -> WorkloadTier {
        match self {
            Self::Downgraded { to, .. } => *to,
            Self::Held { .. } | Self::Escalated { .. } => self.classified(),
        }
    }

    /// The tier the pressure terms read: the **lower** of the classified and
    /// the preferred tier. A downgrade reaches the spending protections —
    /// a leaf-tier reading makes `low_tier_spend` and the reserve gate
    /// stricter, the fail-closed direction — and an escalation never
    /// reaches them: a preference for a stronger resource must not be what
    /// unlocks protected premium reserve for work the classifier did not
    /// rate that high (the hazard practice §79 records for exactly this
    /// input).
    pub fn pressure_tier(&self) -> WorkloadTier {
        self.classified().min(self.preferred_tier())
    }

    /// Whether the preference actually moved.
    pub fn fired(&self) -> bool {
        !matches!(self, Self::Held { .. })
    }

    /// One sentence a person reads — the whole decision and its reason.
    pub fn describe(&self) -> String {
        match self {
            Self::Held {
                tier,
                reason: HoldReason::NoTrigger { retry_after },
            } => {
                let mut out = format!(
                    "the classified `{tier}` tier stands — no escalation trigger fired and the \
                     work is not routine support work under premium pressure"
                );
                if let Some(class) = retry_after {
                    out.push_str(&format!(
                        "; the last exchange's `{class}` failure is a provider-health or quota \
                         fact, priced by provider health and not promoted on (line 1564)"
                    ));
                }
                out
            }
            Self::Held {
                tier,
                reason: HoldReason::AtTop { triggers },
            } => format!(
                "`{tier}` is the top of the scale, so it stands although {} asked to escalate",
                list(triggers)
            ),
            Self::Held {
                tier,
                reason: HoldReason::NoTarget { to, triggers },
            } => format!(
                "{} asked to escalate to `{to}` and no healthy candidate is established at that \
                 tier — the preference stays at `{tier}` rather than pointing at nothing",
                list(triggers)
            ),
            Self::Held {
                tier,
                reason: HoldReason::RetryCost { duration },
            } => format!(
                "routine work under premium pressure, kept at `{tier}`: it is expected to run \
                 {duration}, and a multi-turn task that fails below its tier is redone in full \
                 on the premium resource the downgrade meant to spare (line 1563)"
            ),
            Self::Held {
                tier,
                reason: HoldReason::WarmContext { session, affinity },
            } => format!(
                "routine work under premium pressure, kept at `{tier}`: `{session}` is an \
                 existing session established at or above it whose session affinity \
                 ({affinity:+.3}) outweighs the tier delta ({WARM_CONTEXT_TIER_DELTA:+.3}) a \
                 downgrade would trade its context for (line 1561)"
            ),
            Self::Held {
                tier,
                reason: HoldReason::NoFreeResource { to },
            } => format!(
                "routine work under premium pressure, kept at `{tier}`: no free candidate \
                 adequate for it and able to serve `{to}` is available to take it (line 1562)"
            ),
            Self::Escalated {
                from,
                to,
                trigger,
                capped,
            } => {
                let mut out = format!(
                    "escalated from `{from}` to `{to}` — {trigger}; one tier per decision \
                     (line 1565)"
                );
                if !capped.is_empty() {
                    out.push_str(&format!(
                        ", and {} would also have escalated and were capped",
                        list(capped)
                    ));
                }
                out
            }
            Self::Downgraded { from, to, target } => format!(
                "downgraded from `{from}` to `{to}` — routine support work, every metered \
                 candidate with a reading is in the {DOWNGRADE_PRESSURE_BAND} band or worse, \
                 and `{target}` is a free resource able to take it (line 1562)"
            ),
        }
    }
}

fn list(triggers: &[EscalationTrigger]) -> String {
    triggers
        .iter()
        .map(|trigger| format!("[{trigger}]"))
        .collect::<Vec<_>>()
        .join(" and ")
}

/// The tier one step down, when there is one a model serves.
/// [`WorkloadTier::Deterministic`] is *"should not require an LLM"*, so
/// nothing below [`WorkloadTier::Leaf`] is a place to route model work to.
fn one_tier_below(tier: WorkloadTier) -> Option<WorkloadTier> {
    match tier {
        WorkloadTier::Deterministic | WorkloadTier::Leaf => None,
        WorkloadTier::Standard => Some(WorkloadTier::Leaf),
        WorkloadTier::Heavy => Some(WorkloadTier::Standard),
        WorkloadTier::Frontier => Some(WorkloadTier::Heavy),
    }
}

/// Whether `destination` is one line 1559 would escalate away from: refused
/// or cooling down (the two facts [`provider_health`] prices at
/// [`HEALTH_UNAVAILABLE_PENALTY`]), exhausted (the band the pressure terms
/// read), or repeatedly failing ([`REPEATED_FAILURES`]).
fn struggling(destination: &Destination, inputs: &RouterInputs<'_>) -> bool {
    let health = inputs.health.health(&FreeResource::new(
        destination.backend().credential().clone(),
        destination.backend().model().label(),
    ));
    health.credential_was_rejected()
        || !health.is_available(inputs.now)
        || health.consecutive_failures() >= REPEATED_FAILURES
        || destination.capacity_facts().band() == Some(CapacityBand::Exhausted)
}

/// Lines 1559–1565 in one function: decide whether this decision moves the
/// tier it prefers, over the candidates that survived the capability
/// constraints and before the tier gate runs.
///
/// `None` when no tier was stated — no task, so nothing to move and nothing
/// to explain, the same preservation clause every tier term keeps.
///
/// Escalation is decided first and downgrade second, and they are exclusive:
/// a decision that escalates does not also ask whether it should have
/// downgraded. Within escalation the triggers are collected in a fixed
/// order, the first moves the tier by [`MAX_ESCALATION_STEPS`], and the rest
/// are reported as capped (line 1565). An escalation needs somewhere to go:
/// a candidate established at the tier above that is not itself struggling,
/// or the preference would point at nothing.
fn decide_tier_movement(
    candidates: &[&Destination],
    inputs: &RouterInputs<'_>,
    retry_after: Option<FailureClass>,
) -> Option<TierMovement> {
    let from = inputs.requirements.minimum_tier?;
    let answer = inputs.requirements.classification.as_ref();

    // --- escalation triggers, in order ------------------------------------
    let mut triggers = Vec::new();
    if let Some(class @ (FailureClass::RequestIncompatibility | FailureClass::EmptyCompletion)) =
        retry_after
    {
        triggers.push(EscalationTrigger::AttributableFailure(class));
    }
    let at_tier: Vec<&Destination> = candidates
        .iter()
        .copied()
        .filter(|destination| destination.tier_ceiling() == Some(from))
        .collect();
    if !at_tier.is_empty()
        && at_tier
            .iter()
            .all(|destination| struggling(destination, inputs))
    {
        triggers.push(EscalationTrigger::TierStruggling);
    }
    if let Some(answer) = answer
        && answer.classification().source().is_heuristic()
        && answer.stated_tier() >= EXPENSIVE_FAILURE_TIER
        && !answer.is_conservative()
    {
        triggers.push(EscalationTrigger::HeuristicHeavy);
    }

    if !triggers.is_empty() {
        let steps = triggers.len().min(MAX_ESCALATION_STEPS);
        let mut to = from;
        for _ in 0..steps {
            to = to.escalate();
        }
        if to == from {
            return Some(TierMovement::Held {
                tier: from,
                reason: HoldReason::AtTop { triggers },
            });
        }
        let target_exists = candidates.iter().any(|destination| {
            destination
                .tier_ceiling()
                .is_some_and(|ceiling| ceiling >= to)
                && !struggling(destination, inputs)
        });
        if !target_exists {
            return Some(TierMovement::Held {
                tier: from,
                reason: HoldReason::NoTarget { to, triggers },
            });
        }
        let trigger = triggers[0];
        let capped = triggers[steps..].to_vec();
        return Some(TierMovement::Escalated {
            from,
            to,
            trigger,
            capped,
        });
    }

    let held = |reason| Some(TierMovement::Held { tier: from, reason });
    let no_trigger = HoldReason::NoTrigger { retry_after };

    // --- downgrade: routine support work under premium pressure -----------
    let Some(answer) = answer else {
        return held(no_trigger);
    };
    let routine = matches!(
        answer.task_class(),
        TaskClass::Question | TaskClass::Investigation
    ) && from <= ROUTINE_SUPPORT_CEILING;
    let Some(to) = one_tier_below(from).filter(|_| routine) else {
        return held(no_trigger);
    };
    // "Premium capacity is tight": every metered candidate with a reading
    // is in the tight band or worse, and at least one was read. An unread
    // resource is neither tight nor healthy, here as everywhere.
    let metered_bands: Vec<CapacityBand> = candidates
        .iter()
        .filter(|destination| !destination.backend().cost().is_free())
        .filter_map(|destination| destination.capacity_facts().band())
        .collect();
    if metered_bands.is_empty()
        || metered_bands
            .iter()
            .any(|band| *band > DOWNGRADE_PRESSURE_BAND)
    {
        return held(no_trigger);
    }
    // Somewhere to go: free, able to serve now, adequate for the task, and
    // not established below the tier the downgrade moves to.
    let Some(target) = candidates.iter().find(|destination| {
        destination.backend().cost().is_free()
            && provider_available(destination, inputs.health, inputs.now)
            && is_adequate(destination, &inputs.requirements)
            && destination
                .tier_ceiling()
                .is_none_or(|ceiling| ceiling >= to)
    }) else {
        return held(HoldReason::NoFreeResource { to });
    };
    // Line 1563: the retry brake.
    let duration = answer.expected_duration();
    if duration > DOWNGRADE_RETRY_TOLERANCE {
        return held(HoldReason::RetryCost { duration });
    }
    // Line 1561: the warm-context brake, on the affinity term itself.
    if let Some((session, affinity)) = candidates
        .iter()
        .filter(|destination| {
            !destination.is_fresh()
                && destination
                    .tier_ceiling()
                    .is_some_and(|ceiling| ceiling >= from)
        })
        .map(|destination| {
            (
                destination.id().to_owned(),
                // `current` is `None` here on purpose: this brake runs before a
                // destination is chosen, so there is no move whose cache locality
                // the prompt-cache facet could read. Leaving it unknown makes the
                // brake slightly LESS likely to hold, which is the conservative
                // side for a rule that keeps work on a premium session.
                session_affinity(destination, None, &inputs.requirements).magnitude(),
            )
        })
        .find(|(_, affinity)| *affinity >= WARM_CONTEXT_TIER_DELTA)
    {
        return held(HoldReason::WarmContext { session, affinity });
    }
    Some(TierMovement::Downgraded {
        from,
        to,
        target: target.id().to_owned(),
    })
}

/// The movement as a zero-weight line in every candidate's explanation, so
/// the terms it changed are read beside the reason they changed.
fn tier_movement_note(movement: &TierMovement) -> Contribution {
    Contribution::new("tier movement", 0.0, movement.describe())
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

// ---------------------------------------------------------------------------
// Map line 1970 — the tier-preserving fallback across the pool.
//
// The user's ruling of 2026-08-31 (`design-decisions.md` §Phase 56A, "Step
// 4's fallback order") is the instruction of record for everything in this
// block, and its shape was settled there too: *"line 1970's fallback becomes
// a post-ranking reselection over `Routed::considered()`'s already-complete
// list — which preserves additive, never a filter"*. Nothing here removes a
// candidate, changes a score, or reorders the ranking; it moves the winner,
// once, and says so.
// ---------------------------------------------------------------------------

/// Why map line 1970's fallback fired — the two triggers the line names and
/// no others.
///
/// There is deliberately no third variant for *scored badly*. A fallback is
/// not a preference: the ruling's whole risk is a silent downgrade, so the
/// trigger is a fact about the account (its capacity band reads exhausted,
/// or the ledger recorded a throttle against it) and never a comparison
/// between two accounts. An untriggered decision is byte-identical to the
/// one this router made before line 1970 existed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackReason {
    /// The chosen entitlement's capacity band reads
    /// [`CapacityBand::Exhausted`].
    Exhausted,
    /// The evidence window recorded at least one informative throttle
    /// against the chosen entitlement.
    Throttled,
}

impl FallbackReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Exhausted => "exhausted",
            Self::Throttled => "throttled",
        }
    }
}

impl std::fmt::Display for FallbackReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which step of the ruling's order matched — map line 1970's *"stated
/// order"*, as four steps rather than the line's three words, because the
/// ruling adds the constraint the line does not say and which is the part
/// that matters: **it is tier-preserving**.
///
/// > *"switch to another subscription always if same model or model of
/// > similar capability is available in another. You can't put a fable 5
/// > task and switch it to a nemotron v3 … If subscription model of
/// > capability is not available switch to api one - if available."*
///
/// [`Self::ORDER`] is that sentence, in order, and it is the only place the
/// order is written down: the reselection walks this slice and stops at the
/// first match, so a build that ranked an API-credit account above a
/// subscription one would have to reorder this constant to do it. That is
/// what the order mutation in this package's evidence changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackStep {
    /// Another **subscription** serving the **same model**.
    SubscriptionSameModel,
    /// Another **subscription** serving a model of the **same capability
    /// tier**. Unreachable until Phase 34F's axis lands — see
    /// [`super::same_capability_tier`].
    SubscriptionSameTier,
    /// An **API-credit** account serving the **same model**.
    ApiCreditsSameModel,
    /// An **API-credit** account serving a model of the **same capability
    /// tier**. Unreachable until Phase 34F's axis lands.
    ApiCreditsSameTier,
}

impl FallbackStep {
    /// The ruling's order, and the only statement of it in this build.
    pub const ORDER: [Self; 4] = [
        Self::SubscriptionSameModel,
        Self::SubscriptionSameTier,
        Self::ApiCreditsSameModel,
        Self::ApiCreditsSameTier,
    ];

    /// What a candidate must be paid for by to match this step.
    /// [`EntitlementSource::Unstated`] matches no step: an entry that names
    /// no backing is *listed, never matched, never charged*, and an order
    /// over subscriptions and API credits has no place for it.
    fn source(self) -> EntitlementSource {
        match self {
            Self::SubscriptionSameModel | Self::SubscriptionSameTier => {
                EntitlementSource::Subscription
            }
            Self::ApiCreditsSameModel | Self::ApiCreditsSameTier => EntitlementSource::ApiCredits,
        }
    }

    /// Whether this step accepts a move from `from` to `to` on the model
    /// axis — the ruling's tier-preserving constraint, and the whole of
    /// what stops a fallback from silently downgrading the work.
    ///
    /// **Unknown narrows, and never widens.** A pair Phase 34F's axis has
    /// not ranked reads [`TierRelation::Unknown`], which is `false` on the
    /// tier steps; so while the axis is absent the order collapses to its
    /// two same-model steps, and a fallback that cannot establish it is
    /// preserving the tier simply does not happen. The ruling is explicit
    /// that this is the safe direction: a fallback that silently downgrades
    /// the model *"is worse than a refusal, because the work continues and
    /// looks fine"*.
    fn accepts(self, from: &Destination, to: &Destination) -> bool {
        match self {
            Self::SubscriptionSameModel | Self::ApiCreditsSameModel => same_model(from, to),
            Self::SubscriptionSameTier | Self::ApiCreditsSameTier => {
                match (from.backend().model().name(), to.backend().model().name()) {
                    (Some(from_model), Some(to_model)) => {
                        from_model != to_model
                            && same_capability_tier(from.capability_tier(), to.capability_tier())
                                == TierRelation::Same
                    }
                    // A model the harness picks itself has no name for an
                    // axis to have ranked. Unknown, and unknown narrows.
                    _ => false,
                }
            }
        }
    }

    /// The clause a person reads inside the fallback's own sentence.
    pub fn describe(self) -> &'static str {
        match self {
            Self::SubscriptionSameModel => "another subscription serving the same model",
            Self::SubscriptionSameTier => {
                "another subscription serving a model of the same capability tier"
            }
            Self::ApiCreditsSameModel => "an API-credit account serving the same model",
            Self::ApiCreditsSameTier => {
                "an API-credit account serving a model of the same capability tier"
            }
        }
    }
}

/// One fallback map line 1970 made: which account the ranking had chosen,
/// which one the work went to instead, why, and which step of the order
/// matched.
///
/// Carried on [`Routed`] beside [`TierMovement`] and for its reason — the
/// decision is the router's, and what to do with the record is the caller's.
/// `glasshouse route` renders it (see [`Routed::render`]) and records
/// nothing, exactly as it does for a moved tier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntitlementFallback {
    from: String,
    to: String,
    from_destination: String,
    to_destination: String,
    reason: FallbackReason,
    step: FallbackStep,
}

impl EntitlementFallback {
    /// The entitlement the ranking had chosen and the fallback moved off.
    pub fn from(&self) -> &str {
        &self.from
    }

    /// The entitlement the work went to instead.
    pub fn to(&self) -> &str {
        &self.to
    }

    /// [`Destination::id`] of the candidate the ranking had chosen.
    pub fn from_destination(&self) -> &str {
        &self.from_destination
    }

    /// [`Destination::id`] the work went to instead.
    pub fn to_destination(&self) -> &str {
        &self.to_destination
    }

    pub fn reason(&self) -> FallbackReason {
        self.reason
    }

    pub fn step(&self) -> FallbackStep {
        self.step
    }

    /// The sentence a person reads, and the one a durable record should
    /// carry: both accounts, the trigger, and the step of the order that
    /// matched. One wording, so an explanation and a record cannot describe
    /// one fallback two ways.
    pub fn describe(&self) -> String {
        format!(
            "entitlement `{}` is {} — the work moved to `{}`, {} (map line 1970's order)",
            self.from,
            self.reason,
            self.to,
            self.step.describe()
        )
    }
}

/// Whether these two candidates would run the **same model** — map line
/// 1970's first and third steps.
///
/// Two named models are the same when their names are. Two
/// [`super::AssignedModel::HarnessDefault`] candidates are the same when
/// they are the **same harness on the same launch profile**, which is a
/// fact rather than a guess: the harness makes one choice for one profile,
/// so two accounts offered for that profile differ in the account and in
/// nothing else — and that is the pool's own commonest shape, the
/// `fresh:<harness>:<profile>@<account>` candidates line 1953 splits a
/// profile into. Anything else — one named and one harness-picked, or two
/// harness-picked candidates on different profiles — is **not established**
/// to be the same model and is therefore not the same model, on the ruling's
/// own direction.
fn same_model(from: &Destination, to: &Destination) -> bool {
    match (from.backend().model().name(), to.backend().model().name()) {
        (Some(from), Some(to)) => from == to,
        (None, None) => {
            from.harness() == to.harness() && from.launch_profile() == to.launch_profile()
        }
        _ => false,
    }
}

/// Whether this entitlement is in a state map line 1970 falls back **from**,
/// and which of the line's two triggers it is.
///
/// Exhaustion outranks throttling when both read true: a spent allowance is
/// the stronger fact and the one a person would name. An entitlement whose
/// band nobody read and whose ledger nobody consulted answers `None` —
/// *unknown is not exhausted*, the rule every other gate in this module
/// follows, and the one that keeps a build with no telemetry routing exactly
/// as it did before this function existed.
fn fallback_trigger(entitlement: &super::Entitlement) -> Option<FallbackReason> {
    if entitlement.capacity_band() == Some(CapacityBand::Exhausted) {
        return Some(FallbackReason::Exhausted);
    }
    if entitlement
        .throttling()
        .is_some_and(|throttling| throttling.throttled() > 0)
    {
        return Some(FallbackReason::Throttled);
    }
    None
}

/// Map line 1970's reselection: given the ranking and the index it settled
/// on, the index the work should move to instead and the record of why.
///
/// `ranked` is the already-ranked, already-gated list `Routed` keeps as
/// [`Routed::considered`], best first — so every candidate this function can
/// reach has passed every hard constraint, **map line 1971's rules
/// included**. That is the whole of
/// how *"an exhausted pool does not license exceeding a rule"* is enforced:
/// there is no path from here to a candidate the gate removed, because the
/// gate ran first and this function never sees its rejections.
///
/// `None` — no fallback — whenever any of these holds, and each is a
/// deliberate narrowing rather than an omission:
///
/// - the candidate set carries fewer than two configured entitlements, so
///   there is no pool to fall back across (the gate every pool term checks);
/// - the chosen candidate carries no entitlement, or its entitlement is
///   neither exhausted nor throttled — the untriggered case, which must stay
///   byte-identical to today's decision;
/// - no step of [`FallbackStep::ORDER`] matched a **healthy** candidate on a
///   **different** account. A sibling in the same state is not a refuge, and
///   a second candidate on the *same* account is the same account.
pub fn entitlement_fallback(
    ranked: &[&Destination],
    chosen_index: usize,
    pool: &EntitlementPoolView,
) -> Option<(usize, EntitlementFallback)> {
    if !pool.offers_a_choice() {
        return None;
    }
    let from = ranked.get(chosen_index)?;
    let from_entitlement = from.entitlement()?;
    let reason = fallback_trigger(from_entitlement)?;

    for step in FallbackStep::ORDER {
        for (index, candidate) in ranked.iter().enumerate() {
            if index == chosen_index {
                continue;
            }
            let Some(entitlement) = candidate.entitlement() else {
                continue;
            };
            if entitlement.name() == from_entitlement.name() {
                continue;
            }
            if fallback_trigger(entitlement).is_some() {
                continue;
            }
            if entitlement.source() != step.source() {
                continue;
            }
            if !step.accepts(from, candidate) {
                continue;
            }
            return Some((
                index,
                EntitlementFallback {
                    from: from_entitlement.name().to_owned(),
                    to: entitlement.name().to_owned(),
                    from_destination: from.id().to_owned(),
                    to_destination: candidate.id().to_owned(),
                    reason,
                    step,
                },
            ));
        }
    }
    None
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
                    forecast: destination.burn_forecast(),
                };
                let explanation = score(
                    destination,
                    current,
                    inputs,
                    &pressure,
                    movement.as_ref(),
                    &pool,
                    &self.prices,
                    &self.score_weights,
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

        // Step 5, map line 1970: the post-ranking reselection. It runs after
        // the override and over the same already-gated list, so it can only
        // move the work to a candidate every hard constraint already
        // admitted — which is how line 1971's rules hold under fallback
        // pressure without a second check.
        //
        // **It never moves an account the user named.** An override *"may
        // overrule a ranking and not a fact about what can serve"*, and this
        // is neither: it is Glasshouse preferring one admissible account
        // over another, which is exactly the choice a person who named the
        // account has already made. Their account being throttled is a thing
        // the explanation tells them — the throttling term is right there in
        // the block — not a thing to overrule them about.
        //
        // *Named the account*, and not merely used an override: `--to` with
        // a bare `fresh:<harness>:<profile>` names the **profile**, and
        // [`destination_answers_to`] records that the ranking still chooses
        // the account among that profile's candidates (line 1969). So a
        // prefix override, and `--fresh`, leave the account to Glasshouse
        // and the fallback applies to it; only an exact id — `@<account>`
        // included — and `--hold`, which says *stay where you are*, take the
        // account out of Glasshouse's hands. An override that was refused
        // leaves the ranking's own answer standing, and the fallback applies
        // to that.
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

/// Lines 1595 to 1600, in the order a reader compares them: what the harness
/// can do, what the session already holds, what the provider has cached, what
/// is left of the quota, how the provider has behaved, and what the move
/// costs.
#[allow(clippy::too_many_arguments)]
fn score(
    destination: &Destination,
    current: Option<&Destination>,
    inputs: &RouterInputs<'_>,
    pressure: &PressureInputs<'_>,
    movement: Option<&TierMovement>,
    pool: &EntitlementPoolView,
    prices: &PriceTable,
    weights: &ScoreWeights,
) -> RoutingExplanation {
    let mut explanation = RoutingExplanation::new();
    if let Some(answer) = &inputs.requirements.classification {
        explanation.push(classification_note(answer));
    }
    explanation.push(harness_capability_fit(destination, inputs.overrides));
    explanation.push(pairing_prior(destination, inputs));
    explanation.push(capability_fit(destination, &inputs.requirements));
    // `movement` is `Some` exactly when a tier was stated — `decide_tier_movement`
    // answers `None` otherwise — so the three terms under it keep the
    // preservation clause every tier term has kept: a launch that states no
    // task renders exactly what it rendered before any of them existed.
    if let Some(movement) = movement {
        explanation.push(workload_tier_fit(destination, movement.preferred_tier()));
        // Line 1558, pushed under the same condition and for the same
        // reason: "the cheapest candidate that satisfies the required
        // workload tier" has no subject until a tier has been required.
        explanation.push(cost_preference(destination));
        explanation.push(tier_movement_note(movement));
    }
    let affinity = session_affinity(destination, current, &inputs.requirements);
    let affinity_magnitude = affinity.magnitude();
    explanation.push(affinity);
    // Phase 56A lines 1953/1966–1969: the pool's own terms, right after the
    // affinity factor they share line 1966 with. Inert — every one of them,
    // saying so — for a candidate set that carries fewer than two configured
    // entitlements, which is what keeps a zero-or-one-entitlement user's
    // ranking byte-for-byte what it was.
    if let Some(note) = entitlement_stickiness_note(destination, pool, affinity_magnitude) {
        explanation.push(note);
    }
    explanation.push(entitlement_capacity(destination, pool));
    explanation.push(entitlement_reset_boundary(destination, pool));
    explanation.push(entitlement_throttling(destination, pool));
    explanation.push(entitlement_model_availability(destination, pool));
    explanation.push(prompt_cache_state(destination, current));
    explanation.push(quota_pressure(destination, weights));
    // Phase 35D, lines 1570–1577: the band the quota reading falls in, and
    // what the scope's reserve policy makes of it — placed right after the
    // reading it qualifies, so a reader sees the percentage and the band
    // together.
    explanation.push(pressure::capacity_band_pressure(pressure));
    // Phase 32E line 1280: the forecast, right after the band it
    // strengthens — a reader sees "tight" and "and it may not reach its
    // reset" together, in that order. Inert and saying so for every
    // destination with no forecast, which is what keeps a ranking on a
    // build that reads no ledger byte-for-byte what it was.
    explanation.push(pressure::exhaustion_forecast_pressure(pressure));
    explanation.push(pressure::low_tier_spend(pressure));
    explanation.push(provider_health(
        destination,
        inputs.health,
        inputs.now,
        weights,
    ));
    explanation.push(cadence_availability(destination, inputs.health, inputs.now));
    explanation.push(switching_and_bootstrap_cost(destination, current));
    // Line 1538, pushed unconditionally (unlike `cost_preference` above) and
    // reading the un-shadowed parameter so it sees `None` exactly when the
    // block above did not run — see `expected_marginal_cost`'s own doc for
    // why the two must never both price a candidate.
    explanation.push(expected_marginal_cost(destination, movement, prices));
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

/// Line 1518's exclusion, read directly from `pool.health` rather than
/// through [`provider_available`]. `provider_available` folds credential
/// rejection and *any* cooldown — declared or invented — into one boolean
/// for its two existing callers ([`decide_tier_movement`],
/// [`alternatives_for`]), and both must keep pricing an invented cooldown
/// as a soft penalty rather than excluding on it (line 534). Extending it
/// with the cause would hand that distinction to callers that must not act
/// on it; a second, narrower read is smaller than teaching the existing one
/// a case its callers need to ignore.
///
/// `None` when nothing here excludes: no rejection, no cooldown, or a
/// cooldown whose cause is `Invented` or was never established (adopted
/// health — see `FreePool::adopt_observed`) — [`super::free::ResourceHealth::declared_wait_remaining`]
/// already answers exactly that question.
fn provider_unavailable_cause(
    destination: &Destination,
    pool: &FreePool,
    now: Instant,
) -> Option<ProviderUnavailableCause> {
    let health = pool.health(&FreeResource::new(
        destination.backend().credential().clone(),
        destination.backend().model().label(),
    ));
    if health.credential_was_rejected() {
        return Some(ProviderUnavailableCause::CredentialRejected);
    }
    if health.declared_wait_remaining(now).is_some() {
        return Some(ProviderUnavailableCause::DeclaredCooldown);
    }
    None
}

/// The gate step 2 runs. Five constraints and no others, for the same
/// reason [`crate::routing::interactive`]'s `compatible` has two: each is a
/// fact about whether the destination *can* serve, not a preference about
/// whether it *should*.
///
/// Two of the five — map lines 1517 and 1518 — are asked on both passes,
/// like tool semantics and protocol: whether a destination lacks a required
/// hard capability, or whether its provider has refused the credential or
/// declared a still-active cooldown, does not depend on which tier the
/// movement settled. Both follow the same "established, not merely unread"
/// rule as the others: an unverified capability axis and an *invented*
/// cooldown are not "cannot," so neither excludes — see [`is_adequate`] and
/// [`provider_unavailable_cause`].
///
/// The fifth — map line 1516 — fires only on an **established** ceiling
/// strictly below the required tier. A destination with no ceiling stated
/// passes, because "nobody has said" is not "cannot"; the same rule the
/// other constraints already follow for `Unverified` tool semantics and an
/// unknown protocol.
///
/// `minimum_tier` is the tier the gate reads — [`TierMovement::gate_tier`]
/// once the movement is decided, and `None` for the pass that decides it
/// (the two capability constraints only). It is an argument rather than
/// `inputs.requirements.minimum_tier` so a downgrade (line 1562) can admit
/// a resource the classified tier would have refused, in exactly one place.
fn hard_constraint(
    destination: &Destination,
    inputs: &RouterInputs<'_>,
    minimum_tier: Option<WorkloadTier>,
    entitlement_axis: bool,
) -> Result<(), HardConstraint> {
    // Phase 56 line 1954, asked first: the user's own rule about what a
    // entitlement may be charged for is the strongest statement in this
    // gate, and when a destination fails it *and* a capability fact, the
    // constraint a person reads should be the one they wrote. The harness
    // half is asked on both passes; the tier half reads `minimum_tier`, so it
    // — like line 1516's ceiling gate below — fires only on the pass that
    // knows the tier the movement settled, and never against an unknown one
    // (`super::EntitlementRules::refusal`).
    if let Some(entitlement) = destination.entitlement() {
        entitlement.constraint(destination.harness(), minimum_tier)?;
        // Map line 1971's fourth axis, asked beside the other three and on
        // both passes: a spend ceiling is a rule the **user wrote**, not a
        // reading this build took, so — unlike the model half below — it is
        // not gated on the pool axis. A person who set a ceiling on their
        // one account meant it, and an account over its ceiling is over it
        // whether or not a second one exists. It refuses only when the
        // ceiling and the spend are BOTH established; see
        // `super::Entitlement::spend_constraint`.
        entitlement.spend_constraint()?;
        // Line 1953's model half, asked on both passes like the harness
        // half (the destination's model is known independently of any
        // tier), and only when the offered set carries the entitlement axis
        // at all — see `gate` for why a pool of one is exempt. A declared
        // catalogue that does not name the model refuses the candidate by
        // name; harness-decided and unknown facets constrain nothing.
        if entitlement_axis {
            entitlement.model_constraint(destination.backend().model())?;
        }
    }
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
    // Line 1517, asked on both passes like the two facts above: whether the
    // destination *can* serve is independent of which tier movement decided
    // to admit, so this does not wait for `minimum_tier` to resolve.
    // `is_adequate` refuses only an axis established absent
    // (`Declared::Verified { value: false }`); an unverified axis is "nobody
    // has said," not "cannot," and keeps passing to be priced by
    // `capability_fit` exactly as before this gate existed.
    if !is_adequate(destination, &inputs.requirements) {
        return Err(HardConstraint::Capability);
    }
    // Line 1518, same reasoning: a provider that has refused the credential
    // or declared a cooldown still in force cannot serve either pass asks
    // about, so it is excluded rather than merely priced worse by
    // `provider_health`. An *invented* cooldown is Glasshouse's own guess
    // (line 534) and stays a soft penalty — see `provider_unavailable_cause`.
    if let Some(cause) = provider_unavailable_cause(destination, inputs.health, inputs.now) {
        return Err(HardConstraint::ProviderUnavailable {
            credential: destination.backend().credential().label(),
            cause,
        });
    }
    if let (Some(required), Some(offered)) = (minimum_tier, destination.tier_ceiling())
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

#[cfg(test)]
mod burn_urgency_tests {
    use super::{RESET_BURN_HORIZON_SECONDS, RESET_PRESERVE_HORIZON_SECONDS, burn_urgency};

    /// Line 1967's reset-boundary term: a reset within the burn horizon is
    /// urgent (+1.0), at or past the preserve horizon it is not (0.0) — and a
    /// reset ALREADY PAST (negative seconds, routine over the persisted,
    /// deliberately un-staled capacity cache) must score 0.0, never the max
    /// +1.0 an unguarded `<= horizon` gave it. The 2026-08-31 investigation
    /// swarm caught the unguarded form rewarding the *stalest* account over a
    /// fresh healthy one, inverting this line's own intent.
    #[test]
    fn a_reset_already_past_is_not_urgent() {
        assert_eq!(burn_urgency(4_800), 1.0, "1h20m — the user's burn example");
        assert_eq!(burn_urgency(RESET_BURN_HORIZON_SECONDS), 1.0);
        assert_eq!(burn_urgency(RESET_PRESERVE_HORIZON_SECONDS), 0.0);
        assert_eq!(burn_urgency(0), 0.0, "resetting now is already replenished");
        assert_eq!(burn_urgency(-1), 0.0, "one second past reset");
        assert_eq!(
            burn_urgency(-345_600),
            0.0,
            "days past — the stale-cache case"
        );
    }
}

#[cfg(test)]
mod provider_health_tests {
    use super::*;
    use crate::routing::{AssignedModel, Cost, CredentialId};
    use crate::secret::SecretRef;

    fn destination(credential_var: &str) -> Destination {
        Destination::fresh(
            "dest-1",
            IntegrationId::ClaudeCode,
            "profile",
            Backend::new(
                "anthropic",
                "anthropic-messages",
                AssignedModel::named("claude-opus-4-1"),
                CredentialId::new(
                    "anthropic",
                    SecretRef::Environment {
                        var: credential_var.to_owned(),
                    },
                ),
                Cost::Metered,
                ToolSemantics::Verified,
            ),
            None,
        )
    }

    /// A pool whose only recorded fact is `failures` consecutive failures on
    /// `destination`'s resource, with no cooldown in effect — built through
    /// [`FreePool::adopt_observed`], the public entry point that states a
    /// failure count directly rather than deriving one from timed `observe`
    /// calls, so the test needs no assumption about `routing::free`'s
    /// cooldown length.
    fn health_with_failures(destination: &Destination, failures: u32) -> FreePool {
        let mut pool = FreePool::new();
        let resource = FreeResource::new(
            destination.backend().credential().clone(),
            destination.backend().model().label(),
        );
        pool.adopt_observed(&resource, failures, None, None, false);
        pool
    }

    /// Line 1353: keep an *additive* failure penalty, not a boolean one —
    /// two consecutive failures must price worse than one, and the additive
    /// climb must still be bounded at [`HEALTH_PENALTY_FLOOR`] rather than
    /// worsening without limit.
    #[test]
    fn the_failure_penalty_is_additive_and_bounded() {
        let now = Instant::now();
        let dest = destination("PROVIDER_HEALTH_TEST_KEY");

        let weights = ScoreWeights::default();
        let one = provider_health(&dest, &health_with_failures(&dest, 1), now, &weights);
        let two = provider_health(&dest, &health_with_failures(&dest, 2), now, &weights);
        assert!(
            two.magnitude() < one.magnitude(),
            "two consecutive failures ({}) must price worse than one ({}) — \
             an additive penalty, not a boolean",
            two.magnitude(),
            one.magnitude()
        );

        let many = provider_health(&dest, &health_with_failures(&dest, 50), now, &weights);
        assert_eq!(
            many.magnitude(),
            HEALTH_PENALTY_FLOOR,
            "the additive climb is bounded, never worsening without limit"
        );
    }
}

/// Map lines 1517 and 1518 — the two new `hard_constraint` exclusion arms —
/// driven through `SessionRouter::choose`, the real production path, per
/// `GH-CANDIDATE-GEN`'s acceptance tests.
#[cfg(test)]
mod hard_constraint_tests {
    use super::*;
    use crate::config::pairing::WarmSessionState;
    use crate::routing::free::CooldownCause;
    use crate::routing::{AssignedModel, Cost, CredentialId};
    use crate::secret::SecretRef;
    use std::time::Duration;

    fn anthropic_destination(id: &str, credential_var: &str) -> Destination {
        Destination::fresh(
            id,
            IntegrationId::ClaudeCode,
            "profile",
            Backend::new(
                "anthropic",
                "anthropic-messages",
                AssignedModel::named("claude-opus-4-1"),
                CredentialId::new(
                    "anthropic",
                    SecretRef::Environment {
                        var: credential_var.to_owned(),
                    },
                ),
                Cost::Metered,
                ToolSemantics::Verified,
            ),
            None,
        )
    }

    /// A gateway-backed candidate, built the same way `main.rs::destination_backend`
    /// builds one for `BackendResource::GlasshouseGateway` — the provider and
    /// credential name it, never a routing-level type, so this is what "gateway
    /// candidate" means at this layer.
    fn gateway_destination(id: &str) -> Destination {
        Destination::fresh(
            id,
            IntegrationId::ClaudeCode,
            "profile",
            Backend::new(
                "the Glasshouse gateway",
                "anthropic-messages",
                AssignedModel::named("claude-opus-4-1"),
                CredentialId::new(
                    "the Glasshouse gateway",
                    SecretRef::OsCredential {
                        service: "glasshouse-gateway".to_owned(),
                        account: "assigned when the session starts".to_owned(),
                    },
                ),
                Cost::Metered,
                ToolSemantics::Verified,
            ),
            None,
        )
    }

    fn inputs<'a>(
        overrides: &'a pairing::PairingOverrides,
        health: &'a FreePool,
        now: Instant,
        requirements: TaskRequirements,
    ) -> RouterInputs<'a> {
        RouterInputs {
            overrides,
            health,
            now,
            requirements,
        }
    }

    /// Line 1517. A gateway-backed candidate established to lack a required
    /// hard capability is excluded outright, never merely scored; an
    /// unverified axis on the surviving candidate still passes and is priced
    /// by `capability_fit` exactly as before this gate existed. The gateway
    /// candidate also stands as line 1513's capability-half production
    /// evidence: a fresh gateway-backed candidate is filtered by the same
    /// hard-constraint gate as every other backend.
    #[test]
    fn an_established_absent_hard_capability_excludes_and_an_unverified_one_passes() {
        let now = Instant::now();
        let overrides = pairing::PairingOverrides::default();
        let health = FreePool::new();

        let lacking = gateway_destination("gateway-no-shell").with_resource_facts(ResourceFacts {
            shell_tool_use: Declared::verified(false, "test evidence"),
            ..ResourceFacts::UNVERIFIED
        });
        let adequate = anthropic_destination("anthropic-unverified", "CAP_TEST_KEY");

        let requirements = TaskRequirements {
            hard_capabilities: vec![HardCapability::ShellExecution],
            ..TaskRequirements::default()
        };
        let router_inputs = inputs(&overrides, &health, now, requirements);

        let routed = SessionRouter::new()
            .choose(
                RoutingMoment::SessionStart,
                None,
                &[lacking.clone(), adequate.clone()],
                &router_inputs,
            )
            .expect("an adequate destination was offered");

        assert_eq!(
            routed.chosen().id(),
            "anthropic-unverified",
            "an established-absent capability must not win over an adequate destination"
        );
        assert_eq!(routed.rejected().len(), 1);
        assert_eq!(routed.rejected()[0].0.id(), "gateway-no-shell");
        assert_eq!(routed.rejected()[0].1, HardConstraint::Capability);
        assert!(
            routed
                .considered()
                .iter()
                .any(|(d, _)| d.id() == "anthropic-unverified"),
            "an unverified axis must still be scored, not excluded"
        );
    }

    /// Line 1518. A credential the provider refused is excluded, never merely
    /// priced worse.
    #[test]
    fn a_credential_the_provider_rejected_is_excluded() {
        let now = Instant::now();
        let overrides = pairing::PairingOverrides::default();
        let rejected_dest = anthropic_destination("rejected", "REJECTED_TEST_KEY");
        let healthy_dest = anthropic_destination("healthy", "HEALTHY_TEST_KEY");

        let mut health = FreePool::new();
        let resource = FreeResource::new(
            rejected_dest.backend().credential().clone(),
            rejected_dest.backend().model().label(),
        );
        health.adopt_observed(&resource, 0, None, None, true);

        let router_inputs = inputs(&overrides, &health, now, TaskRequirements::default());
        let routed = SessionRouter::new()
            .choose(
                RoutingMoment::SessionStart,
                None,
                &[rejected_dest.clone(), healthy_dest.clone()],
                &router_inputs,
            )
            .expect("a healthy destination was offered");

        assert_eq!(routed.chosen().id(), "healthy");
        assert_eq!(routed.rejected().len(), 1);
        assert_eq!(routed.rejected()[0].0.id(), "rejected");
        assert_eq!(
            routed.rejected()[0].1,
            HardConstraint::ProviderUnavailable {
                credential: rejected_dest.backend().credential().label(),
                cause: ProviderUnavailableCause::CredentialRejected,
            }
        );
        assert!(
            routed.rejected()[0]
                .1
                .reason()
                .expect("a provider-unavailable constraint always carries a reason")
                .contains("refused by its provider"),
            "the refusal reason must be a sentence a person can read"
        );
    }

    /// Line 1518. A cooldown the provider itself declared, still in force at
    /// `inputs.now`, is authoritative per line 1319 and excludes.
    #[test]
    fn a_declared_cooldown_still_in_force_is_excluded() {
        let now = Instant::now();
        let overrides = pairing::PairingOverrides::default();
        let cooling_dest = anthropic_destination("cooling", "DECLARED_TEST_KEY");
        let healthy_dest = anthropic_destination("healthy", "HEALTHY_TEST_KEY_2");

        let mut health = FreePool::new();
        let resource = FreeResource::new(
            cooling_dest.backend().credential().clone(),
            cooling_dest.backend().model().label(),
        );
        health.adopt_observed(
            &resource,
            1,
            Some(now + Duration::from_secs(120)),
            Some(CooldownCause::Declared),
            false,
        );

        let router_inputs = inputs(&overrides, &health, now, TaskRequirements::default());
        let routed = SessionRouter::new()
            .choose(
                RoutingMoment::SessionStart,
                None,
                &[cooling_dest.clone(), healthy_dest.clone()],
                &router_inputs,
            )
            .expect("a healthy destination was offered");

        assert_eq!(routed.chosen().id(), "healthy");
        assert_eq!(routed.rejected().len(), 1);
        assert_eq!(routed.rejected()[0].0.id(), "cooling");
        assert_eq!(
            routed.rejected()[0].1,
            HardConstraint::ProviderUnavailable {
                credential: cooling_dest.backend().credential().label(),
                cause: ProviderUnavailableCause::DeclaredCooldown,
            }
        );
    }

    /// Line 1518's own preservation clause. An *invented* cooldown — line 534's
    /// bounded backoff Glasshouse imposed on itself — is not authoritative,
    /// so it must never exclude and must keep pricing exactly as
    /// `provider_health` did before this gate existed.
    #[test]
    fn an_invented_cooldown_is_priced_softly_and_never_excludes() {
        let now = Instant::now();
        let overrides = pairing::PairingOverrides::default();
        let cooling_dest = anthropic_destination("cooling", "INVENTED_TEST_KEY");
        let other_dest = anthropic_destination("other", "OTHER_TEST_KEY");

        let mut health = FreePool::new();
        let resource = FreeResource::new(
            cooling_dest.backend().credential().clone(),
            cooling_dest.backend().model().label(),
        );
        health.adopt_observed(
            &resource,
            3,
            Some(now + Duration::from_secs(60)),
            Some(CooldownCause::Invented),
            false,
        );

        let router_inputs = inputs(&overrides, &health, now, TaskRequirements::default());
        let routed = SessionRouter::new()
            .choose(
                RoutingMoment::SessionStart,
                None,
                &[cooling_dest.clone(), other_dest.clone()],
                &router_inputs,
            )
            .expect("a destination was offered");

        assert!(
            routed.rejected().is_empty(),
            "an invented cooldown must not exclude — line 534 keeps it probeable by real work"
        );
        assert!(
            routed.considered().iter().any(|(d, _)| d.id() == "cooling"),
            "the cooling destination must still be scored, not excluded"
        );
    }

    /// The gate applies to an existing (warm) session exactly as it does to a
    /// fresh one — a session already running cannot serve either, if its
    /// provider has refused the credential.
    #[test]
    fn an_existing_warm_session_is_excluded_when_its_provider_is_unavailable() {
        let now = Instant::now();
        let overrides = pairing::PairingOverrides::default();
        let warm_backend = Backend::new(
            "anthropic",
            "anthropic-messages",
            AssignedModel::named("claude-opus-4-1"),
            CredentialId::new(
                "anthropic",
                SecretRef::Environment {
                    var: "WARM_REJECTED_KEY".to_owned(),
                },
            ),
            Cost::Metered,
            ToolSemantics::Verified,
        );
        let warm_dest = Destination::existing(
            "warm",
            IntegrationId::ClaudeCode,
            "profile",
            warm_backend,
            WarmSession {
                state: WarmSessionState::Live,
                idle_seconds: 0,
            },
        );
        let fresh_dest = anthropic_destination("fresh", "FRESH_KEY");

        let mut health = FreePool::new();
        let resource = FreeResource::new(
            warm_dest.backend().credential().clone(),
            warm_dest.backend().model().label(),
        );
        health.adopt_observed(&resource, 0, None, None, true);

        let router_inputs = inputs(&overrides, &health, now, TaskRequirements::default());
        let routed = SessionRouter::new()
            .choose(
                RoutingMoment::SessionStart,
                Some(&warm_dest),
                &[warm_dest.clone(), fresh_dest.clone()],
                &router_inputs,
            )
            .expect("a fresh destination was offered");

        assert_eq!(
            routed.chosen().id(),
            "fresh",
            "an existing session must not be favoured over the gate that excludes its unavailable provider"
        );
        assert_eq!(routed.rejected().len(), 1);
        assert_eq!(routed.rejected()[0].0.id(), "warm");
    }

    /// With no candidate either new arm would touch, the gate excludes
    /// nothing extra: both candidates are still scored, destination order is
    /// still the tiebreaker, and no "rejected" section renders — the ranking
    /// and explanation this package must not disturb.
    #[test]
    fn a_candidate_set_with_no_excluded_candidate_ranks_exactly_as_before_this_gate() {
        let now = Instant::now();
        let overrides = pairing::PairingOverrides::default();
        let health = FreePool::new();
        let first = anthropic_destination("first", "INERT_TEST_KEY_1");
        let second = anthropic_destination("second", "INERT_TEST_KEY_2");

        let router_inputs = inputs(&overrides, &health, now, TaskRequirements::default());
        let routed = SessionRouter::new()
            .choose(
                RoutingMoment::SessionStart,
                None,
                &[first.clone(), second.clone()],
                &router_inputs,
            )
            .expect("two destinations were offered");

        assert!(
            routed.rejected().is_empty(),
            "neither candidate should be excluded by the new gate arms"
        );
        assert_eq!(
            routed.considered().len(),
            2,
            "both candidates must still be scored"
        );
        assert_eq!(
            routed.chosen().id(),
            "first",
            "with every term tied, destination order is still the tiebreaker"
        );
        assert!(
            !routed.render_overview().contains("rejected"),
            "no rejected section renders when nothing is excluded"
        );
    }
}

#[cfg(test)]
mod pairing_prior_tests {
    use super::*;
    use crate::config::pairing::WarmSessionState;
    use crate::routing::{AssignedModel, Cost, CredentialId};
    use crate::secret::SecretRef;

    /// `claude-fable-5` under Claude Code is `PairingClass::VendorNative`
    /// (`crate::harness::pairing::tests::a_vendor_native_pairing_needs_the_family_and_the_developer`).
    /// `gpt-5.5` under Claude Code is not — attributed to a different vendor
    /// than Claude Code's own, so it never satisfies the family-and-developer
    /// check regardless of route (the comment on
    /// `crate::harness::pairing::tests::a_harness_speaking_anthropic_messages_on_a_chat_only_route_is_translated`).
    /// Both share the same wire protocol Claude Code itself speaks, so the
    /// only axis a fresh pair built from these two ever varies on is the one
    /// this package adds.
    const NATIVE_MODEL: &str = "claude-fable-5";
    const OTHER_MODEL: &str = "gpt-5.5";

    fn backend(model: &str, credential_var: &str) -> Backend {
        Backend::new(
            "anthropic",
            "anthropic-messages",
            AssignedModel::named(model),
            CredentialId::new(
                "anthropic",
                SecretRef::Environment {
                    var: credential_var.to_owned(),
                },
            ),
            Cost::Metered,
            ToolSemantics::Verified,
        )
    }

    fn fresh(id: &str, model: &str, credential_var: &str) -> Destination {
        Destination::fresh(
            id,
            IntegrationId::ClaudeCode,
            "profile",
            backend(model, credential_var),
            None,
        )
    }

    fn warm(id: &str, model: &str, credential_var: &str, idle_seconds: i64) -> Destination {
        Destination::existing(
            id,
            IntegrationId::ClaudeCode,
            "profile",
            backend(model, credential_var),
            WarmSession {
                state: WarmSessionState::Live,
                idle_seconds,
            },
        )
    }

    fn inputs<'a>(
        overrides: &'a pairing::PairingOverrides,
        health: &'a FreePool,
        now: Instant,
    ) -> RouterInputs<'a> {
        RouterInputs {
            overrides,
            health,
            now,
            requirements: TaskRequirements::default(),
        }
    }

    fn term(explanation: &RoutingExplanation) -> &Contribution {
        explanation
            .contributions()
            .iter()
            .find(|c| c.name() == "pairing prior")
            .expect("every scored destination's explanation must carry the pairing prior term")
    }

    /// 566, 1540: two fresh, cold, equally healthy destinations of one
    /// harness, differing only in `PairingClass`. Listed non-native-first so
    /// a stable tie-break cannot be mistaken for the term actually
    /// separating them — if [`PAIRING_PRIOR`] were zeroed, the first-listed
    /// candidate would win regardless, and this assertion would catch it.
    #[test]
    fn a_tied_pair_differing_only_in_vendor_native_class_is_won_by_the_native_one() {
        let now = Instant::now();
        let overrides = pairing::PairingOverrides::default();
        let health = FreePool::new();
        let inputs = inputs(&overrides, &health, now);

        let other = fresh("other", OTHER_MODEL, "PAIRING_PRIOR_TEST_A");
        let native = fresh("native", NATIVE_MODEL, "PAIRING_PRIOR_TEST_B");

        let routed = SessionRouter::new()
            .choose(RoutingMoment::SessionStart, None, &[other, native], &inputs)
            .expect("two eligible fresh destinations must produce a decision");

        assert!(
            routed.rejected().is_empty(),
            "no candidate is ever refused on this axis: {:?}",
            routed.rejected()
        );
        assert_eq!(
            routed.chosen().id(),
            "native",
            "the vendor-native pairing must win the tie"
        );

        let winner_term = term(routed.explanation());
        assert!(
            winner_term.magnitude() > 0.0,
            "the native pairing's prior must be positive: {}",
            winner_term.magnitude()
        );
        assert!(
            winner_term.evidence().contains("vendor-native")
                && winner_term.evidence().contains("starting assumption"),
            "the explanation must name the class and call it a starting assumption, not a \
             quality claim: {}",
            winner_term.evidence()
        );

        let (_, loser_explanation) = routed
            .considered()
            .iter()
            .find(|(d, _)| d.id() == "other")
            .expect("the non-native candidate must still be considered, never rejected");
        let loser_term = term(loser_explanation);
        assert_eq!(
            loser_term.magnitude(),
            0.0,
            "a non-native pairing contributes nothing"
        );
        assert!(
            loser_term
                .evidence()
                .contains("inert: not a vendor-native pairing"),
            "a non-native pairing's explanation must say so plainly: {}",
            loser_term.evidence()
        );
    }

    /// 569, killed directly rather than through a set that also prices
    /// bootstrap cost and a hot prompt cache (a fresh-vs-existing comparison
    /// would still choose the warm side even with a mutated, oversized
    /// prior, which would make that test a weak witness for this line —
    /// practice §41). This is the dedicated killer, isolating exactly the
    /// weight the packet names: [`PAIRING_PRIOR`] must stay strictly below
    /// the `warmth` facet's own ceiling — a live warm session at zero idle,
    /// worth `1.5` (this module's own header comment, and
    /// [`AffinityBreakdown::warmth`]) — never the full breakdown total,
    /// which other facets such as a hot prompt cache also add to.
    #[test]
    fn pairing_prior_stays_below_a_live_warm_sessions_own_warmth_facet() {
        let warm_dest = warm("warm", OTHER_MODEL, "PAIRING_PRIOR_TEST_C", 0);
        let breakdown = affinity_breakdown(&warm_dest, None, &TaskRequirements::default())
            .expect("an existing destination always has a breakdown");
        assert!(
            PAIRING_PRIOR < breakdown.warmth.magnitude(),
            "the pairing prior ({PAIRING_PRIOR}) must stay strictly below a live warm \
             session's own warmth facet ({}), or it could outrank one",
            breakdown.warmth.magnitude()
        );
    }

    /// 569's behavioural half: the same tied pair as the first test, except
    /// the non-native candidate is now a relevant warm existing session
    /// instead of a fresh one. The warm side must win.
    #[test]
    fn a_relevant_warm_session_outweighs_the_native_pairing_prior() {
        let now = Instant::now();
        let overrides = pairing::PairingOverrides::default();
        let health = FreePool::new();
        let inputs = inputs(&overrides, &health, now);

        let native = fresh("native", NATIVE_MODEL, "PAIRING_PRIOR_TEST_D");
        let warm_other = warm("other", OTHER_MODEL, "PAIRING_PRIOR_TEST_E", 0);

        let routed = SessionRouter::new()
            .choose(
                RoutingMoment::SessionStart,
                None,
                &[native, warm_other],
                &inputs,
            )
            .expect("two eligible candidates must produce a decision");

        assert!(routed.rejected().is_empty());
        assert_eq!(
            routed.chosen().id(),
            "other",
            "a relevant warm session must outweigh the native pairing's starting prior"
        );
    }

    /// 1923, 1541: the same tied pair, except the native candidate has
    /// accumulated at least [`PAIRING_PRIOR_EVIDENCE_THRESHOLD`] local
    /// observations. Its own `pairing prior` term must read `0.0` with text
    /// saying observed evidence replaced the starting prior — the direct
    /// killer for "remove the evidence decay (always apply the prior)": with
    /// that mutation this assertion reads [`PAIRING_PRIOR`], not `0.0`.
    #[test]
    fn accumulated_local_evidence_decays_the_prior_to_zero() {
        let now = Instant::now();
        let overrides = pairing::PairingOverrides::default();
        let health = FreePool::new();
        let inputs = inputs(&overrides, &health, now);

        let other = fresh("other", OTHER_MODEL, "PAIRING_PRIOR_TEST_F");
        let seasoned_native = fresh("native", NATIVE_MODEL, "PAIRING_PRIOR_TEST_G")
            .with_pairing_prior_evidence(PAIRING_PRIOR_EVIDENCE_THRESHOLD);

        let routed = SessionRouter::new()
            .choose(
                RoutingMoment::SessionStart,
                None,
                &[other, seasoned_native],
                &inputs,
            )
            .expect("two eligible fresh destinations must produce a decision");

        assert!(routed.rejected().is_empty());

        let (_, native_explanation) = routed
            .considered()
            .iter()
            .find(|(d, _)| d.id() == "native")
            .expect("the seasoned native candidate must still be considered");
        let decayed = term(native_explanation);
        assert_eq!(
            decayed.magnitude(),
            0.0,
            "accumulated local evidence must decay the prior to zero"
        );
        assert!(
            decayed
                .evidence()
                .contains("observed evidence has replaced the starting prior"),
            "the explanation must say evidence replaced the prior: {}",
            decayed.evidence()
        );
    }

    /// 1923's "user choice": a `RoutingOverride` naming the non-native
    /// destination wins even though the native one's prior would otherwise
    /// carry the tie (it is listed first too, so an unhonoured override
    /// would still pick it on both counts). The override is asserted here,
    /// never rebuilt from the prior's own logic.
    #[test]
    fn a_user_override_naming_the_non_native_destination_wins() {
        let now = Instant::now();
        let overrides = pairing::PairingOverrides::default();
        let health = FreePool::new();
        let inputs = inputs(&overrides, &health, now);

        let native = fresh("native", NATIVE_MODEL, "PAIRING_PRIOR_TEST_H");
        let other = fresh("other", OTHER_MODEL, "PAIRING_PRIOR_TEST_I");

        let routed = SessionRouter::with_override(RoutingOverride::to("other"))
            .choose(RoutingMoment::SessionStart, None, &[native, other], &inputs)
            .expect("an override naming an eligible destination must produce a decision");

        assert!(routed.rejected().is_empty());
        assert!(
            routed.override_refused().is_none(),
            "the override must be honoured: {:?}",
            routed.override_refused()
        );
        assert_eq!(
            routed.chosen().id(),
            "other",
            "the user's own override must win over the native pairing's prior"
        );
    }

    /// The map's own "ranks byte-for-byte" requirement: a candidate set with
    /// no vendor-native member gets a `pairing prior` term of exactly `0.0`
    /// on every candidate, so the total this term adds to is unchanged from
    /// what the ranking summed to before this package existed.
    #[test]
    fn a_set_with_no_vendor_native_member_adds_nothing_to_the_ranking() {
        let now = Instant::now();
        let overrides = pairing::PairingOverrides::default();
        let health = FreePool::new();
        let inputs = inputs(&overrides, &health, now);

        let a = fresh("a", OTHER_MODEL, "PAIRING_PRIOR_TEST_J");
        let b = fresh("b", OTHER_MODEL, "PAIRING_PRIOR_TEST_K");

        let routed = SessionRouter::new()
            .choose(RoutingMoment::SessionStart, None, &[a, b], &inputs)
            .expect("two eligible fresh destinations must produce a decision");

        assert!(routed.rejected().is_empty());
        for (destination, explanation) in routed.considered() {
            let t = term(explanation);
            assert_eq!(
                t.magnitude(),
                0.0,
                "`{}` is not vendor-native, so the term must contribute nothing to its total",
                destination.id()
            );
        }
    }
}
