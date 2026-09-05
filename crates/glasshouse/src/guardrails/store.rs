//! The assumption ledger — `task_assumptions` and `assumption_transitions`,
//! `crate::database` migration 19.
//!
//! `task_assumptions` holds six agent-stated fields and nothing else, never
//! updated: a trigger refuses every `UPDATE`. `assumption_transitions` is
//! the append-only history — the current state is the latest row, read with
//! `MAX(seq)`, a second trigger refusing `UPDATE` there too. A transition
//! with no `assumption_id` is a session-level event (see [`TransitionKind`]).
//! Project scope is migration 15's two triggers, copied exactly. Task
//! assumptions are transient (line 1017) — what is worth keeping is
//! promoted into `memories` — so this ledger keeps [`Retention::DEFAULT`]'s
//! 90 days or 100,000 transitions, trimmed oldest-first on the handle
//! already open and writing (practice §65).
//!
//! Every free-text field is passed through [`super::sanitize`]. A claim
//! over [`super::MAX_CLAIM_CHARS`] is **refused**, not cut; other fields are
//! cut visibly, and brackets are rewritten by [`super::quote`] wherever
//! rendered into a block an agent reads. History: design-decisions.md,
//! "Trims: the remaining module docs, second packet", guardrails/store.rs
//! module doc.

use std::fmt;
use std::sync::Arc;

use rusqlite::{Connection, OptionalExtension, params};

use super::{
    AssumptionState, EvidenceSource, GuardrailOverride, GuardrailResponse, MAX_CLAIM_CHARS,
    MAX_FIELD_CHARS, Origin, TransitionKind, Uncertainty, sanitize,
};
use crate::Runtime;
use crate::database::PROJECT_ID_KEY;

/// A clock, injectable for tests.
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

fn system_clock() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// An assumption's identifier: 32 lowercase hex characters from SQLite's
/// own CSPRNG, the same shape as a memory's.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AssumptionId(String);

impl AssumptionId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The leading twelve characters — enough to be unambiguous in one
    /// project's ledger, and what `glasshouse assumptions` prints.
    pub fn short(&self) -> &str {
        let end = self
            .0
            .char_indices()
            .nth(12)
            .map_or(self.0.len(), |(i, _)| i);
        &self.0[..end]
    }
}

impl fmt::Display for AssumptionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(&self.0)
    }
}

/// How much history this ledger keeps — the evaluation ledger's bounds, for
/// the reason the module header gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retention {
    pub max_age_secs: i64,
    pub max_rows: i64,
    /// The trim runs once every this many appended transitions, counted on
    /// `seq` so the cadence survives short-lived processes.
    pub trim_every: i64,
}

impl Retention {
    /// 90 days, 100,000 transitions, trimmed every 256 rows.
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

/// Everything that can go wrong in this ledger, written for the door: no
/// variant names a path.
#[derive(Debug, thiserror::Error)]
pub enum GuardrailError {
    #[error("the project database has no project identifier bound")]
    UnboundDatabase,
    #[error("a claim must say something: it was empty after control characters were removed")]
    EmptyClaim,
    #[error(
        "a claim is one sentence of at most {max} characters; shorten it rather than \
             letting the ledger cut it"
    )]
    ClaimTooLong { max: usize },
    #[error("`{id}` is not an assumption identifier: expected leading hex characters")]
    MalformedId { id: String },
    #[error("no assumption `{id}` in this project")]
    NotFound { id: String },
    #[error("`{prefix}` matches more than one assumption in this project; give more of the id")]
    Ambiguous { prefix: String },
    #[error("assumption {id} stored an unrecognized {column} value `{value}`")]
    UnknownValue {
        id: String,
        column: &'static str,
        value: String,
    },
    #[error("`waived_by_user` is a person's decision: state `origin: user` to record it")]
    WaiverNeedsUser,
    #[error(
        "assumption {id} is `{state}`, not `supported`; only a supported assumption can be \
             promoted (line 1017)"
    )]
    NotSupported { id: String, state: AssumptionState },
    #[error(
        "a failed-approach memory is written only from a `refuted` transition, not from \
             `{state}`"
    )]
    NotRefuted { state: AssumptionState },
    #[error("could not {action} in the assumption ledger")]
    Sql {
        action: &'static str,
        #[source]
        source: rusqlite::Error,
    },
}

fn sql_err(action: &'static str) -> impl Fn(rusqlite::Error) -> GuardrailError {
    move |source| GuardrailError::Sql { action, source }
}

/// The six fields, plus who is stating them and for which session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAssumption {
    pub session: Option<String>,
    pub claim: String,
    pub evidence: String,
    pub evidence_source: EvidenceSource,
    pub uncertainty: Uncertainty,
    pub affected: String,
    pub verification: String,
    pub origin: Origin,
}

/// One stored assumption — the six fields as recorded, never edited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssumptionRecord {
    pub id: AssumptionId,
    pub session_id: Option<String>,
    pub created_at: i64,
    pub origin: Origin,
    pub claim: String,
    pub evidence: String,
    pub evidence_source: EvidenceSource,
    pub uncertainty: Uncertainty,
    pub affected: String,
    pub verification: String,
}

/// One row of the append-only history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition {
    pub seq: i64,
    pub assumption_id: Option<AssumptionId>,
    pub session_id: Option<String>,
    pub at: i64,
    pub kind: TransitionKind,
    pub state: Option<AssumptionState>,
    pub origin: Origin,
    /// Machine-written, in the vocabulary of `kind`: the override, the
    /// gate's `<risk>/<factor>/<verdict>`, the exceeded axis, or a memory
    /// identifier a transition produced.
    pub subject: Option<String>,
    pub response: Option<GuardrailResponse>,
    /// Free text from the caller, sanitized.
    pub note: Option<String>,
}

