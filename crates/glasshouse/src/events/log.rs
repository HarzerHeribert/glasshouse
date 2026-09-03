//! The append-only project event log.
//!
//! [`crate::events::bus`] gets an event to everyone who needs it *now*, in
//! memory, bounded. This is where the same events go to outlive the process:
//! one row per event in the project's own SQLite database, appended and never
//! rewritten.
//!
//! # Append-only is enforced by the database, not by discipline
//!
//! Phase 18's fixed architectural requirement is that *derived interpretation
//! must not overwrite or masquerade as the original event*. Two triggers
//! created by migration 5 abort every `UPDATE` and every `DELETE` on
//! `lifecycle_events`, so that property holds against any code that opens the
//! file — this crate's, a later phase's, or a hand-typed `sqlite3` session.
//! A rule a future query could forget is not the same kind of thing as a rule
//! the file itself refuses to break; the project database already draws that
//! distinction for project isolation, and this is the same argument.
//!
//! The cost is stated rather than hidden: **nothing can prune this table.**
//! Retention, if it is ever wanted, is a migration and a decision, not a
//! `DELETE` somebody adds one afternoon.
//!
//! # The raw observation is kept beside the normalized event, not inside it
//!
//! The same requirement asks that raw observations stay available as
//! diagnostic source evidence *while normalized and derived records remain
//! distinguishable from them*. So a row carries both: `kind` and its variant
//! payload are Glasshouse's normalized reading, and `observed_harness` /
//! `observed_event` are the harness's own two words, exactly as it spelled
//! them, in their own columns. Neither can be mistaken for the other, and a
//! row that was never translated from a harness report simply has NULL there.
//!
//! **There is no column that could hold a conversation.** A hook payload
//! carries the user's prompt and the model's last message; the handler drains
//! that stream unread, and what reaches this module is
//! [`crate::events::RawObservation`]'s `harness` and `event` and nothing else
//! — `detail`, the one field an adapter could fill from a payload, is not
//! stored. That is a property of the schema, so no future writer can change
//! it without a migration.
//!
//! # Why the sink does not write from the publishing thread
//!
//! [`crate::events::EventSink::record`] is called on the publishing thread,
//! and that thread is sometimes the one draining a pseudo-terminal. A
//! terminal that stops being drained fills, and then the harness itself
//! blocks on `write` — Glasshouse would have stopped the product it exists to
//! host. A SQLite insert is not a long wait, but it is not a bounded one
//! either: the connection carries a five-second busy timeout, and one other
//! process holding the write lock is all it takes.
//!
//! So [`EventLogSink`] is a bounded queue with a writer thread behind it, and
//! `record` is a `try_send` that drops the event and counts the drop rather
//! than ever waiting. That is exactly the trade the bus already makes for a
//! subscriber that stops draining, and for the same reason.
//!
//! A short-lived process that is *not* draining a terminal — the hook
//! handler, which exists for a few milliseconds and then exits — uses
//! [`EventLog`] directly and writes synchronously, because queueing behind a
//! thread it is about to drop would lose the event it was run to record.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension};

use crate::database::PROJECT_ID_KEY;
use crate::session::SessionId;

use super::{
    EventSink, GatewayFailure, LifecycleEvent, MessageOrigin, ProcessExit, RecordedEvent,
    TurnOutcome,
};

/// How many events may wait for the writer thread before the oldest are lost.
///
/// Bounded for the reason the module doc gives: an unbounded queue turns a
/// stalled writer into unbounded memory, which is a worse failure than a gap
/// in a diagnostic log.
pub const DEFAULT_SINK_QUEUE: usize = 1024;

/// What an adapter saw, as it is stored.
///
/// The owned counterpart of [`crate::events::RawObservation`], minus
/// `detail`. See the module doc for why that field does not survive the trip
/// into the database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    /// The harness that reported it, as an integration slug.
    pub harness: String,
    /// The event's own name, exactly as the harness spelled it.
    pub event: String,
}

impl Observation {
    pub fn new(harness: impl Into<String>, event: impl Into<String>) -> Self {
        Self {
            harness: harness.into(),
            event: event.into(),
        }
    }
}

impl LoggedEvent {
    /// This event as the one type every consumer already speaks.
    ///
    /// The interface merges events it published itself with events another
    /// process recorded, and a second type would make every consumer handle
    /// both.
    ///
    /// One caveat travels with the conversion: `RecordedEvent::seq` is the
    /// position in whichever stream produced the record — a bus's for a
    /// published one, this log's for a rebuilt one. It orders a stream and
    /// does not identify an event across streams.
    ///
    /// (The constructor it goes through is crate-private and deliberately
    /// unlinked here: a doc link to a private item is a compile-time
    /// reference like any other and fails the rustdoc gate — practice §18's
    /// documentation half.)
    pub fn into_recorded(self) -> RecordedEvent {
        RecordedEvent::from_log(
            u64::try_from(self.seq).unwrap_or(0),
            self.session,
            self.at,
            self.event,
        )
    }
}

impl<'a> From<&crate::events::RawObservation<'a>> for Observation {
    /// Deliberately drops `detail`. See the module doc.
    fn from(raw: &crate::events::RawObservation<'a>) -> Self {
        Self::new(raw.harness, raw.event)
    }
}

