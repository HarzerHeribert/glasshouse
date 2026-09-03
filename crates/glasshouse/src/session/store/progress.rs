//! Declared task progress — capability map lines 1294 and 1610, the honest
//! producer of
//! [`crate::provider::quota::ReserveDecisionInputs::task_nearly_complete`].
//!
//! # What a declaration is, and the two things it is not
//!
//! A declaration is one row saying *"whoever is running this Glasshouse
//! session says its current task is nearly complete, and still said so as of
//! this second."* It is a **statement somebody made on purpose**, about one
//! named session, that stops being true on its own.
//!
//! It is not an **inference**. Nothing in this build observes task progress:
//! [`crate::events::LifecycleEvent`] is binary and retrospective, and the
//! only completion fact available where the reserve verdict is computed is
//! that a turn is already over. Every available proxy — a turn count, an
//! elapsed time — reports "almost complete" for work that has merely been
//! running a while, which is precisely the long-running work a protected
//! reserve exists to keep serving. The field this feeds is the *first*
//! branch [`crate::provider::quota::evaluate_reserve_spend`] takes,
//! outranking every other signal including the user's own override, so a
//! fabricated value there does not degrade the policy — it inverts it, at
//! the one moment the protection matters.
//!
//! It is not a **setting**. A configuration value is sticky by nature, and a
//! declaration that outlives the task it described re-creates that inversion
//! by a slower route: the reserve would be permanently open on behalf of a
//! task that finished last week. So the source is a row that expires, and
//! the shape is [`super::claims`]'s — session-scoped, project-scoped, and a
//! no-match by default, which is what makes its arrival a no-op for every
//! caller that declares nothing.
//!
//! # Project isolation
//!
//! Migration 28's two triggers refuse a row whose `project_id` is not the
//! bound one, every statement below also names `project_id` explicitly, and
//! the database file *is* the project. A declaration made in one project
//! cannot be named by a query in another.

use std::collections::BTreeSet;

use rusqlite::OptionalExtension;

use super::{SessionId, SessionLifecycle, SessionStore, SessionStoreError};

/// How long a declaration nobody renewed keeps protecting its session's work.
///
/// # Why thirty minutes, and why shorter than a claim
///
/// The two failure directions are **not symmetric, and they are not
/// symmetric the other way round from [`super::STALE_CLAIM_AFTER`]**, which
/// is why this is a second constant and not a reuse of that one.
///
/// Expiring too early costs the operator the protection they asked for, and
/// they get it back by declaring again. The behaviour it falls back to is
/// exactly today's — the reserve decides on its own signals — so an early
/// expiry is the *safe* direction.
///
/// Expiring too late is the failure this whole design exists to prevent. A
/// declaration left standing keeps forcing the first branch to `Allow` for
/// whatever that session does next, which is a stale statement about a task
/// that is gone being applied to a task nobody described. That is the
/// inversion the design note refuses, arriving by the slower route.
///
/// So the horizon points short. Thirty minutes is longer than a harness turn
/// — a turn is one prompt-to-stop cycle, minutes rather than hours, and a
/// session genuinely finishing a task renews as it goes — and short enough
/// that a declaration somebody forgot cannot protect an unrelated later
/// task. A task still not finished half an hour after somebody called it
/// nearly complete was not nearly complete. It is a judgement, not a
/// measurement, and it is one constant so that changing it is one edit.
pub const TASK_PROGRESS_EXPIRES_AFTER: i64 = 30 * 60;

/// The horizon is deliberately shorter than a file claim's, and the constant
/// above says why: an early expiry falls back to today's behaviour, while a
/// late one keeps a dead statement outranking every other signal the reserve
/// policy has. Checked at compile time rather than by a test, because both
/// values are constants and a test comparing two constants is a build-time
/// fact wearing a runtime cost. Making the two equal fails the build, which
/// is the point: the asymmetry has to be decided against, not lost.
const _: () = assert!(
    TASK_PROGRESS_EXPIRES_AFTER < super::claims::STALE_CLAIM_AFTER,
    "a task-progress declaration must not outlive a file claim"
);

/// The lifecycle states whose sessions may hold a declaration.
///
/// Exactly [`SessionLifecycle::is_live`]'s `true` arm, listed here because
/// SQL cannot call it — the statements below bind these four words. Pinned
/// against `is_live` by `live_lifecycles_are_exactly_the_live_ones` below,
/// so an eighth lifecycle cannot be added on one side only.
const LIVE_LIFECYCLES: [SessionLifecycle; 4] = [
    SessionLifecycle::Starting,
    SessionLifecycle::Running,
    SessionLifecycle::Idle,
    SessionLifecycle::WaitingForUser,
];

