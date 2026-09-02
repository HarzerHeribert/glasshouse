//! The evidence ledger's writer: `EvidenceLedger::open` and `record` — the
//! only two operations that touch the database for anything but a read. See
//! `super` for the shared row types and `EvidenceLedger` struct itself.

use super::*;

use std::sync::Mutex;

use rusqlite::{OptionalExtension, params};

use crate::database::PROJECT_ID_KEY;

impl EvidenceLedger {
    /// Open the active project's database and read its binding.
    ///
    /// The path comes from `runtime` and nowhere else — the same door
    /// [`crate::memory::ProjectMemory::open`] uses, so every check
    /// `crate::database::open` performs (the symlink refusal, the read-only
    /// refusal, the project-identity check, the migrations) applies here too.
    /// This is also the whole of this ledger's contribution to line 1343's
    /// "keep the evidence ledger physically project-scoped": there is no
    /// second constructor that accepts a path, a project id, or another
    /// project's already-open connection, so nothing built on this type can
    /// name another project's file.
    pub fn open(runtime: &crate::Runtime) -> anyhow::Result<Self> {
        let conn = crate::database::open(runtime)?;
        let project_id: Option<String> = conn
            .query_row(
                "SELECT value FROM project_metadata WHERE key = ?1",
                [PROJECT_ID_KEY],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql_err("read the project identifier"))?;
        Ok(Self {
            project_id: project_id.ok_or(EvidenceLedgerError::UnboundDatabase)?,
            conn: Mutex::new(conn),
        })
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    /// Append one observation. There is no corresponding `update` — see this
    /// module's own header and migration 11's own doc comment for why: line
    /// 1329's "append-oriented" is a property of this store's method list,
    /// not merely of the schema underneath it.
    ///
    /// Returns the row's `seq`.
    pub fn record(
        &self,
        new: NewObservation,
        observed_at_unix: i64,
    ) -> Result<i64, EvidenceLedgerError> {
        let (cost_micro_usd, cost_confidence) = match new.cost {
            Some(cost) => (Some(cost.micro_usd), Some(cost.confidence.as_str())),
            None => (None, None),
        };
        let conn = self.lock();
        conn.execute(
            "INSERT INTO routing_observations (
                project_id, observed_at,
                provider, model, route, quota_context, harness, purpose,
                dispatched_at, first_byte_at, first_token_at, first_tool_call_at, completed_at,
                input_tokens, output_tokens, cached_input_tokens,
                cost_micro_usd, cost_confidence,
                tool_rounds, retries, repairs, failovers, outcome,
                context_state, failure_class, task_class,
                session_id, effort_level, turn_shape,
                first_byte_ms, first_token_ms, first_tool_call_ms, completed_ms
            ) VALUES (
                ?1, ?2,
                ?3, ?4, ?5, ?6, ?7, ?8,
                ?9, ?10, ?11, ?12, ?13,
                ?14, ?15, ?16,
                ?17, ?18,
                ?19, ?20, ?21, ?22, ?23,
                ?24, ?25, ?26,
                ?27, ?28, ?29,
                ?30, ?31, ?32, ?33
            )",
            params![
                self.project_id,
                observed_at_unix,
                new.provider,
                new.model,
                new.route,
                new.quota_context,
                new.harness,
                new.purpose,
                new.dispatched_at_unix,
                new.first_byte_at_unix,
                new.first_token_at_unix,
                new.first_tool_call_at_unix,
                new.completed_at_unix,
                new.input_tokens,
                new.output_tokens,
                new.cached_input_tokens,
                cost_micro_usd,
                cost_confidence,
                new.tool_rounds,
                new.retries,
                new.repairs,
                new.failovers,
                new.outcome.map(Outcome::as_str),
                new.context_state.as_str(),
                new.failure_class.map(FailureClass::as_str),
                new.task_class
                    .map(crate::routing::request::TaskClass::as_str),
                new.session_id,
                new.effort_level.map(EffortLevel::as_str),
                new.turn_shape.map(TurnShape::as_str),
                new.first_byte_ms,
                new.first_token_ms,
                new.first_tool_call_ms,
                new.completed_ms,
            ],
        )
        .map_err(sql_err("record a routing observation"))?;
        Ok(conn.last_insert_rowid())
    }
}
