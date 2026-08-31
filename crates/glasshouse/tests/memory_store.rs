//! Durable project memory, exercised the way a caller reaches it.
//!
//! An integration test on purpose: every path here goes through
//! `glasshouse::bootstrap`, which is what a real launch does, so the migration,
//! the project binding, the file permissions and the triggers are all in play
//! rather than mocked away.

use std::path::Path;
use std::sync::{Arc, Mutex};

use glasshouse::memory::{
    ConflictResolver, MemoryAuthority, MemoryId, MemoryKind, MemoryRefusal, MemoryStatus,
    MemoryStoreError, NewMemory, ProjectMemory, ReviewReason,
};
use glasshouse::{Cli, Runtime};

use clap::Parser;

/// A bootstrapped project inside `base`, sharing `base`'s data and config
/// roots. Two fixtures over one `base` are two real projects on one machine.
struct Fixture {
    _root: std::path::PathBuf,
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
        let runtime = glasshouse::bootstrap(&cli, &root).unwrap();
        Fixture {
            _root: root,
            runtime,
        }
    }

    fn memory(&self) -> ProjectMemory {
        ProjectMemory::open(&self.runtime).unwrap()
    }

    /// A memory store whose clock is under the test's control, so timestamps
    /// can be asserted exactly instead of slept for.
    fn memory_at(&self, ticks: &Arc<Mutex<i64>>) -> ProjectMemory {
        let ticks = Arc::clone(ticks);
        ProjectMemory::open_with_clock(&self.runtime, Arc::new(move || *ticks.lock().unwrap()))
            .unwrap()
    }
}

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

// -------------------------------------------------------------------------
// Phase 20 — the table, its kinds, its columns.
// -------------------------------------------------------------------------

/// All six of Phase 20's memory kinds round-trip through SQLite unchanged.
///
/// One test rather than six, because the property is the same property and
/// `MemoryKind::ALL` makes it exhaustive: a seventh kind added without a
/// migration fails this test on the `CHECK` constraint rather than passing
/// unnoticed.
#[test]
fn every_memory_kind_round_trips_through_the_project_database() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    assert_eq!(
        MemoryKind::ALL.len(),
        6,
        "Phase 20 names exactly six memory kinds"
    );

    for kind in MemoryKind::ALL {
        let recorded = store
            .record(NewMemory::new(
                *kind,
                format!("a durable {kind} worth keeping across sessions"),
            ))
            .unwrap_or_else(|error| panic!("recording a {kind} failed: {error}"));
        assert_eq!(recorded.kind, *kind);

        let read_back = store.get(&recorded.id).unwrap().expect("just stored");
        assert_eq!(
            read_back.kind, *kind,
            "{kind} did not survive the round trip"
        );
        assert_eq!(read_back, recorded);
    }
}

/// All seven of Phase 20's and Phase 22's lifecycle statuses round-trip.
#[test]
fn every_lifecycle_status_round_trips_and_only_active_is_current() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    // The six Phase 20 requires, plus Phase 22's conflict state.
    for required in [
        MemoryStatus::Active,
        MemoryStatus::Superseded,
        MemoryStatus::Rejected,
        MemoryStatus::Resolved,
        MemoryStatus::NeedsReview,
        MemoryStatus::Invalidated,
        MemoryStatus::Conflicted,
    ] {
        assert!(
            MemoryStatus::ALL.contains(&required),
            "{required} must be a supported status"
        );
    }

    for status in MemoryStatus::ALL {
        let recorded = store
            .record(NewMemory::new(
                MemoryKind::Finding,
                format!("a finding that ends up {status}"),
            ))
            .unwrap();
        let moved = store.set_status(&recorded.id, *status).unwrap();
        assert_eq!(moved.status, *status);

        let read_back = store.get(&recorded.id).unwrap().expect("just stored");
        assert_eq!(
            read_back.status, *status,
            "{status} did not survive the round trip"
        );
        assert_eq!(
            read_back.is_current(),
            *status == MemoryStatus::Active,
            "only an active memory is current knowledge; {status} claimed otherwise"
        );
    }
}

/// Subject, session and commit are stored when available and stay absent when
/// they are not — `None` and `Some("")` must not become the same fact.
#[test]
fn subject_session_and_commit_are_stored_when_available_and_absent_when_not() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let with_everything = store
        .record(
            NewMemory::new(
                MemoryKind::Decision,
                "the gateway streams responses through untouched, so no \
                 transparent decompression may be enabled",
            )
            .with_subject(Some("gateway passes bodies through"))
            .with_source_session(Some("session-7"))
            .with_source_commit(Some("a4ccc3b")),
        )
        .unwrap();

    let read_back = store.get(&with_everything.id).unwrap().unwrap();
    assert_eq!(
        read_back.subject.as_deref(),
        Some("gateway passes bodies through")
    );
    assert_eq!(read_back.source_session_id.as_deref(), Some("session-7"));
    assert_eq!(read_back.source_commit.as_deref(), Some("a4ccc3b"));
    assert_eq!(
        read_back.body,
        "the gateway streams responses through untouched, so no \
         transparent decompression may be enabled"
    );

    let bare = store
        .record(NewMemory::new(
            MemoryKind::Finding,
            "ConPTY reflows long lines; a Unix pty does not",
        ))
        .unwrap();
    let read_back = store.get(&bare.id).unwrap().unwrap();
    assert_eq!(read_back.subject, None);
    assert_eq!(read_back.source_session_id, None);
    assert_eq!(read_back.source_commit, None);

    // An empty subject is not a subject. Storing `Some("")` would make
    // "no subject was available" and "the subject is blank" indistinguishable.
    let blank = store
        .record(
            NewMemory::new(MemoryKind::Todo, "wire the memory command into main")
                .with_subject(Some("   "))
                .with_source_commit(Some("")),
        )
        .unwrap();
    let read_back = store.get(&blank.id).unwrap().unwrap();
    assert_eq!(read_back.subject, None);
    assert_eq!(read_back.source_commit, None);
}

