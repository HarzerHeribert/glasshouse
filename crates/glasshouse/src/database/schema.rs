//! Schema vocabularies and pinned constants, split out of `database.rs`
//! by Phase 59's decomposition. Values are unchanged from before the move;
//! only the visibility of items a sibling file needs widened.

/// The highest schema version this build knows how to migrate to.
///
/// Version 1 is the empty-but-initialized schema plus the `project_metadata`
/// table. Version 2 adds `sessions`. Version 3 adds `sessions.launch_profile`
/// and `sessions.backend_resource`. Version 4 adds `memories` and its FTS5
/// index. Version 5 adds `lifecycle_events` and `checkpoints`. Version 6 adds
/// event provenance and decision provenance to `memories`, and rebuilds the
/// FTS5 index over the rationale. Version 7 admits `gateway_backend_changed`
/// to `lifecycle_events.kind` and adds the three columns that carry it,
/// rebuilding the table rather than altering its `CHECK` — see the
/// migration's own doc comment for why `seq` survives that rebuild unchanged.
/// Version 8 adds the rest of what Phase 10 line 645 requires a session to
/// record — `model`, `pairing_class`, `protocol`, `response_profile` and
/// `response_mechanism` — plus the two labels a person owns, `display_name`
/// and `purpose`. Version 9 adds the supervision columns Phase 10A needs — the
/// identity of the process a session was started in (`process_id`,
/// `process_started_at`, `process_host`) and what supervision has since
/// concluded about it (`supervision`, `supervision_reason`). Version 10 adds
/// Phase 21C's validity and invalidation conditions and the review/decay
/// bookkeeping Phase 21D needs (`review_reason`, `review_marked_at`,
/// `last_validated_at`). Version 11 adds `routing_observations`, Phase 33A's
/// append-oriented ledger of what actually happened on a routed turn — see the
/// migration's own doc comment for its shape and why it accepts no `UPDATE`.
/// Version 12 adds `sessions.source_session_id`, Phase 40 line 1646's record of
/// which session, if any, a session was bootstrapped from. Version 13 adds
/// `memories.superseded_reason`, map line 925's record of *why* a decision was
/// superseded — the sentence that stops a future agent resurrecting it without
/// context. Version 14 adds `checkpoints.seq`, the order checkpoints were
/// written in — `created_at` is whole seconds, so two written inside one
/// second were previously separated by a coin flip on a random identifier, and
/// *"the most recent checkpoint"* was wrong about half the time.
/// Version 15 adds `evaluation_observations`, Phase 51's record of a decision
/// Glasshouse made whose wisdom is only visible later — see the migration's
/// own doc comment for why its `kind` carries no `CHECK` and why it is the
/// first table in this schema that is *deliberately prunable*.
/// Version 16 adds `sessions.observed_compactions`, Phase 30's count of the
/// times a harness told Glasshouse it was about to compact its own context —
/// the one fact in that phase that was observed by the shipped binary and
/// then written down nowhere. See the migration's own doc comment for why it
/// is a counter on `sessions` rather than a twelfth `lifecycle_events` kind,
/// and for why it is the *only* column Phase 30 needed.
/// Version 17 adds `memory_files`, the first association in this schema
/// between a memory and a file — one row per (memory, path) pair, written
/// from what the working tree was observed to be at the moment extraction
/// ran. See the migration's own doc comment for why it is a join table
/// rather than a column, why `path` is repo-relative and `/`-separated, and
/// why its `provenance` says `observed` and must never say `referenced`.
/// Version 18 adds `routing_observations.failure_class`, capability map line
/// 1364's nine-way failure vocabulary — one nullable `TEXT` column with no
/// `CHECK`, for migration 15's reason. See the migration's own doc comment
/// for why it is a column beside `outcome` rather than a widening of it, and
/// [`FAILURE_CLASSES`] for where the vocabulary actually lives.
/// Version 19 adds `task_assumptions` and `assumption_transitions`, Phase
/// 21K's ledger of the premises an agent *states* a change rests on — two
/// tables, project-scoped by migration 15's two triggers and made append-only
/// by a third on each, prunable like migration 15's ledger and unlike
/// migration 5's stream. See the migration's own doc comment for why the
/// current state of an assumption is its latest transition and nothing is
/// ever `UPDATE`d, and [`crate::guardrails::store`] for the writer.
/// Version 20 adds `sessions.presentation_ref`, Phase 17 line 760's optional
/// presentation metadata — one nullable `TEXT` column naming the cmux
/// workspace a session is presented in, with no `CHECK` for migration 15's
/// reason. See the migration's own doc comment for why it is a column beside
/// `presentation` rather than a widening of it, and
/// [`crate::integrations::cmux`] for the only code that ever reads it back.
/// Version 21 adds `sessions.last_seen_commit` and
/// `memories.extraction_trigger`, capability map lines 1149 and 1153: where
/// HEAD stood the last time Glasshouse looked at a session, and which of
/// Phase 29's four memory-commit triggers produced a memory. See the
/// migration's own doc comment for why both live in one migration, why
/// neither carries a `CHECK`, and why the trigger is a column beside
/// `memories.source_commit` rather than something derived from it.
/// Version 22 adds `sessions.entitlement`, capability map line 1972: which
/// configured `[entitlements.<name>]` account served this session. See the
/// migration's own doc comment for why `backend_resource` could not answer
/// it, why the column holds a name and never a credential, and why it is
/// nullable with no `CHECK`.
/// Version 23 adds `routing_observations.task_class`, capability map line
/// 1276's *"requests consumed per task class"* — one nullable `TEXT` column
/// with no `CHECK`, migration 18's shape exactly. See the migration's own
/// doc comment for why the class is persisted rather than recomputed, why an
/// unrecognised stored word reads back as `None` rather than as an error
/// (unlike `failure_class`), and
/// [`crate::routing::burn`] for the only reader.
/// Version 24 adds `routing_observations.session_id`, `.effort_level` and
/// `.turn_shape` — capability map line 2019's *per-session* cache ratio and
/// line 2039's shadow measurement, both of which need a gateway-written row
/// to name the session it served. Three nullable `TEXT` columns with no
/// `CHECK`, no `REFERENCES` and no index, migration 23's shape exactly. See
/// the migration's own doc comment for each of those four choices and for
/// why an unrecognised stored word reads back as `None`, and
/// `docs/product/design-decisions.md`'s *A session identity on the routing
/// evidence rows* for the identity itself.
/// Version 25 adds `routing_observations.first_byte_ms`, `.first_token_ms`,
/// `.first_tool_call_ms` and `.completed_ms` — capability map lines 1347,
/// 1348, 1349 and 1355, whose TTFC, TTFT and decode-throughput figures a
/// one-second timestamp cannot express. Four nullable `INTEGER` columns,
/// each a number of milliseconds **since the upstream request was sent** and
/// never an absolute instant, each with the same column-scoped
/// `CHECK (col IS NULL OR col >= 0)` migration 11's token columns carry. See
/// the migration's own doc comment for why these are offsets rather than
/// instants, why their zero is not `dispatched_at`, and
/// `docs/product/design-decisions.md`'s *Millisecond offsets on the routing
/// row — Cluster G's second column set* for the design.
/// Version 26 admits the twelfth `lifecycle_events.kind`, `file_touched`,
/// and gives it the one payload column it carries — `path` — for capability
/// map line 1139, *"track file paths explicitly referenced by durable
/// memories"*. Migration 5's `kind` is a `CHECK` and SQLite cannot alter
/// one, so this is a table rebuild in migration 7's exact shape, `seq`
/// named explicitly so a memory's provenance range keeps pointing at the
/// same events. See the migration's own doc comment, and
/// `docs/product/design-decisions.md`'s *File paths a memory explicitly
/// references* for why the record is an event rather than a table.
/// Later migrations are appended to [`MIGRATIONS`], and this constant moves
/// with them.
pub(super) const SUPPORTED_SCHEMA_VERSION: i64 = 26;

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
pub(crate) const EVALUATION_KINDS: [&str; 15] = [
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
];

