//! Which decision, and how it turned out — the kind and outcome vocabularies
//! `evaluation_observations` stores, split out of `evaluation/mod.rs` by
//! Phase 59's decomposition. Values are unchanged from before the move.

/// What was decided — the `evaluation_observations.kind` vocabulary, in Rust
/// because migration 15 deliberately gives that column no SQL `CHECK`.
///
/// The store encodes through an exhaustive `match`, so a new variant is a
/// compile error at the writer rather than a constraint violation on whatever
/// thread happens to be recording. `database::EVALUATION_KINDS` is
/// the constant a test pins this against, for the same reason
/// `LIFECYCLE_EVENT_KINDS` exists beside its own `CHECK`.
///
/// **One variant per landed producer.** Variants are added as producers land,
/// never in advance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EvaluationKind {
    /// A memory search returned this memory to a caller — `memory_id` names
    /// which one, and [`RetrievalScope`] is the `subject`.
    ///
    /// One row per *returned memory*, not per search: a search that returned
    /// nothing records nothing, which is why this ledger counts retrieved
    /// memories rather than retrievals.
    MemoryRetrieved,
    /// A memory search on a production door matched nothing at all — the
    /// miss counterpart of [`Self::MemoryRetrieved`], and map line 1865's own
    /// measurement: *"do not add vector retrieval until FTS5 retrieval
    /// failures are observed and recorded in real projects."* `subject` is
    /// the [`RetrievalScope`] the search asked with; there is no `memory_id`,
    /// because a miss names no memory.
    ///
    /// **One row per zero-result search, not one row per query.** A search
    /// that matched something writes [`Self::MemoryRetrieved`] rows and no
    /// miss row; the two are mutually exclusive at every door.
    MemoryRetrievalMiss,
    /// Glasshouse routed one of its own bounded support jobs — memory
    /// extraction today — and this is the rationale it decided on. `subject`
    /// is the [`crate::routing::disposable::JobKind`]'s own name and `detail`
    /// is the rendered explanation, verbatim.
    ///
    /// **Decided, not chosen.** A run where no resource could serve is a
    /// decision with a reason too, and it is the one a reader most wants to
    /// see; a kind named for the success case would have had nowhere to put
    /// it.
    ///
    /// **Text, and deliberately not a structured route.**
    /// [`crate::routing::disposable::DisposableChoice`]'s fields are private
    /// and nothing outside that module constructs one — its module header
    /// records that as an enforced safety invariant, because a choice on a
    /// metered resource must not be reproducible from a policy that withheld
    /// it. So this ledger stores what was decided as the sentence production
    /// already renders, and a reader renders that sentence rather than
    /// rebuilding the decision.
    DisposableRouteDecided,
    /// A launch's session-boundary routing decided whether the automatic
    /// ranking stood or a user override changed it — map line 1829.
    /// `subject` is `"automatic"` or `"overridden"`; `detail` is, only when
    /// overridden, the destination id the ranking would have chosen instead
    /// (`crate::routing::session::Routed::overrode`).
    ///
    /// **Recorded every launch that reached a routing decision, not only the
    /// overridden ones.** Unlike [`Self::MemoryRetrieved`], omitting the
    /// non-event here would leave no way to tell "never overridden" from
    /// "never launched", and line 1829 asks about the former.
    RoutingOverrideDecided,
    /// The same launch's decision on the other axis: whether the chosen
    /// destination continues a warm session or starts fresh — map line 1830.
    /// `subject` is `"existing"` or `"fresh"`
    /// (`crate::routing::session::Destination::is_fresh`); `detail` is the
    /// chosen destination's id.
    RoutingContinuationDecided,
    /// The cost class of the destination a launch actually routed to,
    /// attributed to **the session that launch produced** — map line 1835.
    /// `subject` is `"free"`, `"metered"` or `"unknown"`
    /// ([`crate::routing::Cost::as_str`], plus the third state this ledger
    /// adds); `detail` is the chosen destination's id; `session_id` is the
    /// session, which is what makes an outcome attachable to it later.
    ///
    /// A third row rather than a rewrite of the two above:
    /// [`crate::evaluation::record_routing_decision`] runs before a fresh
    /// launch has minted a session id, so its `session_id` is absent on
    /// purpose, and the decision keeps its own moment while this row records
    /// the session it turned into.
    ///
    /// `unknown` is a real answer, not a gap: a destination on a harness's
    /// own sign-in has no configured provider and no marked model, and a
    /// reader that folded that into `metered` would report a number nobody
    /// measured.
    ///
    /// History: design-decisions.md, "Trims: config, checkpoint, evaluation and codex module docs", kinds.rs `EvaluationKind::RoutingCostClassObserved`.
    RoutingCostClassObserved,
    /// How much observed evidence the router actually held about the
    /// destination it chose, at the moment it chose it — map line 1854's
    /// *sparse* half and nothing else. `subject` is `"observed"` or
    /// `"absent"`; `detail` is the chosen destination's id.
    ///
    /// **Two states, and neither is a confidence.**
    /// [`crate::routing::evidence::AggregateReading::confidence`] belongs to
    /// the gateway's own aggregate ledger, which the session router never
    /// reads: `RouterInputs` carries a [`crate::routing::free::FreePool`],
    /// and that pool's health entries carry no confidence and no timestamp.
    /// So what can be said honestly at a launch is whether the pool held a
    /// health reading for the chosen destination at all. *Stale* and
    /// *incorrectly segmented* — line 1854's other two — have no producer on
    /// this path and are not guessed at.
    RoutingEvidenceObserved,
    /// The harness's own verdict on one turn of a routed session — the
    /// outcome half of map lines 1834, 1835, 1845 and 1854. `subject` is
    /// `"completed"` or `"failed"`, from
    /// [`crate::events::TurnOutcome`]; `detail` is the destination id the
    /// decision chose, copied from this session's
    /// [`Self::RoutingCostClassObserved`] row.
    ///
    /// **Written only from a harness's own `TurnEnded`**, which
    /// `crate::session::lifecycle::event_for` is the single construction site
    /// for. A process that exited, output that went quiet, and a user who
    /// walked away all record nothing, and a decision whose session never
    /// reports a turn end is counted as *unknown* by every reader here —
    /// never as a failure and never as a success.
    RoutingOutcomeObserved,
    /// The workload tier the launch-path classifier decided this session's
    /// work needed, and whether line 1459's conservative rule moved it —
    /// **map line 1834**. `subject` is [`RoutingTier::as_str`]'s closed
    /// vocabulary (the tier, with `-escalated` when the tier the decision
    /// used is not the tier the classifier stated, plus `unclassified`);
    /// `detail` is the tier the classifier itself stated, absent for a
    /// launch that stated no task; `session_id` is the session the launch
    /// produced.
    ///
    /// A launch with no `--task` records `unclassified`, never nothing —
    /// the bucket is its own and is never folded into a tier. The tier and
    /// the escalation are one bucket rather than two columns, because line
    /// 1834 asks about the pair: does a tier predict a successful turn
    /// **without** escalation?
    ///
    /// History: design-decisions.md, "Trims: config, checkpoint, evaluation and codex module docs", kinds.rs `EvaluationKind::RoutingTierObserved`.
    RoutingTierObserved,
    /// Whether the failure-domain term changed which candidate a gateway
    /// failover chose — **map line 1851**. `subject` is
    /// [`FailoverPrevention::as_str`]'s two words; `detail` is the label of
    /// the candidate the term displaced, present only when one was.
    ///
    /// **Derived from one comparison, never from a rejection.** Design
    /// decision 1 makes failure-domain diversity additive — a `-1.0`
    /// contribution, never a filter — so nothing in production *decides* a
    /// prevention. What can be established is whether the ranking's winner
    /// differs from the winner of the same ranking with that one term
    /// removed, and a difference is only ever possible when the displaced
    /// candidate shared the failed backend's provider. That is exactly the
    /// map line's *"failover onto the same unhealthy upstream"*, and it is
    /// observed rather than caused.
    ///
    /// No `session_id`: the gateway that ranks a failover is serving one
    /// session but holds no Glasshouse session id, and inventing an
    /// attribution would be worse than a count that honestly has none. The
    /// rendered figure is a ratio over failovers, which needs no session.
    FailoverPrevented,
    /// A person's or an agent's own verdict on a memory Glasshouse retrieved
    /// — `glasshouse memory rate <memory-id> <verdict>` — map lines 1821,
    /// 1823, 1824, 1825, 1831 and **939**'s explicit half. `subject` carries
    /// the [`RetrievalScope`] word of the retrieval this rating judges, or is
    /// absent when the memory was never retrieved; `outcome` carries the
    /// verdict word itself ([`EvaluationOutcome`]'s eight
    /// non-[`EvaluationOutcome::Unknown`] values), `memory_id` is the rated
    /// memory, `session_id` is the session the rating is about when one was
    /// given, and `detail` is the operator's own note, never parsed.
    ///
    /// This is the explicit half of "explicit rating when given, a labelled
    /// proxy otherwise" (design decision, Phase 51 / RC-B, user ruling
    /// 2026-09-02). A rating is a new row, never an edit — it judges a
    /// [`Self::MemoryRetrieved`] row without touching it, the same
    /// append-only shape every kind in this ledger keeps.
    ///
    /// History: design-decisions.md, "Trims: config, checkpoint, evaluation and codex module docs", kinds.rs `EvaluationKind::MemoryRated`.
    MemoryRated,
    /// `glasshouse memory revalidate <id> <outcome>` happened — map line
    /// 1824's own denominator. `subject` is the outcome word verbatim
    /// (`reaffirmed`, `needs-review`, `superseded` or `invalidated`);
    /// `memory_id` is the revalidated memory; `outcome` stays
    /// [`EvaluationOutcome::Unknown`], because this row is not a verdict on
    /// whether the revalidation was *correct* — [`Self::MemoryRated`]'s
    /// `revalidation-correct`/`revalidation-wrong` words already carry that
    /// judgement. This row only answers *"did a revalidation happen"*.
    ///
    /// **Its own row, not a reuse of an existing column.** `main.rs::memory_revalidate`'s
    /// four outcomes write to different places in `memories` —
    /// `last_validated_at`, `review_marked_at` (shared with `memory
    /// challenge`, so it cannot double as this line's denominator without
    /// conflating the two — see [`Self::MemoryRated`]'s challenge doc), and
    /// two outcomes with no distinguishing column at all — so no single
    /// production column ever meant "a revalidation happened" until this one.
    MemoryRevalidated,
    /// The harness's own verdict on one turn of **any** session that runs
    /// the hook — map lines 1821 and 1831's proxy denominator; not
    /// [`Self::RoutingOutcomeObserved`], which refuses to write for a
    /// session with no routed destination. `subject` is `"completed"` or
    /// `"failed"`, spelled exactly as [`Self::RoutingOutcomeObserved`]'s own
    /// vocabulary — the same [`crate::events::TurnOutcome`].
    ///
    /// Written for every session that reaches the hook's `TurnEnded` arm,
    /// routed or not: a door-spawned session that was never routed gets
    /// this row and never a `RoutingOutcomeObserved` one; a CLI-launched
    /// session gets both. The memory-quality readers (1821, 1831) join a
    /// session-attributed retrieval to this row rather than to the routing
    /// row, because the proxy's definition is about the *session's* turn,
    /// not the *route's*.
    ///
    /// History: design-decisions.md, "Trims: config, checkpoint, evaluation and codex module docs", kinds.rs `EvaluationKind::TurnOutcomeObserved`.
    TurnOutcomeObserved,
    /// Why a launch's session-boundary routing chose the destination it did
    /// — map lines 1757 and 1766, design decision *"The session router's
    /// rationale row"*. `subject` is the chosen destination id; `detail` is
    /// the winning [`crate::routing::RoutingExplanation`]'s contributions as
    /// a compact JSON array of `{name, magnitude, evidence}`, in the
    /// explanation's own order — structured, not rendered text, because
    /// 1766 ranks by magnitude and a rendered string cannot be ranked.
    ///
    /// **Recorded beside [`Self::RoutingCostClassObserved`] and
    /// [`Self::RoutingEvidenceObserved`], at the same instant with the same
    /// `session_id`.** It records the decision the launch actually made,
    /// never a recomputed explanation — the batch-50 refusal's own words,
    /// *"the factors of a decision that was never made."* An explanation
    /// with no contributions still writes a row, `detail` `"[]"`: the
    /// decision happened even when nothing weighed in.
    SessionRouteDecided,
    /// The launch-path routing decision's own expected output-token size for
    /// this session's task class — map line 1855's token half, and
    /// [`crate::routing::evidence::EvidenceLedger::output_estimate_accuracy`]'s
    /// own row. `subject` is the task class word
    /// ([`crate::routing::request::TaskClass::as_str`]); `detail` is the
    /// median output-token count [`crate::routing::burn::output_tokens_by_class`]
    /// held for that class at the moment of launch, as decimal text;
    /// `session_id` is the session the decision produced.
    ///
    /// **Written only when there is a real median to write.** A launch whose
    /// task class has no comparable rows in the window — the common case for
    /// a class this project has not routed before — records nothing at all
    /// rather than a fabricated zero; see
    /// [`crate::evaluation::record_routing_consumption_estimate`]'s own doc comment.
    RoutingConsumptionEstimated,
    /// Whether protected quota remained available for a task the reserve
    /// exists to protect — map line 1837, design decision *"Protected
    /// quota's availability is recorded when a high-tier task is routed, and
    /// read back as a rate"* (2026-09-05). Decided at the moment a launch is
    /// routed, from two facts the router already holds: the task's workload
    /// tier ([`RoutingTier`], the same value [`Self::RoutingTierObserved`]
    /// records) and the chosen destination's capacity band
    /// (`Destination::capacity_facts().band()`).
    ///
    /// **Written only when the tier is `Heavy` or `Frontier`** — the tiers
    /// the reserve exists to protect; a `Standard`, `Leaf`, `Deterministic`
    /// or unclassified launch writes nothing, because *needed* is the line's
    /// own word. `subject` is the band the router read, in
    /// [`crate::provider::quota::CapacityBand`]'s own spelling, or
    /// `"unknown"` when the destination carried no reading; `detail` is the
    /// tier word; `session_id` is the launched session's.
    ReserveAvailabilityObserved,
    /// An operator's or agent's own verdict on a session's route —
    /// `glasshouse rate-route <session-id> useful|not-useful` — the explicit
    /// half of map line 1846's own design note, *"The routing half of RC-B:
    /// an explicit route rating when given, the turn-outcome proxy
    /// otherwise"* (2026-09-05). `subject` is the destination id the session
    /// was routed to (the same word [`Self::RoutingCostClassObserved`]'s
    /// `detail` carries); `outcome` is [`EvaluationOutcome::Useful`] or
    /// [`EvaluationOutcome::NotUseful`] only — [`ROUTE_RATING_VERDICTS`]'
    /// closed two-word vocabulary, reusing [`EvaluationKind::MemoryRated`]'s
    /// own words rather than inventing a second scale for the same question;
    /// `session_id` is required — a route rating is about a session's route,
    /// never a memory; `detail` is the operator's own note, never parsed.
    ///
    /// **A rating is a new row, never an edit** — the same append-only shape
    /// [`Self::MemoryRated`] keeps, and never a rewrite of
    /// [`Self::RoutingOutcomeObserved`]. **Replaces the proxy, never sums
    /// with it**: every reader that counts a session's route as a success or
    /// a failure from [`Self::RoutingOutcomeObserved`] substitutes this row's
    /// verdict for that session instead, and prints the two counts apart. A
    /// session with two ratings takes the latest.
    RoutingRated,
    /// How one hook-triggered memory extraction ended — dogfooding 2026-09-06
    /// finding 4: extraction routed to a disposable resource and then nothing
    /// durable said whether the model answered, timed out at its bound, or
    /// returned nothing worth storing; the hook's one stderr line (the binary
    /// crate's `commands::memory_extraction::lost_extraction_notice`) is the
    /// only trace today and the harness swallows it. `subject` is the
    /// [`crate::memory::extract::ExtractionTrigger::as_str`] word; `detail`
    /// is one line built by [`crate::evaluation::record_memory_extraction`]
    /// from the model description, the outcome's own counts or its failure's
    /// fixed `Display` phrase, or — when extraction never produced an outcome
    /// at all — which of preparation failing or the bound expiring it was,
    /// plus the elapsed milliseconds.
    ///
    /// **One row per hook-triggered extraction, whatever it did — including a
    /// `NothingToExtract` failure.** The hook's stderr notice stays silent
    /// for that one case on purpose (a warning on every empty compaction
    /// teaches people to ignore it), but this ledger is not the notice: a
    /// reader here must be able to tell "nothing to extract" from "extraction
    /// never ran". Never written for `glasshouse memory commit`
    /// (`ExtractionTrigger::Manual` prints its own report in front of a
    /// person watching; this row is for the triggers nobody is watching).
    ///
    /// **No memory body, activity line, provider response body or credential
    /// value ever reaches `detail`.** Only counts (`.len()`), a fixed failure
    /// phrase, the model's own rendered description, and a duration do.
    MemoryExtractionObserved,
}