/// Creation and update timestamps are recorded, and only the update timestamp
/// moves when a memory changes.
#[test]
fn creation_and_update_timestamps_are_recorded_and_only_updated_at_moves() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let ticks = Arc::new(Mutex::new(1_000i64));
    let memory = fixture.memory_at(&ticks);
    let store = memory.store();

    let recorded = store
        .record(NewMemory::new(
            MemoryKind::Constraint,
            "MSRV is read out of Cargo.toml so the gate and the manifest \
             cannot drift",
        ))
        .unwrap();
    assert_eq!(recorded.created_at, 1_000);
    assert_eq!(recorded.updated_at, 1_000);

    *ticks.lock().unwrap() = 2_500;
    let moved = store
        .set_status(&recorded.id, MemoryStatus::NeedsReview)
        .unwrap();
    assert_eq!(
        moved.created_at, 1_000,
        "creation time is when it was learned and never moves"
    );
    assert_eq!(moved.updated_at, 2_500);
}

/// The authority column exists from Phase 20 so Phase 21A needs no migration,
/// it round-trips every class, and an unclassified memory stays unclassified.
#[test]
fn authority_round_trips_every_class_and_unclassified_stays_none() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    assert_eq!(
        MemoryAuthority::ALL.len(),
        7,
        "Phase 21A names exactly seven authority classes"
    );

    for authority in MemoryAuthority::ALL {
        let recorded = store
            .record(
                NewMemory::new(
                    MemoryKind::Decision,
                    format!("something held at {authority} authority"),
                )
                .with_authority(Some(*authority)),
            )
            .unwrap();
        let read_back = store.get(&recorded.id).unwrap().unwrap();
        assert_eq!(read_back.authority, Some(*authority));
    }

    // Nothing classifies yet, so the common case is `None` — and it must stay
    // `None` rather than being defaulted into some class nobody chose.
    let unclassified = store
        .record(NewMemory::new(
            MemoryKind::Finding,
            "the leaf tier returned 171 of 171 quotes verbatim",
        ))
        .unwrap();
    assert_eq!(unclassified.authority, None);
    assert_eq!(
        store.get(&unclassified.id).unwrap().unwrap().authority,
        None
    );
}

/// An existing version-3 database gains the memory table on the next launch,
/// with everything already in it intact.
///
/// The migration is the expensive thing to get wrong in this batch: every
/// project that has ever run Glasshouse already has a database, so migration 4
/// runs against real files far more often than it runs against a fresh one.
/// A test that only ever bootstraps from nothing would never exercise that
/// path at all.
#[test]
fn a_version_three_database_gains_the_memory_table_with_its_sessions_intact() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let db_path = fixture.runtime.database_path();

    // Roll the database back to what version 3 left behind: drop everything
    // migration 4 created, and forget that it ran.
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "INSERT INTO sessions (id, project_id, harness, role, lifecycle, \
                                   presentation, created_at, last_activity_at) \
             SELECT 'session-1', value, 'claude-code', 'worker', 'stopped', \
                    'headless', 111, 222 FROM project_metadata \
             WHERE key = 'project_id';
             DROP TRIGGER memories_fts_after_update;
             DROP TRIGGER memories_fts_after_delete;
             DROP TRIGGER memories_fts_after_insert;
             DROP TABLE memories_fts;
             DROP TRIGGER memories_reject_unknown_supersession_update;
             DROP TRIGGER memories_reject_unknown_supersession_insert;
             DROP TRIGGER memories_reject_foreign_project_update;
             DROP TRIGGER memories_reject_foreign_project_insert;
             DROP TABLE memories;
             DROP TABLE IF EXISTS lifecycle_events;
             DROP TABLE IF EXISTS checkpoints;
             -- Migration 8's columns go with the row that records it: the
             -- runner resumes from MAX(version), so leaving them behind
             -- re-applies 8 against a table that already has them.
             ALTER TABLE sessions DROP COLUMN model;
             ALTER TABLE sessions DROP COLUMN pairing_class;
             ALTER TABLE sessions DROP COLUMN protocol;
             ALTER TABLE sessions DROP COLUMN response_profile;
             ALTER TABLE sessions DROP COLUMN response_mechanism;
             ALTER TABLE sessions DROP COLUMN display_name;
             ALTER TABLE sessions DROP COLUMN purpose;
             ALTER TABLE sessions DROP COLUMN process_id;
             ALTER TABLE sessions DROP COLUMN process_started_at;
             ALTER TABLE sessions DROP COLUMN process_host;
             ALTER TABLE sessions DROP COLUMN supervision;
             ALTER TABLE sessions DROP COLUMN supervision_reason;
             ALTER TABLE sessions DROP COLUMN source_session_id;
             ALTER TABLE sessions DROP COLUMN observed_compactions;

             -- Migration 21's column, for the same reason as every other
             -- `sessions` column above: the re-run would meet a column it had
             -- already added. `memories.extraction_trigger` needs no undo here
             -- because this rollback drops `memories` outright.
             ALTER TABLE sessions DROP COLUMN last_seen_commit;
             ALTER TABLE sessions DROP COLUMN presentation_ref;
             DROP TABLE IF EXISTS routing_observations;
             DROP TABLE IF EXISTS evaluation_observations;
             DROP TABLE IF EXISTS memory_files;
             DROP TABLE IF EXISTS assumption_transitions;
             DROP TABLE IF EXISTS task_assumptions;
             DELETE FROM schema_migrations WHERE version >= 4;",
        )
        .unwrap();

        let version: i64 = conn
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, 3, "the rollback must land on version 3");
    }

    // The next launch is an ordinary bootstrap; nothing special is asked of it.
    let cli = Cli::try_parse_from([
        "glasshouse",
        "--data-dir",
        tmp.path().join("data").to_str().unwrap(),
        "--config-dir",
        tmp.path().join("config").to_str().unwrap(),
    ])
    .unwrap();
    let migrated = glasshouse::bootstrap(&cli, fixture.runtime.project().root()).unwrap();

    let conn = rusqlite::Connection::open(migrated.database_path()).unwrap();
    let version: i64 = conn
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(
        version, 21,
        "the launch must have applied migrations 4 through 21"
    );

    // The session recorded before the migration is untouched.
    let harness: String = conn
        .query_row(
            "SELECT harness FROM sessions WHERE id = 'session-1'",
            [],
            |row| row.get(0),
        )
        .expect("a session recorded before the migration must survive it");
    assert_eq!(harness, "claude-code");

    // And the memory table works, full-text index included, on the upgraded
    // file rather than only on a freshly created one.
    let memory = ProjectMemory::open(&migrated).unwrap();
    let store = memory.store();
    let recorded = store
        .record(
            NewMemory::new(
                MemoryKind::Finding,
                "the upgraded database indexes new memories for search",
            )
            .with_subject(Some("upgrade path")),
        )
        .unwrap();
    assert_eq!(store.get(&recorded.id).unwrap().unwrap().id, recorded.id);

    let indexed: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM memories_fts WHERE memories_fts MATCH ?1",
            ["\"indexes\""],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        indexed, 1,
        "migration 4's triggers must index a memory written after the upgrade"
    );
}