/// One row of the log, read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoggedEvent {
    /// Position in the log, from 1. The database assigns it, so it is the
    /// project's ordering rather than one process's.
    pub seq: i64,
    pub session: SessionId,
    /// Seconds since the Unix epoch.
    pub at: i64,
    /// Glasshouse's normalized reading of what happened.
    pub event: LifecycleEvent,
    /// The harness report it was translated from, when it was translated from
    /// one at all. `None` for an event Glasshouse observed itself — a process
    /// exiting, a line being delivered.
    pub observed: Option<Observation>,
}

/// Everything that can go wrong appending to or reading the log.
#[derive(Debug, thiserror::Error)]
pub enum EventLogError {
    #[error("could not {action} in the project event log")]
    Sql {
        action: &'static str,
        #[source]
        source: rusqlite::Error,
    },
    #[error("the project database has no project identifier bound")]
    UnboundDatabase,
    #[error("event {seq} stored an unrecognized {column} value `{value}`")]
    UnknownValue {
        seq: i64,
        column: &'static str,
        value: String,
    },
    #[error(
        "event {seq} is of kind `{value}`, which this build does not know; \
         the kinds it reads are {}",
        crate::database::LIFECYCLE_EVENT_KINDS.join(", ")
    )]
    UnknownKind { seq: i64, value: String },
    #[error("event {seq} is a `{kind}` with no {column} recorded")]
    MissingValue {
        seq: i64,
        kind: String,
        column: &'static str,
    },
}

/// The project's durable event log, over an open connection it owns.
///
/// Opening goes through the crate's own `database::open`, so the symlink
/// refusal, the read-only refusal, the project-identity check and the
/// migrations all apply, and the path comes from the runtime rather than from
/// a caller.
pub struct EventLog {
    conn: Connection,
    project_id: String,
}

impl std::fmt::Debug for EventLog {
    /// Prints no events. A `Debug` that dumped the log would put a project's
    /// whole session activity into a panic message.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventLog")
            .field("project_id", &self.project_id)
            .finish_non_exhaustive()
    }
}

impl EventLog {
    /// Open the active project's event log.
    pub fn open(runtime: &crate::Runtime) -> anyhow::Result<Self> {
        let conn = crate::database::open(runtime)?;
        Ok(Self::over(conn)?)
    }

    /// Take an already-open project connection.
    ///
    /// The project identifier is read from the database's own binding rather
    /// than accepted as an argument, for the reason
    /// [`crate::session::SessionStore::new`] gives: the identifier written is
    /// then by construction the identifier the triggers compare against.
    pub(crate) fn over(conn: Connection) -> Result<Self, EventLogError> {
        let project_id: Option<String> = conn
            .query_row(
                "SELECT value FROM project_metadata WHERE key = ?1",
                [PROJECT_ID_KEY],
                |row| row.get(0),
            )
            .optional()
            .map_err(|source| EventLogError::Sql {
                action: "read the project identifier",
                source,
            })?;
        Ok(Self {
            project_id: project_id.ok_or(EventLogError::UnboundDatabase)?,
            conn,
        })
    }