/// An assumption with its current state — the latest transition's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssumptionView {
    pub record: AssumptionRecord,
    pub state: AssumptionState,
    pub latest: Transition,
    /// How many transitions the assumption has had, the first included.
    pub transitions: i64,
}

impl AssumptionView {
    /// Whether the assumption still needs an answer: neither supported,
    /// refuted nor waived.
    pub fn is_open(&self) -> bool {
        matches!(
            self.state,
            AssumptionState::Proposed | AssumptionState::Probing | AssumptionState::Unresolved
        )
    }
}

/// One transition to append.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTransition {
    /// The new state, or `None` to re-state the current one — a response or
    /// a note recorded without a move.
    pub state: Option<AssumptionState>,
    pub origin: Origin,
    pub note: Option<String>,
    pub response: Option<GuardrailResponse>,
    pub subject: Option<String>,
}

impl NewTransition {
    pub fn to(state: AssumptionState, origin: Origin) -> Self {
        Self {
            state: Some(state),
            origin,
            note: None,
            response: None,
            subject: None,
        }
    }

    pub fn restate(origin: Origin) -> Self {
        Self {
            state: None,
            origin,
            note: None,
            response: None,
            subject: None,
        }
    }

    pub fn with_note(mut self, note: Option<impl Into<String>>) -> Self {
        self.note = note.map(Into::into);
        self
    }

    pub fn with_response(mut self, response: Option<GuardrailResponse>) -> Self {
        self.response = response;
        self
    }

    pub fn with_subject(mut self, subject: Option<impl Into<String>>) -> Self {
        self.subject = subject.map(Into::into);
        self
    }
}

/// A refuted transition or an exceeded budget, as `Request::Events` and the
/// watcher's completion line carry it — with the claim beside it when the
/// row names an assumption, so a reader knows *what* was refuted without a
/// second call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub transition: Transition,
    pub claim: Option<String>,
}

/// The ledger, over one project's database.
pub struct AssumptionStore {
    conn: Connection,
    project_id: String,
    retention: Retention,
    clock: Clock,
}

impl fmt::Debug for AssumptionStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AssumptionStore")
            .field("project_id", &self.project_id)
            .field("retention", &self.retention)
            .finish_non_exhaustive()
    }
}

const TRANSITION_COLUMNS: &str =
    "seq, assumption_id, session_id, at, kind, state, origin, subject, response, note";

impl AssumptionStore {
    /// Open the active project's database with the shipped retention.
    ///
    /// The path comes from `runtime` and nowhere else — the same door every
    /// other store here uses, so every check `crate::database::open`
    /// performs applies.
    pub fn open(runtime: &Runtime) -> anyhow::Result<Self> {
        Self::open_with(runtime, Retention::DEFAULT, Arc::new(system_clock))
    }

