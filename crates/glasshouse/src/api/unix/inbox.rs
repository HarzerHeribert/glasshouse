//! The two halves of a session inbox on this door — capability map line
//! 2479: [`deliver`], which stores a message for a recipient that has
//! no terminal to type it into, and [`inbox`], which hands one session's
//! messages out against a cursor.
//!
//! **The recipient's harness decides delivery, and nothing else does.** Every
//! session Glasshouse holds is written to by typing into its pseudo-terminal;
//! a harness that reads its input as one batch per turn has no terminal for
//! that, so a message to it is stored instead. That is the *only* thing this
//! module changes about `Request::SendMessage`: a recipient under any other
//! harness falls through to the path it has always taken, with the same
//! refusals, the same memory briefing and the same policy delivery.
//!
//! **A briefing is typed into a terminal, and an inbox is not one.** The
//! memory selection and policy delivery `super::dispatch` performs on the
//! terminal path are deliberately not performed here — recorded as a limit of
//! this package rather than an oversight, and a successor if dogfooding asks
//! for them.
//!
//! **Nothing here puts a message body anywhere a body has not always been.**
//! The lifecycle stream gets the same `TextDelivered { origin, bytes }` the
//! terminal path publishes — a byte count, never the text — so every
//! `Request::Events` reader sees that a delivery happened and no more. The
//! body rests in `session_messages` and comes back out of [`inbox`], and
//! nowhere else: not in an error message, not in this door's stderr.

use std::sync::Mutex;

use glasshouse::events::LifecycleEvent;
use glasshouse::integrations::IntegrationId;
use glasshouse::session::api::SessionApi;
use glasshouse::session::{SessionId, SessionRuntime, SessionStore};

use super::events::MAX_EVENTS_LIMIT;
use super::{api_error, lock};
use crate::api::protocol::{RequestOrigin, Response};

/// Store `text` for `id` and answer, or `None` if this recipient is not one
/// whose messages are stored.
///
/// **`pane` here is the harness, not a cmux workspace.** `sessions.rs`'s
/// `send_through_pane`, thirty lines below this function's one call site, is
/// about a session *presented* in a cmux pane and reached by running `cmux`;
/// this is about a session whose recorded `harness` is the `pane` binary,
/// which reads its input as a batch per turn. The two are unrelated and a
/// message can be neither, either or — in principle — both, in which case
/// this decides first, because where a session is shown says nothing about
/// how it reads.
///
/// `None` is the whole of how the terminal path stays untouched: the caller
/// falls through to it, and every refusal a message to a non-pane session
/// could earn is still produced by that path in its own words. That includes
/// a session this project does not have — resolving it here and reporting
/// the refusal from here would move a message to `claude-code` onto a
/// different error path than the one it has today, which is exactly what this
/// package must not do.
///
/// The order inside a `SendMessage` matters and is the ruling's: the mute
/// (line 1717) is checked by the caller *before* this runs, so a muted
/// session's inbox is never written to.
pub(super) fn deliver(
    store: &SessionStore<'_>,
    live: &Mutex<SessionRuntime>,
    id: &SessionId,
    text: &str,
    from: Option<&str>,
    origin: RequestOrigin,
) -> Option<Response> {
    // Scope through `SessionApi::state`, the same seam the mute check uses,
    // before the record is read for its harness: a session that is not this
    // project's must not be looked at here at all, and `state` is where that
    // is decided for every other verb on this door. The record itself then
    // comes from the store, as `send_through_pane` already reads it — the
    // `SessionApi` surface answers a lifecycle, and the question here is
    // which harness owns the session.
    {
        let mut guard = lock(live);
        let api = SessionApi::new(store, &mut guard);
        api.state(id).ok()?;
    }
    let record = store.get(id).ok()??;
    if record.harness != IntegrationId::Pane.slug() {
        return None;
    }

    let seq = match store.append_session_message(id, from, text) {
        Ok(seq) => seq,
        Err(err) => return Some(Response::err(err)),
    };

    // Published after the row is committed, so `Request::Events` never
    // reports a delivery that did not happen. Onto the runtime's own bus —
    // the one `ServerContext::open` built and attached the recorder to — so
    // this reaches the durable log by exactly the path a terminal delivery
    // reaches it, rather than by a second writer that could drift.
    {
        let guard = lock(live);
        guard.events().publish(
            id,
            LifecycleEvent::TextDelivered {
                origin: origin.message_origin(),
                bytes: text.len(),
            },
        );
    }

    Some(Response::ok(serde_json::json!({
        "via": "inbox",
        "seq": seq,
    })))
}

/// One session's messages after `after` — `Request::Inbox`.
///
/// Refuses a session that is not this project's with the same sentence every
/// other verb here gives it, and caps `limit` at [`MAX_EVENTS_LIMIT`]
/// regardless of what was asked for: a caller may lower the ceiling and
/// cannot raise it, which is `project_events`' own rule and for its reason.
pub(super) fn inbox(
    store: &SessionStore<'_>,
    live: &Mutex<SessionRuntime>,
    session: String,
    after: i64,
    limit: usize,
) -> Response {
    let id = SessionId::new(session);
    {
        let mut guard = lock(live);
        let api = SessionApi::new(store, &mut guard);
        if let Err(err) = api.state(&id) {
            return Response::err(api_error(err));
        }
    }

    let messages = match store.session_messages(&id, after, limit.min(MAX_EVENTS_LIMIT)) {
        Ok(messages) => messages,
        Err(err) => return Response::err(err),
    };
    // Read after the page, so a message stored between the two shows up as a
    // `head` the caller has not reached rather than as a page it can never
    // ask for again.
    let head = match store.session_messages_head(&id) {
        Ok(head) => head,
        Err(err) => return Response::err(err),
    };

    Response::ok(serde_json::json!({
        "messages": messages
            .iter()
            .map(|message| serde_json::json!({
                "seq": message.seq,
                // `sender` on the row, `from` on the wire: the wire word is
                // the one `Request::SendMessage` accepts, and a caller that
                // sends `from` and reads `sender` back would have to learn
                // two names for one fact.
                "from": message.sender,
                "text": message.body,
                "at": message.at,
            }))
            .collect::<Vec<_>>(),
        "head": head,
    }))
}
