//! Migrations 14 onwards, split out of `database.rs`'s `MIGRATIONS`
//! array by Phase 59's decomposition. Bodies through 26 are verbatim.

pub(super) const MIGRATIONS_V14_ON: [&str; 14] = [
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
    // 26: `file_touched` — capability map line 1139's producer. The context
    // firewall's `PostToolUse` hook already sees the `file_path` of every
    // `Edit`, `Write`, `MultiEdit` and `NotebookEdit` a Claude Code session
    // makes; this is where it keeps one.
    //
    // # Migration 7's shape, for migration 7's reason
    //
    // SQLite cannot add or drop a `CHECK`, and migration 5's `kind` column
    // is one. Admitting a twelfth value is therefore rename, recreate, copy,
    // drop, then recreate the index and all three triggers — exactly what
    // migration 7 paid to admit the eleventh, and the comment there is the
    // one to read for why the alternative does not exist.
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
    // # `path`, and its two `CHECK`s
    //
    // Repo-relative, `/`-separated, never absolute — `crate::memory::store::
    // normalize_observed_path`'s contract, applied by the writer, for the
    // reasons migration 17 gives about `memory_files.path`: the schema can
    // refuse an empty string and nothing more, because `CHECK (path NOT LIKE
    // '/%')` would miss `C:\...` and a `CHECK` forbidding `\` or `:` would
    // reject file names that are legal on Unix.
    //
    // The second `CHECK` is the biconditional the other payload columns do
    // not have and this one can: `file_touched` is the only kind that
    // carries a path, and a path is the only thing that kind carries. So
    // `(kind = 'file_touched') = (path IS NOT NULL)` refuses both a
    // `file_touched` with nothing to point at and a `turn_ended` that
    // somehow acquired a path. `crate::events::log::read_row` would report
    // the first as `MissingValue`; the schema is where it is cheaper to
    // refuse it than to read it back.
    //
    // # Why an event, and not a table of its own
    //
    // `crate::memory::extract::lifecycle::chunk_for_session` already reads a
    // session's events in order, renders each with `describe`, and derives
    // every memory's provenance range from the tail that survived the
    // budget. A second source would need a second ordering and a second
    // range; an event slots into the reader that exists.
    //
    // This is not the noise `REPORTED_EVENTS` refuses. That list keeps
    // `PostToolUse` out of the *lifecycle state machine*: `file_touched` is
    // appended by the firewall subprocess that already runs on every tool
    // call, `crate::events::LifecycleEvent::implied_state` answers `None`
    // for it, and every `match` on that enum in this crate is exhaustive, so
    // the compiler names each consumer that has to say so.
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
    // # One row per (session, path), which is what makes a renew a renew
    //
    // The primary key is `(session_id, path)`, so a session claiming a file
    // it already holds can only ever move `renewed_at` and `expires_at` on
    // the row it already has — line 2395's *"renew rather than create a
    // second one"* is the table's shape and not a rule the writer has to
    // remember. `claimed_at` is left alone by a renew, so *"since when"* and
    // *"still wanted as of"* stay two separate facts.
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
    // # Project scope — line 2397
    //
    // Migration 15's two triggers, copied exactly. The database file is the
    // project, so a claim written in one project is not merely filtered out
    // of another project's reads — there is no query in another project's
    // database that could name it — and a row carrying a foreign
    // `project_id` is refused before it is written.
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
];
