//! What may happen to a task whose session died.
//!
//! Phase 45's three lines, and nothing else: resume a failed task in the same
//! native session when possible, hand it to a fresh session when appropriate,
//! and refuse to retry a destructive task on a different harness without
//! enough task-state information to know that is safe. [`plan`] is the single
//! decision point; it is pure, so a caller supplies everything it needs and
//! reads back exactly one of three outcomes.
//!
//! # What counts as "enough task-state information"
//!
//! Narrowly: a [`CheckpointRef`] and nothing else. A [`TaskState`] may also
//! carry an `event_history` — what the harness reported doing, turn by turn —
//! but [`plan`] never reads it when deciding whether a cross-harness retry is
//! safe. An event history records what happened to the *session*: turns
//! starting and ending, text arriving. It does not record what the *task* had
//! already done to the world, which is the only question a destructive retry
//! needs answered. Only a portable checkpoint answers it, and Phase 19 — which
//! produces checkpoints — is not implemented yet, so in this build the answer
//! is effectively always "no" for a destructive or unknown-kind cross-harness
//! retry. That is the correct outcome, not a gap: the capability map's line
//! asks Glasshouse to *avoid* the retry, and refusing does exactly that.

use crate::session::store::{SessionDisposition, SessionId, SessionLifecycle, SessionRecord};

/// A reference to a portable session checkpoint.
///
/// Opaque on purpose. Phase 19 owns what a checkpoint *is*; this module only
/// needs to know whether one exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointRef(String);

impl CheckpointRef {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

/// What a task would do again if it were retried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    /// Retrying it costs time and nothing else.
    Ordinary,
    /// Retrying it could repeat an effect on the world — a write, a push, a
    /// deployment, a deletion.
    Destructive,
    /// Glasshouse was not told. Treated as `Destructive`, because the cost of
    /// being wrong is asymmetric.
    Unknown,
}

/// What Glasshouse knows about the work a session was doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskState {
    pub kind: TaskKind,
    /// What the harness reported doing, turn by turn, if Glasshouse tracked
    /// it. Provenance for a person reading a report, not proof of safety —
    /// see the module docs for why [`plan`] never reads this field.
    pub event_history: Vec<String>,
    /// A portable checkpoint of what the task had already done to the world,
    /// if one exists. The only field that can make a cross-harness retry of a
    /// destructive or unknown task safe.
    pub checkpoint: Option<CheckpointRef>,
}

/// Where a recovery would put the work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// The same native conversation, in the same harness.
    SameSession,
    /// A new session, in the named harness.
    FreshSession { harness: String },
}

/// What Glasshouse will do about a session whose task did not finish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recovery {
    /// Continue the same native conversation. Carries what a resume needs.
    ResumeInPlace {
        native_session_id: String,
        harness: String,
    },
    /// Start a new session in the named harness.
    FreshSession { harness: String },
    /// Do nothing automatically, and say why.
    Refuse { reason: RefusalReason },
}

/// Why an automatic recovery was refused.
///
/// Every variant's `Display` is read by a person deciding what to do next, so
/// it names the specific session and the specific thing that is missing —
/// the same register as [`crate::session::store::SessionStoreError`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RefusalReason {
    /// Rule 1: a live session is not recovered.
    #[error(
        "session `{id}` is still running; recovering it now would duplicate \
         work that is already in flight"
    )]
    SessionStillRunning { id: SessionId },

    /// Rule 2, when the task was known to be destructive.
    #[error(
        "session `{id}`'s task is destructive and there is no portable \
         checkpoint of what it already did to the world, so it will not be \
         retried on harness `{to_harness}` (it ran on `{from_harness}`)"
    )]
    DestructiveRetryWithoutCheckpoint {
        id: SessionId,
        from_harness: String,
        to_harness: String,
    },

    /// Rule 2, when the task's kind was never recorded. Refused for the same
    /// reason as [`Self::DestructiveRetryWithoutCheckpoint`] — see
    /// [`RefusalReason::is_unaccounted_effect`].
    #[error(
        "session `{id}`'s task kind was never recorded, so Glasshouse cannot \
         tell whether retrying it would repeat an effect on the world; \
         treated as destructive, it will not be retried on harness \
         `{to_harness}` (it ran on `{from_harness}`) without a checkpoint"
    )]
    UnknownTaskRetryWithoutCheckpoint {
        id: SessionId,
        from_harness: String,
        to_harness: String,
    },

    /// Rule 1b: the user retired this record themselves.
    #[error(
        "session `{id}` was closed by the user, so Glasshouse will not reopen \
         it automatically; start it again explicitly if that is what you want"
    )]
    SessionClosedByUser { id: SessionId },

    /// Rule 4: `Target::SameSession` with nothing recorded to resume to.
    #[error(
        "session `{id}` has no recorded native session identifier, so there \
         is nothing for the same harness to resume"
    )]
    NoNativeSessionToResume { id: SessionId },
}