/// The `subject` this ledger writes for a destination whose cost class no
/// production fact states — [`EvaluationKind::RoutingCostClassObserved`]'s
/// third value, beside [`crate::routing::Cost`]'s two.
pub const UNKNOWN_COST_CLASS: &str = "unknown";

/// How old a gateway-health reading may be before the launch path calls it
/// stale — [`RoutingEvidence::from_observation`]'s horizon, and map line
/// 1854's *stale* word made into a number that can be argued with.
///
/// **Fifteen minutes.** A persisted reading is written by
/// `crate::gateway::session::SessionRouting::health_readings_for` on every
/// exchange a gateway serves, so a project being worked in refreshes it
/// continuously and nothing near this bound is reached. The bound is for the
/// other case — the last gateway ran this morning and the router is about to
/// weigh what it left behind — and it is set at the scale of
/// `crate::routing::free`'s own cooldowns: past a quarter hour, a resource
/// that was cooling down has long since come back and a resource that was
/// healthy has had time to stop being, so the reading no longer describes
/// the thing the router is about to choose.
///
/// It is a horizon, not a filter. A stale reading is still adopted into the
/// pool and still ranked on — this constant only decides what the ledger
/// *calls* the evidence a decision was made with, which is exactly what line
/// 1854 asks to be measured.
pub const HEALTH_EVIDENCE_HORIZON_SECONDS: i64 = 15 * 60;