/// The `sessions` subquery every statement shares: the live sessions of this
/// project. Written once so they cannot drift apart.
const LIVE_SESSIONS: &str = "SELECT id FROM sessions WHERE lifecycle IN (?3, ?4, ?5, ?6)";

/// One active declaration, as stored.
///
/// There is deliberately no field describing the work. A declaration is one
/// bit — *nearly complete* — plus the session it is about and the horizon it
/// stops being true at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskProgressDeclaration {
    /// The Glasshouse session whose current task was declared nearly
    /// complete. Never a process identifier.
    pub session_id: SessionId,
    /// When the declaration was first made. A renew does not move it.
    pub declared_at: i64,
    /// When it was last renewed, or [`TaskProgressDeclaration::declared_at`]
    /// if never.
    pub renewed_at: i64,
    /// When it stops being honoured without a renew — see
    /// [`TASK_PROGRESS_EXPIRES_AFTER`].
    pub expires_at: i64,
}

impl SessionStore<'_> {
    /// Declare that a session's current task is nearly complete, or renew the
    /// declaration it already carries — capability map lines 1294 and 1610.
    ///
    /// One row per session: declaring again moves `renewed_at` and
    /// `expires_at` on the row that exists and creates nothing, so *"since
    /// when"* survives a renew.
    ///
    /// **Both refusals are about the caller's own argument.** A session this
    /// project does not have cannot hold a declaration, and neither can one
    /// that has stopped, failed or been closed — a finished session has no
    /// current task to be nearly done with, and a declaration it made would
    /// be honoured by nothing the moment anything read it.
    pub fn declare_task_nearly_complete(
        &self,
        session: &SessionId,
    ) -> Result<TaskProgressDeclaration, SessionStoreError> {
        let now = (self.clock)();
        let action = "record a task-progress declaration";

        self.in_a_write_transaction(action, || {
            let Some(record) = self.get(session)? else {
                return Err(SessionStoreError::NotFound {
                    id: session.clone(),
                });
            };
            if !record.lifecycle.is_live() {
                return Err(SessionStoreError::NotDeclarable {
                    id: session.clone(),
                    lifecycle: record.lifecycle,
                });
            }

            // Before the write rather than on a timer, for
            // `release_abandoned_locked`'s reason: a sweep needs the write
            // lock, and the write lock is already held exactly here.
            self.withdraw_abandoned_task_progress_locked(now)?;

            self.conn
                .execute(
                    "INSERT INTO task_progress_declarations \
                     (project_id, session_id, declared_at, renewed_at, expires_at) \
                     VALUES (?1, ?2, ?3, ?3, ?4) \
                     ON CONFLICT (session_id) DO UPDATE SET \
                     renewed_at = excluded.renewed_at, expires_at = excluded.expires_at",
                    rusqlite::params![
                        self.project_id,
                        session.as_str(),
                        now,
                        now + TASK_PROGRESS_EXPIRES_AFTER,
                    ],
                )
                .map_err(|source| SessionStoreError::Sql { action, source })?;

            self.task_progress_of_locked(session)?
                .ok_or(SessionStoreError::Sql {
                    action,
                    source: rusqlite::Error::QueryReturnedNoRows,
                })
        })
    }

    /// Withdraw one session's declaration. `false` if it carried none.
    ///
    /// The explicit half of the horizon: somebody who finishes the task, or
    /// who declared the wrong session, says so rather than waiting out
    /// [`TASK_PROGRESS_EXPIRES_AFTER`].
    pub fn withdraw_task_progress(&self, session: &SessionId) -> Result<bool, SessionStoreError> {
        let action = "withdraw a task-progress declaration";
        let removed = self
            .conn
            .execute(
                "DELETE FROM task_progress_declarations \
                 WHERE project_id = ?1 AND session_id = ?2",
                rusqlite::params![self.project_id, session.as_str()],
            )
            .map_err(|source| SessionStoreError::Sql { action, source })?;
        Ok(removed > 0)
    }

    /// Every declaration this project currently honours, oldest first.
    ///
    /// **A read can never return an expired or abandoned declaration.** The
    /// filter is in the query and not in the caller: a declaration past
    /// [`TASK_PROGRESS_EXPIRES_AFTER`], and one whose session has exited,
    /// failed or been closed, are both invisible here whether or not a write
    /// has swept the row out of the table yet. That is what makes "never
    /// sticky" a property of the storage rather than a rule every reader has
    /// to remember.
    pub fn active_task_progress(&self) -> Result<Vec<TaskProgressDeclaration>, SessionStoreError> {
        let action = "list active task-progress declarations";
        let now = (self.clock)();
        let mut statement = self
            .conn
            .prepare(&format!(
                "SELECT session_id, declared_at, renewed_at, expires_at \
                 FROM task_progress_declarations \
                 WHERE project_id = ?1 AND expires_at > ?2 \
                   AND session_id IN ({LIVE_SESSIONS}) \
                 ORDER BY declared_at ASC, session_id ASC"
            ))
            .map_err(|source| SessionStoreError::Sql { action, source })?;
        let rows = statement
            .query_map(
                rusqlite::params![
                    self.project_id,
                    now,
                    LIVE_LIFECYCLES[0].as_str(),
                    LIVE_LIFECYCLES[1].as_str(),
                    LIVE_LIFECYCLES[2].as_str(),
                    LIVE_LIFECYCLES[3].as_str(),
                ],
                read_declaration,
            )
            .map_err(|source| SessionStoreError::Sql { action, source })?;

        let mut declared = Vec::new();
        for row in rows {
            declared.push(row.map_err(|source| SessionStoreError::Sql { action, source })?);
        }
        Ok(declared)
    }

    /// The identifiers of the sessions [`Self::active_task_progress`]
    /// reports, in the shape the routers scope themselves with.
    ///
    /// The routing layer's question is membership — *is the session I am
    /// deciding for one of the sessions that declared?* — and both routers
    /// already answer the identical question about the user's reserve
    /// override with a set. This hands them the same shape so neither has to
    /// know that a declaration is a row, and so the set is built once per
    /// decision rather than queried per candidate.
    pub fn sessions_declaring_task_nearly_complete(
        &self,
    ) -> Result<BTreeSet<String>, SessionStoreError> {
        Ok(self
            .active_task_progress()?
            .into_iter()
            .map(|declared| declared.session_id.as_str().to_owned())
            .collect())
    }

    /// Delete the declarations that are past their horizon or owned by a
    /// session that is no longer live.
    ///
    /// Housekeeping, not the guarantee — [`Self::active_task_progress`]
    /// cannot report one of these whether or not this has run. What this adds
    /// is that the row goes away.
    fn withdraw_abandoned_task_progress_locked(
        &self,
        now: i64,
    ) -> Result<usize, SessionStoreError> {
        self.conn
            .execute(
                &format!(
                    "DELETE FROM task_progress_declarations \
                     WHERE project_id = ?1 \
                       AND (expires_at <= ?2 OR session_id NOT IN ({LIVE_SESSIONS}))"
                ),
                rusqlite::params![
                    self.project_id,
                    now,
                    LIVE_LIFECYCLES[0].as_str(),
                    LIVE_LIFECYCLES[1].as_str(),
                    LIVE_LIFECYCLES[2].as_str(),
                    LIVE_LIFECYCLES[3].as_str(),
                ],
            )
            .map_err(|source| SessionStoreError::Sql {
                action: "withdraw abandoned task-progress declarations",
                source,
            })
    }

    /// One session's declaration as stored, without the liveness filter —
    /// what [`Self::declare_task_nearly_complete`] hands back to its caller.
    fn task_progress_of_locked(
        &self,
        session: &SessionId,
    ) -> Result<Option<TaskProgressDeclaration>, SessionStoreError> {
        self.conn
            .query_row(
                "SELECT session_id, declared_at, renewed_at, expires_at \
                 FROM task_progress_declarations \
                 WHERE project_id = ?1 AND session_id = ?2",
                rusqlite::params![self.project_id, session.as_str()],
                read_declaration,
            )
            .optional()
            .map_err(|source| SessionStoreError::Sql {
                action: "read a task-progress declaration back",
                source,
            })
    }
}

fn read_declaration(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskProgressDeclaration> {
    Ok(TaskProgressDeclaration {
        session_id: SessionId::new(row.get::<_, String>(0)?),
        declared_at: row.get(1)?,
        renewed_at: row.get(2)?,
        expires_at: row.get(3)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [`LIVE_LIFECYCLES`] is what the SQL binds and
    /// [`SessionLifecycle::is_live`] is what the rest of the crate asks. A
    /// new lifecycle classified in one place and not the other would make a
    /// declaration invisible, or a dead session's declaration honoured, with
    /// nothing failing.
    #[test]
    fn live_lifecycles_are_exactly_the_live_ones() {
        for lifecycle in [
            SessionLifecycle::Starting,
            SessionLifecycle::Running,
            SessionLifecycle::Idle,
            SessionLifecycle::WaitingForUser,
            SessionLifecycle::Stopped,
            SessionLifecycle::Failed,
            SessionLifecycle::Closed,
        ] {
            assert_eq!(
                LIVE_LIFECYCLES.contains(&lifecycle),
                lifecycle.is_live(),
                "`{}` is classified differently by LIVE_LIFECYCLES and is_live",
                lifecycle.as_str()
            );
        }
    }
}
