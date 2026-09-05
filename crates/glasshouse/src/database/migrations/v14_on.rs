//! Migrations 14 onwards, split out of `database.rs`'s `MIGRATIONS`
//! array by Phase 59's decomposition. Bodies through 26 are verbatim.

pub(super) const MIGRATIONS_V14_ON: [&str; 15] = [
    // 14: the order checkpoints were actually written in, because
    // `created_at` cannot carry it.
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
    // History: design-decisions.md, "Trims: database/migrations/v14_on.rs", migration 14.
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
    // # Retention, which is part of this migration's contract
    //
    // **The three ledgers before this one grow forever, and this one has the
    // highest write rate.** `lifecycle_events` cannot be trimmed even
    // deliberately, and `routing_observations`' own doc comment anticipates
    // "some future retention policy" that was never written.
    //
    // History: design-decisions.md, "Trims: database/migrations/v14_on.rs", migration 15.
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
    // History: design-decisions.md, "Trims: database/migrations/v14_on.rs", migration 16.
    "
    ALTER TABLE sessions ADD COLUMN observed_compactions INTEGER
        CHECK (observed_compactions IS NULL OR observed_compactions >= 0);
    ",
    // 17: which files were being worked on when a memory was learned —
    // Phase 28's missing primitive, and deliberately not Phase 28's
    // capability.
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
    // History: design-decisions.md, "Trims: database/migrations/v14_on.rs", migration 17.
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
    // History: design-decisions.md, "Trims: database/migrations/v14_on.rs", migration 18.
    "
    ALTER TABLE routing_observations ADD COLUMN failure_class TEXT;
    ",
    // 19: Phase 21K's assumption ledger — the few premises an agent states a
    // substantial change rests on, and what became of each.
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
    // History: design-decisions.md, "Trims: database/migrations/v14_on.rs", migration 19.
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
    // History: design-decisions.md, "Trims: database/migrations/v14_on.rs", migration 20.
    "
    ALTER TABLE sessions ADD COLUMN presentation_ref TEXT;
    ",
    // 21: the two facts a *memory commit* needs and this schema could not
    // hold — capability map lines 1147-1154.
    //
    // `NULL` is *"Glasshouse has not looked at HEAD for this session yet"*,
    // and the first look records it **without** treating it as a boundary —
    // nothing changed, a position was simply learned. That is the same
    // distinction migration 16 draws for `observed_compactions`, reached the
    // other way round: that column starts at a measured `0` because `create`
    // can measure it, and this one cannot, because `SessionStore::create` has
    // no project root to read a repository from.
    //
    // History: design-decisions.md, "Trims: database/migrations/v14_on.rs", migration 21.
    "
    ALTER TABLE sessions ADD COLUMN last_seen_commit TEXT;
    ALTER TABLE memories ADD COLUMN extraction_trigger TEXT;
    ",
    // 22: which entitlement served this session — capability map line 1972's
    // durable half, *"what it served"*.
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
    // History: design-decisions.md, "Trims: database/migrations/v14_on.rs", migration 22.
    "
    ALTER TABLE sessions ADD COLUMN entitlement TEXT;
    ",
    // 23: `routing_observations.task_class` — capability map line 1276's
    // *"short moving average of requests consumed per task class"*, whose
    // producer has existed since Phase 34C and whose row has never carried
    // it.
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
    // History: design-decisions.md, "Trims: database/migrations/v14_on.rs", migration 23.
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
    // # An unrecognised word reads back as `None`
    //
    // Migration 23's rule, not migration 18's: both stored vocabularies are
    // *bucketing* inputs to a ratio, and a row whose word this build does
    // not recognise is exactly as informative as a row from before the
    // column existed. Failing the whole row would let a future build's fifth
    // effort word break this build's savings readout, which is worse than
    // this build ignoring a bucket it never knew about.
    //
    // History: design-decisions.md, "Trims: database/migrations/v14_on.rs", migration 24.
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
    // History: design-decisions.md, "Trims: database/migrations/v14_on.rs", migration 25.
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
    // 26: `file_touched` — capability map line 1139's producer. The context
    // firewall's `PostToolUse` hook already sees the `file_path` of every
    // `Edit`, `Write`, `MultiEdit` and `NotebookEdit` a Claude Code session
    // makes; this is where it keeps one.
    //
    // **`seq` is named explicitly in both the column list and the `SELECT`,
    // and that is load-bearing rather than tidy.** `memories.source_event_
    // first` and `.source_event_last` reference it, so a rebuild that let
    // `AUTOINCREMENT` assign fresh values would silently re-point every
    // extracted memory's provenance at different events — nothing would
    // fail, the data would just be wrong.
    // `a_memorys_provenance_survives_the_seq_rebuild` in
    // `tests/events_lifecycle.rs` was written against a deliberately naive
    // rebuild and covers this one too.
    //
    // History: design-decisions.md, "Trims: database/migrations/v14_on.rs", migration 26.
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
                            'gateway_backend_changed', 'file_touched')),

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
        path             TEXT
            CHECK (path IS NULL OR path <> ''),

        -- The harness report this was translated from, when it was translated
        -- from one. Both or neither.
        observed_harness TEXT,
        observed_event   TEXT,
        CHECK ((observed_harness IS NULL) = (observed_event IS NULL)),
        CHECK ((kind = 'file_touched') = (path IS NOT NULL))
    );

    INSERT INTO lifecycle_events_new (
        seq, project_id, session_id, at, kind,
        turn_outcome, origin, bytes, exit_code, exit_signal,
        resource, gateway_reason, gateway_provider, gateway_model,
        gateway_cause, observed_harness, observed_event
    )
    SELECT
        seq, project_id, session_id, at, kind,
        turn_outcome, origin, bytes, exit_code, exit_signal,
        resource, gateway_reason, gateway_provider, gateway_model,
        gateway_cause, observed_harness, observed_event
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
    // 27: `file_claims` — Phase 60's soft, project-scoped, turn-scoped file
    // claims (map lines 2392-2398).
    //
    // # Two sessions may claim one path, and that is not an error
    //
    // Nothing here is unique on `path`. A claim is coordination metadata: it
    // never locks, never blocks and never fails another session's write, so a
    // second claimant is the overlap a later package reports, not a
    // constraint violation this one raises.
    //
    // # `session_id` and not a process id — line 2396
    //
    // `NOT NULL`, and migration 12's rule on the reference itself: a bare id
    // with no `REFERENCES`, because the sessions row may be trimmed and a
    // read that cannot resolve one drops the claim rather than failing. A pid
    // is deliberately absent: pids are recycled, and a recycled pid resolving
    // to a live claim is exactly what the line forbids.
    //
    // History: design-decisions.md, "Trims: database/migrations/v14_on.rs", migration 27.
    "
    CREATE TABLE file_claims (
        project_id  TEXT    NOT NULL,
        -- The owning Glasshouse session, never a process id.
        session_id  TEXT    NOT NULL,
        -- Repo-relative, `/`-separated, UTF-8, never absolute:
        -- `memory_files.path`'s spelling, enforced by the same function
        -- (`crate::memory::normalize_observed_path`) at the same kind of
        -- door. The schema can only refuse the empty string, for migration
        -- 17's reason.
        path        TEXT    NOT NULL CHECK (path <> ''),
        -- Seconds since the Unix epoch. `claimed_at` survives a renew;
        -- `renewed_at` and `expires_at` are what a renew moves.
        claimed_at  INTEGER NOT NULL,
        renewed_at  INTEGER NOT NULL,
        expires_at  INTEGER NOT NULL,

        PRIMARY KEY (session_id, path)
    );

    -- The other direction: who else holds this path. Deliberately not
    -- UNIQUE -- see the comment above.
    CREATE INDEX file_claims_by_path ON file_claims (path, session_id);

    CREATE TRIGGER file_claims_reject_foreign_project_insert
    BEFORE INSERT ON file_claims
    FOR EACH ROW
    WHEN NEW.project_id IS NOT (
        SELECT value FROM project_metadata WHERE key = 'project_id'
    )
    BEGIN
        SELECT RAISE(ABORT, 'file claim belongs to a different project');
    END;

    CREATE TRIGGER file_claims_reject_foreign_project_update
    BEFORE UPDATE OF project_id ON file_claims
    FOR EACH ROW
    WHEN NEW.project_id IS NOT (
        SELECT value FROM project_metadata WHERE key = 'project_id'
    )
    BEGIN
        SELECT RAISE(ABORT, 'file claim belongs to a different project');
    END;
    ",
    // 28: `task_progress_declarations` — the honest producer of
    // `provider::quota::ReserveDecisionInputs::task_nearly_complete`, map
    // lines 1294 and 1610.
    //
    // # Nothing here describes the work
    //
    // There is deliberately no note, label or summary column. A declaration
    // is one bit plus a scope plus a horizon; a free-text column would be a
    // place for prompt text or session content to reach a table that exists
    // to answer a boolean, and `session::store::tests`'s schema inventory
    // holds this build to having nowhere to put one.
    //
    // History: design-decisions.md, "Trims: database/migrations/v14_on.rs", migration 28.
    "
    CREATE TABLE task_progress_declarations (
        project_id   TEXT    NOT NULL,
        -- The session whose current task was declared nearly complete,
        -- never a process id.
        session_id   TEXT    NOT NULL,
        -- Seconds since the Unix epoch. `declared_at` survives a renew;
        -- `renewed_at` and `expires_at` are what a renew moves.
        declared_at  INTEGER NOT NULL,
        renewed_at   INTEGER NOT NULL,
        expires_at   INTEGER NOT NULL,

        PRIMARY KEY (session_id)
    );

    CREATE TRIGGER task_progress_reject_foreign_project_insert
    BEFORE INSERT ON task_progress_declarations
    FOR EACH ROW
    WHEN NEW.project_id IS NOT (
        SELECT value FROM project_metadata WHERE key = 'project_id'
    )
    BEGIN
        SELECT RAISE(ABORT, 'task progress belongs to a different project');
    END;

    CREATE TRIGGER task_progress_reject_foreign_project_update
    BEFORE UPDATE OF project_id ON task_progress_declarations
    FOR EACH ROW
    WHEN NEW.project_id IS NOT (
        SELECT value FROM project_metadata WHERE key = 'project_id'
    )
    BEGIN
        SELECT RAISE(ABORT, 'task progress belongs to a different project');
    END;
    ",
];
