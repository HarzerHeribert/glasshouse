//! The per-project SQLite database.
//!
//! Each project owns exactly one SQLite database file, physically separate
//! from every other project's file, at `<state_dir>/glasshouse.db`. It is the
//! only future home for that project's memory (the Phase 20 memory table will
//! live here). Nothing else in Glasshouse is allowed to open a database file
//! anywhere else: the path is derived from [`crate::Runtime`], never accepted
//! from a caller.
//!
//! The module deliberately stays small: a deterministic migration mechanism
//! (`schema_migrations`), the `project_metadata` table that binds the database
//! to one project identifier, and the tables later phases have needed —
//! `sessions` and, from version 4, `memories` with its FTS5 index. It holds no
//! credentials, no WAL configuration, and no async wrappers; what a table
//! *means* lives with the module that owns it ([`crate::session::store`],
//! [`crate::memory`]), and only the schema itself lives here.
//!
//! Safety properties enforced on every open:
//!
//! - A newly created database file is owner-only (`0600` on Unix).
//! - A final database path that is a symbolic link is refused by an explicit
//!   `symlink_metadata` check performed on every launch. This handles the
//!   ordinary case; it is an open-time check, not a guarantee about files
//!   being swapped while Glasshouse runs.
//! - Any other non-regular entry at the final database path (directory,
//!   device, FIFO, socket) is refused as well; nothing but a regular file is
//!   ever opened or created there.
//! - A connection that SQLite could only open read-only (for example a
//!   mode-0400 file) is refused instead of silently degrading to a session
//!   that cannot store anything.
//! - A database whose recorded project identifier differs from the active
//!   project is refused; it must have been copied across projects.
//! - A database written by a newer Glasshouse (higher schema version) is
//!   refused. Corrupt or too-new databases are never deleted or recreated:
//!   the user keeps their data and decides what to do.

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, OptionalExtension};

use crate::Runtime;

/// File name of the project database inside the project state directory.
pub(crate) const DATABASE_FILE_NAME: &str = "glasshouse.db";

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
/// Later migrations are appended to [`MIGRATIONS`], and this constant moves
/// with them.
const SUPPORTED_SCHEMA_VERSION: i64 = 25;