// -------------------------------------------------------------------------
// Phase 20 — what the table refuses to hold.
// -------------------------------------------------------------------------

/// A body that is nothing but a conversational acknowledgement is refused, and
/// leaves no row behind.
#[test]
fn raw_conversation_filler_is_refused_and_stores_nothing() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    for filler in ["ok", "OK!", "  thanks. ", "Sounds good", "lgtm", "got it"] {
        let error = store
            .record(NewMemory::new(MemoryKind::Finding, filler))
            .expect_err(&format!("`{filler}` is filler and must be refused"));
        assert!(
            matches!(
                error,
                MemoryStoreError::Refused(MemoryRefusal::ConversationFiller)
            ),
            "`{filler}` was refused for the wrong reason: {error}"
        );
    }

    let empty = store
        .record(NewMemory::new(MemoryKind::Finding, "   \n  "))
        .expect_err("an empty body is not a memory");
    assert!(matches!(
        empty,
        MemoryStoreError::Refused(MemoryRefusal::Empty)
    ));

    assert_eq!(
        store.count(MemoryStatus::Active).unwrap(),
        0,
        "a refused memory must leave no row behind"
    );

    // The guard is about a body that is *nothing but* filler. A real memory
    // that happens to contain one of those words is a real memory.
    store
        .record(NewMemory::new(
            MemoryKind::Finding,
            "ok is not a useful search term because FTS5 strips it as a stopword",
        ))
        .expect("a real memory containing a filler word is still a memory");
    assert_eq!(store.count(MemoryStatus::Active).unwrap(), 1);
}

/// A step-by-step plan is refused unless it is being recorded as the decision
/// or constraint it became — Phase 20's one explicit escape.
#[test]
fn a_step_by_step_plan_is_refused_unless_it_became_a_decision_or_constraint() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    const PLAN: &str = "1. read the failure log\n\
                        2. reproduce it locally under load\n\
                        3. only then write the fix";

    for refused in [
        MemoryKind::Feature,
        MemoryKind::Finding,
        MemoryKind::FailedAttempt,
        MemoryKind::Todo,
    ] {
        let error = store
            .record(NewMemory::new(refused, PLAN))
            .expect_err(&format!("a plan filed as {refused} must be refused"));
        assert!(
            matches!(
                error,
                MemoryStoreError::Refused(MemoryRefusal::TemporaryPlan)
            ),
            "a plan filed as {refused} was refused for the wrong reason: {error}"
        );
    }

    for permitted in [MemoryKind::Decision, MemoryKind::Constraint] {
        store
            .record(NewMemory::new(permitted, PLAN))
            .unwrap_or_else(|error| {
                panic!("a plan that became a {permitted} must be stored: {error}")
            });
    }

    // An enumeration is not a plan. Three findings numbered out of order, or a
    // two-item list, stay storable — the guard's bar is consecutive steps from
    // one, and everything below it is admitted.
    store
        .record(NewMemory::new(
            MemoryKind::Finding,
            "3. the third probe answered 401\n1. the first answered 200",
        ))
        .expect("an out-of-order enumeration is not a step-by-step plan");
    store
        .record(NewMemory::new(
            MemoryKind::Finding,
            "1. macOS reflows nothing\n2. Windows reflows at the buffer width",
        ))
        .expect("a two-item list is not a step-by-step plan");
}

