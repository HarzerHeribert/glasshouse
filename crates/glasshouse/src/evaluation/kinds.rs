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
    /// the hook — map lines 1821 and 1831's proxy denominator. `subject` is
    /// `"completed"` or `"failed"`, from [`crate::events::TurnOutcome`].
    ///
    /// Written for every session that reaches the hook's `TurnEnded` arm.
    /// The memory-quality readers (1821, 1831) join a session-attributed
    /// retrieval to this row to tell whether the retrieving session's turn
    /// completed.
    ///
    /// History: design-decisions.md, "Trims: config, checkpoint, evaluation and codex module docs", kinds.rs `EvaluationKind::TurnOutcomeObserved`.
    TurnOutcomeObserved,
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

impl EvaluationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MemoryRetrieved => "memory_retrieved",
            Self::MemoryRetrievalMiss => "memory_retrieval_miss",
            Self::MemoryRated => "memory_rated",
            Self::MemoryRevalidated => "memory_revalidated",
            Self::TurnOutcomeObserved => "turn_outcome_observed",
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
            "memory_rated" => Some(Self::MemoryRated),
            "memory_revalidated" => Some(Self::MemoryRevalidated),
            "turn_outcome_observed" => Some(Self::TurnOutcomeObserved),
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