/// The `lifecycle_events.kind` values migration 5's `CHECK` constraint allows.
///
/// Here rather than only in the SQL so that
/// [`crate::events::LifecycleEvent::kind`] can be pinned against it by a test.
/// A renamed variant otherwise compiles perfectly and then fails as a
/// constraint violation on a background writer thread, where nobody is
/// looking.
pub(crate) const LIFECYCLE_EVENT_KINDS: [&str; 11] = [
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
pub(crate) const EVALUATION_KINDS: [&str; 14] = [
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
const FAILURE_CLASSES: [&str; 9] = [
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
const TASK_CLASSES: [&str; 5] = [
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
/// # `#[cfg(test)]`, and exactly when that stops being right
///
/// [`EVALUATION_KINDS`] is not gated because it reaches production through an
/// error message: something *reads* that table and has to say what it could
/// not interpret. Nothing reads `memory_files` yet — this package lands the
/// producer and no consumer — so the only consumer of this constant is the
/// test that pins it against [`crate::memory::FileAssociation`], and an
/// ungated constant with a `#[cfg(test)]`-only consumer is dead code that
/// `-D warnings` refuses. Gating it is the honest shape until a reader lands;
/// the moment one does, it un-gates and grows the same "which values this
/// build knows" error [`EVALUATION_KINDS`] already has.
#[cfg(test)]
pub(crate) const MEMORY_FILE_PROVENANCE: [&str; 1] = ["observed"];

/// The largest checkpoint the project database will store, in bytes.
///
/// The map's constraint — *keep checkpoints deliberately small enough to
/// bootstrap a fresh session cheaply* — expressed where it cannot be talked
/// out of. [`crate::checkpoint`] trims to fit before it ever gets here; this
/// is what makes the bound a property of the stored data rather than of one
/// builder remembering to apply it.
pub(crate) const MAX_CHECKPOINT_BYTES: usize = 8 * 1024;

/// Migration `index + 1` upgrades a database from schema version `index` to
/// version `index + 1`. Migrations run in order inside one transaction, so a
/// partially applied upgrade can never be observed.
///
/// Migrations are append-only. Editing one that has shipped would leave
/// already-migrated databases silently disagreeing with new ones, because the
/// recorded version would match while the schema did not.
const MIGRATIONS: [&str; SUPPORTED_SCHEMA_VERSION as usize] = [
    // 1: identity of the project this database belongs to. Memory (Phase 20)
    // and everything else project-scoped joins against these rows.
    "
    CREATE TABLE project_metadata (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL
    ) WITHOUT ROWID;
    ",
    // 2: Glasshouse session metadata.
    //
    // This is Glasshouse's own record of a session and is deliberately not
    // derived from any harness's session files: `native_session_id` is a
    // nullable *reference* to the harness's own identifier, never the source
    // of truth. A session exists here whether or not the harness kept a file,
    // and deleting the harness's history does not delete this row.
    //
    // The `CHECK` constraints keep the enum columns honest at the storage
    // layer, so a future writer cannot invent a lifecycle value that readers
    // would have to guess about.
    //
    // The two triggers are the structural half of the project-isolation rule.
    // Filtering by `project_id` in queries would be a convention any new query
    // could forget; a `BEFORE INSERT`/`BEFORE UPDATE` guard cannot be
    // forgotten, because SQLite enforces it against the binding in
    // `project_metadata` no matter which code writes the row. `IS NOT` rather
    // than `<>` is deliberate: if the binding row were somehow missing, the
    // subquery yields NULL and `<>` would silently evaluate to NULL and let
    // the write through, whereas `IS NOT` aborts. The guard fails closed.
    "
    CREATE TABLE sessions (
        id                TEXT PRIMARY KEY,
        project_id        TEXT NOT NULL,
        harness           TEXT NOT NULL,
        native_session_id TEXT,
        role              TEXT NOT NULL
            CHECK (role IN ('normal', 'orchestrator', 'worker')),
        lifecycle         TEXT NOT NULL
            CHECK (lifecycle IN ('starting', 'running', 'idle',
                                 'waiting_for_user', 'stopped', 'failed',
                                 'closed')),
        presentation      TEXT NOT NULL
            CHECK (presentation IN ('embedded', 'headless', 'external')),
        created_at        INTEGER NOT NULL,
        last_activity_at  INTEGER NOT NULL
    ) WITHOUT ROWID;

    CREATE INDEX sessions_by_last_activity
        ON sessions (last_activity_at DESC);

    -- A native session belongs to at most one Glasshouse session, which is
    -- what makes the column a mapping rather than a loose annotation. Scoped
    -- per harness because two harnesses may coincidentally use the same
    -- identifier format.
    --
    -- The `WHERE` clause is not what lets many sessions sit without a native
    -- identifier: SQLite already treats NULLs as distinct in a unique index,
    -- so they would never collide either way. It is here to keep the index
    -- from carrying an entry for every not-yet-identified session, and to say
    -- plainly that the constraint is about real identifiers. Sentinel values
    -- would break that — an empty-string default in place of NULL really
    -- would collide.
    CREATE UNIQUE INDEX sessions_native_id
        ON sessions (harness, native_session_id)
        WHERE native_session_id IS NOT NULL;

    CREATE TRIGGER sessions_reject_foreign_project_insert
    BEFORE INSERT ON sessions
    FOR EACH ROW
    WHEN NEW.project_id IS NOT (
        SELECT value FROM project_metadata WHERE key = 'project_id'
    )
    BEGIN
        SELECT RAISE(ABORT, 'session belongs to a different project');
    END;

    CREATE TRIGGER sessions_reject_foreign_project_update
    BEFORE UPDATE OF project_id ON sessions
    FOR EACH ROW
    WHEN NEW.project_id IS NOT (
        SELECT value FROM project_metadata WHERE key = 'project_id'
    )
    BEGIN
        SELECT RAISE(ABORT, 'session belongs to a different project');
    END;
    ",
    // 3: which launch profile a session ran under.
    //
    // A reference, never a definition: profiles are configuration, and the
    // project database must not become a second place they live. NULL means a
    // session recorded before this column existed, which is a different fact
    // from a session that ran the Native profile — a sentinel default would
    // erase that difference, so NULL stays NULL.
    "
    ALTER TABLE sessions ADD COLUMN launch_profile TEXT;
    ALTER TABLE sessions ADD COLUMN backend_resource TEXT;
    ",
    // 4: durable project memory (Phase 20), its lifecycle (Phase 22) and the
    // full-text index it is searched through (Phase 23).
    //
    // # Why `kind` and `authority` are two columns
    //
    // They answer different questions and Phase 21A depends on the answer to
    // the second. `kind` is *what sort of thing was remembered* — Phase 20's
    // six kinds. `authority` is *how binding it is* — Phase 21A's seven
    // classes. The two lists overlap in spelling (`decision`, `constraint`
    // appear in both) and that is precisely why they must not be one column: a
    // `finding` can be an invariant, and a `decision` can have decayed to
    // `historical`. Folding them together would make "this finding is binding"
    // unrepresentable and would force Phase 21A to migrate the table.
    //
    // `authority` ships here, unused by any classifier yet, so that Phase 21A
    // adds *classification* rather than a migration — the packet's explicit
    // requirement. It is nullable on purpose: NULL means "no authority has
    // been assigned", which is a different fact from every one of the seven
    // classes, exactly as `sessions.launch_profile`'s NULL is a different fact
    // from `'native'`. Retrieval must therefore treat NULL conservatively and
    // never as an invariant; a sentinel default would have erased the
    // distinction and quietly promoted unclassified text to some class.
    //
    // # Why `status` carries a seventh value
    //
    // Phase 20 requires "at least" active, superseded, rejected, resolved,
    // needs_review and invalidated. Phase 22 requires "a conflict state for
    // memories whose current truth cannot be resolved automatically", which is
    // a lifecycle state and not an authority, so `conflicted` joins the same
    // column rather than becoming a second flag two writers could disagree
    // about.
    //
    // # Why this table has a rowid and `sessions` does not
    //
    // FTS5's external-content mode joins on `content_rowid`, so `memories`
    // cannot be `WITHOUT ROWID`. That is the whole reason; nothing else about
    // the table wants an implicit key.
    //
    // # Two triggers for project isolation, for the reason migration 2 gives
    //
    // A query can forget to filter by `project_id`; a `BEFORE INSERT` /
    // `BEFORE UPDATE` guard cannot be forgotten. `IS NOT` rather than `<>` so
    // that a missing binding row aborts instead of evaluating to NULL and
    // letting the write through. The guard fails closed.
    //
    // # Two more for supersession, instead of a foreign key
    //
    // `PRAGMA foreign_keys` is off by default in SQLite, so a `REFERENCES`
    // clause here would be decoration unless every connection remembered to
    // turn it on. A trigger is enforced by the file itself no matter who opens
    // it, and it is already this schema's idiom for exactly this reason.
    //
    // The two `CHECK`s beside them are the other half of Phase 22's
    // "mark superseded memories as non-current": a row that names a
    // superseder cannot also claim to be active, and nothing may supersede
    // itself. A memory may still be `superseded` with `superseded_by` NULL —
    // the map asks for the identifier only "when a direct supersession
    // relationship is known".
    "
    CREATE TABLE memories (
        id                TEXT PRIMARY KEY,
        project_id        TEXT NOT NULL,
        kind              TEXT NOT NULL
            CHECK (kind IN ('decision', 'constraint', 'feature',
                            'finding', 'failed_attempt', 'todo')),
        authority         TEXT
            CHECK (authority IS NULL OR authority IN
                   ('invariant', 'constraint', 'decision', 'preference',
                    'hypothesis', 'idea', 'historical')),
        status            TEXT NOT NULL
            CHECK (status IN ('active', 'superseded', 'rejected', 'resolved',
                              'needs_review', 'invalidated', 'conflicted')),
        subject           TEXT,
        body              TEXT NOT NULL,
        source_session_id TEXT,
        source_commit     TEXT,
        superseded_by     TEXT,
        created_at        INTEGER NOT NULL,
        updated_at        INTEGER NOT NULL,

        CHECK (superseded_by IS NULL OR superseded_by <> id),
        CHECK (superseded_by IS NULL OR status = 'superseded')
    );

    -- Normal retrieval is active, most recently updated first; the history
    -- search is the same index read with a different status.
    CREATE INDEX memories_by_status_updated
        ON memories (status, updated_at DESC);

    -- The project snapshot groups by kind within the active status.
    CREATE INDEX memories_by_kind_status
        ON memories (kind, status);

    -- Walking a supersession chain forwards, and finding what a given memory
    -- replaced. Partial, because most memories supersede nothing.
    CREATE INDEX memories_by_supersession
        ON memories (superseded_by)
        WHERE superseded_by IS NOT NULL;

    CREATE TRIGGER memories_reject_foreign_project_insert
    BEFORE INSERT ON memories
    FOR EACH ROW
    WHEN NEW.project_id IS NOT (
        SELECT value FROM project_metadata WHERE key = 'project_id'
    )
    BEGIN
        SELECT RAISE(ABORT, 'memory belongs to a different project');
    END;

    CREATE TRIGGER memories_reject_foreign_project_update
    BEFORE UPDATE OF project_id ON memories
    FOR EACH ROW
    WHEN NEW.project_id IS NOT (
        SELECT value FROM project_metadata WHERE key = 'project_id'
    )
    BEGIN
        SELECT RAISE(ABORT, 'memory belongs to a different project');
    END;

    CREATE TRIGGER memories_reject_unknown_supersession_insert
    BEFORE INSERT ON memories
    FOR EACH ROW
    WHEN NEW.superseded_by IS NOT NULL
     AND NOT EXISTS (SELECT 1 FROM memories WHERE id = NEW.superseded_by)
    BEGIN
        SELECT RAISE(ABORT, 'superseding memory does not exist');
    END;

    CREATE TRIGGER memories_reject_unknown_supersession_update
    BEFORE UPDATE OF superseded_by ON memories
    FOR EACH ROW
    WHEN NEW.superseded_by IS NOT NULL
     AND NOT EXISTS (SELECT 1 FROM memories WHERE id = NEW.superseded_by)
    BEGIN
        SELECT RAISE(ABORT, 'superseding memory does not exist');
    END;

    -- Phase 23's index. External content, so the text lives once in
    -- `memories` and the index holds only what BM25 needs; the three triggers
    -- below are what keeps the two in step, and are the documented way to
    -- drive an external-content FTS5 table.
    --
    -- `unicode61` with `remove_diacritics 2` is named rather than left to the
    -- default so the tokenizer cannot change under the index when the bundled
    -- SQLite moves.
    CREATE VIRTUAL TABLE memories_fts USING fts5(
        subject,
        body,
        content = 'memories',
        content_rowid = 'rowid',
        tokenize = 'unicode61 remove_diacritics 2'
    );

    CREATE TRIGGER memories_fts_after_insert
    AFTER INSERT ON memories
    BEGIN
        INSERT INTO memories_fts (rowid, subject, body)
        VALUES (NEW.rowid, NEW.subject, NEW.body);
    END;

    CREATE TRIGGER memories_fts_after_delete
    AFTER DELETE ON memories
    BEGIN
        INSERT INTO memories_fts (memories_fts, rowid, subject, body)
        VALUES ('delete', OLD.rowid, OLD.subject, OLD.body);
    END;

    CREATE TRIGGER memories_fts_after_update
    AFTER UPDATE ON memories
    BEGIN
        INSERT INTO memories_fts (memories_fts, rowid, subject, body)
        VALUES ('delete', OLD.rowid, OLD.subject, OLD.body);
        INSERT INTO memories_fts (rowid, subject, body)
        VALUES (NEW.rowid, NEW.subject, NEW.body);
    END;
    ",
    // 5: the append-only project event log (Phase 18) and portable session
    // checkpoints (Phase 19).
    //
    // # Why `lifecycle_events` refuses UPDATE and DELETE
    //
    // Phase 18's fixed architectural requirement is that derived
    // interpretation must not overwrite or masquerade as the original event.
    // Two triggers enforce that against anything that opens this file, which
    // is a different kind of promise from a rule every future query has to
    // remember — the same argument migration 2 makes for project isolation.
    //
    // The cost is real and is stated rather than hidden: **nothing can prune
    // this table.** Retention is then a migration and a decision, not a
    // `DELETE` somebody adds one afternoon.
    //
    // # Why the raw observation gets its own two columns
    //
    // The same requirement asks that raw observations stay available as
    // diagnostic source evidence while normalized records remain
    // distinguishable from them. `kind` and its payload columns are
    // Glasshouse's normalized reading; `observed_harness` and
    // `observed_event` are the harness's own two words. Neither can be
    // mistaken for the other, and an event Glasshouse observed itself — a
    // process exiting — simply has NULL there.
    //
    // **There is deliberately no column a conversation could reach.** A hook
    // payload carries the user's prompt and the model's last message; the
    // handler drains that stream unread, and the only fields that travel this
    // far are an integration slug and an event name. `RawObservation`'s
    // `detail` — the one field an adapter could fill from a payload — has no
    // column, so no future writer can persist one without a migration.
    //
    // # No `REFERENCES sessions(id)`, on purpose
    //
    // `PRAGMA foreign_keys` is off by default in SQLite, so the clause would
    // be decoration unless every connection remembered to turn it on — the
    // reason migration 4 uses triggers for supersession. And a foreign key
    // here would be the wrong shape regardless: an event that arrives for a
    // session this database has never heard of is a fact worth keeping, and
    // refusing it would make the log lie by omission at exactly the moment
    // something is wrong.
    //
    // # `checkpoints` is a separate table from `memories`, which is the point
    //
    // Phase 19 requires checkpoints to be stored separately from durable
    // project memory. They are different things with different lifetimes: a
    // checkpoint is bounded handoff context for one session, and a memory is
    // durable project knowledge. The `CHECK` on the document's byte length is
    // Phase 19's size constraint made structural — `length(CAST(x AS BLOB))`
    // rather than `length(x)`, which counts characters and would let a
    // checkpoint full of non-ASCII past a byte bound.
    //
    // **`document` is the checkpoint; the columns beside it are an index.**
    // Only the three a query actually needs are lifted out, and every one of
    // them is written from the document in one place, so there is nothing for
    // the row and the document to drift about — see
    // `a_stored_row_never_disagrees_with_its_own_document`. The harness and
    // the Git position stay inside the document alone for exactly that
    // reason: nothing queries on them, so a second copy would be a liability
    // with no use.
    "
    CREATE TABLE lifecycle_events (
        seq              INTEGER PRIMARY KEY AUTOINCREMENT,
        project_id       TEXT    NOT NULL,
        session_id       TEXT    NOT NULL,
        at               INTEGER NOT NULL,
        kind             TEXT    NOT NULL
            CHECK (kind IN ('session_started', 'session_resumed',
                            'turn_started', 'turn_ended',
                            'waiting_for_user', 'text_delivered',
                            'interrupt_delivered', 'process_exited',
                            'output_ended', 'gateway_unhealthy')),

        -- Variant payloads, each NULL for the kinds that do not carry them.
        turn_outcome     TEXT
            CHECK (turn_outcome IS NULL OR
                   turn_outcome IN ('completed', 'failed')),
        origin           TEXT
            CHECK (origin IS NULL OR
                   origin IN ('user_keystroke', 'machine')),
        bytes            INTEGER,
        exit_code        INTEGER,
        exit_signal      TEXT,
        resource         TEXT,
        gateway_reason   TEXT
            CHECK (gateway_reason IS NULL OR
                   gateway_reason IN ('unreachable', 'timed_out', 'rejected')),

        -- The harness report this was translated from, when it was translated
        -- from one. Both or neither.
        observed_harness TEXT,
        observed_event   TEXT,
        CHECK ((observed_harness IS NULL) = (observed_event IS NULL))
    );

    CREATE INDEX lifecycle_events_by_session
        ON lifecycle_events (session_id, seq);

    CREATE TRIGGER lifecycle_events_reject_foreign_project_insert
    BEFORE INSERT ON lifecycle_events
    FOR EACH ROW
    WHEN NEW.project_id IS NOT (
        SELECT value FROM project_metadata WHERE key = 'project_id'
    )
    BEGIN
        SELECT RAISE(ABORT, 'event belongs to a different project');
    END;

    CREATE TRIGGER lifecycle_events_are_append_only_update
    BEFORE UPDATE ON lifecycle_events
    FOR EACH ROW
    BEGIN
        SELECT RAISE(ABORT, 'the project event log is append-only');
    END;

    CREATE TRIGGER lifecycle_events_are_append_only_delete
    BEFORE DELETE ON lifecycle_events
    FOR EACH ROW
    BEGIN
        SELECT RAISE(ABORT, 'the project event log is append-only');
    END;

    CREATE TABLE checkpoints (
        id           TEXT PRIMARY KEY,
        project_id   TEXT NOT NULL,
        session_id   TEXT NOT NULL,
        created_at   INTEGER NOT NULL,
        reason       TEXT NOT NULL
            CHECK (reason IN ('manual', 'task_boundary')),
        document     TEXT NOT NULL
            CHECK (length(CAST(document AS BLOB)) <= 8192)
    ) WITHOUT ROWID;

    CREATE INDEX checkpoints_by_session
        ON checkpoints (session_id, created_at DESC);

    CREATE TRIGGER checkpoints_reject_foreign_project_insert
    BEFORE INSERT ON checkpoints
    FOR EACH ROW
    WHEN NEW.project_id IS NOT (
        SELECT value FROM project_metadata WHERE key = 'project_id'
    )
    BEGIN
        SELECT RAISE(ABORT, 'checkpoint belongs to a different project');
    END;

    CREATE TRIGGER checkpoints_reject_foreign_project_update
    BEFORE UPDATE OF project_id ON checkpoints
    FOR EACH ROW
    WHEN NEW.project_id IS NOT (
        SELECT value FROM project_metadata WHERE key = 'project_id'
    )
    BEGIN
        SELECT RAISE(ABORT, 'checkpoint belongs to a different project');
    END;
    ",
    // 6: where a memory came from, and why the decision in it was made.
    //
    // # Two integers, because extraction reads a slice
    //
    // Phase 21's *"store the originating session and event references so
    // extracted memory retains provenance"*. `source_session_id` has been
    // here since migration 4; what was missing is *which part* of that
    // session. Extraction is fed a bounded chunk of the project event log,
    // so the honest reference is the range of `lifecycle_events.seq` that
    // chunk covered — a memory is rarely traceable to one event, and naming
    // a single one would be a precision the producer does not have.
    //
    // Nullable, and both-or-neither: a hand-written memory with **no** event
    // range is a different fact from one with an empty range, and the two
    // triggers below are what stop a half-filled range being stored at all.
    // The same argument migration 5 makes for `observed_harness` /
    // `observed_event`.
    //
    // # Phase 21B, one column per line of the map
    //
    // `rationale` is the one that already had a home: until this migration
    // the extractor folded it into `body` behind a marker so that it stayed
    // in the FTS index. That fold is removed with this migration and the
    // index is rebuilt over the new column, so nothing that used to be
    // findable stops being findable — see the rebuild below.
    //
    // The eight beside it are the assumptions and references Phase 21B asks
    // to be preserved so that a remembered decision can be revalidated later
    // rather than obeyed forever. They are deliberately **flat, concise, and
    // nullable** rather than a related table: each holds one sentence, NULL
    // means "not known" and never "none", and a decision that recorded no
    // security assumption is thereby distinguishable from one that recorded
    // that security was not a factor.
    //
    // `project_phase` is the only one of them drawn from a fixed set, so it
    // is the only one with a `CHECK`; SQLite accepts a column `CHECK` in
    // `ADD COLUMN` as long as it admits NULL, which every existing row is.
    //
    // # What these columns can hold, asked one at a time
    //
    // `rationale`, `problem`, the five assumption columns, `evidence` and
    // `source_excerpt` are **free text, and free text can hold a
    // credential** — exactly like `subject` and `body`, and unlike the
    // nineteen fixed-vocabulary columns migration 5 added.
    // `source_excerpt` is the sharpest of them, because it is verbatim
    // session text rather than a model's paraphrase. Nothing in this schema
    // can stop that, and this migration does not pretend otherwise: the
    // control is on the producer side, where `memory::extract::chunk`
    // scrubs everything on the way in and `memory::extract::schema::judge`
    // screens each emitted element **whole, before any field of it is
    // read** — which is what makes coverage of a new field automatic rather
    // than a rule someone has to remember. See
    // `the_project_database_schema_has_nowhere_to_put_a_credential`, which
    // records the same judgement for migrations 4 and 5.
    //
    // # The FTS5 index is rebuilt, not altered
    //
    // `memories_fts` is an external-content index over `subject` and `body`.
    // There is no `ALTER` that adds a column to an FTS5 table, so making the
    // rationale searchable means dropping the index and its three triggers,
    // recreating both over three columns, and asking FTS5 to rebuild itself
    // from `memories`. The shadow tables go with the `DROP TABLE`.
    //
    // **Only `rationale` joins the index.** The other eight provenance
    // columns are attributes of a decision somebody has already found, not
    // the words they would search for, and every indexed column costs index
    // size and shifts BM25's weighting of the ones that matter. The
    // rationale is different only because it was inside `body` yesterday:
    // indexing it keeps every search that worked before this migration
    // working after it.
    //
    // **Existing folded bodies are left alone.** A body ending in the old
    // marker is still a correct memory and is still indexed; splitting it
    // automatically would mean guessing which occurrence of the marker was
    // the fold, in text a person may have edited, for rows this project has
    // never shipped a way to create automatically. The fold is gone from
    // the producer, not retroactively from the store.
    "
    ALTER TABLE memories ADD COLUMN source_event_first INTEGER;
    ALTER TABLE memories ADD COLUMN source_event_last  INTEGER;

    ALTER TABLE memories ADD COLUMN rationale                 TEXT;
    ALTER TABLE memories ADD COLUMN project_phase             TEXT
        CHECK (project_phase IS NULL OR project_phase IN
               ('prototype', 'alpha', 'beta', 'production', 'migration'));
    ALTER TABLE memories ADD COLUMN problem                   TEXT;
    ALTER TABLE memories ADD COLUMN assumptions               TEXT;
    ALTER TABLE memories ADD COLUMN scale_assumptions         TEXT;
    ALTER TABLE memories ADD COLUMN security_assumptions      TEXT;
    ALTER TABLE memories ADD COLUMN compatibility_assumptions TEXT;
    ALTER TABLE memories ADD COLUMN operational_assumptions   TEXT;
    ALTER TABLE memories ADD COLUMN evidence                  TEXT;
    ALTER TABLE memories ADD COLUMN source_excerpt            TEXT;

    -- Everything one session contributed, in the order it was learned.
    -- Partial, because a memory nobody extracted has no session to group by.
    CREATE INDEX memories_by_source_session
        ON memories (source_session_id, source_event_first)
        WHERE source_session_id IS NOT NULL;

    CREATE TRIGGER memories_reject_broken_event_range_insert
    BEFORE INSERT ON memories
    FOR EACH ROW
    WHEN (NEW.source_event_first IS NULL) <> (NEW.source_event_last IS NULL)
      OR (NEW.source_event_first IS NOT NULL
          AND NEW.source_event_first > NEW.source_event_last)
    BEGIN
        SELECT RAISE(ABORT, 'a memory names both ends of its source event range or neither');
    END;

    CREATE TRIGGER memories_reject_broken_event_range_update
    BEFORE UPDATE OF source_event_first, source_event_last ON memories
    FOR EACH ROW
    WHEN (NEW.source_event_first IS NULL) <> (NEW.source_event_last IS NULL)
      OR (NEW.source_event_first IS NOT NULL
          AND NEW.source_event_first > NEW.source_event_last)
    BEGIN
        SELECT RAISE(ABORT, 'a memory names both ends of its source event range or neither');
    END;

    DROP TRIGGER memories_fts_after_insert;
    DROP TRIGGER memories_fts_after_delete;
    DROP TRIGGER memories_fts_after_update;
    DROP TABLE memories_fts;

    CREATE VIRTUAL TABLE memories_fts USING fts5(
        subject,
        body,
        rationale,
        content = 'memories',
        content_rowid = 'rowid',
        tokenize = 'unicode61 remove_diacritics 2'
    );

    INSERT INTO memories_fts (memories_fts) VALUES ('rebuild');

    CREATE TRIGGER memories_fts_after_insert
    AFTER INSERT ON memories
    BEGIN
        INSERT INTO memories_fts (rowid, subject, body, rationale)
        VALUES (NEW.rowid, NEW.subject, NEW.body, NEW.rationale);
    END;

    CREATE TRIGGER memories_fts_after_delete
    AFTER DELETE ON memories
    BEGIN
        INSERT INTO memories_fts (memories_fts, rowid, subject, body, rationale)
        VALUES ('delete', OLD.rowid, OLD.subject, OLD.body, OLD.rationale);
    END;

    CREATE TRIGGER memories_fts_after_update
    AFTER UPDATE ON memories
    BEGIN
        INSERT INTO memories_fts (memories_fts, rowid, subject, body, rationale)
        VALUES ('delete', OLD.rowid, OLD.subject, OLD.body, OLD.rationale);
        INSERT INTO memories_fts (rowid, subject, body, rationale)
        VALUES (NEW.rowid, NEW.subject, NEW.body, NEW.rationale);
    END;
    ",
    // 7: `gateway_backend_changed` — Phase 9H's durable record of failover
    // changing the provider or model serving a live session.
    //
    // # Why this rebuilds the table instead of altering its `CHECK`
    //
    // SQLite cannot add or drop a `CHECK` constraint. Migration 5's `kind`
    // column is one, so admitting an eleventh value means rename, recreate,
    // copy, drop, then recreate the index and all three triggers — the same
    // cost migration 6 paid to add a column FTS5 could not `ALTER` in.
    //
    // # Why `seq` must survive this rebuild unchanged
    //
    // `lifecycle_events.seq` is `INTEGER PRIMARY KEY AUTOINCREMENT`, and
    // migration 6 made `memories.source_event_first` and
    // `memories.source_event_last` reference it. A rebuild that let `seq`
    // renumber would silently re-point every extracted memory's provenance
    // at the wrong events — nothing would fail, the data would just be
    // wrong. So the copy below names `seq` explicitly in both the column
    // list and the `SELECT`, rather than letting the new table's own
    // `AUTOINCREMENT` assign fresh values, and the old table is dropped only
    // after the copy has landed. SQLite's own `sqlite_sequence` bookkeeping
    // follows an explicit-valued insert exactly as it follows a generated
    // one, so the next event appended after this migration continues from
    // the old table's highest `seq` rather than restarting at it.
    // `a_memorys_provenance_survives_the_seq_rebuild` in
    // `tests/events_lifecycle.rs` is the proof, exercised against a
    // deliberately naive rebuild that lets `seq` renumber before this one
    // was written.
    //
    // # The three new columns
    //
    // `provider`, `model` and `cause` are names only, never a credential —
    // the same Phase 9 acceptance condition every other free-text column in
    // this schema already meets. They are prefixed `gateway_` to keep them
    // visually grouped with `gateway_reason` beside them, and because a bare
    // `model` column beside `resource` would read as naming the same thing
    // `gateway_unhealthy` already names with `resource`, when it does not.
    "
    CREATE TABLE lifecycle_events_new (
        seq              INTEGER PRIMARY KEY AUTOINCREMENT,
        project_id       TEXT    NOT NULL,
        session_id       TEXT    NOT NULL,
        at               INTEGER NOT NULL,
        kind             TEXT    NOT NULL
            CHECK (kind IN ('session_started', 'session_resumed',
                            'turn_started', 'turn_ended',
                            'waiting_for_user', 'text_delivered',
                            'interrupt_delivered', 'process_exited',
                            'output_ended', 'gateway_unhealthy',
                            'gateway_backend_changed')),

        -- Variant payloads, each NULL for the kinds that do not carry them.
        turn_outcome     TEXT
            CHECK (turn_outcome IS NULL OR
                   turn_outcome IN ('completed', 'failed')),
        origin           TEXT
            CHECK (origin IS NULL OR
                   origin IN ('user_keystroke', 'machine')),
        bytes            INTEGER,
        exit_code        INTEGER,
        exit_signal      TEXT,
        resource         TEXT,
        gateway_reason   TEXT
            CHECK (gateway_reason IS NULL OR
                   gateway_reason IN ('unreachable', 'timed_out', 'rejected')),
        gateway_provider TEXT,
        gateway_model    TEXT,
        gateway_cause    TEXT,

        -- The harness report this was translated from, when it was translated
        -- from one. Both or neither.
        observed_harness TEXT,
        observed_event   TEXT,
        CHECK ((observed_harness IS NULL) = (observed_event IS NULL))
    );

    INSERT INTO lifecycle_events_new (
        seq, project_id, session_id, at, kind,
        turn_outcome, origin, bytes, exit_code, exit_signal,
        resource, gateway_reason, observed_harness, observed_event
    )
    SELECT
        seq, project_id, session_id, at, kind,
        turn_outcome, origin, bytes, exit_code, exit_signal,
        resource, gateway_reason, observed_harness, observed_event
    FROM lifecycle_events;

    DROP TABLE lifecycle_events;
    ALTER TABLE lifecycle_events_new RENAME TO lifecycle_events;

    CREATE INDEX lifecycle_events_by_session
        ON lifecycle_events (session_id, seq);

    CREATE TRIGGER lifecycle_events_reject_foreign_project_insert
    BEFORE INSERT ON lifecycle_events
    FOR EACH ROW
    WHEN NEW.project_id IS NOT (
        SELECT value FROM project_metadata WHERE key = 'project_id'
    )
    BEGIN
        SELECT RAISE(ABORT, 'event belongs to a different project');
    END;

    CREATE TRIGGER lifecycle_events_are_append_only_update
    BEFORE UPDATE ON lifecycle_events
    FOR EACH ROW
    BEGIN
        SELECT RAISE(ABORT, 'the project event log is append-only');
    END;

    CREATE TRIGGER lifecycle_events_are_append_only_delete
    BEFORE DELETE ON lifecycle_events
    FOR EACH ROW
    BEGIN
        SELECT RAISE(ABORT, 'the project event log is append-only');
    END;
    ",
    // 8: the rest of Phase 10 line 645 — *"store the harness, launch profile,
    // backend resource, model, pairing class, protocol, and response profile
    // as distinct session metadata"* — and the two labels lines 650 and 651
    // give the user.
    //
    // # Seven columns, and why not fewer
    //
    // The phase's second fixed architectural requirement is that these things
    // *"remain separately represented rather than collapsed into one
    // ambiguous agent identifier"*. A column each is what that means at the
    // storage layer, and the Rust side carries it further: each one reads
    // back as its own type, so a build that assigned the pairing class from
    // the launch profile would not compile. See `session::store`.
    //
    // # `ALTER TABLE ADD COLUMN`, never a rebuild
    //
    // Migration 3 is the shape: append a column, leave every existing row
    // alone. A rewrite would be refused here for migration 7's reason —
    // rebuilding a table risks the data that already lives in it — and none
    // of these needs one, because none of them adds or drops a constraint on
    // a column that already exists.
    //
    // # What NULL means, and what it must never be allowed to mean
    //
    // NULL is *"the build that wrote this row recorded nothing here"*, exactly
    // as it is for `launch_profile`. That is why `model` does not simply hold
    // a model id: *"Glasshouse assigned no model, so the harness chose"* is a
    // real recorded answer and a different fact from *"this was never
    // recorded"*, and a bare id column would have had one slot for both. So
    // the column holds `harness-default` or `named:<id>`, which cannot
    // collide however a model is named. `pairing_class` and `protocol` have
    // the same problem and already have their own words for it: `unknown` is
    // a recorded answer, NULL is not an answer at all.
    //
    // # The `CHECK`s copy three vocabularies, on purpose
    //
    // `pairing_class`, `protocol` and `response_mechanism` are owned by
    // `harness::pairing`, `harness` and `harness::response` respectively, so
    // the lists below are second copies and could drift. They are here for
    // migration 2's reason — a future writer must not be able to store a
    // value readers would have to guess about — and the drift is answered the
    // way `LIFECYCLE_EVENT_KINDS` answers it: `session::store` encodes each
    // one through an exhaustive `match` (so a new variant is a compile error
    // there) and `every_stored_vocabulary_is_one_the_schema_accepts` inserts
    // every variant through the schema.
    //
    // `response_profile` gets no `CHECK`. It is five axes joined, not one
    // word, and pinning 4 x 3 x 3 x 3 x 3 combinations in SQL would be a
    // vocabulary this file has no business holding. An encoding the reader
    // does not recognise is reported as `SessionStoreError::UnknownValue`
    // rather than guessed at, which is the same visible-degradation rule the
    // enum columns follow when a row arrives from a newer build.
    "
    ALTER TABLE sessions ADD COLUMN model TEXT
        CHECK (model IS NULL
               OR model = 'harness-default'
               OR (substr(model, 1, 6) = 'named:' AND length(model) > 6));

    ALTER TABLE sessions ADD COLUMN pairing_class TEXT
        CHECK (pairing_class IS NULL
               OR pairing_class IN ('vendor-native', 'vendor-supported',
                                    'protocol-native', 'protocol-compatible',
                                    'protocol-translated', 'unknown'));

    ALTER TABLE sessions ADD COLUMN protocol TEXT
        CHECK (protocol IS NULL
               OR protocol IN ('anthropic-messages', 'openai-responses',
                               'openai-chat', 'unknown'));

    ALTER TABLE sessions ADD COLUMN response_profile TEXT;

    ALTER TABLE sessions ADD COLUMN response_mechanism TEXT
        CHECK (response_mechanism IS NULL
               OR response_mechanism IN ('native', 'additive', 'none'));

    -- A name a person gave this session. Never the native session
    -- identifier, which lives in its own column and which renaming does not
    -- touch -- line 650.
    ALTER TABLE sessions ADD COLUMN display_name TEXT
        CHECK (display_name IS NULL
               OR (display_name <> '' AND length(display_name) <= 64));

    -- A lightweight purpose such as auth, tests, or research -- line 651.
    -- Free text rather than an enumeration: the map says such as, so the
    -- three it names are examples, and a fixed list would refuse the fourth
    -- thing a user actually does.
    ALTER TABLE sessions ADD COLUMN purpose TEXT
        CHECK (purpose IS NULL
               OR (purpose <> '' AND length(purpose) <= 32));
    ",
    // 9: Phase 10A — the durable process identity a session is supervised by,
    // and what supervision has concluded about it.
    //
    // # Why a process id is not an identity
    //
    // Operating systems reuse process ids. A record holding `4711` alone will
    // eventually match a stranger that happens to be `4711` today, and a
    // control plane that trusted it would report someone else's process as
    // this project's session — or, worse, refuse to start a session because a
    // text editor is sitting on the number. `process_started_at` is what makes
    // the pair an identity: the kernel's own start time for that process, in
    // milliseconds since the Unix epoch, which no later process can inherit.
    //
    // Milliseconds since the epoch, rather than each platform's native unit,
    // for one reason: Linux reports a process's start time in clock ticks
    // *since boot*, which repeats after every reboot, so storing it raw would
    // leave the same collision this column exists to close. `session::
    // supervision` converts on the way in — see its `observe`.
    //
    // `process_host` is the third part. A project directory can be shared or
    // synchronised between machines, and a process id from another host means
    // nothing here. A record whose host is not this one is never verified and
    // never assumed dead; it is reported as unverifiable, which is the second
    // architectural requirement of this phase applied to a case that has
    // nothing to do with processes dying.
    //
    // # Why supervision is recorded rather than recomputed each time
    //
    // Quarantine is a conclusion about a process that was observed at a
    // particular moment. The next Glasshouse to open this database may not be
    // able to observe the same thing — the process may have gone in between —
    // and "there was something alive here that I could not account for" must
    // survive that. `supervision_reason` carries the sentence a person needs,
    // because "quarantined" on its own tells nobody what was seen.
    //
    // # NULL, here as everywhere in this schema
    //
    // NULL is *"the build that wrote this row recorded nothing here"*, never a
    // default. A session recorded before this migration has no process
    // identity, and supervision must therefore refuse to conclude anything
    // about it rather than treating it as stopped — see
    // `session::supervision::Verdict::Unrecorded`.
    //
    // # `ALTER TABLE ADD COLUMN`, and nothing else
    //
    // Migration 3's shape, for migration 8's reasons. No table is rebuilt, no
    // existing `CHECK` is altered — SQLite cannot alter one — and no existing
    // row is touched. In particular `lifecycle_events` is left alone: its
    // `seq` is `AUTOINCREMENT` and `memories` references it, so a supervision
    // conclusion is a column on `sessions` and never a new event kind.
    "
    ALTER TABLE sessions ADD COLUMN process_id INTEGER
        CHECK (process_id IS NULL OR process_id > 0);

    ALTER TABLE sessions ADD COLUMN process_started_at INTEGER
        CHECK (process_started_at IS NULL OR process_started_at >= 0);

    ALTER TABLE sessions ADD COLUMN process_host TEXT
        CHECK (process_host IS NULL OR process_host <> '');

    -- What supervision concluded, in the vocabulary `session::supervision`
    -- encodes through an exhaustive match. `owned` is this Glasshouse's own
    -- session; `adopted` is one it verified alive and took back rather than
    -- starting a second beside it; `quarantined` is alive and unaccounted
    -- for; `lost` is a recorded process that is no longer running.
    ALTER TABLE sessions ADD COLUMN supervision TEXT
        CHECK (supervision IS NULL
               OR supervision IN ('owned', 'adopted', 'quarantined', 'lost'));

    -- Why. A quarantine with no stated reason is an accusation, and boxes 9
    -- and 10 of this phase both ask for a stated reason outright.
    ALTER TABLE sessions ADD COLUMN supervision_reason TEXT
        CHECK (supervision_reason IS NULL OR supervision_reason <> '');
    ",
    // 10: Phase 21C's validity and invalidation conditions, and the
    // review/decay bookkeeping Phase 21D needs.
    //
    // # `ADD COLUMN` only, for migration 8's reasons
    //
    // No table is rebuilt, no existing `CHECK` is altered, no existing row is
    // touched. In particular `memories_fts` is left alone: none of these five
    // columns joins it. They are attributes of a memory somebody has already
    // found — a validity condition, why it was flagged, when, and whether it
    // has since been rechecked — not words a search would match on, and every
    // indexed column shifts BM25's weighting of the ones that matter. Making
    // one searchable later is the same rebuild migration 6 paid for
    // `rationale`; nothing here asks for it.
    //
    // # `validity_conditions` and `invalidation_conditions`
    //
    // Phase 21C's *"allow a durable memory to define explicit validity [or
    // invalidation] conditions when known"*. Free text, like the Phase 21B
    // provenance columns beside them, and for the same reason: a condition is
    // a sentence someone wrote down, not a value from a fixed vocabulary, and
    // `NULL` means "no condition was recorded" rather than "none apply."
    //
    // # `review_reason`, one value per map line
    //
    // The six values are lines 885-890 of the capability map, in order, and
    // this `CHECK` is their only definition — [`crate::memory::ReviewReason`]
    // reads it back the way `every_project_phase_the_type_supports_is_one_
    // the_schema_accepts` reads migration 6's, so the two cannot silently
    // drift apart.
    //
    // # `review_marked_at` and `last_validated_at`: `NULL` is "unknown," never zero
    //
    // The same argument every other nullable timestamp in this schema makes,
    // sharpened by Phase 21D line 898: a memory written before this migration
    // has no `last_validated_at`, and the decay policy must treat that as
    // *unknown* — never reaffirmed, not yet due for one, no basis to prefer it
    // over a memory that has one — rather than as *never validated as of
    // epoch zero*, which would make every pre-migration memory look infinitely
    // stale the instant this migration runs. `review_marked_at` carries the
    // same distinction for the same reason: a memory nobody has flagged has no
    // answer to "when," not an answer of zero.
    "
    ALTER TABLE memories ADD COLUMN validity_conditions     TEXT;
    ALTER TABLE memories ADD COLUMN invalidation_conditions TEXT;

    ALTER TABLE memories ADD COLUMN review_reason TEXT
        CHECK (review_reason IS NULL OR review_reason IN
               ('project_state', 'project_phase_change', 'production_incident',
                'benchmark_or_scale', 'security_requirement',
                'architecture_drift'));

    ALTER TABLE memories ADD COLUMN review_marked_at INTEGER
        CHECK (review_marked_at IS NULL OR review_marked_at >= 0);

    ALTER TABLE memories ADD COLUMN last_validated_at INTEGER
        CHECK (last_validated_at IS NULL OR last_validated_at >= 0);
    ",
    // 11: Phase 33A's routing evidence ledger — an append-oriented record of
    // what actually happened on a routed turn, so a routing decision can be
    // audited and its aggregation recalibrated against the raw rows rather
    // than a counter that has already forgotten what produced it.
    //
    // # A new table, not more columns on `sessions`
    //
    // `sessions` is one row per session and this is many rows per session —
    // every measurable turn a session makes, at whatever rate its harness
    // makes them. Folding that into `sessions` would mean either widening one
    // row's meaning to "the latest turn" (losing every one before it, exactly
    // what line 1329 forbids) or a one-to-many column nothing else in this
    // schema does. A dedicated table with its own `seq` is migration 4's own
    // argument for `lifecycle_events` over a column on `sessions`, applied
    // here for the same reason.
    //
    // # `AUTOINCREMENT`, and no `UPDATE` path
    //
    // Append-oriented is a property of the code as much as the schema: this
    // migration adds no trigger that would let a later migration alter a
    // measurement in place, and [`crate::routing::evidence`]'s store offers a
    // `record` method and reads, never a method that edits a recorded
    // observation. `AUTOINCREMENT` (migration 4's own reasoning for
    // `lifecycle_events` and `memories`) means a `seq` is never reused even
    // after rows are pruned by some future retention policy, so a stored
    // reference to one observation can never come to mean another.
    //
    // # Identity: six columns, because two turns are the same evidence only
    // when all of them agree
    //
    // `provider`, `model` and `route` are line 1338's "materially different
    // model versions, quantizations, routes, or changing stealth-model
    // identities" kept apart rather than averaged together; `harness` and
    // `purpose` are line 1330's own list; `quota_context` is the authenticated
    // credential or account context a reading is scoped to, so two credentials
    // against the same provider are never folded into one rate. All nullable
    // except `provider` and `model`, because a row this schema will accept
    // must at minimum say which provider and which model it is evidence
    // about — see [`crate::routing::evidence`]'s own doc comment for which of
    // these a real gateway exchange can actually supply today.
    //
    // # Timing, tokens, cost: nullable, every one, for the reason line 1331
    // gives
    //
    // "When the protocol exposes them." A gateway that forwards bytes without
    // parsing them cannot see inside a response stream, so
    // `first_token_at`/`first_tool_call_at` are NULL from that producer today
    // — not zero, not the dispatch time, `NULL`, which is this schema's
    // standing rule for "the build that wrote this row recorded nothing
    // here." The same is true of the token and cost columns: nothing in this
    // migration invents a way to read them, it only makes room for a producer
    // that can. `cost_confidence`'s `CHECK` is paired with `cost_micro_usd` so
    // that a cost can never be stored without saying how well it is known —
    // line 1333's "explicit confidence label" enforced at the storage layer
    // rather than left to a caller's discipline, the same move migration 6
    // makes for `project_phase` and migration 10 for `review_reason`.
    //
    // # `context_state` is the one column that is `NOT NULL`
    //
    // Every other column's NULL means "not recorded." This one may not be
    // silently absent, because line 1337 forbids *averaging away* cache
    // effects: a row that does not know whether its context was warm or cold
    // must say `unknown` outright, so that a rolling summary can separate the
    // three rather than one of them quietly vanishing into the others.
    // `DEFAULT 'unknown'` is what makes that automatic for any future insert
    // path that forgets to think about it.
    //
    // # Two triggers, migration 4's pair, unchanged
    //
    // `IS NOT` rather than `<>`, so a missing binding row aborts the write
    // instead of the comparison evaluating to NULL and letting it through —
    // migration 2's argument, copied verbatim rather than re-derived. This is
    // the structural half of line 1343's "keep the evidence ledger physically
    // project-scoped"; the second half — "require explicit export before
    // observations leave the project" — is a property of which functions
    // exist in [`crate::routing::evidence`], not of the schema, and is
    // recorded there.
    "
    CREATE TABLE routing_observations (
        seq                INTEGER PRIMARY KEY AUTOINCREMENT,
        project_id         TEXT    NOT NULL,
        observed_at        INTEGER NOT NULL,

        provider           TEXT    NOT NULL,
        model              TEXT    NOT NULL,
        route              TEXT,
        quota_context      TEXT,
        harness            TEXT,
        purpose            TEXT,

        dispatched_at      INTEGER,
        first_byte_at      INTEGER,
        first_token_at     INTEGER,
        first_tool_call_at INTEGER,
        completed_at       INTEGER,

        input_tokens        INTEGER CHECK (input_tokens        IS NULL OR input_tokens        >= 0),
        output_tokens       INTEGER CHECK (output_tokens       IS NULL OR output_tokens       >= 0),
        cached_input_tokens INTEGER CHECK (cached_input_tokens IS NULL OR cached_input_tokens >= 0),
        cost_micro_usd      INTEGER CHECK (cost_micro_usd      IS NULL OR cost_micro_usd       >= 0),
        cost_confidence     TEXT
            CHECK (cost_confidence IS NULL
                   OR cost_confidence IN ('exact', 'estimated', 'unknown')),

        tool_rounds        INTEGER CHECK (tool_rounds IS NULL OR tool_rounds >= 0),
        retries            INTEGER CHECK (retries     IS NULL OR retries     >= 0),
        repairs            INTEGER CHECK (repairs     IS NULL OR repairs     >= 0),
        failovers          INTEGER CHECK (failovers   IS NULL OR failovers   >= 0),
        outcome            TEXT
            CHECK (outcome IS NULL
                   OR outcome IN ('succeeded', 'failed', 'cancelled', 'unknown')),

        context_state      TEXT NOT NULL DEFAULT 'unknown'
            CHECK (context_state IN ('warm', 'cold', 'unknown')),

        CHECK (cost_micro_usd IS NULL OR cost_confidence IS NOT NULL)
    );

    -- Reading back one route's recent observations, newest first: the access
    -- pattern every rolling summary in `crate::routing::evidence` uses.
    CREATE INDEX routing_observations_by_route_time
        ON routing_observations (provider, model, route, observed_at DESC);

    CREATE TRIGGER routing_observations_reject_foreign_project_insert
    BEFORE INSERT ON routing_observations
    FOR EACH ROW
    WHEN NEW.project_id IS NOT (
        SELECT value FROM project_metadata WHERE key = 'project_id'
    )
    BEGIN
        SELECT RAISE(ABORT, 'routing observation belongs to a different project');
    END;

    CREATE TRIGGER routing_observations_reject_foreign_project_update
    BEFORE UPDATE OF project_id ON routing_observations
    FOR EACH ROW
    WHEN NEW.project_id IS NOT (
        SELECT value FROM project_metadata WHERE key = 'project_id'
    )
    BEGIN
        SELECT RAISE(ABORT, 'routing observation belongs to a different project');
    END;
    ",
    // 12: Phase 40 line 1646 — which session, if any, a session was
    // bootstrapped from.
    //
    // # `ALTER TABLE ADD COLUMN`, migration 3's shape, for migration 8's
    // reasons
    //
    // No table is rebuilt, no existing `CHECK` is altered, no existing row is
    // touched.
    //
    // # No `CHECK`, and no foreign key
    //
    // Unlike `display_name` (migration 8), this column holds a `SessionId`,
    // not user text, so there is no length or emptiness to police. Unlike a
    // relational id, it names no `REFERENCES`: a source session can be in
    // another project (this column does not resolve across the project
    // boundary — see `session::store`), can already be gone, and the
    // precedent this follows, `memories.source_session_id` (migration 6), is
    // itself a bare nullable `TEXT` with no foreign key.
    //
    // # NULL, here as everywhere in this schema
    //
    // NULL is *"this session was not started from a checkpoint,"* never a
    // placeholder value. A session recorded before this migration, and any
    // session started without `--from-checkpoint`, has no source and must
    // read back as `None` rather than some invented default.
    //
    // # One direction only
    //
    // This column answers "what did this session come from." It
    // deliberately does not add an index, a reverse table, or a descendants
    // column: `SessionStore::list()` already enumerates every session in the
    // project with no required key, so "what came from this session" is a
    // filter over an existing enumeration, not a missing capability.
    "
    ALTER TABLE sessions ADD COLUMN source_session_id TEXT;
    ",
    // 13: capability map line 925 — "record why a decision was superseded so
    // future agents do not resurrect it without context."
    //
    // # `ALTER TABLE ADD COLUMN`, migration 12's shape
    //
    // No table is rebuilt, no existing `CHECK` is altered, no existing row is
    // touched. Every memory recorded before this migration reads back with no
    // supersession reason, which is the truth about it.
    //
    // # Why not `review_reason`
    //
    // `review_reason` is a six-value enumeration meaning *why this memory
    // needs review*, constrained by migration 10's own `CHECK`. "Why it was
    // superseded" is a different question with a different answer type — a
    // person's sentence, not a vocabulary — and reusing the column would
    // either need that `CHECK` widened, which this file's own house rule
    // forbids doing in place, or would silently store a value readers of
    // `review_reason` would have to guess about. Adding a column is neither.
    //
    // # No `CHECK` tying it to `status`
    //
    // `superseded_by` has one — `CHECK (superseded_by IS NULL OR status =
    // 'superseded')` — and it is a **table** constraint on the original
    // `CREATE TABLE`. `ALTER TABLE ADD COLUMN` cannot add a table constraint,
    // and rebuilding `memories` to gain one would risk the data already in it
    // for a rule the store already enforces: `MemoryStore::set_status` clears
    // this column in the same expression it clears `superseded_by`, so the two
    // cannot drift apart through any door this binary has.
    //
    // # The `CHECK` it does get
    //
    // Migration 8's shape for operator free text: not empty, and bounded.
    // Empty is refused because `--reason ""` must not read back as *"a reason
    // was recorded"* — the store maps it to NULL before it ever gets here, and
    // this is the constraint that makes that a property of the data rather
    // than of one caller remembering. The bound is 512 rather than
    // `display_name`'s 64: this is a sentence explaining a decision, not a
    // label, and the whole point of the line is that it carries enough context
    // to stop a resurrection.
    //
    // # NULL, here as everywhere in this schema
    //
    // NULL is *"no reason was recorded"* — for a memory superseded before this
    // migration, and for one superseded today without `--reason`, which stays
    // legal. It is never a placeholder for an empty reason.
    "
    ALTER TABLE memories ADD COLUMN superseded_reason TEXT
        CHECK (superseded_reason IS NULL
               OR (superseded_reason <> '' AND length(superseded_reason) <= 512));
    ",
    // 14: the order checkpoints were actually written in, because
    // `created_at` cannot carry it.
    //
    // # The defect this closes, measured
    //
    // `CheckpointStore::latest_for` and `::latest` ordered by
    // `created_at DESC, id DESC`. `created_at` is whole seconds and `id` is
    // `lower(hex(randomblob(16)))`, so two checkpoints written inside one
    // second tie on the first key and are separated by a **coin flip on a
    // random identifier**. Measured through the real store over 800
    // back-to-back pairs, 798 of which shared a second: **414 resolved to the
    // older checkpoint** — 52%, which is what a fair coin looks like.
    //
    // That is not an internal tidiness problem. `latest` is what
    // `glasshouse checkpoint show`, `glasshouse launch --from-checkpoint
    // latest` and the automatic task-boundary carry-forward resolve through,
    // so a user resuming from *"the latest checkpoint"* got the wrong one
    // about half the time whenever two landed in the same second — and a
    // manual `checkpoint save` beside the task-boundary checkpoint
    // `shell::checkpoint_task_boundaries` takes does exactly that.
    //
    // # Why a counter and not a finer clock
    //
    // Sub-second timestamps would shrink the window and not close it, and
    // they would make the answer depend on the wall clock going forwards.
    // It does not: a clock that steps backwards — NTP, a suspended laptop, a
    // container starting with a bad time — would then make an older
    // checkpoint win, which is the same defect with a rarer trigger. `seq` is
    // *"how many checkpoints this project had written before this one"*, and
    // it has nothing to do with what time it was.
    //
    // # `ALTER TABLE ADD COLUMN`, migration 8's shape
    //
    // Migration 7's rule stands: a table is never rebuilt, because rebuilding
    // risks the rows already in it. Nothing here needs one. An added column
    // cannot be `NOT NULL` without a constant default, so it gets `DEFAULT 0`
    // — and 0 is deliberately outside the range the backfill assigns (1..n)
    // and outside the range `CheckpointStore::save` assigns (n+1 upwards), so
    // a row reading 0 is exactly *"written by something that did not go
    // through `save`"* and sorts as the oldest thing in the table rather than
    // silently winning.
    //
    // # What the backfill can and cannot recover
    //
    // Existing rows are ranked by `(created_at ASC, id ASC)`. The
    // between-second order was always recorded and is preserved exactly. The
    // within-second order **was never recorded anywhere**, so it cannot be
    // recovered and is not invented: rows tied on `created_at` keep the order
    // `id ASC` gave them, which is the order the old query already reported
    // for them. A database that migrates therefore answers every old question
    // exactly as it did before, and every new one correctly.
    //
    // # The indexes
    //
    // `checkpoints_by_session` is redefined on `(session_id, seq DESC)` so
    // `latest_for` keeps its seek-and-take-one shape rather than sorting the
    // session's rows; the `(session_id, created_at DESC)` it replaces is
    // indexing a key nothing orders by any more. `checkpoints_by_seq` is new
    // and serves `latest` and `list`, which previously had no index at all.
    // An index holds no data of its own, so dropping one is not the rebuild
    // migration 7 refuses — every row survives untouched, which is what
    // `a_version_thirteen_database_migrates_forward_keeping_the_order_it_could_record`
    // proves.
    "
    ALTER TABLE checkpoints ADD COLUMN seq INTEGER NOT NULL DEFAULT 0;

    UPDATE checkpoints SET seq = (
        SELECT COUNT(*) FROM checkpoints AS earlier
         WHERE earlier.created_at < checkpoints.created_at
            OR (earlier.created_at = checkpoints.created_at
                AND earlier.id <= checkpoints.id)
    );

    DROP INDEX checkpoints_by_session;
    CREATE INDEX checkpoints_by_session
        ON checkpoints (session_id, seq DESC);

    CREATE INDEX checkpoints_by_seq
        ON checkpoints (seq DESC);
    ",
    // 15: Phase 51's evaluation ledger — one row per decision Glasshouse made
    // whose wisdom is only visible later, written at the moment of the
    // decision.
    //
    // # What it is for, and the one question it does not answer
    //
    // Glasshouse can already answer questions about what it *is* — a memory's
    // status, a session's mechanism — and cannot answer questions about what
    // it *did*. A retrieval happens inside one function call, changes what the
    // user gets, and is gone. Phase 51's verb in 26 of its 37 lines is
    // *"measure how often"*, and nothing can count what was never written
    // down. This table answers *how often*, over a window, split by arm.
    //
    // It deliberately answers nothing about *how much*: cost, tokens and
    // latency belong to `routing_observations` (migration 11), and a column
    // for any of them here would be a second source of truth for a fact that
    // ledger already models. `routing_seq` is how a row points at the
    // observation that measured a turn instead of copying it.
    //
    // # A new table, for migration 11's own reasons one level up
    //
    // Not a widening of `lifecycle_events`. All eleven values in
    // [`LIFECYCLE_EVENT_KINDS`] are things that happened *to a session's
    // process or its harness*; these are decisions *Glasshouse* made, and
    // `crate::events`'s own module doc keeps that stream narrow on purpose.
    // Widening its `kind` would also be a third rebuild of the one table
    // `memories.source_event_first`/`_last` reference by `seq` — the hazard
    // migration 7 documents and the house rule below refuses. And, decisively:
    // `lifecycle_events` has three triggers that `RAISE(ABORT)` on every
    // `UPDATE` and every `DELETE`, so anything folded into it is permanent by
    // construction, and this table *must* be prunable (see "Retention").
    //
    // Not a view either: the rows a view would project — *a retrieval
    // happened* — are not stored anywhere. `memory_search_grouped` returns its
    // result and forgets, which is precisely and only what this table adds.
    //
    // # Why `kind` has no `CHECK`, and why that is not a lapse
    //
    // A `CHECK (kind IN (...))` is what `lifecycle_events` has, and it is why
    // map lines 310, 327 and 1316 are refused today: SQLite cannot widen a
    // `CHECK` in place, so an eleventh value cost a full table rebuild and a
    // twelfth is forbidden by the house rule at the top of migration 8. Phase
    // 51 is the phase whose vocabulary is *guaranteed* to grow — every future
    // measurable feature wants a new kind — so putting a SQL vocabulary here
    // would be manufacturing migration 7's problem deliberately, in the one
    // table most certain to need widening.
    //
    // The house already has the answer twice. [`LIFECYCLE_EVENT_KINDS`] exists
    // because the SQL `CHECK` was not trusted alone — its own doc says the
    // Rust constant plus a pinning test is what actually catches drift — and
    // `response_profile` (migration 8) gets no `CHECK` at all, on the stated
    // ground that pinning its combinations "would be a vocabulary this file
    // has no business holding". This column is `response_profile`'s case:
    // [`EVALUATION_KINDS`] beside an exhaustive `match` at the single writer,
    // pinned by a test that inserts every pair the enum can produce through
    // the real schema. What is given up is that a hand-written `INSERT` at a
    // `sqlite3` prompt can store nonsense; that is true of `response_profile`
    // today and has not hurt. `CHECK (kind <> '')` is kept because an empty
    // kind is not an unrecognised vocabulary, it is a missing one.
    //
    // `outcome` is the same case for a sharper reason: its vocabulary is *per
    // kind* — `helped`/`stale` for a retrieval, `preferred`/`displaced` for a
    // route — so a single global `CHECK` would be two vocabularies in one
    // column, which is the first objection this migration makes to widening
    // `lifecycle_events` at all.
    //
    // # `outcome` is the one column that is `NOT NULL DEFAULT 'unknown'`
    //
    // Migration 11's argument for `context_state`, verbatim: every other
    // column's NULL means *"not recorded"*, but a row that does not say how it
    // turned out must not be countable as *"turned out badly"*.
    // `DEFAULT 'unknown'` makes that automatic for any future insert path that
    // forgets to think about it, and it is what lets a rate report an honest
    // denominator with an honest unknown bucket instead of a flattering ratio.
    //
    // # Outcomes learned later are new rows, never an `UPDATE`
    //
    // A retrieval is recorded when it happens; whether it helped may only be
    // knowable a turn later. The answer is a second row with the same
    // `memory_id` and a later `observed_at`. This is migration 11's
    // "append-oriented is a property of the code as much as the schema":
    // `crate::evaluation`'s store offers `record` and reads, and no method
    // that edits a recorded observation. A measurement edited in place is a
    // falsified measurement.
    //
    // # Retention, which is part of this migration's contract
    //
    // **The three ledgers before this one grow forever, and this one has the
    // highest write rate.** `lifecycle_events` cannot be trimmed even
    // deliberately, and `routing_observations`' own doc comment anticipates
    // "some future retention policy" that was never written.
    //
    // So migration 5's three append-only triggers are **deliberately not
    // copied here** — they are exactly what makes `lifecycle_events`
    // unprunable, and repeating them would be repeating a known defect.
    // Migration 11's two project-scope triggers are copied exactly, and they
    // are the only ones. That is the load-bearing difference between the two
    // precedents, and it is why this table is named `evaluation_observations`
    // and not `evaluation_events`: the name should pull a future author toward
    // migration 11's prunable ledger and away from migration 5's permanent
    // stream. The bounds themselves (90 days, 100,000 rows, trimmed
    // oldest-first in the writer's own transaction) live with the writer, in
    // [`crate::evaluation::Retention`].
    //
    // `AUTOINCREMENT` means a `seq` is never reused after a delete, so pruning
    // can never make one row's identity come to mean another's — which is what
    // makes a prunable ledger safe to point at.
    //
    // # Two triggers, migration 11's pair, unchanged
    //
    // `IS NOT` rather than `<>`, so a missing binding row aborts the write
    // instead of the comparison evaluating to NULL and letting it through.
    // This is the structural half of map line 1856's *"keep evaluation data
    // local and project-scoped"*; the other half — that nothing exports it —
    // is a property of which functions exist in `crate::evaluation`, not of
    // the schema, and is recorded there.
    //
    // # Bare ids, no `REFERENCES`
    //
    // `memory_id` and `routing_seq` are migration 12's rule: a bare nullable
    // reference, no foreign key. A pointed-at row may be pruned or may never
    // have existed, and a read that cannot resolve one must report that rather
    // than lose the observation.
    //
    // # One index, and the second one is an experiment, not an omission
    //
    // `(kind, observed_at)` serves the shape every Phase 51 line reduces to:
    // how many rows of one kind fell in a window. An A/B split adds
    // `feature`/`arm` to the `WHERE`, which this index does not cover — do not
    // add `(feature, arm, kind, observed_at)` on speculation; fill the table
    // to its retention ceiling, read `EXPLAIN QUERY PLAN`, and add it if and
    // only if the plan is a scan and the scan is slow.
    "
    CREATE TABLE evaluation_observations (
        seq          INTEGER PRIMARY KEY AUTOINCREMENT,
        project_id   TEXT    NOT NULL,
        observed_at  INTEGER NOT NULL,

        -- What was decided. NOT a SQL vocabulary — see this migration's own
        -- 'Why `kind` has no CHECK' above.
        kind         TEXT    NOT NULL CHECK (kind <> ''),

        -- How it turned out, as far as was known when this row was written.
        -- Never silently absent: migration 11's `context_state` argument.
        outcome      TEXT    NOT NULL DEFAULT 'unknown' CHECK (outcome <> ''),

        -- What it was about, in the vocabulary of `kind`. Free text, never a
        -- count key on its own.
        subject      TEXT,

        -- The session the decision was made for, when it was made for one.
        session_id   TEXT,

        -- The A/B half. Two columns, never one joined string: migration 8's
        -- 'remain separately represented' rule. Their pairing is the table
        -- constraint at the bottom, because SQLite accepts no column
        -- definition after the first table constraint.
        feature      TEXT,
        arm          TEXT,

        -- Provenance: the row in the ledger that owns the measurement, so this
        -- table never copies one. Bare ids, no REFERENCES — migration 12's
        -- rule.
        memory_id    TEXT,
        routing_seq  INTEGER,

        -- The sentence a human reads after a count surprises them. Never
        -- parsed, never a WHERE key. `gateway_cause` (migration 7) is the
        -- precedent.
        detail       TEXT,

        -- An A/B arm without its feature is an arm of nothing, and a feature
        -- without its arm is a switch with no side recorded. Neither is a fact
        -- a count could use.
        CHECK ((feature IS NULL) = (arm IS NULL))
    );

    -- The one access pattern this table exists for: how many rows of one kind
    -- fell in a window. Everything Phase 51 asks is a filter on this index
    -- plus a GROUP BY outcome.
    CREATE INDEX evaluation_observations_by_kind_time
        ON evaluation_observations (kind, observed_at);

    CREATE TRIGGER evaluation_observations_reject_foreign_project_insert
    BEFORE INSERT ON evaluation_observations
    FOR EACH ROW
    WHEN NEW.project_id IS NOT (
        SELECT value FROM project_metadata WHERE key = 'project_id'
    )
    BEGIN
        SELECT RAISE(ABORT, 'evaluation observation belongs to a different project');
    END;

    CREATE TRIGGER evaluation_observations_reject_foreign_project_update
    BEFORE UPDATE OF project_id ON evaluation_observations
    FOR EACH ROW
    WHEN NEW.project_id IS NOT (
        SELECT value FROM project_metadata WHERE key = 'project_id'
    )
    BEGIN
        SELECT RAISE(ABORT, 'evaluation observation belongs to a different project');
    END;
    ",
    // 16: Phase 30's one missing fact — how many times a harness has told
    // this session it was about to compact its own context.
    //
    // # Why this is the only column Phase 30 needed
    //
    // The phase asks for eight things about a session's context. Seven of
    // them were already answerable from what this schema holds, and the
    // package that closed the phase says so line by line in
    // `session::store::SessionContext`: the most recent request or turn time
    // is `sessions.last_activity_at`, already stamped by the single
    // `UPDATE` that moves a session's lifecycle; a recent portable
    // checkpoint is `checkpoints.created_at` for the session, which
    // migration 5 recorded and migration 14 ordered; and a task-continuity
    // flag is a count of this session's `turn_ended` rows, which the event
    // log has stored with their `turn_outcome` since migration 5. Adding a
    // column for any of
    // those would be a second source of truth for a fact the schema holds
    // exactly once — migration 15's own objection to copying a token count
    // out of `routing_observations`, one table over.
    //
    // A compaction is the one that had nowhere to live.
    // `session::lifecycle::precedes_native_compaction` is called on the
    // production hook path and its answer was, until this migration, used to
    // fire a trigger and then discarded — its own doc comment said the fact
    // was "recorded nowhere". So this column is not a convenience: it is the
    // only durable record that the event happened at all.
    //
    // # Why a counter here and not a twelfth `lifecycle_events` kind
    //
    // Migration 7's rule, which migration 15 restates as this file's house
    // rule: `lifecycle_events.kind` carries a `CHECK`, SQLite cannot widen a
    // `CHECK` in place, and an eleventh value already cost a full table
    // rebuild of the one table `memories.source_event_first`/`_last`
    // reference by `seq`. A twelfth is refused outright. `precedes_native_
    // compaction`'s own documentation reached the same conclusion from the
    // other side and declined to invent a `LifecycleEvent` for it.
    //
    // That refusal blocks an *event row*. It does not block a *column*, and
    // the two are not the same claim: an event says "this happened at this
    // instant, in order, beside every other thing that happened"; a counter
    // says "this has now happened n times". Phase 30's line asks for the
    // number, not the timeline — *"track the number of observed compactions
    // for a session when known"* — so the counter is what the line wants and
    // is also the only one of the two this schema can add.
    //
    // # `ALTER TABLE ADD COLUMN`, migration 12's shape
    //
    // No table is rebuilt, no existing `CHECK` is altered, no existing row is
    // touched, and no index is added: nothing orders or filters by this
    // column, and migration 15's closing note about not adding an index on
    // speculation applies here with more force, because this one is written
    // far more often than it is read.
    //
    // # NULL, here as everywhere in this schema, and the distinction is the
    // whole point
    //
    // NULL is *"the build that recorded this session was not counting"*. Zero
    // is *"counted, and no compaction was observed"*. They are different
    // facts and a router must be able to tell them apart: a session with a
    // NULL is one whose context history is unknown, and one reading `0` is a
    // session Glasshouse watched from the start and saw compact nothing. A
    // `NOT NULL DEFAULT 0` would have collapsed the two and quietly promoted
    // every session recorded before this migration to "watched, and clean" —
    // which is exactly the confident wrong answer `sessions.launch_profile`'s
    // own doc comment (migration 3) refuses to allow.
    //
    // So this column is nullable and has no default. `SessionStore::create`
    // writes `0` for every session *this* build starts, which is what makes
    // the two states reachable at all, and the increment is
    // `COALESCE(observed_compactions, 0) + 1` so that a row from an older
    // build begins counting at its first observation rather than staying
    // unknowable for ever. What is given up, and it is stated rather than
    // hidden: for such a row the count is a **lower bound**, because
    // compactions before the upgrade were never observed by anything. For a
    // row this build created it is exact.
    //
    // # The `CHECK`
    //
    // Migration 9's shape for a counted quantity (`process_id > 0`): a
    // negative number of compactions is not an unrecognised value, it is an
    // impossible one, and the schema is where that is cheapest to refuse.
    "
    ALTER TABLE sessions ADD COLUMN observed_compactions INTEGER
        CHECK (observed_compactions IS NULL OR observed_compactions >= 0);
    ",
    // 17: which files were being worked on when a memory was learned —
    // Phase 28's missing primitive, and deliberately not Phase 28's
    // capability.
    //
    // # What this is, said before what it is not
    //
    // One row per (memory, path) pair, written from
    // `crate::checkpoint::WorkingTreeStatus::changed_files` at the moment
    // extraction ran. That list is what the git index says differs from the
    // working tree right now: no model, no subprocess, no guess.
    //
    // **It is a correlation with the session, not a reference by the
    // memory.** A session that dirtied twenty files and yielded three
    // memories associates all three with all twenty, and that is not a
    // rounding error in the signal — it *is* the signal. Map line 1139 asks
    // for the files a memory *"explicitly references"*, and on the automatic
    // extraction path the model's input contains no prose at all
    // (`memory::extract::lifecycle`'s own doc comment; `lifecycle_events` has
    // no text column), so a model asked to name files there would be
    // fabricating from an empty input. Map line 1294's rule — a fabricated
    // value does not degrade the policy, it inverts it — is why this table
    // records what was *observed* and says so in a column, rather than
    // claiming what was *referenced*.
    //
    // # A join table, which this schema has never had, and why not the
    // alternatives
    //
    // - **Not a column on `memories`.** A delimited or JSON list cannot be
    //   indexed for exact enumeration, which reproduces `checkpoints.document`'s
    //   defect one table over: you can look a row up, you cannot query the
    //   set.
    // - **Not a column in `memories_fts`.** FTS5 tokenisation destroys a path
    //   at both ends — `src/memory/store.rs` indexes and queries as four
    //   unrelated words, so every memory sharing any directory component
    //   would match — and migration 6 shows the cost is a full `DROP` /
    //   `CREATE` / `'rebuild'` plus three triggers.
    // - **Not `evaluation_observations`.** That table is *deliberately
    //   prunable* (90 days / 100,000 rows) and its `subject` is documented as
    //   free text that is "never a count key on its own". An association that
    //   expires after 90 days is not an association: the whole value of a
    //   file→memory link is that it outlives the session that made it.
    // - **Not `checkpoints.document`.** It already holds real observed paths,
    //   but it associates them with a *session* rather than a *memory*, in
    //   opaque JSON, reachable only by a full scan.
    //
    // # No `ALTER`, no rebuild, no existing `CHECK` touched
    //
    // Migration 15's shape: `CREATE TABLE` plus one index plus migration 11's
    // two project-scope triggers. `lifecycle_events` is untouched and no new
    // `LIFECYCLE_EVENT_KINDS` value is added, so map lines 310, 327 and 1316
    // keep the refusal the register gives them, word for word.
    //
    // # `path`: repo-relative, `/`-separated, UTF-8, never absolute
    //
    // **This is schema, not an implementation detail, and it is the one place
    // this table can fail invisibly.** A Windows path, a symlinked mount and
    // a relative-versus-absolute spelling are three values for one file; two
    // spellings become two rows and the exact-match index silently misses.
    // A missed association is invisible, where a wrong one is merely wrong,
    // so the normalisation is stated here and enforced at the writer by
    // `crate::memory::normalize_observed_path`.
    //
    // The observed producer needs no normalisation *work*: git's index stores
    // every path as UTF-8, repo-relative and `/`-separated on every platform,
    // Windows included, and `checkpoint::git::parse_index` reads it straight
    // with no separator translation. The contract exists for the writers that
    // come after it — a model-emitted or user-typed path is five spellings of
    // one file and must be normalised or refused before it reaches this
    // column.
    //
    // Repo-relative is also what keeps the `/var` versus `/private/var` class
    // of hazard out of the index key: that ambiguity lives in the *root*, and
    // the root is never stored here. An absolute path would import it
    // directly into the one column this table matches on.
    //
    // Enforcement is at the writer rather than in a `CHECK` because the
    // schema cannot express it: `CHECK (path NOT LIKE '/%')` would miss
    // `C:\...`, and a `CHECK` forbidding `\` or `:` would reject file names
    // that are legal on Unix. The schema refuses only what is never a path at
    // all — the empty string.
    //
    // # `seq`, and bare ids
    //
    // `AUTOINCREMENT`, migration 11's and 15's shape for an append-oriented
    // row: this table has no `UPDATE` path, and an identifier is never reused
    // even after a future retention policy prunes rows. `memory_id` is a bare
    // id with no `REFERENCES`, migration 12's rule as migration 15 restates
    // it: a pointed-at row may be gone, and a read that cannot resolve one
    // must say so rather than lose the observation.
    //
    // # Zero rows is one fact here, not two, and that is deliberate
    //
    // A join table cannot distinguish *"the tree was clean"* from
    // *"extraction ran before this feature existed"* — both are no rows. A
    // marker column on `memories` would separate them and is exactly the
    // `ALTER` this migration refuses; the distinction is not worth widening
    // the schema's blast radius for while nothing reads it. Stated rather
    // than hidden: for a memory recorded by an older build, the absence of
    // rows means *unknown*, and for one recorded by this build it means the
    // reader found nothing to name.
    //
    // # One index, and only the one
    //
    // `(path)` serves the only access pattern this table exists for: which
    // memories were learned while this file was being worked on. Migration
    // 15's closing note applies unchanged — do not add a second index on
    // speculation.
    "
    CREATE TABLE memory_files (
        seq         INTEGER PRIMARY KEY AUTOINCREMENT,
        project_id  TEXT    NOT NULL,

        -- The memory this row is about. A bare id, no REFERENCES —
        -- migration 12's rule.
        memory_id   TEXT    NOT NULL,

        -- Repo-relative, '/'-separated, UTF-8, never absolute. See this
        -- migration's own 'path' section for why that contract lives at the
        -- writer and what the schema can and cannot police.
        path        TEXT    NOT NULL CHECK (path <> ''),

        -- HOW the association was made, so a later narrower signal is never
        -- silently averaged together with this one. NOT a SQL vocabulary —
        -- see `MEMORY_FILE_PROVENANCE`.
        provenance  TEXT    NOT NULL CHECK (provenance <> ''),

        -- When it was observed. Not the memory's own `created_at`: the two
        -- are written by the same call today and need not stay that way.
        observed_at INTEGER NOT NULL CHECK (observed_at >= 0)
    );

    -- The one access pattern: which memories were learned while this exact
    -- path was being worked on. Exact equality, never a text match.
    CREATE INDEX memory_files_by_path ON memory_files (path);

    CREATE TRIGGER memory_files_reject_foreign_project_insert
    BEFORE INSERT ON memory_files
    FOR EACH ROW
    WHEN NEW.project_id IS NOT (
        SELECT value FROM project_metadata WHERE key = 'project_id'
    )
    BEGIN
        SELECT RAISE(ABORT, 'memory file association belongs to a different project');
    END;

    CREATE TRIGGER memory_files_reject_foreign_project_update
    BEFORE UPDATE OF project_id ON memory_files
    FOR EACH ROW
    WHEN NEW.project_id IS NOT (
        SELECT value FROM project_metadata WHERE key = 'project_id'
    )
    BEGIN
        SELECT RAISE(ABORT, 'memory file association belongs to a different project');
    END;
    ",
    // 18: `routing_observations.failure_class` — capability map line 1364's
    // nine-way failure classification, and lines 1316 and 1365's separation
    // of a rate-limit response from a transport or model failure, and of
    // cadence throttling from a spent long-window quota.
    //
    // # A column beside `outcome`, not a widening of it
    //
    // `outcome` answers *did the turn succeed* and carries a four-value
    // `CHECK`; this answers *what kind of failure was it* and carries none.
    // Widening `outcome`'s `CHECK` would cost a table rebuild per new value
    // (migration 7's lesson, restated at migration 15) and would blur two
    // questions into one column. A served exchange has an `outcome` and no
    // `failure_class`; a failed one has both.
    //
    // # `ADD COLUMN`, nullable, no `CHECK`, no index
    //
    // Migration 10's shape for `validity_conditions`: an `ALTER TABLE … ADD
    // COLUMN` backfills every existing row with `NULL`, which is the honest
    // reading for a row written before the classification existed — "this
    // build recorded no kind here", never "no failure". No `CHECK`, for
    // `FAILURE_CLASSES`' reason: the vocabulary lives in Rust and is pinned
    // by a test. No index: the reads that want this column
    // (`EvidenceLedger::failure_classes_by_provider`) are a `GROUP BY` over
    // a time window already served by `routing_observations_by_route_time`,
    // and migration 15's closing note applies — measure before indexing.
    //
    // # What may write it, and from what
    //
    // The gateway's connection thread, from the status line, the rate-limit
    // headers it already reads to forward them, the byte count it already
    // keeps to relay the body, and how the stream ended as its framing said.
    // Never from a byte of the body: `crate::gateway::ingress` remains
    // structurally unable to carry one, and the design ruling that framing
    // is not content is in `docs/product/design-decisions.md`.
    "
    ALTER TABLE routing_observations ADD COLUMN failure_class TEXT;
    ",
    // 19: Phase 21K's assumption ledger — the few premises an agent states a
    // substantial change rests on, and what became of each.
    //
    // # What a row is, and what no row is
    //
    // `task_assumptions` holds the six fields capability map lines 1014 and
    // 1016 name — claim, current evidence, evidence-source class,
    // uncertainty, affected scope, cheapest verification — and who stated
    // them, for which session, when. **Nothing here was inferred.** Every
    // row was said through `api::protocol::Request::RecordAssumption` or its
    // MCP twin; Glasshouse reads no transcript and no output for one (line
    // 998), and there is no column that could hold reasoning if it did.
    //
    // `assumption_transitions` is the append-only history. A row naming an
    // `assumption_id` moves it to one of line 1018's six states — or
    // re-states the current one with a response or a note — and **the
    // current state is the latest such row** (`MAX(seq)`), which is why the
    // assumption row itself carries no `state` column: there is exactly one
    // place a state can be, so it can never be two things at once. A row
    // with no `assumption_id` is a session-level event (`kind` is `gate`,
    // `override` or `budget_exceeded`): the fact that a preflight fired and
    // which factor fired it (line 1049), the per-task override a person
    // recorded (line 1008), a budget found exceeded (line 1050). The two
    // table constraints say exactly that: a row is about an assumption or a
    // session, and an assumption row always carries a state.
    //
    // # No `CHECK` on any vocabulary, for migration 15's reason
    //
    // `state`, `kind`, `origin`, `evidence_source`, `uncertainty` and
    // `response` are each a vocabulary that lives in Rust —
    // `crate::guardrails`' enums, one stored spelling per variant, an
    // exhaustive `match` at the single writer — and none of them gets a SQL
    // `CHECK`, because a `CHECK` is what cost `lifecycle_events` a table
    // rebuild for its eleventh value. `CHECK (x <> '')` is kept where a
    // value is required: an empty spelling is a missing one, not a strange
    // one.
    //
    // # Append-only by trigger, prunable by design
    //
    // A `BEFORE UPDATE` trigger on each table refuses every edit, so *"no
    // `UPDATE` of a recorded transition"* is the schema's guarantee and not
    // only the store's method list. **No `DELETE` trigger**, deliberately:
    // task assumptions are transient (line 1017 — what is worth keeping is
    // promoted into `memories`), so this ledger keeps the evaluation ledger's
    // bounds (`crate::guardrails::store::Retention`: 90 days or 100,000
    // transitions) and is trimmed oldest-first in the writer's own
    // transaction. `AUTOINCREMENT` means a trimmed `seq` is never reused, so
    // a watcher's cursor can never come to mean a different row.
    //
    // # Project scope
    //
    // Migration 15's two triggers, copied exactly, on both tables. The
    // database path comes from `Runtime` and nowhere else, and every
    // session-keyed request goes through `SessionApi` before this store is
    // opened, so a foreign session identifier is refused before a row could
    // be written for it.
    //
    // # Bare ids, no `REFERENCES`
    //
    // `assumption_id` and `session_id` are migration 12's rule: a pointed-at
    // row may be trimmed, and a read that cannot resolve one reports that
    // rather than losing the transition.
    "
    CREATE TABLE task_assumptions (
        id               TEXT    PRIMARY KEY,
        project_id       TEXT    NOT NULL,
        session_id       TEXT,
        created_at       INTEGER NOT NULL,
        origin           TEXT    NOT NULL CHECK (origin <> ''),

        -- The six fields, and only these. Free text is sanitized by the
        -- writer; the vocabularies are Rust's.
        claim            TEXT    NOT NULL CHECK (claim <> ''),
        evidence         TEXT    NOT NULL,
        evidence_source  TEXT    NOT NULL CHECK (evidence_source <> ''),
        uncertainty      TEXT    NOT NULL CHECK (uncertainty <> ''),
        affected         TEXT    NOT NULL,
        verification     TEXT    NOT NULL
    );

    CREATE INDEX task_assumptions_by_session
        ON task_assumptions (session_id, created_at DESC);

    CREATE TRIGGER task_assumptions_reject_foreign_project_insert
    BEFORE INSERT ON task_assumptions
    FOR EACH ROW
    WHEN NEW.project_id IS NOT (
        SELECT value FROM project_metadata WHERE key = 'project_id'
    )
    BEGIN
        SELECT RAISE(ABORT, 'task assumption belongs to a different project');
    END;

    CREATE TRIGGER task_assumptions_reject_foreign_project_update
    BEFORE UPDATE OF project_id ON task_assumptions
    FOR EACH ROW
    WHEN NEW.project_id IS NOT (
        SELECT value FROM project_metadata WHERE key = 'project_id'
    )
    BEGIN
        SELECT RAISE(ABORT, 'task assumption belongs to a different project');
    END;

    CREATE TRIGGER task_assumptions_never_edited
    BEFORE UPDATE ON task_assumptions
    FOR EACH ROW
    BEGIN
        SELECT RAISE(ABORT, 'a task assumption is never edited: its state lives in assumption_transitions');
    END;

    CREATE TABLE assumption_transitions (
        seq            INTEGER PRIMARY KEY AUTOINCREMENT,
        project_id     TEXT    NOT NULL,
        assumption_id  TEXT,
        session_id     TEXT,
        at             INTEGER NOT NULL,

        -- transition | gate | override | budget_exceeded. Rust's vocabulary.
        kind           TEXT    NOT NULL CHECK (kind <> ''),
        -- One of the six states for an assumption row; NULL for a
        -- session-level row unless the row is a waiver.
        state          TEXT,
        origin         TEXT    NOT NULL CHECK (origin <> ''),
        -- Machine-written, in the vocabulary of `kind`.
        subject        TEXT,
        -- One of the seven responses to a guardrail event, when one was chosen.
        response       TEXT,
        -- Free text from the caller, sanitized by the writer.
        note           TEXT,

        CHECK (assumption_id IS NOT NULL OR session_id IS NOT NULL),
        CHECK (assumption_id IS NULL OR state IS NOT NULL)
    );

    CREATE INDEX assumption_transitions_by_assumption
        ON assumption_transitions (assumption_id, seq DESC);
    CREATE INDEX assumption_transitions_by_session
        ON assumption_transitions (session_id, seq DESC);

    CREATE TRIGGER assumption_transitions_reject_foreign_project_insert
    BEFORE INSERT ON assumption_transitions
    FOR EACH ROW
    WHEN NEW.project_id IS NOT (
        SELECT value FROM project_metadata WHERE key = 'project_id'
    )
    BEGIN
        SELECT RAISE(ABORT, 'assumption transition belongs to a different project');
    END;

    CREATE TRIGGER assumption_transitions_reject_foreign_project_update
    BEFORE UPDATE OF project_id ON assumption_transitions
    FOR EACH ROW
    WHEN NEW.project_id IS NOT (
        SELECT value FROM project_metadata WHERE key = 'project_id'
    )
    BEGIN
        SELECT RAISE(ABORT, 'assumption transition belongs to a different project');
    END;

    CREATE TRIGGER assumption_transitions_append_only
    BEFORE UPDATE ON assumption_transitions
    FOR EACH ROW
    BEGIN
        SELECT RAISE(ABORT, 'assumption_transitions is append-only: a transition is never edited');
    END;
    ",
    // 20: `sessions.presentation_ref` — Phase 17 line 760, *"record the cmux
    // surface or pane identifier as optional session presentation
    // metadata."*
    //
    // # A column beside `presentation`, not a widening of it
    //
    // `presentation` answers *where is this session shown* with one of three
    // words and carries a `CHECK`; this answers *which pane*, and only for a
    // session whose answer to the first question is `external`. Folding the
    // reference into `presentation` (`external:workspace:349`) would make one
    // column carry two facts and would break every reader that matches the
    // three words, including the shell's — which is exactly the layer line
    // 762 keeps ignorant of cmux.
    //
    // # `ADD COLUMN`, nullable, no `CHECK`, no index
    //
    // Migration 18's shape. Every existing row backfills to `NULL`, which is
    // the honest reading: a session recorded before this column existed was
    // presented somewhere Glasshouse did not write down, which is a different
    // fact from a session recorded now with no pane. No `CHECK` on the shape
    // (`workspace:<n>` / `surface:<n>`): the validation lives in Rust, at the
    // one place the value is handed back to cmux
    // (`integrations::cmux::PaneRef::parse`), so a cmux that changed its
    // reference syntax would be met in one file rather than by a table
    // rebuild. No index: the only reads are a session's own row and a short
    // bounded poll after a pane is opened.
    //
    // # What may write it, and from what
    //
    // `SessionStore::create`, once, from `NewSession::presentation_ref`,
    // which only `main.rs`'s launch path fills — from the reference cmux
    // itself printed (`cmux identify --json`), or from one a caller supplied
    // by hand. Nothing in `session/**` interprets the string; the session
    // abstraction stores and returns it and learns nothing else (line 762).
    "
    ALTER TABLE sessions ADD COLUMN presentation_ref TEXT;
    ",
    // 21: the two facts a *memory commit* needs and this schema could not
    // hold — capability map lines 1147-1154.
    //
    // # `sessions.last_seen_commit`: how a commit is noticed without a Git hook
    //
    // Line 1149 wants a memory commit *"after a successful Git commit"*.
    // Glasshouse installs no Git hook and will not: a repository's hooks are
    // the user's, `core.hooksPath` can point anywhere, and a tool that writes
    // into `.git/hooks` to learn something it can read directly has taken
    // over a file it does not own. It does not need one. The harness hook
    // already runs at every `TurnEnded`, and `checkpoint::git::GitPosition`
    // already reads HEAD without spawning `git` — so *"a commit landed"* is
    // the comparison between HEAD now and HEAD the last time this session was
    // looked at, and this column is the second half of that comparison.
    //
    // Per **session**, not per project: two sessions in one project each have
    // their own idea of what they have seen, and a shared column would let
    // one session's turn silently consume the other's boundary.
    //
    // `NULL` is *"Glasshouse has not looked at HEAD for this session yet"*,
    // and the first look records it **without** treating it as a boundary —
    // nothing changed, a position was simply learned. That is the same
    // distinction migration 16 draws for `observed_compactions`, reached the
    // other way round: that column starts at a measured `0` because `create`
    // can measure it, and this one cannot, because `SessionStore::create` has
    // no project root to read a repository from.
    //
    // # `memories.extraction_trigger`: what made Glasshouse look
    //
    // Lines 1147-1151 ask for four ways to start a memory commit and line
    // 1153 asks that the commit be recorded *"with memories produced from a
    // code-change boundary"*. `memories.source_commit` has existed since
    // migration 6 and answers a different question — **where the project
    // stood** when something was learned — and `glasshouse memory extract`,
    // run by hand, fills it from `GitPosition::detect`. So a reader inferring
    // "this came from a code-change boundary" from a commit being present
    // would report every hand-run extraction as one. The trigger is the fact
    // that was missing, and it is a column rather than a derivation.
    //
    // # Both in one migration
    //
    // They are one capability: the trigger vocabulary has a `git_commit` word
    // only because `last_seen_commit` can produce it. Splitting them would
    // create an intermediate schema version in which the word exists and
    // nothing can ever write it.
    //
    // # `ADD COLUMN`, nullable, no `CHECK`, no index
    //
    // Migration 18's shape and its reasons, unchanged. `NULL` backfills every
    // existing row, which is the honest reading for a row written before
    // either fact was observable. No `CHECK` on `extraction_trigger` for
    // `FAILURE_CLASSES`' reason — the vocabulary lives in Rust, on
    // `ExtractionTrigger`, and is pinned there by a test; a `CHECK` would cost
    // a table rebuild per new trigger, and `memories` is the table
    // `memories_fts` shadows and `memory_files` references. No index: nothing
    // queries by trigger, and migration 15's closing note applies.
    //
    // # What may write them
    //
    // `last_seen_commit`: `SessionStore::record_seen_commit`, from the hook
    // path's `TurnEnded` arm, with a full object name `GitPosition::detect`
    // read out of `.git`. `extraction_trigger`: `Extractor::store_one`, from
    // `ExtractionTrigger::as_str`, which is `&'static str` precisely so that
    // no runtime string — a commit hash least of all — can reach this column.
    "
    ALTER TABLE sessions ADD COLUMN last_seen_commit TEXT;
    ALTER TABLE memories ADD COLUMN extraction_trigger TEXT;
    ",
    // 22: which entitlement served this session — capability map line 1972's
    // durable half, *"what it served"*.
    //
    // # Why `backend_resource` could not answer this
    //
    // `sessions.backend_resource` has held the resolved resource since its
    // own `ADD COLUMN` above, and it stores
    // `crate::profile::BackendResource::slug`, whose whole vocabulary is
    // three coarse words: `native`, `direct-provider:<provider>`, and
    // `glasshouse-gateway`. Phase 56A's unit of capacity is the
    // **entitlement** — two Claude accounts of one vendor, each with its own
    // credential, capacity and reset — and both of those accounts slug to
    // the same `native`. So the one question line 1972 asks of the durable
    // record, *which account served this session*, is the one question
    // `backend_resource` is structurally unable to answer, and no widening
    // of its vocabulary would help: it names a **kind** of resource, and the
    // entitlement is an **instance**.
    //
    // # What may write it, and from what
    //
    // `SessionStore::create`, once, from `NewSession::entitlement`, which
    // only `main.rs`'s launch path fills — from
    // `ResolvedEntitlement::name`, the `[entitlements.<name>]` table key,
    // for the entitlement that path has already resolved and announced
    // (`announce_entitlement`). That is the router's own winner where the
    // router ran (`Routed::chosen`'s `Destination::entitlement`, re-resolved
    // by name), and the one-account lookup where it did not. Nothing else
    // writes the column and nothing derives it: a session whose serving
    // account was never established records `NULL` rather than a guess.
    //
    // # A name, and never a credential
    //
    // The value is the entry's **name** — the key a person typed in their
    // own configuration file. An entitlement's authentication is a
    // `crate::secret::SecretRef`, a reference resolved through the operating
    // system's secret storage at the moment of use, and it does not travel
    // this way: the `sessions` table's own doc says why (this database is
    // backed up casually and checked into nothing). The name is the same
    // string `glasshouse status` already prints, so this column adds no
    // fact that was not already displayable.
    //
    // # `ADD COLUMN`, nullable, no `CHECK`, no index
    //
    // Migration 20's shape and its stated rationale — validation in Rust,
    // not in SQL — unchanged. `NULL` backfills every existing row, which is
    // the honest reading for a session recorded before Glasshouse could
    // observe which account served it, and it is a **different fact** from
    // any name: `launch_profile`'s `None` draws exactly this distinction and
    // for exactly this reason. No `CHECK`, because the set of valid values
    // is the user's own `[entitlements]` tables — it is not a fixed
    // vocabulary this schema could enumerate, and it changes when a person
    // edits a configuration file rather than when Glasshouse ships. No
    // index: the reads are a session's own row and one bounded pass over a
    // project's sessions for `glasshouse entitlements`, and migration 15's
    // closing note applies.
    "
    ALTER TABLE sessions ADD COLUMN entitlement TEXT;
    ",
    // 23: `routing_observations.task_class` — capability map line 1276's
    // *"short moving average of requests consumed per task class"*, whose
    // producer has existed since Phase 34C and whose row has never carried
    // it.
    //
    // # Persisted, not recomputed
    //
    // `crate::routing::request::RouterAnswer::task_class` derives the class
    // from a `TaskClassification` that lives only for the duration of one
    // routing decision: the classification is not stored anywhere, so a
    // reader looking at yesterday's rows has nothing to derive from. A
    // moving average over task classes is a read of *history*, and history
    // is exactly what is unavailable unless the class is written down at the
    // moment it is known. `main.rs::record_routing_latency` already holds
    // the `RouterAnswer` and already writes the row; this column is the one
    // missing link between them.
    //
    // # `ADD COLUMN`, nullable, no `CHECK`, no index
    //
    // Migration 18's shape and its reasons, unchanged. `NULL` backfills every
    // existing row, which is the honest reading for a row written before the
    // class was recorded — "this build named no class here", never "no
    // class". No `CHECK`, for `FAILURE_CLASSES`' reason: the vocabulary is
    // `crate::routing::request::TaskClass`, five variants pinned in Rust by
    // `every_task_class_the_type_supports_is_one_the_schema_records`, and a
    // `CHECK` would cost a table rebuild the first time a sixth class is
    // added. No index: the one reader
    // (`crate::routing::burn::task_class_request_rates`) is a bounded pass
    // over a window `routing_observations_by_route_time` already serves, and
    // migration 15's closing note applies — measure before indexing.
    //
    // # An unrecognised word reads back as `None`, unlike `failure_class`
    //
    // `row_to_observation` reports an unrecognised `failure_class` as
    // `EvidenceLedgerError::UnknownValue`, because a failure whose kind this
    // build cannot name is a fact a reader must not silently lose. A task
    // class is not that: it is a *bucketing* input to an average, and a row
    // whose class this build does not recognise is exactly as informative as
    // a row from before the column existed — one more request, of no class
    // this build counts. Failing the whole row would make a future build's
    // sixth class break an older build's burn rate, which is a worse outcome
    // than the older build ignoring one bucket it never knew about.
    //
    // # What may write it
    //
    // `main.rs::record_routing_latency`, from
    // `crate::routing::request::TaskClass::as_str`, which is
    // `&'static str` precisely so no runtime string can reach this column.
    // Nothing parses a relayed response body to fill it: the class comes
    // from Glasshouse's own classification of the *request*, never from
    // anything a provider said.
    "
    ALTER TABLE routing_observations ADD COLUMN task_class TEXT;
    ",
    // 24: `routing_observations.session_id`, `.effort_level` and
    // `.turn_shape` — capability map line 2019's *"show the per-session
    // cache ratio beside the routing evidence"* and line 2039's shadow
    // measurement, neither of which has a producer while this table cannot
    // name the session an exchange belonged to. Designed before it was
    // written: `docs/product/design-decisions.md`, *A session identity on
    // the routing evidence rows — Cluster G's first column*.
    //
    // # Which identity, and which two facts beside it
    //
    // `sessions.id` — Glasshouse's own session id — and nothing else. Not
    // the harness's `metadata.user_id`: carrying that would mean the relay
    // reading a body it never reads (`crate::gateway::ingress`'s own
    // `an_exchange_has_nowhere_to_put_a_body`), and it names an account this
    // ledger has no business holding. Not `sessions.native_session_id`
    // either: that column already resolves the harness-side mapping, and the
    // Glasshouse id is the value `evaluation_observations.session_id`
    // already keys by, so every join these columns exist for is on one value
    // with no translation step.
    //
    // The other two are facts of the *request*, filled at the one seam that
    // holds a decoded one — `crate::gateway::translate::serve` — and they
    // ride here so that line 2039's shadow needs no second migration:
    // `effort_level` is the four-word ladder
    // `crate::gateway::translate::canonical::EffortRequest::level` reduces
    // the harness's thinking request to, and `turn_shape` is *tool-resume*
    // when the last user message's blocks are all tool results and *prompt*
    // otherwise. A relayed exchange, whose body is never read, records
    // `NULL` for both: unread, not absent.
    //
    // # What may write them
    //
    // `crate::gateway::session::SessionRouting::record_routing_observation`,
    // once per exchange the gateway serves, from the id `main.rs`'s two
    // launch doors hand it through `SessionRouting::serve_session` after the
    // session record exists. A gateway nothing has told is a gateway serving
    // no session, and its rows say so with `NULL` rather than an invented
    // id. `main.rs::record_routing_latency`'s own row — written before the
    // record exists — stays `NULL` and says why in its doc comment: it is a
    // row about the routing decision, not about a served exchange.
    //
    // # `ADD COLUMN`, nullable, no `CHECK`, no `REFERENCES`, no index
    //
    // Migration 23's shape and its reasons. `NULL` backfills every existing
    // row, which is the honest reading for a row written before a session
    // could be named — "this build recorded none here", never "none". No
    // `CHECK`: `effort_level` and `turn_shape` are Rust enums pinned by
    // tests exactly as `task_class` is, and `session_id` is an opaque
    // identifier with no enumerable vocabulary at all. No `REFERENCES`:
    // migration 12's rule, and a routing row must outlive the deletion of
    // the session it names, as the evaluation rows already do. No index: the
    // readers are bounded window passes `routing_observations_by_route_time`
    // already serves, and migration 15's closing note applies — measure
    // before indexing.
    //
    // # An unrecognised word reads back as `None`
    //
    // Migration 23's rule, not migration 18's: both stored vocabularies are
    // *bucketing* inputs to a ratio, and a row whose word this build does
    // not recognise is exactly as informative as a row from before the
    // column existed. Failing the whole row would let a future build's fifth
    // effort word break this build's savings readout, which is worse than
    // this build ignoring a bucket it never knew about.
    "
    ALTER TABLE routing_observations ADD COLUMN session_id TEXT;
    ALTER TABLE routing_observations ADD COLUMN effort_level TEXT;
    ALTER TABLE routing_observations ADD COLUMN turn_shape TEXT;
    ",
    // 25: `routing_observations.first_byte_ms`, `.first_token_ms`,
    // `.first_tool_call_ms` and `.completed_ms` — capability map lines 1347
    // (TTFC as the responsiveness measure for tool-using work), 1348 (TTFT
    // kept apart from it), 1349 (decode tokens per second) and 1355 (all of
    // them shown separately). Designed before it was written:
    // `docs/product/design-decisions.md`, *Millisecond offsets on the
    // routing row — Cluster G's second column set*.
    //
    // # Why a column at all, when five timestamps are already here
    //
    // Every timestamp on this table is a unix second: `dispatched_at`,
    // `first_byte_at`, `completed_at`, and since migration 11's two
    // late-written columns `first_token_at` and `first_tool_call_at`. At
    // that resolution *time to first byte* and *time to first token* are
    // zero or one on nearly every exchange — honest, and useless for the
    // comparison lines 1347 to 1355 ask for. The producer wall is gone (the
    // translated seam decodes what it needs); what remains is resolution,
    // and resolution is a column decision.
    //
    // # Offsets, not instants, and their zero is not `dispatched_at`
    //
    // A monotonic clock (`std::time::Instant`) is what the gateway can read
    // at millisecond precision; a wall clock is not, and two wall readings
    // subtracted across a clock step produce a negative "duration" that
    // means nothing. So each column is a number of milliseconds since a
    // `std::time::Instant` taken **immediately before the upstream request
    // was sent** — `crate::gateway::ingress::forward` for a relayed
    // exchange, `crate::gateway::translate::serve` for a translated one.
    //
    // That zero is deliberately *not* `dispatched_at`, whose own comment in
    // `crate::gateway::accept_loop` says it is the instant the connection
    // was handed to `ingress::serve`, not the instant a request left for the
    // provider. The five `*_at` columns stay, are written exactly as before,
    // and remain this row's only absolute timestamps.
    //
    // # A `CHECK`, unlike migrations 23 and 24
    //
    // Migration 11's token columns' shape, not migration 24's: these hold a
    // quantity with an arithmetic floor rather than a word from a
    // vocabulary. A negative offset is not an unrecognised bucket a later
    // build might have meant — it is a reading no monotonic clock can
    // produce, so the schema refuses it rather than letting a reader average
    // it. The `CHECK` is column-scoped and is therefore dropped with its own
    // column, migration 16's rule.
    //
    // Nullable, no index: `NULL` keeps the meaning every other optional
    // column here has — *this producer did not measure* — and backfills
    // every row written before this migration; the readers are the same
    // bounded window passes `routing_observations_by_route_time` already
    // serves.
    //
    // # What may write them
    //
    // `crate::gateway::session::SessionRouting::record_routing_observation`,
    // from the four offsets `crate::gateway::ingress::Exchange` carries. A
    // relayed exchange carries `first_byte_ms` and `completed_ms` and
    // `NULL` for the two token offsets, exactly as it does for
    // `first_token_at` and `first_tool_call_at`. The support-work rows
    // `main.rs::record_extraction_observation` writes keep their seconds:
    // that producer takes no `Instant` of its own, and inventing one from
    // two wall readings is the defect the `CHECK` exists to refuse.
    "
    ALTER TABLE routing_observations ADD COLUMN first_byte_ms INTEGER
        CHECK (first_byte_ms IS NULL OR first_byte_ms >= 0);
    ALTER TABLE routing_observations ADD COLUMN first_token_ms INTEGER
        CHECK (first_token_ms IS NULL OR first_token_ms >= 0);
    ALTER TABLE routing_observations ADD COLUMN first_tool_call_ms INTEGER
        CHECK (first_tool_call_ms IS NULL OR first_tool_call_ms >= 0);
    ALTER TABLE routing_observations ADD COLUMN completed_ms INTEGER
        CHECK (completed_ms IS NULL OR completed_ms >= 0);
    ",
];

pub(crate) const PROJECT_ID_KEY: &str = "project_id";

/// Everything that can go wrong while preparing a project database.
///
/// Every variant carries the database path in its message so an error is
/// actionable even when it surfaces far from where the path was known.
#[derive(Debug, thiserror::Error)]
pub(crate) enum DatabaseError {
    #[error(
        "project database `{path}` is a symbolic link; refusing to follow it \
         because the link target could change what Glasshouse reads and writes"
    )]
    Symlinked { path: PathBuf },
    #[error("project database `{path}` exists but is {actual}; refusing to use it as a database")]
    NotARegularFile { path: PathBuf, actual: &'static str },
    #[error("could not inspect project database `{path}`")]
    Inspect {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not create project database `{path}`")]
    Create {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not open project database `{path}`")]
    Open {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },
    #[error("could not configure project database `{path}`")]
    Configure {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },
    #[error(
        "project database `{path}` was opened read-only; Glasshouse cannot \
         store project memory in a database it cannot write to, so check the \
         file's permissions"
    )]
    ReadOnly { path: PathBuf },
    #[error("could not prepare the schema of project database `{path}`")]
    Sql {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },
    #[error(
        "project database `{path}` was written by a newer Glasshouse \
         (schema version {found}; this build supports up to {supported}); \
         refusing to guess how to read it"
    )]
    TooNew {
        path: PathBuf,
        found: i64,
        supported: i64,
    },
    #[error(
        "project database `{path}` belongs to project `{actual}`, not to the \
         active project `{expected}`; refusing to mix project memories"
    )]
    ProjectMismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },
    #[error(
        "project database `{path}` has project metadata but no project identifier; \
         refusing to adopt an unbound database because it may belong to another project"
    )]
    MissingProjectId { path: PathBuf },
    #[error(
        "project database `{path}` exists but is empty (zero bytes); a genuinely new \
         project has no database file at all, so an existing file this size means it was \
         likely truncated — by a crashed copy, an interrupted restore, or a disk-full \
         write. Restore it from a backup, or delete the file deliberately if you want to \
         start this project fresh"
    )]
    EmptyExisting { path: PathBuf },
}

