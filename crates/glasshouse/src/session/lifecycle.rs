//! Turning a harness's own lifecycle events into Glasshouse's.
//!
//! A harness reports what happened in its own vocabulary — Claude Code says
//! `UserPromptSubmit`, Codex says `user_prompt_submit` — and Glasshouse
//! records one of a handful of states that mean something to a session
//! overview. This module is the translation, and it is deliberately the only
//! place that knows both vocabularies at once.
//!
//! # Why an unknown event changes nothing
//!
//! Harnesses gain events between releases. An event this build has never
//! heard of must leave the session exactly as it was, because the alternative
//! — guessing a state from an unfamiliar name — would show the user a session
//! that is idle when it is working, or working when it is waiting for them.
//!
//! # Why a finished session cannot be revived
//!
//! Hook processes are separate processes, and a slow one can deliver its event
//! after the harness it belongs to has exited. Applying it would resurrect a
//! stopped session in the records, which is worse than losing a note about a
//! session that has already ended.

use crate::session::SessionLifecycle;

/// The Glasshouse state a harness event implies, or `None` when the event
/// says nothing about the session's state.
///
/// Event names are the harness's own, exactly as its adapter declares them.
pub fn lifecycle_for(event: &str) -> Option<SessionLifecycle> {
    match event {
        // Claude Code. A prompt was submitted, so the session is working.
        "UserPromptSubmit" => Some(SessionLifecycle::Running),
        // The harness is asking the user to allow something and will not
        // proceed until they answer.
        "PermissionRequest" => Some(SessionLifecycle::WaitingForUser),
        // The turn ended. `StopFailure` is the same state: the turn is over
        // and the session is alive and waiting for whatever comes next — the
        // *session* has not failed, and recording it as failed would make a
        // perfectly usable session look dead.
        "Stop" | "StopFailure" => Some(SessionLifecycle::Idle),
        _ => None,
    }
}

/// Whether `current` may be moved to `next` by a harness event.
///
/// Only a live session can change state this way. A session that has stopped,
/// failed, or been closed is finished, and a hook arriving afterwards — from a
/// process that outlived its harness — must not bring it back.
pub fn may_apply(current: SessionLifecycle, next: SessionLifecycle) -> bool {
    current.is_live() && current != next
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_codes_events_translate_to_the_states_they_mean() {
        assert_eq!(
            lifecycle_for("UserPromptSubmit"),
            Some(SessionLifecycle::Running)
        );
        assert_eq!(
            lifecycle_for("PermissionRequest"),
            Some(SessionLifecycle::WaitingForUser)
        );
        assert_eq!(lifecycle_for("Stop"), Some(SessionLifecycle::Idle));
    }

    #[test]
    fn a_failed_turn_leaves_the_session_alive() {
        // `StopFailure` is a turn that ended badly, not a session that died.
        // Recording it as failed would make a perfectly usable session look
        // dead in every listing.
        assert_eq!(lifecycle_for("StopFailure"), Some(SessionLifecycle::Idle));
    }

    #[test]
    fn an_unfamiliar_event_changes_nothing() {
        // Harnesses gain events between releases. Guessing a state from an
        // unfamiliar name would show a session as idle while it works, or
        // working while it waits for the user.
        for unknown in ["SessionStart", "PreToolUse", "Notification", "", "stop"] {
            assert_eq!(lifecycle_for(unknown), None, "{unknown}");
        }
    }

    #[test]
    fn a_finished_session_is_never_revived_by_a_late_hook() {
        // Hook processes outlive their harness. One arriving after the session
        // ended must not bring it back.
        for finished in [
            SessionLifecycle::Stopped,
            SessionLifecycle::Failed,
            SessionLifecycle::Closed,
        ] {
            for next in [
                SessionLifecycle::Running,
                SessionLifecycle::Idle,
                SessionLifecycle::WaitingForUser,
            ] {
                assert!(
                    !may_apply(finished, next),
                    "{finished:?} was revived as {next:?}"
                );
            }
        }
    }

    #[test]
    fn a_live_session_follows_its_harness() {
        assert!(may_apply(
            SessionLifecycle::Running,
            SessionLifecycle::WaitingForUser
        ));
        assert!(may_apply(
            SessionLifecycle::Starting,
            SessionLifecycle::Running
        ));
        // A state that is already current is not a change.
        assert!(!may_apply(
            SessionLifecycle::Running,
            SessionLifecycle::Running
        ));
    }
}