    /// [`Self::open`] with the retention and the clock replaced, so a test
    /// can watch the trim work on a handful of rows at chosen times.
    pub fn open_with(
        runtime: &Runtime,
        retention: Retention,
        clock: Clock,
    ) -> anyhow::Result<Self> {
        let conn = crate::database::open(runtime)?;
        let project_id: Option<String> = conn
            .query_row(
                "SELECT value FROM project_metadata WHERE key = ?1",
                [PROJECT_ID_KEY],
                |row| row.get(0),
            )
            .optional()?;
        let project_id = project_id.ok_or(GuardrailError::UnboundDatabase)?;
        Ok(Self {
            conn,
            project_id,
            retention,
            clock,
        })
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    fn now(&self) -> i64 {
        (self.clock)()
    }

    fn generate_id(&self) -> Result<String, GuardrailError> {
        self.conn
            .query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))
            .map_err(sql_err("generate an assumption identifier"))
    }

    /// Record one assumption in its `proposed` state — one transaction, so
    /// there is never an assumption without a first transition.
    pub fn record(&mut self, new: NewAssumption) -> Result<AssumptionRecord, GuardrailError> {
        let claim = sanitize(&new.claim, MAX_CLAIM_CHARS);
        if claim.truncated {
            return Err(GuardrailError::ClaimTooLong {
                max: MAX_CLAIM_CHARS,
            });
        }
        if claim.text.is_empty() {
            return Err(GuardrailError::EmptyClaim);
        }
        let field = |text: &str| sanitize(text, MAX_FIELD_CHARS).text;

        let record = AssumptionRecord {
            id: AssumptionId(self.generate_id()?),
            session_id: new.session.filter(|s| !s.trim().is_empty()),
            created_at: self.now(),
            origin: new.origin,
            claim: claim.text,
            evidence: field(&new.evidence),
            evidence_source: new.evidence_source,
            uncertainty: new.uncertainty,
            affected: field(&new.affected),
            verification: field(&new.verification),
        };

        let tx = self
            .conn
            .transaction()
            .map_err(sql_err("begin recording an assumption"))?;
        tx.execute(
            "INSERT INTO task_assumptions (
                id, project_id, session_id, created_at, origin, claim, evidence,
                evidence_source, uncertainty, affected, verification
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                record.id.as_str(),
                self.project_id,
                record.session_id,
                record.created_at,
                record.origin.as_str(),
                record.claim,
                record.evidence,
                record.evidence_source.as_str(),
                record.uncertainty.as_str(),
                record.affected,
                record.verification,
            ],
        )
        .map_err(sql_err("record an assumption"))?;
        let seq = append_within(
            &tx,
            &self.project_id,
            Some(record.id.as_str()),
            record.session_id.as_deref(),
            record.created_at,
            TransitionKind::Transition,
            Some(AssumptionState::Proposed),
            record.origin,
            None,
            None,
            None,
        )?;
        maybe_trim(&tx, self.retention, seq, record.created_at)?;
        tx.commit()
            .map_err(sql_err("commit recording an assumption"))?;
        Ok(record)
    }

    /// Append one transition. The current state is read inside the same
    /// transaction the new row is written in, so two concurrent writers
    /// cannot both restate a stale one.
    ///
    /// `waived_by_user` is refused unless `origin` is [`Origin::User`]: the
    /// door cannot authenticate that claim, but it can insist the caller make
    /// it, which is what keeps the ledger attributable.
    pub fn transition(
        &mut self,
        id: &AssumptionId,
        new: NewTransition,
    ) -> Result<Transition, GuardrailError> {
        if new.state == Some(AssumptionState::WaivedByUser) && new.origin != Origin::User {
            return Err(GuardrailError::WaiverNeedsUser);
        }
        let note = new
            .note
            .as_deref()
            .map(|text| sanitize(text, MAX_FIELD_CHARS).text)
            .filter(|text| !text.is_empty());
        let at = self.now();

        let tx = self
            .conn
            .transaction()
            .map_err(sql_err("begin an assumption transition"))?;
        let current = current_view(&tx, id)?.ok_or_else(|| GuardrailError::NotFound {
            id: id.as_str().to_owned(),
        })?;
        let state = new.state.unwrap_or(current.state);
        let seq = append_within(
            &tx,
            &self.project_id,
            Some(id.as_str()),
            current.record.session_id.as_deref(),
            at,
            TransitionKind::Transition,
            Some(state),
            new.origin,
            new.subject.as_deref(),
            new.response,
            note.as_deref(),
        )?;
        maybe_trim(&tx, self.retention, seq, at)?;
        let written = read_transition(&tx, seq)?;
        tx.commit()
            .map_err(sql_err("commit an assumption transition"))?;
        Ok(written)
    }

    /// Append a session-level event: a gate, an override, an exceeded
    /// budget. `state` is `Some(waived_by_user)` for a `skip` override and
    /// `None` otherwise.
    pub fn record_session_event(
        &mut self,
        session: &str,
        kind: TransitionKind,
        state: Option<AssumptionState>,
        origin: Origin,
        subject: Option<&str>,
        note: Option<&str>,
    ) -> Result<Transition, GuardrailError> {
        let note = note
            .map(|text| sanitize(text, MAX_FIELD_CHARS).text)
            .filter(|text| !text.is_empty());
        let at = self.now();
        let tx = self
            .conn
            .transaction()
            .map_err(sql_err("begin a session guardrail event"))?;
        let seq = append_within(
            &tx,
            &self.project_id,
            None,
            Some(session),
            at,
            kind,
            state,
            origin,
            subject,
            None,
            note.as_deref(),
        )?;
        maybe_trim(&tx, self.retention, seq, at)?;
        let written = read_transition(&tx, seq)?;
        tx.commit()
            .map_err(sql_err("commit a session guardrail event"))?;
        Ok(written)
    }

    /// Enforce the retention bounds now, and report how many rows went.
    pub fn trim(&mut self, now: i64) -> Result<usize, GuardrailError> {
        let tx = self
            .conn
            .transaction()
            .map_err(sql_err("begin an assumption trim"))?;
        let removed = trim_within(&tx, self.retention, now)?;
        tx.commit().map_err(sql_err("commit an assumption trim"))?;
        Ok(removed)
    }

    /// One assumption with its current state, or `None`.
    pub fn get(&self, id: &AssumptionId) -> Result<Option<AssumptionView>, GuardrailError> {
        current_view(&self.conn, id)
    }

    /// An identifier from its unambiguous leading part — the same rule
    /// `glasshouse memory show` uses.
    pub fn resolve_id(&self, prefix: &str) -> Result<AssumptionId, GuardrailError> {
        let prefix = prefix.trim();
        if prefix.is_empty() || !prefix.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(GuardrailError::MalformedId {
                id: prefix.to_owned(),
            });
        }
        let lowered = prefix.to_ascii_lowercase();
        let mut statement = self
            .conn
            .prepare("SELECT id FROM task_assumptions WHERE id LIKE ?1 || '%' LIMIT 2")
            .map_err(sql_err("prepare an identifier lookup"))?;
        let matches: Vec<String> = statement
            .query_map([&lowered], |row| row.get(0))
            .map_err(sql_err("look up an identifier"))?
            .collect::<Result<_, _>>()
            .map_err(sql_err("read an identifier"))?;
        match matches.as_slice() {
            [] => Err(GuardrailError::NotFound {
                id: prefix.to_owned(),
            }),
            [one] => Ok(AssumptionId(one.clone())),
            _ => Err(GuardrailError::Ambiguous {
                prefix: prefix.to_owned(),
            }),
        }
    }

    /// Every assumption, newest first, with its current state — optionally
    /// one session's only.
    pub fn list(
        &self,
        session: Option<&str>,
        limit: usize,
    ) -> Result<Vec<AssumptionView>, GuardrailError> {
        let mut statement = self
            .conn
            .prepare(&format!(
                "{VIEW_SELECT} WHERE (?1 IS NULL OR a.session_id = ?1) \
                 ORDER BY a.created_at DESC, a.id DESC LIMIT ?2"
            ))
            .map_err(sql_err("prepare an assumption listing"))?;
        let rows = statement
            .query_map(params![session, limit as i64], row_to_view)
            .map_err(sql_err("list assumptions"))?;
        collect_views(rows)
    }

    /// One session's open assumptions — proposed, probing or unresolved —
    /// oldest first, which is the order to re-evaluate them in.
    pub fn open_for_session(&self, session: &str) -> Result<Vec<AssumptionView>, GuardrailError> {
        let mut statement = self
            .conn
            .prepare(&format!(
                "{VIEW_SELECT} WHERE a.session_id = ?1 \
                 AND t.state IN ('proposed', 'probing', 'unresolved') \
                 ORDER BY a.created_at ASC, a.id ASC"
            ))
            .map_err(sql_err("prepare an open-assumption listing"))?;
        let rows = statement
            .query_map([session], row_to_view)
            .map_err(sql_err("list open assumptions"))?;
        collect_views(rows)
    }

    /// Every transition of one assumption, oldest first.
    pub fn history(&self, id: &AssumptionId) -> Result<Vec<Transition>, GuardrailError> {
        let mut statement = self
            .conn
            .prepare(&format!(
                "SELECT {TRANSITION_COLUMNS} FROM assumption_transitions \
                 WHERE assumption_id = ?1 ORDER BY seq ASC"
            ))
            .map_err(sql_err("prepare a history read"))?;
        let rows = statement
            .query_map([id.as_str()], row_to_transition)
            .map_err(sql_err("read a history"))?;
        collect_transitions(rows)
    }

    /// One session's session-level events, newest first — every kind, or
    /// one kind.
    pub fn session_events(
        &self,
        session: &str,
        kind: Option<TransitionKind>,
        limit: usize,
    ) -> Result<Vec<Transition>, GuardrailError> {
        let mut statement = self
            .conn
            .prepare(&format!(
                "SELECT {TRANSITION_COLUMNS} FROM assumption_transitions \
                 WHERE session_id = ?1 AND assumption_id IS NULL \
                 AND (?2 IS NULL OR kind = ?2) \
                 ORDER BY seq DESC LIMIT ?3"
            ))
            .map_err(sql_err("prepare a session event read"))?;
        let rows = statement
            .query_map(
                params![session, kind.map(TransitionKind::as_str), limit as i64],
                row_to_transition,
            )
            .map_err(sql_err("read session events"))?;
        collect_transitions(rows)
    }

    /// The per-task override in force for a session: the latest override
    /// row, decoded. A row whose subject this build cannot read is reported
    /// as such rather than silently treated as no override.
    pub fn latest_override(
        &self,
        session: &str,
    ) -> Result<Option<(GuardrailOverride, Transition)>, GuardrailError> {
        let latest = self
            .session_events(session, Some(TransitionKind::Override), 1)?
            .into_iter()
            .next();
        match latest {
            None => Ok(None),
            Some(row) => {
                let subject = row.subject.clone().unwrap_or_default();
                let kind = GuardrailOverride::from_stored(&subject).ok_or_else(|| {
                    GuardrailError::UnknownValue {
                        id: format!("transition {}", row.seq),
                        column: "subject",
                        value: subject,
                    }
                })?;
                Ok(Some((kind, row)))
            }
        }
    }

    /// Line 1050's two notifications — a `refuted` transition and an
    /// exceeded budget — newer than `after`, oldest first, bounded.
    pub fn notifications_since(
        &self,
        after: i64,
        limit: usize,
    ) -> Result<Vec<Notification>, GuardrailError> {
        self.notifications(None, after, limit)
    }

    /// The same, for one session's rows only.
    pub fn notifications_for_session_since(
        &self,
        session: &str,
        after: i64,
        limit: usize,
    ) -> Result<Vec<Notification>, GuardrailError> {
        self.notifications(Some(session), after, limit)
    }

    fn notifications(
        &self,
        session: Option<&str>,
        after: i64,
        limit: usize,
    ) -> Result<Vec<Notification>, GuardrailError> {
        let mut statement = self
            .conn
            .prepare(
                "SELECT t.seq, t.assumption_id, t.session_id, t.at, t.kind, t.state, t.origin, \
                        t.subject, t.response, t.note, a.claim \
                 FROM assumption_transitions t \
                 LEFT JOIN task_assumptions a ON a.id = t.assumption_id \
                 WHERE t.seq > ?1 \
                 AND (?2 IS NULL OR t.session_id = ?2) \
                 AND ((t.kind = 'transition' AND t.state = 'refuted') \
                      OR t.kind = 'budget_exceeded') \
                 ORDER BY t.seq ASC LIMIT ?3",
            )
            .map_err(sql_err("prepare a notification read"))?;
        let rows = statement
            .query_map(params![after, session, limit as i64], |row| {
                Ok((row_to_transition(row)?, row.get::<_, Option<String>>(10)?))
            })
            .map_err(sql_err("read notifications"))?;
        let mut out = Vec::new();
        for row in rows {
            let (transition, claim) = row.map_err(sql_err("decode a notification"))?;
            out.push(Notification {
                transition: transition?,
                claim,
            });
        }
        Ok(out)
    }

    /// The newest transition's `seq`, or `0` for an empty ledger — the
    /// position a watcher starts from.
    pub fn head(&self) -> Result<i64, GuardrailError> {
        self.conn
            .query_row(
                "SELECT COALESCE(MAX(seq), 0) FROM assumption_transitions",
                [],
                |row| row.get(0),
            )
            .map_err(sql_err("read the ledger head"))
    }

    /// How many assumptions are in each current state — every state, zero
    /// included, in [`AssumptionState::ALL`]'s order.
    pub fn counts(
        &self,
        session: Option<&str>,
    ) -> Result<Vec<(AssumptionState, i64)>, GuardrailError> {
        let mut statement = self
            .conn
            .prepare(
                "SELECT t.state, COUNT(*) FROM task_assumptions a \
                 JOIN assumption_transitions t ON t.seq = ( \
                     SELECT MAX(seq) FROM assumption_transitions WHERE assumption_id = a.id) \
                 WHERE (?1 IS NULL OR a.session_id = ?1) \
                 GROUP BY t.state",
            )
            .map_err(sql_err("prepare a state count"))?;
        let rows = statement
            .query_map([session], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(sql_err("count states"))?;
        let mut counted = Vec::new();
        for row in rows {
            let (state, count) = row.map_err(sql_err("read a state count"))?;
            let state = AssumptionState::from_stored(&state).ok_or_else(|| {
                GuardrailError::UnknownValue {
                    id: "count".to_owned(),
                    column: "state",
                    value: state,
                }
            })?;
            counted.push((state, count));
        }
        Ok(AssumptionState::ALL
            .iter()
            .map(|&state| {
                let count = counted
                    .iter()
                    .find(|(s, _)| *s == state)
                    .map_or(0, |(_, n)| *n);
                (state, count)
            })
            .collect())
    }
}