/// Create or validate the project database of the given runtime's project.
///
/// Both the database path (`<state_dir>/glasshouse.db`) and the binding
/// project identifier are derived from `runtime`; no caller — inside or
/// outside the crate — can point this initializer at another file or bind it
/// to another project.
///
/// Called from `bootstrap`, so a successful [`crate::Runtime`] always has a
/// valid project database waiting in its state directory. On success the
/// connection is closed again; nothing holds it open between launches.
///
/// Use [`open`] instead when the caller actually needs to read or write.
pub(crate) fn ensure_ready(runtime: &Runtime) -> Result<(), DatabaseError> {
    // Dropping the connection closes it. Validation is the point of the call.
    open(runtime).map(drop)
}

/// Open the project database, applying every check [`ensure_ready`] applies,
/// and hand back the live connection.
///
/// This is the only way anything in Glasshouse obtains a usable connection, so
/// the symlink refusal, the read-only refusal, the project-identity check, and
/// the migrations are not steps a caller can skip or reorder. The path and the
/// binding identifier both come from `runtime`; neither is a parameter.
pub(crate) fn open(runtime: &Runtime) -> Result<Connection, DatabaseError> {
    let db_path = runtime.database_path();
    let project_id = runtime.project().id().as_str();

    prepare_file(&db_path)?;

    let mut conn = Connection::open_with_flags(
        &db_path,
        // No SQLITE_OPEN_CREATE: the file was just created above with the
        // right permissions, and if it vanished since then we want the open
        // to fail rather than silently recreate it.
        //
        // No SQLITE_OPEN_NOFOLLOW either, despite being offered: it makes
        // SQLite reject a symlink in *any* path component, not just the final
        // one, which breaks entirely legitimate locations such as macOS's
        // `/var` -> `/private/var`. A symlink at the final database path is
        // refused explicitly by `prepare_file` instead.
        OpenFlags::SQLITE_OPEN_READ_WRITE,
    )
    .map_err(|source| DatabaseError::Open {
        path: db_path.clone(),
        source,
    })?;

    // SQLITE_OPEN_READWRITE degrades silently to a read-only connection when
    // the file itself is not writable (e.g. mode 0400). That must not pass:
    // every later write — memory included — would fail far from this check.
    configure(&conn, &db_path)?;

    // Identity first, read-only: if an existing database is bound to another
    // project, refuse before any write is even attempted, so even a copied
    // database whose migration state looks stale or absent is left
    // byte-for-byte unmodified by the failed attempt.
    verify_identity(&conn, &db_path, project_id)?;

    // One BEGIN IMMEDIATE transaction from before the first schema statement
    // until after the project binding: concurrent first launches serialize on
    // SQLite's write lock instead of racing between "read version" and
    // "create table" or between "query binding" and "insert binding". Losers
    // of the lock wait here (bounded by the busy timeout), then see the
    // winner's committed state and proceed idempotently.
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|source| DatabaseError::Sql {
            path: db_path.clone(),
            source,
        })?;

    migrate(&tx, &db_path)?;
    bind_project(&tx, &db_path, project_id)?;

    tx.commit().map_err(|source| DatabaseError::Sql {
        path: db_path.clone(),
        source,
    })?;

    Ok(conn)
}

