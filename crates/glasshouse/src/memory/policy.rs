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

use super::store::{MemoryAuthority, MemoryKind, NewMemory};

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

/// The floor a decayed memory's retrieval weight never falls below.
///
/// Decay demotes a memory; it must never make one effectively unfindable —
/// only [`super::search::SearchScope::Historical`] is allowed to make a
/// memory hard to reach, and that is a status boundary, not a decay curve.
/// Chosen low enough that a fresh, high-authority memory with a weak match
/// can still outrank a maximally decayed, low-authority memory with a much
/// stronger one — line 904's requirement, and the ordering
/// `tests/memory_decay.rs` drives directly — while staying above zero, so a
/// decayed memory is demoted rather than erased from ranking.
const RETRIEVAL_WEIGHT_FLOOR: f64 = 0.15;

/// How many days it takes a memory of this authority to decay halfway from
/// full weight to [`RETRIEVAL_WEIGHT_FLOOR`] — Phase 21D line 898's *"age
/// never overrides authority"*, made into policy instead of a magic number
/// inside the ranker.
///
/// A full `match` on every class, never a lookup table with a default: a
/// class added to [`MemoryAuthority`] must be given an explicit rate here
/// rather than silently inheriting one meant for something else.
///
/// - [`MemoryAuthority::Invariant`] has no half-life at all — see
///   [`retrieval_weight`], which returns full weight for it before this is
///   ever consulted. Line 898: *"do not make age alone invalidate a genuine
///   invariant."*
/// - [`MemoryAuthority::Constraint`] decays slowly: it is still a currently
///   binding limit, and a limit does not stop applying merely because time
///   passed.
/// - [`MemoryAuthority::Decision`] and unclassified memories decay at the
///   map's own "ordinary decision" rate (line 899).
/// - [`MemoryAuthority::Preference`], [`MemoryAuthority::Hypothesis`] and
///   [`MemoryAuthority::Idea`] decay fastest (line 900) — they were never
///   binding, so staleness costs nothing to make visible quickly.
/// - [`MemoryAuthority::Historical`] decays at the ordinary rate: it already
///   explains rather than directs, so there is no faster-decaying class
///   below it that the map names, and treating it as ordinary is the
///   conservative middle rather than an invented rule.
fn half_life_days(authority: Option<MemoryAuthority>) -> f64 {
    match authority {
        Some(MemoryAuthority::Invariant) => f64::INFINITY,
        Some(MemoryAuthority::Constraint) => 365.0,
        Some(MemoryAuthority::Decision) | Some(MemoryAuthority::Historical) | None => 120.0,
        Some(MemoryAuthority::Preference)
        | Some(MemoryAuthority::Hypothesis)
        | Some(MemoryAuthority::Idea) => 30.0,
    }
}

/// The retrieval-weight multiplier a memory of this authority, age and
/// validation history should carry — Phase 21D.
///
/// `1.0` means no decay at all. Applied by [`super::store::MemoryStore::search`]
/// to the raw BM25 score of every current result, so an old, low-authority
/// memory that matches the query text well still ranks below a fresh,
/// high-authority memory that matches it poorly — see that method's own
/// documentation for why the multiplier is applied there and not baked into
/// the SQL.
///
/// # Why the reference point is `last_validated_at.unwrap_or(created_at)`
///
/// Line 901: *"allow recently reaffirmed memories to regain retrieval weight
/// without changing their original creation timestamp."* Reaffirming
/// ([`super::store::MemoryStore::reaffirm`]) writes only
/// [`super::store::MemoryRecord::last_validated_at`], so decay has to measure
/// age from whichever of the two is more recent information about when this
/// memory was last known to be true — and a memory that has never been
/// reaffirmed has no more recent information than its creation. This is also
/// line 899's *"when they have not been reaffirmed"*: a memory that has been
/// keeps its full weight for a fresh interval measured from the reaffirming,
/// not from when it was first written down.
///
/// # Why age never invalidates an invariant
///
/// Line 898. Checked before anything else, and unconditionally: no age, no
/// validation history, and no half-life computation can move an invariant's
/// weight away from `1.0`.
pub fn retrieval_weight(
    authority: Option<MemoryAuthority>,
    now: i64,
    created_at: i64,
    last_validated_at: Option<i64>,
) -> f64 {
    if authority == Some(MemoryAuthority::Invariant) {
        return 1.0;
    }

    let reference = last_validated_at.unwrap_or(created_at);
    let age_seconds = now.saturating_sub(reference).max(0);
    let age_days = age_seconds as f64 / 86_400.0;
    let half_life = half_life_days(authority);

    let decayed = (-age_days / half_life).exp();
    RETRIEVAL_WEIGHT_FLOOR + (1.0 - RETRIEVAL_WEIGHT_FLOOR) * decayed
}

