//! Why a task ends, and it is never a count of the work it did.
//!
//! **The user's ruling of 2026-09-17: "Limits are dumb for abstract tasks."**
//! Two counters used to end a task here — a 120-cell default and three prose
//! turns — and on the morning of that ruling the first of them ended a
//! four-hour session at cell 120 of 120, mid-implementation, with code that
//! did not compile, cancelling that session's own `cargo test` job on the way
//! out. Neither survives.
//!
//! What ends a task now is either a ceiling **this person set themselves**, or
//! evidence that the task has stopped producing anything:
//!
//! - the supervisor's repeated verdict — the judged ender, absent when no
//!   model is configured to judge with;
//! - a run of stall windows — the deterministic ender, which needs no model
//!   and is what an unsupervised session falls back on;
//! - turns that ran no program at all, which is the one thing still counted
//!   anywhere, because a turn without a cell writes no record for either of
//!   the other two to read.
//!
//! The order matters and is tested: a ceiling the person set is reported as
//! theirs before anything infers a reason on their behalf.

use crate::prompt::ExhaustedReason;

/// Everything the loop knows that bears on whether this task should end.
///
/// A plain snapshot rather than a borrow of the loop's state, so the decision
/// is a pure function of observations and can be read — and tested — without
/// a session.
pub(super) struct Ending<'a> {
    /// `[limits] cells`, when this person set one, and whether it is reached.
    pub(super) cap: Option<u64>,
    pub(super) cap_reached: bool,
    /// Consecutive supervisor looks that decided to intervene, and the
    /// criterion the last of them chose.
    pub(super) verdicts: u32,
    pub(super) criterion: Option<&'a str>,
    /// Turns in a row that carried no program at all.
    pub(super) turns_without_a_program: u32,
    /// Whole stall windows since the last progress of any kind.
    pub(super) stalled_windows: u32,
}

/// The reason this task ends now, or `None` to keep going.
pub(super) fn exhausted(
    ending: &Ending<'_>,
    verdict_limit: u32,
    program_limit: u32,
    stall_limit: u32,
    stall_window: u32,
) -> Option<ExhaustedReason> {
    if let Some(cap) = ending.cap.filter(|_| ending.cap_reached) {
        return Some(ExhaustedReason::CellLimit { cap });
    }
    if ending.verdicts >= verdict_limit {
        return Some(ExhaustedReason::Supervised {
            reason: crate::supervisor::criterion_phrase(ending.criterion.unwrap_or_default())
                .to_string(),
            looks: ending.verdicts,
        });
    }
    if ending.turns_without_a_program >= program_limit {
        return Some(ExhaustedReason::NoProgram {
            turns: ending.turns_without_a_program,
        });
    }
    if ending.stalled_windows >= stall_limit {
        return Some(ExhaustedReason::Stalled {
            windows: ending.stalled_windows,
            cells: ending.stalled_windows * stall_window,
        });
    }
    None
}

/// Put the ending's sentence in front of the model, on **every carrier that
/// message uses**.
///
/// A model answering with `execute_cell` as a provider-native call receives
/// its feedback as a `tool_result`, not as the text answer — so a sentence
/// written only into the answer reaches exactly the sessions a tool-calling
/// model does not run. That defect was found in the supervisor's nudge on
/// 2026-09-17 and was still live here, unnoticed, until a test for the ending
/// drove a session down the native path. A task that ends without telling the
/// model why ends the same way the next time.
pub(super) fn announce(
    reason: &ExhaustedReason,
    answer: &mut Option<String>,
    historical: &mut Option<String>,
    native: Option<&mut crate::contract::Message>,
) {
    let preamble = crate::prompt::exhausted_preamble(reason);
    *answer = answer.take().map(|text| format!("{preamble}\n\n{text}"));
    *historical = historical
        .take()
        .map(|text| format!("{preamble}\n\n{text}"));
    if let Some(result) = native {
        for block in &mut result.content {
            if let crate::contract::Block::ToolResult { content, .. } = block {
                *content = format!("{preamble}\n\n{content}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nothing() -> Ending<'static> {
        Ending {
            cap: None,
            cap_reached: false,
            verdicts: 0,
            criterion: None,
            turns_without_a_program: 0,
            stalled_windows: 0,
        }
    }

    fn decide(ending: &Ending<'_>) -> Option<ExhaustedReason> {
        exhausted(ending, 3, 6, 3, 6)
    }

    #[test]
    fn a_task_that_is_getting_somewhere_never_ends_here() {
        assert_eq!(decide(&nothing()), None);
        // Short of every threshold, including a cap that exists and is not
        // reached: none of these is a reason on its own.
        let working = Ending {
            cap: Some(40),
            verdicts: 2,
            turns_without_a_program: 5,
            stalled_windows: 2,
            ..nothing()
        };
        assert_eq!(decide(&working), None);
    }

    #[test]
    fn a_ceiling_this_person_set_is_reported_as_theirs_first() {
        // Every other reason is also true here; the configured one wins,
        // because inferring a reason over the person's own is presumptuous.
        let ended = Ending {
            cap: Some(40),
            cap_reached: true,
            verdicts: 9,
            criterion: Some("repeating_a_failing_call"),
            turns_without_a_program: 9,
            stalled_windows: 9,
        };
        assert_eq!(decide(&ended), Some(ExhaustedReason::CellLimit { cap: 40 }));
    }

    #[test]
    fn a_cap_that_is_not_set_can_never_be_reached() {
        let ended = Ending {
            cap: None,
            cap_reached: true,
            ..nothing()
        };
        assert_eq!(decide(&ended), None);
    }

    #[test]
    fn the_verdict_names_the_criterion_and_the_stall_names_its_cells() {
        let judged = Ending {
            verdicts: 3,
            criterion: Some("looping_over_the_same_reads"),
            ..nothing()
        };
        assert_eq!(
            decide(&judged),
            Some(ExhaustedReason::Supervised {
                reason: "the same files keep being read without a change".to_string(),
                looks: 3,
            })
        );
        // An unknown criterion still says something true rather than nothing.
        let vague = Ending {
            verdicts: 3,
            criterion: None,
            ..nothing()
        };
        assert!(matches!(
            decide(&vague),
            Some(ExhaustedReason::Supervised { .. })
        ));
        let stalled = Ending {
            stalled_windows: 3,
            ..nothing()
        };
        assert_eq!(
            decide(&stalled),
            Some(ExhaustedReason::Stalled {
                windows: 3,
                cells: 18,
            })
        );
    }

    #[test]
    fn turns_without_a_program_end_a_task_the_other_enders_cannot_see() {
        // A prose turn writes no record, so the stall counter and the
        // supervisor's cadence both stay at zero however long it goes on.
        let talking = Ending {
            turns_without_a_program: 6,
            ..nothing()
        };
        assert_eq!(
            decide(&talking),
            Some(ExhaustedReason::NoProgram { turns: 6 })
        );
    }
}
