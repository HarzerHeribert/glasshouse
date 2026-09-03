//! Soft, project-scoped, turn-scoped file claims — capability map lines 2392
//! to 2398, Phase 60's A+F slice.
//!
//! # What a claim is, and the four things it is not
//!
//! A claim is one row saying *"this Glasshouse session is working on this
//! file, and still wanted it as of this second."* It is **coordination
//! metadata**. Taking one never blocks, never locks, never changes a file's
//! permissions, and never fails another session's write; two sessions may
//! hold a claim on the same path, and that is the overlap a later package
//! reports rather than an error raised here. Nothing in this build consults a
//! claim before deciding anything.
//!
//! # It belongs to a session, never to a process
//!
//! Line 2396. The owner is a [`SessionId`], so a recycled process identifier
//! can never resolve to a live claim — there is no process identifier here to
//! recycle. A claim for a session this project does not have is refused
//! before a row exists.
//!
//! # Project isolation
//!
//! Line 2397, and it holds three times over: the database file *is* the
//! project, migration 27's two triggers refuse a row whose `project_id` is
//! not the bound one, and every statement below also names `project_id`
//! explicitly. A claim taken in one project cannot be named by a query in
//! another.

use rusqlite::OptionalExtension;

use super::{SessionId, SessionLifecycle, SessionStore, SessionStoreError};

/// How long a claim nobody renewed stays active — line 2394's *"safe
/// stale-claim timeout"*.
///
/// # Why two hours
///
/// This is a **backstop**, not the ordinary release path. A claim is normally
/// released when the turn ends (`commands::hook`'s `TurnEnded` arm), and a
/// claim whose owning session has stopped or failed is neither reported by
/// [`SessionStore::active_claims`] nor kept, whatever the clock says. What is
/// left for a timeout is the case both of those miss: a machine that lost power,
/// or a harness killed hard enough that no hook ran and no lifecycle write
/// landed.
///
/// The two failure directions are not symmetric. Too short expires a claim
/// under a session that is still editing, and the claim silently stops
/// describing real work. Too long leaves a ghost that outlives the machine it
/// was made on. Two hours is longer than any single harness turn — a turn is
/// one prompt-to-stop cycle, minutes rather than hours, and a session working
/// for longer than that renews as it goes — and short enough that a claim
/// orphaned by a crash does not survive the working day. It is a judgement,
/// not a measurement, and it is one constant so that changing it is one edit.
pub const STALE_CLAIM_AFTER: i64 = 2 * 60 * 60;

/// The lifecycle states whose sessions may hold a claim.
///
/// Exactly [`SessionLifecycle::is_live`]'s `true` arm, listed here because
/// SQL cannot call it — the claim queries bind these four words. Pinned
/// against `is_live` by `live_lifecycles_are_exactly_the_live_ones` below, so
/// an eighth lifecycle cannot be added on one side only.
const LIVE_LIFECYCLES: [SessionLifecycle; 4] = [
    SessionLifecycle::Starting,
    SessionLifecycle::Running,
    SessionLifecycle::Idle,
    SessionLifecycle::WaitingForUser,
];

/// One active claim, as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileClaim {
    /// The Glasshouse session that holds it — line 2396.
    pub session_id: SessionId,
    /// Repo-relative, `/`-separated, UTF-8: the spelling
    /// [`crate::memory::normalize_observed_path`] defines, which is the same
    /// one `memory_files.path` stores.
    pub path: String,
    /// When the claim was first taken. A renew does not move it.
    pub claimed_at: i64,
    /// When it was last renewed, or [`FileClaim::claimed_at`] if never.
    pub renewed_at: i64,
    /// When it stops being active without a renew — see
    /// [`STALE_CLAIM_AFTER`].
    pub expires_at: i64,
}

/// The `sessions` subquery both statements share: the live sessions of this
/// project. Written once so the two cannot drift apart.
const LIVE_SESSIONS: &str = "SELECT id FROM sessions WHERE lifecycle IN (?3, ?4, ?5, ?6)";

