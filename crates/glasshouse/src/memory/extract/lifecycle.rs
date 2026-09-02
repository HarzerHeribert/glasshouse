//! Turning a session's recorded lifecycle events into something extraction
//! can read (Phase 21).
//!
//! # Why the event log, and not the terminal
//!
//! Phase 21 asks the extractor to be fed *"bounded session/event chunks"*.
//! The **event** half is what Glasshouse actually has after a turn ends: a
//! `glasshouse hook` process is a separate short-lived program with no access
//! to the interface's scrollback, and the project database is the only thing
//! both it and the interface can see.
//!
//! It is also the only source that is safe by construction. A hook payload
//! carries the user's prompt and the model's last message; Glasshouse's
//! handler drains that stream **unread**, and `lifecycle_events` has no
//! column a conversation could reach — migration 5 says so and
//! `the_hook_command_never_reads_its_payload` enforces it. So a chunk built
//! here cannot contain conversation text, because there is none to contain.
//!
//! **State the cost plainly, because it is the honest limit of this
//! path.** What an event chunk carries is the *shape* of a session — turns
//! starting and ending, how a turn ended, how much text was delivered and
//! from where, a process exiting, a gateway failing. That is enough for a
//! model to record a finding about how a session behaved and nowhere near
//! enough for it to recover why a decision was made. Until Glasshouse has a
//! richer source that does not read a conversation, automatic extraction is
//! bounded by this, and `glasshouse memory extract --activity` remains the
//! way to feed it something a person chose.
//!
//! # Why the range is computed from what survived the budget
//!
//! [`SessionChunk::build`] keeps the newest entries when the budget binds. A
//! provenance range naming events whose text never reached the model would be
//! a claim this module cannot support, so [`chunk_for_session`] narrows the
//! range to the entries that actually got in. See its implementation note.

use crate::events::log::LoggedEvent;
use crate::events::{LifecycleEvent, TurnOutcome};
use crate::memory::SourceEvents;
use crate::session::SessionId;

use super::chunk::{ChunkLimits, SessionChunk};

/// How many of a session's most recent events are read for one extraction.
///
/// Larger than [`ChunkLimits::max_entries`] on purpose: the chunk's own
/// budget is the bound that matters, and reading a few more rows than it can
/// hold costs one bounded SQL query while letting the whole-chunk character
/// cap do its job over real candidates rather than over exactly as many as
/// it can take.
pub const EVENT_WINDOW: usize = 200;

/// One logged event as a line of session activity.
///
/// Deliberately plain English rather than the stored `kind`: this text is
/// read by a model, and `text_delivered` teaches it less than *"640 bytes of
/// text arrived from the user"*. Nothing here is a harness's vocabulary —
/// [`crate::session::lifecycle`] is the only place that knows one — and
/// nothing here is free text from outside the program.
///
/// Never empty, which [`chunk_for_session`] depends on: an entry that
/// scrubbed to nothing would be dropped by the chunk builder and the
/// surviving-range arithmetic would then name the wrong events.
pub fn describe(event: &LoggedEvent) -> String {
    let what = match &event.event {
        LifecycleEvent::SessionStarted => "the session's process started".to_owned(),
        LifecycleEvent::SessionResumed => {
            "the session was resumed, continuing its own conversation".to_owned()
        }
        LifecycleEvent::TurnStarted => "the harness started working".to_owned(),
        LifecycleEvent::TurnEnded { outcome } => match outcome {
            TurnOutcome::Completed => "a turn ended, completed".to_owned(),
            TurnOutcome::Failed => "a turn ended, failed".to_owned(),
        },
        LifecycleEvent::WaitingForUser => "the harness is waiting for the user".to_owned(),
        LifecycleEvent::TextDelivered { origin, bytes } => {
            format!("{bytes} bytes of text arrived from the {origin}")
        }
        LifecycleEvent::InterruptDelivered { origin } => {
            format!("an interrupt arrived from the {origin}")
        }
        LifecycleEvent::ProcessExited { exit } => format!("the process exited: {exit}"),
        LifecycleEvent::OutputEnded => "the terminal had no more output to give".to_owned(),
        LifecycleEvent::GatewayUnhealthy { resource, reason } => {
            format!("the backend resource `{resource}` stopped serving: {reason}")
        }
        LifecycleEvent::GatewayBackendChanged {
            provider,
            model,
            cause,
        } => format!("the gateway backend changed to {provider}/{model} ({cause})"),
        // Migration 26. The path is repo-relative by the writer's own
        // contract, and it is rendered **verbatim** because that is what
        // makes the reliability guard in `super::Extractor::run` mechanical:
        // a path the model returns is kept only when it is byte-equal to one
        // of these, so any prettifying here would silently make every
        // returned path unmatchable.
        LifecycleEvent::FileTouched { path } => format!("edited {path}"),
    };

    match &event.observed {
        Some(observed) => format!(
            "[{}] {what} (reported by {} as {})",
            event.seq, observed.harness, observed.event
        ),
        None => format!("[{}] {what}", event.seq),
    }
}

