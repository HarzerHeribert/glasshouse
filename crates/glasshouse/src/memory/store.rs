//! The `memories` table, and the only way to read or write it.
//!
//! # Project isolation
//!
//! Enforced in three independent places, for the reason each is different:
//!
//! - **At the file**, because [`ProjectMemory::open`] goes through
//!   `database::open`, which derives the path from the runtime and
//!   refuses a database bound to another project outright.
//! - **At the row**, by the two SQLite triggers migration 4 creates. A query
//!   can forget to filter by `project_id`; a `BEFORE INSERT` / `BEFORE UPDATE`
//!   guard cannot be forgotten, and it holds against any writer, including one
//!   written later by someone who never read this module.
//! - **At the read boundary**, by [`MemoryStore::get`], which compares the
//!   stored identifier against the active project before handing a record
//!   back.
//!
//! The third is not redundant with the second, for the reason
//! [`crate::session::store`] gives about resume: the trigger governs what this
//! database will *accept* from now on, while the boundary check governs what
//! Glasshouse will *act on* — including a row that predates a guard, arrived
//! through a restored backup, or was written by a build whose triggers
//! differed. Retrieval is the operation that turns a stored row into something
//! an agent will treat as true, so it verifies rather than assumes.
//!
//! # No credentials
//!
//! There is no column here for a token, a key, or a provider secret, and there
//! is no field for one either. The project database is checked into nothing
//! and backed up casually; the operating system's secret storage
//! ([`crate::secret`]) is where a credential lives. `body` is free text an
//! extractor produced, which is exactly why nothing may *route* a credential
//! into it.

use std::fmt;
use std::sync::Arc;

use rusqlite::{Connection, OptionalExtension, Row};

use crate::database::PROJECT_ID_KEY;

use super::policy::{MemoryRefusal, admit};

/// A durable memory's identifier, unique inside one project.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MemoryId(String);

impl MemoryId {
    /// Wrap an identifier that already exists, such as one read back from the
    /// database or supplied on the command line.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MemoryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(&self.0)
    }
}

/// What sort of thing was remembered — Phase 20's six kinds.
///
/// Independent of [`MemoryAuthority`], which says how binding it is, and of
/// [`MemoryStatus`], which says where it sits in its lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MemoryKind {
    /// An implementation or architecture choice that was accepted.
    Decision,
    /// A limit the project currently works within.
    Constraint,
    /// Something the project has or plans to have.
    Feature,
    /// Something established by investigation, which rediscovering would cost.
    Finding,
    /// An approach that was tried and did not work. Kept precisely so it is
    /// not tried again.
    FailedAttempt,
    /// Work that is known to be outstanding.
    Todo,
}

/// How binding a memory is — Phase 21A's seven authority classes.
///
/// Stored from Phase 20 onwards so that Phase 21A adds *classification* rather
/// than a migration.
///
/// **Automatic extraction classifies now** (Phase 21A line 862, closed
/// 2026-08-29): `memory::extract::schema` refuses a proposed memory that
/// declares no authority, and `memory::extract::authority::conservative`
/// decides what it is stored under — never stronger than declared, and never
/// stronger than `EXTRACTOR_CEILING`. This comment previously read *"Nothing
/// classifies yet"*, which stopped being true when that extractor shipped and
/// then sat here as an expired claim of exactly the kind the evidence-ledger
/// sweeps hunt.
///
/// The column and this field stay optional anyway, for a different and still
/// live reason: a memory recorded before classification existed, or written by
/// a path that does not classify, has **no** authority assigned. `None` is
/// that fact and is distinct from every one of the seven classes. Retrieval
/// must treat `None` conservatively — see [`MemoryAuthority::is_binding`],
/// which `None` is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MemoryAuthority {
    /// Must not be violated without explicit review.
    Invariant,
    /// A currently binding technical, security, legal, compatibility or
    /// product limit.
    Constraint,
    /// An accepted choice that may later be revisited.
    Decision,
    /// A desired direction that must not force unnecessary complexity.
    Preference,
    /// A belief that still requires validation.
    Hypothesis,
    /// Exploratory. Must never be injected as a binding instruction.
    Idea,
    /// Useful for understanding the project; must not direct current work.
    Historical,
}

impl MemoryAuthority {
    /// Whether a memory of this authority may be presented to an agent as a
    /// rule rather than as context.
    ///
    /// A full `match`: a new authority class must be classified here instead
    /// of defaulting to either side.
    pub fn is_binding(self) -> bool {
        match self {
            Self::Invariant | Self::Constraint | Self::Decision => true,
            Self::Preference | Self::Hypothesis | Self::Idea | Self::Historical => false,
        }
    }
}

/// Where a memory sits in its lifecycle — Phase 20's six statuses, plus
/// Phase 22's conflict state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MemoryStatus {
    /// Current knowledge. The only status normal retrieval returns.
    Active,
    /// Replaced by a newer memory. Kept, never deleted: the history is what
    /// stops a future agent resurrecting the same idea.
    Superseded,
    /// Considered and turned down. Searchable as historical knowledge so the
    /// rejection is not rediscovered as a fresh proposal.
    Rejected,
    /// A todo that was completed. Queryable, but never presented as open work.
    Resolved,
    /// Something changed that may invalidate this; a person or a stronger
    /// agent has to look.
    NeedsReview,
    /// A known invalidation condition occurred. Historical evidence, never a
    /// current instruction.
    Invalidated,
    /// Phase 22: this memory contradicts another current memory and the
    /// conflict could not be resolved automatically.
    Conflicted,
}

impl MemoryStatus {
    /// Whether a memory with this status is current project knowledge.
    ///
    /// Only [`MemoryStatus::Active`] is. Everything else is history — still
    /// stored, still searchable when history is asked for explicitly, never
    /// returned by a default search.
    pub fn is_current(self) -> bool {
        match self {
            Self::Active => true,
            Self::Superseded
            | Self::Rejected
            | Self::Resolved
            | Self::NeedsReview
            | Self::Invalidated
            | Self::Conflicted => false,
        }
    }

    /// Whether a [`MemoryKind::Todo`] with this status is still open work.
    ///
    /// A resolved todo remains queryable — Phase 22 requires it — and this is
    /// what keeps it from being *presented* as something still to do.
    pub fn is_open_work(self) -> bool {
        match self {
            Self::Active | Self::NeedsReview | Self::Conflicted => true,
            Self::Superseded | Self::Rejected | Self::Resolved | Self::Invalidated => false,
        }
    }
}

/// The stage a project was in when a decision was made — Phase 21B's own
/// list, verbatim.
///
/// Recorded because the memory-validity principle turns on it: *"a decision
/// made during an alpha prototype … should not automatically constrain a
/// production implementation weeks later"*. A decision cannot be judged stale
/// without knowing what it was made under, and nothing else in a memory
/// carries that.
///
/// A fixed set rather than free text, so that Phase 21C can compare the phase
/// a memory was made in against the project's current one without parsing
/// somebody's prose. Migration 6's `CHECK` lists exactly these strings, and
/// `every_project_phase_the_type_supports_is_one_the_schema_accepts` reads
/// that list back out of the migration to keep the two in step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ProjectPhase {
    /// Exploratory code that nothing depends on yet.
    Prototype,
    Alpha,
    Beta,
    /// Serving real users.
    Production,
    /// Moving from one architecture, platform or version to another.
    Migration,
}

/// Why a memory was marked for review — Phase 21C's six conditions, one
/// value per capability-map line, in the map's own order.
///
/// A memory is never marked for review without one of these: the pair
/// `status = NeedsReview` plus a reason is the whole record, so that a person
/// or a stronger agent looking at it later knows *what changed*, not only
/// *that something did*. Migration 10's `CHECK` is the vocabulary's only
/// definition; `every_review_reason_the_type_supports_is_one_the_schema_
/// accepts` reads it back out of the migration so the two cannot drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ReviewReason {
    /// The project's current state no longer matches this memory's recorded
    /// assumptions.
    ProjectState,
    /// The project phase has changed materially since this memory was
    /// created.
    ProjectPhaseChange,
    /// A production incident contradicts the assumptions behind this memory.
    ProductionIncident,
    /// A newer benchmark or scale measurement invalidates the original
    /// performance assumption.
    BenchmarkOrScale,
    /// A newer security requirement conflicts with the original design.
    SecurityRequirement,
    /// Current source architecture no longer resembles the architecture this
    /// memory's decision depended on.
    ArchitectureDrift,
}

