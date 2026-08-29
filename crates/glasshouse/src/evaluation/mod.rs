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

/// What was decided — the `evaluation_observations.kind` vocabulary, in Rust
/// because migration 15 deliberately gives that column no SQL `CHECK`.
///
/// The store encodes through an exhaustive `match`, so a new variant is a
/// compile error at the writer rather than a constraint violation on whatever
/// thread happens to be recording. `database::EVALUATION_KINDS` is
/// the constant a test pins this against, for the same reason
/// `LIFECYCLE_EVENT_KINDS` exists beside its own `CHECK`.
///
/// **One variant, because this package lands one producer.** Variants are
/// added as producers land, never in advance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EvaluationKind {
    /// A memory search returned this memory to a caller — `memory_id` names
    /// which one, and [`RetrievalScope`] is the `subject`.
    ///
    /// One row per *returned memory*, not per search: a search that returned
    /// nothing records nothing, which is why this ledger counts retrieved
    /// memories rather than retrievals.
    MemoryRetrieved,
}

impl EvaluationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MemoryRetrieved => "memory_retrieved",
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
}

impl EvaluationOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
        }
    }

    pub fn from_stored(value: &str) -> Option<Self> {
        match value {
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }
}

/// The `subject` vocabulary for [`EvaluationKind::MemoryRetrieved`]: which of
/// the two questions the search asked.
///
/// This distinction is load-bearing for map line 1826 rather than decoration.
/// A search run with `--history` is *asking* for superseded memories, so a
/// superseded memory in its results is the feature working, not a memory
/// "incorrectly resurfaced as current guidance". A count that folded the two
/// together would report the tool's own history command as a defect.
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
}

impl RetrievalScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Historical => "historical",
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
            .prepare(
                "SELECT seq, observed_at, kind, outcome, subject, session_id,
                        feature, arm, memory_id, routing_seq, detail
                   FROM evaluation_observations
                  ORDER BY seq DESC
                  LIMIT ?1",
            )
            .map_err(sql_err("read evaluation observations"))?;
        let rows = statement
            .query_map(params![limit as i64], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<i64>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                ))
            })
            .map_err(sql_err("read evaluation observations"))?;

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
/// map lines 1822 and 1826.
///
/// **This never fails a retrieval.** Memory search is on the user's path and
/// bookkeeping is not allowed to break it, so every error here is a
/// `tracing::warn!` and a return: the caller gets its results whether or not
/// the ledger could be written.
///
/// The database handle is opened here, and only here, and only when there is
/// something to record — practice §65's rule that a resource is acquired where
/// its consumer starts. A search that returned nothing opens nothing.
pub fn record_memory_retrieval<'a>(
    runtime: &Runtime,
    scope: RetrievalScope,
    memory_ids: impl IntoIterator<Item = &'a str>,
    observed_at_unix: i64,
) {
    let observations: Vec<NewObservation> = memory_ids
        .into_iter()
        .map(|id| {
            NewObservation::new(EvaluationKind::MemoryRetrieved)
                .with_subject(scope.as_str())
                .with_memory_id(id)
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

/// Seconds since the Unix epoch, the way every other store in this crate reads
/// the clock.
pub fn now_unix() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(elapsed) => i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX),
        Err(_) => 0,
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
        let declared = [EvaluationKind::MemoryRetrieved];
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
}