/// A bounded, scrubbed chunk of one session's recorded events, carrying the
/// range of the log it actually covers.
///
/// `events` is oldest first, as [`crate::events::EventLog::recent_for_session`]
/// returns it. A session with no recorded events produces an empty chunk
/// rather than `None`, so that a caller still gets an
/// [`super::ExtractionOutcome`] saying `no session activity to extract from`
/// instead of having to invent one.
pub fn chunk_for_session(
    session: &SessionId,
    events: &[LoggedEvent],
    commit: Option<&str>,
    limits: ChunkLimits,
) -> SessionChunk {
    let chunk = SessionChunk::build(
        session.as_str(),
        commit,
        events.iter().map(describe),
        limits,
    );

    // Which events survived the budget. `SessionChunk::build` walks
    // backwards and keeps the newest, and `describe` never returns an empty
    // string — so the entries that got in are exactly the last `kept` of the
    // ones supplied. Narrowing the range this way is the difference between
    // provenance and a guess: a chunk that dropped the first forty events
    // must not claim a memory came from them.
    let kept = chunk.entries().len();
    let window = &events[events.len().saturating_sub(kept)..];
    let range = match (window.first(), window.last()) {
        (Some(first), Some(last)) => SourceEvents::new(first.seq, last.seq),
        _ => None,
    };

    // Map line 1139's reliability guard, one half of it. The set is derived
    // from the **events** in the surviving window, never re-parsed out of the
    // rendered text: `describe` writes `edited <path>` and a reader that
    // recovered the path by stripping that prefix would be one prose change
    // away from silently admitting every path the model returns. The window
    // is the same one the range above is computed from, so a path whose entry
    // the budget dropped is not in the set — the model never saw it, and a
    // memory cannot reference what was not shown.
    let touched: Vec<String> = window
        .iter()
        .filter_map(|event| match &event.event {
            LifecycleEvent::FileTouched { path } => Some(path.clone()),
            _ => None,
        })
        .collect();

    chunk.with_source_events(range).with_touched_paths(touched)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::MessageOrigin;
    use crate::events::log::Observation;

    fn logged(seq: i64, event: LifecycleEvent, observed: Option<Observation>) -> LoggedEvent {
        LoggedEvent {
            seq,
            session: SessionId::new("s-1"),
            at: 1_700_000_000 + seq,
            event,
            observed,
        }
    }

    fn turn_ended(seq: i64) -> LoggedEvent {
        logged(
            seq,
            LifecycleEvent::TurnEnded {
                outcome: TurnOutcome::Completed,
            },
            Some(Observation::new("claude-code", "Stop")),
        )
    }

    /// The surviving-range arithmetic depends on it, and a blank entry is
    /// silently dropped by the chunk builder — so this is load-bearing
    /// rather than cosmetic.
    ///
    /// **The first version of this test was near-vacuous and a mutation
    /// found it.** It asserted the whole line was non-blank, which the
    /// unconditional `[seq]` prefix guarantees no matter what the match arm
    /// returns: an arm replaced by `String::new()` produced `"[8] "`, which
    /// trims to `"[8]"` and passed. What can actually vary is the text
    /// *after* the prefix, and whether two variants describe themselves the
    /// same way — so that is what this asserts now.
    #[test]
    fn no_event_describes_itself_as_nothing() {
        let events = [
            LifecycleEvent::SessionStarted,
            LifecycleEvent::SessionResumed,
            LifecycleEvent::TurnStarted,
            LifecycleEvent::TurnEnded {
                outcome: TurnOutcome::Failed,
            },
            LifecycleEvent::WaitingForUser,
            LifecycleEvent::TextDelivered {
                origin: MessageOrigin::UserKeystroke,
                bytes: 12,
            },
            LifecycleEvent::InterruptDelivered {
                origin: MessageOrigin::Machine,
            },
            LifecycleEvent::OutputEnded,
            LifecycleEvent::GatewayUnhealthy {
                resource: "openrouter".to_owned(),
                reason: crate::events::GatewayFailure::TimedOut,
            },
        ];
        let mut described: Vec<String> = Vec::new();
        for (index, event) in events.into_iter().enumerate() {
            let prefix = format!("[{}] ", index + 1);
            let line = describe(&logged(index as i64 + 1, event.clone(), None));
            assert!(
                line.starts_with(&prefix),
                "every entry names its own position: {line}"
            );
            let what = line[prefix.len()..].trim().to_owned();
            assert!(
                !what.is_empty(),
                "`{}` described itself as nothing",
                event.kind()
            );
            assert!(
                !described.contains(&what),
                "`{}` describes itself exactly like an earlier event: {what}",
                event.kind()
            );
            described.push(what);
        }
    }

    #[test]
    fn a_chunk_of_events_names_the_range_it_covers() {
        let events: Vec<LoggedEvent> = (1..=5).map(turn_ended).collect();
        let chunk = chunk_for_session(
            &SessionId::new("s-1"),
            &events,
            Some("a938fcc"),
            ChunkLimits::default(),
        );

        assert_eq!(chunk.entries().len(), 5);
        assert_eq!(chunk.source_events(), SourceEvents::new(1, 5));
        assert_eq!(chunk.commit(), Some("a938fcc"));
    }

    /// The property the module documentation calls the difference between
    /// provenance and a guess.
    #[test]
    fn a_budget_that_drops_the_oldest_events_narrows_the_range_it_claims() {
        let events: Vec<LoggedEvent> = (1..=20).map(turn_ended).collect();
        let limits = ChunkLimits {
            max_entries: 4,
            max_entry_chars: 2_000,
            max_total_chars: 24_000,
        };
        let chunk = chunk_for_session(&SessionId::new("s-1"), &events, None, limits);

        assert_eq!(chunk.entries().len(), 4);
        assert_eq!(
            chunk.source_events(),
            SourceEvents::new(17, 20),
            "the range must name the events that actually reached the model"
        );
        assert!(chunk.dropped() > 0);
    }

    #[test]
    fn a_session_with_no_recorded_events_produces_an_empty_chunk_and_no_range() {
        let chunk = chunk_for_session(&SessionId::new("s-1"), &[], None, ChunkLimits::default());
        assert!(chunk.is_empty());
        assert_eq!(chunk.source_events(), None);
    }

    /// The harness's own two words travel with the line, because an event
    /// Glasshouse translated and one it observed itself are different
    /// evidence — and neither is a conversation.
    #[test]
    fn a_translated_event_says_which_harness_reported_it() {
        let line = describe(&turn_ended(7));
        assert!(line.contains("a turn ended, completed"));
        assert!(line.contains("reported by claude-code as Stop"));
    }
}