/// How much observed health evidence the router held about the destination it
/// chose — the `subject` vocabulary for
/// [`EvaluationKind::RoutingEvidenceObserved`].
///
/// **Three states now, where there were two.** The `observed` half of line
/// 1854 has been split by [`HEALTH_EVIDENCE_HORIZON_SECONDS`] into fresh and
/// stale, which is the line's second word. Rows written by earlier builds
/// carry the old `observed` and are **not** re-labelled: nothing decodes this
/// column back into this type, every reader groups on the stored string, so
/// an old row appears in its own `observed` bucket and a reader can see that
/// this project's history has both vocabularies in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RoutingEvidence {
    /// The pool held a reading for exactly this destination's credential and
    /// model, and it was written within [`HEALTH_EVIDENCE_HORIZON_SECONDS`]
    /// of this decision.
    ObservedFresh,
    /// The pool held such a reading and it is older than the horizon. The
    /// ranking still used it; this records that what it used had aged.
    ObservedStale,
    /// It held none, or held one that could not be dated. The ranking still
    /// happened; it happened without any usable observation of this
    /// destination's recent behaviour.
    ///
    /// **A reading whose age is unknown is `absent`, never fresh.** A cache
    /// file this build cannot date says nothing about when it was written,
    /// and reading "no timestamp" as "just now" would be the one direction
    /// that turns a missing fact into a favourable one.
    Absent,
}

