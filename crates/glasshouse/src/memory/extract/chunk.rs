//! What the extractor is allowed to be shown: a bounded, scrubbed chunk.
//!
//! # One constructor, two guarantees
//!
//! [`SessionChunk::build`] is the only way to make one, and it does two
//! things no caller can skip:
//!
//! 1. **It bounds.** Phase 21 requires bounded session/event chunks "rather
//!    than entire unbounded session histories". A limit a caller passes is a
//!    limit a caller forgets, so the bound is applied here, three ways at
//!    once — a cap on entries, a cap on each entry, and a cap on the whole —
//!    and the third is the one that matters: without it, a thousand entries
//!    just under the per-entry cap is an unbounded chunk assembled out of
//!    bounded parts.
//!
//! 2. **It scrubs.** Every entry goes through
//!    [`super::credentials::scrub`] on the way in, so there is no
//!    `SessionChunk` anywhere in the program holding un-scrubbed text. That
//!    is what makes "the extractor is never fed credential material" a
//!    property of the type rather than a rule someone has to remember at
//!    every call site — and the prompt can only be built from this type.
//!
//! # Newest first, and why the tail is what survives
//!
//! When there is more activity than the budget allows, the **most recent**
//! entries are kept. A task's conclusion is at its end: what was decided,
//! what failed, what was agreed. The beginning is where the exploring
//! happened, and Phase 21A specifically does not want an idea discussed
//! early to arrive with the authority of a decision made late.
//!
//! # Nothing is dropped silently
//!
//! [`SessionChunk::dropped`], [`SessionChunk::truncated`] and
//! [`SessionChunk::redactions`] report exactly what the budget and the
//! scrubber removed. A chunk that lost half a session and says so is
//! evidence; one that lost half a session quietly is a bug that looks like a
//! result.

use super::credentials;
use crate::memory::SourceEvents;

/// How much session activity one extraction may look at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkLimits {
    /// The most entries a chunk carries.
    pub max_entries: usize,
    /// The most characters any single entry carries. Measured in `char`s so
    /// a multi-byte character is never split.
    pub max_entry_chars: usize,
    /// The most characters the whole chunk carries, across all entries.
    ///
    /// The load-bearing one. `max_entries * max_entry_chars` is a bound too,
    /// but it is the product of two numbers chosen for other reasons and is
    /// far larger than any prompt should be.
    pub max_total_chars: usize,
}

impl Default for ChunkLimits {
    /// Sized for a cheap model's context with room for the contract and the
    /// response, not for the largest context available. Phase 21's whole
    /// point is that extraction is a bounded support job.
    fn default() -> Self {
        Self {
            max_entries: 60,
            max_entry_chars: 2_000,
            max_total_chars: 24_000,
        }
    }
}

/// Bounded, scrubbed session activity, with its provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionChunk {
    session_id: String,
    commit: Option<String>,
    events: Option<SourceEvents>,
    entries: Vec<String>,
    dropped: usize,
    truncated: usize,
    redactions: usize,
}

