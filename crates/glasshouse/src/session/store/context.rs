//! What Glasshouse can say about one session's context — Phase 30, split out
//! of `session/store.rs` (`GH-DECOMP-SESSION-STORE`). [`super::SessionStore::context`]
//! is the one place these are computed; this file holds only the value types
//! it returns.

use std::fmt;

use super::record::SessionId;

/// The four prompt-cache states map line 1162 requires — *"at least hot,
/// warm, cold, or unknown"*.
///
/// Never constructed directly outside this module: the only way to obtain one
/// is through [`AdvisoryCacheState`], which is line 1163's requirement made
/// structural rather than written in a comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheState {
    /// A provider-side cached prefix is likely to still exist.
    Hot,
    /// One may exist. No provider in scope guarantees it this far out.
    Warm,
    /// Every published cache lifetime this project knows of has passed.
    Cold,
    /// The question could not be answered from what is recorded.
    Unknown,
}

impl CacheState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hot => "hot",
            Self::Warm => "warm",
            Self::Cold => "cold",
            Self::Unknown => "unknown",
        }
    }
}

impl fmt::Display for CacheState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

/// A prompt-cache state Glasshouse **estimated** — map line 1163's *"treat
/// cache-state estimates as advisory when the provider does not expose
/// authoritative cache telemetry."*
///
/// A wrapper, not a comment on [`CacheState`]: its field is private and its
/// only constructors are [`AdvisoryCacheState::estimate`] and
/// [`AdvisoryCacheState::unknown`], so nothing outside this module can claim
/// an authority it does not have — every cache state in this crate arrives
/// wrapped in the word "advisory".
///
/// Estimated from elapsed time since the session's last recorded activity
/// alone; Glasshouse observes neither a provider cache's presence nor its
/// lifetime. **Deliberately not a function of resumability** — map line
/// 1161: resumability comes from
/// [`super::record::SessionRecord::disposition`], and the two never overlap
/// as inputs.
// History: design-decisions.md, "Trims: session module docs, second packet", session/store/context.rs `AdvisoryCacheState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdvisoryCacheState(CacheState);

/// How long a provider-side cached prefix is likely to survive, in seconds.
///
/// Five minutes is the shortest published default among the providers in
/// scope, and the one `crate::config::pairing`'s own note is about when it
/// says such caches "expire in minutes". Inside it, a cached prefix plausibly
/// still exists.
const HOT_PROMPT_CACHE_SECONDS: i64 = 5 * 60;

/// How long one might survive, in seconds.
///
/// One hour is the longest extended cache lifetime any provider in scope
/// offers, and it is offered as an option rather than a default. Between
/// [`HOT_PROMPT_CACHE_SECONDS`] and this, "warm" is the honest word: not the
/// default lifetime, not past every lifetime.
///
/// **Both numbers are reasoning, not measurement**, exactly like the warm
/// session window they sit beside. The measurement that would change them is
/// a provider that reports a cache hit; none does, which is the reason this
/// whole type is advisory.
const WARM_PROMPT_CACHE_SECONDS: i64 = 60 * 60;

impl AdvisoryCacheState {
    /// Estimate from how long a session has been idle.
    ///
    /// `now` before `last_activity_at` yields [`CacheState::Unknown`] rather
    /// than a clamp to zero. A clock that steps backwards is real — migration
    /// 14's own doc comment is about exactly that case — and reporting a
    /// session as `Hot` because the clock moved would be inventing the one
    /// answer this type is least entitled to give.
    pub fn estimate(now: i64, last_activity_at: i64) -> Self {
        let Some(idle_seconds) = now.checked_sub(last_activity_at) else {
            return Self(CacheState::Unknown);
        };
        if idle_seconds < 0 {
            return Self(CacheState::Unknown);
        }
        Self(if idle_seconds <= HOT_PROMPT_CACHE_SECONDS {
            CacheState::Hot
        } else if idle_seconds <= WARM_PROMPT_CACHE_SECONDS {
            CacheState::Warm
        } else {
            CacheState::Cold
        })
    }

    /// An estimate that declines to guess.
    pub fn unknown() -> Self {
        Self(CacheState::Unknown)
    }

    /// The estimated state, which is all this type has ever held.
    pub fn state(self) -> CacheState {
        self.0
    }
}

impl fmt::Display for AdvisoryCacheState {
    /// Prints the word "estimated" beside the state, so that a value reaching
    /// a user through a listing carries line 1163 with it rather than relying
    /// on the reader knowing the type.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (estimated)", self.0)
    }
}