// -------------------------------------------------------------------------
// Phase 22 — lifecycle and supersession.
// -------------------------------------------------------------------------

/// A new memory supersedes an older one: the old one stops being current,
/// names its successor, and is still there.
#[test]
fn superseding_a_memory_retires_it_without_deleting_its_history() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let old = store
        .record(NewMemory::new(
            MemoryKind::Decision,
            "the MSRV gate runs `rustup run 1.85.0 cargo check`",
        ))
        .unwrap();
    let new = store
        .record(NewMemory::new(
            MemoryKind::Decision,
            "the MSRV gate pins both cargo and rustc through `rustup which`, \
             because `rustup run` lets cargo resolve rustc from PATH",
        ))
        .unwrap();

    let retired = store.supersede(&old.id, &new.id).unwrap();

    assert_eq!(retired.status, MemoryStatus::Superseded);
    assert!(
        !retired.is_current(),
        "a superseded memory is not current knowledge"
    );
    assert_eq!(
        retired.superseded_by.as_ref(),
        Some(&new.id),
        "the superseding memory's identifier is what stops a later agent \
         resurrecting the old decision"
    );

    // History, not deletion: the old memory is still readable in full.
    let still_there = store.get(&old.id).unwrap().expect("history is not deleted");
    assert_eq!(still_there.body, old.body);
    assert_eq!(still_there.created_at, old.created_at);

    // And the replacement is untouched and current.
    let replacement = store.get(&new.id).unwrap().unwrap();
    assert_eq!(replacement.status, MemoryStatus::Active);
    assert_eq!(replacement.superseded_by, None);
}

/// Normal retrieval prefers active memories; rejected decisions and failed
/// approaches stay reachable as history.
#[test]
fn normal_retrieval_returns_active_memories_while_history_stays_reachable() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let current = store
        .record(NewMemory::new(
            MemoryKind::Decision,
            "supersession is enforced by a trigger, not a foreign key",
        ))
        .unwrap();
    let rejected = store
        .record(NewMemory::new(
            MemoryKind::Decision,
            "turn PRAGMA foreign_keys on for every connection",
        ))
        .unwrap();
    store
        .set_status(&rejected.id, MemoryStatus::Rejected)
        .unwrap();
    let failed = store
        .record(NewMemory::new(
            MemoryKind::FailedAttempt,
            "rustup target add x86_64-unknown-linux-gnu — core/std did not resolve",
        ))
        .unwrap();
    store
        .set_status(&failed.id, MemoryStatus::Invalidated)
        .unwrap();

    let active: Vec<MemoryId> = store
        .with_status(MemoryStatus::Active, 50)
        .unwrap()
        .into_iter()
        .map(|record| record.id)
        .collect();
    assert_eq!(
        active,
        vec![current.id.clone()],
        "normal retrieval returns only current knowledge"
    );

    // Both are still there, still readable, still carrying their reason.
    assert_eq!(
        store.get(&rejected.id).unwrap().unwrap().status,
        MemoryStatus::Rejected,
        "a rejected decision stays searchable as historical knowledge"
    );
    assert_eq!(
        store.get(&failed.id).unwrap().unwrap().body,
        failed.body,
        "a failed approach stays readable so it is not tried again"
    );
}

/// A resolved todo is still queryable and is no longer open work.
#[test]
fn a_resolved_todo_stays_queryable_without_being_open_work() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let todo = store
        .record(NewMemory::new(
            MemoryKind::Todo,
            "prove the Codex hook document actually blocks",
        ))
        .unwrap();
    assert!(todo.is_open_todo(), "a new todo is open work");

    let resolved = store.set_status(&todo.id, MemoryStatus::Resolved).unwrap();
    assert!(
        !resolved.is_open_todo(),
        "a resolved todo must never be presented as open work"
    );

    let queried = store
        .get(&todo.id)
        .unwrap()
        .expect("a resolved todo is still queryable");
    assert_eq!(queried.status, MemoryStatus::Resolved);
    assert_eq!(queried.body, todo.body);
    assert_eq!(
        store.count(MemoryStatus::Resolved).unwrap(),
        1,
        "it is still in the database, just not open"
    );
}

/// A supersession that names a memory this project does not have is refused,
/// and nothing may supersede itself.
#[test]
fn a_supersession_must_name_a_memory_this_project_actually_has() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let existing = store
        .record(NewMemory::new(
            MemoryKind::Decision,
            "memories carry a rowid because external-content FTS5 needs one",
        ))
        .unwrap();

    let invented = MemoryId::new("ffffffffffffffffffffffffffffffff");
    let error = store
        .supersede(&existing.id, &invented)
        .expect_err("a supersession pointing nowhere must be refused");
    assert!(
        matches!(error, MemoryStoreError::NotFound { .. }),
        "unexpected error: {error}"
    );

    let error = store
        .supersede(&existing.id, &existing.id)
        .expect_err("nothing supersedes itself");
    assert!(
        matches!(error, MemoryStoreError::SelfSupersession { .. }),
        "unexpected error: {error}"
    );

    // The refusals changed nothing.
    let untouched = store.get(&existing.id).unwrap().unwrap();
    assert_eq!(untouched.status, MemoryStatus::Active);
    assert_eq!(untouched.superseded_by, None);
}

