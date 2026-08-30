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
/// Later migrations are appended to [`MIGRATIONS`], and this constant moves
/// with them.
const SUPPORTED_SCHEMA_VERSION: i64 = 17;

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
pub(crate) const EVALUATION_KINDS: [&str; 4] = [
    "memory_retrieved",
    "disposable_route_decided",
    "routing_override_decided",
    "routing_continuation_decided",
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
/// Returns `Ok(false)` only when the path definitively does not exist (so the
/// caller should create it), `Ok(true)` when an existing regular file is
/// ready to open. Any other inspection failure — permission denied and
/// friends — is preserved with its source rather than being mistaken for
/// permission to create the file.
fn check_existing(db_path: &Path) -> Result<bool, DatabaseError> {
    let metadata = match fs::symlink_metadata(db_path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(false);
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
            // Lost the race; hold the winner to the same checks.
            check_existing(db_path).map(|_| ())
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
    /// triggers with it — and it goes **first**, because the rollback runs
    /// newest-migration-first for the same reason the ladder runs
    /// oldest-first.
    const UNDO_MIGRATIONS_ABOVE_THIRTEEN: &str = "
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
            conn.execute_batch(
                "DROP TABLE memory_files;
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
}
