//! One session's inbox — capability map line 2479, migration 29.
//!
//! A row here is a line somebody sent to a session that has no terminal to
//! type it into. Every other recipient is written to through
//! [`crate::session::runtime::SessionRuntime`], which stores nothing: the
//! bytes go to a pseudo-terminal and Glasshouse keeps a byte count. A harness
//! that reads its input as a batch per turn instead has nowhere for those
//! bytes to go, so they rest here until it asks — and asking is a cursor, not
//! a drain, so two readers of one inbox see the same messages rather than
//! racing to take them.
//!
//! **`seq` is one counter for the whole project, and a page is ordered by it
//! within one session.** It is never handed out as an identity across
//! sessions: [`SessionStore::session_messages`] filters by session first, and
//! [`SessionStore::session_messages_head`] answers the highest `seq` *that
//! session* has, so a caller's cursor advances only when its own session is
//! written to.
//!
//! Rows are never deleted here. Retention is a successor: nothing in this
//! build removes a message once it is stored, and an inbox therefore grows
//! with everything ever sent to its session.

use rusqlite::OptionalExtension;

use super::{SessionId, SessionStore, SessionStoreError};

/// One inbound message, as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMessage {
    /// This message's position in the project's message counter — the cursor
    /// a reader hands back as `after`. See the module doc for why it is
    /// project-wide and still a per-session cursor.
    pub seq: i64,
    /// The recipient.
    pub session: SessionId,
    /// Who the sender said it was, or `None` when it said nothing.
    ///
    /// **Attribution, never authentication.** Nothing verified this string;
    /// the control door's peer-uid check is the whole authorization, exactly
    /// as it is for `origin`. A reader that treats this as proof of identity
    /// has believed something no part of Glasshouse checked.
    pub sender: Option<String>,
    /// The line that was sent, verbatim.
    pub body: String,
    /// Seconds since the Unix epoch, from this store's clock.
    pub at: i64,
}

impl SessionStore<'_> {
    /// Append one message to `session`'s inbox and return its `seq`.
    ///
    /// The row is this project's by construction: `project_id` is the store's
    /// own, which is the identifier migration 29's triggers compare against,
    /// so a message for another project's session cannot be written even by a
    /// caller that resolved the wrong identifier.
    ///
    /// Deliberately **does not** check that `session` exists or is live. The
    /// control door resolves the recipient through
    /// [`crate::session::api::SessionApi`] before it gets here — that is
    /// where a foreign or unknown session is refused, and refusing it twice
    /// in two vocabularies would give one rule two enforcement points that
    /// can drift. A message for a session this database later forgets stays a
    /// fact worth keeping, which is migration 5's own posture and why there
    /// is no `REFERENCES sessions(id)`.
    pub fn append_session_message(
        &self,
        session: &SessionId,
        sender: Option<&str>,
        body: &str,
    ) -> Result<i64, SessionStoreError> {
        let action = "append a message to a session's inbox";
        let at = (self.clock)();
        self.conn
            .execute(
                "INSERT INTO session_messages (project_id, session, sender, body, at) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![self.project_id, session.as_str(), sender, body, at],
            )
            .map_err(|source| SessionStoreError::Sql { action, source })?;
        Ok(self.conn.last_insert_rowid())
    }

    /// One session's messages after `after`, oldest first, at most `limit`.
    ///
    /// `after` is exclusive and `0` means the start of this session's inbox,
    /// exactly as `Request::Events`' cursor reads. The bound is the caller's
    /// to lower and the door's to cap; this applies whatever it is given.
    pub fn session_messages(
        &self,
        session: &SessionId,
        after: i64,
        limit: usize,
    ) -> Result<Vec<SessionMessage>, SessionStoreError> {
        let action = "read a session's inbox";
        let mut statement = self
            .conn
            .prepare(
                "SELECT seq, session, sender, body, at FROM session_messages \
                 WHERE project_id = ?1 AND session = ?2 AND seq > ?3 \
                 ORDER BY seq ASC LIMIT ?4",
            )
            .map_err(|source| SessionStoreError::Sql { action, source })?;
        let rows = statement
            .query_map(
                rusqlite::params![self.project_id, session.as_str(), after, limit as i64],
                read_message,
            )
            .map_err(|source| SessionStoreError::Sql { action, source })?;

        let mut messages = Vec::new();
        for row in rows {
            messages.push(row.map_err(|source| SessionStoreError::Sql { action, source })?);
        }
        Ok(messages)
    }

    /// The highest `seq` this session's inbox holds, or `0` when it is empty.
    ///
    /// The cursor a reader hands back next time, returned whether or not the
    /// page it came with had anything in it — `Request::Events`' rule, for
    /// its reason: a caller whose page was cut short by `limit` still needs
    /// to know how far behind it is, and a caller whose page was empty still
    /// needs a cursor to resume from.
    pub fn session_messages_head(&self, session: &SessionId) -> Result<i64, SessionStoreError> {
        let action = "read the head of a session's inbox";
        let head: Option<i64> = self
            .conn
            .query_row(
                "SELECT MAX(seq) FROM session_messages \
                 WHERE project_id = ?1 AND session = ?2",
                rusqlite::params![self.project_id, session.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|source| SessionStoreError::Sql { action, source })?
            .flatten();
        Ok(head.unwrap_or(0))
    }
}

fn read_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionMessage> {
    Ok(SessionMessage {
        seq: row.get(0)?,
        session: SessionId::new(row.get::<_, String>(1)?),
        sender: row.get(2)?,
        body: row.get(3)?,
        at: row.get(4)?,
    })
}