/// Whether a session has a portable checkpoint that still describes where it
/// is — map line 1164.
///
/// # "Recent" is measured against the session, not against the clock
///
/// A wall-clock window would need a threshold nobody could defend: a
/// checkpoint five minutes old is stale if the session did an hour of work in
/// between, and one from yesterday is current if the session has not moved
/// since. So the comparison is `checkpoints.created_at` against the session's
/// own `last_activity_at`, and the answer is a fact about the data rather
/// than a tuning knob.
///
/// Both columns are whole seconds, so a checkpoint written in the same second
/// as the last recorded activity counts as [`CheckpointRecency::Current`] —
/// the tie goes to the checkpoint, because within one second the checkpoint
/// is at least as new as the activity and reporting it stale would be the
/// answer that costs a user a checkpoint they have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointRecency {
    /// Nothing has been recorded as happening in this session since this
    /// checkpoint was written. Seconds since the Unix epoch.
    Current(i64),
    /// A checkpoint exists and the session has recorded activity after it.
    Stale(i64),
    /// No checkpoint has ever been stored for this session.
    Never,
}

impl CheckpointRecency {
    /// Line 1164's question in one word.
    pub fn is_current(self) -> bool {
        matches!(self, Self::Current(_))
    }

    /// When the newest checkpoint was written, if there is one.
    pub fn stored_at(self) -> Option<i64> {
        match self {
            Self::Current(at) | Self::Stale(at) => Some(at),
            Self::Never => None,
        }
    }

    /// A bare word, with no timestamp. `Never` prints as `"never"`, not a
    /// date and not `"stale"` — the two readings that would make "no
    /// checkpoint exists" indistinguishable from "one exists and is old".
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Current(_) => "current",
            Self::Stale(_) => "stale",
            Self::Never => "never",
        }
    }
}

impl fmt::Display for CheckpointRecency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

/// A lightweight flag for whether a session is still working on the task it
/// started — map line 1165.
///
/// Counts completed task boundaries from this session's own `turn_ended`
/// rows in the project event log — `main`'s hook path treats
/// `TurnEnded { Completed }` as the moment a harness finishes a task, so this
/// is Glasshouse's own record of boundaries it already acted on.
///
/// Says nothing about what the tasks **were**: comparing tasks would mean
/// storing transcript content, which a session record must never hold. Two
/// turns on one feature are indistinguishable here from two on unrelated
/// ones — this only distinguishes a session doing one thing from one
/// carrying many finished tasks.
// History: design-decisions.md, "Trims: session module docs, second packet", session/store/context.rs `TaskContinuity`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskContinuity {
    /// The event log holds nothing at all for this session, so nothing has
    /// been observed about its turns — a harness that reports no events, or a
    /// session that has not run yet. Never confused with `OneTask`: a session
    /// nobody watched is not a session seen doing one thing.
    Unknown,
    /// Work has been observed and no completed task boundary among it.
    /// Everything this session holds belongs to the one piece of work it
    /// started.
    OneTask,
    /// How many completed task boundaries have been observed. At one or more,
    /// the task the session began is finished, and its context spans more
    /// than whatever it is doing now.
    BoundariesCrossed(i64),
}

impl fmt::Display for TaskContinuity {
    /// `Unknown` prints as `"unknown"`, never as `"one task"` — a harness
    /// that has reported nothing is not a session seen doing one thing, and
    /// this rendering must not read as a signal either way. See this type's
    /// own doc comment.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown => f.pad("unknown"),
            Self::OneTask => f.pad("one task"),
            Self::BoundariesCrossed(1) => f.pad("1 task completed"),
            Self::BoundariesCrossed(n) => write!(f, "{n} tasks completed"),
        }
    }
}

/// What Glasshouse can say about one session's context — Phase 30, read
/// together so that a caller cannot assemble half of it.
///
/// # Line 1158 is still absent from this struct, now by a different design
///
/// The refusal this section once recorded ended: `routing_observations` now
/// carries the gateway's own token counts (migration 24). But a copied token
/// count would be a second source of truth — the same reason [`SessionContext`]
/// itself gives migration 15 no field for one — so the estimate is not stored
/// here either. It is read on demand by
/// [`crate::routing::evidence::estimated_context_tokens`] over
/// `routing_observations` and attached to
/// [`crate::routing::session::SessionContextFacts`]. See `design-decisions.md`,
/// *"Context size is read off the gateway's own exchange, never guessed"*.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionContext {
    pub session: SessionId,
    /// Line 1159. `None` is *"nobody was counting"*, never zero — see
    /// [`super::record::SessionRecord::observed_compactions`].
    pub observed_compactions: Option<i64>,
    /// Line 1160, and it is `sessions.last_activity_at` itself rather than a
    /// second column meaning almost the same thing. Seconds since the Unix
    /// epoch.
    ///
    /// The single `UPDATE` that moves a session's lifecycle stamps it, and
    /// `main`'s hook handler is what calls that on every translated harness
    /// event — so `UserPromptSubmit` (a request) and `Stop` (a turn ending)
    /// both move it, which is exactly the pair the line names.
    pub last_activity_at: i64,
    /// Lines 1161, 1162 and 1163.
    pub prompt_cache: AdvisoryCacheState,
    /// Line 1164.
    pub checkpoint: CheckpointRecency,
    /// Line 1165.
    pub task_continuity: TaskContinuity,
}