/// The one `INSERT` into `assumption_transitions`. Every writer goes through
/// it, and it is the only statement in this file that touches the table's
/// contents — there is no `UPDATE` anywhere in this module, and a test reads
/// the source to keep it that way.
#[allow(clippy::too_many_arguments)]
fn append_within(
    tx: &rusqlite::Transaction<'_>,
    project_id: &str,
    assumption_id: Option<&str>,
    session_id: Option<&str>,
    at: i64,
    kind: TransitionKind,
    state: Option<AssumptionState>,
    origin: Origin,
    subject: Option<&str>,
    response: Option<GuardrailResponse>,
    note: Option<&str>,
) -> Result<i64, GuardrailError> {
    tx.execute(
        "INSERT INTO assumption_transitions (
            project_id, assumption_id, session_id, at, kind, state, origin, subject, response, note
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            project_id,
            assumption_id,
            session_id,
            at,
            kind.as_str(),
            state.map(AssumptionState::as_str),
            origin.as_str(),
            subject,
            response.map(GuardrailResponse::as_str),
            note,
        ],
    )
    .map_err(sql_err("append an assumption transition"))?;
    Ok(tx.last_insert_rowid())
}

/// Run the retention trim when this append crossed a
/// [`Retention::trim_every`] boundary — on `seq`, so the cadence survives a
/// process that appends one row and exits.
fn maybe_trim(
    tx: &rusqlite::Transaction<'_>,
    retention: Retention,
    seq: i64,
    now: i64,
) -> Result<(), GuardrailError> {
    let every = retention.trim_every.max(1);
    if seq % every == 0 {
        trim_within(tx, retention, now)?;
    }
    Ok(())
}

