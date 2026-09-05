use rusqlite::{OptionalExtension, params};

use crate::harness::pairing::PairingClass;
use crate::routing::evidence::{MIN_SAMPLE_FOR_SUMMARY, UNKNOWN_HARNESS};

use super::{
    EvaluationError, EvaluationKind, EvaluationObservation, EvaluationObservations,
    EvaluationOutcome, RetrievalScope, TURN_COMPLETED, TURN_FAILED, UNKNOWN_COST_CLASS, sql_err,
};

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

impl EvaluationObservations {
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
    /// [`crate::evaluation::NewObservation::with_session_id`] when its caller knows one, so a
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
    /// [`crate::evaluation::record_memory_rating`] is attributing this rating to — map line
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
    pub(super) fn most_recent_retrieval_scope(
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
/// [`crate::evaluation::record_routing_outcome`] refuses to write anything for a session with no
/// prior routed destination, and only `main.rs::launch_session` (the CLI
/// `glasshouse launch` path) ever calls [`crate::evaluation::record_routed_session`] — the
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
    /// ([`crate::evaluation::record_memory_rating`]'s attribution lookup found nothing).
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
    /// an unstated denominator. Never includes a session that was rated:
    /// [`Self::rated_useful`] and [`Self::rated_not_useful`] hold those.
    pub sessions_without_outcome: i64,
    /// Sessions in this bucket whose **latest** [`EvaluationKind::RoutingRated`]
    /// row carries [`EvaluationOutcome::Useful`] — map line 1846's design
    /// note, *"The routing half of RC-B"* (2026-09-05). Counted apart from
    /// [`Self::completed`], never summed into it: an explicit rating
    /// **replaces** the [`EvaluationKind::RoutingOutcomeObserved`] proxy for
    /// that session rather than adding to it, so [`Self::completed`] and
    /// [`Self::failed`] exclude every session counted here.
    pub rated_useful: i64,
    /// The same rule for [`EvaluationOutcome::NotUseful`].
    pub rated_not_useful: i64,
}

impl RouteOutcomeCounts {
    /// The denominator for the success ratio: turns a harness actually
    /// reported on. Never includes [`Self::sessions_without_outcome`] or
    /// either rated count.
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
    /// [`crate::evaluation::record_routing_outcome`] writes nothing for it rather than a row
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
                 ),
                 rating AS (
                     SELECT session_id AS session_id,
                            outcome    AS outcome,
                            MAX(seq)   AS seq
                       FROM evaluation_observations
                      WHERE kind = ?8
                        AND session_id IS NOT NULL
                        AND observed_at >= ?2
                        AND observed_at <= ?3
                      GROUP BY session_id
                 )
                 SELECT COALESCE(d.bucket, ?7),
                        COUNT(*),
                        COALESCE(SUM(CASE WHEN r.session_id IS NULL THEN v.completed ELSE 0 END), 0),
                        COALESCE(SUM(CASE WHEN r.session_id IS NULL THEN v.failed ELSE 0 END), 0),
                        SUM(CASE WHEN r.session_id IS NULL AND v.session_id IS NULL THEN 1 ELSE 0 END),
                        SUM(CASE WHEN r.outcome = ?9 THEN 1 ELSE 0 END),
                        SUM(CASE WHEN r.outcome = ?10 THEN 1 ELSE 0 END)
                   FROM decision AS d
                   LEFT JOIN verdict AS v ON v.session_id = d.session_id
                   LEFT JOIN rating AS r ON r.session_id = d.session_id
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
                    EvaluationKind::RoutingRated.as_str(),
                    EvaluationOutcome::Useful.as_str(),
                    EvaluationOutcome::NotUseful.as_str(),
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
                 ),
                 rating AS (
                     SELECT session_id AS session_id,
                            outcome    AS outcome,
                            MAX(seq)   AS seq
                       FROM evaluation_observations
                      WHERE kind = ?9
                        AND session_id IS NOT NULL
                        AND observed_at >= ?2
                        AND observed_at <= ?3
                      GROUP BY session_id
                 )
                 SELECT COALESCE(s.pairing_class, ?7),
                        COUNT(*),
                        COALESCE(SUM(CASE WHEN r.session_id IS NULL THEN v.completed ELSE 0 END), 0),
                        COALESCE(SUM(CASE WHEN r.session_id IS NULL THEN v.failed ELSE 0 END), 0),
                        SUM(CASE WHEN r.session_id IS NULL AND v.session_id IS NULL THEN 1 ELSE 0 END),
                        SUM(CASE WHEN r.outcome = ?10 THEN 1 ELSE 0 END),
                        SUM(CASE WHEN r.outcome = ?11 THEN 1 ELSE 0 END)
                   FROM decision AS d
                   LEFT JOIN sessions AS s
                          ON s.id = d.session_id AND s.project_id = ?8
                   LEFT JOIN verdict AS v ON v.session_id = d.session_id
                   LEFT JOIN rating AS r ON r.session_id = d.session_id
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
                    EvaluationKind::RoutingRated.as_str(),
                    EvaluationOutcome::Useful.as_str(),
                    EvaluationOutcome::NotUseful.as_str(),
                ],
                read_outcome_row,
            )
            .map_err(sql_err("read routed sessions by pairing class"))?;
        collect_outcome_counts(rows)
    }
}

