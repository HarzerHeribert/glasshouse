//! Phase 51 — the project-local evaluation ledger.
//!
//! One row per **decision Glasshouse made whose wisdom is only visible
//! later**, written at the moment of the decision, in
//! `evaluation_observations` (`crate::database` migration 15). It answers
//! *how often*, over a window; it deliberately answers nothing about *how
//! much*, because cost, tokens and latency belong to
//! [`crate::routing::evidence`] and a second column for any of them here would
//! be a second source of truth for a fact that ledger already models.
//!
//! # What this ledger does **not** do, and why that is the deliverable
//!
//! Map line 1856 — *"keep evaluation data local and project-scoped unless the
//! user explicitly exports it"* — is carried in two halves, exactly as
//! [`crate::routing::evidence`] carries line 1343:
//!
//! - **Structurally, by the schema.** Migration 15's two triggers `RAISE`
//!   `ABORT` on an `INSERT` or an `UPDATE` that names any `project_id` but the
//!   one bound in `project_metadata`. A row for another project cannot be
//!   written by this store, by a future store, or by a hand-typed `INSERT` at
//!   a `sqlite3` prompt. The database path itself comes from
//!   [`crate::Runtime`] and nowhere else — there is no argument a caller can
//!   pass to reach another project's file.
//! - **Structurally, by this module's method list.** There is no `export`, no
//!   `to_json`, no `write_to`, no serialization of an observation to anything
//!   outside the process, and no method that hands out a [`Connection`]. Every
//!   read here returns counts or decoded rows to Rust callers in this process.
//!   *"Unless the user explicitly exports it"* is therefore a capability that
//!   does not exist yet rather than one guarded by a flag, which is the
//!   stronger of the two.
//!
//! And **no observation stores memory content.** A row carries a `memory_id`,
//! not a subject line and not a body: everything a count needs is already
//! durable in `memories`, so copying any of it here would be duplicating
//! project knowledge into a ledger with a shorter retention than the knowledge
//! itself.
//!
//! # Append-oriented, and prunable — which are not in tension
//!
//! There is a [`EvaluationObservations::record`] and there are reads, and
//! there is no method that edits a recorded observation: an outcome learned a
//! turn later is a *second row* with the same `memory_id`, never an `UPDATE`,
//! because a measurement edited in place is a falsified measurement.
//!
//! That is the [`crate::routing::evidence`] half. The other half is the one
//! `lifecycle_events` gets wrong: migration 5's append-only `DELETE` trigger
//! makes that table impossible to trim *even deliberately*, and an evaluation
//! ledger that grows per decision and can never be trimmed is a defect with a
//! delay. Migration 15 copies migration 11's two project-scope triggers and
//! **not** migration 5's three, and [`Retention`] is what fills the gap: 90
//! days or 100,000 rows, whichever binds first, trimmed oldest-first in the
//! writer's own transaction.
//!
//! Trimming happens on the connection that is already open and already
//! writing — never on a background thread with a second handle. Practice §65
//! is the reason: a SQLite handle opened on a path nobody asserts about is
//! free on the developer's machine and billed on Windows, where it hung six
//! tests for 37 minutes.
//!
//! # What a count means once rows are pruned
//!
//! A count over a window that reaches back past the oldest retained row is
//! wrong, and this module refuses it rather than returning a small number —
//! see [`EvaluationError::WindowNotRetained`]. Visible degradation, the same
//! rule the enum columns follow.
//!
//! The test is whether anything was *actually* trimmed, which `seq` answers
//! exactly: a ledger that has never pruned answers a window reaching back to
//! the epoch, because for that ledger the answer is simply everything it
//! holds.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::{Connection, OptionalExtension, params};

use crate::Runtime;
use crate::database::{EVALUATION_KINDS, PROJECT_ID_KEY};
use crate::routing::evidence::{
    MIN_SAMPLE_FOR_SUMMARY, RouteResponsiveness, RoutingObservation, UNKNOWN_HARNESS,
    row_to_observation,
};

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
    /// **This is the link row, and it is a third row rather than a rewrite of
    /// the two above.** [`record_routing_decision`] runs before a fresh
    /// launch has minted a session id, and its `session_id` is absent on
    /// purpose. Moving that call later — the other way to link a decision to
    /// a session — would change what lines 1829 and 1830 count: a launch that
    /// is refused while resolving its profile reaches the router and never
    /// reaches a session record, and those two lines are about the decision,
    /// not about what became of it. So the decision keeps its own moment and
    /// this row records the session it turned into.
    ///
    /// **`unknown` is a real answer, not a gap.** A destination on a
    /// harness's own sign-in has no configured provider and no marked model,
    /// and Glasshouse does not know what that costs at the margin; saying so
    /// is the [`crate::routing::Cost`] doc's own fail-closed stance carried
    /// into a count, and a reader that folded it into `metered` would report
    /// a number nobody measured.
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
    /// produced, which is what lets [`Self::RoutingOutcomeObserved`] be
    /// counted against it.
    ///
    /// **A launch with no `--task` records `unclassified`, never nothing.**
    /// The alternative — writing no row — would make *"this project never
    /// states its tasks"* indistinguishable from *"this project never
    /// launches"*, which is [`Self::RoutingOverrideDecided`]'s own argument
    /// one line over. The bucket is its own; it is never folded into a tier.
    ///
    /// **The tier and the escalation are one bucket rather than two
    /// columns**, because line 1834's question is about the pair: *does a
    /// tier predict a successful turn **without** escalation?* A reader
    /// grouping on `subject` alone therefore already has the comparison,
    /// with no second key and no join.
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
    /// the [`RetrievalScope`] word of the retrieval this rating judges (see
    /// [`record_memory_rating`]'s own doc comment), or is absent when the
    /// memory was never retrieved; `outcome` carries the verdict word itself
    /// ([`EvaluationOutcome`]'s eight non-[`EvaluationOutcome::Unknown`]
    /// values), `memory_id` is the rated memory, `session_id` is the
    /// session the rating is about when one was given, and `detail` is the
    /// operator's own note, never parsed.
    ///
    /// Design decision, "Phase 51, the memory half of RC-B: an explicit
    /// rating when given, a labelled proxy otherwise — user ruling
    /// 2026-09-02": *"Both: explicit rating when given, the labelled proxy
    /// otherwise."* This is the explicit half; every reader here labels the
    /// other half `proxy` and never folds the two together.
    ///
    /// **A rating is a new row, never an edit.** It judges a
    /// [`Self::MemoryRetrieved`] row (or, for 1823/1824/1825, a memory that
    /// was never retrieved in this exact window at all) without touching it
    /// — the same append-only shape every kind in this ledger keeps.
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
    /// the hook — map lines 1821 and 1831's proxy denominator, and the row
    /// [`Self::RoutingOutcomeObserved`] cannot be for this purpose, because
    /// that row refuses to write for a session with no routed destination.
    /// `subject` is `"completed"` or `"failed"`, spelled exactly as
    /// [`Self::RoutingOutcomeObserved`]'s own vocabulary — the same
    /// [`crate::events::TurnOutcome`], not a second word for the same fact.
    ///
    /// Design ruling, refusal register *"Phase 51's memory proxy — 1821 and
    /// 1831"*: option (b), because `api::unix::spawn_session` makes no
    /// routing decision, and writing a routed row for it would fabricate
    /// one. This row makes no claim about a route at all — it is the
    /// harness's verdict on the session's turn, full stop.
    ///
    /// **Written for every session that reaches the hook's `TurnEnded` arm,
    /// routed or not.** `main.rs`'s hook handler records this row and then
    /// [`Self::RoutingOutcomeObserved`] as before — a door-spawned session
    /// that was never routed gets this row and never a
    /// `RoutingOutcomeObserved` one; a CLI-launched session gets both. The
    /// memory-quality readers (1821, 1831) join a session-attributed
    /// retrieval to this row rather than to the routing row, because the
    /// proxy's definition is about the *session's* turn, not the *route's*.
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
    /// [`record_routing_consumption_estimate`]'s own doc comment.
    RoutingConsumptionEstimated,
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
/// word (see [`RouteOutcomeCounts::bucket`]).
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
        }
    }

    /// The inverse, for reads.
    ///
    /// [`None`] is *"a kind this build does not know"*, and every caller here
    /// turns it into [`EvaluationError::UnknownValue`] rather than bucketing
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

/// One observation to append. Everything but `kind` and `outcome` is optional
/// because most decisions know only some of it, and absent must stay
/// distinguishable from empty.
#[derive(Debug, Clone)]
pub struct NewObservation {
    pub kind: EvaluationKind,
    pub outcome: EvaluationOutcome,
    /// What it was about, in the vocabulary of `kind`.
    pub subject: Option<String>,
    /// The session the decision was made for, when it was made for one.
    pub session_id: Option<String>,
    /// The A/B half. Both or neither — the schema's own `CHECK`.
    pub feature: Option<String>,
    pub arm: Option<String>,
    /// The memory this decision was about. A bare id, never content.
    pub memory_id: Option<String>,
    /// The `routing_observations.seq` that owns this turn's measurement, so
    /// this ledger points at a cost instead of copying one.
    pub routing_seq: Option<i64>,
    /// The sentence a human reads after a count surprises them. Never parsed,
    /// never a `WHERE` key.
    pub detail: Option<String>,
}

impl NewObservation {
    /// An observation of one kind, with everything optional left absent and
    /// the outcome honestly unknown.
    pub fn new(kind: EvaluationKind) -> Self {
        Self {
            kind,
            outcome: EvaluationOutcome::Unknown,
            subject: None,
            session_id: None,
            feature: None,
            arm: None,
            memory_id: None,
            routing_seq: None,
            detail: None,
        }
    }

    pub fn with_subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = Some(subject.into());
        self
    }

    /// Set the outcome explicitly. Every other producer in this module
    /// leaves [`NewObservation::new`]'s honest `Unknown` in place — see this
    /// module's own header — so this exists for
    /// [`EvaluationKind::MemoryRated`] alone, whose whole point is that an
    /// outcome *is* known: the rater said so.
    pub fn with_outcome(mut self, outcome: EvaluationOutcome) -> Self {
        self.outcome = outcome;
        self
    }

    pub fn with_memory_id(mut self, memory_id: impl Into<String>) -> Self {
        self.memory_id = Some(memory_id.into());
        self
    }

    pub fn with_routing_seq(mut self, routing_seq: i64) -> Self {
        self.routing_seq = Some(routing_seq);
        self
    }

    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// One stored observation, decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvaluationObservation {
    pub seq: i64,
    pub observed_at: i64,
    pub kind: EvaluationKind,
    pub outcome: EvaluationOutcome,
    pub subject: Option<String>,
    pub session_id: Option<String>,
    pub feature: Option<String>,
    pub arm: Option<String>,
    pub memory_id: Option<String>,
    pub routing_seq: Option<i64>,
    pub detail: Option<String>,
}

/// How much history this ledger keeps, and how often it enforces that.
///
/// **Part of migration 15's contract, not a follow-up.** The three ledgers
/// before this one grow forever and this one has the highest write rate, so
/// the bounds ship with the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retention {
    /// Rows older than this many seconds are trimmed. 90 days by default: a
    /// window comfortably longer than any A/B comparison, and Phase 51's
    /// questions are *rate* questions, which need a window and not a history.
    pub max_age_secs: i64,
    /// At most this many rows are kept, newest first. 100,000 by default,
    /// which at roughly 150 bytes a row plus one index is a ceiling near
    /// 15 MB.
    pub max_rows: i64,
    /// The trim runs once every this many appended rows.
    ///
    /// **Counted on `seq`, not on a per-process counter, and that is the whole
    /// point.** `glasshouse memory search` is a process that appends a handful
    /// of rows and exits; an in-memory "every 256th insert" counter would
    /// reset every time and the trim would never run at all in the usage this
    /// ledger's rows actually come from.
    pub trim_every: i64,
}

impl Retention {
    /// 90 days, 100,000 rows, trimmed every 256 rows.
    pub const DEFAULT: Retention = Retention {
        max_age_secs: 90 * 24 * 60 * 60,
        max_rows: 100_000,
        trim_every: 256,
    };
}

impl Default for Retention {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Everything that can go wrong reading or writing this ledger.
#[derive(Debug, thiserror::Error)]
pub enum EvaluationError {
    #[error("the project database has no project identifier bound")]
    UnboundDatabase,
    #[error("evaluation observation {seq} stored an unrecognized {column} value `{value}`")]
    UnknownValue {
        seq: i64,
        column: &'static str,
        value: String,
    },
    #[error(
        "evaluation observation {seq} is of kind `{value}`, which this build \
         does not know; the kinds it reads are {}",
        EVALUATION_KINDS.join(", ")
    )]
    UnknownKind { seq: i64, value: String },
    #[error(
        "an evaluation count from {from} would reach past the oldest retained \
         observation ({oldest}); rows before it have been trimmed by the \
         retention policy, so the count would be an undercount rather than an \
         answer"
    )]
    WindowNotRetained { from: i64, oldest: i64 },
    #[error(
        "the evaluation ledger has been trimmed empty, so no window can be \
         counted; every observation it held is gone and a zero would read as \
         `this never happened`"
    )]
    LedgerFullyTrimmed,
    #[error("could not {action} in the evaluation ledger")]
    Sql {
        action: &'static str,
        #[source]
        source: rusqlite::Error,
    },
}

fn sql_err(action: &'static str) -> impl Fn(rusqlite::Error) -> EvaluationError {
    move |source| EvaluationError::Sql { action, source }
}

/// How often a retrieval handed back a memory that was not current knowledge.
///
/// Map lines 1822 and 1826, and **"stale" is not a judgement here**: it is
/// `memories.status = 'superseded'` or `memories.review_reason IS NOT NULL`,
/// columns migration 10 already added. Nothing new is inferred about a
/// memory; the only fact this ledger adds is *that a retrieval happened at
/// all*.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StaleRetrievalCounts {
    /// Every memory handed back in the window — the denominator.
    pub retrievals: i64,
    /// Of those, how many are superseded now. **Map line 1826.**
    pub superseded: i64,
    /// Of those, how many carry a review reason now.
    pub needs_review: i64,
    /// Either of the two. **Map line 1822.**
    pub stale: i64,
    /// Of `stale`, how many came from a search that explicitly asked for
    /// history. These are the tool doing what it was told, and a rate that
    /// counted them as defects would be measuring `--history` rather than
    /// staleness.
    pub stale_under_history: i64,
    /// Rows whose `memory_id` no longer resolves in `memories`. Reported
    /// rather than dropped: a join that silently loses rows makes every other
    /// number here a fraction of an unstated denominator.
    pub unresolved: i64,
}

/// This project's evaluation observations.
pub struct EvaluationObservations {
    conn: Mutex<Connection>,
    project_id: String,
    retention: Retention,
    /// Rows appended by this handle, only so a batch can tell whether it
    /// crossed a [`Retention::trim_every`] boundary. Never the trim's own
    /// clock — see [`Retention::trim_every`].
    appended: AtomicU64,
}

impl std::fmt::Debug for EvaluationObservations {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EvaluationObservations")
            .field("project_id", &self.project_id)
            .field("retention", &self.retention)
            .finish_non_exhaustive()
    }
}

