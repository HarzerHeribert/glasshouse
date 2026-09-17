use rusqlite::{OptionalExtension, params};

use super::{
    EvaluationError, EvaluationKind, EvaluationObservation, EvaluationObservations,
    EvaluationOutcome, RetrievalScope, TURN_COMPLETED, sql_err,
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
/// The proxy for 1821/1831 needs a [`EvaluationKind::MemoryRetrieved`] row's
/// `session_id` to find "the retrieving session", and a same-session row
/// saying how its turn ended. The queries below join on
/// [`EvaluationKind::TurnOutcomeObserved`] — a row `record_turn_outcome`
/// writes for **every** session that reaches the hook's `TurnEnded` arm.
///
/// **The router's own override signal is gone** (Glasshouse deletes its
/// router, design-decisions.md 2026-09-16): the proxy no longer has an
/// override-shaped negative signal to join on, so it reads purely off turn
/// completion.
///
/// History: design-decisions.md, "Trims: the remaining module docs, second
/// packet", evaluation/readers.rs `impl EvaluationObservations` (memory readers).
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
                     ))",
            params![
                EvaluationKind::MemoryRated.as_str(),
                EvaluationOutcome::PreventedRepetition.as_str(),
                from,
                to,
                EvaluationKind::MemoryRetrieved.as_str(),
                EvaluationKind::TurnOutcomeObserved.as_str(),
                TURN_COMPLETED,
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