fn trim_within(
    tx: &rusqlite::Transaction<'_>,
    retention: Retention,
    now: i64,
) -> Result<usize, GuardrailError> {
    let cutoff = now.saturating_sub(retention.max_age_secs);
    let mut removed = tx
        .execute("DELETE FROM assumption_transitions WHERE at < ?1", [cutoff])
        .map_err(sql_err("trim aged transitions"))?;
    let count: i64 = tx
        .query_row("SELECT COUNT(*) FROM assumption_transitions", [], |row| {
            row.get(0)
        })
        .map_err(sql_err("count transitions"))?;
    let excess = count - retention.max_rows;
    if excess > 0 {
        removed += tx
            .execute(
                "DELETE FROM assumption_transitions WHERE seq IN ( \
                     SELECT seq FROM assumption_transitions ORDER BY seq ASC LIMIT ?1)",
                [excess],
            )
            .map_err(sql_err("trim excess transitions"))?;
    }
    removed += tx
        .execute(
            "DELETE FROM task_assumptions WHERE NOT EXISTS ( \
                 SELECT 1 FROM assumption_transitions t WHERE t.assumption_id = task_assumptions.id)",
            [],
        )
        .map_err(sql_err("trim assumptions with no history"))?;
    Ok(removed)
}

const VIEW_SELECT: &str = "SELECT a.id, a.session_id, a.created_at, a.origin, a.claim, a.evidence, \
        a.evidence_source, a.uncertainty, a.affected, a.verification, \
        t.seq, t.assumption_id, t.session_id, t.at, t.kind, t.state, t.origin, t.subject, \
        t.response, t.note, \
        (SELECT COUNT(*) FROM assumption_transitions x WHERE x.assumption_id = a.id) \
     FROM task_assumptions a \
     JOIN assumption_transitions t ON t.seq = ( \
         SELECT MAX(seq) FROM assumption_transitions WHERE assumption_id = a.id)";

fn current_view(
    conn: &Connection,
    id: &AssumptionId,
) -> Result<Option<AssumptionView>, GuardrailError> {
    let mut statement = conn
        .prepare(&format!("{VIEW_SELECT} WHERE a.id = ?1"))
        .map_err(sql_err("prepare an assumption read"))?;
    let view = statement
        .query_row([id.as_str()], row_to_view)
        .optional()
        .map_err(sql_err("read an assumption"))?;
    view.transpose()
}

fn read_transition(conn: &Connection, seq: i64) -> Result<Transition, GuardrailError> {
    conn.query_row(
        &format!("SELECT {TRANSITION_COLUMNS} FROM assumption_transitions WHERE seq = ?1"),
        [seq],
        row_to_transition,
    )
    .map_err(sql_err("read back a transition"))?
}

type Decoded<T> = Result<T, GuardrailError>;