impl EvaluationObservations {
    /// Open the active project's database with the shipped retention policy.
    ///
    /// The path comes from `runtime` and nowhere else — the same door
    /// [`crate::memory::ProjectMemory::open`] and
    /// [`crate::routing::evidence::EvidenceLedger::open`] use, so every check
    /// `crate::database::open` performs (the symlink refusal, the read-only
    /// refusal, the project-identity check, the migrations) applies here too.
    /// This is the whole of this ledger's own contribution to map line 1856's
    /// *"local and project-scoped"*: there is no second door.
    pub fn open(runtime: &Runtime) -> anyhow::Result<Self> {
        Self::open_with_retention(runtime, Retention::DEFAULT)
    }

    /// [`Self::open`] with the retention bounds replaced, so a test can watch
    /// the trim work on a handful of rows instead of a hundred thousand.
    pub fn open_with_retention(runtime: &Runtime, retention: Retention) -> anyhow::Result<Self> {
        let conn = crate::database::open(runtime)?;
        let project_id: Option<String> = conn
            .query_row(
                "SELECT value FROM project_metadata WHERE key = ?1",
                [PROJECT_ID_KEY],
                |row| row.get(0),
            )
            .optional()?;
        let project_id = project_id.ok_or(EvaluationError::UnboundDatabase)?;
        Ok(Self {
            conn: Mutex::new(conn),
            project_id,
            retention,
            appended: AtomicU64::new(0),
        })
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    pub fn retention(&self) -> Retention {
        self.retention
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Append one observation. Returns its `seq`.
    ///
    /// There is no corresponding `update`: see this module's own header.
    pub fn record(
        &self,
        new: NewObservation,
        observed_at_unix: i64,
    ) -> Result<i64, EvaluationError> {
        let seqs = self.record_all(std::slice::from_ref(&new), observed_at_unix)?;
        Ok(seqs.last().copied().unwrap_or_default())
    }

    /// Append several observations that describe one decision, in one
    /// transaction, and run the retention trim in that same transaction when
    /// this batch crosses a [`Retention::trim_every`] boundary.
    ///
    /// One transaction because a retrieval that returned five memories is one
    /// decision: a reader must never see three of its rows. The trim shares
    /// the transaction for the reason migration 15's doc comment gives — the
    /// connection is already open and already writing, so retention costs no
    /// new path, no new handle and no background thread.
    ///
    /// Returns the appended `seq` values in order.
    pub fn record_all(
        &self,
        new: &[NewObservation],
        observed_at_unix: i64,
    ) -> Result<Vec<i64>, EvaluationError> {
        if new.is_empty() {
            return Ok(Vec::new());
        }

        let mut conn = self.lock();
        let tx = conn
            .transaction()
            .map_err(sql_err("begin an evaluation append"))?;

        let mut seqs = Vec::with_capacity(new.len());
        {
            let mut statement = tx
                .prepare(
                    "INSERT INTO evaluation_observations (
                        project_id, observed_at, kind, outcome, subject, session_id,
                        feature, arm, memory_id, routing_seq, detail
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                )
                .map_err(sql_err("prepare an evaluation append"))?;
            for observation in new {
                statement
                    .execute(params![
                        self.project_id,
                        observed_at_unix,
                        observation.kind.as_str(),
                        observation.outcome.as_str(),
                        observation.subject,
                        observation.session_id,
                        observation.feature,
                        observation.arm,
                        observation.memory_id,
                        observation.routing_seq,
                        observation.detail,
                    ])
                    .map_err(sql_err("record an evaluation observation"))?;
                seqs.push(tx.last_insert_rowid());
            }
        }

        // `seq` is the durable insert counter, so the cadence survives a
        // process that appends five rows and exits. A batch trims at most
        // once, and only when it actually crossed a boundary.
        let last = *seqs.last().expect("a non-empty batch appended a row");
        let first = last - (seqs.len() as i64) + 1;
        let every = self.retention.trim_every.max(1);
        if last / every != (first - 1) / every {
            trim_within(&tx, self.retention, observed_at_unix)?;
        }

        tx.commit()
            .map_err(sql_err("commit an evaluation append"))?;
        self.appended
            .fetch_add(seqs.len() as u64, Ordering::Relaxed);
        Ok(seqs)
    }

    /// How many rows this handle has appended. Diagnostics only — the trim's
    /// cadence is `seq`, not this.
    pub fn appended(&self) -> u64 {
        self.appended.load(Ordering::Relaxed)
    }

    /// Enforce the retention bounds now, and report how many rows went.
    ///
    /// [`Self::record_all`] runs this on its own cadence; this is the same
    /// operation exposed so that retention is something a test can watch
    /// happen rather than something a comment claims.
    pub fn trim(&self, now_unix: i64) -> Result<usize, EvaluationError> {
        let mut conn = self.lock();
        let tx = conn
            .transaction()
            .map_err(sql_err("begin an evaluation trim"))?;
        let removed = trim_within(&tx, self.retention, now_unix)?;
        tx.commit().map_err(sql_err("commit an evaluation trim"))?;
        Ok(removed)
    }

    /// The `observed_at` of the oldest row still retained, or [`None`] when
    /// the ledger is empty.
    pub fn oldest_retained_at(&self) -> Result<Option<i64>, EvaluationError> {
        let conn = self.lock();
        conn.query_row(
            "SELECT MIN(observed_at) FROM evaluation_observations",
            [],
            |row| row.get::<_, Option<i64>>(0),
        )
        .map_err(sql_err("read the oldest retained observation"))
    }

    /// How many rows of one kind fell in `[from, to]` — the shape every
    /// Phase 51 line reduces to, and the one
    /// `evaluation_observations_by_kind_time` exists to serve.
    pub fn count(&self, kind: EvaluationKind, from: i64, to: i64) -> Result<i64, EvaluationError> {
        self.refuse_unretained_window(from)?;
        let conn = self.lock();
        conn.query_row(
            "SELECT COUNT(*) FROM evaluation_observations
              WHERE kind = ?1 AND observed_at >= ?2 AND observed_at <= ?3",
            params![kind.as_str(), from, to],
            |row| row.get(0),
        )
        .map_err(sql_err("count evaluation observations"))
    }

    /// How often a retrieval in `[from, to]` handed back a memory that is not
    /// current knowledge — **map lines 1822 and 1826**.
    ///
    /// The join is to `memories`, so "stale" is read out of the columns
    /// migration 10 already maintains rather than judged here. That has one
    /// honest consequence, and it is not hidden: this answers *"is the memory
    /// stale now"*, not *"was it stale when it was handed back"*. A memory
    /// superseded after a retrieval counts against that retrieval. Recording
    /// the status at retrieval time instead would put a second copy of
    /// `memories.status` in this table, which is the duplication migration 15
    /// exists to avoid.
    pub fn stale_retrievals(
        &self,
        from: i64,
        to: i64,
    ) -> Result<StaleRetrievalCounts, EvaluationError> {
        self.refuse_unretained_window(from)?;
        let conn = self.lock();
        conn.query_row(
            "SELECT
                 COUNT(*),
                 COALESCE(SUM(CASE WHEN m.status = 'superseded' THEN 1 ELSE 0 END), 0),
                 COALESCE(SUM(CASE WHEN m.review_reason IS NOT NULL THEN 1 ELSE 0 END), 0),
                 COALESCE(SUM(CASE WHEN m.status = 'superseded'
                                     OR m.review_reason IS NOT NULL
                                   THEN 1 ELSE 0 END), 0),
                 COALESCE(SUM(CASE WHEN (m.status = 'superseded'
                                          OR m.review_reason IS NOT NULL)
                                        AND o.subject = ?4
                                   THEN 1 ELSE 0 END), 0),
                 COALESCE(SUM(CASE WHEN m.id IS NULL THEN 1 ELSE 0 END), 0)
             FROM evaluation_observations AS o
             LEFT JOIN memories AS m
                    ON m.id = o.memory_id AND m.project_id = o.project_id
             WHERE o.kind = ?1
               AND o.observed_at >= ?2
               AND o.observed_at <= ?3",
            params![
                EvaluationKind::MemoryRetrieved.as_str(),
                from,
                to,
                RetrievalScope::Historical.as_str(),
            ],
            |row| {
                Ok(StaleRetrievalCounts {
                    retrievals: row.get(0)?,
                    superseded: row.get(1)?,
                    needs_review: row.get(2)?,
                    stale: row.get(3)?,
                    stale_under_history: row.get(4)?,
                    unresolved: row.get(5)?,
                })
            },
        )
        .map_err(sql_err("count stale memory retrievals"))
    }

    /// The most recent observations, newest first.
    ///
    /// A row whose `kind` or `outcome` this build does not recognize is an
    /// error naming the row and the value, never a row bucketed into a
    /// neighbour.
    pub fn recent(&self, limit: usize) -> Result<Vec<EvaluationObservation>, EvaluationError> {
        let conn = self.lock();
        let mut statement = conn
            .prepare(&format!(
                "SELECT {OBSERVATION_COLUMNS}
                   FROM evaluation_observations
                  ORDER BY seq DESC
                  LIMIT ?1"
            ))
            .map_err(sql_err("read evaluation observations"))?;
        let rows = statement
            .query_map(params![limit as i64], read_observation_row)
            .map_err(sql_err("read evaluation observations"))?;
        collect_observations(rows)
    }

    /// [`Self::recent`] narrowed to one kind.
    ///
    /// **Additive, and the reason it exists is the one `observed_identities`
    /// gives in [`crate::routing::evidence`]:** a view about *one* kind of
    /// decision cannot be built out of an unkeyed listing. [`Self::recent`]
    /// returns the newest rows of every kind, so a reader wanting the last
    /// twenty routing decisions would get twenty memory retrievals on any
    /// project that had searched recently, and would have to ask for an
    /// unbounded number of rows to be sure of finding one. The narrowing is
    /// done in SQL for the same reason: `LIMIT` after `WHERE` is the only
    /// order that answers *"the newest twenty of this kind"*.
    ///
    /// It also cannot fail on a row this build does not understand, where
    /// [`Self::recent`] can: `kind` is bound as a parameter, so a row written
    /// by a later Glasshouse under a kind this build has never heard of is
    /// never selected, never decoded, and cannot turn one reader's view into
    /// an error about a different reader's data.
    pub fn recent_of_kind(
        &self,
        kind: EvaluationKind,
        limit: usize,
    ) -> Result<Vec<EvaluationObservation>, EvaluationError> {
        let conn = self.lock();
        let mut statement = conn
            .prepare(&format!(
                "SELECT {OBSERVATION_COLUMNS}
                   FROM evaluation_observations
                  WHERE kind = ?1
                  ORDER BY seq DESC
                  LIMIT ?2"
            ))
            .map_err(sql_err("read evaluation observations of one kind"))?;
        let rows = statement
            .query_map(params![kind.as_str(), limit as i64], read_observation_row)
            .map_err(sql_err("read evaluation observations of one kind"))?;
        collect_observations(rows)
    }

    /// [`Self::recent_of_kind`] narrowed further, to one session — map line
    /// 1759's debug view: which memories were retrieved for a routed task,
    /// the task being the session the retrieval was attributed to.
    ///
    /// [`crate::evaluation::record_memory_retrieval`] only calls
    /// [`NewObservation::with_session_id`] when its caller knows one, so a
    /// retrieval recorded with no session id is never returned here — a
    /// stated limit of the view, not a defect of this reader.
    pub fn retrievals_for_session(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<EvaluationObservation>, EvaluationError> {
        let conn = self.lock();
        let mut statement = conn
            .prepare(&format!(
                "SELECT {OBSERVATION_COLUMNS}
                   FROM evaluation_observations
                  WHERE kind = ?1 AND session_id = ?2
                  ORDER BY seq DESC
                  LIMIT ?3"
            ))
            .map_err(sql_err("read a session's evaluation observations"))?;
        let rows = statement
            .query_map(
                params![
                    EvaluationKind::MemoryRetrieved.as_str(),
                    session_id,
                    limit as i64
                ],
                read_observation_row,
            )
            .map_err(sql_err("read a session's evaluation observations"))?;
        collect_observations(rows)
    }

    /// The `subject` (the [`RetrievalScope`] word) of the retrieval
    /// [`record_memory_rating`] is attributing this rating to — map line
    /// 939. The most recent [`EvaluationKind::MemoryRetrieved`] row for
    /// `memory_id` carrying the given `session_id` when one is given and a
    /// row matches it, else the most recent such row for `memory_id`
    /// regardless of session, else [`None`] when the memory was never
    /// retrieved at all.
    ///
    /// **One query.** The `ORDER BY` puts a session match first (when
    /// `session_id` is [`Some`]) and falls back to recency alone otherwise —
    /// a plain `session_id = ?3` in that position would rank a real,
    /// differing session above a `NULL` one whenever `session_id` is
    /// [`None`], which is not "the most recent at all".
    fn most_recent_retrieval_scope(
        &self,
        memory_id: &str,
        session_id: Option<&str>,
    ) -> Result<Option<String>, EvaluationError> {
        let conn = self.lock();
        conn.query_row(
            "SELECT subject
               FROM evaluation_observations
              WHERE kind = ?1 AND memory_id = ?2
              ORDER BY CASE WHEN session_id = ?3 THEN 1 ELSE 0 END DESC, seq DESC
              LIMIT 1",
            params![
                EvaluationKind::MemoryRetrieved.as_str(),
                memory_id,
                session_id
            ],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(sql_err("look up a memory rating's retrieval scope"))
    }

    /// Refuse a window that reaches back past what retention kept.
    ///
    /// **The test is whether anything was actually trimmed, not whether the
    /// window starts before the first row.** A ledger that has never pruned
    /// can answer a window reaching back to the epoch perfectly well — the
    /// answer is *"everything I hold"* — and refusing that would make the most
    /// natural question unaskable while proving nothing.
    ///
    /// `seq` is what makes the distinction exact rather than a guess.
    /// `AUTOINCREMENT` numbers from 1 and never reuses a value, so
    /// `MIN(seq) == 1` is *"nothing has ever been removed from the oldest
    /// end"*, and `MIN(seq) > 1` is *"rows before the oldest one I hold were
    /// trimmed"*. The same column closes the case an oldest-row test cannot
    /// see at all: an empty table whose `sqlite_sequence` high-water mark is
    /// non-zero once held rows and now holds none, where a zero would read as
    /// *"this never happened"*.
    fn refuse_unretained_window(&self, from: i64) -> Result<(), EvaluationError> {
        let conn = self.lock();
        let (lowest_seq, oldest_at) = conn
            .query_row(
                "SELECT MIN(seq), MIN(observed_at) FROM evaluation_observations",
                [],
                |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?)),
            )
            .map_err(sql_err("read the retained window"))?;

        match lowest_seq.zip(oldest_at) {
            // Rows are present and none was ever trimmed from the front.
            Some((1, _)) => Ok(()),
            Some((_, oldest)) if from < oldest => {
                Err(EvaluationError::WindowNotRetained { from, oldest })
            }
            Some(_) => Ok(()),
            None => {
                // Empty. Did it always hold nothing, or was it emptied?
                let high_water: Option<i64> = conn
                    .query_row(
                        "SELECT seq FROM sqlite_sequence WHERE name = ?1",
                        ["evaluation_observations"],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(sql_err("read the evaluation ledger's high-water mark"))?;
                match high_water {
                    Some(seq) if seq > 0 => Err(EvaluationError::LedgerFullyTrimmed),
                    _ => Ok(()),
                }
            }
        }
    }
}

/// The five readers for "Phase 51, the memory half of RC-B" — map lines
/// 1821, 1823, 1824, 1825 and 1831 — kept in their own block for practice
/// §77's reason: a second worker's reader and this one must not be able to
/// land on the same lines.
///
/// # The proxy's join key, closed by `GH-TURN-OUTCOME-ROW`
///
/// The design decision's proxy for 1821/1831 is *"the retrieving session's
/// turn ended `Completed` … with no failover, retry, override or early
/// abandonment recorded against it."* That needs a
/// [`EvaluationKind::MemoryRetrieved`] row's `session_id` to find "the
/// retrieving session" at all, and a same-session row saying how its turn
/// ended. `GH-RETRIEVAL-ATTRIBUTION` gave the launch-time briefing door —
/// `api/unix.rs::deliver_memory` — the first: a successful injection carries
/// the session it was delivered to. `main.rs::memory_search_grouped`'s two
/// callers still pass `None`: `glasshouse memory search` has no session to
/// attribute a person's own command to, and the machine door's
/// `query_memory` has no session field on its `Request::QueryMemory` to
/// thread one from at all.
///
/// The second used to be [`EvaluationKind::RoutingOutcomeObserved`], and that
/// row **never arises for a door-spawned session**:
/// [`record_routing_outcome`] refuses to write anything for a session with no
/// prior routed destination, and only `main.rs::launch_session` (the CLI
/// `glasshouse launch` path) ever calls [`record_routed_session`] — the
/// door's own `Request::SpawnSession`/`Request::SendMessage`, which is what
/// actually calls `deliver_memory`, never routes a session at all. So the two
/// producers could never meet on one session (refusal register, *"Phase 51's
/// memory proxy — 1821 and 1831"*).
///
/// The queries below join instead on [`EvaluationKind::TurnOutcomeObserved`]
/// — a row `record_turn_outcome` writes for **every** session that reaches
/// the hook's `TurnEnded` arm, routed or not. A door-spawned session's turn
/// end now lands a row on the same session id `deliver_memory` already
/// attached to its retrieval, so the join has a real producer on both sides
/// that actually meet. [`EvaluationKind::RoutingOutcomeObserved`] is
/// unchanged and still feeds the routing readers below; this join no longer
/// uses it.
///
/// Of the four negative signals the design names — failover, retry,
/// override, early abandonment — only **override**
/// ([`EvaluationKind::RoutingOverrideDecided`], `subject = "overridden"`)
/// has a row shape this ledger can join on a session id at all, and that row
/// is written only for a routed (launched) session, so it never suppresses a
/// door-spawned session's proxy hit — there being no override row to find is
/// the correct answer for a session an override could never have applied to.
/// [`EvaluationKind::FailoverPrevented`] carries no `session_id` by its own
/// design (see that variant's doc comment), no evaluation kind here
/// observes a "retry", and [`crate::events::TurnOutcome`] has exactly two
/// values — `Completed` and `Failed` — so "early abandonment" is not a
/// state this ledger can tell apart from ordinary silence. Those three are
/// therefore omitted from the join by name, not invented.
impl EvaluationObservations {
    /// **Map line 1821**: *"Measure how often retrieved memory is actually
    /// useful to the receiving agent."*
    pub fn usefulness(&self, from: i64, to: i64) -> Result<UsefulnessCounts, EvaluationError> {
        self.refuse_unretained_window(from)?;
        let conn = self.lock();
        conn.query_row(
            "SELECT
                 (SELECT COUNT(*) FROM evaluation_observations
                    WHERE kind = ?1 AND outcome = ?2
                      AND observed_at >= ?4 AND observed_at <= ?5),
                 (SELECT COUNT(*) FROM evaluation_observations
                    WHERE kind = ?1 AND outcome = ?3
                      AND observed_at >= ?4 AND observed_at <= ?5),
                 (SELECT COUNT(*) FROM evaluation_observations
                    WHERE kind = ?6 AND observed_at >= ?4 AND observed_at <= ?5),
                 (SELECT COUNT(*) FROM evaluation_observations AS r
                    WHERE r.kind = ?6 AND r.session_id IS NOT NULL
                      AND r.observed_at >= ?4 AND r.observed_at <= ?5
                      AND EXISTS (
                          SELECT 1 FROM evaluation_observations AS c
                           WHERE c.kind = ?7 AND c.subject = ?8
                             AND c.session_id = r.session_id
                      )
                      AND NOT EXISTS (
                          SELECT 1 FROM evaluation_observations AS o
                           WHERE o.kind = ?9 AND o.subject = ?10
                             AND o.session_id = r.session_id
                      ))",
            params![
                EvaluationKind::MemoryRated.as_str(),
                EvaluationOutcome::Useful.as_str(),
                EvaluationOutcome::NotUseful.as_str(),
                from,
                to,
                EvaluationKind::MemoryRetrieved.as_str(),
                EvaluationKind::TurnOutcomeObserved.as_str(),
                TURN_COMPLETED,
                EvaluationKind::RoutingOverrideDecided.as_str(),
                "overridden",
            ],
            |row| {
                let explicit_useful: i64 = row.get(0)?;
                let explicit_not_useful: i64 = row.get(1)?;
                let retrieved: i64 = row.get(2)?;
                let proxy: i64 = row.get(3)?;
                Ok(UsefulnessCounts {
                    explicit_useful,
                    explicit_not_useful,
                    proxy_useful: proxy,
                    proxy_denominator: proxy,
                    unknown: (retrieved - proxy).max(0),
                    retrieved,
                })
            },
        )
        .map_err(sql_err("count memory usefulness ratings"))
    }

    /// **Map line 1831**: *"Measure how often memory prevents repetition of
    /// a recorded failed approach."* Scoped to retrievals of
    /// `memories.kind = 'failed_attempt'` — the memory's own class, not a
    /// judgement made here.
    pub fn prevented_repetition(
        &self,
        from: i64,
        to: i64,
    ) -> Result<PreventedRepetitionCounts, EvaluationError> {
        self.refuse_unretained_window(from)?;
        let conn = self.lock();
        conn.query_row(
            "SELECT
                 (SELECT COUNT(*) FROM evaluation_observations
                    WHERE kind = ?1 AND outcome = ?2
                      AND observed_at >= ?3 AND observed_at <= ?4),
                 (SELECT COUNT(*) FROM evaluation_observations AS r
                    JOIN memories AS m
                      ON m.id = r.memory_id AND m.project_id = r.project_id
                   WHERE r.kind = ?5 AND m.kind = 'failed_attempt'
                     AND r.observed_at >= ?3 AND r.observed_at <= ?4),
                 (SELECT COUNT(*) FROM evaluation_observations AS r
                    JOIN memories AS m
                      ON m.id = r.memory_id AND m.project_id = r.project_id
                   WHERE r.kind = ?5 AND m.kind = 'failed_attempt'
                     AND r.session_id IS NOT NULL
                     AND r.observed_at >= ?3 AND r.observed_at <= ?4
                     AND EXISTS (
                         SELECT 1 FROM evaluation_observations AS c
                          WHERE c.kind = ?6 AND c.subject = ?7
                            AND c.session_id = r.session_id
                     )
                     AND NOT EXISTS (
                         SELECT 1 FROM evaluation_observations AS o
                          WHERE o.kind = ?8 AND o.subject = ?9
                            AND o.session_id = r.session_id
                     ))",
            params![
                EvaluationKind::MemoryRated.as_str(),
                EvaluationOutcome::PreventedRepetition.as_str(),
                from,
                to,
                EvaluationKind::MemoryRetrieved.as_str(),
                EvaluationKind::TurnOutcomeObserved.as_str(),
                TURN_COMPLETED,
                EvaluationKind::RoutingOverrideDecided.as_str(),
                "overridden",
            ],
            |row| {
                let explicit: i64 = row.get(0)?;
                let retrieved: i64 = row.get(1)?;
                let proxy: i64 = row.get(2)?;
                Ok(PreventedRepetitionCounts {
                    explicit,
                    proxy,
                    proxy_denominator: proxy,
                    unknown: (retrieved - proxy).max(0),
                    retrieved,
                })
            },
        )
        .map_err(sql_err("count prevented-repetition ratings"))
    }

    /// **Map line 1823**: *"Measure how often an old decision causes an
    /// agent to add unnecessary implementation complexity."* Explicit only
    /// — no observation in this build bears on whether a decision *caused*
    /// complexity, so there is no proxy. Scoped to retrievals of
    /// `memories.kind = 'decision'`.
    pub fn caused_complexity(
        &self,
        from: i64,
        to: i64,
    ) -> Result<CausedComplexityCounts, EvaluationError> {
        self.refuse_unretained_window(from)?;
        let conn = self.lock();
        conn.query_row(
            "SELECT
                 (SELECT COUNT(*) FROM evaluation_observations
                    WHERE kind = ?1 AND outcome = ?2
                      AND observed_at >= ?3 AND observed_at <= ?4),
                 (SELECT COUNT(*) FROM evaluation_observations AS r
                    JOIN memories AS m
                      ON m.id = r.memory_id AND m.project_id = r.project_id
                   WHERE r.kind = ?5 AND m.kind = 'decision'
                     AND r.observed_at >= ?3 AND r.observed_at <= ?4)",
            params![
                EvaluationKind::MemoryRated.as_str(),
                EvaluationOutcome::CausedComplexity.as_str(),
                from,
                to,
                EvaluationKind::MemoryRetrieved.as_str(),
            ],
            |row| {
                let explicit: i64 = row.get(0)?;
                let retrieved: i64 = row.get(1)?;
                Ok(CausedComplexityCounts {
                    explicit,
                    unknown: (retrieved - explicit).max(0),
                    retrieved,
                })
            },
        )
        .map_err(sql_err("count caused-complexity ratings"))
    }

    /// **Map line 1824**: *"Measure how often revalidation correctly
    /// identifies a decision whose original assumptions no longer hold."*
    /// Explicit ratings over a real denominator: `glasshouse memory
    /// revalidate`'s four outcomes share no single production *memory*
    /// column that means "a revalidation happened" — `reaffirmed` writes
    /// `last_validated_at`, `needs-review` reuses `mark_for_review`'s
    /// `review_marked_at` (the same column [`Self::challenge_accuracy`]
    /// reads, so it cannot serve as *this* line's own denominator without
    /// double meaning), and `superseded`/`invalidated` write no
    /// distinguishing column at all. `GH-RETRIEVAL-ATTRIBUTION` closes that
    /// gap with its own row instead —
    /// [`EvaluationKind::MemoryRevalidated`], written once per call to
    /// `main.rs::memory_revalidate` regardless of which outcome — so the
    /// denominator below counts that kind, not a `memories` column.
    pub fn revalidation_accuracy(
        &self,
        from: i64,
        to: i64,
    ) -> Result<RevalidationAccuracyCounts, EvaluationError> {
        self.refuse_unretained_window(from)?;
        let conn = self.lock();
        conn.query_row(
            "SELECT
                 (SELECT COUNT(*) FROM evaluation_observations
                    WHERE kind = ?1 AND outcome = ?2
                      AND observed_at >= ?4 AND observed_at <= ?5),
                 (SELECT COUNT(*) FROM evaluation_observations
                    WHERE kind = ?1 AND outcome = ?3
                      AND observed_at >= ?4 AND observed_at <= ?5),
                 (SELECT COUNT(*) FROM evaluation_observations
                    WHERE kind = ?6
                      AND observed_at >= ?4 AND observed_at <= ?5)",
            params![
                EvaluationKind::MemoryRated.as_str(),
                EvaluationOutcome::RevalidationCorrect.as_str(),
                EvaluationOutcome::RevalidationWrong.as_str(),
                from,
                to,
                EvaluationKind::MemoryRevalidated.as_str(),
            ],
            |row| {
                let correct: i64 = row.get(0)?;
                let wrong: i64 = row.get(1)?;
                let revalidations: i64 = row.get(2)?;
                Ok(RevalidationAccuracyCounts {
                    correct,
                    wrong,
                    revalidations,
                    unknown: (revalidations - correct - wrong).max(0),
                })
            },
        )
        .map_err(sql_err("count revalidation-accuracy ratings"))
    }

    /// **Map line 1825**: *"Measure how often agents challenge a remembered
    /// decision and whether the challenge was justified."* Explicit only.
    /// The denominator is `memories.review_marked_at` in the window —
    /// `MemoryStore::mark_for_review`'s own column, which is what both
    /// `glasshouse memory challenge` and a `glasshouse memory revalidate …
    /// needs-review` outcome write. **Recorded limit, not a blocker**: the
    /// two are indistinguishable in this column, so a revalidation that
    /// re-flags an already-challenged memory counts here as a second
    /// challenge.
    pub fn challenge_accuracy(
        &self,
        from: i64,
        to: i64,
    ) -> Result<ChallengeAccuracyCounts, EvaluationError> {
        self.refuse_unretained_window(from)?;
        let conn = self.lock();
        conn.query_row(
            "SELECT
                 (SELECT COUNT(*) FROM evaluation_observations
                    WHERE kind = ?1 AND outcome = ?2
                      AND observed_at >= ?4 AND observed_at <= ?5),
                 (SELECT COUNT(*) FROM evaluation_observations
                    WHERE kind = ?1 AND outcome = ?3
                      AND observed_at >= ?4 AND observed_at <= ?5),
                 (SELECT COUNT(*) FROM memories
                    WHERE project_id = ?6
                      AND review_marked_at >= ?4 AND review_marked_at <= ?5)",
            params![
                EvaluationKind::MemoryRated.as_str(),
                EvaluationOutcome::ChallengeJustified.as_str(),
                EvaluationOutcome::ChallengeUnjustified.as_str(),
                from,
                to,
                self.project_id,
            ],
            |row| {
                let justified: i64 = row.get(0)?;
                let unjustified: i64 = row.get(1)?;
                let challenges: i64 = row.get(2)?;
                Ok(ChallengeAccuracyCounts {
                    justified,
                    unjustified,
                    unknown: (challenges - justified - unjustified).max(0),
                    challenges,
                })
            },
        )
        .map_err(sql_err("count challenge-accuracy ratings"))
    }

    /// **Map line 939**: *"Record false-positive or harmful memory
    /// retrievals so the retrieval policy can be evaluated."* One row per
    /// [`RetrievalScope`] word present on any [`EvaluationKind::MemoryRetrieved`]
    /// or [`EvaluationKind::MemoryRated`] row in the window, plus one row with
    /// `scope: None` for [`EvaluationKind::MemoryRated`] rows whose `subject`
    /// is unset — a rating of a memory this window never saw retrieved
    /// ([`record_memory_rating`]'s attribution lookup found nothing).
    ///
    /// `retrieved` counts that scope's [`EvaluationKind::MemoryRetrieved`]
    /// rows; `not_useful` and `caused_complexity` count that scope's
    /// [`EvaluationKind::MemoryRated`] rows carrying those two verdicts
    /// only — [`EvaluationOutcome::Useful`] and the other five verdicts are
    /// never counted here, because this reader answers "was this retrieval
    /// a false positive or harmful", not [`Self::usefulness`]'s question.
    pub fn false_positives_by_scope(
        &self,
        from: i64,
        to: i64,
    ) -> Result<Vec<FalsePositivesByScope>, EvaluationError> {
        self.refuse_unretained_window(from)?;
        let conn = self.lock();
        let mut statement = conn
            .prepare(
                "WITH scopes AS (
                     SELECT DISTINCT subject FROM evaluation_observations
                      WHERE kind = ?1 AND observed_at >= ?5 AND observed_at <= ?6
                     UNION
                     SELECT DISTINCT subject FROM evaluation_observations
                      WHERE kind = ?2 AND observed_at >= ?5 AND observed_at <= ?6
                 )
                 SELECT
                     s.subject,
                     (SELECT COUNT(*) FROM evaluation_observations r
                        WHERE r.kind = ?1 AND r.subject IS s.subject
                          AND r.observed_at >= ?5 AND r.observed_at <= ?6),
                     (SELECT COUNT(*) FROM evaluation_observations o
                        WHERE o.kind = ?2 AND o.subject IS s.subject AND o.outcome = ?3
                          AND o.observed_at >= ?5 AND o.observed_at <= ?6),
                     (SELECT COUNT(*) FROM evaluation_observations o
                        WHERE o.kind = ?2 AND o.subject IS s.subject AND o.outcome = ?4
                          AND o.observed_at >= ?5 AND o.observed_at <= ?6)
                 FROM scopes s
                 ORDER BY s.subject IS NULL, s.subject",
            )
            .map_err(sql_err("read false-positive counts by retrieval scope"))?;
        let rows = statement
            .query_map(
                params![
                    EvaluationKind::MemoryRetrieved.as_str(),
                    EvaluationKind::MemoryRated.as_str(),
                    EvaluationOutcome::NotUseful.as_str(),
                    EvaluationOutcome::CausedComplexity.as_str(),
                    from,
                    to,
                ],
                |row| {
                    Ok(FalsePositivesByScope {
                        scope: row.get(0)?,
                        retrieved: row.get(1)?,
                        not_useful: row.get(2)?,
                        caused_complexity: row.get(3)?,
                    })
                },
            )
            .map_err(sql_err("read false-positive counts by retrieval scope"))?;
        rows.collect::<Result<Vec<_>, rusqlite::Error>>()
            .map_err(sql_err("read false-positive counts by retrieval scope"))
    }
}

/// **Map line 1821**'s counts: explicit ratings, the labelled proxy, and
/// unknown — see this block's own header for why the proxy is always zero
/// until a producer attaches `session_id` to a retrieval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UsefulnessCounts {
    /// `glasshouse memory rate <id> useful` calls in the window.
    pub explicit_useful: i64,
    /// `glasshouse memory rate <id> not-useful` calls in the window.
    pub explicit_not_useful: i64,
    /// Retrievals whose session's own verdict qualifies for the proxy.
    /// Equal to [`Self::proxy_denominator`]: nothing here yet distinguishes
    /// a qualifying session that *was* useful from one that was not, so
    /// every retrieval the proxy can attribute at all counts toward this.
    pub proxy_useful: i64,
    /// The proxy's own denominator: retrievals joined to a session whose
    /// turn ended `Completed` with no override recorded.
    pub proxy_denominator: i64,
    /// `retrieved` minus the proxy denominator — retrievals this ledger
    /// cannot attribute to a qualifying session at all.
    pub unknown: i64,
    /// Every memory returned in the window — the denominator for
    /// [`Self::unknown`].
    pub retrieved: i64,
}

/// **Map line 1831**'s counts, the same shape as [`UsefulnessCounts`] but
/// with one explicit verdict word instead of two.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PreventedRepetitionCounts {
    pub explicit: i64,
    pub proxy: i64,
    pub proxy_denominator: i64,
    pub unknown: i64,
    /// Retrievals of `memories.kind = 'failed_attempt'` in the window.
    pub retrieved: i64,
}