impl SessionChunk {
    /// Bound and scrub `activity` into a chunk.
    ///
    /// `activity` is in the order it happened, oldest first; the newest
    /// entries are the ones kept when the budget binds. `session_id` and
    /// `commit` become the provenance of every memory extracted from this
    /// chunk.
    ///
    /// Entries that are empty or whitespace after scrubbing are discarded
    /// and counted as dropped: a blank line carries nothing, and spending
    /// budget on it costs a real entry.
    pub fn build(
        session_id: impl Into<String>,
        commit: Option<impl Into<String>>,
        activity: impl IntoIterator<Item = String>,
        limits: ChunkLimits,
    ) -> Self {
        let all: Vec<String> = activity.into_iter().collect();
        let supplied = all.len();

        let mut dropped = 0;
        let mut truncated = 0;
        let mut redactions = 0;
        let mut kept: Vec<String> = Vec::new();
        let mut total = 0usize;

        // Walk backwards so the newest entries claim the budget first, then
        // restore chronological order once the budget has bound.
        //
        // The kept set must be a contiguous *tail* of `activity`, because
        // `lifecycle::chunk_for_session` derives a memory's source-event
        // range from `kept.len()` alone. Both budget refusals below therefore
        // stop the walk rather than skipping one entry and admitting an
        // older, shorter one behind it, which would leave a hole the
        // provenance arithmetic cannot describe.
        for (index, entry) in all.iter().rev().enumerate() {
            if kept.len() >= limits.max_entries {
                dropped += supplied - index;
                break;
            }

            let scrubbed = credentials::scrub(entry);
            redactions += scrubbed.removals();

            let mut text = scrubbed.text().trim().to_owned();
            if text.is_empty() {
                dropped += 1;
                continue;
            }

            if text.chars().count() > limits.max_entry_chars {
                text = text.chars().take(limits.max_entry_chars).collect();
                truncated += 1;
            }

            let length = text.chars().count();
            if total + length > limits.max_total_chars {
                // The whole-chunk cap. Everything older than this is
                // dropped rather than partially admitted: half an entry
                // arriving as if it were whole is worse than a reported
                // absence.
                dropped += supplied - index;
                break;
            }

            total += length;
            kept.push(text);
        }

        kept.reverse();

        Self {
            session_id: session_id.into(),
            commit: commit.map(Into::into).filter(|c| !c.trim().is_empty()),
            events: None,
            entries: kept,
            dropped,
            truncated,
            redactions,
        }
    }

    /// The session this activity came from. Becomes
    /// [`crate::memory::MemoryRecord::source_session_id`].
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// The commit the project was at. Becomes
    /// [`crate::memory::MemoryRecord::source_commit`].
    pub fn commit(&self) -> Option<&str> {
        self.commit.as_deref()
    }

    /// Record which slice of the project event log this activity came from.
    ///
    /// Separate from [`SessionChunk::build`] rather than an argument to it,
    /// because it is genuinely optional and absent must stay distinguishable
    /// from empty: activity read out of a file — which is what
    /// `glasshouse memory extract --activity` supplies — has **no** event
    /// range, and that is a different fact from a range covering no events.
    /// [`crate::memory::extract::lifecycle`] is where a chunk that does have
    /// one is built.
    #[must_use]
    pub fn with_source_events(mut self, events: Option<SourceEvents>) -> Self {
        self.events = events;
        self
    }

    /// The event-log slice this activity came from, when it came from one.
    /// Becomes [`crate::memory::MemoryRecord::source_events`].
    pub fn source_events(&self) -> Option<SourceEvents> {
        self.events
    }

    /// The scrubbed entries, oldest first.
    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    /// Whether there is anything here worth asking a model about.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// How many entries the budget refused.
    pub fn dropped(&self) -> usize {
        self.dropped
    }

    /// How many entries were cut to [`ChunkLimits::max_entry_chars`].
    pub fn truncated(&self) -> usize {
        self.truncated
    }

    /// How many credentials the scrubber removed on the way in.
    pub fn redactions(&self) -> usize {
        self.redactions
    }