impl RefusalReason {
    /// True for the two variants that refuse a cross-harness retry because
    /// Glasshouse cannot account for what the task already did to the world —
    /// whether the task was known destructive, or its kind was never told.
    /// Rule 2 treats both the same way; this is how a caller (or a test)
    /// checks that it did.
    pub fn is_unaccounted_effect(&self) -> bool {
        matches!(
            self,
            Self::DestructiveRetryWithoutCheckpoint { .. }
                | Self::UnknownTaskRetryWithoutCheckpoint { .. }
        )
    }
}

/// Decide what to do about `record`, given what is known about its task and
/// where the caller proposes to put it.
///
/// The rules apply in order; a later rule never rescues a case an earlier one
/// refused.
pub fn plan(record: &SessionRecord, task: &TaskState, target: &Target) -> Recovery {
    // Rule 1: a live session is not recovered.
    if record.disposition() == SessionDisposition::Active {
        return Recovery::Refuse {
            reason: RefusalReason::SessionStillRunning {
                id: record.id.clone(),
            },
        };
    }

    // Rule 1b: a record the user closed is not reopened automatically.
    //
    // Not folded into rule 1's disposition check, which would be the obvious
    // way and the wrong one: `SessionDisposition::Closed` also covers a
    // stopped session that never had a native identifier, and that case has
    // its own, more useful refusal in rule 4. The lifecycle column is what
    // records a deliberate act by the user, so that is what is read.
    //
    // This is fail-closed in the direction the product invariants point.
    // Closing a session is the user saying they are done with it; quietly
    // reopening it because some automation decided the task was unfinished is
    // exactly the "silently moving work between sessions" that Phase 44
    // forbids by name.
    if record.lifecycle == SessionLifecycle::Closed {
        return Recovery::Refuse {
            reason: RefusalReason::SessionClosedByUser {
                id: record.id.clone(),
            },
        };
    }

    // Rule 2: a destructive-or-unknown task moving to a different harness is
    // refused unless the task state is sufficient.
    if let Target::FreshSession { harness } = target
        && *harness != record.harness
        && let Some(reason) = insufficient_state_reason(record, task, harness)
    {
        return Recovery::Refuse { reason };
    }

    match target {
        // Rule 3 and rule 4: same-session resumes when it can, and refuses
        // rather than silently starting a fresh session when it cannot.
        Target::SameSession => match &record.native_session_id {
            Some(native_session_id) => Recovery::ResumeInPlace {
                native_session_id: native_session_id.clone(),
                harness: record.harness.clone(),
            },
            None => Recovery::Refuse {
                reason: RefusalReason::NoNativeSessionToResume {
                    id: record.id.clone(),
                },
            },
        },
        // Rule 5: otherwise, hand off.
        Target::FreshSession { harness } => Recovery::FreshSession {
            harness: harness.clone(),
        },
    }
}