impl RoutingEvidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ObservedFresh => "observed-fresh",
            Self::ObservedStale => "observed-stale",
            Self::Absent => "absent",
        }
    }

    /// From the two facts a caller on the launch path can establish: whether
    /// the pool it handed the router carried this destination at all, and —
    /// when it did — the unix second the reading it carried was written.
    ///
    /// `None` covers both *"no reading"* and *"a reading nothing could
    /// date"*, and both answer [`Self::Absent`] for that variant's own
    /// reason. A reading stamped in this launch's own future (two clocks, or
    /// one that moved) is fresh rather than negative-aged: the horizon is a
    /// bound on staleness, and nothing here invents a verdict from a skew it
    /// cannot explain.
    pub fn from_observation(observed_at_unix: Option<i64>, now_unix: i64) -> Self {
        match observed_at_unix {
            None => Self::Absent,
            Some(observed_at)
                if now_unix.saturating_sub(observed_at) > HEALTH_EVIDENCE_HORIZON_SECONDS =>
            {
                Self::ObservedStale
            }
            Some(_) => Self::ObservedFresh,
        }
    }
}

/// The workload tier a launch's routing decision used, together with whether
/// line 1459's conservative rule moved it — the `subject` vocabulary for
/// [`EvaluationKind::RoutingTierObserved`], and **map line 1834**'s bucket.
///
/// One closed list of eleven words rather than a composed label: the pair is
/// the thing line 1834 asks about, and a `subject` built by concatenating two
/// columns at the reader would be a derived label rather than a vocabulary
/// word (see [`crate::evaluation::RouteOutcomeCounts::bucket`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingTier {
    /// The launch stated a task, and this is the tier the decision used.
    ///
    /// `escalated` is **whether the tier actually moved** —
    /// `RouterAnswer::required_tier() != RouterAnswer::stated_tier()` — and
    /// not merely whether the conservative rule fired. The two differ at the
    /// top of the scale, where `WorkloadTier::escalate` is a fixed point: a
    /// low-confidence `frontier` classification runs the rule and comes back
    /// with the tier the classifier already stated, and calling that row
    /// *escalated* would put a tier nobody changed on the escalated side of
    /// the very comparison line 1834 exists to make.
    Classified {
        tier: crate::routing::classify::WorkloadTier,
        escalated: bool,
    },
    /// The launch stated no task, so nothing classified it. Its own bucket,
    /// never a tier and never nothing.
    Unclassified,
}

