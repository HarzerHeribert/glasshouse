//! The internal API for driving and inspecting a live session.
//!
//! [`SessionApi`] is the one surface that sends text to, interrupts, or
//! inspects a session by identifier — the seam an orchestrator, the MCP
//! surface, or anything else internal to Glasshouse goes through instead of
//! reaching into [`super::store::SessionStore`] and [`super::runtime::SessionRuntime`]
//! directly.
//! Project scope is checked once, here, for every entry point: every method
//! resolves the identifier through the store first and compares its
//! `project_id` against the active project before doing anything else — a
//! foreign session that also happens to be stopped is still refused as
//! foreign, never as merely not running.
//! Who sent a message is recorded, never inferred: every write goes through
//! [`super::runtime::SessionRuntime::send_text_from`] and
//! [`super::runtime::SessionRuntime::interrupt_from`] with an origin its
//! **caller** supplies, not the plain `send_text` / `interrupt` that assume
//! a person's keyboard — Glasshouse callers still pass `Machine`, and the
//! control door passes what its request said, defaulting to `Machine` when
//! it said nothing.
//! History: design-decisions.md, "Trims: memory and session module docs", session/api/mod.rs module doc.

use crate::events::MessageOrigin;

use super::{
    RuntimeError, SessionId, SessionLifecycle, SessionRecord, SessionRuntime, SessionStore,
    SessionStoreError,
};

/// Why a call into [`SessionApi`] could not be carried out.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("no session `{id}` in this project")]
    NotFound { id: SessionId },
    #[error(
        "session `{id}` belongs to project `{actual}`, not to the active \
         project `{expected}`; refusing to act on another project's session"
    )]
    ForeignProject {
        id: SessionId,
        expected: String,
        actual: String,
    },
    #[error("session `{id}` is not live in this Glasshouse")]
    NotLive { id: SessionId },
    #[error(
        "a person has been typing into session `{id}`; machine messages to it are refused \
         for another {seconds}s so they do not land in the middle of what that person is \
         doing. The user has the keyboard. An interrupt is never refused this way."
    )]
    UserHasTheKeyboard { id: SessionId, seconds: u64 },
    #[error(transparent)]
    Store(#[from] SessionStoreError),
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
}

/// The internal surface for sending to, interrupting, and inspecting one
/// project's sessions.
///
/// Borrows both halves it coordinates rather than owning either: the store
/// is the project's durable record, the runtime is whichever live sessions
/// this Glasshouse process actually holds, and neither belongs to this type.
pub struct SessionApi<'a> {
    store: &'a SessionStore<'a>,
    live: &'a mut SessionRuntime,
}

impl<'a> SessionApi<'a> {
    pub fn new(store: &'a SessionStore<'a>, live: &'a mut SessionRuntime) -> Self {
        Self { store, live }
    }

    /// Look a session up and confirm it belongs to the active project.
    ///
    /// The one check every other method starts with. It is deliberately
    /// unconcerned with liveness — that is a separate question a caller asks
    /// afterwards — so that a foreign session is always refused for being
    /// foreign, never for whatever else might also be true about it.
    fn resolve(&self, id: &SessionId) -> Result<SessionRecord, ApiError> {
        let record = self
            .store
            .get(id)?
            .ok_or_else(|| ApiError::NotFound { id: id.clone() })?;

        if record.project_id != self.store.project_id() {
            return Err(ApiError::ForeignProject {
                id: id.clone(),
                expected: self.store.project_id().to_owned(),
                actual: record.project_id,
            });
        }

        Ok(record)
    }

    /// Every session in the active project, most recently active first.
    ///
    /// Filtered by project here as well as trusting the store, so that a row
    /// which should never exist — one bearing another project's identifier,
    /// however it got into this database — cannot surface in a listing even
    /// though [`SessionStore::list`] itself has nothing to filter by; see
    /// that module's doc comment for why the store does not filter.
    pub fn list(&self) -> Result<Vec<SessionRecord>, ApiError> {
        Ok(self
            .store
            .list()?
            .into_iter()
            .filter(|record| record.project_id == self.store.project_id())
            .collect())
    }

    /// The lifecycle state of one session, as the store recorded it.
    pub fn state(&self, id: &SessionId) -> Result<SessionLifecycle, ApiError> {
        Ok(self.resolve(id)?.lifecycle)
    }