    /// The project every row in this log belongs to.
    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    /// Append one event, with the harness observation it was translated from
    /// when there was one.
    ///
    /// Synchronous. Callers on a thread that must never wait use
    /// [`EventLogSink`] instead — see the module doc.
    pub fn append(
        &self,
        recorded: &RecordedEvent,
        observed: Option<&Observation>,
    ) -> Result<(), EventLogError> {
        let event = recorded.event();
        let (
            turn_outcome,
            origin,
            bytes,
            exit_code,
            exit_signal,
            resource,
            gateway_reason,
            gateway_provider,
            gateway_model,
            gateway_cause,
            path,
        ) = payload_columns(event);

        self.conn
            .execute(
                "INSERT INTO lifecycle_events (
                     project_id, session_id, at, kind,
                     turn_outcome, origin, bytes, exit_code, exit_signal,
                     resource, gateway_reason,
                     gateway_provider, gateway_model, gateway_cause, path,
                     observed_harness, observed_event
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, \
                 ?16, ?17)",
                rusqlite::params![
                    &self.project_id,
                    recorded.session().as_str(),
                    recorded.at(),
                    event.kind(),
                    turn_outcome,
                    origin,
                    bytes,
                    exit_code,
                    exit_signal,
                    resource,
                    gateway_reason,
                    gateway_provider,
                    gateway_model,
                    gateway_cause,
                    path,
                    observed.map(|o| o.harness.as_str()),
                    observed.map(|o| o.event.as_str()),
                ],
            )
            .map_err(|source| EventLogError::Sql {
                action: "append an event",
                source,
            })?;
        Ok(())
    }

    /// Every event in the project, oldest first.
    pub fn all(&self) -> Result<Vec<LoggedEvent>, EventLogError> {
        self.query("SELECT {C} FROM lifecycle_events ORDER BY seq", &[])
    }

    /// Every event recorded for one session, oldest first.
    ///
    /// This is the reconstruction the map means by *source material*: a
    /// session's whole history, in order, after the process that produced it
    /// is gone.
    pub fn for_session(&self, session: &SessionId) -> Result<Vec<LoggedEvent>, EventLogError> {
        self.query(
            "SELECT {C} FROM lifecycle_events WHERE session_id = ?1 ORDER BY seq",
            &[&session.as_str()],
        )
    }

    /// The most recent `limit` events recorded for one session, oldest first
    /// within the window.
    ///
    /// [`EventLog::for_session`] reads a session's whole history, which is
    /// the right answer for reconstruction and the wrong one for a caller
    /// that wants the tail — memory extraction runs after every completed
    /// turn and would otherwise re-read a week of log to fill a chunk that
    /// holds sixty entries. The bound is applied in SQL rather than by
    /// truncating afterwards, so the rows never exist.
    pub fn recent_for_session(
        &self,
        session: &SessionId,
        limit: usize,
    ) -> Result<Vec<LoggedEvent>, EventLogError> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut rows = self.query(
            "SELECT {C} FROM lifecycle_events WHERE session_id = ?1 \
             ORDER BY seq DESC LIMIT ?2",
            &[&session.as_str(), &limit],
        )?;
        rows.reverse();
        Ok(rows)
    }

    /// The most recent `limit` events, oldest first within the window.
    pub fn recent(&self, limit: usize) -> Result<Vec<LoggedEvent>, EventLogError> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut rows = self.query(
            "SELECT {C} FROM lifecycle_events ORDER BY seq DESC LIMIT ?1",
            &[&limit],
        )?;
        rows.reverse();
        Ok(rows)
    }

    /// Every event after position `after`, oldest first.
    ///
    /// # Why this exists beside [`EventLog::observed_since`]
    ///
    /// `observed_since`'s filter is a **de-duplication** rule, not a
    /// relevance one. Its own doc gives the reason: a consumer that is
    /// already subscribed to this process's [`crate::events::EventBus`]
    /// receives everything this process published, so reading the whole log
    /// would show each of those events twice. That premise is true of
    /// `shell::run`, which holds both a subscription and a log tail, and it
    /// is the query that belongs there.
    ///
    /// **A reader in another process holds no such subscription.** For it
    /// there is nothing to double, and the filter stops being
    /// de-duplication and becomes loss: it hides precisely the events the
    /// logging process produced itself. For `glasshouse api serve` — which
    /// owns the pseudo-terminal of every orchestrated worker — that is every
    /// spawn, every intervention and every exit, which is to say the whole
    /// history the orchestrator on the far end of the socket is asking for.
    ///
    /// So the choice between the two is a question about **where the reader
    /// is**, not about which events matter. This one is for a reader that is
    /// somewhere else.
    ///
    /// It is also the query [`EventLog::head`] already agrees with: `head`
    /// is `MAX(seq)` over the whole table and never was filtered, so a
    /// caller paging with `after`/`head` against `observed_since` was
    /// carrying a cursor that counted rows it could not be shown.
    pub fn since(&self, after: i64, limit: usize) -> Result<Vec<LoggedEvent>, EventLogError> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        self.query(
            "SELECT {C} FROM lifecycle_events WHERE seq > ?1 ORDER BY seq LIMIT ?2",
            &[&after, &limit],
        )
    }

    /// Harness-reported events after position `after`, oldest first.
    ///
    /// # Why this filters to observed events
    ///
    /// It is the interface's window onto what happened in *another process*.
    /// Everything the interface's own runtime does is published onto its own
    /// [`crate::events::EventBus`] and reaches its consumers directly; the one
    /// class of event it cannot see that way is a harness report, which
    /// arrives in a separate short-lived hook process and can only come back
    /// through the project database. Those rows are exactly the ones carrying
    /// an observation, so that is the filter — a rule about where an event
    /// came from, not a guess about which ones matter.
    ///
    /// Reading the whole log instead would show every in-process event twice.
    ///
    /// That reasoning is about the reader's *location*, so it does not
    /// survive being carried to a reader in another process — see
    /// [`EventLog::since`], which is the query for one.
    pub fn observed_since(
        &self,
        after: i64,
        limit: usize,
    ) -> Result<Vec<LoggedEvent>, EventLogError> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        self.query(
            "SELECT {C} FROM lifecycle_events \
             WHERE seq > ?1 AND observed_harness IS NOT NULL ORDER BY seq LIMIT ?2",
            &[&after, &limit],
        )
    }

    /// The highest position in the log, or 0 when it is empty.
    ///
    /// What a reader starts from when it only wants what happens next: the
    /// interface opening should not replay a week of history into its
    /// activity view.
    pub fn head(&self) -> Result<i64, EventLogError> {
        let head: i64 = self
            .conn
            .query_row(
                "SELECT COALESCE(MAX(seq), 0) FROM lifecycle_events",
                [],
                |row| row.get(0),
            )
            .map_err(|source| EventLogError::Sql {
                action: "read the log's position",
                source,
            })?;
        Ok(head)
    }

    /// How many events the log holds.
    pub fn len(&self) -> Result<u64, EventLogError> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM lifecycle_events", [], |row| {
                row.get(0)
            })
            .map_err(|source| EventLogError::Sql {
                action: "count events",
                source,
            })?;
        Ok(count.max(0) as u64)
    }

    pub fn is_empty(&self) -> Result<bool, EventLogError> {
        Ok(self.len()? == 0)
    }

    fn query(
        &self,
        sql: &str,
        params: &[&dyn rusqlite::ToSql],
    ) -> Result<Vec<LoggedEvent>, EventLogError> {
        let sql = sql.replace("{C}", ALL_COLUMNS);
        let mut statement = self
            .conn
            .prepare(&sql)
            .map_err(|source| EventLogError::Sql {
                action: "prepare an event query",
                source,
            })?;
        let rows = statement
            .query_map(params, |row| Ok(read_row(row)))
            .map_err(|source| EventLogError::Sql {
                action: "read the event log",
                source,
            })?;

        let mut out = Vec::new();
        for row in rows {
            let logged = row.map_err(|source| EventLogError::Sql {
                action: "read an event row",
                source,
            })?;
            out.push(logged?);
        }
        Ok(out)
    }
}