impl SessionStore<'_> {
    /// Claim a file for a session, or renew the claim it already holds —
    /// lines 2392 and 2395.
    ///
    /// One row per (session, path): a session claiming a file it already
    /// holds moves `renewed_at` and `expires_at` on that row and creates
    /// nothing, which is line 2395. `claimed_at` is left alone, so *"since
    /// when"* survives a renew.
    ///
    /// **Never blocks and never refuses on another session's account.** A
    /// path another session already claims is claimed anyway. The two refusals
    /// are about the caller's own arguments: a path that cannot be brought to
    /// the canonical spelling, and a session that is not a live session of
    /// this project.
    pub fn claim_file(
        &self,
        session: &SessionId,
        path: &str,
    ) -> Result<FileClaim, SessionStoreError> {
        let path = normalize_claim_path(path)?;
        let now = (self.clock)();
        let action = "record a file claim";

        self.in_a_write_transaction(action, || {
            let Some(record) = self.get(session)? else {
                return Err(SessionStoreError::NotFound {
                    id: session.clone(),
                });
            };
            if !record.lifecycle.is_live() {
                return Err(SessionStoreError::NotClaimable {
                    id: session.clone(),
                    lifecycle: record.lifecycle,
                });
            }

            // Line 2394, before the write rather than on a timer: whatever
            // else this transaction does, the claims left behind by sessions
            // that are gone do not survive it.
            self.release_abandoned_locked(now)?;

            self.conn
                .execute(
                    "INSERT INTO file_claims \
                     (project_id, session_id, path, claimed_at, renewed_at, expires_at) \
                     VALUES (?1, ?2, ?3, ?4, ?4, ?5) \
                     ON CONFLICT (session_id, path) DO UPDATE SET \
                     renewed_at = excluded.renewed_at, expires_at = excluded.expires_at",
                    rusqlite::params![
                        self.project_id,
                        session.as_str(),
                        path,
                        now,
                        now + STALE_CLAIM_AFTER,
                    ],
                )
                .map_err(|source| SessionStoreError::Sql { action, source })?;

            self.claim_of_locked(session, &path)?
                .ok_or(SessionStoreError::Sql {
                    action,
                    source: rusqlite::Error::QueryReturnedNoRows,
                })
        })
    }

    /// Release one session's claim on one path. `false` if it held none.
    pub fn release_claim(
        &self,
        session: &SessionId,
        path: &str,
    ) -> Result<bool, SessionStoreError> {
        let path = normalize_claim_path(path)?;
        let action = "release a file claim";
        let removed = self
            .conn
            .execute(
                "DELETE FROM file_claims \
                 WHERE project_id = ?1 AND session_id = ?2 AND path = ?3",
                rusqlite::params![self.project_id, session.as_str(), path],
            )
            .map_err(|source| SessionStoreError::Sql { action, source })?;
        Ok(removed > 0)
    }

    /// Release every claim one session holds — line 2393's turn boundary, and
    /// the write `commands::hook`'s `TurnEnded` arm makes.
    ///
    /// Returns how many were released. A session that held none releases
    /// none, which is not an error: most turns claim nothing.
    ///
    /// Deliberately says nothing about *why* the turn ended. A turn that
    /// failed is a turn that finished, so `TurnEnded { Failed }` releases
    /// exactly as `Completed` does.
    pub fn release_claims_of(&self, session: &SessionId) -> Result<usize, SessionStoreError> {
        let action = "release a session's file claims";
        self.conn
            .execute(
                "DELETE FROM file_claims WHERE project_id = ?1 AND session_id = ?2",
                rusqlite::params![self.project_id, session.as_str()],
            )
            .map_err(|source| SessionStoreError::Sql { action, source })
    }

    /// Every active claim in this project, by path and then by age.
    ///
    /// **A read can never return an abandoned claim** — line 2394. The filter
    /// is in the query and not in the caller: a claim past
    /// [`STALE_CLAIM_AFTER`], and a claim whose owning session has exited,
    /// failed or been closed, are both invisible here whether or not a write
    /// has swept them out of the table yet.
    ///
    /// Ordered by path first so that the several sessions claiming one file
    /// are adjacent. That is the surfacing line 2398 asks for; it is not a
    /// conflict verdict, which is a later package's.
    pub fn active_claims(&self) -> Result<Vec<FileClaim>, SessionStoreError> {
        let action = "list active file claims";
        let now = (self.clock)();
        let mut statement = self
            .conn
            .prepare(&format!(
                "SELECT session_id, path, claimed_at, renewed_at, expires_at \
                 FROM file_claims \
                 WHERE project_id = ?1 AND expires_at > ?2 \
                   AND session_id IN ({LIVE_SESSIONS}) \
                 ORDER BY path ASC, claimed_at ASC, session_id ASC"
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
                read_claim,
            )
            .map_err(|source| SessionStoreError::Sql { action, source })?;

        let mut claims = Vec::new();
        for row in rows {
            claims.push(row.map_err(|source| SessionStoreError::Sql { action, source })?);
        }
        Ok(claims)
    }

    /// Delete the claims line 2394 abandons: past their timeout, or owned by
    /// a session that is no longer live.
    ///
    /// # Why this is housekeeping and not the guarantee
    ///
    /// A claim has no effect on anything except by being read, and
    /// [`SessionStore::active_claims`] cannot report an abandoned one — the
    /// filter is in the query. So a claim is *released*, in every sense a
    /// caller can observe, the moment its session stops or its timeout
    /// passes, whether or not this has run. What this adds is that the row
    /// goes away, and it runs inside the next claim written to this project
    /// rather than on a timer: a sweep needs the write lock, and the write
    /// lock is already held exactly there.
    fn release_abandoned_locked(&self, now: i64) -> Result<usize, SessionStoreError> {
        self.conn
            .execute(
                &format!(
                    "DELETE FROM file_claims \
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
                action: "release abandoned file claims",
                source,
            })
    }

    /// One session's claim on one path, as stored, without the liveness
    /// filter — what [`SessionStore::claim_file`] hands back to its caller.
    fn claim_of_locked(
        &self,
        session: &SessionId,
        path: &str,
    ) -> Result<Option<FileClaim>, SessionStoreError> {
        self.conn
            .query_row(
                "SELECT session_id, path, claimed_at, renewed_at, expires_at \
                 FROM file_claims \
                 WHERE project_id = ?1 AND session_id = ?2 AND path = ?3",
                rusqlite::params![self.project_id, session.as_str(), path],
                read_claim,
            )
            .optional()
            .map_err(|source| SessionStoreError::Sql {
                action: "read a file claim back",
                source,
            })
    }
}