/// Two contradictory current memories are flagged rather than silently both
/// returned as truth.
#[test]
fn contradictory_current_memories_are_flagged_and_leave_normal_retrieval() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let one = store
        .record(NewMemory::new(
            MemoryKind::Constraint,
            "the project database is opened read-write or not at all",
        ))
        .unwrap();
    let other = store
        .record(NewMemory::new(
            MemoryKind::Constraint,
            "a read-only project database is acceptable for search",
        ))
        .unwrap();

    // Before flagging, both look like settled current knowledge — which is
    // exactly the state Phase 22 forbids leaving them in.
    assert_eq!(
        store.with_status(MemoryStatus::Active, 50).unwrap().len(),
        2
    );

    let (first, second) = store.mark_conflicted(&one.id, &other.id).unwrap();
    assert_eq!(first.status, MemoryStatus::Conflicted);
    assert_eq!(second.status, MemoryStatus::Conflicted);
    assert!(!first.is_current() && !second.is_current());

    assert!(
        store
            .with_status(MemoryStatus::Active, 50)
            .unwrap()
            .is_empty(),
        "neither side of an unresolved conflict may still be returned as \
         current truth"
    );
    assert_eq!(store.count(MemoryStatus::Conflicted).unwrap(), 2);
}

/// A high-impact conflict may not be resolved automatically; review can.
#[test]
fn a_high_impact_conflict_needs_review_and_an_ordinary_one_does_not() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    // Binding authorities are high-impact.
    for binding in [
        MemoryAuthority::Invariant,
        MemoryAuthority::Constraint,
        MemoryAuthority::Decision,
    ] {
        let record = store
            .record(
                NewMemory::new(
                    MemoryKind::Constraint,
                    format!("a {binding} that ended up in conflict"),
                )
                .with_authority(Some(binding)),
            )
            .unwrap();
        store
            .set_status(&record.id, MemoryStatus::Conflicted)
            .unwrap();

        let refusal = store
            .resolve_conflict(
                &record.id,
                MemoryStatus::Active,
                ConflictResolver::Automatic,
            )
            .expect_err(&format!("{binding} is high-impact and needs review"));
        assert!(
            matches!(refusal, MemoryStoreError::ReviewRequired { .. }),
            "unexpected error for {binding}: {refusal}"
        );
        assert_eq!(
            store.get(&record.id).unwrap().unwrap().status,
            MemoryStatus::Conflicted,
            "a refused automatic resolution must change nothing"
        );

        // A reviewer may settle exactly the same conflict.
        let settled = store
            .resolve_conflict(&record.id, MemoryStatus::Active, ConflictResolver::Reviewed)
            .unwrap();
        assert_eq!(settled.status, MemoryStatus::Active);
    }

    // An unclassified authority is treated as high-impact too: `None` means
    // nobody judged how binding it is, and unknown must not mean safe.
    let unclassified = store
        .record(NewMemory::new(
            MemoryKind::Finding,
            "nobody has classified how binding this is",
        ))
        .unwrap();
    store
        .set_status(&unclassified.id, MemoryStatus::Conflicted)
        .unwrap();
    let refusal = store
        .resolve_conflict(
            &unclassified.id,
            MemoryStatus::Active,
            ConflictResolver::Automatic,
        )
        .expect_err("an unclassified authority must fail closed");
    assert!(matches!(refusal, MemoryStoreError::ReviewRequired { .. }));

    // A low-authority memory is ordinary work an agent may settle itself.
    for ordinary in [
        MemoryAuthority::Preference,
        MemoryAuthority::Hypothesis,
        MemoryAuthority::Idea,
        MemoryAuthority::Historical,
    ] {
        let record = store
            .record(
                NewMemory::new(MemoryKind::Feature, format!("an ordinary {ordinary}"))
                    .with_authority(Some(ordinary)),
            )
            .unwrap();
        store
            .set_status(&record.id, MemoryStatus::Conflicted)
            .unwrap();
        let settled = store
            .resolve_conflict(
                &record.id,
                MemoryStatus::Rejected,
                ConflictResolver::Automatic,
            )
            .unwrap_or_else(|error| panic!("{ordinary} is not high-impact: {error}"));
        assert_eq!(settled.status, MemoryStatus::Rejected);
    }
}

// -------------------------------------------------------------------------
// Phase 21G — revalidation: mark a memory reaffirmed, needs-review,
// superseded, or invalidated, gated the way Phase 22 already gates a
// conflict resolution.
// -------------------------------------------------------------------------