const ALL_COLUMNS: &str = "seq, session_id, at, kind, turn_outcome, origin, bytes, \
                           exit_code, exit_signal, resource, gateway_reason, \
                           gateway_provider, gateway_model, gateway_cause, path, \
                           observed_harness, observed_event";

/// One event's variant payload, spread across the columns that hold it.
///
/// A tuple rather than a struct because it is consumed once, immediately, by
/// the one `INSERT` above. Every column is `None` for the kinds that do not
/// carry it, which is what makes the row self-describing: a `turn_ended` with
/// no `turn_outcome` is a corrupt row, and [`read_row`] says so rather than
/// guessing a verdict.
type PayloadColumns = (
    Option<&'static str>,
    Option<&'static str>,
    Option<i64>,
    Option<i64>,
    Option<String>,
    Option<String>,
    Option<&'static str>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn payload_columns(event: &LifecycleEvent) -> PayloadColumns {
    let mut columns: PayloadColumns = (
        None, None, None, None, None, None, None, None, None, None, None,
    );
    match event {
        LifecycleEvent::TurnEnded { outcome } => columns.0 = Some(outcome_sql(*outcome)),
        LifecycleEvent::TextDelivered { origin, bytes } => {
            columns.1 = Some(origin_sql(*origin));
            columns.2 = Some(i64::try_from(*bytes).unwrap_or(i64::MAX));
        }
        LifecycleEvent::InterruptDelivered { origin } => columns.1 = Some(origin_sql(*origin)),
        LifecycleEvent::ProcessExited { exit } => {
            columns.3 = Some(i64::from(exit.code()));
            columns.4 = exit.signal().map(str::to_owned);
        }
        LifecycleEvent::GatewayUnhealthy { resource, reason } => {
            columns.5 = Some(resource.clone());
            columns.6 = Some(gateway_reason_sql(*reason));
        }
        LifecycleEvent::GatewayBackendChanged {
            provider,
            model,
            cause,
        } => {
            columns.7 = Some(provider.clone());
            columns.8 = Some(model.clone());
            columns.9 = Some(cause.clone());
        }
        // Migration 26's `CHECK ((kind = 'file_touched') = (path IS NOT
        // NULL))` is the other half of this line: the only kind that sets
        // this column, and the only column that kind sets.
        LifecycleEvent::FileTouched { path } => columns.10 = Some(path.clone()),
        LifecycleEvent::SessionStarted
        | LifecycleEvent::SessionResumed
        | LifecycleEvent::TurnStarted
        | LifecycleEvent::WaitingForUser
        | LifecycleEvent::OutputEnded => {}
    }
    columns
}

// ---------------------------------------------------------------------
// Stored spellings.
//
// Written out here rather than borrowed from each type's `as_str`, and the
// difference is not pedantry: `GatewayFailure::as_str` renders `timed out`,
// with a space, because it is what a person reads in a diagnostic. A schema
// `CHECK` constraint built from a `Display` is a schema that changes when
// somebody improves a sentence. `every_stored_spelling_round_trips` pins
// both directions over every variant.
// ---------------------------------------------------------------------

fn outcome_sql(outcome: TurnOutcome) -> &'static str {
    match outcome {
        TurnOutcome::Completed => "completed",
        TurnOutcome::Failed => "failed",
    }
}

fn outcome_from_sql(value: &str) -> Option<TurnOutcome> {
    match value {
        "completed" => Some(TurnOutcome::Completed),
        "failed" => Some(TurnOutcome::Failed),
        _ => None,
    }
}

fn origin_sql(origin: MessageOrigin) -> &'static str {
    match origin {
        MessageOrigin::UserKeystroke => "user_keystroke",
        MessageOrigin::Machine => "machine",
    }
}

fn origin_from_sql(value: &str) -> Option<MessageOrigin> {
    match value {
        "user_keystroke" => Some(MessageOrigin::UserKeystroke),
        "machine" => Some(MessageOrigin::Machine),
        _ => None,
    }
}

fn gateway_reason_sql(reason: GatewayFailure) -> &'static str {
    match reason {
        GatewayFailure::Unreachable => "unreachable",
        GatewayFailure::TimedOut => "timed_out",
        GatewayFailure::Rejected => "rejected",
    }
}

fn gateway_reason_from_sql(value: &str) -> Option<GatewayFailure> {
    match value {
        "unreachable" => Some(GatewayFailure::Unreachable),
        "timed_out" => Some(GatewayFailure::TimedOut),
        "rejected" => Some(GatewayFailure::Rejected),
        _ => None,
    }
}