/// **Map line 1846**'s own question, kept in its own block (practice §77) so
/// it cannot land on the same lines as another worker's: from what point
/// does local pairing evidence predict a routed session's outcome at least
/// as well as the same-vendor prior (`routing::session::PAIRING_PRIOR`) did
/// before any local evidence existed.
///
/// **This measures the prior's predictiveness; it never re-tunes it.**
/// Nothing here reads or writes `routing::session::PAIRING_PRIOR` or
/// `routing::session::PAIRING_PRIOR_EVIDENCE_THRESHOLD` (both private to
/// that module) — the design note behind this line (*"The routing half of
/// RC-B"*, 2026-09-05) has the prior stand regardless of what this
/// comparison finds; it exists for a person reading the numbers to argue
/// with, not for this reader to act on.
impl EvaluationObservations {
    /// One row per session in `[from, to]` with a routing decision, a
    /// pairing class, and an outcome — the latest
    /// [`EvaluationKind::RoutingRated`] verdict when one exists, the
    /// [`EvaluationKind::RoutingOutcomeObserved`] proxy otherwise, the same
    /// substitution [`Self::route_outcomes_by`] makes — bucketed by how much
    /// earlier evidence *of that session's own pairing class* this project
    /// already held when it was routed.
    ///
    /// # `k` counts a class's own history, never the window
    ///
    /// A session's `k` is the number of its pairing class's earlier sessions
    /// with an outcome, ordered by when each was routed — never this
    /// session's position in `[from, to]` as a whole.
    /// `routing::session::PAIRING_PRIOR` is a per-destination decay, and a
    /// bucket keyed on anything else would answer a different question than
    /// the one line 1846 asks.
    ///
    /// # `k = 0` is scored as wrong, never skipped
    ///
    /// A session with no earlier same-class evidence has nothing for a local
    /// success rate to be computed from. Excluding it from the count would
    /// let an empty bucket claim it "agrees" with itself; scoring it as a
    /// wrong local prediction is the honest reading, and the one the
    /// packet's own contract requires.
    ///
    /// # Pure SQL, plus a small in-memory ordering pass
    ///
    /// The rows this needs — a session's decision timestamp, its pairing
    /// class, its rated or proxied outcome — are one query, the same join
    /// shape [`Self::route_outcomes_by_pairing_class`] already runs.
    /// Ordering them per class and walking each class's history is
    /// arithmetic no `GROUP BY` expresses, so it happens here instead of in
    /// a second query.
    pub fn pairing_prior_crossover(
        &self,
        from: i64,
        to: i64,
    ) -> Result<PairingCrossover, EvaluationError> {
        self.refuse_unretained_window(from)?;
        let rows: Vec<(String, i64, String, i64, i64, Option<String>)> = {
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
                     decision_row AS (
                         SELECT d.session_id AS session_id, o.observed_at AS decided_at
                           FROM decision AS d
                           JOIN evaluation_observations AS o
                             ON o.kind = ?1 AND o.session_id = d.session_id AND o.seq = d.seq
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
                     ),
                     rating AS (
                         SELECT session_id AS session_id, outcome AS outcome, MAX(seq) AS seq
                           FROM evaluation_observations
                          WHERE kind = ?7
                            AND session_id IS NOT NULL
                            AND observed_at >= ?2
                            AND observed_at <= ?3
                          GROUP BY session_id
                     )
                     SELECT d.session_id, dr.decided_at, COALESCE(s.pairing_class, ?9),
                            COALESCE(v.completed, 0), COALESCE(v.failed, 0), r.outcome
                       FROM decision AS d
                       JOIN decision_row AS dr ON dr.session_id = d.session_id
                       LEFT JOIN sessions AS s ON s.id = d.session_id AND s.project_id = ?8
                       LEFT JOIN verdict AS v ON v.session_id = d.session_id
                       LEFT JOIN rating AS r ON r.session_id = d.session_id",
                )
                .map_err(sql_err("read sessions for the pairing-prior crossover"))?;
            let mapped = statement
                .query_map(
                    params![
                        EvaluationKind::RoutingCostClassObserved.as_str(),
                        from,
                        to,
                        EvaluationKind::RoutingOutcomeObserved.as_str(),
                        TURN_COMPLETED,
                        TURN_FAILED,
                        EvaluationKind::RoutingRated.as_str(),
                        self.project_id,
                        UNKNOWN_COST_CLASS,
                    ],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, i64>(3)?,
                            row.get::<_, i64>(4)?,
                            row.get::<_, Option<String>>(5)?,
                        ))
                    },
                )
                .map_err(sql_err("read sessions for the pairing-prior crossover"))?;
            let mut rows = Vec::new();
            for row in mapped {
                rows.push(
                    row.map_err(sql_err("read one session for the pairing-prior crossover"))?,
                );
            }
            rows
        };

        // One vector per pairing class, in the order each session was
        // routed — the history [`Self`]'s local prediction walks.
        let mut by_class: std::collections::BTreeMap<String, Vec<(i64, String, bool)>> =
            std::collections::BTreeMap::new();
        for (session_id, decided_at, pairing_class, completed, failed, rating) in rows {
            let success =
                if let Some(outcome) = rating.as_deref().and_then(EvaluationOutcome::from_stored) {
                    match outcome {
                        EvaluationOutcome::Useful => true,
                        EvaluationOutcome::NotUseful => false,
                        // `ROUTE_RATING_VERDICTS` never writes another value —
                        // treated as no rating rather than guessed at.
                        _ => continue,
                    }
                } else if completed > 0 || failed > 0 {
                    completed > failed
                } else {
                    // No rating and no reported turn: nothing for an outcome to
                    // be. Not evidence, not scored, not counted toward `k`.
                    continue;
                };
            by_class
                .entry(pairing_class)
                .or_default()
                .push((decided_at, session_id, success));
        }

        let mut buckets = [
            CrossoverBucket::new("0-4"),
            CrossoverBucket::new("5-9"),
            CrossoverBucket::new("10-19"),
            CrossoverBucket::new("20+"),
        ];

        for (pairing_class, mut group) in by_class {
            // Earliest-routed first; the session id breaks a tie so two
            // decisions landing in the same second stay deterministic.
            group.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
            let prior_predicts_success = pairing_class == PairingClass::VendorNative.slug();
            let mut earlier_successes = 0usize;
            for (k, (_, _, success)) in group.into_iter().enumerate() {
                let prior_correct = prior_predicts_success == success;
                // k = 0: no history to predict from, scored wrong outright.
                let local_correct =
                    k > 0 && (earlier_successes as f64 / k as f64 >= 0.5) == success;

                let bucket = &mut buckets[bucket_index(k)];
                bucket.sessions += 1;
                if prior_correct {
                    bucket.prior_correct += 1;
                }
                if local_correct {
                    bucket.local_correct += 1;
                }

                if success {
                    earlier_successes += 1;
                }
            }
        }

        let crossover = buckets
            .iter()
            .find(|bucket| {
                bucket.sessions >= MIN_SAMPLE_FOR_SUMMARY as i64
                    && bucket.local_correct >= bucket.prior_correct
            })
            .map(|bucket| bucket.bucket);

        Ok(PairingCrossover {
            buckets: buckets.to_vec(),
            crossover,
        })
    }
}