impl RoutingTier {
    pub fn as_str(self) -> &'static str {
        use crate::routing::classify::WorkloadTier;
        match self {
            Self::Unclassified => "unclassified",
            Self::Classified { tier, escalated } => match (tier, escalated) {
                (WorkloadTier::Deterministic, false) => "deterministic",
                (WorkloadTier::Deterministic, true) => "deterministic-escalated",
                (WorkloadTier::Leaf, false) => "leaf",
                (WorkloadTier::Leaf, true) => "leaf-escalated",
                (WorkloadTier::Standard, false) => "standard",
                (WorkloadTier::Standard, true) => "standard-escalated",
                (WorkloadTier::Heavy, false) => "heavy",
                (WorkloadTier::Heavy, true) => "heavy-escalated",
                (WorkloadTier::Frontier, false) => "frontier",
                (WorkloadTier::Frontier, true) => "frontier-escalated",
            },
        }
    }

    /// The tier the classifier itself stated, for the row's `detail` — absent
    /// for [`Self::Unclassified`], where no classifier ran and there is
    /// nothing to state.
    pub fn stated_tier(self) -> Option<crate::routing::classify::WorkloadTier> {
        match self {
            Self::Unclassified => None,
            Self::Classified { tier, escalated } => Some(if escalated {
                // The decision's tier is one step above what was stated, and
                // `escalate` is the only step this build takes.
                unescalate(tier)
            } else {
                tier
            }),
        }
    }
}

