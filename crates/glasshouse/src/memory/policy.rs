//! What the memory table refuses to hold.
//!
//! Phase 20 states four properties of durable project memory as prohibitions:
//! no raw conversation filler, no temporary step-by-step plans unless they
//! became an accepted constraint or decision, no obvious source-code facts,
//! and a preference for information whose rediscovery would be expensive.
//!
//! # Only two of the four are enforced here, deliberately
//!
//! The first two are *mechanically decidable* from the text itself, so they
//! are enforced at the one place every memory must pass through:
//! [`crate::memory::MemoryStore::record`] refuses them, and the refusal is a
//! typed error rather than a silent drop.
//!
//! The other two are not decidable here and are not faked. Whether a statement
//! is an "obvious source-code fact", or whether rediscovering it "would
//! require significant exploration", is a judgment about the project that only
//! the producer of the memory can make — Phase 21's extractor, or a person. A
//! keyword heuristic pretending to make that call would refuse real memories
//! and admit fake ones, and would produce a test that passed for the wrong
//! reason. This module's job is to be a floor that cannot be argued with, not
//! a classifier.
//!
//! So [`MemoryRefusal`] is a **closed, conservative** guard. It refuses text
//! that is *nothing but* an acknowledgement, and text that is *unambiguously*
//! an ordered plan. Anything it is unsure about, it admits — the cost of a
//! wrongly-admitted memory is one bad search result, and the cost of a wrongly
//! refused one is knowledge that is gone.

use super::store::{MemoryKind, NewMemory};

/// Why a memory was refused a place in the project's durable memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MemoryRefusal {
    /// The body was empty or contained nothing but whitespace. There is no
    /// durable fact in it to store.
    #[error("a memory needs a body; this one is empty")]
    Empty,
    /// The body was, in its entirety, a conversational acknowledgement.
    ///
    /// Matched against a closed list of complete utterances, never as a
    /// substring: a memory that happens to contain the word "ok" is a memory,
    /// and only a body that reduces to nothing more than "ok" is filler.
    #[error(
        "a memory whose entire body is a conversational acknowledgement is \
         filler, not durable project knowledge"
    )]
    ConversationFiller,
    /// The body was an ordered step-by-step plan and the memory did not claim
    /// to be an accepted decision or constraint.
    ///
    /// Phase 20 allows exactly one escape: a plan that "became an accepted
    /// project constraint or decision" is stored, under that kind. Recording
    /// it as a [`MemoryKind::Todo`] or a [`MemoryKind::Finding`] is the case
    /// this refuses — a plan filed as something else is still a plan.
    #[error(
        "a step-by-step plan is not durable project memory unless it became an \
         accepted decision or constraint; record it as `decision` or \
         `constraint` if it did"
    )]
    TemporaryPlan,
}

/// Every complete utterance that counts as raw conversation filler.
///
/// Compared against the whole normalized body, so this list can be blunt
/// without being dangerous. Sorted only for readability.
const FILLER: &[&str] = &[
    "ack",
    "acknowledged",
    "agreed",
    "ah",
    "alright",
    "all right",
    "certainly",
    "cool",
    "done",
    "exactly",
    "fair enough",
    "fine",
    "got it",
    "great",
    "hm",
    "hmm",
    "i see",
    "indeed",
    "k",
    "lgtm",
    "makes sense",
    "no",
    "no problem",
    "noted",
    "np",
    "of course",
    "oh",
    "ok",
    "okay",
    "perfect",
    "right",
    "sounds good",
    "sure",
    "thank you",
    "thanks",
    "understood",
    "will do",
    "yeah",
    "yep",
    "yes",
];

/// Decide whether a memory may be stored.
///
/// Called by [`crate::memory::MemoryStore::record`] before anything reaches
/// SQLite, so there is no path into the table that skips it.
pub fn admit(new: &NewMemory) -> Result<(), MemoryRefusal> {
    if new.body.trim().is_empty() {
        return Err(MemoryRefusal::Empty);
    }
    if is_filler(&new.body) {
        return Err(MemoryRefusal::ConversationFiller);
    }
    if is_step_by_step_plan(&new.body) && !plans_may_be_kept_as(new.kind) {
        return Err(MemoryRefusal::TemporaryPlan);
    }
    Ok(())
}

/// The two kinds Phase 20 names as the plan's way out.
///
/// A full `match` rather than `matches!`, so that adding a memory kind is a
/// compile error here instead of silently joining the permitted side.
fn plans_may_be_kept_as(kind: MemoryKind) -> bool {
    match kind {
        MemoryKind::Decision | MemoryKind::Constraint => true,
        MemoryKind::Feature
        | MemoryKind::Finding
        | MemoryKind::FailedAttempt
        | MemoryKind::Todo => false,
    }
}

/// True when the entire body is one conversational acknowledgement.
///
/// Normalization is trim, lowercase, and strip surrounding punctuation — so
/// `"OK!"`, `" ok. "` and `"Ok"` are the same utterance — and nothing else.
/// A body of two sentences is never filler by this rule even if both of them
/// are, because the second sentence might be the memory.
fn is_filler(body: &str) -> bool {
    let normalized: String = body
        .trim()
        .trim_matches(|c: char| c.is_ascii_punctuation() || c.is_whitespace())
        .to_lowercase();
    FILLER.contains(&normalized.as_str())
}

/// True when the body is an ordered step-by-step plan.
///
/// The bar is deliberately high, because "contains numbered lines" is not the
/// same claim: a finding may well enumerate three observations. What makes a
/// plan a plan is that its steps are *consecutively numbered from one*, so
/// that is what is required — at least three lines opening with `1.`, `2.`,
/// `3.` (or `1)`, or `Step 1`), in that order, with no gaps.
///
/// Anything short of that is admitted. See the module documentation for why
/// erring towards admission is the right direction here.
fn is_step_by_step_plan(body: &str) -> bool {
    const MINIMUM_STEPS: u32 = 3;

    let mut expected: u32 = 1;
    for line in body.lines() {
        let Some(number) = step_number(line) else {
            continue;
        };
        if number == expected {
            expected += 1;
        } else if number == 1 {
            // A second list restarting at one; begin counting again rather
            // than abandoning the scan.
            expected = 2;
        } else {
            // Out of order, so this is an enumeration of something rather
            // than a sequence of steps.
            return false;
        }
    }
    expected > MINIMUM_STEPS
}

/// The step number a line opens with, if it opens with one.
///
/// Recognizes `1.`, `1)`, `- 1.`, `* 1)`, and `Step 1` with any of those
/// leading bullets, each followed by whitespace or the end of the line.
fn step_number(line: &str) -> Option<u32> {
    let rest = line
        .trim_start()
        .trim_start_matches(['-', '*', '#'])
        .trim_start();
    let rest = match rest.get(..5) {
        Some(prefix) if prefix.eq_ignore_ascii_case("step ") => &rest[5..],
        _ => rest,
    };

    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    let after = &rest[digits.len()..];
    let terminated = after.is_empty()
        || after.starts_with(['.', ')', ':'])
        || after.starts_with(char::is_whitespace);
    if !terminated {
        return None;
    }
    digits.parse().ok()
}