fn read_claim(row: &rusqlite::Row<'_>) -> rusqlite::Result<FileClaim> {
    Ok(FileClaim {
        session_id: SessionId::new(row.get::<_, String>(0)?),
        path: row.get(1)?,
        claimed_at: row.get(2)?,
        renewed_at: row.get(3)?,
        expires_at: row.get(4)?,
    })
}

/// The one spelling `file_claims.path` accepts, or a refusal.
///
/// [`crate::memory::normalize_observed_path`] is the definition and this adds
/// nothing to it: claims are compared by exact string equality, so two
/// spellings of one file would be two claims and the overlap a later package
/// looks for would be missed silently. Reusing that function rather than
/// writing a second canonicalisation is what keeps a claimed path and a
/// remembered path the same string.
///
/// **Case is not folded.** `src/a.rs` and `SRC/a.rs` are two claims here even
/// where the filesystem says they are one file, exactly as `memory_files`
/// already behaves. Folding would be wrong on Linux, where they *are* two
/// files, and this module has no business deciding which platform's rule the
/// stored string follows.
fn normalize_claim_path(path: &str) -> Result<String, SessionStoreError> {
    crate::memory::normalize_observed_path(path).ok_or_else(|| SessionStoreError::ClaimPath {
        path: path.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [`LIVE_LIFECYCLES`] is what the SQL binds and
    /// [`SessionLifecycle::is_live`] is what the rest of the crate asks. A
    /// new lifecycle classified in one place and not the other would make a
    /// claim invisible, or a dead session's claim visible, with nothing
    /// failing.
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