/// `None` when a cross-harness retry is safe to hand off; `Some` naming why
/// it is not. `task.checkpoint` — never `task.event_history` — is what makes
/// it safe; see the module docs.
fn insufficient_state_reason(
    record: &SessionRecord,
    task: &TaskState,
    to_harness: &str,
) -> Option<RefusalReason> {
    if task.checkpoint.is_some() {
        return None;
    }
    match task.kind {
        TaskKind::Ordinary => None,
        TaskKind::Destructive => Some(RefusalReason::DestructiveRetryWithoutCheckpoint {
            id: record.id.clone(),
            from_harness: record.harness.clone(),
            to_harness: to_harness.to_owned(),
        }),
        TaskKind::Unknown => Some(RefusalReason::UnknownTaskRetryWithoutCheckpoint {
            id: record.id.clone(),
            from_harness: record.harness.clone(),
            to_harness: to_harness.to_owned(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::store::{SessionLifecycle, SessionPresentation, SessionRole};

    fn task(kind: TaskKind) -> TaskState {
        TaskState {
            kind,
            event_history: Vec::new(),
            checkpoint: None,
        }
    }

    fn record_with(
        lifecycle: SessionLifecycle,
        harness: &str,
        native_session_id: Option<&str>,
    ) -> SessionRecord {
        SessionRecord {
            id: SessionId::new("session-under-test"),
            project_id: "project-1".to_owned(),
            harness: harness.to_owned(),
            native_session_id: native_session_id.map(str::to_owned),
            role: SessionRole::Normal,
            lifecycle,
            presentation: SessionPresentation::Embedded,
            created_at: 0,
            last_activity_at: 0,
            launch_profile: None,
            backend_resource: None,
            model: None,
            pairing_class: None,
            protocol: None,
            response_profile: None,
            response_mechanism: None,
            display_name: None,
            purpose: None,
            source_session_id: None,
        }
    }

    #[test]
    fn a_failed_task_resumes_in_the_same_native_session_when_there_is_one() {
        let record = record_with(SessionLifecycle::Failed, "claude-code", Some("native-1"));
        let outcome = plan(&record, &task(TaskKind::Destructive), &Target::SameSession);
        assert_eq!(
            outcome,
            Recovery::ResumeInPlace {
                native_session_id: "native-1".to_owned(),
                harness: "claude-code".to_owned(),
            }
        );
    }

    #[test]
    fn the_same_session_target_refuses_rather_than_silently_starting_a_fresh_one() {
        let record = record_with(SessionLifecycle::Failed, "claude-code", None);
        let outcome = plan(&record, &task(TaskKind::Ordinary), &Target::SameSession);
        assert!(matches!(
            outcome,
            Recovery::Refuse {
                reason: RefusalReason::NoNativeSessionToResume { .. }
            }
        ));
        assert!(!matches!(outcome, Recovery::FreshSession { .. }));
    }

    #[test]
    fn a_destructive_task_is_never_retried_on_another_harness_without_a_checkpoint() {
        let record = record_with(SessionLifecycle::Failed, "claude-code", Some("native-1"));
        let outcome = plan(
            &record,
            &task(TaskKind::Destructive),
            &Target::FreshSession {
                harness: "codex".to_owned(),
            },
        );
        assert!(matches!(
            outcome,
            Recovery::Refuse {
                reason: RefusalReason::DestructiveRetryWithoutCheckpoint { .. }
            }
        ));
    }

    #[test]
    fn an_unknown_task_kind_is_refused_exactly_as_a_destructive_one_is() {
        let record = record_with(SessionLifecycle::Failed, "claude-code", Some("native-1"));
        let target = Target::FreshSession {
            harness: "codex".to_owned(),
        };

        let destructive = plan(&record, &task(TaskKind::Destructive), &target);
        let unknown = plan(&record, &task(TaskKind::Unknown), &target);

        let destructive_reason = match destructive {
            Recovery::Refuse { reason } => reason,
            other => panic!("expected a refusal, got {other:?}"),
        };
        let unknown_reason = match unknown {
            Recovery::Refuse { reason } => reason,
            other => panic!("expected a refusal, got {other:?}"),
        };

        assert!(destructive_reason.is_unaccounted_effect());
        assert!(unknown_reason.is_unaccounted_effect());
    }

    #[test]
    fn an_ordinary_task_may_be_handed_off_to_another_harness() {
        let record = record_with(SessionLifecycle::Failed, "claude-code", Some("native-1"));
        let outcome = plan(
            &record,
            &task(TaskKind::Ordinary),
            &Target::FreshSession {
                harness: "codex".to_owned(),
            },
        );
        assert_eq!(
            outcome,
            Recovery::FreshSession {
                harness: "codex".to_owned(),
            }
        );
    }

    #[test]
    fn a_destructive_task_may_be_handed_off_to_the_same_harness_it_ran_in() {
        let record = record_with(SessionLifecycle::Failed, "claude-code", Some("native-1"));
        let outcome = plan(
            &record,
            &task(TaskKind::Destructive),
            &Target::FreshSession {
                harness: "claude-code".to_owned(),
            },
        );
        assert_eq!(
            outcome,
            Recovery::FreshSession {
                harness: "claude-code".to_owned(),
            }
        );
    }

    #[test]
    fn an_event_history_is_not_accepted_as_task_state() {
        let record = record_with(SessionLifecycle::Failed, "claude-code", Some("native-1"));
        let rich_history = TaskState {
            kind: TaskKind::Destructive,
            event_history: vec![
                "SessionStarted".to_owned(),
                "TurnStarted".to_owned(),
                "TurnEnded { outcome: Failed }".to_owned(),
            ],
            checkpoint: None,
        };
        let outcome = plan(
            &record,
            &rich_history,
            &Target::FreshSession {
                harness: "codex".to_owned(),
            },
        );
        assert!(matches!(
            outcome,
            Recovery::Refuse {
                reason: RefusalReason::DestructiveRetryWithoutCheckpoint { .. }
            }
        ));
    }

    #[test]
    fn a_running_session_is_never_recovered() {
        let record = record_with(SessionLifecycle::Running, "claude-code", Some("native-1"));

        let same_session = plan(&record, &task(TaskKind::Ordinary), &Target::SameSession);
        assert!(matches!(
            same_session,
            Recovery::Refuse {
                reason: RefusalReason::SessionStillRunning { .. }
            }
        ));

        let fresh = plan(
            &record,
            &task(TaskKind::Ordinary),
            &Target::FreshSession {
                harness: "codex".to_owned(),
            },
        );
        assert!(matches!(
            fresh,
            Recovery::Refuse {
                reason: RefusalReason::SessionStillRunning { .. }
            }
        ));
    }

    #[test]
    fn every_refusal_says_which_session_and_why() {
        let id = "session-under-test";

        let still_running = RefusalReason::SessionStillRunning {
            id: SessionId::new(id),
        };
        assert!(still_running.to_string().contains(id));
        assert!(still_running.to_string().contains("still running"));

        let destructive = RefusalReason::DestructiveRetryWithoutCheckpoint {
            id: SessionId::new(id),
            from_harness: "claude-code".to_owned(),
            to_harness: "codex".to_owned(),
        };
        assert!(destructive.to_string().contains(id));
        assert!(destructive.to_string().contains("claude-code"));
        assert!(destructive.to_string().contains("codex"));
        assert!(destructive.to_string().contains("checkpoint"));

        let unknown = RefusalReason::UnknownTaskRetryWithoutCheckpoint {
            id: SessionId::new(id),
            from_harness: "claude-code".to_owned(),
            to_harness: "codex".to_owned(),
        };
        assert!(unknown.to_string().contains(id));
        assert!(unknown.to_string().contains("claude-code"));
        assert!(unknown.to_string().contains("codex"));
        assert!(unknown.to_string().contains("never recorded"));

        let no_native = RefusalReason::NoNativeSessionToResume {
            id: SessionId::new(id),
        };
        assert!(no_native.to_string().contains(id));
        assert!(no_native.to_string().contains("no recorded native session"));

        let closed = RefusalReason::SessionClosedByUser {
            id: SessionId::new(id),
        };
        assert!(closed.to_string().contains(id));
        assert!(closed.to_string().contains("closed by the user"));
    }

    /// A record the user closed is not reopened by automation.
    ///
    /// Added by the team lead on review: the packet's five rules did not
    /// cover it, so a closed session carrying a native identifier would have
    /// been resumed in place — automation quietly undoing a deliberate act by
    /// the user, which is the thing Phase 44 forbids by name. The refusal is
    /// checked for *both* targets, because a hand-off to a fresh harness
    /// would reopen the work just as surely as a resume would.
    #[test]
    fn a_session_the_user_closed_is_not_reopened_by_automation() {
        let record = record_with(SessionLifecycle::Closed, "claude-code", Some("native-123"));

        for target in [
            Target::SameSession,
            Target::FreshSession {
                harness: "claude-code".to_owned(),
            },
        ] {
            let plan = plan(&record, &task(TaskKind::Ordinary), &target);
            match plan {
                Recovery::Refuse {
                    reason: RefusalReason::SessionClosedByUser { ref id },
                } => assert_eq!(id, &record.id),
                other => panic!("a closed session must not be reopened: {other:?}"),
            }
        }

        // And the rule is specific to the user's own act, not to the
        // disposition it shares with a stopped session that has nothing to
        // resume to — that case keeps its own, more useful refusal.
        let stopped = record_with(SessionLifecycle::Stopped, "claude-code", None);
        assert!(matches!(
            plan(&stopped, &task(TaskKind::Ordinary), &Target::SameSession),
            Recovery::Refuse {
                reason: RefusalReason::NoNativeSessionToResume { .. }
            }
        ));
    }
}