/// The inverse of `WorkloadTier::escalate`, for the one place a stated tier
/// has to be recovered from an escalated one.
///
/// Total because [`RoutingTier::Classified`] only ever carries `escalated`
/// when the tier genuinely moved, and `escalate` moves each tier exactly one
/// step: the bottom of the scale is never an escalation's *result*, so it
/// answers itself rather than being made unrepresentable.
fn unescalate(
    tier: crate::routing::classify::WorkloadTier,
) -> crate::routing::classify::WorkloadTier {
    use crate::routing::classify::WorkloadTier;
    match tier {
        WorkloadTier::Deterministic => WorkloadTier::Deterministic,
        WorkloadTier::Leaf => WorkloadTier::Deterministic,
        WorkloadTier::Standard => WorkloadTier::Leaf,
        WorkloadTier::Heavy => WorkloadTier::Standard,
        WorkloadTier::Frontier => WorkloadTier::Heavy,
    }
}

/// Whether the failure-domain term changed which candidate a failover chose
/// — the `subject` vocabulary for [`EvaluationKind::FailoverPrevented`], and
/// **map line 1851**'s two answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailoverPrevention {
    /// The winner differs from the winner of the same ranking with the
    /// failure-domain term removed — so the term steered this failover off a
    /// candidate that shares the failed backend's provider.
    Prevented,
    /// The term changed nothing about which candidate won. Recorded, not
    /// omitted: without it the denominator of *"how often"* would be the
    /// number of preventions, which is not a rate.
    NotPrevented,
}

impl FailoverPrevention {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Prevented => "prevented",
            Self::NotPrevented => "not-prevented",
        }
    }
}