    /// Total characters of activity in this chunk.
    ///
    /// Never more than [`ChunkLimits::max_total_chars`], which is the
    /// property `a_chunk_is_bounded_however_much_activity_it_is_given`
    /// asserts.
    pub fn chars(&self) -> usize {
        self.entries.iter().map(|e| e.chars().count()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn activity(count: usize, each: usize) -> Vec<String> {
        (0..count)
            .map(|i| format!("{i}:{}", "x".repeat(each)))
            .collect()
    }

    #[test]
    fn a_chunk_is_bounded_however_much_activity_it_is_given() {
        let limits = ChunkLimits {
            max_entries: 10,
            max_entry_chars: 50,
            max_total_chars: 200,
        };
        let chunk = SessionChunk::build("s1", None::<String>, activity(5_000, 500), limits);

        assert!(chunk.entries().len() <= limits.max_entries);
        assert!(
            chunk.chars() <= limits.max_total_chars,
            "chunk carried {} chars against a {} cap",
            chunk.chars(),
            limits.max_total_chars
        );
        assert!(chunk.dropped() > 0, "4990+ entries went somewhere");
    }

    /// The whole-chunk cap refusing a large newer entry must not leave a hole
    /// for a smaller *older* entry to fill — `chunk_for_session` derives a
    /// memory's `source_events` range from `kept.len()` alone, which is only
    /// correct if the kept set is a contiguous tail of `activity`. Finding
    /// break/memory#4.
    ///
    /// Walking newest-to-oldest: `"new"` (3 chars) fits under a 10-char
    /// budget. The 50-char entry behind it does not, and the bug this guards
    /// is `continue`ing past that refusal instead of stopping the walk: two
    /// older, smaller entries then squeeze into the remaining budget, kept
    /// alongside `"new"` while the 50-char entry between them is dropped —
    /// a hole neither `chunk_for_session` nor a reader of `entries()` can see
    /// coming. The fixed walk stops at the first refusal, so whatever
    /// survives is always a suffix of `activity`.
    #[test]
    fn a_refused_entry_does_not_leave_a_hole_for_a_smaller_older_one_to_fill() {
        let limits = ChunkLimits {
            max_entries: 10,
            max_entry_chars: 200,
            max_total_chars: 10,
        };
        let activity = vec![
            "old".to_owned(),
            "mid".to_owned(),
            "x".repeat(50),
            "new".to_owned(),
        ];
        let chunk = SessionChunk::build("s1", None::<String>, activity.clone(), limits);

        assert!(
            activity.ends_with(chunk.entries()),
            "the kept set {:?} is not a contiguous tail of {activity:?}",
            chunk.entries()
        );
    }

    /// The per-entry cap alone does not bound a chunk. Many entries each
    /// just under it add up without limit, which is exactly the unbounded
    /// history Phase 21 forbids.
    #[test]
    fn the_whole_chunk_cap_binds_even_when_every_entry_is_within_its_own_cap() {
        let limits = ChunkLimits {
            max_entries: 1_000,
            max_entry_chars: 100,
            max_total_chars: 250,
        };
        let chunk = SessionChunk::build("s1", None::<String>, activity(100, 90), limits);

        assert!(chunk.entries().len() < 100, "the total cap did not bind");
        assert!(chunk.chars() <= 250);
    }

    #[test]
    fn the_newest_activity_is_what_survives_the_budget() {
        let limits = ChunkLimits {
            max_entries: 2,
            max_entry_chars: 100,
            max_total_chars: 100,
        };
        let chunk = SessionChunk::build(
            "s1",
            None::<String>,
            vec![
                "oldest".to_owned(),
                "middle".to_owned(),
                "newest".to_owned(),
            ],
            limits,
        );

        assert_eq!(chunk.entries(), ["middle", "newest"]);
    }

    #[test]
    fn a_chunk_cannot_be_constructed_holding_a_credential() {
        let planted = "hunter2xyzabcdefghijklmn";
        let chunk = SessionChunk::build(
            "s1",
            Some("a938fcc"),
            vec![
                "we decided the gateway holds the key".to_owned(),
                format!("API_KEY={planted}"),
            ],
            ChunkLimits::default(),
        );

        let joined = chunk.entries().join("\n");
        assert!(!joined.contains(planted), "chunk carried the credential");
        assert_eq!(chunk.redactions(), 1);
        assert!(joined.contains("the gateway holds the key"));
    }

    #[test]
    fn provenance_survives_and_an_empty_commit_is_absent_rather_than_blank() {
        let chunk = SessionChunk::build(
            "session-7",
            Some("   "),
            vec!["something".to_owned()],
            ChunkLimits::default(),
        );
        assert_eq!(chunk.session_id(), "session-7");
        assert_eq!(chunk.commit(), None);
    }

    #[test]
    fn blank_activity_is_dropped_rather_than_spending_budget() {
        let chunk = SessionChunk::build(
            "s1",
            None::<String>,
            vec!["  ".to_owned(), "\n".to_owned(), "real".to_owned()],
            ChunkLimits {
                max_entries: 2,
                max_entry_chars: 100,
                max_total_chars: 100,
            },
        );
        assert_eq!(chunk.entries(), ["real"]);
        assert_eq!(chunk.dropped(), 2);
    }
}