#[cfg(test)]
mod decay_tests {
    use super::*;

    /// Line 898: age alone must never move an invariant's weight, at any age.
    #[test]
    fn an_invariant_never_decays() {
        let now = 1_000_000_000;
        let ancient = now - 10 * 365 * 86_400;
        assert_eq!(
            retrieval_weight(Some(MemoryAuthority::Invariant), now, ancient, None),
            1.0
        );
    }

    /// A brand new memory of any authority has not had time to decay.
    #[test]
    fn a_freshly_created_memory_carries_full_weight_regardless_of_authority() {
        let now = 1_000_000_000;
        for authority in MemoryAuthority::ALL.iter().copied().map(Some).chain([None]) {
            let weight = retrieval_weight(authority, now, now, None);
            assert!(
                (weight - 1.0).abs() < 1e-9,
                "{authority:?} at age zero was {weight}, not 1.0"
            );
        }
    }

    /// Line 900: an idea decays faster than an ordinary decision at the same
    /// age, so it demotes below the floor sooner.
    #[test]
    fn an_idea_decays_faster_than_an_ordinary_decision() {
        let now = 1_000_000_000;
        let sixty_days_old = now - 60 * 86_400;
        let idea = retrieval_weight(Some(MemoryAuthority::Idea), now, sixty_days_old, None);
        let decision = retrieval_weight(Some(MemoryAuthority::Decision), now, sixty_days_old, None);
        assert!(
            idea < decision,
            "an idea (weight {idea}) must decay faster than a decision (weight {decision})"
        );
    }

    /// A constraint, still a currently binding limit, decays more slowly than
    /// an ordinary decision at the same age.
    #[test]
    fn a_constraint_decays_more_slowly_than_a_decision() {
        let now = 1_000_000_000;
        let a_year_old = now - 365 * 86_400;
        let constraint = retrieval_weight(Some(MemoryAuthority::Constraint), now, a_year_old, None);
        let decision = retrieval_weight(Some(MemoryAuthority::Decision), now, a_year_old, None);
        assert!(
            constraint > decision,
            "a constraint (weight {constraint}) must decay more slowly than a decision \
             (weight {decision})"
        );
    }

    /// Line 901: reaffirming resets the effective age without moving
    /// `created_at`, so a memory reaffirmed yesterday outranks the decay it
    /// would otherwise have accumulated since it was first written.
    #[test]
    fn a_recently_reaffirmed_memory_regains_full_weight() {
        let now = 1_000_000_000;
        let old_creation = now - 400 * 86_400;
        let never_reaffirmed =
            retrieval_weight(Some(MemoryAuthority::Decision), now, old_creation, None);
        let reaffirmed_yesterday = retrieval_weight(
            Some(MemoryAuthority::Decision),
            now,
            old_creation,
            Some(now - 86_400),
        );
        assert!(
            reaffirmed_yesterday > never_reaffirmed,
            "a memory reaffirmed yesterday (weight {reaffirmed_yesterday}) must outrank the \
             same memory never reaffirmed (weight {never_reaffirmed})"
        );
        assert!(
            (reaffirmed_yesterday - 1.0).abs() < 0.05,
            "a memory reaffirmed one day ago should be close to full weight, got \
             {reaffirmed_yesterday}"
        );
    }

    /// Weight is bounded below by the floor, however old the memory is —
    /// decay demotes, it never makes a memory disappear from ranking.
    #[test]
    fn weight_never_falls_below_the_floor() {
        let now = 1_000_000_000;
        let ancient = 0;
        for authority in [
            MemoryAuthority::Idea,
            MemoryAuthority::Hypothesis,
            MemoryAuthority::Preference,
        ] {
            let weight = retrieval_weight(Some(authority), now, ancient, None);
            assert!(
                weight >= RETRIEVAL_WEIGHT_FLOOR,
                "{authority} decayed to {weight}, below the floor"
            );
        }
    }
}