macro_rules! sql_enum {
    ($ty:ty { $($variant:ident => $text:literal),+ $(,)? }) => {
        impl $ty {
            /// The value stored in SQLite. Migration 4's `CHECK` constraint
            /// lists exactly these strings, so adding a variant here without a
            /// migration makes writes fail loudly rather than silently storing
            /// something readers cannot interpret.
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text,)+
                }
            }

            /// Parse the value stored in SQLite, which is also the spelling
            /// a user types on the command line.
            ///
            /// Deliberately not named `from_str`: that name belongs to
            /// [`std::str::FromStr`], and a public inherent method wearing it
            /// would be picked up by `.parse()`-shaped expectations it does not
            /// satisfy.
            pub fn from_stored(value: &str) -> Option<Self> {
                match value {
                    $($text => Some(Self::$variant),)+
                    _ => None,
                }
            }

            /// Every variant, in schema order. Used by the CLI's help text and
            /// by round-trip tests; a variant missing here is a variant no
            /// user-facing surface can name.
            pub const ALL: &'static [Self] = &[$(Self::$variant,)+];
        }

        impl fmt::Display for $ty {
            /// `pad`, not `write_str`: a `Display` that writes straight to the
            /// formatter silently ignores width and alignment, so `{:<14}` in
            /// a table would produce ragged columns.
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.pad(self.as_str())
            }
        }
    };
}

sql_enum!(MemoryKind {
    Decision => "decision",
    Constraint => "constraint",
    Feature => "feature",
    Finding => "finding",
    FailedAttempt => "failed_attempt",
    Todo => "todo",
});

sql_enum!(MemoryAuthority {
    Invariant => "invariant",
    Constraint => "constraint",
    Decision => "decision",
    Preference => "preference",
    Hypothesis => "hypothesis",
    Idea => "idea",
    Historical => "historical",
});

sql_enum!(ProjectPhase {
    Prototype => "prototype",
    Alpha => "alpha",
    Beta => "beta",
    Production => "production",
    Migration => "migration",
});

sql_enum!(ReviewReason {
    ProjectState => "project_state",
    ProjectPhaseChange => "project_phase_change",
    ProductionIncident => "production_incident",
    BenchmarkOrScale => "benchmark_or_scale",
    SecurityRequirement => "security_requirement",
    ArchitectureDrift => "architecture_drift",
});

sql_enum!(MemoryStatus {
    Active => "active",
    Superseded => "superseded",
    Rejected => "rejected",
    Resolved => "resolved",
    NeedsReview => "needs_review",
    Invalidated => "invalidated",
    Conflicted => "conflicted",
});

/// The slice of the project event log a memory was extracted from.
///
/// A **range**, not an identifier. Extraction is fed a bounded chunk of a
/// session's recorded events and produces memories from the whole of it, so
/// naming one event would be a precision the producer does not have. Both
/// ends are inclusive positions in `lifecycle_events.seq`, and migration 6's
/// two triggers refuse a row that names one end without the other or names
/// them out of order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceEvents {
    /// The first event position the extraction slice covered, inclusive.
    pub first: i64,
    /// The last, inclusive. Never less than [`SourceEvents::first`].
    pub last: i64,
}

impl SourceEvents {
    /// A range over `first..=last`, or `None` if the two are the wrong way
    /// round.
    ///
    /// Refused here as well as by the trigger so that a caller finds out
    /// before it reaches SQLite, and so the invariant is stated once in Rust
    /// rather than being a property only the database knows.
    pub fn new(first: i64, last: i64) -> Option<Self> {
        (first <= last).then_some(Self { first, last })
    }

    /// How many event positions the slice spans, both ends included.
    ///
    /// Never zero: a range that exists covers at least one position, which
    /// is why this is `span` and not `len` — there is no empty case for an
    /// `is_empty` to report, and `None` is what "no range" looks like.
    pub fn span(self) -> u64 {
        (self.last - self.first).unsigned_abs() + 1
    }
}

impl fmt::Display for SourceEvents {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.first == self.last {
            f.pad(&format!("event {}", self.first))
        } else {
            f.pad(&format!("events {}-{}", self.first, self.last))
        }
    }
}

/// Why a durable decision was made, and what it assumed — Phase 21B.
///
/// # Why these are fields and not one blob of prose
///
/// The memory-validity principle is that *"an old decision is not still
/// correct merely because it was remembered"*. Deciding whether a decision
/// still holds means checking its assumptions against the project as it is
/// now, and that is only mechanisable if the assumptions are separable: a
/// scale assumption is rechecked against a benchmark, a security assumption
/// against a new requirement, a compatibility assumption against a platform
/// bump. Phase 21C is the phase that does the rechecking; this is the shape
/// it needs to find.
///
/// # `None` means "not known", never "none"
///
/// Every field is optional and absent is never the same as empty. A decision
/// that recorded no security assumption is a decision nobody asked that
/// question about; a decision that recorded *"none: this path handles no
/// user data"* has answered it. Collapsing the two would make Phase 21B's
/// *"when they influenced the decision"* unrepresentable, and would make
/// [`DecisionProvenance::is_thin`] — which drives Phase 21B's
/// lower-confidence rule — meaningless.
///
/// # Every field here is free text, and free text can hold a credential
///
/// The same statement `subject` and `body` carry, recorded in migration 6
/// rather than left to be inferred. The control is on the producer side:
/// `super::extract::schema::judge` screens each emitted element **whole**,
/// before reading any field, so a field added to this struct is covered
/// automatically. [`DecisionProvenance::source_excerpt`] is the sharpest of
/// them because it is verbatim session text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DecisionProvenance {
    /// Why the decision was made, in a sentence — Phase 21B's first line,
    /// *"when the rationale materially affects whether the decision remains
    /// valid"*.
    pub rationale: Option<String>,
    /// The stage the project was in at the time.
    pub project_phase: Option<ProjectPhase>,
    /// The task or problem the decision was meant to solve.
    pub problem: Option<String>,
    /// The assumptions that made it reasonable, where none of the four
    /// specific kinds below fits.
    pub assumptions: Option<String>,
    /// Expected user count, request volume, data size, latency target or
    /// deployment topology, where one influenced the decision.
    pub scale_assumptions: Option<String>,
    /// Security assumptions, where they influenced the decision.
    pub security_assumptions: Option<String>,
    /// Compatibility assumptions, where they influenced the decision.
    pub compatibility_assumptions: Option<String>,
    /// Operational assumptions — single-instance versus distributed
    /// deployment, and the like.
    pub operational_assumptions: Option<String>,
    /// Benchmark results, production incidents, tests, commits or external
    /// requirements the decision rests on.
    pub evidence: Option<String>,
    /// Enough of the original wording, or a reference to it, to audit how
    /// the memory was derived — Phase 21B's last line.
    pub source_excerpt: Option<String>,
}

impl DecisionProvenance {
    /// Whether any assumption at all was recorded, of any of the five kinds.
    pub fn has_assumptions(&self) -> bool {
        self.assumptions.is_some()
            || self.scale_assumptions.is_some()
            || self.security_assumptions.is_some()
            || self.compatibility_assumptions.is_some()
            || self.operational_assumptions.is_some()
    }

    /// Whether this is Phase 21B's *"missing rationale and missing
    /// assumptions"* — the condition that makes a decision lower-confidence
    /// than a well-proven one of the same authority class.
    ///
    /// **And** rather than **or**, because that is what the line says: a
    /// decision that recorded why it was made is not thin merely because it
    /// listed no assumptions, and one that listed its assumptions is not
    /// thin merely because the reason was obvious.
    pub fn is_thin(&self) -> bool {
        self.rationale.is_none() && !self.has_assumptions()
    }