/// Acceptance test 2: each revalidation outcome leaves the memory in the
/// matching status, and `superseded` records the successor named.
#[test]
fn every_revalidation_outcome_leaves_the_matching_status() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let reaffirmed = store
        .record(NewMemory::new(MemoryKind::Finding, "reaffirmed candidate"))
        .unwrap();
    store
        .mark_for_review(&reaffirmed.id, ReviewReason::ProjectState)
        .unwrap();
    let result = store
        .revalidate_reaffirmed(&reaffirmed.id, ConflictResolver::Reviewed)
        .unwrap();
    assert_eq!(result.status, MemoryStatus::Active);
    assert!(
        result.last_validated_at.is_some(),
        "reaffirmed must record a validation timestamp"
    );

    let needs_review = store
        .record(NewMemory::new(
            MemoryKind::Finding,
            "needs-review candidate",
        ))
        .unwrap();
    let result = store
        .revalidate_needs_review(
            &needs_review.id,
            ReviewReason::ArchitectureDrift,
            ConflictResolver::Reviewed,
        )
        .unwrap();
    assert_eq!(result.status, MemoryStatus::NeedsReview);
    assert_eq!(result.review_reason, Some(ReviewReason::ArchitectureDrift));

    let old = store
        .record(NewMemory::new(MemoryKind::Finding, "old candidate"))
        .unwrap();
    let successor = store
        .record(NewMemory::new(MemoryKind::Finding, "successor candidate"))
        .unwrap();
    let result = store
        .revalidate_superseded(&old.id, &successor.id, None, ConflictResolver::Reviewed)
        .unwrap();
    assert_eq!(result.status, MemoryStatus::Superseded);
    assert_eq!(result.superseded_by, Some(successor.id));

    let invalidated = store
        .record(NewMemory::new(MemoryKind::Finding, "invalidated candidate"))
        .unwrap();
    let result = store
        .revalidate_invalidated(&invalidated.id, ConflictResolver::Reviewed)
        .unwrap();
    assert_eq!(result.status, MemoryStatus::Invalidated);
    assert!(
        !result.is_current(),
        "an invalidated memory must never be current knowledge"
    );
}

/// Acceptance test 3: an automatic reviewer is refused a high-impact
/// revalidation — a binding authority, and an unclassified one — while a
/// reviewed actor may revalidate either. Mirrors
/// `a_high_impact_conflict_needs_review_and_an_ordinary_one_does_not`
/// exactly, because revalidation reuses `resolve_conflict`'s own gate rather
/// than a new one.
#[test]
fn an_automatic_reviewer_is_refused_a_high_impact_revalidation_and_a_reviewed_one_is_not() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    for binding in [
        MemoryAuthority::Invariant,
        MemoryAuthority::Constraint,
        MemoryAuthority::Decision,
    ] {
        let record = store
            .record(
                NewMemory::new(
                    MemoryKind::Finding,
                    format!("a {binding} needing revalidation"),
                )
                .with_authority(Some(binding)),
            )
            .unwrap();
        store
            .mark_for_review(&record.id, ReviewReason::ProjectState)
            .unwrap();

        let refusal = store
            .revalidate_reaffirmed(&record.id, ConflictResolver::Automatic)
            .expect_err(&format!("{binding} is high-impact and needs review"));
        assert!(
            matches!(refusal, MemoryStoreError::ReviewRequired { .. }),
            "unexpected error for {binding}: {refusal}"
        );
        assert_eq!(
            store.get(&record.id).unwrap().unwrap().status,
            MemoryStatus::NeedsReview,
            "a refused automatic revalidation must change nothing"
        );

        let settled = store
            .revalidate_reaffirmed(&record.id, ConflictResolver::Reviewed)
            .unwrap();
        assert_eq!(settled.status, MemoryStatus::Active);
    }

    // An unclassified authority is treated as high-impact too: `None` means
    // nobody judged how binding it is, and unknown must not mean safe.
    let unclassified = store
        .record(NewMemory::new(
            MemoryKind::Finding,
            "nobody has classified how binding this is",
        ))
        .unwrap();
    store
        .mark_for_review(&unclassified.id, ReviewReason::ProjectState)
        .unwrap();
    let refusal = store
        .revalidate_reaffirmed(&unclassified.id, ConflictResolver::Automatic)
        .expect_err("an unclassified authority must fail closed");
    assert!(matches!(refusal, MemoryStoreError::ReviewRequired { .. }));

    // A low-authority memory is ordinary work an automatic reviewer may
    // settle itself.
    for ordinary in [
        MemoryAuthority::Preference,
        MemoryAuthority::Hypothesis,
        MemoryAuthority::Idea,
        MemoryAuthority::Historical,
    ] {
        let record = store
            .record(
                NewMemory::new(MemoryKind::Finding, format!("an ordinary {ordinary}"))
                    .with_authority(Some(ordinary)),
            )
            .unwrap();
        store
            .mark_for_review(&record.id, ReviewReason::ProjectState)
            .unwrap();
        let settled = store
            .revalidate_reaffirmed(&record.id, ConflictResolver::Automatic)
            .unwrap_or_else(|error| panic!("{ordinary} is not high-impact: {error}"));
        assert_eq!(settled.status, MemoryStatus::Active);
    }
}

