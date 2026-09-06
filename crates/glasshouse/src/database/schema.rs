//! Schema vocabularies and pinned constants, split out of `database.rs`
//! by Phase 59's decomposition. Values are unchanged from before the move;
//! only the visibility of items a sibling file needs widened.

/// The highest schema version this build knows how to migrate to.
///
/// Each version is one additive migration; its own doc comment (in
/// `migrations/v1_to_v13.rs` or `migrations/v14_on.rs`) carries the rationale
/// for that specific change — the column shapes, the `CHECK`s chosen or
/// refused, and the map line or phase it serves. This constant only tracks
/// the ceiling; later migrations are appended alongside the existing ones and
/// this constant moves with them.
///
/// History: design-decisions.md, "Trims: the remaining module docs, second
/// packet", `SUPPORTED_SCHEMA_VERSION`.
pub(super) const SUPPORTED_SCHEMA_VERSION: i64 = 28;

/// The `lifecycle_events.kind` values migration 5's `CHECK` constraint allows.
///
/// Here rather than only in the SQL so that
/// [`crate::events::LifecycleEvent::kind`] can be pinned against it by a test.
/// A renamed variant otherwise compiles perfectly and then fails as a
/// constraint violation on a background writer thread, where nobody is
/// looking.
pub(crate) const LIFECYCLE_EVENT_KINDS: [&str; 12] = [
    "session_started",
    "session_resumed",
    "turn_started",
    "turn_ended",
    "waiting_for_user",
    "text_delivered",
    "interrupt_delivered",
    "process_exited",
    "output_ended",
    "gateway_unhealthy",
    "gateway_backend_changed",
    "file_touched",
];

/// The `evaluation_observations.kind` values this build writes.
///
/// **Deliberately not a SQL `CHECK`.** Migration 15's own doc comment argues
/// this at length: `lifecycle_events.kind`'s `CHECK` is exactly why an
/// eleventh value cost a full table rebuild (migration 7) and a twelfth is
/// refused outright by this file's house rule, and Phase 51 is the phase whose
/// vocabulary is guaranteed to grow. So the vocabulary lives in Rust —
/// [`crate::evaluation::EvaluationKind`], an exhaustive `match` at the single
/// writer, and this constant pinned against it by a test — which is where
/// [`LIFECYCLE_EVENT_KINDS`]'s own doc comment says the real guarantee already
/// lives. `response_profile` (migration 8) is the precedent for a column with
/// no `CHECK` at all.
///
/// One entry per landed producer. Variants are added as producers land, never
/// in advance: an enum written before its writers is the same mistake as a
/// table written before its counts.
pub(crate) const EVALUATION_KINDS: [&str; 18] = [
    "memory_retrieved",
    "memory_retrieval_miss",
    "disposable_route_decided",
    "routing_override_decided",
    "routing_continuation_decided",
    "routing_cost_class_observed",
    "routing_evidence_observed",
    "routing_outcome_observed",
    "routing_tier_observed",
    "failover_prevented",
    "memory_rated",
    "memory_revalidated",
    "turn_outcome_observed",
    "session_route_decided",
    "routing_consumption_estimated",
    "reserve_availability_observed",
    "routing_rated",
    "memory_extraction_observed",
];

/// The `routing_observations.failure_class` values this build writes —
/// capability map line 1364's vocabulary, migration 18.
///
/// Deliberately not a SQL `CHECK` — a failure vocabulary grows as providers
/// invent new ways to fail. The vocabulary lives in Rust
/// ([`crate::routing::evidence::FailureClass`], an exhaustive `match` at the
/// single writer) and this constant is pinned against it by
/// `every_failure_class_the_type_supports_is_one_the_schema_records`, its
/// only consumer (`#[cfg(test)]`): the production reader reports an
/// unrecognised value through `FailureClass::from_stored`, not through this
/// list.
///
/// History: design-decisions.md, "Trims: the remaining module docs, second
/// packet", `FAILURE_CLASSES`.
#[cfg(test)]
pub(super) const FAILURE_CLASSES: [&str; 9] = [
    "throttle",
    "exhausted_quota",
    "upstream_5xx",
    "timeout",
    "stream_abort",
    "empty_completion",
    "credential_failure",
    "request_incompatibility",
    "unknown",
];

/// The `routing_observations.task_class` values this build writes —
/// capability map line 1276's vocabulary, migration 23.
///
/// **Deliberately not a SQL `CHECK`**, for [`FAILURE_CLASSES`]' reasons, and
/// with one more of its own: the production reader
/// (`crate::routing::request::TaskClass::from_stored`) answers `None` for an
/// unrecognised word rather than failing the row, so a `CHECK` would be the
/// *only* thing in the system that could refuse one — and it would refuse it
/// at the writer, on a future build's own valid class.
///
/// Five entries, in [`crate::routing::request::TaskClass`]'s declaration
/// order, pinned against it by
/// `every_task_class_the_type_supports_is_one_the_schema_records`.
///
/// `#[cfg(test)]` for [`FAILURE_CLASSES`]' reason: the pinning test is this
/// constant's only consumer.
#[cfg(test)]
pub(super) const TASK_CLASSES: [&str; 5] = [
    "question",
    "investigation",
    "code modification",
    "shell work",
    "browser work",
];

/// The `memory_files.provenance` values this build writes.
///
/// Deliberately not a SQL `CHECK` — this vocabulary will grow. The vocabulary
/// lives in Rust as [`crate::memory::FileAssociation`], with an exhaustive
/// `as_str` at the single writer and a test pinning the two against each
/// other. `observed` means *"this file differed from the index at the
/// moment the memory was extracted"*, a correlation, never a claim that the
/// memory refers to the file; `referenced` (map line 1139, migration 26) is
/// a **different producer** — a path the extraction model chose out of the
/// files the session demonstrably edited — so a reader can weigh them apart
/// rather than averaging a correlation with a claim. `#[cfg(test)]` because
/// an unrecognised value is dropped by
/// [`crate::memory::FileAssociation::from_stored`] rather than reported, so
/// only the pinning test consumes this constant.
///
/// History: design-decisions.md, "Trims: the remaining module docs, second
/// packet", `MEMORY_FILE_PROVENANCE`.
#[cfg(test)]
pub(crate) const MEMORY_FILE_PROVENANCE: [&str; 2] = ["observed", "referenced"];

/// The largest checkpoint the project database will store, in bytes.
///
/// The map's constraint — *keep checkpoints deliberately small enough to
/// bootstrap a fresh session cheaply* — expressed where it cannot be talked
/// out of. [`crate::checkpoint`] trims to fit before it ever gets here; this
/// is what makes the bound a property of the stored data rather than of one
/// builder remembering to apply it.
pub(crate) const MAX_CHECKPOINT_BYTES: usize = 8 * 1024;