    /// Whether anything at all was recorded.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// One stored memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryRecord {
    pub id: MemoryId,
    /// The project this memory belongs to. Always the active project for any
    /// record this module hands out.
    pub project_id: String,
    pub kind: MemoryKind,
    /// `None` means no authority has been assigned — see [`MemoryAuthority`].
    pub authority: Option<MemoryAuthority>,
    pub status: MemoryStatus,
    /// A concise subject, when the producer had one. `None` is not an empty
    /// subject: it records that none was available, which Phase 20 explicitly
    /// allows ("when available").
    pub subject: Option<String>,
    /// The durable body. Never empty; the admission guard refuses that.
    pub body: String,
    /// The session this memory was extracted from, when known.
    ///
    /// A *reference*, not a foreign key, for the same reason
    /// `sessions.native_session_id` is: a memory may be extracted from a
    /// session Glasshouse never recorded, and deleting a session's record must
    /// not delete what was learned in it.
    pub source_session_id: Option<String>,
    /// The Git commit the project was at when this was learned, when known.
    pub source_commit: Option<String>,
    /// The slice of the project event log this was extracted from, when it
    /// was extracted from one. `None` for a memory whose activity came from
    /// somewhere the event log does not reach — a file of session activity
    /// handed to `glasshouse memory extract`, say.
    pub source_events: Option<SourceEvents>,
    /// Why the decision in this memory was made, and what it assumed.
    pub provenance: DecisionProvenance,
    /// The memory that replaced this one, when a direct supersession
    /// relationship is known. `None` on a superseded memory means it was
    /// retired without a single identifiable successor.
    pub superseded_by: Option<MemoryId>,
    /// Map line 925: **why** this memory was superseded, in the words of
    /// whoever superseded it — *"so future agents do not resurrect it without
    /// context."*
    ///
    /// `None` is *"no reason was recorded"*: a memory superseded before
    /// migration 13, and one superseded today without `--reason`, which stays
    /// legal. Never an empty reason — [`MemoryStore::supersede_with_reason`]
    /// maps blank text to `None` and the schema refuses `''` outright.
    ///
    /// Cleared together with [`Self::superseded_by`] whenever a memory leaves
    /// [`MemoryStatus::Superseded`], because a memory that is current again
    /// was not replaced, and a leftover explanation of a supersession that no
    /// longer holds is worse than none.
    pub superseded_reason: Option<String>,
    /// Phase 21C: the condition under which this memory should still be
    /// treated as true, when the producer knew one. `None` means no
    /// condition was recorded, never "always valid."
    pub validity_conditions: Option<String>,
    /// Phase 21C: the condition under which this memory should be treated as
    /// invalidated, when the producer knew one.
    pub invalidation_conditions: Option<String>,
    /// Phase 21C: why this memory is [`MemoryStatus::NeedsReview`], when it
    /// is. `None` on a memory that has never been marked for review, and also
    /// on one whose review has since been resolved back to another status —
    /// see [`MemoryStore::mark_for_review`].
    pub review_reason: Option<ReviewReason>,
    /// Phase 21C: when this memory was marked for review, seconds since the
    /// Unix epoch. `None` means never — not "at epoch zero."
    pub review_marked_at: Option<i64>,
    /// Phase 21D: when this memory was last reaffirmed against current
    /// project state, seconds since the Unix epoch. `None` means *unknown* —
    /// migration 10's own distinction — never "validated at epoch zero," so
    /// the decay policy in `super::policy` falls back to [`Self::created_at`]
    /// rather than treating an unvalidated memory as infinitely stale. See
    /// [`MemoryStore::reaffirm`].
    pub last_validated_at: Option<i64>,
    /// Seconds since the Unix epoch.
    pub created_at: i64,
    /// Seconds since the Unix epoch.
    pub updated_at: i64,
}

impl MemoryRecord {
    /// Whether this memory is current project knowledge that may be retrieved
    /// by default.
    pub fn is_current(&self) -> bool {
        self.status.is_current()
    }

    /// Whether this memory should be presented as work still outstanding.
    ///
    /// Only a todo ever can be: Phase 22's requirement is specifically that a
    /// resolved todo stays queryable without appearing open, and nothing else
    /// is "open work" to begin with.
    pub fn is_open_todo(&self) -> bool {
        self.kind == MemoryKind::Todo && self.status.is_open_work()
    }

    /// Whether this is a decision Phase 21B calls lower-confidence: one with
    /// *"missing rationale and missing assumptions"*.
    ///
    /// Restricted to [`MemoryKind::Decision`] because that is the line's own
    /// subject. A `finding` with no assumptions is not a decision that failed
    /// to record its reasoning; it is a fact somebody established, and
    /// demoting it would be inventing a rule the map does not state.
    ///
    /// This is what [`MemoryStore::binding`] orders by, so that a decision
    /// nobody wrote a reason for never reaches an agent ahead of a
    /// well-proven one — see that method.
    pub fn is_lower_confidence_decision(&self) -> bool {
        self.kind == MemoryKind::Decision && self.provenance.is_thin()
    }
}

/// What a caller supplies to record a memory.
///
/// There is no field for a credential, a token, or a provider key, and no
/// column for one either — see the module documentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMemory {
    pub kind: MemoryKind,
    pub body: String,
    pub subject: Option<String>,
    /// Left `None` until Phase 21A classifies. Never guessed here: an
    /// unclassified memory that defaulted to a class would be a promotion
    /// nobody made.
    pub authority: Option<MemoryAuthority>,
    pub source_session_id: Option<String>,
    pub source_commit: Option<String>,
    /// The event-log slice this was extracted from, when there was one.
    pub source_events: Option<SourceEvents>,
    /// Phase 21B's decision provenance. Defaults to all-absent, which is what
    /// a caller that knows none of it should store.
    pub provenance: DecisionProvenance,
    /// Phase 21C: the condition under which this memory should still be
    /// treated as true, when known.
    pub validity_conditions: Option<String>,
    /// Phase 21C: the condition under which this memory should be treated as
    /// invalidated, when known.
    pub invalidation_conditions: Option<String>,
}

impl NewMemory {
    /// A memory of one kind with one body. Everything else is optional
    /// because Phase 20 stores it "when available", and absent must stay
    /// distinguishable from empty.
    pub fn new(kind: MemoryKind, body: impl Into<String>) -> Self {
        Self {
            kind,
            body: body.into(),
            subject: None,
            authority: None,
            source_session_id: None,
            source_commit: None,
            source_events: None,
            provenance: DecisionProvenance::default(),
            validity_conditions: None,
            invalidation_conditions: None,
        }
    }

    /// Record a concise subject.
    ///
    /// A subject that is empty or whitespace is stored as `None` rather than
    /// as `Some("")`: "no subject was available" and "the subject is the empty
    /// string" are the same fact, and only one of them should be
    /// representable.
    pub fn with_subject(mut self, subject: Option<impl Into<String>>) -> Self {
        self.subject = subject
            .map(Into::into)
            .filter(|value| !value.trim().is_empty());
        self
    }

    /// Record the authority class, once something has classified it.
    pub fn with_authority(mut self, authority: Option<MemoryAuthority>) -> Self {
        self.authority = authority;
        self
    }

    /// Record the session this was extracted from.
    pub fn with_source_session(mut self, session: Option<impl Into<String>>) -> Self {
        self.source_session_id = session
            .map(Into::into)
            .filter(|value| !value.trim().is_empty());
        self
    }

    /// Record the Git commit the project was at.
    pub fn with_source_commit(mut self, commit: Option<impl Into<String>>) -> Self {
        self.source_commit = commit
            .map(Into::into)
            .filter(|value| !value.trim().is_empty());
        self
    }

    /// Record which slice of the project event log this came from.
    pub fn with_source_events(mut self, events: Option<SourceEvents>) -> Self {
        self.source_events = events;
        self
    }

    /// Record why the decision was made and what it assumed.
    ///
    /// Whitespace-only strings are stored as `None` for the same reason
    /// [`NewMemory::with_subject`] does it: "nobody recorded a rationale" and
    /// "the rationale is the empty string" are the same fact, and only one of
    /// them should be representable.
    pub fn with_provenance(mut self, provenance: DecisionProvenance) -> Self {
        fn tidy(value: Option<String>) -> Option<String> {
            value
                .map(|text| text.trim().to_owned())
                .filter(|text| !text.is_empty())
        }
        self.provenance = DecisionProvenance {
            rationale: tidy(provenance.rationale),
            project_phase: provenance.project_phase,
            problem: tidy(provenance.problem),
            assumptions: tidy(provenance.assumptions),
            scale_assumptions: tidy(provenance.scale_assumptions),
            security_assumptions: tidy(provenance.security_assumptions),
            compatibility_assumptions: tidy(provenance.compatibility_assumptions),
            operational_assumptions: tidy(provenance.operational_assumptions),
            evidence: tidy(provenance.evidence),
            source_excerpt: tidy(provenance.source_excerpt),
        };
        self
    }