/// Acceptance test 4 (Phase 21E, box 924): an automatic actor is refused
/// *supersession* specifically — not only reaffirmation, which the previous
/// test already covers — of a binding authority and of an unclassified one;
/// a reviewed actor may supersede either. `MemoryStore::supersede`'s first
/// production caller is `revalidate_superseded` (the packet's feasibility
/// note), and until this test only `revalidate_reaffirmed`'s side of the
/// shared gate had a regression test entering through its own production
/// path — see `require_reviewed_for_high_impact`, called identically by all
/// four `revalidate_*` methods.
#[test]
fn an_automatic_reviewer_is_refused_a_high_impact_supersession_and_a_reviewed_one_is_not() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    for binding in [
        MemoryAuthority::Invariant,
        MemoryAuthority::Constraint,
        MemoryAuthority::Decision,
    ] {
        let old = store
            .record(
                NewMemory::new(MemoryKind::Finding, format!("a {binding} to be superseded"))
                    .with_authority(Some(binding)),
            )
            .unwrap();
        let successor = store
            .record(NewMemory::new(
                MemoryKind::Finding,
                format!("{binding}'s successor"),
            ))
            .unwrap();

        let refusal = store
            .revalidate_superseded(&old.id, &successor.id, None, ConflictResolver::Automatic)
            .expect_err(&format!(
                "{binding} is high-impact and needs review to be superseded"
            ));
        assert!(
            matches!(refusal, MemoryStoreError::ReviewRequired { .. }),
            "unexpected error for {binding}: {refusal}"
        );
        assert_eq!(
            store.get(&old.id).unwrap().unwrap().status,
            MemoryStatus::Active,
            "a refused automatic supersession must change nothing"
        );

        let settled = store
            .revalidate_superseded(&old.id, &successor.id, None, ConflictResolver::Reviewed)
            .unwrap();
        assert_eq!(settled.status, MemoryStatus::Superseded);
        assert_eq!(settled.superseded_by, Some(successor.id));
    }

    // Unclassified is high-impact too: nobody has judged how binding it is,
    // and treating "unknown" as safe would let an automatic caller supersede
    // any memory recorded before a classifier existed.
    let unclassified = store
        .record(NewMemory::new(
            MemoryKind::Finding,
            "nobody has classified how binding this superseded memory is",
        ))
        .unwrap();
    let unclassified_successor = store
        .record(NewMemory::new(
            MemoryKind::Finding,
            "the unclassified memory's successor",
        ))
        .unwrap();
    let refusal = store
        .revalidate_superseded(
            &unclassified.id,
            &unclassified_successor.id,
            None,
            ConflictResolver::Automatic,
        )
        .expect_err("an unclassified authority must fail closed against automatic supersession");
    assert!(matches!(refusal, MemoryStoreError::ReviewRequired { .. }));
    assert_eq!(
        store.get(&unclassified.id).unwrap().unwrap().status,
        MemoryStatus::Active,
        "a refused automatic supersession must change nothing"
    );
    let settled = store
        .revalidate_superseded(
            &unclassified.id,
            &unclassified_successor.id,
            None,
            ConflictResolver::Reviewed,
        )
        .unwrap();
    assert_eq!(settled.status, MemoryStatus::Superseded);

    // A low-authority memory is ordinary work an automatic reviewer may
    // supersede itself.
    let ordinary = store
        .record(
            NewMemory::new(MemoryKind::Finding, "an ordinary preference")
                .with_authority(Some(MemoryAuthority::Preference)),
        )
        .unwrap();
    let ordinary_successor = store
        .record(NewMemory::new(
            MemoryKind::Finding,
            "the preference's successor",
        ))
        .unwrap();
    let settled = store
        .revalidate_superseded(
            &ordinary.id,
            &ordinary_successor.id,
            None,
            ConflictResolver::Automatic,
        )
        .unwrap_or_else(|error| panic!("a preference is not high-impact: {error}"));
    assert_eq!(settled.status, MemoryStatus::Superseded);
}

// -------------------------------------------------------------------------
// Phases 23 and 26 — the project boundary.
// -------------------------------------------------------------------------

/// Two projects sharing one data root keep entirely separate memories, and
/// neither store can see the other's.
#[test]
fn two_projects_sharing_one_data_root_cannot_see_each_other_s_memories() {
    let tmp = tempdir();
    let alpha = Fixture::new(tmp.path(), "alpha");
    let beta = Fixture::new(tmp.path(), "beta");

    assert_ne!(
        alpha.runtime.database_path(),
        beta.runtime.database_path(),
        "each project owns its own database file"
    );

    let alpha_memory = alpha.memory();
    let alpha_store = alpha_memory.store();
    let secret = alpha_store
        .record(NewMemory::new(
            MemoryKind::Finding,
            "alpha learned that ConPTY reflows at the buffer width",
        ))
        .unwrap();

    let beta_memory = beta.memory();
    let beta_store = beta_memory.store();

    assert_ne!(alpha_store.project_id(), beta_store.project_id());

    // Beta holds alpha's identifier and still gets nothing: the identifier is
    // not a capability, because the store it is presented to only ever reads
    // the one database its runtime resolved.
    assert_eq!(
        beta_store.get(&secret.id).unwrap(),
        None,
        "one project's memory identifier must mean nothing in another project"
    );
    assert_eq!(beta_store.count(MemoryStatus::Active).unwrap(), 0);
    assert!(
        beta_store
            .with_status(MemoryStatus::Active, 50)
            .unwrap()
            .is_empty()
    );

    // And alpha still has it.
    assert_eq!(
        alpha_store.get(&secret.id).unwrap().unwrap().body,
        secret.body
    );
}