/// Rebuild one event from its row.
///
/// A missing payload column is an error naming the row, never a guessed
/// default. That matters most for `turn_ended`: defaulting a missing outcome
/// to `Completed` would manufacture the one claim about the work itself that
/// the whole event model is careful never to invent.
fn read_row(row: &rusqlite::Row<'_>) -> Result<LoggedEvent, EventLogError> {
    // `row.get_unwrap` panics on any conversion failure, including a TEXT
    // column whose stored bytes are not valid UTF-8 -- which a single bit
    // flip in an otherwise untouched database file can produce without
    // `PRAGMA integrity_check` ever noticing, and which then crashes every
    // future command that reads this log. `col` reports that the same way
    // every other store in this crate reports a SQL failure. (`seq` is the
    // table's `INTEGER PRIMARY KEY` -- the rowid itself, guaranteed a valid
    // 64-bit integer by the file format rather than a record value that
    // corruption could reinterpret -- so it could use `col` too but cannot
    // exercise the failure path `col` exists for.)
    fn col<T: rusqlite::types::FromSql>(
        row: &rusqlite::Row<'_>,
        index: usize,
    ) -> Result<T, EventLogError> {
        row.get(index).map_err(|source| EventLogError::Sql {
            action: "read an event column",
            source,
        })
    }

    let seq: i64 = col(row, 0)?;
    let session = SessionId::new(col::<String>(row, 1)?);
    let at: i64 = col(row, 2)?;
    let kind: String = col(row, 3)?;
    let turn_outcome: Option<String> = col(row, 4)?;
    let origin: Option<String> = col(row, 5)?;
    let bytes: Option<i64> = col(row, 6)?;
    let exit_code: Option<i64> = col(row, 7)?;
    let exit_signal: Option<String> = col(row, 8)?;
    let resource: Option<String> = col(row, 9)?;
    let gateway_reason: Option<String> = col(row, 10)?;
    let gateway_provider: Option<String> = col(row, 11)?;
    let gateway_model: Option<String> = col(row, 12)?;
    let gateway_cause: Option<String> = col(row, 13)?;
    let path: Option<String> = col(row, 14)?;
    let observed_harness: Option<String> = col(row, 15)?;
    let observed_event: Option<String> = col(row, 16)?;

    let missing = |column: &'static str| EventLogError::MissingValue {
        seq,
        kind: kind.clone(),
        column,
    };
    let unknown =
        |column: &'static str, value: String| EventLogError::UnknownValue { seq, column, value };

    let event = match kind.as_str() {
        "session_started" => LifecycleEvent::SessionStarted,
        "session_resumed" => LifecycleEvent::SessionResumed,
        "turn_started" => LifecycleEvent::TurnStarted,
        "waiting_for_user" => LifecycleEvent::WaitingForUser,
        "output_ended" => LifecycleEvent::OutputEnded,
        "turn_ended" => {
            let stored = turn_outcome.ok_or_else(|| missing("turn_outcome"))?;
            LifecycleEvent::TurnEnded {
                outcome: outcome_from_sql(&stored)
                    .ok_or_else(|| unknown("turn_outcome", stored.clone()))?,
            }
        }
        "text_delivered" => {
            let stored = origin.ok_or_else(|| missing("origin"))?;
            LifecycleEvent::TextDelivered {
                origin: origin_from_sql(&stored)
                    .ok_or_else(|| unknown("origin", stored.clone()))?,
                bytes: usize::try_from(bytes.ok_or_else(|| missing("bytes"))?).unwrap_or(0),
            }
        }
        "interrupt_delivered" => {
            let stored = origin.ok_or_else(|| missing("origin"))?;
            LifecycleEvent::InterruptDelivered {
                origin: origin_from_sql(&stored)
                    .ok_or_else(|| unknown("origin", stored.clone()))?,
            }
        }
        "process_exited" => LifecycleEvent::ProcessExited {
            exit: ProcessExit::from_parts(
                u32::try_from(exit_code.ok_or_else(|| missing("exit_code"))?).unwrap_or(u32::MAX),
                exit_signal,
            ),
        },
        "gateway_unhealthy" => {
            let stored = gateway_reason.ok_or_else(|| missing("gateway_reason"))?;
            LifecycleEvent::GatewayUnhealthy {
                resource: resource.ok_or_else(|| missing("resource"))?,
                reason: gateway_reason_from_sql(&stored)
                    .ok_or_else(|| unknown("gateway_reason", stored.clone()))?,
            }
        }
        "gateway_backend_changed" => LifecycleEvent::GatewayBackendChanged {
            provider: gateway_provider.ok_or_else(|| missing("gateway_provider"))?,
            model: gateway_model.ok_or_else(|| missing("gateway_model"))?,
            cause: gateway_cause.ok_or_else(|| missing("gateway_cause"))?,
        },
        "file_touched" => LifecycleEvent::FileTouched {
            path: path.ok_or_else(|| missing("path"))?,
        },
        // Named separately from the payload columns because the answer a
        // reader needs is different: an unknown *kind* is a row written by a
        // build that models something this one does not, and the useful thing
        // to say is which kinds this build does read.
        other => {
            return Err(EventLogError::UnknownKind {
                seq,
                value: other.to_owned(),
            });
        }
    };

    // Both halves or neither: a harness name with no event name, or the
    // reverse, would be a row claiming a translation it cannot describe.
    let observed = match (observed_harness, observed_event) {
        (Some(harness), Some(event)) => Some(Observation { harness, event }),
        (None, None) => None,
        (Some(_), None) => return Err(missing("observed_event")),
        (None, Some(_)) => return Err(missing("observed_harness")),
    };

    Ok(LoggedEvent {
        seq,
        session,
        at,
        event,
        observed,
    })
}

// ---------------------------------------------------------------------
// The sink.
// ---------------------------------------------------------------------

/// One thing for the writer thread to do.
enum Message {
    Append(RecordedEvent, Option<Observation>),
    /// Answer once everything queued before this has been written. Used to
    /// make the log durable at a point a caller chooses — leaving the shell,
    /// or a test asserting on rows.
    Flush(std::sync::mpsc::SyncSender<()>),
}

/// An [`EventSink`] that never waits on the database.
///
/// See the module doc for why this exists rather than a direct insert.
/// Publishing hands the event to a bounded queue and returns; a writer thread
/// drains it. When the queue is full the event is dropped and counted, which
/// is the same choice [`crate::events::bus`] makes for a subscriber that
/// stopped draining, for the same reason: recent events are the useful ones,
/// and nothing may be allowed to stall a pseudo-terminal.
pub struct EventLogSink {
    queue: SyncSender<Message>,
    dropped: AtomicU64,
    accepted: AtomicU64,
}