/// Per-connection configuration that must hold before any work happens.
fn configure(conn: &Connection, db_path: &Path) -> Result<(), DatabaseError> {
    let configure_err = |source| DatabaseError::Configure {
        path: db_path.to_path_buf(),
        source,
    };

    // Bound wait instead of an immediate `database is locked` failure when
    // another Glasshouse process holds the write lock briefly.
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(configure_err)?;

    if conn.is_readonly("main").map_err(configure_err)? {
        return Err(DatabaseError::ReadOnly {
            path: db_path.to_path_buf(),
        });
    }

    Ok(())
}

/// Refuse an existing metadata table that is unbound or belongs to a different
/// project. A genuinely brand-new database has no metadata table yet and passes
/// straight through to migration and [`bind_project`].
fn verify_identity(
    conn: &Connection,
    db_path: &Path,
    project_id: &str,
) -> Result<(), DatabaseError> {
    let sql_err = |source| DatabaseError::Sql {
        path: db_path.to_path_buf(),
        source,
    };

    let table_present: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master \
             WHERE type = 'table' AND name = 'project_metadata'",
            [],
            |row| row.get(0),
        )
        .map_err(sql_err)?;
    if table_present == 0 {
        return Ok(());
    }

    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM project_metadata WHERE key = ?1",
            [PROJECT_ID_KEY],
            |row| row.get(0),
        )
        .optional()
        .map_err(sql_err)?;

    match stored {
        Some(actual) if actual != project_id => Err(DatabaseError::ProjectMismatch {
            path: db_path.to_path_buf(),
            expected: project_id.to_owned(),
            actual,
        }),
        Some(_) => Ok(()),
        None => Err(DatabaseError::MissingProjectId {
            path: db_path.to_path_buf(),
        }),
    }
}