/// The database itself refuses a memory row belonging to another project, so
/// the isolation does not depend on any query remembering to filter.
#[test]
fn the_database_refuses_a_memory_row_bound_to_another_project() {
    let tmp = tempdir();
    let alpha = Fixture::new(tmp.path(), "alpha");
    let beta = Fixture::new(tmp.path(), "beta");

    let beta_id = beta.memory().store().project_id().to_owned();
    let alpha_memory = alpha.memory();
    let alpha_store = alpha_memory.store();
    assert_ne!(alpha_store.project_id(), beta_id);

    // Bypass the store entirely and write raw SQL, the way a future query
    // written by someone who never read the store module would.
    let conn = rusqlite::Connection::open(alpha.runtime.database_path()).unwrap();
    let insert = conn.execute(
        "INSERT INTO memories (id, project_id, kind, status, body, created_at, updated_at) \
         VALUES ('planted', ?1, 'finding', 'active', 'beta''s memory in alpha''s file', 1, 1)",
        [&beta_id],
    );
    let error = insert.expect_err("the trigger must abort a foreign-project row");
    assert!(
        error.to_string().contains("different project"),
        "unexpected error: {error}"
    );

    // An update cannot smuggle one in either.
    let mine = alpha_store
        .record(NewMemory::new(MemoryKind::Finding, "alpha's own memory"))
        .unwrap();
    let update = conn.execute(
        "UPDATE memories SET project_id = ?1 WHERE id = ?2",
        rusqlite::params![&beta_id, mine.id.as_str()],
    );
    assert!(
        update.is_err(),
        "a memory must not be re-bound to another project"
    );
    assert_eq!(
        alpha_store.get(&mine.id).unwrap().unwrap().project_id,
        alpha_store.project_id()
    );
}

/// A memory whose stored project identifier is not this project's is refused
/// at the read boundary rather than handed back.
///
/// Not redundant with the trigger above: the trigger governs what this database
/// will accept from now on, while this governs what Glasshouse will *act on* —
/// including a row that arrived through a restored backup or was written by a
/// build whose triggers differed.
#[test]
fn a_foreign_memory_row_that_is_already_present_is_refused_when_read() {
    let tmp = tempdir();
    let alpha = Fixture::new(tmp.path(), "alpha");
    let beta = Fixture::new(tmp.path(), "beta");
    let beta_id = beta.memory().store().project_id().to_owned();

    // Plant the row with the guard temporarily gone, which is exactly the
    // situation a restored backup or an older build produces.
    let conn = rusqlite::Connection::open(alpha.runtime.database_path()).unwrap();
    conn.execute_batch(
        "DROP TRIGGER memories_reject_foreign_project_insert;
         DROP TRIGGER memories_reject_foreign_project_update;",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO memories (id, project_id, kind, status, body, created_at, updated_at) \
         VALUES ('deadbeef', ?1, 'finding', 'active', 'from another project', 1, 1)",
        [&beta_id],
    )
    .unwrap();
    drop(conn);

    let alpha_memory = alpha.memory();
    let alpha_store = alpha_memory.store();
    let planted = MemoryId::new("deadbeef");
    let error = alpha_store
        .get(&planted)
        .expect_err("a foreign row must be refused, not returned");
    match error {
        MemoryStoreError::ForeignProject {
            expected, actual, ..
        } => {
            assert_eq!(expected, alpha_store.project_id());
            assert_eq!(actual, beta_id);
        }
        other => panic!("unexpected error: {other}"),
    }
}

/// Memory retrieval reaches exactly one file: the active project's database.
///
/// A structural claim, so it is proved structurally — the store's only door is
/// `ProjectMemory::open(&Runtime)`, and there is no method anywhere on it that
/// takes a path or a project identifier. This scans the module's own source for
/// one, which is what would have to appear for the guarantee to break.
#[test]
fn nothing_in_the_memory_module_accepts_a_database_path_or_a_project_id() {
    // `str::lines` rather than a multi-line literal search: on a checkout that
    // converts line endings, a literal containing `\n` silently finds nothing
    // (practice §14).
    const SOURCES: [(&str, &str); 4] = [
        ("store.rs", include_str!("../src/memory/store.rs")),
        ("policy.rs", include_str!("../src/memory/policy.rs")),
        ("search.rs", include_str!("../src/memory/search.rs")),
        ("snapshot.rs", include_str!("../src/memory/snapshot.rs")),
    ];

    for (name, source) in SOURCES {
        for (number, line) in source.lines().enumerate() {
            let line = line.trim();
            if line.starts_with("//") {
                continue;
            }
            assert!(
                !line.contains("Connection::open"),
                "{name}:{} opens a database connection of its own; every \
                 connection must come from `database::open`, which derives the \
                 path from the runtime: {line}",
                number + 1
            );
            assert!(
                !line.contains("db_path") && !line.contains("database_path"),
                "{name}:{} names a database path; the memory module must never \
                 be able to be pointed at a file: {line}",
                number + 1
            );
        }
    }
}

/// No vector database, and no dependency that would be one — Phase 23's
/// standing prohibition, checked against the manifest rather than remembered.
#[test]
fn no_vector_database_dependency_has_been_introduced() {
    const MANIFEST: &str = include_str!("../../../Cargo.toml");
    for forbidden in [
        "qdrant",
        "lancedb",
        "chromadb",
        "usearch",
        "hnsw",
        "faiss",
        "sqlite-vec",
        "sqlite_vss",
        "pgvector",
        "milvus",
    ] {
        assert!(
            !MANIFEST.to_lowercase().contains(forbidden),
            "`{forbidden}` appeared in the workspace manifest; Phase 23 forbids \
             a vector database until lexical retrieval is shown insufficient in \
             real usage"
        );
    }
}