impl EvaluationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MemoryRetrieved => "memory_retrieved",
            Self::MemoryRetrievalMiss => "memory_retrieval_miss",
            Self::DisposableRouteDecided => "disposable_route_decided",
            Self::RoutingOverrideDecided => "routing_override_decided",
            Self::RoutingContinuationDecided => "routing_continuation_decided",
            Self::RoutingCostClassObserved => "routing_cost_class_observed",
            Self::RoutingEvidenceObserved => "routing_evidence_observed",
            Self::RoutingOutcomeObserved => "routing_outcome_observed",
            Self::RoutingTierObserved => "routing_tier_observed",
            Self::FailoverPrevented => "failover_prevented",
            Self::MemoryRated => "memory_rated",
            Self::MemoryRevalidated => "memory_revalidated",
            Self::TurnOutcomeObserved => "turn_outcome_observed",
            Self::SessionRouteDecided => "session_route_decided",
            Self::RoutingConsumptionEstimated => "routing_consumption_estimated",
            Self::ReserveAvailabilityObserved => "reserve_availability_observed",
            Self::RoutingRated => "routing_rated",
            Self::MemoryExtractionObserved => "memory_extraction_observed",
        }
    }

    /// The inverse, for reads.
    ///
    /// [`None`] is *"a kind this build does not know"*, and every caller here
    /// turns it into [`crate::evaluation::EvaluationError::UnknownValue`] rather than bucketing
    /// the row into a neighbouring kind: a count that silently absorbs an
    /// unknown kind is worse than one that refuses.
    pub fn from_stored(value: &str) -> Option<Self> {
        match value {
            "memory_retrieved" => Some(Self::MemoryRetrieved),
            "memory_retrieval_miss" => Some(Self::MemoryRetrievalMiss),
            "disposable_route_decided" => Some(Self::DisposableRouteDecided),
            "routing_override_decided" => Some(Self::RoutingOverrideDecided),
            "routing_continuation_decided" => Some(Self::RoutingContinuationDecided),
            "routing_cost_class_observed" => Some(Self::RoutingCostClassObserved),
            "routing_evidence_observed" => Some(Self::RoutingEvidenceObserved),
            "routing_outcome_observed" => Some(Self::RoutingOutcomeObserved),
            "routing_tier_observed" => Some(Self::RoutingTierObserved),
            "failover_prevented" => Some(Self::FailoverPrevented),
            "memory_rated" => Some(Self::MemoryRated),
            "memory_revalidated" => Some(Self::MemoryRevalidated),
            "turn_outcome_observed" => Some(Self::TurnOutcomeObserved),
            "session_route_decided" => Some(Self::SessionRouteDecided),
            "routing_consumption_estimated" => Some(Self::RoutingConsumptionEstimated),
            "reserve_availability_observed" => Some(Self::ReserveAvailabilityObserved),
            "routing_rated" => Some(Self::RoutingRated),
            "memory_extraction_observed" => Some(Self::MemoryExtractionObserved),
            _ => None,
        }
    }
}

/// How a decision turned out, as far as was known when the row was written.
///
/// The vocabulary is **per kind** — `helped`/`stale` for a retrieval,
/// `preferred`/`displaced` for a route — which is why migration 15 gives this
/// column no global `CHECK` either: one would be two vocabularies in one
/// column.
///
/// **One variant, and it is the honest one.** No producer in this build knows
/// how a decision turned out at the moment it makes it, and an outcome learned
/// later is a new row rather than an edit, so `unknown` is the only value
/// anything writes. A row that does not say how it turned out must never be
/// countable as *"turned out badly"* — migration 11's `context_state`
/// argument, which is why the column is `NOT NULL DEFAULT 'unknown'` rather
/// than nullable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EvaluationOutcome {
    Unknown,
    /// [`EvaluationKind::MemoryRated`]'s eight verdict words, `useful`
    /// through `challenge-unjustified` below — map lines 1821, 1823, 1824,
    /// 1825 and 1831's closed vocabulary, decided in "Phase 51, the memory
    /// half of RC-B" and spelled once here for [`Self::as_str`] and
    /// [`Self::from_stored`] to round-trip.
    Useful,
    NotUseful,
    PreventedRepetition,
    CausedComplexity,
    RevalidationCorrect,
    RevalidationWrong,
    ChallengeJustified,
    ChallengeUnjustified,
}