/// Inspect the final database path; refuse symlinks and non-regular entries.
///
/// Returns `None` only when the path definitively does not exist (so the
/// caller should create it), `Some(metadata)` for an existing regular file.
/// Any other inspection failure — permission denied and friends — is
/// preserved with its source rather than being mistaken for permission to
/// create the file. Deliberately says nothing about the file's *length* —
/// see [`check_existing`], its only caller that also needs that judgment.
fn inspect_existing(db_path: &Path) -> Result<Option<fs::Metadata>, DatabaseError> {
    let metadata = match fs::symlink_metadata(db_path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(source) => {
            return Err(DatabaseError::Inspect {
                path: db_path.to_path_buf(),
                source,
            });
        }
    };
    if metadata.file_type().is_symlink() {
        return Err(DatabaseError::Symlinked {
            path: db_path.to_path_buf(),
        });
    }
    if !metadata.is_file() {
        // Anything that is not a regular file — a directory, a device, a
        // FIFO, a socket — must not be opened as (or replaced by) a
        // database. Special files in particular could block or misbehave
        // when SQLite tries to read and write them.
        return Err(DatabaseError::NotARegularFile {
            path: db_path.to_path_buf(),
            actual: describe_entry(&metadata),
        });
    }
    // An existing regular file keeps whatever permissions it has;
    // like `create_state_dir`, this call neither widens nor narrows.
    Ok(Some(metadata))
}

/// How long [`check_existing`] tolerates a zero-byte file before concluding
/// it is genuinely empty rather than mid-creation by a concurrent process.
/// SQLite grows a database file's page count as soon as its first `CREATE
/// TABLE` executes, well before that transaction commits, so this only needs
/// to cover "another process is a few instructions into `migrate`" — not a
/// whole migration. Bounded well under [`configure`]'s 5-second
/// `busy_timeout` so a genuinely empty file is still reported promptly.
const EMPTY_FILE_RETRIES: u32 = 50;
const EMPTY_FILE_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(10);

/// Inspect the final database path for the case where it is expected to
/// predate this launch: refuses symlinks and non-regular entries (via
/// [`inspect_existing`]), and additionally refuses a zero-byte existing
/// file, which is never what a genuinely new project looks like.
///
/// Returns `Ok(false)` only when the path definitively does not exist (so the
/// caller should create it), `Ok(true)` when an existing regular,
/// nonempty file is ready to open.
///
/// **Not** the right check for a file [`prepare_file`] just lost an
/// `AlreadyExists` race to create — that file is legitimately zero bytes
/// until the winning process's migration commits, and is not the "this used
/// to hold real data" case this function's empty-file refusal exists for.
/// That caller uses [`inspect_existing`] directly instead.
///
/// A zero-byte file found *here* is not automatically that same in-flight
/// case, though: a straggler among several processes racing this exact
/// function (see `concurrent_first_bootstraps_serialize_on_one_database`)
/// can observe a sibling's just-created, not-yet-migrated file the same way.
/// Retrying briefly before refusing tells the two apart: a fresh file grows
/// past zero bytes within milliseconds once its creator's migration starts
/// writing; a genuinely truncated one never does.
fn check_existing(db_path: &Path) -> Result<bool, DatabaseError> {
    let Some(mut metadata) = inspect_existing(db_path)? else {
        return Ok(false);
    };
    if metadata.len() == 0 {
        for _ in 0..EMPTY_FILE_RETRIES {
            std::thread::sleep(EMPTY_FILE_RETRY_DELAY);
            match inspect_existing(db_path)? {
                Some(retried) if retried.len() > 0 => return Ok(true),
                Some(retried) => metadata = retried,
                // Vanished between retries: nothing left to refuse; the
                // caller should create it.
                None => return Ok(false),
            }
        }
    }
    if metadata.len() == 0 {
        // A zero-byte file is a valid *empty* SQLite database by
        // specification, so nothing downstream — not SQLite, not `migrate`
        // — would ever notice this used to hold a project's sessions,
        // memories, and checkpoints. A genuinely new project has no file
        // here at all; an *existing* file of length zero that stayed zero
        // through the retries above is never that, so this is the one case
        // this function must refuse rather than let fall through to a
        // silent fresh migration.
        return Err(DatabaseError::EmptyExisting {
            path: db_path.to_path_buf(),
        });
    }
    Ok(true)
}

/// Human-readable kind of a final-path entry, for error messages.
fn describe_entry(metadata: &fs::Metadata) -> &'static str {
    let file_type = metadata.file_type();
    if file_type.is_dir() {
        "a directory"
    } else if file_type.is_file() {
        "a regular file"
    } else if file_type.is_symlink() {
        "a symbolic link"
    } else {
        "a special file (device, FIFO, socket, ...)"
    }
}

/// Make sure a regular file exists at `db_path`, created owner-only if new,
/// without following a symlink that may sit at the final component.
///
/// Only a definitive `NotFound` from the inspection counts as "absent"; any
/// other failure is preserved with its source instead of being mistaken for
/// permission to create the file. If creation loses an `AlreadyExists` race
/// with another Glasshouse process, the winning file is re-inspected — it
/// gets no free pass past the symlink refusal.
fn prepare_file(db_path: &Path) -> Result<(), DatabaseError> {
    match check_existing(db_path) {
        Ok(true) => return Ok(()),
        Ok(false) => {}
        Err(err) => return Err(err),
    }

    // Create the file ourselves instead of letting SQLite do it, because
    // SQLite would use plain `0644 &! umask` — world-readable, which no
    // project memory ever should be.
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(db_path) {
        Ok(_) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            // Lost the race to a concurrent Glasshouse process creating this
            // same file right now. That winner's file is legitimately zero
            // bytes until its migration commits under the write lock both
            // processes will serialize on in `open` — this is not the
            // "existing file that used to hold data" case `check_existing`'s
            // empty-file refusal exists for, so hold the winner only to the
            // symlink/regular-file checks, not that one.
            inspect_existing(db_path).map(|_| ())
        }
        Err(source) => Err(DatabaseError::Create {
            path: db_path.to_path_buf(),
            source,
        }),
    }
}

/// Apply pending migrations deterministically and refuse anything this build
/// cannot handle.
///
/// Runs inside the caller's `BEGIN IMMEDIATE` transaction: the ledger is
/// created, read, and advanced under SQLite's write lock, so two concurrent
/// first launches can never interleave "read version 0" with "create table".
/// No commit happens here; [`ensure_ready`] commits once after the project
/// binding is also in place.
fn migrate(conn: &Connection, db_path: &Path) -> Result<(), DatabaseError> {
    let sql_err = |source| DatabaseError::Sql {
        path: db_path.to_path_buf(),
        source,
    };

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
             version INTEGER PRIMARY KEY
         );",
    )
    .map_err(sql_err)?;

    let current: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )
        .map_err(sql_err)?;

    if current > SUPPORTED_SCHEMA_VERSION {
        return Err(DatabaseError::TooNew {
            path: db_path.to_path_buf(),
            found: current,
            supported: SUPPORTED_SCHEMA_VERSION,
        });
    }

    for (index, script) in MIGRATIONS.iter().enumerate() {
        let version = index as i64 + 1;
        if version <= current {
            continue;
        }
        conn.execute_batch(script).map_err(sql_err)?;
        conn.execute(
            "INSERT INTO schema_migrations (version) VALUES (?1)",
            [version],
        )
        .map_err(sql_err)?;
    }

    Ok(())
}