fn decode<T>(
    id: &str,
    column: &'static str,
    value: String,
    parse: impl Fn(&str) -> Option<T>,
) -> Decoded<T> {
    parse(&value).ok_or_else(|| GuardrailError::UnknownValue {
        id: id.to_owned(),
        column,
        value,
    })
}

fn transition_from(
    row: &rusqlite::Row<'_>,
    offset: usize,
) -> rusqlite::Result<Decoded<Transition>> {
    let seq: i64 = row.get(offset)?;
    let assumption_id: Option<String> = row.get(offset + 1)?;
    let session_id: Option<String> = row.get(offset + 2)?;
    let at: i64 = row.get(offset + 3)?;
    let kind: String = row.get(offset + 4)?;
    let state: Option<String> = row.get(offset + 5)?;
    let origin: String = row.get(offset + 6)?;
    let subject: Option<String> = row.get(offset + 7)?;
    let response: Option<String> = row.get(offset + 8)?;
    let note: Option<String> = row.get(offset + 9)?;
    let id = format!("transition {seq}");
    Ok((|| {
        Ok(Transition {
            seq,
            assumption_id: assumption_id.map(AssumptionId),
            session_id,
            at,
            kind: decode(&id, "kind", kind, TransitionKind::from_stored)?,
            state: state
                .map(|s| decode(&id, "state", s, AssumptionState::from_stored))
                .transpose()?,
            origin: decode(&id, "origin", origin, Origin::from_stored)?,
            subject,
            response: response
                .map(|r| decode(&id, "response", r, GuardrailResponse::from_stored))
                .transpose()?,
            note,
        })
    })())
}

fn row_to_transition(row: &rusqlite::Row<'_>) -> rusqlite::Result<Decoded<Transition>> {
    transition_from(row, 0)
}

fn row_to_view(row: &rusqlite::Row<'_>) -> rusqlite::Result<Decoded<AssumptionView>> {
    let id: String = row.get(0)?;
    let session_id: Option<String> = row.get(1)?;
    let created_at: i64 = row.get(2)?;
    let origin: String = row.get(3)?;
    let claim: String = row.get(4)?;
    let evidence: String = row.get(5)?;
    let evidence_source: String = row.get(6)?;
    let uncertainty: String = row.get(7)?;
    let affected: String = row.get(8)?;
    let verification: String = row.get(9)?;
    let latest = transition_from(row, 10)?;
    let transitions: i64 = row.get(20)?;
    Ok((|| {
        let latest = latest?;
        let state = latest.state.ok_or_else(|| GuardrailError::UnknownValue {
            id: id.clone(),
            column: "state",
            value: "NULL".to_owned(),
        })?;
        Ok(AssumptionView {
            record: AssumptionRecord {
                id: AssumptionId(id.clone()),
                session_id,
                created_at,
                origin: decode(&id, "origin", origin, Origin::from_stored)?,
                claim,
                evidence,
                evidence_source: decode(
                    &id,
                    "evidence_source",
                    evidence_source,
                    EvidenceSource::from_stored,
                )?,
                uncertainty: decode(&id, "uncertainty", uncertainty, Uncertainty::from_stored)?,
                affected,
                verification,
            },
            state,
            latest,
            transitions,
        })
    })())
}

fn collect_views(
    rows: rusqlite::MappedRows<
        '_,
        impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<Decoded<AssumptionView>>,
    >,
) -> Result<Vec<AssumptionView>, GuardrailError> {
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(sql_err("decode an assumption"))??);
    }
    Ok(out)
}