    /// Record the condition under which this memory should still be treated
    /// as true — Phase 21C's *"allow a durable memory to define explicit
    /// validity conditions when known."*
    ///
    /// Whitespace-only is stored as `None`, for the reason
    /// [`NewMemory::with_subject`] gives.
    pub fn with_validity_conditions(mut self, conditions: Option<impl Into<String>>) -> Self {
        self.validity_conditions = conditions
            .map(Into::into)
            .filter(|value| !value.trim().is_empty());
        self
    }

    /// Record the condition under which this memory should be treated as
    /// invalidated — Phase 21C's *"allow a durable memory to define explicit
    /// invalidation conditions when known."*
    pub fn with_invalidation_conditions(mut self, conditions: Option<impl Into<String>>) -> Self {
        self.invalidation_conditions = conditions
            .map(Into::into)
            .filter(|value| !value.trim().is_empty());
        self
    }
}

/// Failures a caller has to distinguish.
#[derive(Debug, thiserror::Error)]
pub enum MemoryStoreError {
    #[error("no memory `{id}` in this project")]
    NotFound { id: MemoryId },
    #[error(
        "memory `{id}` belongs to project `{actual}`, not to the active \
         project `{expected}`; refusing to read another project's memory"
    )]
    ForeignProject {
        id: MemoryId,
        expected: String,
        actual: String,
    },
    /// The admission guard refused the memory. See [`MemoryRefusal`].
    #[error(transparent)]
    Refused(#[from] MemoryRefusal),
    #[error(
        "a memory cannot supersede itself (`{id}`); supersession records that \
         one memory replaced another"
    )]
    SelfSupersession { id: MemoryId },
    #[error("memory `{id}` stored an unrecognized {column} value `{value}`")]
    UnknownValue {
        id: MemoryId,
        column: &'static str,
        value: String,
    },
    #[error(
        "memory `{id}` carries {impact} authority, so it may not be settled \
         automatically; a person or a stronger agent has to decide"
    )]
    ReviewRequired { id: MemoryId, impact: &'static str },
    #[error("the project database has no project identifier bound")]
    UnboundDatabase,
    #[error("could not {action} in the project database")]
    Sql {
        action: &'static str,
        #[source]
        source: rusqlite::Error,
    },
}

/// Who is resolving a memory conflict.
///
/// Phase 22 requires "human or stronger-agent review before automatically
/// resolving ambiguous high-impact memory conflicts", so the caller has to say
/// which it is. There is no default: an argument that could be omitted would
/// be omitted, and the omission would always fall on the automatic side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictResolver {
    /// An agent acting on its own judgment, mid-task.
    Automatic,
    /// A person, or an agent the user has put in a review role. Permitted to
    /// resolve any conflict.
    Reviewed,
}

/// Who is changing a memory's authority class.
///
/// Phase 21A's last line allows *"users or trusted review agents to promote or
/// demote memory authority explicitly"*, and its neighbour forbids promoting
/// uncertain memories to invariants automatically. Those two lines together
/// mean the operation needs to know who is asking, so there is no default:
/// an argument that could be omitted would be, and the omission would always
/// fall on the automatic side.
///
/// Deliberately a second enum with the same shape as [`ConflictResolver`]
/// rather than a reuse of it, for the reason [`Clock`] gives about its own
/// duplication: the two answer different questions — *may this conflict be
/// settled?* and *may this authority be raised?* — and one enum serving both
/// would mean a future change to one silently changing the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Classifier {
    /// The memory extractor, or any agent acting on its own judgment
    /// mid-task. May lower an authority freely and may raise one only to a
    /// class below [`MemoryAuthority::Invariant`].
    Extractor,
    /// A person, or an agent the user has put in a review role. May set any
    /// class, including [`MemoryAuthority::Invariant`].
    Reviewed,
}

impl Classifier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Extractor => "extractor",
            Self::Reviewed => "reviewed",
        }
    }
}

impl fmt::Display for Classifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

/// What an authority change did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorityChange {
    /// The memory now carries a class it did not carry before.
    Changed,
    /// It already carried exactly this class. Reported rather than treated as
    /// a change so an idempotent re-run is distinguishable from a real
    /// promotion in an audit.
    Unchanged,
}

/// Reads the wall clock, in seconds since the Unix epoch.
///
/// Injected rather than called directly so tests can assert on exact
/// timestamps instead of sleeping or accepting a range. Shared ownership
/// rather than a bare `fn` pointer because a useful test clock has to
/// *advance*, which means capturing state.
///
/// Deliberately a second declaration of the same shape as
/// `crate::session::store::Clock` rather than a reuse of it: the two modules
/// are being built by two concurrent batches, and a shared alias would couple
/// them for no benefit beyond removing one line. Worth unifying once both have
/// landed.
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// Seconds since the Unix epoch.
///
/// Saturates rather than panicking on a clock set before 1970: a nonsensical
/// timestamp on one row is a far smaller problem than refusing to record what
/// the project learned.
fn system_clock() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(elapsed) => i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX),
        Err(_) => 0,
    }
}

/// Every column of `memories`, in the order [`row_to_record`] reads them.
pub(super) const ALL_COLUMNS: &str = "id, project_id, kind, authority, status, subject, body, \
                                      source_session_id, source_commit, source_event_first, \
                                      source_event_last, superseded_by, created_at, updated_at, \
                                      rationale, project_phase, problem, assumptions, \
                                      scale_assumptions, security_assumptions, \
                                      compatibility_assumptions, operational_assumptions, \
                                      evidence, source_excerpt, validity_conditions, \
                                      invalidation_conditions, review_reason, review_marked_at, \
                                      last_validated_at, superseded_reason";

/// An open project database plus the memories inside it.
///
/// Owns its connection, for callers — the CLI, and eventually the TUI — that
/// want one value to hold. [`MemoryStore`] borrows a connection instead, so a
/// single connection can back several kinds of store.
pub struct ProjectMemory {
    conn: Connection,
    project_id: String,
    clock: Clock,
}

impl fmt::Debug for ProjectMemory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProjectMemory")
            .field("project_id", &self.project_id)
            .finish_non_exhaustive()
    }
}

impl ProjectMemory {
    /// Open the active project's database and read its binding.
    ///
    /// The path comes from `runtime` and nowhere else, so there is no argument
    /// a caller could pass to reach another project's file. Every check
    /// `database::open` performs — the symlink refusal, the read-only
    /// refusal, the project-identity check, the migrations — applies here
    /// because this is the same door.
    pub fn open(runtime: &crate::Runtime) -> anyhow::Result<Self> {
        Self::open_with_clock(runtime, Arc::new(system_clock))
    }

    /// [`ProjectMemory::open`] with the clock replaced.
    pub fn open_with_clock(runtime: &crate::Runtime, clock: Clock) -> anyhow::Result<Self> {
        let conn = crate::database::open(runtime)?;
        let project_id = MemoryStore::with_clock(&conn, Arc::clone(&clock))?
            .project_id()
            .to_owned();
        Ok(Self {
            conn,
            project_id,
            clock,
        })
    }

    /// The memories in this project.
    pub fn store(&self) -> MemoryStore<'_> {
        MemoryStore {
            conn: &self.conn,
            project_id: self.project_id.clone(),
            clock: Arc::clone(&self.clock),
        }
    }
}

/// Memory records for one project.
pub struct MemoryStore<'a> {
    conn: &'a Connection,
    project_id: String,
    clock: Clock,
}

impl fmt::Debug for MemoryStore<'_> {
    /// Hand-written because [`Clock`] is a trait object with no `Debug`.
    /// Prints the project identifier — a hash of the canonical root, not a
    /// secret — and nothing about the connection or its contents.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemoryStore")
            .field("project_id", &self.project_id)
            .finish_non_exhaustive()
    }
}

impl<'a> MemoryStore<'a> {
    /// Open the store over a connection produced by
    /// `database::open`.
    ///
    /// The project identifier is read from the database's own binding rather
    /// than accepted as an argument, so the identifier this store writes is by
    /// construction the identifier the triggers compare against, even if a
    /// caller is confused about which project it is in.
    pub fn new(conn: &'a Connection) -> Result<Self, MemoryStoreError> {
        Self::with_clock(conn, Arc::new(system_clock))
    }