/// **Map line 1823**'s counts: explicit only, no proxy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CausedComplexityCounts {
    pub explicit: i64,
    pub unknown: i64,
    /// Retrievals of `memories.kind = 'decision'` in the window.
    pub retrieved: i64,
}

/// **Map line 1824**'s counts: explicit ratings, denominator from
/// [`EvaluationKind::MemoryRevalidated`] — see
/// [`EvaluationObservations::revalidation_accuracy`]'s own doc comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RevalidationAccuracyCounts {
    pub correct: i64,
    pub wrong: i64,
    /// `glasshouse memory revalidate` calls in the window, any outcome.
    pub revalidations: i64,
    /// Revalidations in the window nobody has rated `revalidation-correct`
    /// or `revalidation-wrong`.
    pub unknown: i64,
}

/// **Map line 1825**'s counts: explicit only, denominator from
/// `memories.review_marked_at`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ChallengeAccuracyCounts {
    pub justified: i64,
    pub unjustified: i64,
    pub unknown: i64,
    /// Memories marked for review (challenged, or re-flagged by a
    /// `needs-review` revalidation — see the reader's own doc comment) in
    /// the window.
    pub challenges: i64,
}

/// **Map line 939**'s counts, one bucket per [`RetrievalScope`] —
/// [`EvaluationObservations::false_positives_by_scope`]'s own row.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FalsePositivesByScope {
    /// The [`RetrievalScope`] word, or [`None`] for ratings of a memory this
    /// window never saw retrieved.
    pub scope: Option<String>,
    /// That scope's [`EvaluationKind::MemoryRetrieved`] rows in the window.
    /// Always 0 when [`Self::scope`] is [`None`] — a retrieval always
    /// carries a scope, so nothing ever populates that bucket's numerator.
    pub retrieved: i64,
    /// That scope's [`EvaluationKind::MemoryRated`] rows carrying
    /// [`EvaluationOutcome::NotUseful`] in the window.
    pub not_useful: i64,
    /// That scope's [`EvaluationKind::MemoryRated`] rows carrying
    /// [`EvaluationOutcome::CausedComplexity`] in the window.
    pub caused_complexity: i64,
}