/// The `routing_observations.failure_class` values this build writes —
/// capability map line 1364's vocabulary, migration 18.
///
/// **Deliberately not a SQL `CHECK`**, for [`EVALUATION_KINDS`]' reasons: a
/// failure vocabulary is exactly the kind that grows as providers invent new
/// ways to fail, and `outcome`'s own four-value `CHECK` two columns over is
/// what this column must not repeat. The vocabulary lives in Rust —
/// [`crate::routing::evidence::FailureClass`], an exhaustive `match` at the
/// single writer, and this constant pinned against it by
/// `every_failure_class_the_type_supports_is_one_the_schema_records`.
///
/// Nine entries, and all nine are the map line's own words. `timeout` has a
/// mapping at the writer (`ureq::Error::Timeout`) but the upstream agent sets
/// no timeout today (`crate::gateway::upstream::agent`), so no row this build
/// writes will carry it until one does — recorded here rather than left for
/// the first reader to wonder about.
///
/// `#[cfg(test)]` for [`MEMORY_FILE_PROVENANCE`]'s reason, not
/// [`EVALUATION_KINDS`]': the production reader
/// (`crate::routing::evidence::row_to_observation`) reports an unrecognised
/// value through `FailureClass::from_stored`, not through this list, so the
/// pinning test is this constant's only consumer.
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
/// **Deliberately not a SQL `CHECK`**, for [`EVALUATION_KINDS`]' reasons
/// exactly: this is a vocabulary that will grow — a narrower signal than the
/// working tree is the obvious next producer — and a `CHECK` on the one
/// column certain to grow is how `lifecycle_events` came to cost a table
/// rebuild for its eleventh value and to refuse its twelfth outright.
/// The vocabulary lives in Rust as [`crate::memory::FileAssociation`], with
/// an exhaustive `as_str` at the single writer and a test pinning the two
/// against each other.
///
/// **One entry, and the second one is the whole point of having a column.**
/// `observed` means *"this file differed from the index at the moment the
/// memory was extracted"* — a correlation with the session, not a claim about
/// the memory. It is emphatically **not** *"the memory refers to this file"*,
/// which is what capability-map line 1139 asks for and what nothing in this
/// build can yet honestly produce. Recording *how* the association was made
/// is what stops a later, narrower producer from being silently averaged
/// together with this one.
///
/// # Two values now, and why the second one is not a wider version of the
/// first
///
/// `referenced` is what map line 1139 asked for and what nothing could
/// honestly produce until the context firewall's `PostToolUse` hook started
/// keeping the paths it already saw (`file_touched`, migration 26). It is a
/// **different producer**, not a better one: `observed` is the dirty index at
/// extraction time, `referenced` is a path the extraction model chose out of
/// the set of files the session demonstrably edited. A row says which, so a
/// reader can weigh them apart rather than averaging a correlation with a
/// claim.
///
/// # Still `#[cfg(test)]`
///
/// [`EVALUATION_KINDS`] is not gated because it reaches production through an
/// error message: something *reads* that table and has to say what it could
/// not interpret. `memory_files` is read by
/// [`crate::memory::MemoryStore::for_path`], but a provenance this build does
/// not know is dropped by [`crate::memory::FileAssociation::from_stored`]
/// rather than reported, so no production caller needs the list. The only
/// consumer of this constant is the test that pins it against
/// [`crate::memory::FileAssociation`], and an ungated constant with a
/// `#[cfg(test)]`-only consumer is dead code that `-D warnings` refuses.
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