    /// [`MemoryStore::new`] with the clock replaced.
    pub fn with_clock(conn: &'a Connection, clock: Clock) -> Result<Self, MemoryStoreError> {
        let project_id: Option<String> = conn
            .query_row(
                "SELECT value FROM project_metadata WHERE key = ?1",
                [PROJECT_ID_KEY],
                |row| row.get(0),
            )
            .optional()
            .map_err(|source| MemoryStoreError::Sql {
                action: "read the project identifier",
                source,
            })?;

        Ok(Self {
            project_id: project_id.ok_or(MemoryStoreError::UnboundDatabase)?,
            conn,
            clock,
        })
    }

    /// The project every record in this store belongs to.
    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    /// The connection, for the search and snapshot surfaces in this module.
    pub(super) fn connection(&self) -> &Connection {
        self.conn
    }

    /// The current time, via this store's injected clock — for the search
    /// and snapshot surfaces in this module, so decay policy is exercised
    /// against the same test-controllable clock as everything else instead
    /// of reading the wall clock directly.
    pub(super) fn now(&self) -> i64 {
        (self.clock)()
    }

    /// Store a memory, if it is one.
    ///
    /// Goes through the admission guard first, so there is no route into the
    /// table that skips Phase 20's prohibitions. A refused memory leaves no
    /// row and no identifier behind.
    ///
    /// The identifier is generated by SQLite's own CSPRNG rather than from the
    /// clock: extraction produces memories in bursts, and anything
    /// time-derived would collide.
    pub fn record(&self, new: NewMemory) -> Result<MemoryRecord, MemoryStoreError> {
        admit(&new)?;

        let now = (self.clock)();
        let record = MemoryRecord {
            id: MemoryId(self.generate_id()?),
            project_id: self.project_id.clone(),
            kind: new.kind,
            authority: new.authority,
            // Everything starts current. A memory is only ever moved out of
            // `Active` by something that knows why.
            status: MemoryStatus::Active,
            subject: new.subject,
            body: new.body,
            source_session_id: new.source_session_id,
            source_commit: new.source_commit,
            source_events: new.source_events,
            provenance: new.provenance,
            superseded_by: None,
            superseded_reason: None,
            validity_conditions: new.validity_conditions,
            invalidation_conditions: new.invalidation_conditions,
            // A newly recorded memory has never been flagged and never been
            // reaffirmed. `None` here is "not yet," not "at epoch zero" —
            // migration 10's own distinction.
            review_reason: None,
            review_marked_at: None,
            last_validated_at: None,
            created_at: now,
            updated_at: now,
        };

        self.conn
            .execute(
                "INSERT INTO memories (id, project_id, kind, authority, status, subject, \
                 body, source_session_id, source_commit, source_event_first, \
                 source_event_last, superseded_by, created_at, updated_at, rationale, \
                 project_phase, problem, assumptions, scale_assumptions, \
                 security_assumptions, compatibility_assumptions, \
                 operational_assumptions, evidence, source_excerpt, validity_conditions, \
                 invalidation_conditions) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, \
                 ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26)",
                rusqlite::params![
                    record.id.as_str(),
                    &record.project_id,
                    record.kind.as_str(),
                    record.authority.map(MemoryAuthority::as_str),
                    record.status.as_str(),
                    &record.subject,
                    &record.body,
                    &record.source_session_id,
                    &record.source_commit,
                    record.source_events.map(|events| events.first),
                    record.source_events.map(|events| events.last),
                    record.superseded_by.as_ref().map(MemoryId::as_str),
                    record.created_at,
                    record.updated_at,
                    &record.provenance.rationale,
                    record.provenance.project_phase.map(ProjectPhase::as_str),
                    &record.provenance.problem,
                    &record.provenance.assumptions,
                    &record.provenance.scale_assumptions,
                    &record.provenance.security_assumptions,
                    &record.provenance.compatibility_assumptions,
                    &record.provenance.operational_assumptions,
                    &record.provenance.evidence,
                    &record.provenance.source_excerpt,
                    &record.validity_conditions,
                    &record.invalidation_conditions,
                ],
            )
            .map_err(|source| MemoryStoreError::Sql {
                action: "record a memory",
                source,
            })?;

        Ok(record)
    }