/// One bucket of routed sessions, and what their harnesses said about their
/// turns — the shape map lines 1834, 1835, 1845 and 1854 all reduce to.
///
/// **Two different denominators, kept apart on purpose.** `sessions` counts
/// routing decisions; `completed` and `failed` count *turns*, because a
/// session runs many and each one is a thing the harness reported on. A
/// reader that divided completions by sessions would produce a rate above 1
/// on any project that works for an afternoon. Every rendering of this must
/// print both, which is what [`Self::reported_turns`] exists to make easy.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RouteOutcomeCounts {
    /// The vocabulary word this bucket groups on — a cost class, an evidence
    /// state, or a session's pairing class. Never a percentage and never a
    /// derived label.
    pub bucket: String,
    /// Routing decisions attributed to a session in this window.
    pub sessions: i64,
    /// Turns those sessions' harnesses reported as completed.
    pub completed: i64,
    /// Turns those sessions' harnesses reported as failed.
    pub failed: i64,
    /// Sessions whose harness never reported a turn end at all — **the
    /// unknown bucket, and it is reported rather than dropped.** A quiet
    /// process is not a failure and an exited one is not a success; a count
    /// that silently omitted these would make every ratio here a fraction of
    /// an unstated denominator.
    pub sessions_without_outcome: i64,
}

impl RouteOutcomeCounts {
    /// The denominator for the success ratio: turns a harness actually
    /// reported on. Never includes [`Self::sessions_without_outcome`].
    pub fn reported_turns(&self) -> i64 {
        self.completed + self.failed
    }
}

/// The three readers this ledger's routing-outcome half adds, kept in their
/// own block so a second worker's reader and this one cannot land on the same
/// lines (practice §77).
impl EvaluationObservations {
    /// The destination id recorded for `session_id`'s routing decision, or
    /// [`None`] when this session has no decision row at all.
    ///
    /// **The `None` is what stops an outcome being invented.** A session
    /// started by an older build, or by a path that never routed, has nothing
    /// for an outcome to be attributed *to*, and
    /// [`record_routing_outcome`] writes nothing for it rather than a row
    /// pointing at no decision.
    ///
    /// `Some("")` is the honest third case — a decision row exists but
    /// recorded no destination — and is deliberately not folded into
    /// [`None`]: one means *nothing was routed*, the other means *something
    /// was, and this ledger cannot say where to*.
    pub fn routed_destination(&self, session_id: &str) -> Result<Option<String>, EvaluationError> {
        let conn = self.lock();
        let found: Option<Option<String>> = conn
            .query_row(
                "SELECT detail
                   FROM evaluation_observations
                  WHERE kind = ?1 AND session_id = ?2
                  ORDER BY seq DESC
                  LIMIT 1",
                params![
                    EvaluationKind::RoutingCostClassObserved.as_str(),
                    session_id
                ],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql_err("read a session's routing decision"))?;
        Ok(found.map(Option::unwrap_or_default))
    }

    /// Routed sessions in `[from, to]`, grouped by the `subject` of their
    /// `decision` row, with what their harnesses reported about their turns —
    /// **map line 1835** with [`EvaluationKind::RoutingCostClassObserved`],
    /// and **map line 1854**'s sparse half with
    /// [`EvaluationKind::RoutingEvidenceObserved`].
    ///
    /// # The window applies to every row counted
    ///
    /// Both the decision and the turn verdicts must fall inside `[from, to]`.
    /// The alternative — decisions in the window, outcomes whenever — makes
    /// the number depend on when it was asked, which is exactly the property
    /// a rate is supposed not to have. A session routed at the very end of
    /// the window therefore appears with no outcome, and appears in
    /// [`RouteOutcomeCounts::sessions_without_outcome`] rather than nowhere.
    ///
    /// # The latest decision per session wins
    ///
    /// `MAX(seq)` with a bare `subject` beside it is SQLite's documented
    /// behaviour — the bare column comes from the row the aggregate selected
    /// — and it is what makes a session that was routed twice count once,
    /// under the class it was last routed to.
    pub fn route_outcomes_by(
        &self,
        decision: EvaluationKind,
        from: i64,
        to: i64,
    ) -> Result<Vec<RouteOutcomeCounts>, EvaluationError> {
        self.refuse_unretained_window(from)?;
        let conn = self.lock();
        let mut statement = conn
            .prepare(
                "WITH decision AS (
                     SELECT session_id AS session_id,
                            subject    AS bucket,
                            MAX(seq)   AS seq
                       FROM evaluation_observations
                      WHERE kind = ?1
                        AND session_id IS NOT NULL
                        AND observed_at >= ?2
                        AND observed_at <= ?3
                      GROUP BY session_id
                 ),
                 verdict AS (
                     SELECT session_id AS session_id,
                            SUM(CASE WHEN subject = ?5 THEN 1 ELSE 0 END) AS completed,
                            SUM(CASE WHEN subject = ?6 THEN 1 ELSE 0 END) AS failed
                       FROM evaluation_observations
                      WHERE kind = ?4
                        AND session_id IS NOT NULL
                        AND observed_at >= ?2
                        AND observed_at <= ?3
                      GROUP BY session_id
                 )
                 SELECT COALESCE(d.bucket, ?7),
                        COUNT(*),
                        COALESCE(SUM(v.completed), 0),
                        COALESCE(SUM(v.failed), 0),
                        SUM(CASE WHEN v.session_id IS NULL THEN 1 ELSE 0 END)
                   FROM decision AS d
                   LEFT JOIN verdict AS v ON v.session_id = d.session_id
                  GROUP BY COALESCE(d.bucket, ?7)
                  ORDER BY COALESCE(d.bucket, ?7)",
            )
            .map_err(sql_err("read routed sessions by decision"))?;
        let rows = statement
            .query_map(
                params![
                    decision.as_str(),
                    from,
                    to,
                    EvaluationKind::RoutingOutcomeObserved.as_str(),
                    TURN_COMPLETED,
                    TURN_FAILED,
                    UNKNOWN_COST_CLASS,
                ],
                read_outcome_row,
            )
            .map_err(sql_err("read routed sessions by decision"))?;
        collect_outcome_counts(rows)
    }

    /// The same counts grouped by the **session's own pairing class** —
    /// **map line 1845**'s *native versus cross-vendor* axis.
    ///
    /// # Why this joins `sessions` instead of storing the class
    ///
    /// `sessions.pairing_class` is written at session creation and is durable
    /// for as long as the session is. Copying it here would be a second
    /// source of truth for a fact this database already holds — the exact
    /// duplication this module's header refuses for memory content, and the
    /// same join [`Self::stale_retrievals`] already makes against `memories`.
    ///
    /// A row whose session is gone, or which predates the column, groups
    /// under `unknown` rather than being dropped.
    pub fn route_outcomes_by_pairing_class(
        &self,
        from: i64,
        to: i64,
    ) -> Result<Vec<RouteOutcomeCounts>, EvaluationError> {
        self.refuse_unretained_window(from)?;
        let conn = self.lock();
        let mut statement = conn
            .prepare(
                "WITH decision AS (
                     SELECT session_id AS session_id,
                            MAX(seq)   AS seq
                       FROM evaluation_observations
                      WHERE kind = ?1
                        AND session_id IS NOT NULL
                        AND observed_at >= ?2
                        AND observed_at <= ?3
                      GROUP BY session_id
                 ),
                 verdict AS (
                     SELECT session_id AS session_id,
                            SUM(CASE WHEN subject = ?5 THEN 1 ELSE 0 END) AS completed,
                            SUM(CASE WHEN subject = ?6 THEN 1 ELSE 0 END) AS failed
                       FROM evaluation_observations
                      WHERE kind = ?4
                        AND session_id IS NOT NULL
                        AND observed_at >= ?2
                        AND observed_at <= ?3
                      GROUP BY session_id
                 )
                 SELECT COALESCE(s.pairing_class, ?7),
                        COUNT(*),
                        COALESCE(SUM(v.completed), 0),
                        COALESCE(SUM(v.failed), 0),
                        SUM(CASE WHEN v.session_id IS NULL THEN 1 ELSE 0 END)
                   FROM decision AS d
                   LEFT JOIN sessions AS s
                          ON s.id = d.session_id AND s.project_id = ?8
                   LEFT JOIN verdict AS v ON v.session_id = d.session_id
                  GROUP BY COALESCE(s.pairing_class, ?7)
                  ORDER BY COALESCE(s.pairing_class, ?7)",
            )
            .map_err(sql_err("read routed sessions by pairing class"))?;
        let rows = statement
            .query_map(
                params![
                    EvaluationKind::RoutingCostClassObserved.as_str(),
                    from,
                    to,
                    EvaluationKind::RoutingOutcomeObserved.as_str(),
                    TURN_COMPLETED,
                    TURN_FAILED,
                    UNKNOWN_COST_CLASS,
                    self.project_id,
                ],
                read_outcome_row,
            )
            .map_err(sql_err("read routed sessions by pairing class"))?;
        collect_outcome_counts(rows)
    }
}

