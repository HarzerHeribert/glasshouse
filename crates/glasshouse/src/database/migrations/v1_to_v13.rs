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
    // `kind` and `authority` are two columns, not one, because they answer
    // different questions -- what sort of thing was remembered (Phase 20's
    // six kinds), and how binding it is (Phase 21A's seven classes) -- and
    // the two vocabularies overlap in spelling, so folding them would make
    // "this finding is binding" unrepresentable. `authority` ships nullable
    // and unused by any classifier yet, so Phase 21A adds classification
    // rather than a migration. `memories` carries a rowid, unlike `sessions`,
    // only because FTS5's external-content mode joins on `content_rowid`.
    // The isolation and supersession triggers enforce migration 2's rule
    // structurally rather than leaving it to callers to remember.
    //
    // History: design-decisions.md, "Trims: migration and native-secret
    // module docs", database/migrations/v1_to_v13.rs migration 4.
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
    // `lifecycle_events` refuses UPDATE and DELETE by trigger, because Phase
    // 18 requires that derived interpretation never overwrite or masquerade
    // as the original event -- so nothing can prune this table. There is
    // deliberately no column a conversation could reach: only an integration
    // slug and an event name travel this far. No `REFERENCES sessions(id)`,
    // because `PRAGMA foreign_keys` is off by default and an event for a
    // session this database never heard of is a fact worth keeping.
    //
    // `checkpoints` is separate from `memories` because Phase 19 requires it:
    // a checkpoint is bounded handoff context, a memory is durable project
    // knowledge. `document` is the checkpoint; the three columns beside it
    // are an index, each written from the document in one place so the row
    // and the document cannot drift.
    //
    // History: design-decisions.md, "Trims: migration and native-secret
    // module docs", database/migrations/v1_to_v13.rs migration 5.
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
    // `source_event_first`/`_last` are two integers, both-or-neither, because
    // a memory is rarely traceable to one event -- the honest reference is
    // the range of `lifecycle_events.seq` a chunk covered. `rationale` and
    // the eight provenance columns beside it are flat, nullable free text,
    // never a related table, so NULL means "not known", never "none"; the
    // credential control is on the producer side, in `memory::extract`.
    // `memories_fts` is rebuilt rather than altered because FTS5 has no
    // `ALTER` that adds a column, and only `rationale` joins the index --
    // the other eight are attributes of a decision already found, not words
    // someone would search for.
    //
    // History: design-decisions.md, "Trims: migration and native-secret
    // module docs", database/migrations/v1_to_v13.rs migration 6.
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
    // This rebuilds `lifecycle_events` rather than altering its `CHECK`,
    // because SQLite cannot add or drop one, and `seq` must survive the
    // rebuild unchanged: migration 6 made `memories.source_event_first`/
    // `_last` reference it, and a renumbered `seq` would silently re-point
    // every extracted memory's provenance at the wrong events with nothing
    // failing. The copy below therefore names `seq` explicitly in both the
    // column list and the `SELECT` instead of letting the new table's own
    // `AUTOINCREMENT` assign fresh values, and the old table is dropped only
    // after the copy lands.
    // `a_memorys_provenance_survives_the_seq_rebuild` in
    // `tests/events_lifecycle.rs` is the proof. `provider`, `model` and
    // `cause` are names only, never a credential, and prefixed `gateway_` so
    // a bare `model` column beside `resource` cannot read as naming the same
    // thing `gateway_unhealthy` already names with `resource`.
    //
    // History: design-decisions.md, "Trims: migration and native-secret
    // module docs", database/migrations/v1_to_v13.rs migration 7.
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
    // Seven columns, not one ambiguous agent identifier, because Phase 10's
    // second fixed requirement asks these to remain separately represented.
    // `ALTER TABLE ADD COLUMN` only, migration 7's reason. `model` holds
    // `harness-default` or `named:<id>` rather than a bare id, because
    // "Glasshouse assigned no model" is a different recorded fact from
    // "never recorded"; `pairing_class` and `protocol` have `unknown` for
    // the same reason. The three `CHECK`s copy vocabularies owned by
    // `harness::pairing`/`harness`/`harness::response`, pinned against drift
    // by `every_stored_vocabulary_is_one_the_schema_accepts`.
    // `response_profile` gets no `CHECK`: five axes joined, not one word.
    //
    // History: design-decisions.md, "Trims: migration and native-secret
    // module docs", database/migrations/v1_to_v13.rs migration 8.
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
    // A process id alone is not an identity: operating systems reuse them, so
    // `process_started_at` -- the kernel's own start time, in milliseconds
    // since the epoch -- makes the pair one. `process_host` is the third
    // part: a record whose host is not this one is only ever reported
    // unverifiable. NULL is "recorded nothing here", so a session predating
    // this migration reads as `session::supervision::Verdict::Unrecorded`,
    // never as stopped. Supervision is recorded rather than recomputed,
    // because the process observed at quarantine time may be gone by the
    // next open, and `supervision_reason` carries the sentence a person
    // needs. `ALTER TABLE ADD COLUMN` only: `lifecycle_events` is untouched,
    // so this is a column on `sessions`, never a new event kind.
    //
    // History: design-decisions.md, "Trims: migration and native-secret
    // module docs", database/migrations/v1_to_v13.rs migration 9.
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
    // `ADD COLUMN` only, migration 8's shape; `memories_fts` is untouched,
    // because these five columns are attributes of a memory already found,
    // not words a search would match on. `validity_conditions` and
    // `invalidation_conditions` are free text like the Phase 21B provenance
    // columns, for the same reason: a condition is a sentence, not a fixed
    // vocabulary, and NULL means "no condition was recorded", never "none
    // apply." `review_reason`'s six values are capability-map lines 885-890
    // in order, and this `CHECK` is their only definition —
    // [`crate::memory::ReviewReason`] reads it back so the two cannot drift.
    // `review_marked_at` and `last_validated_at` follow this schema's
    // standing rule that NULL is "unknown," never zero: a pre-migration
    // memory must decay as never-yet-validated, not as stale-as-of-epoch-zero.
    //
    // History: design-decisions.md, "Trims: migration and native-secret
    // module docs", database/migrations/v1_to_v13.rs migration 10.
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
    // audited against the raw rows rather than a counter that already
    // forgot what produced it.
    //
    // A new table, not more columns on `sessions`: migration 4's own
    // argument for `lifecycle_events` over a `sessions` column applies here.
    // `AUTOINCREMENT` and no `UPDATE` path, matching that
    // [`crate::routing::evidence`]'s store offers `record` and reads, never
    // an edit. `provider`, `model`, `route`, `harness`, `purpose` and
    // `quota_context` are the six columns two turns must agree on to be the
    // same evidence (line 1338, 1330); timing, token and cost columns are
    // nullable for line 1331's reason -- "when the protocol exposes them".
    // `context_state` is `NOT NULL DEFAULT 'unknown'`, because line 1337
    // forbids averaging away cache effects. The two triggers are migration
    // 4's isolation pair, for line 1343's project-scoping half.
    //
    // History: design-decisions.md, "Trims: migration and native-secret
    // module docs", database/migrations/v1_to_v13.rs migration 11.
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
    // `ALTER TABLE ADD COLUMN`, migration 8's shape. No `CHECK` and no
    // foreign key: this column holds a `SessionId`, not user text, and names
    // no `REFERENCES` because a source session can be in another project or
    // already gone -- the same precedent `memories.source_session_id`
    // (migration 6) sets. NULL means "not started from a checkpoint," never
    // a placeholder. One direction only: no index, reverse table or
    // descendants column, because `SessionStore::list()` already enumerates
    // every session, so "what came from this session" is a filter over an
    // existing enumeration, not a missing capability.
    //
    // History: design-decisions.md, "Trims: migration and native-secret
    // module docs", database/migrations/v1_to_v13.rs migration 12.
    "
    ALTER TABLE sessions ADD COLUMN source_session_id TEXT;
    ",
    // 13: capability map line 925 — "record why a decision was superseded so
    // future agents do not resurrect it without context."
    //
    // `ALTER TABLE ADD COLUMN`, migration 12's shape. Not `review_reason`:
    // that is a six-value enumeration meaning *why this memory needs
    // review*, constrained by migration 10's `CHECK`, and "why it was
    // superseded" is a different question with a different answer type -- a
    // sentence, not a vocabulary. No `CHECK` tying it to `status`, because
    // `superseded_by`'s tying `CHECK` is a table constraint `ALTER TABLE ADD
    // COLUMN` cannot add, and the rule is already enforced in code:
    // `MemoryStore::set_status` clears this column in the same expression it
    // clears `superseded_by`. The `CHECK` it does get is migration 8's shape
    // for operator free text -- not empty, bounded at 512 rather than
    // `display_name`'s 64, because this is a sentence explaining a decision.
    // NULL is "no reason was recorded," never a placeholder for an empty one.
    //
    // History: design-decisions.md, "Trims: migration and native-secret
    // module docs", database/migrations/v1_to_v13.rs migration 13.
    "
    ALTER TABLE memories ADD COLUMN superseded_reason TEXT
        CHECK (superseded_reason IS NULL
               OR (superseded_reason <> '' AND length(superseded_reason) <= 512));
    ",
];
