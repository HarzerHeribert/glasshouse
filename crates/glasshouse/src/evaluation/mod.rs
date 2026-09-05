//! Phase 51 — the project-local evaluation ledger.
//!
//! One row per **decision Glasshouse made whose wisdom is only visible
//! later**, written in `evaluation_observations` (migration 15). It answers
//! *how often*, over a window, never *how much* — cost, tokens and latency
//! belong to [`crate::routing::evidence`].
//!
//! Map line 1856's project-scoping is enforced structurally: migration 15's
//! triggers `RAISE ABORT` on any row not bound to the active project, and
//! this module has no `export` or method that hands out a [`Connection`]. No
//! observation stores memory content, only a `memory_id`.
//! [`EvaluationObservations::record`] never edits a row — a new outcome is a
//! new row, since an edited measurement is a falsified one. [`Retention`]
//! trims oldest-first on the handle already open and writing (practice §65),
//! and a window reaching past the oldest retained row is refused rather than
//! undercounted (see [`EvaluationError::WindowNotRetained`]).
//!
//! History: design-decisions.md, "Trims: the remaining module docs, second
//! packet", evaluation/mod.rs module doc.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::{Connection, OptionalExtension, params};

use crate::Runtime;
use crate::database::{EVALUATION_KINDS, PROJECT_ID_KEY};

mod joins;
mod kinds;
mod readers;
#[cfg(test)]
mod tests;
mod writer;

pub use joins::*;
pub use kinds::*;
pub use readers::*;
pub use writer::*;

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