    fn generate_id(&self) -> Result<String, MemoryStoreError> {
        self.conn
            .query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))
            .map_err(|source| MemoryStoreError::Sql {
                action: "generate a memory identifier",
                source,
            })
    }

    /// One memory in full, by identifier — Phase 26's `memory.get`.
    ///
    /// Verifies the stored project identifier before returning anything, which
    /// is the read boundary the module documentation describes. A row bound to
    /// another project is an error, never `None`: silently reporting "no such
    /// memory" would hide the fact that a foreign row is sitting in this
    /// project's file.
    pub fn get(&self, id: &MemoryId) -> Result<Option<MemoryRecord>, MemoryStoreError> {
        let record: Option<MemoryRecord> = self
            .conn
            .query_row(
                &format!("SELECT {ALL_COLUMNS} FROM memories WHERE id = ?1"),
                [id.as_str()],
                row_to_record,
            )
            .optional()
            .map_err(|source| MemoryStoreError::Sql {
                action: "read a memory",
                source,
            })?
            .transpose()?;

        match record {
            Some(record) if record.project_id != self.project_id => {
                Err(MemoryStoreError::ForeignProject {
                    id: record.id,
                    expected: self.project_id.clone(),
                    actual: record.project_id,
                })
            }
            other => Ok(other),
        }
    }

    /// Resolve a whole identifier, or the leading part of one, to exactly one
    /// memory.
    ///
    /// A prefix is a requirement rather than a convenience, for the reason
    /// `glasshouse sessions` established: listings print a short form, so the
    /// short form is the only one a user can copy from the screen. Ambiguity
    /// is refused and every candidate named, and matching uses `substr` rather
    /// than `LIKE` so a `%` typed by the user is a character, not a wildcard
    /// that would match every memory in the project.
    pub fn resolve_id(&self, prefix: &str) -> Result<MemoryId, MemoryStoreError> {
        let prefix = prefix.trim().to_ascii_lowercase();
        if prefix.is_empty() || !prefix.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(MemoryStoreError::NotFound {
                id: MemoryId(prefix),
            });
        }

        let mut statement = self
            .conn
            .prepare("SELECT id FROM memories WHERE substr(id, 1, ?2) = ?1 ORDER BY id")
            .map_err(|source| MemoryStoreError::Sql {
                action: "prepare the memory lookup",
                source,
            })?;
        let matches: Vec<MemoryId> = statement
            .query_map(
                rusqlite::params![&prefix, i64::try_from(prefix.len()).unwrap_or(i64::MAX)],
                |row| row.get::<_, String>(0).map(MemoryId),
            )
            .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
            .map_err(|source| MemoryStoreError::Sql {
                action: "look a memory up by identifier",
                source,
            })?;

        match matches.as_slice() {
            [only] => Ok(only.clone()),
            _ => Err(MemoryStoreError::NotFound {
                id: MemoryId(prefix),
            }),
        }
    }

    /// Record that one memory replaced another — Phase 22.
    ///
    /// The older memory becomes [`MemoryStatus::Superseded`] and names its
    /// successor. Nothing is deleted: the history is the point, and the
    /// superseding identifier is what stops a later agent resurrecting the old
    /// decision without knowing why it went.
    ///
    /// Both identifiers are checked against this project first, so a
    /// supersession can never be recorded across projects even if a caller
    /// somehow held a foreign identifier.
    pub fn supersede(
        &self,
        old: &MemoryId,
        replacement: &MemoryId,
    ) -> Result<MemoryRecord, MemoryStoreError> {
        self.supersede_with_reason(old, replacement, None)
    }

    /// [`MemoryStore::supersede`], recording **why** — map line 925.
    ///
    /// # Why the reason is a separate door rather than a changed signature
    ///
    /// Superseding without a reason stays legal and stores `NULL`: the map
    /// asks that the reason be *recordable*, not that every supersession have
    /// one, and Phase 22's `superseded_by` is already allowed to be absent for
    /// the same kind of reason. Callers that have nothing to say keep calling
    /// [`MemoryStore::supersede`] unchanged.
    ///
    /// # What happens to blank text, and why here rather than at the caller
    ///
    /// `Some("")` and `Some("   ")` are recorded as `None`. A reason that is
    /// only whitespace is not a reason, and if it were stored the row would
    /// read back as *"a reason was recorded"* to every consumer. Migration
    /// 13's `CHECK` refuses `''` outright, so this is the trim that keeps a
    /// blank `--reason` from being an error the user cannot act on.
    ///
    /// # The reason is operator text and never reaches SQL as text
    ///
    /// It is bound as parameter `?4` — never formatted into the statement —
    /// and it is not logged. The `UPDATE` also keeps its `project_id`
    /// predicate, so this cannot write across the project boundary even if a
    /// caller somehow held a foreign identifier.
    pub fn supersede_with_reason(
        &self,
        old: &MemoryId,
        replacement: &MemoryId,
        reason: Option<&str>,
    ) -> Result<MemoryRecord, MemoryStoreError> {
        if old == replacement {
            return Err(MemoryStoreError::SelfSupersession { id: old.clone() });
        }
        // `get` carries the project check, so both ends are verified before
        // anything is written.
        self.get(old)?
            .ok_or_else(|| MemoryStoreError::NotFound { id: old.clone() })?;
        self.get(replacement)?
            .ok_or_else(|| MemoryStoreError::NotFound {
                id: replacement.clone(),
            })?;

        let reason = reason.map(str::trim).filter(|text| !text.is_empty());
        self.conn
            .execute(
                // `superseded_reason` is assigned unconditionally, so a
                // supersession with no reason **clears** whatever an earlier
                // one left. A row explaining a supersession that has since
                // been replaced by a different one would be worse than an
                // empty column.
                "UPDATE memories \
                 SET status = ?2, superseded_by = ?3, superseded_reason = ?4, updated_at = ?5 \
                 WHERE id = ?1 AND project_id = ?6",
                rusqlite::params![
                    old.as_str(),
                    MemoryStatus::Superseded.as_str(),
                    replacement.as_str(),
                    reason,
                    (self.clock)(),
                    &self.project_id,
                ],
            )
            .map_err(|source| MemoryStoreError::Sql {
                action: "record a supersession",
                source,
            })?;

        self.get(old)?
            .ok_or_else(|| MemoryStoreError::NotFound { id: old.clone() })
    }

    /// Move a memory to another lifecycle status.
    ///
    /// Setting [`MemoryStatus::Superseded`] this way is allowed and leaves
    /// `superseded_by` alone — Phase 22 asks for the successor's identifier
    /// only "when a direct supersession relationship is known", so retiring a
    /// memory without one is a real state rather than a missing field.
    ///
    /// Moving *away* from `Superseded` clears the successor, because a memory
    /// that is active again has not been replaced by anything; the schema's
    /// `CHECK` would refuse the inconsistent row anyway, and clearing it here
    /// means the caller gets the intended state instead of an error.
    ///
    /// It clears `superseded_reason` in the same expression, and that is not
    /// cosmetic: migration 13 could not give the reason a `CHECK` tying it to
    /// `status` — `ALTER TABLE ADD COLUMN` cannot add a table constraint — so
    /// this is the one place that keeps *"why it was superseded"* from
    /// outliving the supersession it explains.
    pub fn set_status(
        &self,
        id: &MemoryId,
        status: MemoryStatus,
    ) -> Result<MemoryRecord, MemoryStoreError> {
        self.get(id)?
            .ok_or_else(|| MemoryStoreError::NotFound { id: id.clone() })?;

        let keep_successor = status == MemoryStatus::Superseded;
        self.conn
            .execute(
                "UPDATE memories \
                 SET status = ?2, \
                     superseded_by = CASE WHEN ?3 THEN superseded_by ELSE NULL END, \
                     superseded_reason = \
                         CASE WHEN ?3 THEN superseded_reason ELSE NULL END, \
                     updated_at = ?4 \
                 WHERE id = ?1 AND project_id = ?5",
                rusqlite::params![
                    id.as_str(),
                    status.as_str(),
                    keep_successor,
                    (self.clock)(),
                    &self.project_id,
                ],
            )
            .map_err(|source| MemoryStoreError::Sql {
                action: "change a memory's status",
                source,
            })?;

        self.get(id)?
            .ok_or_else(|| MemoryStoreError::NotFound { id: id.clone() })
    }

    /// Mark a memory for review — Phase 21C: something changed that may
    /// invalidate this memory, and a person or a stronger agent has to look.
    ///
    /// A state change with a stated reason, never a deletion: the memory
    /// moves to [`MemoryStatus::NeedsReview`] and `reason` is recorded beside
    /// it, so a later reader knows *what changed* and not only *that
    /// something did*. Whoever resolves the review moves the status onward
    /// with [`MemoryStore::set_status`] or [`MemoryStore::reaffirm`]; this
    /// method only ever raises the flag, never clears it, which is why it
    /// takes a reason and `set_status` does not.
    pub fn mark_for_review(
        &self,
        id: &MemoryId,
        reason: ReviewReason,
    ) -> Result<MemoryRecord, MemoryStoreError> {
        self.get(id)?
            .ok_or_else(|| MemoryStoreError::NotFound { id: id.clone() })?;

        let now = (self.clock)();
        self.conn
            .execute(
                "UPDATE memories \
                 SET status = ?2, review_reason = ?3, review_marked_at = ?4, updated_at = ?5 \
                 WHERE id = ?1 AND project_id = ?6",
                rusqlite::params![
                    id.as_str(),
                    MemoryStatus::NeedsReview.as_str(),
                    reason.as_str(),
                    now,
                    now,
                    &self.project_id,
                ],
            )
            .map_err(|source| MemoryStoreError::Sql {
                action: "mark a memory for review",
                source,
            })?;

        self.get(id)?
            .ok_or_else(|| MemoryStoreError::NotFound { id: id.clone() })
    }

    /// Reaffirm a memory against current project state — Phase 21D: record
    /// that it was rechecked, without touching anything else.
    ///
    /// Writes [`MemoryRecord::last_validated_at`] and nothing else — not
    /// [`MemoryRecord::created_at`], which the age-tracking box in Phase 21D
    /// asks to stay put, and not [`MemoryRecord::updated_at`] or
    /// [`MemoryRecord::status`], which decay must not touch, because
    /// reaffirming is a comment on how fresh a memory's *validation* is, not
    /// on the memory's lifecycle. A memory that is [`MemoryStatus::NeedsReview`]
    /// stays there after a reaffirm; resolve the review with
    /// [`MemoryStore::set_status`] separately once it has actually been
    /// looked at.
    pub fn reaffirm(&self, id: &MemoryId) -> Result<MemoryRecord, MemoryStoreError> {
        self.get(id)?
            .ok_or_else(|| MemoryStoreError::NotFound { id: id.clone() })?;

        self.conn
            .execute(
                "UPDATE memories SET last_validated_at = ?2 WHERE id = ?1 AND project_id = ?3",
                rusqlite::params![id.as_str(), (self.clock)(), &self.project_id],
            )
            .map_err(|source| MemoryStoreError::Sql {
                action: "reaffirm a memory",
                source,
            })?;

        self.get(id)?
            .ok_or_else(|| MemoryStoreError::NotFound { id: id.clone() })
    }

    /// Refuse an automatic caller a high-impact memory — Phase 22's review
    /// gate, shared with [`MemoryStore::resolve_conflict`] rather than
    /// redesigned for Phase 21G's revalidation.
    ///
    /// Unclassified counting as high-impact is the same fail-closed reasoning
    /// `resolve_conflict` documents: `None` means nobody has judged how
    /// binding this memory is, and treating "unknown" as safe would make
    /// every memory recorded before a classifier existed automatically
    /// revalidatable by an automatic caller.
    fn require_reviewed_for_high_impact(
        &self,
        record: &MemoryRecord,
        by: ConflictResolver,
    ) -> Result<(), MemoryStoreError> {
        if by == ConflictResolver::Automatic
            && let Some(impact) = high_impact_reason(record.authority)
        {
            return Err(MemoryStoreError::ReviewRequired {
                id: record.id.clone(),
                impact,
            });
        }
        Ok(())
    }

    /// Revalidate a memory as reaffirmed — Phase 21G: looked at, still true.
    ///
    /// Two calls into existing primitives, deliberately: [`MemoryStore::reaffirm`]
    /// records that the memory was rechecked without touching its status —
    /// see that method's own documentation, which asks a caller to
    /// "resolve the review with `set_status` separately once it has actually
    /// been looked at." This *is* that review, so it does both: a fresh
    /// `last_validated_at`, and a move back to [`MemoryStatus::Active`].
    pub fn revalidate_reaffirmed(
        &self,
        id: &MemoryId,
        by: ConflictResolver,
    ) -> Result<MemoryRecord, MemoryStoreError> {
        let record = self
            .get(id)?
            .ok_or_else(|| MemoryStoreError::NotFound { id: id.clone() })?;
        self.require_reviewed_for_high_impact(&record, by)?;

        self.reaffirm(id)?;
        self.set_status(id, MemoryStatus::Active)
    }

    /// Revalidate a memory as still needing review — Phase 21G: unresolved,
    /// with a reason that may have changed since it was first flagged.
    pub fn revalidate_needs_review(
        &self,
        id: &MemoryId,
        reason: ReviewReason,
        by: ConflictResolver,
    ) -> Result<MemoryRecord, MemoryStoreError> {
        let record = self
            .get(id)?
            .ok_or_else(|| MemoryStoreError::NotFound { id: id.clone() })?;
        self.require_reviewed_for_high_impact(&record, by)?;

        self.mark_for_review(id, reason)
    }

    /// Revalidate a memory as superseded — Phase 21G: replaced by a named
    /// successor, and map line 925's `reason` for *why*.
    ///
    /// `reason` is `None` when the operator gave none; see
    /// [`MemoryStore::supersede_with_reason`] for what is stored then. The
    /// high-impact gate is asked **before** the reason is looked at, so a
    /// refused supersession records nothing at all — not even the explanation
    /// of a supersession that did not happen.
    pub fn revalidate_superseded(
        &self,
        id: &MemoryId,
        replacement: &MemoryId,
        reason: Option<&str>,
        by: ConflictResolver,
    ) -> Result<MemoryRecord, MemoryStoreError> {
        let record = self
            .get(id)?
            .ok_or_else(|| MemoryStoreError::NotFound { id: id.clone() })?;
        self.require_reviewed_for_high_impact(&record, by)?;

        self.supersede_with_reason(id, replacement, reason)
    }

    /// Revalidate a memory as invalidated — Phase 21G: a known invalidation
    /// condition occurred.
    pub fn revalidate_invalidated(
        &self,
        id: &MemoryId,
        by: ConflictResolver,
    ) -> Result<MemoryRecord, MemoryStoreError> {
        let record = self
            .get(id)?
            .ok_or_else(|| MemoryStoreError::NotFound { id: id.clone() })?;
        self.require_reviewed_for_high_impact(&record, by)?;

        self.set_status(id, MemoryStatus::Invalidated)
    }

    /// Every memory **of this project** with the given status, most recently
    /// updated first.
    ///
    /// The ordering is the `memories_by_status_updated` index read directly.
    ///
    /// # Why the `project_id` predicate is in the `WHERE` and not a guard
    ///
    /// Every other scoped operation here can lean on a leading
    /// [`MemoryStore::get`], which carries the project check. A listing takes
    /// no identifier, so there is nothing to guard: the `WHERE` clause is the
    /// entire boundary. Phase 21G made the same argument for five
    /// `UPDATE memories` statements — a foreign row planted by a restored
    /// backup or an older build must not be reachable — and this is the read
    /// side of it, which is the side that renders another project's memory
    /// *body* on a user's screen: `main.rs::memory_revalidate_list` prints it,
    /// and the shell's project-knowledge panel renders it.
    pub fn with_status(
        &self,
        status: MemoryStatus,
        limit: usize,
    ) -> Result<Vec<MemoryRecord>, MemoryStoreError> {
        let mut statement = self
            .conn
            .prepare(&format!(
                "SELECT {ALL_COLUMNS} FROM memories \
                 WHERE status = ?1 AND project_id = ?3 \
                 ORDER BY updated_at DESC, id ASC LIMIT ?2"
            ))
            .map_err(|source| MemoryStoreError::Sql {
                action: "prepare the memory listing",
                source,
            })?;
        let rows = statement
            .query_map(
                rusqlite::params![
                    status.as_str(),
                    i64::try_from(limit).unwrap_or(i64::MAX),
                    &self.project_id,
                ],
                row_to_record,
            )
            .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
            .map_err(|source| MemoryStoreError::Sql {
                action: "list memories",
                source,
            })?;
        rows.into_iter().collect()
    }

    /// Record that two current memories contradict each other — Phase 22.
    ///
    /// Both move to [`MemoryStatus::Conflicted`], which is what makes the
    /// contradiction *visible*: a conflicted memory is not current knowledge,
    /// so a default search stops returning either of them as if it were
    /// settled truth, and a caller that asks for them gets a status that says
    /// why. Neither is deleted and neither is silently preferred, because
    /// picking one is exactly the judgment this state exists to defer.
    ///
    /// Both identifiers are checked against this project before anything is
    /// written.
    pub fn mark_conflicted(
        &self,
        one: &MemoryId,
        other: &MemoryId,
    ) -> Result<(MemoryRecord, MemoryRecord), MemoryStoreError> {
        if one == other {
            return Err(MemoryStoreError::SelfSupersession { id: one.clone() });
        }
        let first = self.set_status(one, MemoryStatus::Conflicted)?;
        let second = self.set_status(other, MemoryStatus::Conflicted)?;
        Ok((first, second))
    }

    /// Settle a conflicted memory, if the caller is allowed to.
    ///
    /// A [`ConflictResolver::Automatic`] caller may settle a conflict only on
    /// a memory that is *not* high-impact. High-impact means an authority that
    /// [`MemoryAuthority::is_binding`] — an invariant, a constraint, or an
    /// accepted decision — **and also** an authority that has not been
    /// classified at all.
    ///
    /// Unclassified counting as high-impact is the conservative direction and
    /// is deliberate. `None` means nobody has judged how binding this memory
    /// is; treating "unknown" as "safe to resolve without review" would make
    /// every memory recorded before Phase 21A's classifier automatically
    /// resolvable, which is precisely the silent promotion the map's
    /// "treat uncertain authority classification conservatively" line forbids.
    /// Fail closed: an unknown authority needs review.
    ///
    /// A [`ConflictResolver::Reviewed`] caller may settle anything. The
    /// judgment is theirs; this method only records it.
    pub fn resolve_conflict(
        &self,
        id: &MemoryId,
        outcome: MemoryStatus,
        by: ConflictResolver,
    ) -> Result<MemoryRecord, MemoryStoreError> {
        let record = self
            .get(id)?
            .ok_or_else(|| MemoryStoreError::NotFound { id: id.clone() })?;

        if by == ConflictResolver::Automatic
            && let Some(impact) = high_impact_reason(record.authority)
        {
            return Err(MemoryStoreError::ReviewRequired {
                id: record.id,
                impact,
            });
        }

        self.set_status(id, outcome)
    }

    /// Promote or demote a memory's authority class — Phase 21A.
    ///
    /// # Only a reviewer may create an invariant
    ///
    /// An [`Classifier::Extractor`] caller may set any class except
    /// [`MemoryAuthority::Invariant`], and is refused with
    /// [`MemoryStoreError::ReviewRequired`] if it tries. That is the storage
    /// half of the rule `super::extract::authority` implements on the
    /// producer side, and the two are deliberately independent: the extractor
    /// cannot *construct* an invariant, and the store would not *accept* one
    /// from it either. A single control would be a single thing to forget.
    ///
    /// Lowering is never refused, whoever asks. Phase 21A's concern is
    /// memories that become binding without anyone deciding they should;
    /// a memory becoming *less* binding needs no protection, and requiring
    /// review to demote an over-confident classification would leave the
    /// over-confident classification in place.
    ///
    /// Passing `None` clears the class back to unclassified, which
    /// retrieval already treats conservatively — see [`MemoryAuthority`].
    pub fn set_authority(
        &self,
        id: &MemoryId,
        authority: Option<MemoryAuthority>,
        by: Classifier,
    ) -> Result<(MemoryRecord, AuthorityChange), MemoryStoreError> {
        let record = self
            .get(id)?
            .ok_or_else(|| MemoryStoreError::NotFound { id: id.clone() })?;

        if by == Classifier::Extractor && authority == Some(MemoryAuthority::Invariant) {
            return Err(MemoryStoreError::ReviewRequired {
                id: record.id,
                impact: MemoryAuthority::Invariant.as_str(),
            });
        }

        if record.authority == authority {
            return Ok((record, AuthorityChange::Unchanged));
        }

        self.conn
            .execute(
                "UPDATE memories SET authority = ?2, updated_at = ?3 \
                 WHERE id = ?1 AND project_id = ?4",
                rusqlite::params![
                    id.as_str(),
                    authority.map(MemoryAuthority::as_str),
                    (self.clock)(),
                    &self.project_id,
                ],
            )
            .map_err(|source| MemoryStoreError::Sql {
                action: "change a memory's authority",
                source,
            })?;

        let updated = self
            .get(id)?
            .ok_or_else(|| MemoryStoreError::NotFound { id: id.clone() })?;
        Ok((updated, AuthorityChange::Changed))
    }

    /// Every current memory whose authority may be presented to an agent as a
    /// rule — Phase 21A's *"retrieve current active invariants and constraints
    /// separately from historical decisions"*.
    ///
    /// Filters on [`MemoryAuthority::is_binding`] rather than on a list of
    /// class names written out in SQL, so a class added to the enum is
    /// classified in exactly one place. An **unclassified** memory is not
    /// binding and does not appear: `None` means nobody has judged how binding
    /// it is, and the conservative reading of unknown is "not a rule".
    pub fn binding(&self, limit: usize) -> Result<Vec<MemoryRecord>, MemoryStoreError> {
        let mut statement = self
            .conn
            .prepare(&format!(
                "SELECT {ALL_COLUMNS} FROM memories \
                 WHERE project_id = ?1 AND status = ?2 AND authority IS NOT NULL \
                 ORDER BY updated_at DESC, id ASC"
            ))
            .map_err(|source| MemoryStoreError::Sql {
                action: "prepare the binding-memory listing",
                source,
            })?;
        let rows = statement
            .query_map(
                rusqlite::params![&self.project_id, MemoryStatus::Active.as_str()],
                row_to_record,
            )
            .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
            .map_err(|source| MemoryStoreError::Sql {
                action: "list binding memories",
                source,
            })?;

        let mut kept = Vec::new();
        for row in rows {
            let record = row?;
            if record.authority.is_some_and(MemoryAuthority::is_binding) && kept.len() < limit {
                kept.push(record);
            }
        }
        Ok(kept)
    }

    /// How many memories this project holds, by status.
    ///
    /// Scoped in the `WHERE` clause for the reason
    /// [`MemoryStore::with_status`] documents: a count takes no identifier, so
    /// no leading guard stands in front of it, and a foreign row planted in
    /// this project's file would otherwise be counted as this project's.
    pub fn count(&self, status: MemoryStatus) -> Result<i64, MemoryStoreError> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM memories WHERE status = ?1 AND project_id = ?2",
                rusqlite::params![status.as_str(), &self.project_id],
                |row| row.get(0),
            )
            .map_err(|source| MemoryStoreError::Sql {
                action: "count memories",
                source,
            })
    }
}