/// The k-bucket [`EvaluationObservations::pairing_prior_crossover`] groups
/// on — the four widths the packet's own contract fixes: `0-4`, `5-9`,
/// `10-19` and `20+`.
fn bucket_index(k: usize) -> usize {
    match k {
        0..=4 => 0,
        5..=9 => 1,
        10..=19 => 2,
        _ => 3,
    }
}

/// One [`EvaluationObservations::pairing_prior_crossover`] bucket: how many
/// sessions carried that much prior same-class evidence, and how often each
/// of the two predictions — the same-vendor prior's and the local success
/// rate's — matched what actually happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrossoverBucket {
    /// `0-4`, `5-9`, `10-19` or `20+` — never a derived label.
    pub bucket: &'static str,
    pub sessions: i64,
    pub prior_correct: i64,
    pub local_correct: i64,
}

impl CrossoverBucket {
    fn new(bucket: &'static str) -> Self {
        Self {
            bucket,
            sessions: 0,
            prior_correct: 0,
            local_correct: 0,
        }
    }
}

/// [`EvaluationObservations::pairing_prior_crossover`]'s result — map line
/// 1846's own question, answered as a bucket table plus the first bucket (if
/// any) where local evidence caught up to the prior.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PairingCrossover {
    /// Always the four buckets in order, sessions zero where nothing landed
    /// there — never a sparse list a renderer must fill in itself.
    pub buckets: Vec<CrossoverBucket>,
    /// The first bucket, in order, with at least
    /// [`crate::routing::evidence::MIN_SAMPLE_FOR_SUMMARY`] sessions where
    /// local evidence was at least as often correct as the prior. [`None`]
    /// when no bucket qualifies yet.
    pub crossover: Option<&'static str>,
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
    /// The tier-and-escalation bucket — [`crate::evaluation::RoutingTier::as_str`]'s closed
    /// vocabulary, or `unclassified`, read back as the stored string and
    /// never decoded into [`crate::evaluation::RoutingTier`] itself (the same rule
    /// [`crate::evaluation::RoutingEvidence`]'s own doc comment gives for a stored vocabulary
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
            // Map line 1951's own reader has no rating split — see this
            // function's header — so every session here is still counted
            // by its proxy, exactly as before `RoutingRated` existed.
            rated_useful: 0,
            rated_not_useful: 0,
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
        rated_useful: row.get(5)?,
        rated_not_useful: row.get(6)?,
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

/// Capability map line 1463 — how many routing decisions were made per
/// interactive hour, with both numbers beside the ratio so the ratio can
/// never be read without its denominators.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingDecisionRate {
    /// [`EvaluationKind::RoutingContinuationDecided`] rows in the window —
    /// one per launch that reached a routing decision
    /// ([`crate::evaluation::record_routing_decision`] writes exactly one per launch).
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