/// Map line 1845's other five quantities — kept in its own block, practice
/// §77, so it cannot land on the same lines as another worker's.
///
/// # Why this reads `routing_observations` directly
///
/// `usable tool calls`, `repair loops`, `effective TTFC` and `reliability`
/// are all facts this ledger's own `route_outcomes_by_pairing_class`
/// (map line 1845's task-success half, above) has no column for — they live
/// on `crate::routing::evidence::RoutingObservation`, in the same physical
/// database file (`crate::database::open`, this struct's own doc comment on
/// [`Self::open`]) but a different table. The register's note stood
/// (`docs/product/evidence/phase-51.md`, *"three producers, not a join"*)
/// until they landed; this is the join, by `session_id`, exactly the shape
/// [`Self::route_outcomes_by_pairing_class`] already uses to reach
/// `sessions.pairing_class` from an evaluation row.
impl EvaluationObservations {
    /// One [`PairingClassResponsiveness`] per pairing class in `[from, to]`
    /// — the five quantities map line 1845 asks for beside task success.
    pub fn pairing_class_responsiveness(
        &self,
        from: i64,
        to: i64,
    ) -> Result<Vec<PairingClassResponsiveness>, EvaluationError> {
        self.refuse_unretained_window(from)?;

        // Half one: every routing-observation row a session in this project
        // recorded in the window, with the session's own pairing class —
        // `usable tool calls`, `repair loops`, `effective TTFC` and
        // `reliability` are all read from this set.
        let rows: Vec<(RoutingObservation, Option<String>)> = {
            let conn = self.lock();
            let mut statement = conn
                .prepare(
                    "SELECT r.*, s.pairing_class AS pairing_class
                       FROM routing_observations AS r
                       JOIN sessions AS s
                         ON s.id = r.session_id AND s.project_id = ?1
                      WHERE r.project_id = ?1 AND r.session_id IS NOT NULL
                        AND r.observed_at >= ?2 AND r.observed_at <= ?3",
                )
                .map_err(sql_err("read routing observations by pairing class"))?;
            let mapped = statement
                .query_map(params![self.project_id, from, to], |row| {
                    let pairing_class: Option<String> = row.get("pairing_class")?;
                    Ok((row_to_observation(row)?, pairing_class))
                })
                .map_err(sql_err("read routing observations by pairing class"))?;
            let mut rows = Vec::new();
            for row in mapped {
                let (observation, pairing_class) =
                    row.map_err(sql_err("read one routing observation by pairing class"))?;
                rows.push((
                    observation.map_err(|err| {
                        sql_err("decode one routing observation by pairing class")(
                            rusqlite::Error::ToSqlConversionFailure(Box::new(err)),
                        )
                    })?,
                    pairing_class,
                ));
            }
            rows
        };

        let mut by_class: std::collections::BTreeMap<String, Vec<RoutingObservation>> =
            std::collections::BTreeMap::new();
        for (observation, pairing_class) in rows {
            by_class
                .entry(pairing_class.unwrap_or_else(|| UNKNOWN_COST_CLASS.to_owned()))
                .or_default()
                .push(observation);
        }

        // Half two: this project's decisions per class (the same `decision`
        // count [`Self::route_outcomes_by_pairing_class`] reports as
        // `sessions`) and, of those, how many carry an overridden
        // [`EvaluationKind::RoutingOverrideDecided`] row — map line 1845's
        // `user overrides`.
        let (decisions, overridden): (
            std::collections::BTreeMap<String, i64>,
            std::collections::BTreeMap<String, i64>,
        ) = {
            let conn = self.lock();
            let mut statement = conn
                .prepare(
                    "WITH decision AS (
                         SELECT session_id AS session_id, MAX(seq) AS seq
                           FROM evaluation_observations
                          WHERE kind = ?1
                            AND session_id IS NOT NULL
                            AND observed_at >= ?2
                            AND observed_at <= ?3
                          GROUP BY session_id
                     ),
                     overridden AS (
                         SELECT DISTINCT session_id
                           FROM evaluation_observations
                          WHERE kind = ?4 AND subject = ?5
                            AND session_id IS NOT NULL
                            AND observed_at >= ?2
                            AND observed_at <= ?3
                     )
                     SELECT COALESCE(s.pairing_class, ?6),
                            COUNT(*),
                            SUM(CASE WHEN o.session_id IS NOT NULL THEN 1 ELSE 0 END)
                       FROM decision AS d
                       LEFT JOIN sessions AS s
                              ON s.id = d.session_id AND s.project_id = ?7
                       LEFT JOIN overridden AS o ON o.session_id = d.session_id
                      GROUP BY COALESCE(s.pairing_class, ?6)",
                )
                .map_err(sql_err("read decisions and overrides by pairing class"))?;
            let rows = statement
                .query_map(
                    params![
                        EvaluationKind::RoutingCostClassObserved.as_str(),
                        from,
                        to,
                        EvaluationKind::RoutingOverrideDecided.as_str(),
                        "overridden",
                        UNKNOWN_COST_CLASS,
                        self.project_id,
                    ],
                    |row| {
                        let bucket: String = row.get(0)?;
                        let decisions: i64 = row.get(1)?;
                        let overridden: i64 = row.get(2)?;
                        Ok((bucket, decisions, overridden))
                    },
                )
                .map_err(sql_err("read decisions and overrides by pairing class"))?;
            let mut decisions_by_class = std::collections::BTreeMap::new();
            let mut overridden_by_class = std::collections::BTreeMap::new();
            for row in rows {
                let (bucket, decisions, overridden) =
                    row.map_err(sql_err("read one pairing class's decisions and overrides"))?;
                decisions_by_class.insert(bucket.clone(), decisions);
                overridden_by_class.insert(bucket, overridden);
            }
            (decisions_by_class, overridden_by_class)
        };

        // Every bucket either half named — a class with rows but no
        // decision in this exact window (or the reverse) still gets a line,
        // honestly zero on the side it has nothing for, rather than being
        // dropped.
        let mut buckets: std::collections::BTreeSet<String> = by_class.keys().cloned().collect();
        buckets.extend(decisions.keys().cloned());

        Ok(buckets
            .into_iter()
            .map(|bucket| {
                let group = by_class.get(&bucket).cloned().unwrap_or_default();

                let mut tool_rounds_recorded = 0usize;
                let mut tool_rounds_positive = 0usize;
                let mut repairs_sum: i64 = 0;
                let mut repairs_sample = 0usize;
                for observation in &group {
                    if let Some(rounds) = observation.tool_rounds {
                        tool_rounds_recorded += 1;
                        if rounds > 0 {
                            tool_rounds_positive += 1;
                        }
                    }
                    if let Some(repairs) = observation.repairs {
                        repairs_sum += repairs;
                        repairs_sample += 1;
                    }
                }
                let usable_tool_calls = (tool_rounds_recorded >= MIN_SAMPLE_FOR_SUMMARY)
                    .then(|| tool_rounds_positive as f64 / tool_rounds_recorded as f64);
                let repair_loops = (repairs_sample >= MIN_SAMPLE_FOR_SUMMARY)
                    .then(|| repairs_sum as f64 / repairs_sample as f64);

                let responsiveness = RouteResponsiveness::from_observations(&group);
                let reliability = responsiveness.failure_rate.map(|p| 1.0 - p);

                let class_decisions = decisions.get(&bucket).copied().unwrap_or(0);
                let class_overridden = overridden.get(&bucket).copied().unwrap_or(0);
                let user_overrides = (class_decisions as usize >= MIN_SAMPLE_FOR_SUMMARY)
                    .then(|| class_overridden as f64 / class_decisions as f64);

                PairingClassResponsiveness {
                    bucket,
                    decisions: class_decisions,
                    usable_tool_calls,
                    usable_tool_calls_sample: tool_rounds_recorded,
                    repair_loops,
                    repair_loops_sample: repairs_sample,
                    effective_ttfc_ms: responsiveness.effective_ttfc_ms(),
                    effective_ttfc_sample: responsiveness.raw_ttfc_sample,
                    reliability,
                    reliability_sample: responsiveness.failure_rate_sample,
                    user_overrides,
                    user_overrides_sample: class_decisions,
                }
            })
            .collect())
    }
}

/// [`EvaluationObservations::pairing_class_responsiveness`]'s result — map
/// line 1845's other five quantities, one row per pairing class. Every
/// figure carries its own sample count and is `None` — *not enough* — below
/// [`MIN_SAMPLE_FOR_SUMMARY`], the same floor
/// [`RouteOutcomeCounts::reported_turns`]'s own task-success half sits
/// behind through [`RouteResponsiveness`].
#[derive(Debug, Clone, PartialEq)]
pub struct PairingClassResponsiveness {
    pub bucket: String,
    /// This class's routed sessions in the window — the same count
    /// [`EvaluationObservations::route_outcomes_by_pairing_class`] reports
    /// as `sessions`, and [`Self::user_overrides`]'s own denominator.
    pub decisions: i64,
    pub usable_tool_calls: Option<f64>,
    pub usable_tool_calls_sample: usize,
    pub repair_loops: Option<f64>,
    pub repair_loops_sample: usize,
    pub effective_ttfc_ms: Option<f64>,
    pub effective_ttfc_sample: usize,
    pub reliability: Option<f64>,
    pub reliability_sample: usize,
    pub user_overrides: Option<f64>,
    pub user_overrides_sample: i64,
}

/// Map line 1480's own reader — kept in its own block, practice §77, so it
/// cannot land on the same lines as another worker's.
impl EvaluationObservations {
    /// [`Self::route_outcomes_by`]'s existing join
    /// ([`EvaluationKind::RoutingTierObserved`]), with a verdict per tier
    /// instead of a raw count — **map line 1480**, distinct from map line
    /// 1834's raw table: 1834 asks what was recorded, 1480 asks whether
    /// enough of it exists to say how a tier is doing.
    ///
    /// **No new number.** This reuses the join `route_outcomes_by` already
    /// performs rather than duplicating its SQL, and applies
    /// [`crate::routing::evidence::MIN_SAMPLE_FOR_SUMMARY`] — the ledger's
    /// one existing answer to "when enough evidence exists" — to
    /// [`RouteOutcomeCounts::reported_turns`], the count a success-or-failure
    /// summary is actually made from. A session with a tier row and no
    /// outcome row is [`TierOutcome::undecided`] and is never part of that
    /// count and never read as a failure.
    pub fn outcomes_by_tier(
        &self,
        from: i64,
        to: i64,
    ) -> Result<Vec<TierOutcome>, EvaluationError> {
        let counts = self.route_outcomes_by(EvaluationKind::RoutingTierObserved, from, to)?;
        Ok(counts.into_iter().map(TierOutcome::from_counts).collect())
    }
}

/// One [`EvaluationObservations::outcomes_by_tier`] row — map line 1480.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TierOutcome {
    /// The tier-and-escalation bucket — [`RoutingTier::as_str`]'s closed
    /// vocabulary, or `unclassified`, read back as the stored string and
    /// never decoded into [`RoutingTier`] itself (the same rule
    /// [`RoutingEvidence`]'s own doc comment gives for a stored vocabulary
    /// word). Escalated and non-escalated tiers are distinct words in this
    /// vocabulary, so they are distinct buckets here too.
    pub bucket: String,
    /// Sessions whose harness never reported a turn end for this tier —
    /// counted on its own, never folded into a failure.
    pub undecided: i64,
    /// Whether this tier has enough reported turns to summarize, and what
    /// the summary says when it does.
    pub verdict: TierOutcomeVerdict,
}

impl TierOutcome {
    fn from_counts(counts: RouteOutcomeCounts) -> Self {
        let sample_size = counts.reported_turns();
        let verdict = if sample_size < MIN_SAMPLE_FOR_SUMMARY as i64 {
            TierOutcomeVerdict::InsufficientEvidence {
                sample_size,
                required: MIN_SAMPLE_FOR_SUMMARY,
            }
        } else {
            TierOutcomeVerdict::Measured {
                successful: counts.completed,
                failed: counts.failed,
                sample_size,
            }
        };
        Self {
            bucket: counts.bucket,
            undecided: counts.sessions_without_outcome,
            verdict,
        }
    }
}

/// What [`EvaluationObservations::outcomes_by_tier`] answers for one tier —
/// gated the way
/// [`crate::routing::evidence::RouteCorrelation::verdict`] gates a route
/// pair (map line 1376's rule, reused rather than re-invented).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TierOutcomeVerdict {
    /// Fewer than [`crate::routing::evidence::MIN_SAMPLE_FOR_SUMMARY`]
    /// reported turns for this tier. Carries the count so a reader prints
    /// *2 of 5* rather than *unknown*.
    InsufficientEvidence { sample_size: i64, required: usize },
    /// Enough reported turns to summarize successful and failed outcomes.
    Measured {
        successful: i64,
        failed: i64,
        sample_size: i64,
    },
}

/// Map line 1951's own reader — kept in its own block, practice §77, so it
/// cannot land on the same lines as another worker's.
impl EvaluationObservations {
    /// [`Self::outcomes_by_tier`]'s join, with a harness dimension added —
    /// **map line 1951**'s outcome-and-task-class half. `sessions.harness`
    /// is joined the same way [`Self::route_outcomes_by_pairing_class`]
    /// joins `sessions.pairing_class`: a session whose row is gone, or which
    /// predates the join, groups under [`UNKNOWN_HARNESS`] rather than being
    /// dropped, and the tier bucket keeps [`Self::outcomes_by_tier`]'s own
    /// fallback and gate — `TierOutcome::from_counts` is reused unchanged so
    /// the two readers cannot drift on what counts as enough evidence.
    pub fn outcomes_by_tier_and_harness(
        &self,
        from: i64,
        to: i64,
    ) -> Result<Vec<HarnessTierOutcome>, EvaluationError> {
        self.refuse_unretained_window(from)?;
        let conn = self.lock();
        let mut statement = conn
            .prepare(
                "WITH decision AS (
                     SELECT session_id AS session_id,
                            subject    AS bucket,
                            MAX(seq)   AS seq
                       FROM evaluation_observations
                      WHERE kind = ?1
                        AND session_id IS NOT NULL
                        AND observed_at >= ?2
                        AND observed_at <= ?3
                      GROUP BY session_id
                 ),
                 verdict AS (
                     SELECT session_id AS session_id,
                            SUM(CASE WHEN subject = ?5 THEN 1 ELSE 0 END) AS completed,
                            SUM(CASE WHEN subject = ?6 THEN 1 ELSE 0 END) AS failed
                       FROM evaluation_observations
                      WHERE kind = ?4
                        AND session_id IS NOT NULL
                        AND observed_at >= ?2
                        AND observed_at <= ?3
                      GROUP BY session_id
                 )
                 SELECT COALESCE(s.harness, ?8),
                        COALESCE(d.bucket, ?7),
                        COUNT(*),
                        COALESCE(SUM(v.completed), 0),
                        COALESCE(SUM(v.failed), 0),
                        SUM(CASE WHEN v.session_id IS NULL THEN 1 ELSE 0 END)
                   FROM decision AS d
                   LEFT JOIN sessions AS s
                          ON s.id = d.session_id AND s.project_id = ?9
                   LEFT JOIN verdict AS v ON v.session_id = d.session_id
                  GROUP BY COALESCE(s.harness, ?8), COALESCE(d.bucket, ?7)
                  ORDER BY COALESCE(s.harness, ?8), COALESCE(d.bucket, ?7)",
            )
            .map_err(sql_err("read routed sessions by harness and tier"))?;
        let rows = statement
            .query_map(
                params![
                    EvaluationKind::RoutingTierObserved.as_str(),
                    from,
                    to,
                    EvaluationKind::RoutingOutcomeObserved.as_str(),
                    TURN_COMPLETED,
                    TURN_FAILED,
                    UNKNOWN_COST_CLASS,
                    UNKNOWN_HARNESS,
                    self.project_id,
                ],
                read_harness_outcome_row,
            )
            .map_err(sql_err("read routed sessions by harness and tier"))?;
        let mut out = Vec::new();
        for row in rows {
            let (harness, counts) =
                row.map_err(sql_err("decode a routed-session count by harness"))?;
            out.push(HarnessTierOutcome {
                harness,
                outcome: TierOutcome::from_counts(counts),
            });
        }
        Ok(out)
    }
}

/// One [`EvaluationObservations::outcomes_by_tier_and_harness`] row — map
/// line 1951's outcome half: which harness, which task class (the tier
/// bucket [`TierOutcome::bucket`] already carries), and the same verdict
/// [`EvaluationObservations::outcomes_by_tier`] computes for the tier alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessTierOutcome {
    pub harness: String,
    pub outcome: TierOutcome,
}

fn read_harness_outcome_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<(String, RouteOutcomeCounts)> {
    Ok((
        row.get(0)?,
        RouteOutcomeCounts {
            bucket: row.get(1)?,
            sessions: row.get(2)?,
            completed: row.get(3)?,
            failed: row.get(4)?,
            sessions_without_outcome: row.get(5)?,
        },
    ))
}