impl std::fmt::Debug for EventLogSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventLogSink")
            .field("accepted", &self.accepted.load(Ordering::SeqCst))
            .field("dropped", &self.dropped.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

impl EventLogSink {
    /// Start a writer thread that appends to `log`.
    ///
    /// A failed append is logged and the loop continues. A log that cannot be
    /// written is a diagnostics problem; turning it into a dead writer thread
    /// would silently stop recording everything afterwards too.
    pub fn spawn(log: EventLog) -> Arc<Self> {
        Self::with_writer(DEFAULT_SINK_QUEUE, move |recorded, observed| {
            if let Err(err) = log.append(&recorded, observed.as_ref()) {
                tracing::warn!(error = %err, "could not append to the project event log");
            }
        })
    }

    /// [`EventLogSink::spawn`] with the queue bound and the writer replaced,
    /// so a test can prove the publishing thread is never made to wait —
    /// which needs a writer that provably does.
    pub fn with_writer(
        capacity: usize,
        mut write: impl FnMut(RecordedEvent, Option<Observation>) + Send + 'static,
    ) -> Arc<Self> {
        let (queue, inbox) = std::sync::mpsc::sync_channel(capacity);
        std::thread::Builder::new()
            .name("glasshouse-event-log".to_owned())
            .spawn(move || drain(inbox, &mut write))
            .expect("could not start the event-log writer thread");
        Arc::new(Self {
            queue,
            dropped: AtomicU64::new(0),
            accepted: AtomicU64::new(0),
        })
    }

    /// Append an event together with the harness report it was translated
    /// from, without waiting for the write.
    pub fn record_observed(&self, recorded: &RecordedEvent, observed: Option<Observation>) {
        match self
            .queue
            .try_send(Message::Append(recorded.clone(), observed))
        {
            Ok(()) => {
                self.accepted.fetch_add(1, Ordering::SeqCst);
            }
            // `Full` is the whole point of this method: the writer is behind,
            // and the publishing thread walks away rather than joining it.
            // `Disconnected` means the writer thread is gone, which is the
            // same outcome from the caller's side.
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.dropped.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    /// How many events were handed to the writer.
    pub fn accepted(&self) -> u64 {
        self.accepted.load(Ordering::SeqCst)
    }

    /// How many events were dropped because the writer had fallen behind.
    ///
    /// Exposed rather than hidden, for the reason
    /// [`crate::events::Subscription::dropped`] is: a log with a gap should be
    /// able to say so.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::SeqCst)
    }

    /// Wait, at most `bound`, for everything queued so far to be written.
    ///
    /// Returns whether it was. Bounded rather than open-ended because the one
    /// production caller is on the way out of the shell, and failing to
    /// record the last few events is survivable where failing to exit is not
    /// — the same rule [`crate::shutdown`] applies to its own cleanup.
    pub fn flush(&self, bound: Duration) -> bool {
        let (done, is_done) = std::sync::mpsc::sync_channel(1);
        if self.queue.try_send(Message::Flush(done)).is_err() {
            return false;
        }
        is_done.recv_timeout(bound).is_ok()
    }
}

impl EventSink for EventLogSink {
    fn record(&self, event: &RecordedEvent) {
        self.record_observed(event, None);
    }
}

fn drain(inbox: Receiver<Message>, write: &mut impl FnMut(RecordedEvent, Option<Observation>)) {
    while let Ok(message) = inbox.recv() {
        match message {
            Message::Append(recorded, observed) => write(recorded, observed),
            Message::Flush(done) => {
                let _ = done.try_send(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every stored spelling survives a round trip, in both directions, for
    /// every variant of every enum the schema constrains.
    ///
    /// Written as a round trip rather than as a list of expected strings so
    /// that it also proves the two halves agree: a reader that accepted a
    /// spelling the writer never produces would pass a one-directional test
    /// and lose data here.
    #[test]
    fn every_stored_spelling_round_trips() {
        for outcome in [TurnOutcome::Completed, TurnOutcome::Failed] {
            assert_eq!(outcome_from_sql(outcome_sql(outcome)), Some(outcome));
        }
        for origin in [MessageOrigin::UserKeystroke, MessageOrigin::Machine] {
            assert_eq!(origin_from_sql(origin_sql(origin)), Some(origin));
        }
        for reason in [
            GatewayFailure::Unreachable,
            GatewayFailure::TimedOut,
            GatewayFailure::Rejected,
        ] {
            assert_eq!(
                gateway_reason_from_sql(gateway_reason_sql(reason)),
                Some(reason)
            );
        }

        // And the spellings are not the display strings. `timed out` with a
        // space is what a person reads; a schema constraint built from it
        // would move whenever that sentence is improved.
        assert_eq!(gateway_reason_sql(GatewayFailure::TimedOut), "timed_out");
        assert_eq!(GatewayFailure::TimedOut.as_str(), "timed out");

        assert_eq!(outcome_from_sql("Completed"), None);
        assert_eq!(origin_from_sql("keystroke"), None);
        assert_eq!(gateway_reason_from_sql("timed out"), None);
    }

    /// Every variant puts something in the columns its kind is supposed to
    /// carry, and nothing in the ones it is not.
    ///
    /// The negative half is the load-bearing one: a writer that put an origin
    /// on a `turn_ended` would produce a row whose kind and payload disagree,
    /// and `read_row` would hand it back as something it never was.
    #[test]
    fn each_kind_fills_exactly_its_own_columns() {
        let filled = |event: &LifecycleEvent| {
            let (
                outcome,
                origin,
                bytes,
                code,
                signal,
                resource,
                reason,
                provider,
                model,
                cause,
                path,
            ) = payload_columns(event);
            (
                outcome.is_some(),
                origin.is_some(),
                bytes.is_some(),
                code.is_some(),
                signal.is_some(),
                resource.is_some(),
                reason.is_some(),
                provider.is_some(),
                model.is_some(),
                cause.is_some(),
                path.is_some(),
            )
        };

        assert_eq!(
            filled(&LifecycleEvent::SessionStarted),
            (
                false, false, false, false, false, false, false, false, false, false, false
            )
        );
        assert_eq!(
            filled(&LifecycleEvent::TurnEnded {
                outcome: TurnOutcome::Failed
            }),
            (
                true, false, false, false, false, false, false, false, false, false, false
            )
        );
        assert_eq!(
            filled(&LifecycleEvent::TextDelivered {
                origin: MessageOrigin::Machine,
                bytes: 7
            }),
            (
                false, true, true, false, false, false, false, false, false, false, false
            )
        );
        assert_eq!(
            filled(&LifecycleEvent::InterruptDelivered {
                origin: MessageOrigin::UserKeystroke
            }),
            (
                false, true, false, false, false, false, false, false, false, false, false
            )
        );
        assert_eq!(
            filled(&LifecycleEvent::ProcessExited {
                exit: ProcessExit::from_parts(0, None)
            }),
            (
                false, false, false, true, false, false, false, false, false, false, false
            )
        );
        assert_eq!(
            filled(&LifecycleEvent::GatewayUnhealthy {
                resource: "gw".to_owned(),
                reason: GatewayFailure::Rejected
            }),
            (
                false, false, false, false, false, true, true, false, false, false, false
            )
        );
        assert_eq!(
            filled(&LifecycleEvent::GatewayBackendChanged {
                provider: "anthropic".to_owned(),
                model: "claude".to_owned(),
                cause: "failover".to_owned(),
            }),
            (
                false, false, false, false, false, false, false, true, true, true, false
            )
        );
        // Migration 26's biconditional `CHECK`, asserted on the writer's side:
        // the only kind that sets `path`, and the only column it sets.
        assert_eq!(
            filled(&LifecycleEvent::FileTouched {
                path: "crates/x.rs".to_owned(),
            }),
            (
                false, false, false, false, false, false, false, false, false, false, true
            )
        );
    }

    /// The kind names are the ones the schema's `CHECK` constraint lists.
    ///
    /// Pinned here rather than trusted, because a rename would compile
    /// perfectly and then fail as a constraint violation on a background
    /// writer thread, where nobody is looking.
    #[test]
    fn the_kind_names_are_the_ones_the_schema_allows() {
        let kinds = [
            LifecycleEvent::SessionStarted.kind(),
            LifecycleEvent::SessionResumed.kind(),
            LifecycleEvent::TurnStarted.kind(),
            LifecycleEvent::TurnEnded {
                outcome: TurnOutcome::Completed,
            }
            .kind(),
            LifecycleEvent::WaitingForUser.kind(),
            LifecycleEvent::TextDelivered {
                origin: MessageOrigin::Machine,
                bytes: 0,
            }
            .kind(),
            LifecycleEvent::InterruptDelivered {
                origin: MessageOrigin::Machine,
            }
            .kind(),
            LifecycleEvent::ProcessExited {
                exit: ProcessExit::from_parts(0, None),
            }
            .kind(),
            LifecycleEvent::OutputEnded.kind(),
            LifecycleEvent::GatewayUnhealthy {
                resource: String::new(),
                reason: GatewayFailure::Rejected,
            }
            .kind(),
            LifecycleEvent::GatewayBackendChanged {
                provider: String::new(),
                model: String::new(),
                cause: String::new(),
            }
            .kind(),
            LifecycleEvent::FileTouched {
                path: String::new(),
            }
            .kind(),
        ];
        let unique: std::collections::BTreeSet<&str> = kinds.iter().copied().collect();
        assert_eq!(unique.len(), kinds.len(), "two variants share a kind name");
        for kind in kinds {
            assert!(
                crate::database::LIFECYCLE_EVENT_KINDS.contains(&kind),
                "`{kind}` is not one of the kinds migration 5 allows"
            );
        }
        assert_eq!(
            unique.len(),
            crate::database::LIFECYCLE_EVENT_KINDS.len(),
            "the schema allows a kind no event produces, or the reverse"
        );
    }

    /// **"Treat the raw event stream as reconstructable source material
    /// rather than directly injecting it into agent prompts."**
    ///
    /// Reconstruction is proven elsewhere — `for_session` hands back a whole
    /// session's history after the process that produced it is gone. This is
    /// the other half, which is a *negative* and therefore needs a mechanism
    /// rather than a habit: **nothing that composes text for a harness reads
    /// this module.**
    ///
    /// Glasshouse composes harness-bound text in exactly two places today: a
    /// checkpoint's bootstrap prompt, and the session API's `send_text`. Both
    /// are scanned. A future one that reached for the raw stream would fail
    /// here, which is the point — the rule is that a prompt is built from
    /// something curated, and an event log is the opposite of curated.
    #[test]
    fn nothing_that_builds_text_for_a_harness_reads_the_event_log() {
        // Everything before the `#[cfg(test)] mod tests` block, comments
        // stripped, read by `str::lines` so a carriage return cannot hide a
        // match — the idiom `crate::events`'s own source guards use, and for
        // the reason recorded in practice §14.
        let production = |source: &str| -> String {
            let lines: Vec<&str> = source.lines().collect();
            let end = lines
                .windows(2)
                .position(|pair| {
                    pair[0].trim_end() == "#[cfg(test)]"
                        && pair[1].trim_end().starts_with("mod tests")
                })
                .unwrap_or(lines.len());
            lines[..end]
                .iter()
                .filter(|line| !line.trim_start().starts_with("//"))
                .copied()
                .collect::<Vec<_>>()
                .join("\n")
        };

        let composers = [
            (
                "checkpoint/mod.rs",
                include_str!("../checkpoint/mod.rs"),
                "bootstrap_prompt",
            ),
            (
                "session/api/mod.rs",
                include_str!("../session/api/mod.rs"),
                "send_text",
            ),
        ];

        for (name, source, anchor) in composers {
            let code = production(source);
            assert!(
                code.contains(anchor),
                "{name}: the anchor `{anchor}` is gone, so this scan has stopped \
                 covering what it claims to cover"
            );
            for reader in ["EventLog", "LoggedEvent", "lifecycle_events", "events::log"] {
                assert!(
                    !code.contains(reader),
                    "{name} names `{reader}`, so the raw event stream can reach a \
                     harness prompt directly; a prompt is built from curated \
                     material, and this log is deliberately not that"
                );
            }
        }
    }

    /// A row that lost its payload is reported, not guessed.
    ///
    /// `turn_ended` is the case that matters: inventing `Completed` for a
    /// missing outcome would manufacture the one claim about the work itself
    /// that this event model exists to never infer.
    #[test]
    fn a_turn_ended_row_with_no_outcome_is_an_error_rather_than_a_verdict() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE lifecycle_events (
                 seq INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT, at INTEGER,
                 kind TEXT, turn_outcome TEXT, origin TEXT, bytes INTEGER,
                 exit_code INTEGER, exit_signal TEXT, resource TEXT,
                 gateway_reason TEXT, gateway_provider TEXT, gateway_model TEXT,
                 gateway_cause TEXT, path TEXT, observed_harness TEXT,
                 observed_event TEXT);
             INSERT INTO lifecycle_events (session_id, at, kind)
             VALUES ('s', 1, 'turn_ended');",
        )
        .unwrap();

        let error = conn
            .query_row(
                &format!("SELECT {ALL_COLUMNS} FROM lifecycle_events"),
                [],
                |row| Ok(read_row(row)),
            )
            .unwrap()
            .expect_err("a turn_ended with no outcome must not be readable");
        assert!(
            matches!(
                &error,
                EventLogError::MissingValue {
                    column: "turn_outcome",
                    ..
                }
            ),
            "got {error:?}"
        );
    }

    /// Half an observation is a row claiming a translation it cannot
    /// describe, and is refused rather than half-reported.
    #[test]
    fn an_observation_missing_half_of_itself_is_refused() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE lifecycle_events (
                 seq INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT, at INTEGER,
                 kind TEXT, turn_outcome TEXT, origin TEXT, bytes INTEGER,
                 exit_code INTEGER, exit_signal TEXT, resource TEXT,
                 gateway_reason TEXT, gateway_provider TEXT, gateway_model TEXT,
                 gateway_cause TEXT, path TEXT, observed_harness TEXT,
                 observed_event TEXT);
             INSERT INTO lifecycle_events (session_id, at, kind, observed_harness)
             VALUES ('s', 1, 'turn_started', 'some-harness');",
        )
        .unwrap();

        let error = conn
            .query_row(
                &format!("SELECT {ALL_COLUMNS} FROM lifecycle_events"),
                [],
                |row| Ok(read_row(row)),
            )
            .unwrap()
            .expect_err("half an observation must be refused");
        assert!(
            matches!(
                &error,
                EventLogError::MissingValue {
                    column: "observed_event",
                    ..
                }
            ),
            "got {error:?}"
        );
    }

    /// A `session_id` column holding bytes that are not valid UTF-8 — the
    /// shape a single flipped bit in an otherwise-intact row produces,
    /// invisible to `PRAGMA integrity_check` — must be a reported error, not
    /// a panic that takes down every later invocation that reads this log.
    #[test]
    fn a_hostile_column_is_a_reported_error_not_a_panic() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE lifecycle_events (
                 seq INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT, at INTEGER,
                 kind TEXT, turn_outcome TEXT, origin TEXT, bytes INTEGER,
                 exit_code INTEGER, exit_signal TEXT, resource TEXT,
                 gateway_reason TEXT, gateway_provider TEXT, gateway_model TEXT,
                 gateway_cause TEXT, path TEXT, observed_harness TEXT,
                 observed_event TEXT);
             INSERT INTO lifecycle_events (session_id, at, kind)
             VALUES (CAST(x'7b22ff7d' AS TEXT), 1, 'session_started');",
        )
        .unwrap();

        let error = conn
            .query_row(
                &format!("SELECT {ALL_COLUMNS} FROM lifecycle_events"),
                [],
                |row| Ok(read_row(row)),
            )
            .unwrap()
            .expect_err("a hostile column must not panic the caller");
        assert!(matches!(&error, EventLogError::Sql { .. }), "got {error:?}");
    }
}