/// Bind the database to the active project, or verify an existing binding.
///
/// Runs inside the caller's `BEGIN IMMEDIATE` transaction, so the
/// "query binding, then insert if absent" pair cannot interleave with another
/// launcher's. A stored identifier that differs from the active one means
/// this file was copied or moved across projects; opening it would silently
/// merge two projects' memories, so it is refused instead.
fn bind_project(conn: &Connection, db_path: &Path, project_id: &str) -> Result<(), DatabaseError> {
    let sql_err = |source| DatabaseError::Sql {
        path: db_path.to_path_buf(),
        source,
    };

    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM project_metadata WHERE key = ?1",
            [PROJECT_ID_KEY],
            |row| row.get(0),
        )
        .optional()
        .map_err(sql_err)?;

    match stored {
        Some(actual) if actual != project_id => Err(DatabaseError::ProjectMismatch {
            path: db_path.to_path_buf(),
            expected: project_id.to_owned(),
            actual,
        }),
        Some(_) => Ok(()),
        None => {
            conn.execute(
                "INSERT INTO project_metadata (key, value) VALUES (?1, ?2)",
                [PROJECT_ID_KEY, project_id],
            )
            .map_err(sql_err)?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cli, Runtime};
    use clap::Parser;
    use std::path::PathBuf;

    /// A project rooted inside `base`'s `workspace/`, bootstrapped against
    /// `base`'s `data/` and `config/`. Fixtures sharing one `base` therefore
    /// share one GLASSHOUSE data/config root, like two real projects on one
    /// machine.
    struct Fixture {
        base: PathBuf,
        runtime: Runtime,
    }

    impl Fixture {
        fn new(base: &Path, name: &str) -> Self {
            let root = base.join("workspace").join(name);
            std::fs::create_dir_all(root.join(".git")).unwrap();
            let root = std::fs::canonicalize(&root).unwrap();

            let cli = Cli::try_parse_from([
                "glasshouse",
                "--data-dir",
                base.join("data").to_str().unwrap(),
                "--config-dir",
                base.join("config").to_str().unwrap(),
            ])
            .unwrap();
            let runtime = crate::bootstrap(&cli, &root).unwrap();
            Fixture {
                base: base.to_path_buf(),
                runtime,
            }
        }

        /// Bootstrap the same project again, exactly as a later launch would.
        fn rebootstrap(&self) -> anyhow::Result<Runtime> {
            let cli = Cli::try_parse_from([
                "glasshouse",
                "--data-dir",
                self.base.join("data").to_str().unwrap(),
                "--config-dir",
                self.base.join("config").to_str().unwrap(),
            ])
            .unwrap();
            crate::bootstrap(&cli, self.runtime.project().root())
        }
    }

    /// Undo every migration above 13, for a fixture that claims to be an older
    /// database.
    ///
    /// A rollback fixture has to undo **every** migration above the version it
    /// claims to be, not only the one it is about. Migration 14 arrived after
    /// three of these were written, and each of them failed the re-run with
    /// `duplicate column name: seq` until it was added here — which is why
    /// this is one constant rather than three copies for the next migration to
    /// miss. Migration 15 was appended for the same reason and cost nothing,
    /// which is the point of the constant existing.
    ///
    /// Dropping a table takes its indexes and triggers with it, so migration
    /// 15 is one statement. Migration 14 is not: SQLite refuses to drop a
    /// column an index mentions, so its indexes go first, and
    /// `checkpoints_by_session` is put back the way migration 5 left it.
    /// Migration 16 is one statement for the opposite reason: nothing indexes
    /// `observed_compactions`, and a column-scoped `CHECK` goes with the
    /// column it is written on. Migration 17 is one statement for migration
    /// 15's reason — dropping `memory_files` takes its index and its two
    /// triggers with it — and it goes first among the tables, because the
    /// rollback runs newest-migration-first for the same reason the ladder
    /// runs oldest-first. Migration 18 is one statement for migration 16's
    /// reason — nothing indexes `failure_class` and it carries no `CHECK` —
    /// and it goes before all of them, being the newest. Migration 19 is two
    /// statements for migration 15's reason — each table takes its indexes
    /// and triggers with it — and migration 20 is one for migration 16's:
    /// nothing indexes `presentation_ref` and it carries no `CHECK`.
    /// Migrations 21 and 22 are each one statement for the same reason —
    /// nothing indexes `last_seen_commit`, `extraction_trigger` or
    /// `entitlement` and none of the three carries a `CHECK`. Migration 23 is
    /// one statement for the same reason — nothing indexes `task_class` and
    /// it carries no `CHECK`. Migration 24 is three for the same reason
    /// again — nothing indexes `session_id`, `effort_level` or `turn_shape`
    /// and none of the three carries a `CHECK` or a `REFERENCES`. Migration
    /// 25 is four statements, and it is migration 16's reason rather than
    /// 23's: nothing indexes the four millisecond offsets, and each of them
    /// *does* carry a `CHECK` — a column-scoped one, which SQLite drops with
    /// the column it is written on. Newest first, so 25's four lead and
    /// 24's three follow, each set in the reverse of the order it was
    /// added.
    const UNDO_MIGRATIONS_ABOVE_THIRTEEN: &str = "
        ALTER TABLE routing_observations DROP COLUMN completed_ms;
        ALTER TABLE routing_observations DROP COLUMN first_tool_call_ms;
        ALTER TABLE routing_observations DROP COLUMN first_token_ms;
        ALTER TABLE routing_observations DROP COLUMN first_byte_ms;
        ALTER TABLE routing_observations DROP COLUMN turn_shape;
        ALTER TABLE routing_observations DROP COLUMN effort_level;
        ALTER TABLE routing_observations DROP COLUMN session_id;
        ALTER TABLE routing_observations DROP COLUMN task_class;
        ALTER TABLE sessions DROP COLUMN entitlement;
        ALTER TABLE memories DROP COLUMN extraction_trigger;
        ALTER TABLE sessions DROP COLUMN last_seen_commit;
        ALTER TABLE sessions DROP COLUMN presentation_ref;

        DROP TABLE assumption_transitions;
        DROP TABLE task_assumptions;

        ALTER TABLE routing_observations DROP COLUMN failure_class;

        DROP TABLE memory_files;

        ALTER TABLE sessions DROP COLUMN observed_compactions;

        DROP TABLE evaluation_observations;

        DROP INDEX checkpoints_by_seq;
        DROP INDEX checkpoints_by_session;
        ALTER TABLE checkpoints DROP COLUMN seq;
        CREATE INDEX checkpoints_by_session
            ON checkpoints (session_id, created_at DESC);
    ";

    fn stored_project_id(db_path: &Path) -> String {
        let conn = Connection::open(db_path).unwrap();
        conn.query_row(
            "SELECT value FROM project_metadata WHERE key = 'project_id'",
            [],
            |row| row.get(0),
        )
        .unwrap()
    }

    fn schema_version(db_path: &Path) -> i64 {
        let conn = Connection::open(db_path).unwrap();
        conn.query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
            row.get(0)
        })
        .unwrap()
    }

    /// A phase the type can produce and the schema will not accept is a
    /// constraint violation at the moment a memory is stored, on whichever
    /// thread happens to be extracting. Migration 6's `CHECK` is the
    /// authority, so this reads the list **out of the migration itself**
    /// rather than out of a second constant beside it: a constant can drift
    /// from the SQL, and then the pin proves only that two Rust literals
    /// still agree.
    #[test]
    fn every_project_phase_the_type_supports_is_one_the_schema_accepts() {
        use crate::memory::ProjectPhase;

        let migration = MIGRATIONS[5];
        let marker = "project_phase IN";
        let open = migration
            .find(marker)
            .expect("migration 6 checks the phase")
            + marker.len();
        let list = &migration[open..];
        let list = &list[..list.find(')').expect("the CHECK's list is parenthesised")];
        let accepted: Vec<String> = list
            .split(',')
            .map(|value| value.trim().trim_matches(['(', ' ', '\n', '\'']).to_owned())
            .filter(|value| !value.is_empty())
            .collect();

        let declared: Vec<String> = ProjectPhase::ALL
            .iter()
            .map(|phase| phase.as_str().to_owned())
            .collect();

        assert_eq!(
            declared, accepted,
            "a project phase was added or renamed without migration 6's CHECK"
        );

        // And the parse has to be able to fail, or it asserts nothing: the
        // map's own list is five long, so a `CHECK` this failed to read
        // would show up here as an empty vector rather than as a pass.
        assert_eq!(accepted.len(), 5, "the CHECK's list was not read correctly");
    }

    /// Migration 17's `provenance` carries **no** `CHECK`, so nothing in SQL
    /// pins it — this test is the guarantee, exactly as
    /// `EVALUATION_KINDS`' own pinning test is for
    /// `evaluation_observations.kind`.
    ///
    /// Two independently written spellings: [`MEMORY_FILE_PROVENANCE`], which
    /// sits beside the migration where a schema reader looks, and
    /// [`crate::memory::FileAssociation`], which is what the writer actually
    /// stores. Neither is derived from the other — that is the whole point,
    /// and it is why this is not a tautology.
    ///
    /// **The one that must never appear here is `referenced`.** Migration
    /// 17's own text and `FileAssociation::Observed`'s both say why: this
    /// build observes which files were dirty, and calling that a reference
    /// would close capability-map line 1139 on a producer that does not
    /// exist. A future package may add the value — beside this one, with its
    /// own producer — and this test is where it has to be declared.
    #[test]
    fn every_file_association_the_type_supports_is_one_the_schema_records() {
        use crate::memory::FileAssociation;

        let declared: Vec<&str> = FileAssociation::ALL
            .iter()
            .map(|association| association.as_str())
            .collect();
        assert_eq!(
            declared,
            MEMORY_FILE_PROVENANCE.to_vec(),
            "a memory-file provenance was added or renamed on one side only"
        );

        // Every declared value must survive a round trip, or a row this build
        // wrote is a row it cannot read back.
        for association in FileAssociation::ALL {
            assert_eq!(
                FileAssociation::from_stored(association.as_str()),
                Some(*association)
            );
        }
        assert_eq!(FileAssociation::from_stored("referenced"), None);
        assert_eq!(FileAssociation::from_stored(""), None);
    }

    /// Migration 18's `failure_class` carries **no** `CHECK`, so nothing in
    /// SQL pins it — this test is the guarantee, exactly as
    /// `EVALUATION_KINDS`' and [`MEMORY_FILE_PROVENANCE`]'s own are.
    ///
    /// Two independently written spellings: [`FAILURE_CLASSES`], beside the
    /// migration where a schema reader looks, and
    /// [`crate::routing::evidence::FailureClass`], which the writer stores.
    /// Neither is derived from the other, which is why this is not a
    /// tautology. Nine, in capability map line 1364's own order.
    #[test]
    fn every_failure_class_the_type_supports_is_one_the_schema_records() {
        use crate::routing::evidence::FailureClass;

        let declared: Vec<&str> = FailureClass::ALL
            .iter()
            .map(|class| class.as_str())
            .collect();
        assert_eq!(
            declared,
            FAILURE_CLASSES.to_vec(),
            "a failure class was added, renamed or reordered on one side only"
        );
        assert_eq!(FAILURE_CLASSES.len(), 9, "the map line names nine");

        for class in FailureClass::ALL {
            assert_eq!(FailureClass::from_stored(class.as_str()), Some(class));
        }
        // A spelling nothing writes reads as unrecognised, never as a class.
        assert_eq!(FailureClass::from_stored("rate_limited"), None);
        assert_eq!(FailureClass::from_stored(""), None);
    }

    /// Migration 23's `task_class` carries **no** `CHECK`, so nothing in SQL
    /// pins it — this test is the guarantee, exactly as
    /// `every_failure_class_the_type_supports_is_one_the_schema_records` is
    /// for migration 18.
    ///
    /// Two independently written spellings: [`TASK_CLASSES`], beside the
    /// migration where a schema reader looks, and
    /// [`crate::routing::request::TaskClass`], which the writer stores.
    /// Neither is derived from the other.
    #[test]
    fn every_task_class_the_type_supports_is_one_the_schema_records() {
        use crate::routing::request::TaskClass;

        let declared: Vec<&str> = TaskClass::ALL.iter().map(|class| class.as_str()).collect();
        assert_eq!(
            declared,
            TASK_CLASSES.to_vec(),
            "a task class was added, renamed or reordered on one side only"
        );
        assert_eq!(TASK_CLASSES.len(), 5, "the type declares five");

        for class in TaskClass::ALL {
            assert_eq!(TaskClass::from_stored(class.as_str()), Some(class));
        }
        // An unrecognised word reads as no class — never an error, and never
        // a class this build invented. Migration 23's own doc comment says
        // why this differs from `failure_class`.
        assert_eq!(TaskClass::from_stored("code_modification"), None);
        assert_eq!(TaskClass::from_stored(""), None);
    }

    /// The column names of `table`, in declaration order.
    fn columns_of(conn: &Connection, table: &str) -> Vec<String> {
        let mut statement = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap();
        statement
            .query_map([], |row| row.get::<_, String>("name"))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    /// Everything `sqlite_master` holds, in a stable order — the whole
    /// schema as one comparable value.
    fn whole_schema(conn: &Connection) -> Vec<(String, String, Option<String>)> {
        let mut statement = conn
            .prepare("SELECT type, name, sql FROM sqlite_master ORDER BY type, name")
            .unwrap();
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    /// Migration proof for 18: a version-17 database that already holds a
    /// routing observation opens, migrates to 18 adding exactly one column,
    /// reads the old row's `failure_class` as unknown rather than as a class,
    /// records a classified row through the real writer, and the undo takes
    /// the whole schema back to exactly what it was — every table, index and
    /// trigger.
    ///
    /// One connection at a time throughout (practice §65): every handle is
    /// dropped before the next is opened and before the re-bootstrap.
    #[test]
    fn migration_18_adds_failure_class_and_undoes_cleanly() {
        use crate::routing::evidence::{
            EvidenceLedger, FailureClass, NewObservation, ObservationQuery, Outcome,
        };

        // Migrations 20 and 19 are undone first: a rollback undoes every
        // migration above the version it claims, or the re-run fails with
        // `duplicate column name` — `UNDO_MIGRATIONS_ABOVE_THIRTEEN`'s own
        // lesson.
        const UNDO_18: &str = "
            ALTER TABLE routing_observations DROP COLUMN completed_ms;
            ALTER TABLE routing_observations DROP COLUMN first_tool_call_ms;
            ALTER TABLE routing_observations DROP COLUMN first_token_ms;
            ALTER TABLE routing_observations DROP COLUMN first_byte_ms;
            ALTER TABLE routing_observations DROP COLUMN turn_shape;
            ALTER TABLE routing_observations DROP COLUMN effort_level;
            ALTER TABLE routing_observations DROP COLUMN session_id;
            ALTER TABLE routing_observations DROP COLUMN task_class;
            ALTER TABLE sessions DROP COLUMN entitlement;
            ALTER TABLE memories DROP COLUMN extraction_trigger;
            ALTER TABLE sessions DROP COLUMN last_seen_commit;
            ALTER TABLE sessions DROP COLUMN presentation_ref;
            DROP TABLE assumption_transitions;
            DROP TABLE task_assumptions;
            ALTER TABLE routing_observations DROP COLUMN failure_class;
            DELETE FROM schema_migrations WHERE version >= 18;
        ";

        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let db_path = fixture.runtime.database_path();
        let project_id = stored_project_id(&db_path);

        // Back to 17, with a row written the way a version-17 build wrote
        // them — no `failure_class` to name.
        let (schema_at_17, columns_at_17) = {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(UNDO_18).unwrap();
            conn.execute(
                "INSERT INTO routing_observations (project_id, observed_at, provider, model, outcome)
                 VALUES (?1, 1, 'pre-migration', 'm', 'failed')",
                [&project_id],
            )
            .unwrap();
            (
                whole_schema(&conn),
                columns_of(&conn, "routing_observations"),
            )
        };
        assert_eq!(schema_version(&db_path), 17, "the rollback must land on 17");
        assert!(
            !columns_at_17.iter().any(|column| column == "failure_class"),
            "{columns_at_17:?}"
        );

        // Forward: an ordinary bootstrap, exactly as a real upgrade happens.
        let migrated = fixture.rebootstrap().unwrap();
        assert_eq!(
            schema_version(&migrated.database_path()),
            SUPPORTED_SCHEMA_VERSION,
            "the launch must have applied migration 18 and everything above it"
        );
        {
            let conn = Connection::open(&db_path).unwrap();
            let columns = columns_of(&conn, "routing_observations");
            let mut expected = columns_at_17.clone();
            expected.push("failure_class".to_owned());
            // Migrations 23, 24 and 25 append to the same table, so a
            // forward run from 17 now lands nine columns rather than one.
            // All nine are asserted by name and in order, which is the
            // property this test was always about — migration 18 appended
            // `failure_class` and rebuilt nothing.
            expected.push("task_class".to_owned());
            expected.push("session_id".to_owned());
            expected.push("effort_level".to_owned());
            expected.push("turn_shape".to_owned());
            expected.push("first_byte_ms".to_owned());
            expected.push("first_token_ms".to_owned());
            expected.push("first_tool_call_ms".to_owned());
            expected.push("completed_ms".to_owned());
            assert_eq!(columns, expected, "exactly nine columns, all appended");
        }

        // The pre-migration row reads as *unknown kind*, never as a class,
        // and a row written now carries the class it was given.
        {
            let ledger = EvidenceLedger::open(&migrated).unwrap();
            let pre = ledger
                .recent(
                    ObservationQuery {
                        provider: "pre-migration",
                        model: "m",
                        route: None,
                        harness: None,
                    },
                    1,
                )
                .unwrap();
            assert_eq!(pre.len(), 1);
            assert_eq!(pre[0].outcome, Some(Outcome::Failed));
            assert_eq!(
                pre[0].failure_class, None,
                "a row from before the column existed has no kind, not an `unknown` kind"
            );

            ledger
                .record(
                    NewObservation::new("post-migration", "m")
                        .with_outcome(Outcome::Failed)
                        .with_failure_class(Some(FailureClass::Throttle)),
                    2,
                )
                .unwrap();
            let post = ledger
                .recent(
                    ObservationQuery {
                        provider: "post-migration",
                        model: "m",
                        route: None,
                        harness: None,
                    },
                    1,
                )
                .unwrap();
            assert_eq!(post[0].failure_class, Some(FailureClass::Throttle));
        }

        // Back again: the whole schema is what it was at 17, byte for byte,
        // and the rows are still there.
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(UNDO_18).unwrap();
            assert_eq!(whole_schema(&conn), schema_at_17);
            assert_eq!(columns_of(&conn, "routing_observations"), columns_at_17);
            let rows: i64 = conn
                .query_row("SELECT COUNT(*) FROM routing_observations", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(rows, 2, "dropping the column drops no rows");
        }
        assert_eq!(schema_version(&db_path), 17);
    }

    /// Migration proof for 23: a version-22 database that already holds a
    /// routing observation opens, migrates to 23 adding exactly one column,
    /// reads the old row's `task_class` as unnamed rather than as a class,
    /// records a classified row through the real writer, reads an
    /// unrecognised stored word back as `None` **rather than as an error**
    /// (migration 18's one deliberate difference), and the undo takes the
    /// whole schema back to exactly what it was — every table, index and
    /// trigger, the two project-scope triggers included.
    ///
    /// One connection at a time throughout (practice §65): every handle is
    /// dropped before the next is opened and before the re-bootstrap.
    #[test]
    fn migration_23_adds_task_class_and_undoes_cleanly() {
        use crate::routing::evidence::{EvidenceLedger, NewObservation, ObservationQuery, Outcome};
        use crate::routing::request::TaskClass;

        const UNDO_23: &str = "
            ALTER TABLE routing_observations DROP COLUMN completed_ms;
            ALTER TABLE routing_observations DROP COLUMN first_tool_call_ms;
            ALTER TABLE routing_observations DROP COLUMN first_token_ms;
            ALTER TABLE routing_observations DROP COLUMN first_byte_ms;
            ALTER TABLE routing_observations DROP COLUMN turn_shape;
            ALTER TABLE routing_observations DROP COLUMN effort_level;
            ALTER TABLE routing_observations DROP COLUMN session_id;
            ALTER TABLE routing_observations DROP COLUMN task_class;
            DELETE FROM schema_migrations WHERE version >= 23;
        ";

        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let db_path = fixture.runtime.database_path();
        let project_id = stored_project_id(&db_path);

        // Back to 22, with a row written the way a version-22 build wrote
        // them — no `task_class` to name.
        let (schema_at_22, columns_at_22) = {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(UNDO_23).unwrap();
            conn.execute(
                "INSERT INTO routing_observations (project_id, observed_at, provider, model, outcome)
                 VALUES (?1, 1, 'pre-migration', 'm', 'succeeded')",
                [&project_id],
            )
            .unwrap();
            (
                whole_schema(&conn),
                columns_of(&conn, "routing_observations"),
            )
        };
        assert_eq!(schema_version(&db_path), 22, "the rollback must land on 22");
        assert!(
            !columns_at_22.iter().any(|column| column == "task_class"),
            "{columns_at_22:?}"
        );

        // Forward: an ordinary bootstrap, exactly as a real upgrade happens.
        let migrated = fixture.rebootstrap().unwrap();
        assert_eq!(
            schema_version(&migrated.database_path()),
            SUPPORTED_SCHEMA_VERSION,
            "the launch must have applied migration 23"
        );
        assert_eq!(
            SUPPORTED_SCHEMA_VERSION, 25,
            "a fresh database reports the version the newest migration ships"
        );
        {
            let conn = Connection::open(&db_path).unwrap();
            let columns = columns_of(&conn, "routing_observations");
            let mut expected = columns_at_22.clone();
            expected.push("task_class".to_owned());
            // Migration 24's own three and migration 25's own four ride
            // along on this bootstrap, in the order they add them; 23's
            // column is still the first appended.
            expected.push("session_id".to_owned());
            expected.push("effort_level".to_owned());
            expected.push("turn_shape".to_owned());
            expected.push("first_byte_ms".to_owned());
            expected.push("first_token_ms".to_owned());
            expected.push("first_tool_call_ms".to_owned());
            expected.push("completed_ms".to_owned());
            assert_eq!(
                columns, expected,
                "23's column, then 24's three, then 25's four, appended"
            );
        }

        // The pre-migration row names no class; a row written now carries
        // the class it was given.
        {
            let ledger = EvidenceLedger::open(&migrated).unwrap();
            let query = |provider| ObservationQuery {
                provider,
                model: "m",
                route: None,
                harness: None,
            };
            let pre = ledger.recent(query("pre-migration"), 1).unwrap();
            assert_eq!(pre.len(), 1);
            assert_eq!(
                pre[0].task_class, None,
                "a row from before the column existed names no class, not an `unknown` class"
            );

            ledger
                .record(
                    NewObservation::new("post-migration", "m")
                        .with_outcome(Outcome::Succeeded)
                        .with_task_class(Some(TaskClass::CodeModification)),
                    2,
                )
                .unwrap();
            let post = ledger.recent(query("post-migration"), 1).unwrap();
            assert_eq!(post[0].task_class, Some(TaskClass::CodeModification));
        }

        // A word this build does not recognise reads back as *no class*, and
        // — the property that separates this column from `failure_class` —
        // the row itself still reads. An `UnknownValue` here would let a
        // future build's sixth class break this build's burn rate.
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute(
                "INSERT INTO routing_observations
                     (project_id, observed_at, provider, model, outcome, task_class)
                 VALUES (?1, 3, 'future-build', 'm', 'succeeded', 'quantum tinkering')",
                [&project_id],
            )
            .unwrap();
        }
        {
            let ledger = EvidenceLedger::open(&migrated).unwrap();
            let future = ledger
                .recent(
                    ObservationQuery {
                        provider: "future-build",
                        model: "m",
                        route: None,
                        harness: None,
                    },
                    1,
                )
                .unwrap();
            assert_eq!(future.len(), 1, "the row reads, it does not error");
            assert_eq!(future[0].task_class, None);
            assert_eq!(future[0].outcome, Some(Outcome::Succeeded));
        }

        // Project isolation survives 23 → 22 → 23: `ADD COLUMN` does not
        // drop a trigger, and neither does `DROP COLUMN`, but the schema
        // comparison below is what proves it rather than the claim.
        {
            let conn = Connection::open(&db_path).unwrap();
            let refused = conn.execute(
                "INSERT INTO routing_observations (project_id, observed_at, provider, model)
                 VALUES ('another-project', 4, 'p', 'm')",
                [],
            );
            assert!(
                refused.is_err(),
                "the foreign-project trigger must still refuse after 22 → 23"
            );
        }

        // Back again: the whole schema is what it was at 22, byte for byte,
        // and the rows are still there.
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(UNDO_23).unwrap();
            assert_eq!(whole_schema(&conn), schema_at_22);
            assert_eq!(columns_of(&conn, "routing_observations"), columns_at_22);
            let rows: i64 = conn
                .query_row("SELECT COUNT(*) FROM routing_observations", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(rows, 3, "dropping the column drops no rows");
            let refused = conn.execute(
                "INSERT INTO routing_observations (project_id, observed_at, provider, model)
                 VALUES ('another-project', 5, 'p', 'm')",
                [],
            );
            assert!(
                refused.is_err(),
                "the foreign-project trigger must still refuse after 23 → 22"
            );
        }
        assert_eq!(schema_version(&db_path), 22);
    }

    /// Migration proof for 24: a version-23 database with a row written the
    /// way a version-23 build wrote them opens, migrates to 24 adding
    /// exactly three columns in the order the migration names them, reads
    /// that pre-migration row back with `NULL` in all three, records a
    /// stamped row through the real writer, reads an unrecognised
    /// `effort_level` and an unrecognised `turn_shape` back as `None`
    /// **rather than as an error** (migration 23's rule, not 18's), and the
    /// undo takes the whole schema back to exactly what it was — every
    /// table, index and trigger, the two project-scope triggers included.
    ///
    /// One connection at a time throughout (practice §65).
    #[test]
    fn migration_24_adds_the_session_columns_and_undoes_cleanly() {
        use crate::routing::evidence::{
            EffortLevel, EvidenceLedger, NewObservation, ObservationQuery, Outcome, TurnShape,
        };

        const UNDO_24: &str = "
            ALTER TABLE routing_observations DROP COLUMN completed_ms;
            ALTER TABLE routing_observations DROP COLUMN first_tool_call_ms;
            ALTER TABLE routing_observations DROP COLUMN first_token_ms;
            ALTER TABLE routing_observations DROP COLUMN first_byte_ms;
            ALTER TABLE routing_observations DROP COLUMN turn_shape;
            ALTER TABLE routing_observations DROP COLUMN effort_level;
            ALTER TABLE routing_observations DROP COLUMN session_id;
            DELETE FROM schema_migrations WHERE version >= 24;
        ";

        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let db_path = fixture.runtime.database_path();
        let project_id = stored_project_id(&db_path);

        // Back to 23, with a row written the way a version-23 build wrote
        // them — no session, no effort and no shape to name.
        let (schema_at_23, columns_at_23) = {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(UNDO_24).unwrap();
            conn.execute(
                "INSERT INTO routing_observations (project_id, observed_at, provider, model, outcome)
                 VALUES (?1, 1, 'pre-migration', 'm', 'succeeded')",
                [&project_id],
            )
            .unwrap();
            (
                whole_schema(&conn),
                columns_of(&conn, "routing_observations"),
            )
        };
        assert_eq!(schema_version(&db_path), 23, "the rollback must land on 23");
        for column in ["session_id", "effort_level", "turn_shape"] {
            assert!(
                !columns_at_23.iter().any(|held| held == column),
                "{columns_at_23:?}"
            );
        }

        // Forward: an ordinary bootstrap, exactly as a real upgrade happens.
        let migrated = fixture.rebootstrap().unwrap();
        assert_eq!(
            schema_version(&migrated.database_path()),
            SUPPORTED_SCHEMA_VERSION,
            "the launch must have applied migration 24"
        );
        assert_eq!(
            SUPPORTED_SCHEMA_VERSION, 25,
            "a fresh database reports the version the newest migration ships"
        );
        {
            let conn = Connection::open(&db_path).unwrap();
            let columns = columns_of(&conn, "routing_observations");
            let mut expected = columns_at_23.clone();
            expected.push("session_id".to_owned());
            expected.push("effort_level".to_owned());
            expected.push("turn_shape".to_owned());
            // Migration 25's own four ride along on this bootstrap, exactly
            // as 24's three ride along on migration 23's proof above.
            expected.push("first_byte_ms".to_owned());
            expected.push("first_token_ms".to_owned());
            expected.push("first_tool_call_ms".to_owned());
            expected.push("completed_ms".to_owned());
            assert_eq!(
                columns, expected,
                "exactly three columns, appended in order, then 25's four"
            );
        }

        // The pre-migration row names none of the three; a row written now
        // carries what it was given.
        {
            let ledger = EvidenceLedger::open(&migrated).unwrap();
            let query = |provider| ObservationQuery {
                provider,
                model: "m",
                route: None,
                harness: None,
            };
            let pre = ledger.recent(query("pre-migration"), 1).unwrap();
            assert_eq!(pre.len(), 1);
            assert_eq!(
                pre[0].session_id, None,
                "a row from before the column existed names no session, not an invented id"
            );
            assert_eq!(pre[0].effort_level, None);
            assert_eq!(pre[0].turn_shape, None);

            ledger
                .record(
                    NewObservation::new("post-migration", "m")
                        .with_outcome(Outcome::Succeeded)
                        .with_session_id(Some("ses_planted"))
                        .with_effort_level(Some(EffortLevel::Medium))
                        .with_turn_shape(Some(TurnShape::ToolResume)),
                    2,
                )
                .unwrap();
            let post = ledger.recent(query("post-migration"), 1).unwrap();
            assert_eq!(post[0].session_id.as_deref(), Some("ses_planted"));
            assert_eq!(post[0].effort_level, Some(EffortLevel::Medium));
            assert_eq!(post[0].turn_shape, Some(TurnShape::ToolResume));
        }

        // Words this build does not recognise read back as *nothing
        // recorded*, and — the property that separates these columns from
        // `failure_class` — the row itself still reads. An `UnknownValue`
        // here would let a future build's fifth effort word break this
        // build's savings readout.
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute(
                "INSERT INTO routing_observations
                     (project_id, observed_at, provider, model, outcome,
                      session_id, effort_level, turn_shape)
                 VALUES (?1, 3, 'future-build', 'm', 'succeeded',
                         'ses_future', 'transcendent', 'interpretive dance')",
                [&project_id],
            )
            .unwrap();
        }
        {
            let ledger = EvidenceLedger::open(&migrated).unwrap();
            let future = ledger
                .recent(
                    ObservationQuery {
                        provider: "future-build",
                        model: "m",
                        route: None,
                        harness: None,
                    },
                    1,
                )
                .unwrap();
            assert_eq!(future.len(), 1, "the row reads, it does not error");
            assert_eq!(future[0].effort_level, None);
            assert_eq!(future[0].turn_shape, None);
            assert_eq!(
                future[0].session_id.as_deref(),
                Some("ses_future"),
                "the session id has no vocabulary to fail against and is returned as stored"
            );
            assert_eq!(future[0].outcome, Some(Outcome::Succeeded));
        }

        // Project isolation survives 24 → 23 → 24.
        {
            let conn = Connection::open(&db_path).unwrap();
            let refused = conn.execute(
                "INSERT INTO routing_observations (project_id, observed_at, provider, model)
                 VALUES ('another-project', 4, 'p', 'm')",
                [],
            );
            assert!(
                refused.is_err(),
                "the foreign-project trigger must still refuse after 23 → 24"
            );
        }

        // Back again: the whole schema is what it was at 23, byte for byte,
        // and the rows are still there.
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(UNDO_24).unwrap();
            assert_eq!(whole_schema(&conn), schema_at_23);
            assert_eq!(columns_of(&conn, "routing_observations"), columns_at_23);
            let rows: i64 = conn
                .query_row("SELECT COUNT(*) FROM routing_observations", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(rows, 3, "dropping the columns drops no rows");
            let refused = conn.execute(
                "INSERT INTO routing_observations (project_id, observed_at, provider, model)
                 VALUES ('another-project', 5, 'p', 'm')",
                [],
            );
            assert!(
                refused.is_err(),
                "the foreign-project trigger must still refuse after 24 → 23"
            );
        }
        assert_eq!(schema_version(&db_path), 23);
    }

    /// Migration proof for 25: a version-24 database with a row written the
    /// way a version-24 build wrote them opens, migrates to 25 adding
    /// exactly four columns in the order the migration names them, reads
    /// that pre-migration row back with `None` in all four **and with
    /// `duration_ms` still answering from the seconds it does have**,
    /// records a measured row through the real writer, refuses a negative
    /// offset at the schema, and the undo takes the whole schema back to
    /// exactly what it was — every table, index and trigger, the two
    /// project-scope triggers included.
    ///
    /// One connection at a time throughout (practice §65).
    ///
    /// Mutation targets. `fallback-dropped`: making
    /// `RoutingObservation::duration_ms` answer `None` when `completed_ms`
    /// is `None` must fail the pre-migration row's assertion below.
    /// `migration-missing-check`: dropping the `CHECK` from any one of the
    /// four columns must fail the negative-offset refusal below.
    #[test]
    fn migration_25_adds_the_millisecond_offsets_and_undoes_cleanly() {
        use crate::routing::evidence::{EvidenceLedger, NewObservation, ObservationQuery, Outcome};

        const UNDO_25: &str = "
            ALTER TABLE routing_observations DROP COLUMN completed_ms;
            ALTER TABLE routing_observations DROP COLUMN first_tool_call_ms;
            ALTER TABLE routing_observations DROP COLUMN first_token_ms;
            ALTER TABLE routing_observations DROP COLUMN first_byte_ms;
            DELETE FROM schema_migrations WHERE version >= 25;
        ";

        const OFFSETS: [&str; 4] = [
            "first_byte_ms",
            "first_token_ms",
            "first_tool_call_ms",
            "completed_ms",
        ];

        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let db_path = fixture.runtime.database_path();
        let project_id = stored_project_id(&db_path);

        // Back to 24, with a row written the way a version-24 build wrote
        // them: both ends of the exchange in unix seconds and no offset
        // anywhere, because the columns did not exist.
        let (schema_at_24, columns_at_24) = {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(UNDO_25).unwrap();
            conn.execute(
                "INSERT INTO routing_observations
                     (project_id, observed_at, provider, model, outcome,
                      dispatched_at, completed_at)
                 VALUES (?1, 1, 'pre-migration', 'm', 'succeeded', 1000, 1007)",
                [&project_id],
            )
            .unwrap();
            (
                whole_schema(&conn),
                columns_of(&conn, "routing_observations"),
            )
        };
        assert_eq!(schema_version(&db_path), 24, "the rollback must land on 24");
        for column in OFFSETS {
            assert!(
                !columns_at_24.iter().any(|held| held == column),
                "{columns_at_24:?}"
            );
        }

        // Forward: an ordinary bootstrap, exactly as a real upgrade happens.
        let migrated = fixture.rebootstrap().unwrap();
        assert_eq!(
            schema_version(&migrated.database_path()),
            SUPPORTED_SCHEMA_VERSION,
            "the launch must have applied migration 25"
        );
        assert_eq!(
            SUPPORTED_SCHEMA_VERSION, 25,
            "a fresh database reports the version this migration ships"
        );
        {
            let conn = Connection::open(&db_path).unwrap();
            let columns = columns_of(&conn, "routing_observations");
            let mut expected = columns_at_24.clone();
            for column in OFFSETS {
                expected.push(column.to_owned());
            }
            assert_eq!(columns, expected, "exactly four columns, appended in order");
        }

        // The pre-migration row names none of the four — and still answers
        // `duration_ms` from the seconds it does carry, which is the whole
        // point of the fallback: every existing reader improves silently
        // where the offset exists and is unchanged where it does not.
        {
            let ledger = EvidenceLedger::open(&migrated).unwrap();
            let query = |provider| ObservationQuery {
                provider,
                model: "m",
                route: None,
                harness: None,
            };
            let pre = ledger.recent(query("pre-migration"), 1).unwrap();
            assert_eq!(pre.len(), 1);
            assert_eq!(
                pre[0].first_byte_ms, None,
                "a row from before the column existed measured nothing, and invents nothing"
            );
            assert_eq!(pre[0].first_token_ms, None);
            assert_eq!(pre[0].first_tool_call_ms, None);
            assert_eq!(pre[0].completed_ms, None);
            assert_eq!(
                pre[0].duration_ms(),
                Some(7_000),
                "with no measured completion the seconds difference is still the answer"
            );

            ledger
                .record(
                    NewObservation::new("post-migration", "m")
                        .with_outcome(Outcome::Succeeded)
                        .with_timing(Some(2_000), Some(2_009))
                        .with_first_byte_ms(Some(120))
                        .with_first_token_ms(Some(1_450))
                        .with_first_tool_call_ms(Some(2_600))
                        .with_completed_ms(Some(8_910)),
                    2,
                )
                .unwrap();
            let post = ledger.recent(query("post-migration"), 1).unwrap();
            assert_eq!(post[0].first_byte_ms, Some(120));
            assert_eq!(post[0].first_token_ms, Some(1_450));
            assert_eq!(post[0].first_tool_call_ms, Some(2_600));
            assert_eq!(post[0].completed_ms, Some(8_910));
            assert_eq!(
                post[0].duration_ms(),
                Some(8_910),
                "a measured completion is preferred over the 9,000ms the seconds would give"
            );
        }

        // The `CHECK` is the whole difference between these columns and
        // migrations 23 and 24's: a negative offset is not an unrecognised
        // word a later build might have meant, it is a reading no monotonic
        // clock can produce, and the schema refuses it one column at a time.
        {
            let conn = Connection::open(&db_path).unwrap();
            for column in OFFSETS {
                let refused = conn.execute(
                    &format!(
                        "INSERT INTO routing_observations
                             (project_id, observed_at, provider, model, {column})
                         VALUES (?1, 3, 'negative', 'm', -1)"
                    ),
                    [&project_id],
                );
                assert!(
                    refused.is_err(),
                    "`{column}` must refuse a negative offset at the schema"
                );
            }
        }

        // Project isolation survives 24 → 25.
        {
            let conn = Connection::open(&db_path).unwrap();
            let refused = conn.execute(
                "INSERT INTO routing_observations (project_id, observed_at, provider, model)
                 VALUES ('another-project', 4, 'p', 'm')",
                [],
            );
            assert!(
                refused.is_err(),
                "the foreign-project trigger must still refuse after 24 → 25"
            );
        }

        // Back again: the whole schema is what it was at 24, byte for byte —
        // which is also the proof that each column-scoped `CHECK` went with
        // the column it was written on, migration 16's rule.
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(UNDO_25).unwrap();
            assert_eq!(whole_schema(&conn), schema_at_24);
            assert_eq!(columns_of(&conn, "routing_observations"), columns_at_24);
            let rows: i64 = conn
                .query_row("SELECT COUNT(*) FROM routing_observations", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(rows, 2, "dropping the columns drops no rows");
            let refused = conn.execute(
                "INSERT INTO routing_observations (project_id, observed_at, provider, model)
                 VALUES ('another-project', 5, 'p', 'm')",
                [],
            );
            assert!(
                refused.is_err(),
                "the foreign-project trigger must still refuse after 25 → 24"
            );
        }
        assert_eq!(schema_version(&db_path), 24);
    }

    /// Migration proof for 19: a version-18 database opens, migrates to 19
    /// adding exactly two tables with their indexes and triggers, accepts an
    /// assumption and a transition through the real writer, refuses an edit
    /// to either, and the undo takes the whole schema back to exactly what
    /// it was — every table, index and trigger — with `schema_migrations`
    /// at 18.
    ///
    /// The trigger check is by name: migration 15's two scope triggers and
    /// one append-only trigger per table, and **no `DELETE` trigger** — a
    /// future migration that added one would quietly make a prunable ledger
    /// permanent, which is the defect migration 5 documents.
    ///
    /// One connection at a time throughout (practice §65).
    #[test]
    fn migration_19_adds_the_assumption_tables_and_undoes_cleanly() {
        use crate::guardrails::{
            AssumptionState, AssumptionStore, EvidenceSource, NewAssumption, NewTransition, Origin,
            Uncertainty,
        };

        // Reaches past 19, for `UNDO_18`'s reason exactly: this test rolls
        // back to 18 and lets an ordinary bootstrap migrate forward again, so
        // it must undo EVERY migration above 18. Leaving 20's column standing
        // lands an "18" that still has `presentation_ref`, and the re-bootstrap
        // fails with `duplicate column name` instead of proving anything about
        // 19. A migration 21 owes this constant its own line.
        const UNDO_19: &str = "
            ALTER TABLE routing_observations DROP COLUMN completed_ms;
            ALTER TABLE routing_observations DROP COLUMN first_tool_call_ms;
            ALTER TABLE routing_observations DROP COLUMN first_token_ms;
            ALTER TABLE routing_observations DROP COLUMN first_byte_ms;
            ALTER TABLE routing_observations DROP COLUMN turn_shape;
            ALTER TABLE routing_observations DROP COLUMN effort_level;
            ALTER TABLE routing_observations DROP COLUMN session_id;
            ALTER TABLE routing_observations DROP COLUMN task_class;
            ALTER TABLE sessions DROP COLUMN entitlement;
            ALTER TABLE memories DROP COLUMN extraction_trigger;
            ALTER TABLE sessions DROP COLUMN last_seen_commit;
            ALTER TABLE sessions DROP COLUMN presentation_ref;

            DROP TABLE assumption_transitions;
            DROP TABLE task_assumptions;
            DELETE FROM schema_migrations WHERE version >= 19;
        ";

        fn schema_of(conn: &Connection) -> Vec<(String, String, Option<String>)> {
            let mut statement = conn
                .prepare("SELECT type, name, sql FROM sqlite_master ORDER BY type, name")
                .unwrap();
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        }
        fn names_of(conn: &Connection, kind: &str, table: &str) -> Vec<String> {
            let mut statement = conn
                .prepare(
                    "SELECT name FROM sqlite_master WHERE type = ?1 AND tbl_name = ?2 \
                     ORDER BY name",
                )
                .unwrap();
            statement
                .query_map([kind, table], |row| row.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        }

        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let db_path = fixture.runtime.database_path();

        // Back to 18.
        let schema_at_18 = {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(UNDO_19).unwrap();
            schema_of(&conn)
        };
        assert_eq!(schema_version(&db_path), 18, "the rollback must land on 18");
        assert!(
            !schema_at_18
                .iter()
                .any(|(_, name, _)| name.starts_with("task_assumptions")
                    || name.starts_with("assumption_transitions")),
            "{schema_at_18:?}"
        );

        // Forward: an ordinary bootstrap, exactly as a real upgrade happens.
        let migrated = fixture.rebootstrap().unwrap();
        assert_eq!(
            schema_version(&migrated.database_path()),
            SUPPORTED_SCHEMA_VERSION,
            "the launch must have applied migration 19"
        );
        {
            let conn = Connection::open(&db_path).unwrap();
            assert_eq!(
                names_of(&conn, "table", "task_assumptions"),
                ["task_assumptions"]
            );
            assert_eq!(
                names_of(&conn, "table", "assumption_transitions"),
                ["assumption_transitions"]
            );
            assert_eq!(
                names_of(&conn, "index", "task_assumptions"),
                [
                    "sqlite_autoindex_task_assumptions_1",
                    "task_assumptions_by_session"
                ]
            );
            assert_eq!(
                names_of(&conn, "index", "assumption_transitions"),
                [
                    "assumption_transitions_by_assumption",
                    "assumption_transitions_by_session"
                ]
            );
            assert_eq!(
                names_of(&conn, "trigger", "task_assumptions"),
                [
                    "task_assumptions_never_edited",
                    "task_assumptions_reject_foreign_project_insert",
                    "task_assumptions_reject_foreign_project_update"
                ],
                "two scope triggers and one append-only trigger, and no DELETE trigger"
            );
            assert_eq!(
                names_of(&conn, "trigger", "assumption_transitions"),
                [
                    "assumption_transitions_append_only",
                    "assumption_transitions_reject_foreign_project_insert",
                    "assumption_transitions_reject_foreign_project_update"
                ]
            );
        }

        // The real writer, through the migrated schema.
        let recorded = {
            let mut store = AssumptionStore::open(&migrated).unwrap();
            let record = store
                .record(NewAssumption {
                    session: Some("s1".to_owned()),
                    claim: "written through migration 19".to_owned(),
                    evidence: "this test".to_owned(),
                    evidence_source: EvidenceSource::Experiment,
                    uncertainty: Uncertainty::Low,
                    affected: "database.rs".to_owned(),
                    verification: "the undo below".to_owned(),
                    origin: Origin::Agent,
                })
                .unwrap();
            let moved = store
                .transition(
                    &record.id,
                    NewTransition::to(AssumptionState::Refuted, Origin::Agent),
                )
                .unwrap();
            assert_eq!(moved.state, Some(AssumptionState::Refuted));
            assert_eq!(
                store.get(&record.id).unwrap().unwrap().state,
                AssumptionState::Refuted
            );
            record
        };
        {
            let conn = Connection::open(&db_path).unwrap();
            let err = conn
                .execute(
                    "UPDATE assumption_transitions SET state = 'supported' WHERE assumption_id = ?1",
                    [recorded.id.as_str()],
                )
                .unwrap_err();
            assert!(err.to_string().contains("append-only"), "{err}");
            let err = conn
                .execute(
                    "UPDATE task_assumptions SET claim = 'edited' WHERE id = ?1",
                    [recorded.id.as_str()],
                )
                .unwrap_err();
            assert!(err.to_string().contains("never edited"), "{err}");
            let rows: i64 = conn
                .query_row("SELECT COUNT(*) FROM assumption_transitions", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(rows, 2, "the first state and the move");
        }

        // Back again: the whole schema is what it was at 18, byte for byte.
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(UNDO_19).unwrap();
            assert_eq!(schema_of(&conn), schema_at_18);
        }
        assert_eq!(schema_version(&db_path), 18);
    }

    /// Migration proof for 20: a version-19 database that already holds a
    /// session written the way a version-19 build wrote it migrates forward
    /// adding exactly one column, appended; the old row reads as *no pane
    /// recorded*, never as an empty or invented reference; a row written now
    /// carries the reference it was given; and the undo takes the whole
    /// schema back to exactly what it was at 19, keeping every row.
    ///
    /// The `None` assertion is the one this migration most needs: a column
    /// written `NOT NULL DEFAULT ''` would pass every other check here and
    /// would hand `integrations::cmux::PaneRef::parse` an empty string for
    /// every session recorded before the upgrade.
    #[test]
    fn migration_20_adds_presentation_ref_and_undoes_cleanly() {
        use crate::session::{NewSession, ProjectSessions, SessionId, SessionPresentation};

        const UNDO_20: &str = "
            ALTER TABLE routing_observations DROP COLUMN completed_ms;
            ALTER TABLE routing_observations DROP COLUMN first_tool_call_ms;
            ALTER TABLE routing_observations DROP COLUMN first_token_ms;
            ALTER TABLE routing_observations DROP COLUMN first_byte_ms;
            ALTER TABLE routing_observations DROP COLUMN turn_shape;
            ALTER TABLE routing_observations DROP COLUMN effort_level;
            ALTER TABLE routing_observations DROP COLUMN session_id;
            ALTER TABLE routing_observations DROP COLUMN task_class;
            ALTER TABLE sessions DROP COLUMN entitlement;
            ALTER TABLE memories DROP COLUMN extraction_trigger;
            ALTER TABLE sessions DROP COLUMN last_seen_commit;
            ALTER TABLE sessions DROP COLUMN presentation_ref;
            DELETE FROM schema_migrations WHERE version >= 20;
        ";

        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let db_path = fixture.runtime.database_path();
        let project_id = stored_project_id(&db_path);

        // Back to 19, with a row written the way a version-19 build wrote
        // them — no `presentation_ref` to name.
        let (schema_at_19, columns_at_19) = {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(UNDO_20).unwrap();
            conn.execute(
                "INSERT INTO sessions (id, project_id, harness, role, lifecycle, \
                 presentation, created_at, last_activity_at) \
                 VALUES ('pre-migration', ?1, 'claude-code', 'normal', 'stopped', \
                 'external', 1, 1)",
                [&project_id],
            )
            .unwrap();
            (whole_schema(&conn), columns_of(&conn, "sessions"))
        };
        assert_eq!(schema_version(&db_path), 19, "the rollback must land on 19");
        assert!(
            !columns_at_19
                .iter()
                .any(|column| column == "presentation_ref"),
            "{columns_at_19:?}"
        );

        // Forward: an ordinary bootstrap, exactly as a real upgrade happens.
        let migrated = fixture.rebootstrap().unwrap();
        assert_eq!(
            schema_version(&migrated.database_path()),
            SUPPORTED_SCHEMA_VERSION,
            "the launch must have applied migration 20"
        );
        {
            let conn = Connection::open(&db_path).unwrap();
            let columns = columns_of(&conn, "sessions");
            let mut expected = columns_at_19.clone();
            expected.push("presentation_ref".to_owned());
            // A prefix, not an equality: the bootstrap runs every migration
            // above 19, and 21 appends `last_seen_commit` after this one. What
            // this migration owns is that ITS column is the first appended.
            assert_eq!(
                &columns[..expected.len()],
                &expected[..],
                "exactly one column from this migration, appended"
            );
        }

        // The pre-migration row reads as *no pane recorded*, and a row
        // written now carries the reference it was given — through the real
        // store, which is the only writer.
        {
            let sessions = ProjectSessions::open(&migrated).unwrap();
            let store = sessions.store();
            let pre = store
                .get(&SessionId::new("pre-migration"))
                .unwrap()
                .expect("the pre-migration row survives");
            assert_eq!(pre.presentation, SessionPresentation::External);
            assert_eq!(
                pre.presentation_ref, None,
                "a row from before the column existed has no pane, not an empty one"
            );

            let post = store
                .create(
                    NewSession::embedded("claude-code")
                        .with_presentation(SessionPresentation::External)
                        .with_presentation_ref(Some("workspace:349".to_owned())),
                )
                .unwrap();
            let read_back = store.get(&post.id).unwrap().unwrap();
            assert_eq!(read_back.presentation_ref.as_deref(), Some("workspace:349"));
        }

        // Back again: the whole schema is what it was at 19, byte for byte,
        // and both rows are still there.
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(UNDO_20).unwrap();
            assert_eq!(whole_schema(&conn), schema_at_19);
            assert_eq!(columns_of(&conn, "sessions"), columns_at_19);
            let rows: i64 = conn
                .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
                .unwrap();
            assert_eq!(rows, 2, "dropping the column drops no rows");
        }
        assert_eq!(schema_version(&db_path), 19);
    }

    /// Migration proof for 21: a version-20 database holding a session and a
    /// memory opens, migrates to 21 adding exactly one column to each of two
    /// tables, reads both pre-migration rows as *nothing recorded* rather
    /// than as a value, accepts a position and a trigger written through the
    /// real writers, and the undo takes the whole schema back to exactly what
    /// it was — every table, index and trigger.
    ///
    /// Named for what the migration does rather than for its number: the
    /// number is whatever position this script ends up in once the migrations
    /// being written beside it land, and a test name that had to be renumbered
    /// to stay true is a name nobody would renumber.
    ///
    /// One connection at a time throughout (practice §65): every handle is
    /// dropped before the next is opened and before the re-bootstrap.
    #[test]
    fn the_memory_commit_migration_adds_its_two_columns_and_undoes_cleanly() {
        use crate::memory::{MemoryKind, NewMemory, ProjectMemory};
        use crate::session::{NewSession, ProjectSessions};

        const UNDO: &str = "
            ALTER TABLE routing_observations DROP COLUMN completed_ms;
            ALTER TABLE routing_observations DROP COLUMN first_tool_call_ms;
            ALTER TABLE routing_observations DROP COLUMN first_token_ms;
            ALTER TABLE routing_observations DROP COLUMN first_byte_ms;
            ALTER TABLE routing_observations DROP COLUMN turn_shape;
            ALTER TABLE routing_observations DROP COLUMN effort_level;
            ALTER TABLE routing_observations DROP COLUMN session_id;
            ALTER TABLE routing_observations DROP COLUMN task_class;
            ALTER TABLE sessions DROP COLUMN entitlement;
            ALTER TABLE memories DROP COLUMN extraction_trigger;
            ALTER TABLE sessions DROP COLUMN last_seen_commit;
            DELETE FROM schema_migrations WHERE version >= 21;
        ";

        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let db_path = fixture.runtime.database_path();
        let project_id = stored_project_id(&db_path);

        // Back to 20, with a session row and a memory row written the way a
        // version-18 build wrote them — neither has a column to name.
        let (schema_at_18, session_columns_at_18, memory_columns_at_18) = {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(UNDO).unwrap();
            conn.execute(
                "INSERT INTO sessions (id, project_id, harness, role, lifecycle, \
                 presentation, created_at, last_activity_at) \
                 VALUES (\'pre-migration\', ?1, \'claude-code\', \'normal\', \'idle\', \
                 \'embedded\', 1, 1)",
                [&project_id],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO memories (id, project_id, kind, status, body, created_at, updated_at) \
                 VALUES (\'premigration0000\', ?1, \'finding\', \'active\', \
                 \'learned before the column existed\', 1, 1)",
                [&project_id],
            )
            .unwrap();
            (
                whole_schema(&conn),
                columns_of(&conn, "sessions"),
                columns_of(&conn, "memories"),
            )
        };
        assert_eq!(schema_version(&db_path), 20, "the rollback must land on 20");
        assert!(
            !session_columns_at_18
                .iter()
                .any(|column| column == "last_seen_commit"),
            "{session_columns_at_18:?}"
        );
        assert!(
            !memory_columns_at_18
                .iter()
                .any(|column| column == "extraction_trigger"),
            "{memory_columns_at_18:?}"
        );

        // Forward: an ordinary bootstrap, exactly as a real upgrade happens.
        let migrated = fixture.rebootstrap().unwrap();
        assert_eq!(
            schema_version(&migrated.database_path()),
            SUPPORTED_SCHEMA_VERSION,
            "the launch must have applied the memory-commit migration"
        );
        {
            let conn = Connection::open(&db_path).unwrap();
            let mut expected = session_columns_at_18.clone();
            expected.push("last_seen_commit".to_owned());
            // Migration 22 runs in the same forward pass and appends its own
            // column after this one. Named here rather than left out: this
            // assertion is about *append-only*, and a migration that
            // reordered or rebuilt the table is exactly what it must catch.
            expected.push("entitlement".to_owned());
            assert_eq!(
                columns_of(&conn, "sessions"),
                expected,
                "one column per migration, each appended"
            );
            let mut expected = memory_columns_at_18.clone();
            expected.push("extraction_trigger".to_owned());
            assert_eq!(
                columns_of(&conn, "memories"),
                expected,
                "exactly one column, appended"
            );
        }

        // Both pre-migration rows read as *nothing recorded*, never as a
        // value, and rows written now carry what the real writers gave them.
        {
            let sessions = ProjectSessions::open(&migrated).unwrap();
            let store = sessions.store();
            let pre = store
                .get(&crate::session::SessionId::new("pre-migration"))
                .unwrap()
                .expect("the pre-migration session survived");
            assert_eq!(
                pre.last_seen_commit, None,
                "a row from before the column existed has seen no HEAD, \
                 not an empty one"
            );

            let fresh = store.create(NewSession::embedded("claude-code")).unwrap();
            assert_eq!(
                fresh.last_seen_commit, None,
                "a session Glasshouse just created has not looked at HEAD either"
            );
            let noted = store
                .record_seen_commit(&fresh.id, "0123456789abcdef0123456789abcdef01234567")
                .unwrap();
            assert_eq!(
                noted.last_seen_commit.as_deref(),
                Some("0123456789abcdef0123456789abcdef01234567")
            );
        }
        {
            let memory = ProjectMemory::open(&migrated).unwrap();
            let store = memory.store();
            let pre = store
                .get(&crate::memory::MemoryId::new("premigration0000"))
                .unwrap()
                .expect("the pre-migration memory survived");
            assert_eq!(
                pre.extraction_trigger, None,
                "a row from before the column existed has no trigger, \
                 not an `unknown` one"
            );

            let recorded = store
                .record(
                    NewMemory::new(MemoryKind::Finding, "learned at a code-change boundary")
                        .with_extraction_trigger(Some("git_commit")),
                )
                .unwrap();
            assert_eq!(recorded.extraction_trigger.as_deref(), Some("git_commit"));
        }

        // Back again: the whole schema is what it was at 20, and the rows are
        // still there.
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(UNDO).unwrap();
            assert_eq!(whole_schema(&conn), schema_at_18);
            assert_eq!(columns_of(&conn, "sessions"), session_columns_at_18);
            assert_eq!(columns_of(&conn, "memories"), memory_columns_at_18);
            let sessions: i64 = conn
                .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
                .unwrap();
            let memories: i64 = conn
                .query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))
                .unwrap();
            assert_eq!(sessions, 2, "dropping the column drops no rows");
            assert_eq!(memories, 2, "dropping the column drops no rows");
        }
        assert_eq!(schema_version(&db_path), 20);
    }

    /// Migration proof for migration 22: a version-21 database holding a
    /// session written the way a version-21 build wrote it opens, migrates to
    /// 22 adding exactly one column — appended, never a rebuild — reads the
    /// pre-migration row as *no account recorded* rather than as a name,
    /// carries a name written through the real writer, and the undo takes the
    /// whole schema back to exactly what it was at 21, keeping every row.
    ///
    /// The `None` assertion is the one this migration most needs, and it is
    /// migration 20's lesson repeated: a column written `NOT NULL DEFAULT ''`
    /// would pass every other check here and would then tell `glasshouse
    /// entitlements` that every session recorded before the upgrade was
    /// served by an entitlement named by the empty string. *Nothing recorded*
    /// and *an account* are different facts, and only `NULL` keeps them apart.
    ///
    /// Named for what the migration does rather than for its number, for the
    /// reason the memory-commit proof above states.
    ///
    /// One connection at a time throughout (practice §65): every handle is
    /// dropped before the next is opened and before the re-bootstrap.
    #[test]
    fn the_entitlement_migration_adds_its_column_and_undoes_cleanly() {
        use crate::session::{NewSession, ProjectSessions};

        const UNDO: &str = "
            ALTER TABLE routing_observations DROP COLUMN completed_ms;
            ALTER TABLE routing_observations DROP COLUMN first_tool_call_ms;
            ALTER TABLE routing_observations DROP COLUMN first_token_ms;
            ALTER TABLE routing_observations DROP COLUMN first_byte_ms;
            ALTER TABLE routing_observations DROP COLUMN turn_shape;
            ALTER TABLE routing_observations DROP COLUMN effort_level;
            ALTER TABLE routing_observations DROP COLUMN session_id;
            ALTER TABLE routing_observations DROP COLUMN task_class;
            ALTER TABLE sessions DROP COLUMN entitlement;
            DELETE FROM schema_migrations WHERE version >= 22;
        ";

        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let db_path = fixture.runtime.database_path();
        let project_id = stored_project_id(&db_path);

        // Back to 21, with a session row written the way a version-21 build
        // wrote it: it has no column to name an account in.
        let (schema_at_21, session_columns_at_21) = {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(UNDO).unwrap();
            conn.execute(
                "INSERT INTO sessions (id, project_id, harness, role, lifecycle, \
                 presentation, created_at, last_activity_at, backend_resource) \
                 VALUES (\'pre-migration\', ?1, \'claude-code\', \'normal\', \'idle\', \
                 \'embedded\', 1, 1, \'direct-provider:alpha-probe\')",
                [&project_id],
            )
            .unwrap();
            (whole_schema(&conn), columns_of(&conn, "sessions"))
        };
        assert_eq!(schema_version(&db_path), 21, "the rollback must land on 21");
        assert!(
            !session_columns_at_21
                .iter()
                .any(|column| column == "entitlement"),
            "{session_columns_at_21:?}"
        );

        // Forward: an ordinary bootstrap, exactly as a real upgrade happens.
        let migrated = fixture.rebootstrap().unwrap();
        assert_eq!(
            schema_version(&migrated.database_path()),
            SUPPORTED_SCHEMA_VERSION,
            "the launch must have applied the entitlement migration"
        );
        assert_eq!(
            SUPPORTED_SCHEMA_VERSION, 25,
            "a fresh database reports the version the newest migration ships"
        );
        {
            let conn = Connection::open(&db_path).unwrap();
            let mut expected = session_columns_at_21.clone();
            expected.push("entitlement".to_owned());
            assert_eq!(
                columns_of(&conn, "sessions"),
                expected,
                "exactly one column, appended"
            );
        }

        // The pre-migration row reads as *nothing recorded*, and a row
        // written now carries what the real writer gave it.
        {
            let sessions = ProjectSessions::open(&migrated).unwrap();
            let store = sessions.store();
            let pre = store
                .get(&crate::session::SessionId::new("pre-migration"))
                .unwrap()
                .expect("the pre-migration session survived");
            assert_eq!(
                pre.entitlement, None,
                "a row from before the column existed was served by no account                  this build can name — not by one named the empty string"
            );
            assert_eq!(
                pre.backend_resource.as_deref(),
                Some("direct-provider:alpha-probe"),
                "and everything the old row did hold survived the upgrade"
            );

            let fresh = store.create(NewSession::embedded("claude-code")).unwrap();
            assert_eq!(
                fresh.entitlement, None,
                "a session created without one records no account, never a guess"
            );
            let named = store
                .create(
                    NewSession::embedded("claude-code")
                        .with_entitlement(Some("claude-b".to_owned())),
                )
                .unwrap();
            assert_eq!(named.entitlement.as_deref(), Some("claude-b"));
            assert_eq!(
                store
                    .get(&named.id)
                    .unwrap()
                    .expect("the session was recorded")
                    .entitlement
                    .as_deref(),
                Some("claude-b"),
                "and it survives the round trip through the column"
            );
        }

        // Back again: the whole schema is what it was at 21, and every row is
        // still there.
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(UNDO).unwrap();
            assert_eq!(whole_schema(&conn), schema_at_21);
            assert_eq!(columns_of(&conn, "sessions"), session_columns_at_21);
            let sessions: i64 = conn
                .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
                .unwrap();
            assert_eq!(sessions, 3, "dropping the column drops no rows");
        }
        assert_eq!(schema_version(&db_path), 21);
    }

    /// Migration proof for migration 17: a version-16 database opens,
    /// migrates to 17, keeps every memory it had, and comes out with a table
    /// that accepts an association — plus the index and the two triggers, and
    /// nothing else.
    ///
    /// The trigger check is by name and by behaviour: migration 5's three
    /// append-only triggers are deliberately **not** copied here, so a future
    /// migration that adds one fails this rather than quietly making a
    /// prunable table permanent.
    #[test]
    fn a_version_sixteen_database_migrates_forward_keeping_its_memories() {
        use crate::memory::{MemoryKind, NewMemory, ProjectMemory};

        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");

        let recorded = {
            let memory = ProjectMemory::open(&fixture.runtime).unwrap();
            memory
                .store()
                .record(NewMemory::new(
                    MemoryKind::Finding,
                    "a memory written before migration 17 existed",
                ))
                .unwrap()
        };

        let db_path = fixture.runtime.database_path();
        {
            let conn = Connection::open(&db_path).unwrap();
            // Migrations 19 and 18 are undone first: a rollback undoes
            // **every** migration above the version it claims, or the
            // re-run fails — `UNDO_MIGRATIONS_ABOVE_THIRTEEN`'s own lesson.
            conn.execute_batch(
                "ALTER TABLE routing_observations DROP COLUMN completed_ms;
                 ALTER TABLE routing_observations DROP COLUMN first_tool_call_ms;
                 ALTER TABLE routing_observations DROP COLUMN first_token_ms;
                 ALTER TABLE routing_observations DROP COLUMN first_byte_ms;
                 ALTER TABLE routing_observations DROP COLUMN turn_shape;
                 ALTER TABLE routing_observations DROP COLUMN effort_level;
                 ALTER TABLE routing_observations DROP COLUMN session_id;
                 ALTER TABLE routing_observations DROP COLUMN task_class;
                 ALTER TABLE sessions DROP COLUMN entitlement;
                ALTER TABLE memories DROP COLUMN extraction_trigger;
                 ALTER TABLE sessions DROP COLUMN last_seen_commit;
                ALTER TABLE sessions DROP COLUMN presentation_ref;
                 DROP TABLE assumption_transitions;
                 DROP TABLE task_assumptions;
                 ALTER TABLE routing_observations DROP COLUMN failure_class;
                 DROP TABLE memory_files;
                 DELETE FROM schema_migrations WHERE version >= 17;",
            )
            .unwrap();
        }
        assert_eq!(
            schema_version(&db_path),
            16,
            "the rollback must land on version 16"
        );

        let migrated = fixture.rebootstrap().unwrap();
        assert_eq!(
            schema_version(&migrated.database_path()),
            SUPPORTED_SCHEMA_VERSION,
            "the launch must have applied migration 17"
        );

        // The memory that predates the table is still there, and reads back
        // with no associations — which is the truth about it.
        let memory = ProjectMemory::open(&migrated).unwrap();
        let store = memory.store();
        assert_eq!(
            store.get(&recorded.id).unwrap().map(|found| found.body),
            Some(recorded.body.clone())
        );

        let conn = Connection::open(migrated.database_path()).unwrap();
        let associations: i64 = conn
            .query_row("SELECT COUNT(*) FROM memory_files", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            associations, 0,
            "a memory recorded before this migration has no associations to invent"
        );

        // The index the table exists for.
        let index: Option<String> = conn
            .query_row(
                "SELECT name FROM sqlite_master                   WHERE type = 'index' AND name = 'memory_files_by_path'",
                [],
                |row| row.get(0),
            )
            .optional()
            .unwrap();
        assert_eq!(index.as_deref(), Some("memory_files_by_path"));

        // Migration 11's two triggers, and nothing else — no append-only
        // trigger, so this table stays prunable.
        let mut statement = conn
            .prepare(
                "SELECT name FROM sqlite_master                   WHERE type = 'trigger' AND tbl_name = 'memory_files' ORDER BY name",
            )
            .unwrap();
        let triggers: Vec<String> = statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            triggers,
            vec![
                "memory_files_reject_foreign_project_insert".to_owned(),
                "memory_files_reject_foreign_project_update".to_owned(),
            ],
            "migration 11's two project-scope triggers, and nothing else"
        );
        drop(statement);

        // And the table really is prunable, behaviourally — unlike
        // `lifecycle_events`, whose BEFORE DELETE trigger aborts.
        store
            .record_observed_files(
                std::slice::from_ref(&recorded.id),
                &["src/example.rs".to_owned()],
            )
            .unwrap();
        let removed = conn.execute("DELETE FROM memory_files", []).unwrap();
        assert_eq!(removed, 1);
    }

    /// Migration 10's `CHECK` on `review_reason` is the only definition of
    /// Phase 21C's six review reasons — map lines 885-890, one value per
    /// line, in order. Modeled on
    /// `every_project_phase_the_type_supports_is_one_the_schema_accepts`,
    /// reading the list **out of the migration itself** rather than out of a
    /// second constant that could drift from it.
    #[test]
    fn every_review_reason_the_type_supports_is_one_the_schema_accepts() {
        use crate::memory::ReviewReason;

        let migration = MIGRATIONS[9];
        let marker = "review_reason IN";
        let open = migration
            .find(marker)
            .expect("migration 10 checks review_reason")
            + marker.len();
        let list = &migration[open..];
        let list = &list[..list.find(')').expect("the CHECK's list is parenthesised")];
        let accepted: Vec<String> = list
            .split(',')
            .map(|value| value.trim().trim_matches(['(', ' ', '\n', '\'']).to_owned())
            .filter(|value| !value.is_empty())
            .collect();

        let declared: Vec<String> = ReviewReason::ALL
            .iter()
            .map(|reason| reason.as_str().to_owned())
            .collect();

        assert_eq!(
            declared, accepted,
            "a review reason was added or renamed without migration 10's CHECK, \
             or the two fell out of the map's own order"
        );
        assert_eq!(accepted.len(), 6, "the CHECK's list was not read correctly");
    }

    /// Migration proof (a): a version-9 database opens, migrates to 10, and
    /// keeps every existing row — including a memory recorded before any of
    /// Phase 21C's columns existed.
    #[test]
    fn a_version_nine_database_migrates_forward_keeping_its_memories() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let memory = crate::memory::ProjectMemory::open(&fixture.runtime).unwrap();
        let store = memory.store();

        let pre_existing = store
            .record(crate::memory::NewMemory::new(
                crate::memory::MemoryKind::Decision,
                "amethyst decisions predate migration 10",
            ))
            .unwrap();
        drop(store);
        drop(memory);

        let db_path = fixture.runtime.database_path();
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(&format!(
                "{UNDO_MIGRATIONS_ABOVE_THIRTEEN}
                 ALTER TABLE sessions DROP COLUMN source_session_id;
                 DROP TABLE routing_observations;

                 ALTER TABLE memories DROP COLUMN superseded_reason;
                 ALTER TABLE memories DROP COLUMN validity_conditions;
                 ALTER TABLE memories DROP COLUMN invalidation_conditions;
                 ALTER TABLE memories DROP COLUMN review_reason;
                 ALTER TABLE memories DROP COLUMN review_marked_at;
                 ALTER TABLE memories DROP COLUMN last_validated_at;

                 DELETE FROM schema_migrations WHERE version >= 10;"
            ))
            .unwrap();
        }

        assert_eq!(
            schema_version(&db_path),
            9,
            "the rollback must land on version 9"
        );

        // The next launch is an ordinary bootstrap; nothing special is asked
        // of it, matching the way a real upgrade happens.
        let migrated = fixture.rebootstrap().unwrap();
        assert_eq!(
            schema_version(&migrated.database_path()),
            SUPPORTED_SCHEMA_VERSION,
            "the launch must have applied migration 10"
        );

        let reopened = crate::memory::ProjectMemory::open(&migrated).unwrap();
        let intact = reopened
            .store()
            .get(&pre_existing.id)
            .unwrap()
            .expect("the pre-migration memory must survive the upgrade");
        assert_eq!(intact.body, pre_existing.body);

        // Migration proof (c): a pre-migration row's `last_validated_at`
        // reads as unknown, not as zero — the row existed before the column
        // did, so `ALTER TABLE ADD COLUMN` backfills it with `NULL`, and
        // `row_to_record` must not substitute a default for that `NULL`.
        assert_eq!(
            intact.last_validated_at, None,
            "a pre-migration memory's last_validated_at must read as unknown, not as zero"
        );
        assert_eq!(intact.review_reason, None);
        assert_eq!(intact.review_marked_at, None);
    }

    /// Migration proof (a) for migration 13: a version-12 database opens,
    /// migrates to 13, and keeps every existing row — including a memory that
    /// was **already superseded** before the reason column existed.
    ///
    /// That last part is the one worth having. A pre-migration supersession is
    /// the population line 925's column can never fill in, so it has to read
    /// back as *"no reason recorded"* rather than as anything invented, and
    /// the supersession itself has to survive intact.
    #[test]
    fn a_version_twelve_database_migrates_forward_keeping_a_supersession_it_could_not_explain() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let memory = crate::memory::ProjectMemory::open(&fixture.runtime).unwrap();
        let store = memory.store();

        let old = store
            .record(crate::memory::NewMemory::new(
                crate::memory::MemoryKind::Decision,
                "obsidian decisions predate migration 13",
            ))
            .unwrap();
        let replacement = store
            .record(crate::memory::NewMemory::new(
                crate::memory::MemoryKind::Decision,
                "obsidian's successor",
            ))
            .unwrap();
        let superseded = store.supersede(&old.id, &replacement.id).unwrap();
        assert_eq!(superseded.superseded_by.as_ref(), Some(&replacement.id));
        drop(store);
        drop(memory);

        let db_path = fixture.runtime.database_path();
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(&format!(
                "{UNDO_MIGRATIONS_ABOVE_THIRTEEN}
                 ALTER TABLE memories DROP COLUMN superseded_reason;
                 DELETE FROM schema_migrations WHERE version >= 13;"
            ))
            .unwrap();
        }

        assert_eq!(
            schema_version(&db_path),
            12,
            "the rollback must land on version 12"
        );

        // The next launch is an ordinary bootstrap; nothing special is asked
        // of it, matching the way a real upgrade happens.
        let migrated = fixture.rebootstrap().unwrap();
        assert_eq!(
            schema_version(&migrated.database_path()),
            SUPPORTED_SCHEMA_VERSION,
            "the launch must have applied migration 13"
        );

        let reopened = crate::memory::ProjectMemory::open(&migrated).unwrap();
        let intact = reopened
            .store()
            .get(&old.id)
            .unwrap()
            .expect("the pre-migration memory must survive the upgrade");
        assert_eq!(intact.body, old.body);
        assert_eq!(
            intact.status,
            crate::memory::MemoryStatus::Superseded,
            "the supersession recorded before the column existed must survive it"
        );
        assert_eq!(intact.superseded_by.as_ref(), Some(&replacement.id));
        assert_eq!(
            intact.superseded_reason, None,
            "a supersession recorded before migration 13 has no reason, and must not acquire an \
             invented one"
        );
        // The successor is untouched by any of it.
        let successor = reopened
            .store()
            .get(&replacement.id)
            .unwrap()
            .expect("the successor must survive the upgrade");
        assert_eq!(successor.superseded_reason, None);
        assert_eq!(successor.status, crate::memory::MemoryStatus::Active);
    }

    /// Migration proof for migration 14: a version-13 database opens, migrates
    /// to 14, and keeps every checkpoint it had — in the order it could
    /// actually record, and admitting the order it never could.
    ///
    /// The three checkpoints are written into **two seconds**: two into the
    /// first and one into the second. That split is the whole test. The
    /// between-second order was recorded in `created_at` and must survive
    /// exactly; the within-second order was recorded nowhere, so the backfill
    /// cannot recover it and must not invent it — what it owes instead is the
    /// answer the old query already gave, which is `id` order, so that a
    /// database that migrates does not silently change an answer it had
    /// already given the user.
    #[test]
    fn a_version_thirteen_database_migrates_forward_keeping_the_order_it_could_record() {
        use crate::checkpoint::{CheckpointReason, ProjectCheckpoints};
        use crate::session::SessionId;

        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let session = SessionId::new("session-a");

        // Two in the first second, one in the second — through `save`, so the
        // rows are exactly what a version-13 build would have left behind
        // apart from the column that is about to be removed.
        let checkpoints = ProjectCheckpoints::open(&fixture.runtime).unwrap();
        let store = checkpoints.store();
        let earlier_a = store
            .save(sample_checkpoint(&session, 1_000, CheckpointReason::Manual))
            .unwrap();
        let earlier_b = store
            .save(sample_checkpoint(&session, 1_000, CheckpointReason::Manual))
            .unwrap();
        let later = store
            .save(sample_checkpoint(
                &session,
                2_000,
                CheckpointReason::TaskBoundary,
            ))
            .unwrap();
        drop(store);
        drop(checkpoints);

        let db_path = fixture.runtime.database_path();
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(&format!(
                "{UNDO_MIGRATIONS_ABOVE_THIRTEEN}
                 DELETE FROM schema_migrations WHERE version >= 14;"
            ))
            .unwrap();
        }
        assert_eq!(
            schema_version(&db_path),
            13,
            "the rollback must land on version 13"
        );

        let migrated = fixture.rebootstrap().unwrap();
        assert_eq!(
            schema_version(&migrated.database_path()),
            SUPPORTED_SCHEMA_VERSION,
            "the launch must have applied migration 14"
        );

        let reopened = ProjectCheckpoints::open(&migrated).unwrap();
        let store = reopened.store();

        // Nothing was lost, and every document still parses.
        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 3, "the migration must keep every checkpoint");

        // The between-second order survives: the later checkpoint is still
        // the later one, both in the listing and in the resolution.
        assert_eq!(
            store.latest_for(&session).unwrap().unwrap().id,
            later.id,
            "a checkpoint written a second after the others must still resolve as the latest"
        );
        assert_eq!(store.latest().unwrap().unwrap().id, later.id);
        assert_eq!(listed[0].id, later.id);

        // The within-second order is the one the old query reported — `id`
        // order — because nothing else about it was ever recorded. Asserting
        // that rather than a write order is the honest claim: the two rows
        // tied on `created_at`, and this is what the migration promises for
        // them.
        let mut tied = [earlier_a.id.clone(), earlier_b.id.clone()];
        tied.sort();
        assert_eq!(
            [listed[2].id.clone(), listed[1].id.clone()],
            tied,
            "rows tied on created_at must keep the order the old query gave them"
        );

        // And a checkpoint written *after* the migration outranks every
        // backfilled row, which is what stops the counter restarting inside
        // the population it just numbered.
        let after = store
            .save(sample_checkpoint(
                &session,
                // Deliberately *earlier* than everything already stored: the
                // counter is a write order, not a clock reading, so a clock
                // that stepped backwards must not resurrect an older row.
                500,
                CheckpointReason::Manual,
            ))
            .unwrap();
        assert_eq!(
            store.latest_for(&session).unwrap().unwrap().id,
            after.id,
            "the checkpoint written last must win even when its timestamp is the oldest"
        );
        assert_eq!(store.latest().unwrap().unwrap().id, after.id);
    }

    /// A checkpoint with just enough in it to render, parse and be told apart.
    fn sample_checkpoint(
        session: &crate::session::SessionId,
        at: i64,
        reason: crate::checkpoint::CheckpointReason,
    ) -> crate::checkpoint::Checkpoint {
        crate::checkpoint::Checkpoint {
            session: session.clone(),
            harness: "a-harness".to_owned(),
            reason,
            created_at: at,
            git: None,
            working_tree: None,
            handoff: crate::checkpoint::Handoff {
                objective: format!("the objective at {at}"),
                implementation_state: "the state".to_owned(),
                next_actions: vec!["carry on".to_owned()],
                ..crate::checkpoint::Handoff::default()
            },
            trimmed: false,
        }
    }

    /// Migration proof (b) for migration 13: its `CHECK` refuses an empty
    /// reason, so `''` can never read back as *"a reason was recorded"* even
    /// from a hand-edited database.
    #[test]
    fn migration_thirteen_rejects_an_empty_supersession_reason() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let memory = crate::memory::ProjectMemory::open(&fixture.runtime).unwrap();
        let recorded = memory
            .store()
            .record(crate::memory::NewMemory::new(
                crate::memory::MemoryKind::Finding,
                "onyx needs a supersession reason that is not one",
            ))
            .unwrap();
        drop(memory);

        let conn = Connection::open(fixture.runtime.database_path()).unwrap();
        assert!(
            conn.execute(
                "UPDATE memories SET superseded_reason = '' WHERE id = ?1",
                [recorded.id.as_str()],
            )
            .is_err(),
            "an empty supersession reason must be rejected by the CHECK constraint"
        );
        assert!(
            conn.execute(
                "UPDATE memories SET superseded_reason = ?2 WHERE id = ?1",
                rusqlite::params![recorded.id.as_str(), "x".repeat(513)],
            )
            .is_err(),
            "a supersession reason past the bound must be rejected by the CHECK constraint"
        );
    }

    /// Migration proof (b): migration 10's new `CHECK` rejects a
    /// `review_reason` outside the six the map names.
    #[test]
    fn migration_ten_rejects_an_unrecognized_review_reason() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let memory = crate::memory::ProjectMemory::open(&fixture.runtime).unwrap();
        let recorded = memory
            .store()
            .record(crate::memory::NewMemory::new(
                crate::memory::MemoryKind::Finding,
                "beryl needs a review reason that does not exist",
            ))
            .unwrap();
        drop(memory);

        let conn = Connection::open(fixture.runtime.database_path()).unwrap();
        let result = conn.execute(
            "UPDATE memories SET review_reason = 'not-a-real-reason' WHERE id = ?1",
            [recorded.id.as_str()],
        );
        assert!(
            result.is_err(),
            "an unrecognized review_reason must be rejected by the CHECK constraint"
        );
    }

    /// Migration proof (a): a version-10 database migrates to 11 keeping
    /// every existing row, and gains a working `routing_observations` table.
    #[test]
    fn a_version_ten_database_migrates_forward_keeping_its_memories() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let memory = crate::memory::ProjectMemory::open(&fixture.runtime).unwrap();
        let pre_existing = memory
            .store()
            .record(crate::memory::NewMemory::new(
                crate::memory::MemoryKind::Decision,
                "citrine decisions predate migration 11",
            ))
            .unwrap();
        drop(memory);

        let db_path = fixture.runtime.database_path();
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(&format!(
                "{UNDO_MIGRATIONS_ABOVE_THIRTEEN}
                 ALTER TABLE sessions DROP COLUMN source_session_id;
                 ALTER TABLE memories DROP COLUMN superseded_reason;
                 DROP TABLE routing_observations;
                 DELETE FROM schema_migrations WHERE version >= 11;"
            ))
            .unwrap();
        }

        assert_eq!(
            schema_version(&db_path),
            10,
            "the rollback must land on version 10"
        );

        // The next launch is an ordinary bootstrap; nothing special is asked
        // of it, matching the way a real upgrade happens.
        let migrated = fixture.rebootstrap().unwrap();
        assert_eq!(
            schema_version(&migrated.database_path()),
            SUPPORTED_SCHEMA_VERSION,
            "the launch must have applied migration 11"
        );

        let reopened = crate::memory::ProjectMemory::open(&migrated).unwrap();
        let intact = reopened
            .store()
            .get(&pre_existing.id)
            .unwrap()
            .expect("the pre-migration memory must survive the upgrade");
        assert_eq!(intact.body, pre_existing.body);

        let conn = Connection::open(migrated.database_path()).unwrap();
        let project_id = stored_project_id(&migrated.database_path());
        conn.execute(
            "INSERT INTO routing_observations (project_id, observed_at, provider, model) \
             VALUES (?1, 1000, 'fixture', 'fixture-model')",
            [project_id.as_str()],
        )
        .expect("a freshly migrated database must accept a routing observation");
    }

    /// Migration proof (b): the isolation trigger really aborts a foreign
    /// `project_id`, migration 4's own pair applied to `routing_observations`.
    #[test]
    fn migration_eleven_rejects_a_routing_observation_from_a_foreign_project() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let conn = Connection::open(fixture.runtime.database_path()).unwrap();

        let result = conn.execute(
            "INSERT INTO routing_observations (project_id, observed_at, provider, model) \
             VALUES ('a-different-project-entirely', 1000, 'fixture', 'fixture-model')",
            [],
        );
        assert!(
            result.is_err(),
            "an insert naming a foreign project_id must be rejected by the isolation trigger"
        );
    }

    /// Migration proof (c): the `cost_micro_usd`/`cost_confidence` `CHECK`
    /// refuses a cost with no confidence label.
    #[test]
    fn migration_eleven_refuses_a_cost_with_no_confidence_label() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let conn = Connection::open(fixture.runtime.database_path()).unwrap();
        let project_id = stored_project_id(&fixture.runtime.database_path());

        let result = conn.execute(
            "INSERT INTO routing_observations \
             (project_id, observed_at, provider, model, cost_micro_usd) \
             VALUES (?1, 1000, 'fixture', 'fixture-model', 500)",
            [project_id.as_str()],
        );
        assert!(
            result.is_err(),
            "a stored cost with no cost_confidence must be rejected by the CHECK constraint"
        );

        // The paired value is accepted, so the failure above is about the
        // missing label and not about the column existing at all.
        conn.execute(
            "INSERT INTO routing_observations \
             (project_id, observed_at, provider, model, cost_micro_usd, cost_confidence) \
             VALUES (?1, 1000, 'fixture', 'fixture-model', 500, 'estimated')",
            [project_id.as_str()],
        )
        .expect("a cost paired with a confidence label must be accepted");
    }

    #[test]
    fn bootstrap_creates_the_project_database() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let db = fixture.runtime.database_path();

        assert_eq!(db.file_name().unwrap(), DATABASE_FILE_NAME);
        assert!(db.is_file());
        assert_eq!(
            stored_project_id(&db),
            fixture.runtime.project().id().as_str()
        );
        assert_eq!(schema_version(&db), SUPPORTED_SCHEMA_VERSION);
    }

    #[test]
    fn two_projects_sharing_one_data_root_get_separate_databases_and_ids() {
        let tmp = tempfile::tempdir().unwrap();
        // alpha and beta resolve against the SAME GLASSHOUSE data/config
        // root; only their project identities differ.
        let alpha = Fixture::new(tmp.path(), "alpha");
        let beta = Fixture::new(tmp.path(), "beta");

        let alpha_db = alpha.runtime.database_path();
        let beta_db = beta.runtime.database_path();

        // Both databases live under the one shared projects root...
        let projects_root = tmp.path().join("data").join("projects");
        assert_eq!(alpha_db.parent().unwrap().parent().unwrap(), projects_root);
        assert_eq!(beta_db.parent().unwrap().parent().unwrap(), projects_root);
        // ...yet in physically different files and directories.
        assert_ne!(alpha_db.parent(), beta_db.parent());
        assert_ne!(alpha_db, beta_db);

        // And each file records its own project, not its neighbour's.
        let alpha_id = stored_project_id(&alpha_db);
        let beta_id = stored_project_id(&beta_db);
        assert_ne!(alpha_id, beta_id);
        assert_eq!(alpha_id, alpha.runtime.project().id().as_str());
        assert_eq!(beta_id, beta.runtime.project().id().as_str());
    }

    #[test]
    fn reopening_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let db = fixture.runtime.database_path();

        let id_before = stored_project_id(&db);
        let version_before = schema_version(&db);

        // Reopen through bootstrap several times: nothing may drift.
        for _ in 0..3 {
            let runtime = fixture.rebootstrap().unwrap();
            assert_eq!(runtime.database_path(), db);
        }

        assert_eq!(stored_project_id(&db), id_before);
        assert_eq!(schema_version(&db), version_before);
    }

    #[test]
    fn concurrent_first_bootstraps_serialize_on_one_database() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("workspace").join("solo");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();

        const CALLERS: usize = 16;
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(CALLERS));
        let mut handles = Vec::new();
        for _ in 0..CALLERS {
            let barrier = std::sync::Arc::clone(&barrier);
            let root = root.clone();
            let data = tmp.path().join("data");
            let config = tmp.path().join("config");
            handles.push(std::thread::spawn(move || {
                // Release all callers at once so the very first creation of
                // the database file and schema is genuinely contended.
                barrier.wait();
                let cli = Cli::try_parse_from([
                    "glasshouse",
                    "--data-dir",
                    data.to_str().unwrap(),
                    "--config-dir",
                    config.to_str().unwrap(),
                ])
                .unwrap();
                crate::bootstrap(&cli, &root)
            }));
        }

        let mut results = Vec::new();
        for handle in handles {
            results.push(handle.join().expect("bootstrap thread panicked"));
        }
        for result in &results {
            result
                .as_ref()
                .expect("a concurrent first bootstrap failed");
        }

        // All callers agree on one physical database with one binding.
        let expected_db = results[0].as_ref().unwrap().database_path();
        let expected_id = results[0].as_ref().unwrap().project().id().as_str();
        for result in &results {
            let runtime = result.as_ref().unwrap();
            assert_eq!(runtime.database_path(), expected_db);
        }
        assert_eq!(schema_version(&expected_db), SUPPORTED_SCHEMA_VERSION);
        assert_eq!(stored_project_id(&expected_db), expected_id);

        let conn = Connection::open(&expected_db).unwrap();
        let bindings: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM project_metadata WHERE key = 'project_id'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(bindings, 1);
    }

    #[test]
    fn mismatched_copied_database_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let alpha = Fixture::new(tmp.path(), "alpha");
        let beta = Fixture::new(tmp.path(), "beta");

        // Copy alpha's whole database into beta's slot.
        std::fs::copy(alpha.runtime.database_path(), beta.runtime.database_path()).unwrap();

        let err = beta.rebootstrap().unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("belongs to project"), "{msg}");
        assert!(msg.contains(alpha.runtime.project().id().as_str()), "{msg}");
        assert!(
            msg.contains(beta.runtime.database_path().display().to_string().as_str()),
            "{msg}"
        );

        // The copy is left untouched for the user to decide about.
        assert_eq!(
            stored_project_id(&beta.runtime.database_path()),
            stored_project_id(&alpha.runtime.database_path())
        );
    }

    #[test]
    fn metadata_without_a_project_id_is_rejected_and_not_adopted() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let db = fixture.runtime.database_path();

        {
            let conn = Connection::open(&db).unwrap();
            conn.execute(
                "DELETE FROM project_metadata WHERE key = ?1",
                [PROJECT_ID_KEY],
            )
            .unwrap();
        }

        let err = fixture.rebootstrap().unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("no project identifier"), "{msg}");
        assert!(msg.contains(db.display().to_string().as_str()), "{msg}");

        // Rejection happens during the read-only identity preflight: the
        // missing binding is not silently recreated for the active project.
        let conn = Connection::open(&db).unwrap();
        let bindings: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM project_metadata WHERE key = ?1",
                [PROJECT_ID_KEY],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(bindings, 0);
        assert_eq!(schema_version(&db), SUPPORTED_SCHEMA_VERSION);
    }

    #[test]
    fn too_new_schema_is_rejected_and_not_recreated() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let db = fixture.runtime.database_path();

        {
            // What a newer Glasshouse would leave behind: this build's
            // migrations, plus one it has never heard of. Appending rather
            // than rewriting the existing rows keeps the fixture correct as
            // more migrations are added.
            let conn = Connection::open(&db).unwrap();
            conn.execute_batch("INSERT INTO schema_migrations (version) VALUES (99);")
                .unwrap();
        }

        let err = fixture.rebootstrap().unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("newer"), "{msg}");
        assert!(msg.contains("99"), "{msg}");
        assert!(msg.contains(db.display().to_string().as_str()), "{msg}");

        // Refused, not deleted or recreated: the too-new marker survives.
        assert!(db.is_file());
        assert_eq!(schema_version(&db), 99);
    }

    #[test]
    fn corrupt_database_is_refused_and_never_recreated() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let db = fixture.runtime.database_path();

        std::fs::write(&db, b"definitely not a sqlite database").unwrap();

        let err = fixture.rebootstrap().unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains(db.display().to_string().as_str()), "{msg}");

        // Still the same garbage bytes: nothing was silently wiped.
        assert_eq!(
            std::fs::read(&db).unwrap(),
            b"definitely not a sqlite database"
        );
    }

    #[test]
    fn directory_at_the_database_path_is_rejected_and_not_replaced() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let db = fixture.runtime.database_path();

        // Put a plain directory where the database belongs.
        std::fs::remove_file(&db).unwrap();
        std::fs::create_dir(&db).unwrap();

        let err = fixture.rebootstrap().unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("not a regular file") || msg.contains("a directory"),
            "{msg}"
        );
        assert!(msg.contains(db.display().to_string().as_str()), "{msg}");

        // The directory is still there: nothing deleted or recreated it.
        assert!(db.is_dir());
    }

    #[test]
    fn foreign_database_with_pending_migrations_is_rejected_before_any_writes() {
        let tmp = tempfile::tempdir().unwrap();
        let alpha = Fixture::new(tmp.path(), "alpha");
        let beta = Fixture::new(tmp.path(), "beta");

        // Copy alpha's bound database into beta's slot, then make its
        // migration state look pending by dropping the migration ledger.
        // An implementation that migrated before checking identity would
        // recreate the ledger and write into this foreign database.
        std::fs::copy(alpha.runtime.database_path(), beta.runtime.database_path()).unwrap();
        {
            let conn = Connection::open(beta.runtime.database_path()).unwrap();
            conn.execute_batch("DROP TABLE schema_migrations;").unwrap();
        }

        let err = beta.rebootstrap().unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("belongs to project"), "{msg}");
        assert!(msg.contains(alpha.runtime.project().id().as_str()), "{msg}");
        assert!(
            msg.contains(beta.runtime.database_path().display().to_string().as_str()),
            "{msg}"
        );

        // The refusal happened before any schema work: the ledger is still
        // absent and the foreign binding untouched.
        let conn = Connection::open(beta.runtime.database_path()).unwrap();
        let ledger_present: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type = 'table' AND name = 'schema_migrations'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(ledger_present, 0);
        assert_eq!(
            stored_project_id(&beta.runtime.database_path()),
            alpha.runtime.project().id().as_str()
        );
    }

    #[cfg(unix)]
    #[test]
    fn readonly_database_file_is_rejected() {
        use std::os::unix::fs::PermissionsExt;

        // Root can write to 0400 files regardless; the scenario does not
        // exist for that user, so the regression test says nothing there.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }

        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let db = fixture.runtime.database_path();

        std::fs::set_permissions(&db, fs::Permissions::from_mode(0o400)).unwrap();

        let err = fixture.rebootstrap().unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("read-only"), "{msg}");
        assert!(msg.contains(db.display().to_string().as_str()), "{msg}");

        // Restore so the temp directory can be cleaned up.
        std::fs::set_permissions(&db, fs::Permissions::from_mode(0o600)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn new_database_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let mode = std::fs::metadata(fixture.runtime.database_path())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "new database must be owner-only");
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_final_database_path_is_rejected() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let db = fixture.runtime.database_path();

        // Replace the real database with a symlink to an unrelated file.
        let decoy = tmp.path().join("decoy.db");
        std::fs::write(&decoy, b"decoy").unwrap();
        std::fs::remove_file(&db).unwrap();
        symlink(&decoy, &db).unwrap();

        let err = fixture.rebootstrap().unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("symbolic link"), "{msg}");
        assert!(msg.contains(db.display().to_string().as_str()), "{msg}");

        // The symlink itself is left alone; nothing followed or replaced it.
        assert!(
            std::fs::symlink_metadata(&db)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read(&decoy).unwrap(), b"decoy");
    }

    #[test]
    fn a_zero_byte_existing_database_is_refused_not_silently_reinitialized() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let db = fixture.runtime.database_path();

        // Truncate an existing, previously-migrated database to zero bytes —
        // the shape a crashed `cp`, an interrupted restore, or a full-disk
        // write leaves behind.
        std::fs::write(&db, []).unwrap();
        assert_eq!(std::fs::metadata(&db).unwrap().len(), 0);

        let err = fixture.rebootstrap().unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("empty"), "{msg}");
        assert!(msg.contains(db.display().to_string().as_str()), "{msg}");

        // The refused open must not have touched the file: still zero bytes,
        // no migration ever ran against it.
        assert_eq!(
            std::fs::metadata(&db).unwrap().len(),
            0,
            "a refused open must leave the file byte-identical"
        );
    }

    #[test]
    fn a_missing_database_file_still_creates_a_fresh_project() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let db = fixture.runtime.database_path();
        let version_before = schema_version(&db);

        // Unlike the zero-byte case, a database that simply does not exist
        // yet must still be created and migrated exactly as a first launch.
        std::fs::remove_file(&db).unwrap();
        assert!(!db.exists());

        let migrated = fixture.rebootstrap().unwrap();
        assert_eq!(migrated.database_path(), db);
        assert!(db.exists());
        assert_eq!(schema_version(&db), version_before);
    }
}