fn collect_transitions(
    rows: rusqlite::MappedRows<
        '_,
        impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<Decoded<Transition>>,
    >,
) -> Result<Vec<Transition>, GuardrailError> {
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(sql_err("decode a transition"))??);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Cli;
    use clap::Parser;
    use std::path::Path;
    use std::sync::Mutex;

    /// A project under `base/workspace/<name>`, bootstrapped against
    /// `base/data` and `base/config`.
    fn bootstrap(base: &Path, name: &str) -> Runtime {
        let root = base.join("workspace").join(name);
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        let cli = Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            base.join("data").to_str().unwrap(),
            "--config-dir",
            base.join("config").to_str().unwrap(),
        ])
        .unwrap();
        crate::bootstrap(&cli, &root).unwrap()
    }

    fn ticking_clock(start: i64) -> (Clock, Arc<Mutex<i64>>) {
        let now = Arc::new(Mutex::new(start));
        let handle = Arc::clone(&now);
        (Arc::new(move || *handle.lock().unwrap()), now)
    }

    fn open(
        runtime: &Runtime,
        retention: Retention,
        at: i64,
    ) -> (AssumptionStore, Arc<Mutex<i64>>) {
        let (clock, now) = ticking_clock(at);
        (
            AssumptionStore::open_with(runtime, retention, clock).unwrap(),
            now,
        )
    }

    fn premise(session: Option<&str>, claim: &str) -> NewAssumption {
        NewAssumption {
            session: session.map(str::to_owned),
            claim: claim.to_owned(),
            evidence: "grep found one caller".to_owned(),
            evidence_source: EvidenceSource::Repository,
            uncertainty: Uncertainty::Medium,
            affected: "api/unix.rs".to_owned(),
            verification: "run the door's tests".to_owned(),
            origin: Origin::Agent,
        }
    }

    #[test]
    fn a_recorded_assumption_starts_proposed_with_exactly_its_six_fields() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = bootstrap(tmp.path(), "alpha");
        let (mut store, _) = open(&runtime, Retention::DEFAULT, 1_000);

        let record = store
            .record(premise(Some("s1"), "the door has one dispatch"))
            .unwrap();
        let view = store.get(&record.id).unwrap().expect("stored");
        assert_eq!(view.state, AssumptionState::Proposed);
        assert_eq!(view.transitions, 1);
        assert_eq!(view.record, record);
        assert_eq!(view.latest.kind, TransitionKind::Transition);
        assert_eq!(view.latest.origin, Origin::Agent);
        assert_eq!(view.latest.session_id.as_deref(), Some("s1"));

        // The table has the six fields and their bookkeeping, and nothing
        // with room for reasoning in it.
        let columns: Vec<String> = {
            let mut statement = store
                .conn
                .prepare("PRAGMA table_info(task_assumptions)")
                .unwrap();
            statement
                .query_map([], |row| row.get::<_, String>("name"))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        assert_eq!(
            columns,
            [
                "id",
                "project_id",
                "session_id",
                "created_at",
                "origin",
                "claim",
                "evidence",
                "evidence_source",
                "uncertainty",
                "affected",
                "verification"
            ]
        );
    }

    #[test]
    fn a_claim_is_sanitized_and_a_long_or_empty_one_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = bootstrap(tmp.path(), "alpha");
        let (mut store, _) = open(&runtime, Retention::DEFAULT, 1_000);

        let record = store
            .record(premise(None, "one\r\nline\u{1b}[2J only"))
            .unwrap();
        assert_eq!(record.claim, "one line [2J only");

        let long = "x".repeat(MAX_CLAIM_CHARS + 1);
        assert!(matches!(
            store.record(premise(None, &long)),
            Err(GuardrailError::ClaimTooLong { .. })
        ));
        assert!(matches!(
            store.record(premise(None, "\u{1b}\r\n")),
            Err(GuardrailError::EmptyClaim)
        ));
        assert_eq!(
            store.list(None, 10).unwrap().len(),
            1,
            "a refusal leaves no row"
        );
    }

    #[test]
    fn transitions_append_and_the_current_state_is_the_latest() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = bootstrap(tmp.path(), "alpha");
        let (mut store, now) = open(&runtime, Retention::DEFAULT, 1_000);
        let record = store.record(premise(Some("s1"), "claim")).unwrap();

        *now.lock().unwrap() = 1_001;
        store
            .transition(
                &record.id,
                NewTransition::to(AssumptionState::Probing, Origin::Agent),
            )
            .unwrap();
        *now.lock().unwrap() = 1_002;
        let restated = store
            .transition(
                &record.id,
                NewTransition::restate(Origin::Agent)
                    .with_response(Some(GuardrailResponse::Verify))
                    .with_note(Some("running the test")),
            )
            .unwrap();
        assert_eq!(
            restated.state,
            Some(AssumptionState::Probing),
            "restated, not moved"
        );
        assert_eq!(restated.response, Some(GuardrailResponse::Verify));
        *now.lock().unwrap() = 1_003;
        store
            .transition(
                &record.id,
                NewTransition::to(AssumptionState::Refuted, Origin::Agent),
            )
            .unwrap();

        let view = store.get(&record.id).unwrap().unwrap();
        assert_eq!(view.state, AssumptionState::Refuted);
        assert_eq!(view.transitions, 4);
        let history = store.history(&record.id).unwrap();
        assert_eq!(
            history.iter().map(|t| t.state).collect::<Vec<_>>(),
            [
                Some(AssumptionState::Proposed),
                Some(AssumptionState::Probing),
                Some(AssumptionState::Probing),
                Some(AssumptionState::Refuted)
            ]
        );
        assert!(
            history
                .windows(2)
                .all(|w| w[0].seq < w[1].seq && w[0].at <= w[1].at)
        );

        // The schema refuses an edit even from a hand-typed statement.
        let err = store
            .conn
            .execute(
                "UPDATE assumption_transitions SET state = 'supported' WHERE seq = ?1",
                [history[3].seq],
            )
            .unwrap_err();
        assert!(err.to_string().contains("append-only"), "{err}");
        let err = store
            .conn
            .execute("UPDATE task_assumptions SET claim = 'edited'", [])
            .unwrap_err();
        assert!(err.to_string().contains("never edited"), "{err}");
    }

    #[test]
    fn a_waiver_needs_a_user_origin_and_an_unknown_id_is_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = bootstrap(tmp.path(), "alpha");
        let (mut store, _) = open(&runtime, Retention::DEFAULT, 1_000);
        let record = store.record(premise(None, "claim")).unwrap();

        assert!(matches!(
            store.transition(
                &record.id,
                NewTransition::to(AssumptionState::WaivedByUser, Origin::Agent)
            ),
            Err(GuardrailError::WaiverNeedsUser)
        ));
        let waived = store
            .transition(
                &record.id,
                NewTransition::to(AssumptionState::WaivedByUser, Origin::User),
            )
            .unwrap();
        assert_eq!(waived.origin, Origin::User);

        assert!(matches!(
            store.transition(
                &AssumptionId::new("00".repeat(16)),
                NewTransition::to(AssumptionState::Probing, Origin::Agent)
            ),
            Err(GuardrailError::NotFound { .. })
        ));
        assert!(matches!(
            store.resolve_id("zz"),
            Err(GuardrailError::MalformedId { .. })
        ));
        assert_eq!(store.resolve_id(record.id.short()).unwrap(), record.id);
    }

    #[test]
    fn session_events_overrides_and_notifications_read_back() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = bootstrap(tmp.path(), "alpha");
        let (mut store, _) = open(&runtime, Retention::DEFAULT, 1_000);

        assert_eq!(store.head().unwrap(), 0);
        assert_eq!(store.latest_override("s1").unwrap(), None);
        store
            .record_session_event(
                "s1",
                TransitionKind::Override,
                Some(AssumptionState::WaivedByUser),
                Origin::User,
                Some("skip"),
                None,
            )
            .unwrap();
        let (kind, row) = store.latest_override("s1").unwrap().expect("an override");
        assert_eq!(kind, GuardrailOverride::Skip);
        assert_eq!(row.state, Some(AssumptionState::WaivedByUser));
        assert_eq!(store.latest_override("s2").unwrap(), None);

        let gate = store
            .record_session_event(
                "s1",
                TransitionKind::Gate,
                None,
                Origin::Glasshouse,
                Some("substantial/migration/gated"),
                Some("add migration 19"),
            )
            .unwrap();
        assert_eq!(gate.note.as_deref(), Some("add migration 19"));
        let head_before = store.head().unwrap();

        let a = store.record(premise(Some("s1"), "a")).unwrap();
        let b = store.record(premise(Some("s2"), "b")).unwrap();
        store
            .transition(
                &a.id,
                NewTransition::to(AssumptionState::Refuted, Origin::Agent),
            )
            .unwrap();
        store
            .transition(
                &b.id,
                NewTransition::to(AssumptionState::Supported, Origin::Agent),
            )
            .unwrap();
        store
            .record_session_event(
                "s2",
                TransitionKind::BudgetExceeded,
                None,
                Origin::Glasshouse,
                Some("files"),
                Some("spent 9 of 4"),
            )
            .unwrap();

        let all = store.notifications_since(head_before, 10).unwrap();
        assert_eq!(all.len(), 2, "one refutation and one budget: {all:?}");
        assert_eq!(all[0].transition.assumption_id, Some(a.id.clone()));
        assert_eq!(all[0].claim.as_deref(), Some("a"));
        assert_eq!(all[1].transition.kind, TransitionKind::BudgetExceeded);
        assert_eq!(all[1].claim, None);
        let s1 = store.notifications_for_session_since("s1", 0, 10).unwrap();
        assert_eq!(s1.len(), 1);
        assert!(
            store
                .notifications_since(store.head().unwrap(), 10)
                .unwrap()
                .is_empty()
        );

        let counts = store.counts(None).unwrap();
        assert_eq!(counts.len(), AssumptionState::ALL.len());
        assert!(counts.contains(&(AssumptionState::Refuted, 1)));
        assert!(counts.contains(&(AssumptionState::Supported, 1)));
        assert!(counts.contains(&(AssumptionState::Proposed, 0)));
        assert_eq!(store.open_for_session("s1").unwrap().len(), 0);
        assert_eq!(store.list(Some("s1"), 10).unwrap().len(), 1);
        assert_eq!(
            store.session_events("s1", None, 10).unwrap().len(),
            2,
            "the override and the gate, no assumption rows"
        );
    }

    #[test]
    fn retention_trims_oldest_first_in_the_writers_transaction_and_orphans_go_with_it() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = bootstrap(tmp.path(), "alpha");
        let retention = Retention {
            max_age_secs: 100,
            max_rows: 4,
            trim_every: 1,
        };
        let (mut store, now) = open(&runtime, retention, 1_000);

        let old = store.record(premise(None, "old")).unwrap();
        *now.lock().unwrap() = 1_200;
        let fresh = store.record(premise(None, "fresh")).unwrap();
        // The trim ran inside `record`: the old row aged out, taking its
        // assumption with it.
        assert_eq!(store.get(&old.id).unwrap(), None);
        assert!(store.get(&fresh.id).unwrap().is_some());

        for _ in 0..6 {
            store
                .transition(&fresh.id, NewTransition::restate(Origin::Agent))
                .unwrap();
        }
        let count: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM assumption_transitions", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(
            count <= retention.max_rows,
            "{count} rows kept of a cap of 4"
        );
        assert!(
            store.get(&fresh.id).unwrap().is_some(),
            "an assumption with recent transitions survives the row cap"
        );
    }

    #[test]
    fn another_projects_row_is_refused_by_the_schema() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = bootstrap(tmp.path(), "alpha");
        let (store, _) = open(&runtime, Retention::DEFAULT, 1_000);
        let err = store
            .conn
            .execute(
                "INSERT INTO task_assumptions (id, project_id, created_at, origin, claim, evidence, \
                 evidence_source, uncertainty, affected, verification) \
                 VALUES ('ab', 'not-this-project', 1, 'agent', 'c', '', 'observed', 'low', '', '')",
                [],
            )
            .unwrap_err();
        assert!(err.to_string().contains("different project"), "{err}");
        let err = store
            .conn
            .execute(
                "INSERT INTO assumption_transitions (project_id, session_id, at, kind, origin) \
                 VALUES ('not-this-project', 's', 1, 'gate', 'glasshouse')",
                [],
            )
            .unwrap_err();
        assert!(err.to_string().contains("different project"), "{err}");
    }

    /// The store never edits a recorded row. Read from the source, because
    /// the method list is the guarantee the module header makes: the
    /// production half of this file — everything above `#[cfg(test)]` —
    /// contains no `UPDATE` in a code line, and exactly one `INSERT` into
    /// the transitions table.
    #[test]
    fn nothing_in_this_module_updates_a_row() {
        let source = include_str!("store.rs");
        let production = source
            .split("#[cfg(test)]")
            .next()
            .expect("the file has a test module");
        let code: String = production
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !code.to_ascii_uppercase().contains("UPDATE"),
            "an UPDATE appeared in the store's production code"
        );
        assert_eq!(
            code.matches("INSERT INTO assumption_transitions").count(),
            1,
            "every transition is written by the one append"
        );
    }
}