/// Build a record from a row of [`ALL_COLUMNS`].
///
/// The outer `Result` is SQLite's; the inner one is this module's, because an
/// unrecognized enum string is a data problem rather than a database problem
/// and deserves an error that names the column and the value. Nothing here
/// substitutes a default for an unreadable value: a memory whose kind cannot
/// be interpreted must not silently become a `finding`.
pub(super) fn row_to_record(
    row: &Row<'_>,
) -> rusqlite::Result<Result<MemoryRecord, MemoryStoreError>> {
    let id = MemoryId(row.get("id")?);
    let kind_text: String = row.get("kind")?;
    let status_text: String = row.get("status")?;
    let authority_text: Option<String> = row.get("authority")?;

    let Some(kind) = MemoryKind::from_stored(&kind_text) else {
        return Ok(Err(MemoryStoreError::UnknownValue {
            id,
            column: "kind",
            value: kind_text,
        }));
    };
    let Some(status) = MemoryStatus::from_stored(&status_text) else {
        return Ok(Err(MemoryStoreError::UnknownValue {
            id,
            column: "status",
            value: status_text,
        }));
    };
    let authority = match authority_text {
        None => None,
        Some(text) => match MemoryAuthority::from_stored(&text) {
            Some(authority) => Some(authority),
            None => {
                return Ok(Err(MemoryStoreError::UnknownValue {
                    id,
                    column: "authority",
                    value: text,
                }));
            }
        },
    };

    let phase_text: Option<String> = row.get("project_phase")?;
    let project_phase = match phase_text {
        None => None,
        Some(text) => match ProjectPhase::from_stored(&text) {
            Some(phase) => Some(phase),
            None => {
                return Ok(Err(MemoryStoreError::UnknownValue {
                    id,
                    column: "project_phase",
                    value: text,
                }));
            }
        },
    };

    // Both or neither, and in order: migration 6's two triggers refuse
    // anything else on the way in, so a row that fails this came from
    // somewhere those triggers do not run — a hand-edited file. Reported as
    // an unreadable value rather than silently halved, for the reason the
    // enums above are: nothing here substitutes a default for a value it
    // cannot read.
    let first: Option<i64> = row.get("source_event_first")?;
    let last: Option<i64> = row.get("source_event_last")?;
    let source_events = match (first, last) {
        (None, None) => None,
        (Some(first), Some(last)) => match SourceEvents::new(first, last) {
            Some(events) => Some(events),
            None => {
                return Ok(Err(MemoryStoreError::UnknownValue {
                    id,
                    column: "source_event_first",
                    value: format!("{first}..{last}"),
                }));
            }
        },
        (present, _) => {
            return Ok(Err(MemoryStoreError::UnknownValue {
                id,
                column: if present.is_some() {
                    "source_event_last"
                } else {
                    "source_event_first"
                },
                value: "absent".to_owned(),
            }));
        }
    };

    let review_reason_text: Option<String> = row.get("review_reason")?;
    let review_reason = match review_reason_text {
        None => None,
        Some(text) => match ReviewReason::from_stored(&text) {
            Some(reason) => Some(reason),
            None => {
                return Ok(Err(MemoryStoreError::UnknownValue {
                    id,
                    column: "review_reason",
                    value: text,
                }));
            }
        },
    };

    Ok(Ok(MemoryRecord {
        id,
        project_id: row.get("project_id")?,
        kind,
        authority,
        status,
        subject: row.get("subject")?,
        body: row.get("body")?,
        source_session_id: row.get("source_session_id")?,
        source_commit: row.get("source_commit")?,
        source_events,
        provenance: DecisionProvenance {
            rationale: row.get("rationale")?,
            project_phase,
            problem: row.get("problem")?,
            assumptions: row.get("assumptions")?,
            scale_assumptions: row.get("scale_assumptions")?,
            security_assumptions: row.get("security_assumptions")?,
            compatibility_assumptions: row.get("compatibility_assumptions")?,
            operational_assumptions: row.get("operational_assumptions")?,
            evidence: row.get("evidence")?,
            source_excerpt: row.get("source_excerpt")?,
        },
        superseded_by: row.get::<_, Option<String>>("superseded_by")?.map(MemoryId),
        superseded_reason: row.get("superseded_reason")?,
        validity_conditions: row.get("validity_conditions")?,
        invalidation_conditions: row.get("invalidation_conditions")?,
        review_reason,
        review_marked_at: row.get("review_marked_at")?,
        last_validated_at: row.get("last_validated_at")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    }))
}

/// Why a memory counts as high-impact for Phase 22's review requirement, or
/// `None` if it does not.
///
/// Returns the word used in the refusal, so the error says *which* of the two
/// reasons applied rather than making the reader guess.
fn high_impact_reason(authority: Option<MemoryAuthority>) -> Option<&'static str> {
    match authority {
        // Nobody has judged how binding this is. Unknown is not safe.
        None => Some("unclassified"),
        Some(authority) if authority.is_binding() => Some(authority.as_str()),
        Some(_) => None,
    }
}