    /// Send a line of text to a live session, on behalf of `origin`.
    /// A carriage return is appended, the same way `shell::send_session_text`
    /// sends a line typed at the shell's own prompt: this call delivers one
    /// line, not raw bytes.
    /// `origin` is the caller's to state and this method's to record — see
    /// the module doc comment for why it is no longer decided here. Pass
    /// [`MessageOrigin::Machine`] for anything Glasshouse itself originates;
    /// only the control door has a caller it did not write.
    /// A person at this session's keyboard outranks a machine (line 1719):
    /// machine text is **refused** with [`ApiError::UserHasTheKeyboard`]
    /// while a person has put something into this same session within
    /// [`crate::session::runtime::USER_INPUT_PRECEDENCE`] — refused rather
    /// than queued, the same rule `super::runtime::SessionRuntime::deliver`
    /// already applies to a concurrent delivery.
    /// Taken **here**, at the one seam every machine sender in this process
    /// passes through, so there is no machine write path quietly exempt from
    /// it. Deliberately **not** applied to [`SessionApi::interrupt`]: see
    /// that method.
    /// History: design-decisions.md, "Trims: memory and session module docs", session/api/mod.rs send_text.
    pub fn send_text(
        &mut self,
        id: &SessionId,
        text: &str,
        origin: MessageOrigin,
    ) -> Result<(), ApiError> {
        self.resolve(id)?;
        if self.live.get(id).is_none() {
            return Err(ApiError::NotLive { id: id.clone() });
        }
        if origin == MessageOrigin::Machine
            && let Some(refusal) = self.machine_delivery_refusal(id)
        {
            return Err(refusal);
        }
        let mut line = String::with_capacity(text.len() + 1);
        line.push_str(text);
        line.push('\r');
        self.live.send_text_from(id, &line, origin)?;
        Ok(())
    }

    /// The refusal a machine-originated line to `id` would be given right
    /// now, or `None` if it would be delivered — capability map line 1719.
    ///
    /// [`SessionApi::send_text`] takes this decision itself, so no caller has
    /// to ask, and it is **private on purpose**: it was briefly public so the
    /// control door could refuse a machine message before opening this
    /// project's memory store for a briefing it was about to throw away, but
    /// with a copy of the check in front of this seam, a mutation of the
    /// check *inside* `send_text` left the entire suite green — a rule with
    /// two enforcement points is a rule with one that nobody watches.
    ///
    /// So there is one enforcement point and this is its only caller; the
    /// wasted memory open on a refused message is paid only on the path
    /// where a person is already using the session. It reads state and
    /// changes none, and resolves through the same project-scope check
    /// every other method starts with.
    ///
    /// History: design-decisions.md, "Trims: memory and session module docs", session/api/mod.rs machine_delivery_refusal.
    fn machine_delivery_refusal(&self, id: &SessionId) -> Option<ApiError> {
        if let Err(err) = self.resolve(id) {
            return Some(err);
        }
        let remaining = self
            .live
            .user_input_precedence(id, std::time::Instant::now())?;
        Some(ApiError::UserHasTheKeyboard {
            id: id.clone(),
            // Rounded **up**: a refusal that says `0s` while still refusing
            // reads as a bug, and the caller's next question is "how long do
            // I wait", which has to be an answer that works.
            seconds: remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0),
        })
    }

    /// Interrupt a live session, on behalf of `origin`.
    ///
    /// An interrupt is an intervention like any other line, and carries the
    /// same attribution for the same reason: `origin` is the caller's to
    /// state. See [`SessionApi::send_text`].
    ///
    /// # It is never refused for line 1719, and never muted for line 1717
    ///
    /// Both of those controls exist so a person is not talked over. An
    /// interrupt is not talking: it is the one verb that *stops* a session,
    /// and it is what a person reaches for when a worker is running away with
    /// itself. A control that could leave a runaway harness unstoppable for
    /// ten seconds — or for however long a mute was set — would have taken
    /// something away in the name of giving the person control. So text is
    /// held back and a stop never is.
    pub fn interrupt(&mut self, id: &SessionId, origin: MessageOrigin) -> Result<(), ApiError> {
        self.resolve(id)?;
        if self.live.get(id).is_none() {
            return Err(ApiError::NotLive { id: id.clone() });
        }
        self.live.interrupt_from(id, origin)?;
        Ok(())
    }

    /// The most recent terminal output of a session, at most `max_bytes`,
    /// cut at a character boundary.
    ///
    /// Glasshouse does not persist terminal output yet, so a session with no
    /// live process has none to give: returning an empty string would be a
    /// lie the caller has no way to detect, so this refuses with
    /// [`ApiError::NotLive`] instead.
    pub fn recent_output(&self, id: &SessionId, max_bytes: usize) -> Result<String, ApiError> {
        self.resolve(id)?;
        let session = self
            .live
            .get(id)
            .ok_or_else(|| ApiError::NotLive { id: id.clone() })?;
        Ok(session.with_scrollback(|scrollback| tail(&scrollback.text(), max_bytes)))
    }
}

/// The last `max_bytes` of `text`, advanced to the next UTF-8 character
/// boundary so the result never opens with a severed character.
///
/// Advancing forward — never backward — means the returned string can be
/// shorter than `max_bytes` when the cut point lands inside a character, but
/// never longer, and never invalid.
fn tail(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_owned();
    }
    let cut = text.len() - max_bytes;
    let start = (cut..=text.len())
        .find(|&index| text.is_char_boundary(index))
        .unwrap_or(text.len());
    text[start..].to_owned()
}

#[cfg(test)]
mod tests;