/// The one reader whose kind carries no `session_id` — map line 1851's
/// counts, kept in this block for practice §77's reason, the same as the
/// three above it.
impl EvaluationObservations {
    /// How many rows of `kind` fall in `[from, to]`, by `subject`, in the
    /// stored vocabulary and sorted by it.
    ///
    /// **A count and its own denominator, not a ratio.** The caller sums the
    /// buckets to get the total it divides by, so a bucket that is missing
    /// from this project's history is visibly missing rather than silently a
    /// zero in a fraction nobody can check.
    ///
    /// A row with no `subject` groups under [`UNKNOWN_COST_CLASS`], the same
    /// third bucket every other reader here uses, rather than being dropped.
    pub fn counts_by_subject(
        &self,
        kind: EvaluationKind,
        from: i64,
        to: i64,
    ) -> Result<Vec<(String, i64)>, EvaluationError> {
        self.refuse_unretained_window(from)?;
        let conn = self.lock();
        let mut statement = conn
            .prepare(
                "SELECT COALESCE(subject, ?4), COUNT(*)
                   FROM evaluation_observations
                  WHERE kind = ?1
                    AND observed_at >= ?2
                    AND observed_at <= ?3
                  GROUP BY COALESCE(subject, ?4)
                  ORDER BY COALESCE(subject, ?4)",
            )
            .map_err(sql_err("count observations by subject"))?;
        let rows = statement
            .query_map(
                params![kind.as_str(), from, to, UNKNOWN_COST_CLASS],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(sql_err("count observations by subject"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(sql_err("decode a count by subject"))
    }
}

/// One [`RouteOutcomeCounts`] row, in the column order both queries above
/// select.
fn read_outcome_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RouteOutcomeCounts> {
    Ok(RouteOutcomeCounts {
        bucket: row.get(0)?,
        sessions: row.get(1)?,
        completed: row.get(2)?,
        failed: row.get(3)?,
        sessions_without_outcome: row.get(4)?,
    })
}

fn collect_outcome_counts<I>(rows: I) -> Result<Vec<RouteOutcomeCounts>, EvaluationError>
where
    I: Iterator<Item = rusqlite::Result<RouteOutcomeCounts>>,
{
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(sql_err("decode a routed-session count"))
}

/// The column list every read of this table selects, in the order
/// [`read_observation_row`] decodes them.
///
/// Spelled once so [`EvaluationObservations::recent`] and
/// [`EvaluationObservations::recent_of_kind`] cannot drift into two column
/// orders that both compile and decode each other's fields.
const OBSERVATION_COLUMNS: &str = "seq, observed_at, kind, outcome, subject, session_id, \
                                   feature, arm, memory_id, routing_seq, detail";

/// One row of [`OBSERVATION_COLUMNS`], still in the vocabulary the database
/// stores rather than this build's enums.
type StoredObservation = (
    i64,
    i64,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<String>,
);

fn read_observation_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredObservation> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
    ))
}

/// Decode every row, refusing a stored `kind` or `outcome` this build does not
/// know rather than bucketing it into a neighbour.
fn collect_observations<I>(rows: I) -> Result<Vec<EvaluationObservation>, EvaluationError>
where
    I: Iterator<Item = rusqlite::Result<StoredObservation>>,
{
    let mut out = Vec::new();
    for row in rows {
        let (
            seq,
            observed_at,
            kind,
            outcome,
            subject,
            session_id,
            feature,
            arm,
            memory_id,
            routing_seq,
            detail,
        ) = row.map_err(sql_err("decode an evaluation observation"))?;
        out.push(EvaluationObservation {
            seq,
            observed_at,
            kind: EvaluationKind::from_stored(&kind).ok_or(EvaluationError::UnknownKind {
                seq,
                value: kind.clone(),
            })?,
            outcome: EvaluationOutcome::from_stored(&outcome).ok_or(
                EvaluationError::UnknownValue {
                    seq,
                    column: "outcome",
                    value: outcome.clone(),
                },
            )?,
            subject,
            session_id,
            feature,
            arm,
            memory_id,
            routing_seq,
            detail,
        });
    }
    Ok(out)
}

/// The retention `DELETE`, oldest-first, on a connection already inside a
/// transaction.
///
/// Both bounds in one statement so a row that violates either goes in one
/// pass. `seq <= MAX(seq) - max_rows` keeps the newest `max_rows` rows, and
/// `AUTOINCREMENT` guarantees a deleted `seq` is never reused — which is what
/// makes this ledger safe to point at even though it is pruned.
fn trim_within(
    tx: &rusqlite::Transaction<'_>,
    retention: Retention,
    now_unix: i64,
) -> Result<usize, EvaluationError> {
    let cutoff = now_unix.saturating_sub(retention.max_age_secs);
    tx.execute(
        "DELETE FROM evaluation_observations
          WHERE observed_at < ?1
             OR seq <= (SELECT MAX(seq) FROM evaluation_observations) - ?2",
        params![cutoff, retention.max_rows],
    )
    .map_err(sql_err("trim the evaluation ledger"))
}

/// Record that a memory search handed these memories back — the producer for
/// map lines 1822 and 1826, and — when `session_id` is carried — the
/// [`EvaluationKind::MemoryRetrieved`] half of map lines 1821 and 1831's own
/// proxy join (this reader block's own doc comment names the other half).
///
/// **This never fails a retrieval.** Memory search is on the user's path and
/// bookkeeping is not allowed to break it, so every error here is a
/// `tracing::warn!` and a return: the caller gets its results whether or not
/// the ledger could be written.
///
/// The database handle is opened here, and only here, and only when there is
/// something to record — practice §65's rule that a resource is acquired where
/// its consumer starts. A search that returned nothing opens nothing.
///
/// `session_id` is `None` whenever the caller has no session in scope —
/// never guessed. `GH-RETRIEVAL-ATTRIBUTION`'s two production callers today:
/// `main.rs::memory_search_grouped` passes `None` from the CLI's `memory
/// search` (no session to attribute a person's own command to) and from
/// `api::unix::query_memory` (the machine door's `QueryMemory` request
/// carries no session field to thread one from — see that caller's own doc
/// comment); `api::unix::deliver_memory` passes `Some` on every successful
/// launch-time injection, because that door already holds the `SessionId`
/// it is briefing.
pub fn record_memory_retrieval<'a>(
    runtime: &Runtime,
    scope: RetrievalScope,
    memory_ids: impl IntoIterator<Item = &'a str>,
    session_id: Option<&str>,
    observed_at_unix: i64,
) {
    let observations: Vec<NewObservation> = memory_ids
        .into_iter()
        .map(|id| {
            let mut observation = NewObservation::new(EvaluationKind::MemoryRetrieved)
                .with_subject(scope.as_str())
                .with_memory_id(id);
            if let Some(session_id) = session_id {
                observation = observation.with_session_id(session_id);
            }
            observation
        })
        .collect();
    if observations.is_empty() {
        return;
    }

    let ledger = match EvaluationObservations::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "could not open the evaluation ledger; the retrieval stands, \
                 but it was not counted"
            );
            return;
        }
    };
    if let Err(err) = ledger.record_all(&observations, observed_at_unix) {
        tracing::warn!(
            error = %err,
            "could not record a memory retrieval; the retrieval stands, but it \
             was not counted"
        );
    }
}

/// Record that a memory search on a production door matched nothing at all —
/// the miss counterpart of [`record_memory_retrieval`], and the producer map
/// line 1865 needs: *"do not add vector retrieval until FTS5 retrieval
/// failures are observed and recorded in real projects."*
///
/// **This never fails a search or a launch**, for the same reason
/// [`record_memory_retrieval`] does not: bookkeeping is not allowed to break
/// the door it is counting. Every error here is a `tracing::warn!` and a
/// return.
///
/// The database handle is opened here, and only here — practice §65's rule
/// that a resource is acquired where its consumer starts, applied to a door
/// that returned nothing rather than one that returned something. Every
/// caller of this function must have already dropped its memory connection
/// before calling it, for the same reason [`record_memory_retrieval`]'s own
/// callers do.
pub fn record_memory_retrieval_miss(
    runtime: &Runtime,
    scope: RetrievalScope,
    observed_at_unix: i64,
) {
    let ledger = match EvaluationObservations::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "could not open the evaluation ledger; the retrieval miss was \
                 not counted"
            );
            return;
        }
    };
    let observation =
        NewObservation::new(EvaluationKind::MemoryRetrievalMiss).with_subject(scope.as_str());
    if let Err(err) = ledger.record(observation, observed_at_unix) {
        tracing::warn!(
            error = %err,
            "could not record a memory retrieval miss"
        );
    }
}

/// Record the rationale behind one disposable-job routing decision — the
/// producer for [`EvaluationKind::DisposableRouteDecided`].
///
/// **This never fails a turn.** Its one caller is `glasshouse hook`, which
/// runs inside the user's coding session and whose non-zero exit Claude Code
/// treats as a veto on the user's prompt (see `main.rs::report_hook`). So
/// every error here is a `tracing::warn!` and a return, exactly as
/// [`record_memory_retrieval`] is, and for a sharper version of the same
/// reason: a retrieval that went uncounted cost a count, and a turn that went
/// unsent costs the user their words.
///
/// The handle is opened here, and only here, and only when there is something
/// to record — practice §65's rule that a resource is acquired where its
/// consumer starts. A decision with nothing to say about itself opens no
/// database.
///
/// # What is stored, and what is left absent
///
/// `subject` is the job kind's own name and `detail` is `rationale` verbatim:
/// the string the routing decision produced, not a re-derivation of it. The
/// caller passes what production already renders, so what the ledger holds is
/// what the decision said.
///
/// `routing_seq` is **absent, and stays absent.** This path makes no
/// `routing_observations` row — the disposable policy calls no model, so
/// there is no exchange to measure — and a `seq` pointing at some other
/// turn's measurement would be worse than no provenance at all. Map line
/// 1294's standing refusal is the rule: *a fabricated value here does not
/// degrade the policy, it inverts it.* `memory_id`, `feature` and `arm` are
/// absent for the same reason: this decision is about none of them.
pub fn record_disposable_route(
    runtime: &Runtime,
    job: crate::routing::disposable::JobKind,
    session_id: &str,
    rationale: &str,
    observed_at_unix: i64,
) {
    if rationale.trim().is_empty() {
        return;
    }

    let observation = NewObservation::new(EvaluationKind::DisposableRouteDecided)
        .with_subject(job.as_str())
        .with_session_id(session_id)
        .with_detail(rationale);

    let ledger = match EvaluationObservations::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "could not open the evaluation ledger; the routing decision stands, but its \
                 rationale was not recorded"
            );
            return;
        }
    };
    if let Err(err) = ledger.record(observation, observed_at_unix) {
        tracing::warn!(
            error = %err,
            "could not record a disposable routing decision; the decision stands, but its \
             rationale was not recorded"
        );
    }
}

/// Encode `explanation`'s contributions as a compact JSON array of
/// `{"name", "magnitude", "evidence"}`, by hand.
///
/// **No general-purpose serializer here, deliberately.** This module's own
/// header says so: *"no `export`, no `to_json`, no `write_to`, no
/// serialization of an observation to anything outside the process"* — map
/// line 1856's other half, structural rather than advisory, and this
/// module's own pinning test fails the build the moment such a dependency
/// reappears here. `detail` is still a JSON string, because 1766 needs to
/// rank contributions by magnitude and a rendered sentence cannot be ranked
/// — but this ledger writes it itself rather than reaching for a crate
/// whose surface is far wider than one array of three fields.
fn encode_route_contributions(explanation: &crate::routing::RoutingExplanation) -> String {
    let mut out = String::from("[");
    for (index, contribution) in explanation.contributions().iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str("{\"name\":");
        push_json_string(&mut out, contribution.name());
        out.push_str(",\"magnitude\":");
        push_json_number(&mut out, contribution.magnitude());
        out.push_str(",\"evidence\":");
        push_json_string(&mut out, contribution.evidence());
        out.push('}');
    }
    out.push(']');
    out
}

/// A JSON number cannot spell NaN or an infinity; a routing score never
/// produces either, but a value that somehow did degrades to `0` rather
/// than writing a `detail` [`route_contributions`] could not parse back.
fn push_json_number(out: &mut String, value: f64) {
    if value.is_finite() {
        out.push_str(&value.to_string());
    } else {
        out.push('0');
    }
}

/// A JSON string literal, escaped by hand — the same six escapes
/// [`route_contributions`]'s reader decodes.
fn push_json_string(out: &mut String, value: &str) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Record why a launch's session-boundary routing chose the destination it
/// did — the producer for [`EvaluationKind::SessionRouteDecided`], map lines
/// 1757 and 1766.
///
/// Its callers are `main.rs::launch_session`'s same two routed exits
/// [`record_routed_session`] has — called right beside it, with the same
/// `session_id` and the same `observed_at_unix`.
///
/// **This never fails a launch**, exactly as [`record_routed_session`] does
/// not: it is on a person's own command path and a rationale row is not
/// worth a session.
///
/// # What is stored
///
/// `subject` is `destination_id`. `detail` is `explanation.contributions()`
/// as a compact JSON array of `{name, magnitude, evidence}`, in the
/// explanation's own order — built through this module's own
/// `encode_route_contributions`, never through `routing`'s own
/// [`crate::routing::RoutingExplanation::render`], because 1766 ranks by
/// magnitude and a rendered string cannot be ranked. An explanation with no
/// contributions still writes a row, `detail` `"[]"`: the decision happened
/// even when nothing weighed in.
pub fn record_session_route(
    runtime: &Runtime,
    session_id: &str,
    destination_id: &str,
    explanation: &crate::routing::RoutingExplanation,
    observed_at_unix: i64,
) {
    let detail = encode_route_contributions(explanation);

    let observation = NewObservation::new(EvaluationKind::SessionRouteDecided)
        .with_subject(destination_id)
        .with_session_id(session_id)
        .with_detail(detail);

    let ledger = match EvaluationObservations::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "could not open the evaluation ledger; the session is routed, but its rationale \
                 was not recorded"
            );
            return;
        }
    };
    if let Err(err) = ledger.record(observation, observed_at_unix) {
        tracing::warn!(
            error = %err,
            "could not record a session's routing rationale; the session is routed, but its \
             rationale will not be shown"
        );
    }
}