impl EvaluationOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Useful => "useful",
            Self::NotUseful => "not-useful",
            Self::PreventedRepetition => "prevented-repetition",
            Self::CausedComplexity => "caused-complexity",
            Self::RevalidationCorrect => "revalidation-correct",
            Self::RevalidationWrong => "revalidation-wrong",
            Self::ChallengeJustified => "challenge-justified",
            Self::ChallengeUnjustified => "challenge-unjustified",
        }
    }

    pub fn from_stored(value: &str) -> Option<Self> {
        match value {
            "unknown" => Some(Self::Unknown),
            "useful" => Some(Self::Useful),
            "not-useful" => Some(Self::NotUseful),
            "prevented-repetition" => Some(Self::PreventedRepetition),
            "caused-complexity" => Some(Self::CausedComplexity),
            "revalidation-correct" => Some(Self::RevalidationCorrect),
            "revalidation-wrong" => Some(Self::RevalidationWrong),
            "challenge-justified" => Some(Self::ChallengeJustified),
            "challenge-unjustified" => Some(Self::ChallengeUnjustified),
            _ => None,
        }
    }
}

/// [`EvaluationOutcome`]'s eight rating-verdict values — every variant except
/// [`EvaluationOutcome::Unknown`], which a person never types: it is the
/// sentinel every other kind in this ledger writes for "not yet known", and
/// `glasshouse memory rate`'s CLI parser refuses it by name rather than
/// accepting it as a ninth verdict. Used by that parser's error message and
/// by [`EvaluationOutcome`]'s own round-trip test, so the CLI's vocabulary
/// and the type's can never carry two different spellings.
pub const MEMORY_RATING_VERDICTS: [EvaluationOutcome; 8] = [
    EvaluationOutcome::Useful,
    EvaluationOutcome::NotUseful,
    EvaluationOutcome::PreventedRepetition,
    EvaluationOutcome::CausedComplexity,
    EvaluationOutcome::RevalidationCorrect,
    EvaluationOutcome::RevalidationWrong,
    EvaluationOutcome::ChallengeJustified,
    EvaluationOutcome::ChallengeUnjustified,
];

/// [`EvaluationKind::RoutingRated`]'s own closed vocabulary — two of
/// [`MEMORY_RATING_VERDICTS`]' eight words, reused rather than a second
/// scale for the same question (design decision, *"The routing half of
/// RC-B"*, 2026-09-05). `glasshouse rate-route`'s CLI parser refuses every
/// other word, including the six memory-only verdicts, by name.
pub const ROUTE_RATING_VERDICTS: [EvaluationOutcome; 2] =
    [EvaluationOutcome::Useful, EvaluationOutcome::NotUseful];

/// The `subject` vocabulary for [`EvaluationKind::MemoryRetrieved`] and
/// [`EvaluationKind::MemoryRetrievalMiss`]: which of the questions the search
/// asked, and — for a miss — which door asked it.
///
/// The `Current`/`Historical` distinction is load-bearing for map line 1826
/// rather than decoration. A search run with `--history` is *asking* for
/// superseded memories, so a superseded memory in its results is the feature
/// working, not a memory "incorrectly resurfaced as current guidance". A
/// count that folded the two together would report the tool's own history
/// command as a defect.
///
/// It is also the reason `subject` carries a scope here and not the query
/// text. The query is the user's own words about their project, this ledger
/// has a shorter retention than the memories it points at, and no count in
/// Phase 51 needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RetrievalScope {
    /// The default search: current project knowledge only.
    Current,
    /// `--history`: superseded, rejected, resolved, invalidated, needs-review
    /// and conflicted memories were explicitly asked for.
    Historical,
    /// The launch-time briefing door ([`crate::memory::inject::briefing`]),
    /// on a [`EvaluationKind::MemoryRetrievalMiss`] row only — that door
    /// always searches [`crate::memory::search::SearchScope::Current`], so
    /// `Current` would be a truthful label for its own search but would fold
    /// its misses into the CLI/API door's own `current` count. A reader
    /// asking "which door is missing" needs the two distinguishable, and
    /// map line 1865's own reasoning is that the briefing door is almost
    /// certainly the busier of the two — folding it into `current` would
    /// report the quiet door and hide the busy one.
    Injection,
}

impl RetrievalScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Historical => "historical",
            Self::Injection => "injection",
        }
    }

    /// From the `--history` flag the CLI and the machine door both carry.
    pub fn from_history_flag(history: bool) -> Self {
        if history {
            Self::Historical
        } else {
            Self::Current
        }
    }
}
