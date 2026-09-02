//! Migrations 1 through 13, split out of `database.rs`'s `MIGRATIONS`
//! array by Phase 59's decomposition (a package over the ceiling splits
//! once by version range). Bodies are verbatim.

pub(crate) const MIGRATIONS_V1_TO_V13: [&str; 13] = [
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
];