/// Record a launch's own expected output-token size for the session it
/// produced — the producer for
/// [`EvaluationKind::RoutingConsumptionEstimated`], map line 1855's token
/// half.
///
/// Its callers are `main.rs::launch_session`'s same two routed exits
/// [`record_session_route`] has — called right beside it, with the same
/// `session_id` and the same `observed_at_unix`.
///
/// **Written only when there is a real median to write.** `median_output_tokens`
/// is the caller's own
/// [`crate::routing::burn::ClassOutput::median_output_tokens`] for this
/// launch's task class, already `None` below
/// [`crate::routing::evidence::MIN_SAMPLE_FOR_SUMMARY`] comparable rows — a
/// launch with no comparable rows for its class calls this with nothing to
/// write, and this records nothing rather than a fabricated zero. This
/// function does not re-derive the median itself: the caller already read
/// it from the same evidence ledger this row is about, and re-reading it
/// here would be a second, possibly different, read of the same window.
///
/// **This never fails a launch**, exactly as [`record_session_route`] does
/// not: it is on a person's own command path and an estimate row is not
/// worth a session.
///
/// # What is stored
///
/// `subject` is `task_class.as_str()`. `detail` is `median_output_tokens`,
/// rounded to the nearest whole token, as decimal text — never a raw float
/// string, so [`crate::routing::evidence::EvidenceLedger::output_estimate_accuracy`]
/// can parse it back without a locale-dependent format. `session_id` is the
/// session the decision produced.
pub fn record_routing_consumption_estimate(
    runtime: &Runtime,
    session_id: &str,
    task_class: crate::routing::request::TaskClass,
    median_output_tokens: f64,
    observed_at_unix: i64,
) {
    let observation = NewObservation::new(EvaluationKind::RoutingConsumptionEstimated)
        .with_subject(task_class.as_str())
        .with_session_id(session_id)
        .with_detail(format!("{}", median_output_tokens.round() as i64));

    let ledger = match EvaluationObservations::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "could not open the evaluation ledger; the session is routed, but its expected \
                 output-token size was not recorded"
            );
            return;
        }
    };
    if let Err(err) = ledger.record(observation, observed_at_unix) {
        tracing::warn!(
            error = %err,
            "could not record a session's expected output-token size"
        );
    }
}

/// Record one launch's session-boundary routing decision — the producer for
/// [`EvaluationKind::RoutingOverrideDecided`] and
/// [`EvaluationKind::RoutingContinuationDecided`], map lines 1829 and 1830.
///
/// **This never fails a launch.** Its one caller is
/// `main.rs::launch_session`, on the person's own command path, so every
/// error here is a `tracing::warn!` and a return, exactly as
/// [`record_disposable_route`] is.
///
/// The handle is opened here, and only here, and only when there is a routed
/// decision to record — practice §65's rule that a resource is acquired
/// where its consumer starts.
///
/// # What is stored, and what is left absent
///
/// Two rows, always together: `destination_id` and `fresh` are known the
/// instant a destination is chosen, so neither one is ever the "nothing
/// meaningful to say" case the way an empty rationale is for
/// [`record_disposable_route`]. `subject` carries the boolean-shaped fact
/// each line asks about and `detail` carries a destination id — never a file
/// path, prompt text, or credential.
///
/// `session_id` is left absent on both rows. A launch that continues an
/// existing session could name it, but a fresh launch has not minted one yet
/// at this point in `launch_session`, and a producer that filled the field on
/// one branch and not the other would make its absence look like a fact
/// about the decision rather than about when the row was written.
pub fn record_routing_decision(
    runtime: &Runtime,
    destination_id: &str,
    fresh: bool,
    overrode: Option<&str>,
    observed_at_unix: i64,
) {
    let mut override_observation = NewObservation::new(EvaluationKind::RoutingOverrideDecided)
        .with_subject(if overrode.is_some() {
            "overridden"
        } else {
            "automatic"
        });
    if let Some(automatic) = overrode {
        override_observation = override_observation.with_detail(automatic);
    }

    let continuation_observation = NewObservation::new(EvaluationKind::RoutingContinuationDecided)
        .with_subject(if fresh { "fresh" } else { "existing" })
        .with_detail(destination_id);

    let observations = [override_observation, continuation_observation];

    let ledger = match EvaluationObservations::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "could not open the evaluation ledger; the routing decision stands, but it was \
                 not counted"
            );
            return;
        }
    };
    if let Err(err) = ledger.record_all(&observations, observed_at_unix) {
        tracing::warn!(
            error = %err,
            "could not record a routing decision; the decision stands, but it was not counted"
        );
    }
}

/// Capability map line 1463 — how many routing decisions were made per
/// interactive hour, with both numbers beside the ratio so the ratio can
/// never be read without its denominators.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingDecisionRate {
    /// [`EvaluationKind::RoutingContinuationDecided`] rows in the window —
    /// one per launch that reached a routing decision
    /// ([`record_routing_decision`] writes exactly one per launch).
    pub decisions: i64,
    /// Distinct wall-clock hours in the window during which at least one
    /// session record shows activity — see [`interactive_hours`] for the
    /// derivation.
    pub interactive_hours: usize,
    /// `(from, to)`, in Unix seconds.
    pub window: (i64, i64),
}

impl RoutingDecisionRate {
    /// Decisions per interactive hour, or `None` when the window holds no
    /// interactive hour at all — a rate over zero hours is not a rate.
    pub fn per_hour(&self) -> Option<f64> {
        (self.interactive_hours > 0).then(|| self.decisions as f64 / self.interactive_hours as f64)
    }
}

/// How many distinct wall-clock hours inside `[from, to]` at least one of
/// `spans` touches — the "interactive hour" capability map line 1463 divides
/// by, derived from session records rather than from the clock alone.
///
/// A span is one session's `(created_at, last_activity_at)`, both in Unix
/// seconds; an hour is an epoch-aligned bucket of 3600 seconds, and a span
/// that touches a bucket at all counts it — a session that was active for
/// one minute of an hour makes that an interactive hour, which is the
/// reading a person would give it. A span outside the window contributes
/// nothing; a span partly inside is clipped to it. Counting wall-clock
/// hours instead would say a project that ran one session on Monday and
/// none since is making decisions at a vanishing rate all week, which is
/// the fabrication this derivation exists to avoid.
pub fn interactive_hours(spans: impl IntoIterator<Item = (i64, i64)>, from: i64, to: i64) -> usize {
    let mut hours = std::collections::BTreeSet::new();
    for (start, end) in spans {
        let start = start.max(from);
        let end = end.min(to);
        if end < start {
            continue;
        }
        hours.extend(start.div_euclid(3600)..=end.div_euclid(3600));
    }
    hours.len()
}

/// The decisions-per-interactive-hour reader — capability map line 1463 —
/// kept in its own `impl` block beside the writers rather than among the
/// other counts, because it joins two stores: this ledger's count and the
/// session store's activity spans, which the caller supplies so this module
/// opens nothing it does not own.
impl EvaluationObservations {
    /// [`RoutingDecisionRate`] over `[from, to]`, dividing this ledger's
    /// [`EvaluationKind::RoutingContinuationDecided`] count by the
    /// [`interactive_hours`] `spans` cover in the same window.
    pub fn routing_decision_rate(
        &self,
        spans: impl IntoIterator<Item = (i64, i64)>,
        from: i64,
        to: i64,
    ) -> Result<RoutingDecisionRate, EvaluationError> {
        let decisions = self.count(EvaluationKind::RoutingContinuationDecided, from, to)?;
        Ok(RoutingDecisionRate {
            decisions,
            interactive_hours: interactive_hours(spans, from, to),
            window: (from, to),
        })
    }

    /// The newest [`EvaluationKind::SessionRouteDecided`] row for one
    /// session — [`Self::recent_of_kind`] narrowed by `session_id` too, for
    /// `sessions show`'s `routing rationale` block, map line 1757.
    ///
    /// `Ok(None)` is a session with no row — started before this build, or
    /// spawned through the machine door, which is not routed — and the
    /// caller renders that as `-`, never as an error.
    pub fn session_route_for(
        &self,
        session_id: &str,
    ) -> Result<Option<EvaluationObservation>, EvaluationError> {
        let conn = self.lock();
        let mut statement = conn
            .prepare(&format!(
                "SELECT {OBSERVATION_COLUMNS}
                   FROM evaluation_observations
                  WHERE kind = ?1 AND session_id = ?2
                  ORDER BY seq DESC
                  LIMIT 1"
            ))
            .map_err(sql_err("read a session's routing rationale"))?;
        let rows = statement
            .query_map(
                params![EvaluationKind::SessionRouteDecided.as_str(), session_id],
                read_observation_row,
            )
            .map_err(sql_err("read a session's routing rationale"))?;
        Ok(collect_observations(rows)?.into_iter().next())
    }

    /// The newest [`EvaluationKind::SessionRouteDecided`] row in the
    /// project, for `status`'s one-line summary, map line 1766.
    ///
    /// `Ok(None)` is a project with no routed launch yet, rendered as
    /// *none recorded*.
    pub fn latest_session_route(&self) -> Result<Option<EvaluationObservation>, EvaluationError> {
        Ok(self
            .recent_of_kind(EvaluationKind::SessionRouteDecided, 1)?
            .into_iter()
            .next())
    }
}

/// One contribution decoded from a [`EvaluationKind::SessionRouteDecided`]
/// row's `detail`.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordedContribution {
    pub name: String,
    pub magnitude: f64,
    pub evidence: String,
}

/// Parse a [`EvaluationKind::SessionRouteDecided`] row's `detail` back into
/// [`RecordedContribution`]s, in the order they were recorded.
///
/// **Tolerates a malformed or absent `detail` by returning an empty list.**
/// This is a reader dressing up a row for a person, not a validator: a row
/// damaged some other way should render as "no factors" rather than crash
/// `sessions show` or `status`.
///
/// Hand-written, like this module's own `encode_route_contributions` that
/// writes what this reads — this module's own header keeps a
/// general-purpose serializer out of this file entirely, and its pinning
/// test enforces that.
pub fn route_contributions(detail: &str) -> Vec<RecordedContribution> {
    parse_route_contributions(detail).unwrap_or_default()
}

/// A position in `detail`, addressed by `char` rather than by byte, so a
/// multi-byte character in a contribution's evidence never splits — the
/// small price of collecting into a `Vec<char>` up front, paid once per row
/// this reader ever decodes.
struct JsonCursor {
    chars: Vec<char>,
    pos: usize,
}

impl JsonCursor {
    fn new(s: &str) -> Self {
        JsonCursor {
            chars: s.chars().collect(),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(c) if c.is_whitespace()) {
            self.pos += 1;
        }
    }

    fn expect(&mut self, want: char) -> Option<()> {
        if self.peek() == Some(want) {
            self.pos += 1;
            Some(())
        } else {
            None
        }
    }
}

fn parse_json_string(cursor: &mut JsonCursor) -> Option<String> {
    cursor.expect('"')?;
    let mut out = String::new();
    loop {
        match cursor.bump()? {
            '"' => return Some(out),
            '\\' => match cursor.bump()? {
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                '/' => out.push('/'),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'u' => {
                    let mut hex = String::with_capacity(4);
                    for _ in 0..4 {
                        hex.push(cursor.bump()?);
                    }
                    out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                }
                _ => return None,
            },
            other => out.push(other),
        }
    }
}

fn parse_json_number(cursor: &mut JsonCursor) -> Option<f64> {
    let start = cursor.pos;
    if cursor.peek() == Some('-') {
        cursor.pos += 1;
    }
    while matches!(cursor.peek(), Some(c) if c.is_ascii_digit()) {
        cursor.pos += 1;
    }
    if cursor.peek() == Some('.') {
        cursor.pos += 1;
        while matches!(cursor.peek(), Some(c) if c.is_ascii_digit()) {
            cursor.pos += 1;
        }
    }
    if matches!(cursor.peek(), Some('e') | Some('E')) {
        cursor.pos += 1;
        if matches!(cursor.peek(), Some('+') | Some('-')) {
            cursor.pos += 1;
        }
        while matches!(cursor.peek(), Some(c) if c.is_ascii_digit()) {
            cursor.pos += 1;
        }
    }
    if cursor.pos == start {
        return None;
    }
    cursor.chars[start..cursor.pos]
        .iter()
        .collect::<String>()
        .parse::<f64>()
        .ok()
}

fn parse_route_contribution_object(cursor: &mut JsonCursor) -> Option<RecordedContribution> {
    cursor.expect('{')?;
    let mut name = None;
    let mut magnitude = None;
    let mut evidence = None;
    loop {
        cursor.skip_ws();
        if cursor.peek() == Some('}') {
            cursor.pos += 1;
            break;
        }
        let key = parse_json_string(cursor)?;
        cursor.skip_ws();
        cursor.expect(':')?;
        cursor.skip_ws();
        match key.as_str() {
            "name" => name = Some(parse_json_string(cursor)?),
            "evidence" => evidence = Some(parse_json_string(cursor)?),
            "magnitude" => magnitude = Some(parse_json_number(cursor)?),
            // A field this reader does not name yet: skip its value, a
            // string or a number, rather than refusing the whole row for a
            // field it does not need.
            _ if cursor.peek() == Some('"') => {
                parse_json_string(cursor)?;
            }
            _ => {
                parse_json_number(cursor)?;
            }
        }
        cursor.skip_ws();
        match cursor.bump()? {
            ',' => continue,
            '}' => break,
            _ => return None,
        }
    }
    Some(RecordedContribution {
        name: name?,
        magnitude: magnitude?,
        evidence: evidence?,
    })
}

fn parse_route_contributions(detail: &str) -> Option<Vec<RecordedContribution>> {
    let mut cursor = JsonCursor::new(detail);
    cursor.skip_ws();
    cursor.expect('[')?;
    cursor.skip_ws();
    let mut out = Vec::new();
    if cursor.peek() == Some(']') {
        cursor.pos += 1;
        return Some(out);
    }
    loop {
        cursor.skip_ws();
        out.push(parse_route_contribution_object(&mut cursor)?);
        cursor.skip_ws();
        match cursor.bump()? {
            ',' => continue,
            ']' => break,
            _ => return None,
        }
    }
    Some(out)
}

/// Seconds since the Unix epoch, the way every other store in this crate reads
/// the clock.
pub fn now_unix() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(elapsed) => i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX),
        Err(_) => 0,
    }
}

/// The `subject` a completed turn is recorded under —
/// [`EvaluationKind::RoutingOutcomeObserved`]'s vocabulary, spelled once so
/// the writer below and the two readers above cannot drift apart.
const TURN_COMPLETED: &str = "completed";
/// The `subject` a failed turn is recorded under.
const TURN_FAILED: &str = "failed";

/// Which of the two [`crate::events::TurnOutcome`] a row records.
///
/// An exhaustive `match` at the single writer, for
/// [`EvaluationKind`]'s own reason: a third outcome added to that enum must
/// be a compile error here rather than a row silently recorded as one of the
/// two that already exist.
fn turn_subject(outcome: crate::events::TurnOutcome) -> &'static str {
    match outcome {
        crate::events::TurnOutcome::Completed => TURN_COMPLETED,
        crate::events::TurnOutcome::Failed => TURN_FAILED,
    }
}

/// Attribute a launch's routing decision to the session it produced — the
/// producer for [`EvaluationKind::RoutingCostClassObserved`] and
/// [`EvaluationKind::RoutingEvidenceObserved`], map lines 1835 and 1854.
///
/// Its callers are `main.rs::launch_session`'s two routed exits: the branch
/// that continues a warm session, where the destination's id *is* the session
/// id, and the branch that creates a fresh session record, called once that
/// record exists and its id is real.
///
/// **This never fails a launch**, exactly as [`record_routing_decision`]
/// never does, and for the same reason: it is on a person's own command path
/// and an evaluation row is not worth a session.
///
/// # What is stored
///
/// `cost` is [`None`] when no production fact states the destination's class
/// — a harness's own sign-in has no configured provider and no marked model —
/// and that is recorded as [`UNKNOWN_COST_CLASS`], its own bucket in every
/// reader here. `evidence` is whether the pool the router was handed held a
/// reading for this destination, which is the only thing about the router's
/// inputs that can be stated on this path.
///
/// Both rows carry ids and vocabulary words and nothing else: a destination
/// id, a session id, and one word from a closed list.
pub fn record_routed_session(
    runtime: &Runtime,
    session_id: &str,
    destination_id: &str,
    cost: Option<crate::routing::Cost>,
    evidence: RoutingEvidence,
    tier: RoutingTier,
    observed_at_unix: i64,
) {
    let class = NewObservation::new(EvaluationKind::RoutingCostClassObserved)
        .with_subject(cost.map_or(UNKNOWN_COST_CLASS, |cost| cost.as_str()))
        .with_session_id(session_id)
        .with_detail(destination_id);
    let evidence = NewObservation::new(EvaluationKind::RoutingEvidenceObserved)
        .with_subject(evidence.as_str())
        .with_session_id(session_id)
        .with_detail(destination_id);
    // Map line 1834's third row, written in the same call and therefore
    // through the same one handle: a tier that reached the ledger a moment
    // later would be a second open on a person's own launch path, which is
    // the whole of practice §65's finding.
    let mut tier_row = NewObservation::new(EvaluationKind::RoutingTierObserved)
        .with_subject(tier.as_str())
        .with_session_id(session_id);
    if let Some(stated) = tier.stated_tier() {
        tier_row = tier_row.with_detail(stated.as_str());
    }

    let ledger = match EvaluationObservations::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "could not open the evaluation ledger; the session is routed, but its route \
                 was not attributed to it"
            );
            return;
        }
    };
    if let Err(err) = ledger.record_all(&[class, evidence, tier_row], observed_at_unix) {
        tracing::warn!(
            error = %err,
            "could not attribute a route to its session; the session is routed, but its route \
             will not be counted"
        );
    }
}

/// Record what the harness said about one turn of a routed session — the
/// producer for [`EvaluationKind::RoutingOutcomeObserved`], and the outcome
/// half of map lines 1834, 1835, 1845 and 1854.
///
/// Its one caller is `main.rs`'s `glasshouse hook` handler, on the arm that
/// has already translated the harness's event into
/// [`crate::events::LifecycleEvent::TurnEnded`]. **Nothing else may call it**,
/// because nothing else in this build holds a verdict a harness actually
/// stated: a process exit, output ending and a session going idle are all
/// silence, and silence is not an outcome.
///
/// # A session with no routing decision records nothing
///
/// [`EvaluationObservations::routed_destination`] answering [`None`] means
/// this session was never attributed to a route — it predates this build, or
/// it was created by a path that does not route — and there is nothing for an
/// outcome to be *about*. That is a `debug` line and no row, never a row
/// whose decision is invented.
///
/// # One handle, opened here, dropped here (practice §65)
///
/// The hook is a separate process the harness spawns on every event, and an
/// open SQLite handle is free on the developer's machine and billed on
/// Windows. The lookup and the write share the one handle this function
/// opens, and it is opened only after the caller has established that a turn
/// really ended.
pub fn record_routing_outcome(
    runtime: &Runtime,
    session_id: &str,
    outcome: crate::events::TurnOutcome,
    observed_at_unix: i64,
) {
    let ledger = match EvaluationObservations::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "could not open the evaluation ledger; the turn ended, but its outcome was \
                 not counted"
            );
            return;
        }
    };
    let destination = match ledger.routed_destination(session_id) {
        Ok(Some(destination)) => destination,
        Ok(None) => {
            tracing::debug!(
                session = session_id,
                "no routing decision is recorded for this session, so its turn outcome is \
                 not attributed to one"
            );
            return;
        }
        Err(err) => {
            tracing::warn!(
                error = %err,
                "could not read this session's routing decision; its turn outcome was not \
                 counted"
            );
            return;
        }
    };

    let mut observation = NewObservation::new(EvaluationKind::RoutingOutcomeObserved)
        .with_subject(turn_subject(outcome))
        .with_session_id(session_id);
    if !destination.is_empty() {
        observation = observation.with_detail(destination);
    }
    if let Err(err) = ledger.record(observation, observed_at_unix) {
        tracing::warn!(
            error = %err,
            "could not record a turn's outcome; the turn ended, but it was not counted"
        );
    }
}

/// Record what the harness said about one turn of **any** session that runs
/// the hook — the producer for [`EvaluationKind::TurnOutcomeObserved`], and
/// map lines 1821 and 1831's proxy denominator.
///
/// Its one caller is `main.rs`'s `glasshouse hook` handler, on the same
/// `TurnEnded` arm [`record_routing_outcome`] reads — called first, so a
/// session this ledger has never routed still gets an outcome row.
///
/// # Unlike `record_routing_outcome`, this asks no question about routing
///
/// [`record_routing_outcome`] refuses to write for a session with no routed
/// destination because that row is a claim about *the route*. This row
/// makes no claim about a route at all — it is the harness's verdict on the
/// session's turn, full stop — so it is written unconditionally, a
/// door-spawned session (never routed) included. Design ruling: refusal
/// register, *"Phase 51's memory proxy — 1821 and 1831"*, option (b).
///
/// # One handle, opened here, dropped here (practice §65)
///
/// Same reasoning as [`record_routing_outcome`]: the hook is a separate
/// process the harness spawns on every event, and the write shares the one
/// handle this function opens.
pub fn record_turn_outcome(
    runtime: &Runtime,
    session_id: &str,
    outcome: crate::events::TurnOutcome,
    observed_at_unix: i64,
) {
    let ledger = match EvaluationObservations::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "could not open the evaluation ledger; the turn ended, but its outcome was \
                 not counted"
            );
            return;
        }
    };
    let observation = NewObservation::new(EvaluationKind::TurnOutcomeObserved)
        .with_subject(turn_subject(outcome))
        .with_session_id(session_id);
    if let Err(err) = ledger.record(observation, observed_at_unix) {
        tracing::warn!(
            error = %err,
            "could not record a turn's outcome; the turn ended, but it was not counted"
        );
    }
}

/// Record what the failure-domain term did to one gateway failover's ranking
/// — the producer for [`EvaluationKind::FailoverPrevented`], **map line
/// 1851**.
///
/// Its one caller is the sink `main.rs::launch_session` hands the gateway,
/// invoked from the exchange thread that ranked the failover. Nothing else
/// may call it: the comparison it records can only be made where both
/// rankings exist, which is inside
/// [`crate::routing::interactive::InteractiveRouting::on_provider_failure`],
/// and a row written from anywhere else would be an assertion rather than an
/// observation.
///
/// # One handle, opened here, dropped here (practice §65)
///
/// This runs on a gateway exchange thread inside somebody's coding session.
/// The handle is opened only once a failover has actually been decided —
/// which is a small minority of exchanges — and closed before this returns,
/// so no connection is held across the provider hop and none is opened at all
/// by the exchanges that fail over nowhere.
///
/// **This never fails an exchange.** Every error is one `warn` and a return,
/// exactly as [`record_routed_session`] and [`record_routing_outcome`] do,
/// and for the same reason: the session's own work outranks the books kept
/// about it.
pub fn record_failover_prevention(
    runtime: &Runtime,
    prevention: FailoverPrevention,
    displaced: Option<&str>,
    observed_at_unix: i64,
) {
    let mut observation =
        NewObservation::new(EvaluationKind::FailoverPrevented).with_subject(prevention.as_str());
    if let Some(displaced) = displaced {
        observation = observation.with_detail(displaced);
    }
    let ledger = match EvaluationObservations::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "could not open the evaluation ledger; the failover happened, but what the \
                 failure-domain term did to it was not counted"
            );
            return;
        }
    };
    if let Err(err) = ledger.record(observation, observed_at_unix) {
        tracing::warn!(
            error = %err,
            "could not record what the failure-domain term did to a failover"
        );
    }
}

/// Record a person's or an agent's own verdict on a memory Glasshouse
/// retrieved — the producer for [`EvaluationKind::MemoryRated`], and
/// `glasshouse memory rate`'s one write. Returns the appended `seq`.
///
/// # This is allowed to fail loudly, unlike every producer above
///
/// [`record_memory_retrieval`] and its neighbours never fail a search or a
/// launch, because bookkeeping must not break the door it is counting. This
/// producer has no door to protect: it *is* the command, typed by a person
/// or issued by an agent as its own last act, and a rating that silently
/// failed to record would tell its caller their verdict was kept when it
/// was not. Its caller (`main.rs::memory_rate`) propagates this with `?` and
/// prints nothing but a failure.
///
/// `memory_id` is trusted to have already been resolved against this
/// project's own store — `glasshouse memory rate`'s project-isolation check
/// runs before this is ever called, the same way `memory_challenge` and
/// `memory_resolve_conflict` resolve an id before acting on it.
///
/// **Carries the scope of the retrieval it judges — map line 939.** Before
/// writing, this looks up the [`RetrievalScope`] of the retrieval the
/// rating is about (`EvaluationObservations`'s own private attribution
/// lookup) and copies it onto the row's own `subject`, so `false positives by
/// retrieval scope` can be read out per scope rather than only per memory.
/// Every verdict is attributed the same way — the scope is a fact about
/// which retrieval produced the memory being rated, not a judgement the
/// verdict itself makes. A memory this rating never saw retrieved carries no
/// scope. **A lookup failure fails the command exactly as a write failure
/// does** — this producer has no door to protect, per this function's own
/// header above.
pub fn record_memory_rating(
    runtime: &Runtime,
    memory_id: &str,
    verdict: EvaluationOutcome,
    session_id: Option<&str>,
    note: Option<&str>,
    observed_at_unix: i64,
) -> anyhow::Result<i64> {
    let ledger = EvaluationObservations::open(runtime)?;
    let scope = ledger.most_recent_retrieval_scope(memory_id, session_id)?;

    let mut observation = NewObservation::new(EvaluationKind::MemoryRated)
        .with_memory_id(memory_id)
        .with_outcome(verdict);
    if let Some(scope) = scope {
        observation = observation.with_subject(scope);
    }
    if let Some(session_id) = session_id {
        observation = observation.with_session_id(session_id);
    }
    if let Some(note) = note {
        observation = observation.with_detail(note);
    }
    Ok(ledger.record(observation, observed_at_unix)?)
}

/// Record that `glasshouse memory revalidate` ran — the producer for
/// [`EvaluationKind::MemoryRevalidated`], map line 1824's own denominator.
/// Its one caller (`main.rs::memory_revalidate`) calls this after the store
/// has already written the outcome, so a ledger failure here can never leave
/// a revalidation half-applied.
///
/// **Never fails the command**, the same shape [`record_memory_retrieval`]
/// and its neighbours use rather than [`record_memory_rating`]'s: the store
/// mutation is the real act and has already succeeded by the time this runs,
/// so a bookkeeping error here must not turn a successful `memory revalidate`
/// into a failed command exit.
///
/// `outcome` is the CLI's own word (`reaffirmed`, `needs-review`,
/// `superseded` or `invalidated`), stored verbatim as `subject` — this
/// producer does not judge whether the revalidation was correct, only that
/// it happened.
pub fn record_memory_revalidation(
    runtime: &Runtime,
    memory_id: &str,
    outcome: &str,
    observed_at_unix: i64,
) {
    let observation = NewObservation::new(EvaluationKind::MemoryRevalidated)
        .with_memory_id(memory_id)
        .with_subject(outcome);
    let ledger = match EvaluationObservations::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "could not open the evaluation ledger; the revalidation stands, but it was not \
                 counted"
            );
            return;
        }
    };
    if let Err(err) = ledger.record(observation, observed_at_unix) {
        tracing::warn!(
            error = %err,
            "could not record that a memory revalidation happened"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Rust vocabulary and the constant beside the schema must agree.
    ///
    /// `LIFECYCLE_EVENT_KINDS`' own doc comment says why this pin is the real
    /// guarantee rather than the `CHECK`: a renamed variant otherwise compiles
    /// perfectly and fails as a constraint violation somewhere nobody is
    /// looking. Migration 15 has no `CHECK` at all, so this pin is the *only*
    /// guarantee, which makes it load-bearing rather than belt-and-braces.
    #[test]
    fn every_kind_the_type_can_produce_is_one_the_schema_constant_declares() {
        let declared = [
            EvaluationKind::MemoryRetrieved,
            EvaluationKind::MemoryRetrievalMiss,
            EvaluationKind::DisposableRouteDecided,
            EvaluationKind::RoutingOverrideDecided,
            EvaluationKind::RoutingContinuationDecided,
            EvaluationKind::RoutingCostClassObserved,
            EvaluationKind::RoutingEvidenceObserved,
            EvaluationKind::RoutingOutcomeObserved,
            EvaluationKind::RoutingTierObserved,
            EvaluationKind::FailoverPrevented,
            EvaluationKind::MemoryRated,
            EvaluationKind::MemoryRevalidated,
            EvaluationKind::TurnOutcomeObserved,
            EvaluationKind::SessionRouteDecided,
            EvaluationKind::RoutingConsumptionEstimated,
        ];
        let names: Vec<&str> = declared.iter().map(|kind| kind.as_str()).collect();
        assert_eq!(
            names.as_slice(),
            EVALUATION_KINDS.as_slice(),
            "an evaluation kind was added or renamed without the constant \
             beside migration 15"
        );
        for name in EVALUATION_KINDS {
            assert!(
                EvaluationKind::from_stored(name).is_some(),
                "`{name}` is declared beside the schema and cannot be decoded"
            );
        }
    }

    #[test]
    fn an_unrecognized_stored_value_decodes_to_nothing_rather_than_a_neighbour() {
        assert!(EvaluationKind::from_stored("route_preferred").is_none());
        assert!(EvaluationOutcome::from_stored("helped").is_none());
    }

    /// `glasshouse memory rate`'s vocabulary, spelled once — [`EvaluationKind::MemoryRated`]
    /// and [`MEMORY_RATING_VERDICTS`]' eight words round-trip through
    /// `as_str`/`from_stored`, and `Unknown` is not one of them: it is the
    /// sentinel every other kind writes for "not yet known", never a verdict
    /// a person types.
    #[test]
    fn memory_rated_and_its_verdict_vocabulary_round_trip() {
        assert_eq!(
            EvaluationKind::from_stored(EvaluationKind::MemoryRated.as_str()),
            Some(EvaluationKind::MemoryRated)
        );
        for verdict in MEMORY_RATING_VERDICTS {
            assert_eq!(
                EvaluationOutcome::from_stored(verdict.as_str()),
                Some(verdict),
                "`{}` must round-trip",
                verdict.as_str()
            );
            assert_ne!(verdict, EvaluationOutcome::Unknown);
        }
        assert_eq!(MEMORY_RATING_VERDICTS.len(), 8);
    }

    #[test]
    fn the_shipped_retention_is_ninety_days_and_a_hundred_thousand_rows() {
        assert_eq!(Retention::DEFAULT.max_age_secs, 7_776_000);
        assert_eq!(Retention::DEFAULT.max_rows, 100_000);
        assert_eq!(Retention::DEFAULT.trim_every, 256);
        assert_eq!(Retention::default(), Retention::DEFAULT);
    }

    #[test]
    fn the_history_flag_and_the_subject_vocabulary_are_the_same_distinction() {
        assert_eq!(
            RetrievalScope::from_history_flag(true),
            RetrievalScope::Historical
        );
        assert_eq!(
            RetrievalScope::from_history_flag(false),
            RetrievalScope::Current
        );
        assert_eq!(RetrievalScope::Historical.as_str(), "historical");
        assert_eq!(RetrievalScope::Current.as_str(), "current");
    }

    /// The briefing door's own scope, distinct from both search scopes so a
    /// miss row names which door produced it.
    #[test]
    fn the_injection_scope_is_its_own_word_not_current() {
        assert_eq!(RetrievalScope::Injection.as_str(), "injection");
        assert_ne!(
            RetrievalScope::Injection.as_str(),
            RetrievalScope::Current.as_str()
        );
    }
}
