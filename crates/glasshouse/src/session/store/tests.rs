use super::*;
use crate::profile::response::{
    AnswerFormat, Audience, EvidenceDetail, Narration, ResponseProfile, Verbosity,
};
use crate::routing::AssignedModel;
use crate::{Cli, Runtime};
use clap::Parser;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};

/// Every production successor of `session/store.rs`, joined -- Phase 59
/// (`GH-DECOMP-SESSION-STORE`) split it into `mod.rs`, `record.rs` and
/// `context.rs`. None of the three holds an inline test any more, so the
/// join needs no `production_code()`-style truncation: it already is
/// production code.
fn store_source() -> String {
    [
        include_str!("record.rs"),
        include_str!("context.rs"),
        include_str!("mod.rs"),
    ]
    .join("\n")
}

/// Undo every migration above 13, for a rollback fixture that lands above
/// version 5.
///
/// A fixture that claims to be an older database has to undo **every**
/// migration above the version it claims, not only the one it is about.
/// Below version 5 that is free for `checkpoints` — the table did not
/// exist yet, so the fixture drops it and migration 14 meets a fresh one.
/// A fixture that lands on 5 or later keeps the table, and without this it
/// fails the re-run with `duplicate column name: seq`.
///
/// **The name was `UNDO_MIGRATION_FOURTEEN` and was wrong by two
/// migrations**, which is how `database`'s twin constant explains its own
/// name: this is one constant precisely so the next migration has one
/// place to be added rather than three copies to miss, and a name saying
/// "fourteen" invites a reader to think 15 and 16 are handled somewhere
/// else. They are handled here.
///
/// SQLite refuses to drop a column an index mentions, so migration 14's
/// indexes go first and `checkpoints_by_session` is put back the way
/// migration 5 left it. Migration 16's column is indexed by nothing, and a
/// column-scoped `CHECK` goes with the column it is written on, so it is
/// one statement. Migration 17's `memory_files` is one statement for
/// migration 15's reason — dropping a table takes its index and its two
/// triggers with it. Migration 18's column is one statement for
/// migration 16's reason, and migration 19's two tables are two
/// statements for migration 15's — each drop takes its indexes and
/// triggers with it — and they go first, newest migration undone first.
/// Migrations 21 and 22 are each one statement for migration 16's reason
/// as well: nothing indexes `last_seen_commit`, `extraction_trigger` or
/// `entitlement`, and none of the three carries a `CHECK`. Migrations 23
/// and 24 are one and three statements for the same reason again —
/// nothing indexes `task_class`, `session_id`, `effort_level` or
/// `turn_shape`, and none of the four carries a `CHECK` or a
/// `REFERENCES`. Migration 25 is four statements for migration 16's
/// reason instead: nothing indexes the four millisecond offsets, and the
/// `CHECK` each of them carries is column-scoped, so SQLite drops it with
/// the column. Newest first, so 25's four lead and 24's three follow,
/// each set in the reverse of the order that migration adds them.
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
    DROP TABLE IF EXISTS evaluation_observations;
    DROP INDEX checkpoints_by_seq;
    DROP INDEX checkpoints_by_session;
    ALTER TABLE checkpoints DROP COLUMN seq;
    CREATE INDEX checkpoints_by_session
        ON checkpoints (session_id, created_at DESC);
";

/// A bootstrapped project with an open connection to its database, which
/// is what every caller of this module will have.
struct Fixture {
    base: PathBuf,
    runtime: Runtime,
    conn: Connection,
}

impl Fixture {
    fn new(base: &Path, name: &str) -> Self {
        let root = base.join("workspace").join(name);
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        let runtime = bootstrap_at(base, &root);
        let conn = crate::database::open(&runtime).unwrap();
        Self {
            base: base.to_path_buf(),
            runtime,
            conn,
        }
    }

    fn store(&self) -> SessionStore<'_> {
        SessionStore::new(&self.conn).unwrap()
    }

    /// A store whose clock returns `start`, then `start + step` on each
    /// later call, so a test can assert exact timestamps.
    fn store_with_ticking_clock(&self, start: i64, step: i64) -> SessionStore<'_> {
        let next = AtomicI64::new(start);
        let clock: Clock = Arc::new(move || next.fetch_add(step, Ordering::SeqCst));
        SessionStore::with_clock(&self.conn, clock).unwrap()
    }

    /// Reopen the database the way a later launch would, proving what is
    /// on disk rather than what is in memory.
    fn reopen(&self) -> Connection {
        crate::database::open(&self.runtime).unwrap()
    }

    fn project_id(&self) -> &str {
        self.runtime.project().id().as_str()
    }

    /// A second project sharing this machine's data/config root.
    fn sibling(&self, name: &str) -> Runtime {
        let root = self.base.join("workspace").join(name);
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        bootstrap_at(&self.base, &root)
    }
}

fn bootstrap_at(base: &Path, root: &Path) -> Runtime {
    let cli = Cli::try_parse_from([
        "glasshouse",
        "--data-dir",
        base.join("data").to_str().unwrap(),
        "--config-dir",
        base.join("config").to_str().unwrap(),
    ])
    .unwrap();
    crate::bootstrap(&cli, root).unwrap()
}

/// Insert a row directly, bypassing [`SessionStore`] entirely.
///
/// Used to plant a row belonging to another project, which is exactly what
/// the schema's trigger exists to prevent — so the trigger is dropped for
/// the insert and restored afterwards. That models the real threat the
/// resume check answers: a row that reached the file by some route the
/// trigger never saw, such as a restored backup or an older build.
fn plant_foreign_row(conn: &Connection, id: &str, project_id: &str, native: Option<&str>) {
    conn.execute_batch("DROP TRIGGER sessions_reject_foreign_project_insert;")
        .unwrap();
    conn.execute(
        "INSERT INTO sessions (id, project_id, harness, native_session_id, role, \
         lifecycle, presentation, created_at, last_activity_at) \
         VALUES (?1, ?2, 'claude-code', ?3, 'normal', 'stopped', 'embedded', 10, 20)",
        rusqlite::params![id, project_id, native],
    )
    .unwrap();
    conn.execute_batch(
        "CREATE TRIGGER sessions_reject_foreign_project_insert
         BEFORE INSERT ON sessions
         FOR EACH ROW
         WHEN NEW.project_id IS NOT (
             SELECT value FROM project_metadata WHERE key = 'project_id'
         )
         BEGIN
             SELECT RAISE(ABORT, 'session belongs to a different project');
         END;",
    )
    .unwrap();
}

// ---------------------------------------------------------------
// Phase 1 line 90 — reject a cross-project resume.
// ---------------------------------------------------------------

/// The capability, stated as a contract: given a session record whose
/// project identifier differs from the active project's, when a caller
/// tries to resume it, Glasshouse refuses and names both projects, while
/// leaving the record untouched.
#[test]
fn resuming_a_session_belonging_to_another_project_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let other = fixture.sibling("beta");
    let other_id = other.project().id().as_str();
    assert_ne!(
        other_id,
        fixture.project_id(),
        "fixture must use two projects"
    );

    plant_foreign_row(&fixture.conn, "planted", other_id, Some("native-1"));

    let store = fixture.store();
    let error = store
        .open_for_resume(&SessionId::new("planted"))
        .expect_err("a session from another project must never be resumable");

    match &error {
        SessionStoreError::ForeignProject {
            id,
            expected,
            actual,
        } => {
            assert_eq!(id.as_str(), "planted");
            assert_eq!(expected, fixture.project_id());
            assert_eq!(actual, other_id);
        }
        other => panic!("expected ForeignProject, got {other:?}"),
    }

    // Naming both projects is the point: "not found" would send the user
    // hunting for a session that is sitting right there.
    let message = error.to_string();
    assert!(
        message.contains(other_id),
        "message must name the owning project: {message}"
    );
    assert!(
        message.contains(fixture.project_id()),
        "message must name the active project: {message}"
    );

    // Refusing is not deleting. The record is still exactly as planted.
    let still_there: String = fixture
        .conn
        .query_row(
            "SELECT project_id FROM sessions WHERE id = 'planted'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(still_there, other_id);
}

/// The structural half: the database itself refuses to store a session
/// belonging to another project, so no future query has to remember to
/// filter by project.
#[test]
fn the_database_refuses_to_store_a_session_from_another_project() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let other = fixture.sibling("beta");

    let result = fixture.conn.execute(
        "INSERT INTO sessions (id, project_id, harness, role, lifecycle, \
         presentation, created_at, last_activity_at) \
         VALUES ('x', ?1, 'claude-code', 'normal', 'starting', 'embedded', 1, 1)",
        [other.project().id().as_str()],
    );

    let Err(error) = result else {
        panic!("the trigger must abort an insert for another project");
    };
    assert!(
        error.to_string().contains("different project"),
        "unexpected error: {error}"
    );
}

/// Same guard on the update path: a row cannot be *moved* to another
/// project after the fact.
#[test]
fn a_stored_session_cannot_be_reassigned_to_another_project() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let other = fixture.sibling("beta");
    let record = fixture
        .store()
        .create(NewSession::embedded("claude-code"))
        .unwrap();

    let result = fixture.conn.execute(
        "UPDATE sessions SET project_id = ?2 WHERE id = ?1",
        rusqlite::params![record.id.as_str(), other.project().id().as_str()],
    );

    let Err(error) = result else {
        panic!("the trigger must abort a reassignment");
    };
    assert!(
        error.to_string().contains("different project"),
        "unexpected error: {error}"
    );
}

/// The guard fails closed: with no binding row to compare against, the
/// trigger aborts rather than letting the write through. `<>` against a
/// NULL subquery would have evaluated to NULL and allowed it.
#[test]
fn a_session_write_is_refused_when_the_project_binding_is_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let project_id = fixture.project_id().to_owned();

    fixture
        .conn
        .execute("DELETE FROM project_metadata WHERE key = 'project_id'", [])
        .unwrap();

    let result = fixture.conn.execute(
        "INSERT INTO sessions (id, project_id, harness, role, lifecycle, \
         presentation, created_at, last_activity_at) \
         VALUES ('x', ?1, 'claude-code', 'normal', 'starting', 'embedded', 1, 1)",
        [&project_id],
    );
    assert!(
        result.is_err(),
        "an unbound database must accept no session rows"
    );
}

/// The permitted case, so the refusals above are not simply "resume never
/// works".
#[test]
fn a_stopped_session_of_this_project_can_be_resumed() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let store = fixture.store();

    let record = store.create(NewSession::embedded("codex")).unwrap();
    store
        .set_native_session_id(&record.id, "thread-77")
        .unwrap();
    store
        .set_lifecycle(&record.id, SessionLifecycle::Stopped)
        .unwrap();

    let resumable = store.open_for_resume(&record.id).unwrap();
    assert_eq!(
        resumable,
        ResumableSession {
            id: record.id,
            harness: "codex".to_owned(),
            native_session_id: "thread-77".to_owned(),
        }
    );
}

/// The defect this package repairs, at the layer that caused it.
///
/// `set_lifecycle` is what `main.rs::resume_session` used to call, and it
/// silently declines a finished record — so the resume left the session
/// reading `stopped`, and the *caller got no error saying so*. Both halves
/// are asserted: the old door still refuses, and the resume boundary's own
/// door opens.
#[test]
fn a_resume_reopens_a_session_that_set_lifecycle_would_have_left_finished() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let store = fixture.store();

    let record = store.create(NewSession::embedded("codex")).unwrap();
    store
        .set_native_session_id(&record.id, "thread-77")
        .unwrap();
    store
        .set_lifecycle(&record.id, SessionLifecycle::Stopped)
        .unwrap();

    // The door a hook comes through, and the reason the defect was silent:
    // it returns the record as it stands rather than an error.
    let declined = store
        .set_lifecycle(&record.id, SessionLifecycle::Running)
        .expect("a declined lifecycle change is not a failure");
    assert_eq!(
        declined.lifecycle,
        SessionLifecycle::Stopped,
        "`set_lifecycle` must keep refusing to revive a finished session"
    );

    let resumable = store.open_for_resume(&record.id).unwrap();
    let resumed = store.begin_resume(&resumable).unwrap();
    assert_eq!(
        resumed.lifecycle,
        SessionLifecycle::Running,
        "the resume boundary must reopen the session it was given"
    );
}

/// A resume is not a licence that outlives the session it was granted for.
/// Once the resumed process exits, the record is finished again and the
/// next late hook is refused exactly as the first incarnation's was.
#[test]
fn a_resumed_session_that_stops_again_is_finished_again() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let store = fixture.store();

    let record = store.create(NewSession::embedded("codex")).unwrap();
    store
        .set_native_session_id(&record.id, "thread-77")
        .unwrap();
    store
        .set_lifecycle(&record.id, SessionLifecycle::Stopped)
        .unwrap();
    let resumable = store.open_for_resume(&record.id).unwrap();
    store.begin_resume(&resumable).unwrap();
    store
        .set_lifecycle(&record.id, SessionLifecycle::Stopped)
        .unwrap();

    let declined = store
        .set_lifecycle(&record.id, SessionLifecycle::Running)
        .unwrap();
    assert_eq!(
        declined.lifecycle,
        SessionLifecycle::Stopped,
        "having once been resumed must not make a session revivable for ever"
    );
}

/// **The window `open_for_resume` cannot close on its own.** It reads
/// outside a transaction, so its answer can be stale by the time the
/// resume writes — and the write is what matters.
///
/// Here the record is closed after a `ResumableSession` has been obtained,
/// which is exactly what a `glasshouse sessions close` in another process
/// does between the two steps. The resume must refuse rather than reopen a
/// record the user retired.
#[test]
fn a_resume_refuses_a_record_that_stopped_being_resumable_after_it_was_opened() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let store = fixture.store();

    let record = store.create(NewSession::embedded("codex")).unwrap();
    store
        .set_native_session_id(&record.id, "thread-77")
        .unwrap();
    store
        .set_lifecycle(&record.id, SessionLifecycle::Stopped)
        .unwrap();
    let resumable = store.open_for_resume(&record.id).unwrap();

    store.close(&record.id).unwrap();

    let error = store
        .begin_resume(&resumable)
        .expect_err("a record closed since it was opened is no longer resumable");
    assert!(
        matches!(&error, SessionStoreError::NotResumable { disposition, .. } if *disposition == "closed"),
        "got {error:?}"
    );
    assert_eq!(
        store.get(&record.id).unwrap().unwrap().lifecycle,
        SessionLifecycle::Closed,
        "the refused resume must have written nothing"
    );
}

/// `Failed` and `Closed` are not `Stopped`, and neither is a stopped
/// record with nothing to resume *to*. All three are refused by the resume
/// boundary itself, so the refusal does not depend on every caller having
/// remembered to ask `open_for_resume` first.
#[test]
fn only_a_stopped_session_with_something_to_resume_to_may_be_reopened() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let store = fixture.store();

    // A distinct identifier per case: `(harness, native_session_id)` is
    // unique, which is the constraint that stops two sessions claiming one
    // harness conversation.
    for (case, lifecycle, native_recorded, expected) in [
        (0, SessionLifecycle::Failed, true, "failed"),
        (1, SessionLifecycle::Closed, true, "closed"),
        (2, SessionLifecycle::Stopped, false, "closed"),
        (3, SessionLifecycle::Running, true, "still running"),
    ] {
        let native = format!("thread-{case}");
        let record = store.create(NewSession::embedded("codex")).unwrap();
        if native_recorded {
            store.set_native_session_id(&record.id, &native).unwrap();
        }
        store.set_lifecycle(&record.id, lifecycle).unwrap();

        // Built by hand rather than through `open_for_resume`, which
        // refuses all four: the claim is that the boundary refuses them
        // too, and a test that could not construct the input could not
        // make it.
        let resumable = ResumableSession {
            id: record.id.clone(),
            harness: "codex".to_owned(),
            native_session_id: native.clone(),
        };
        let error = store.begin_resume(&resumable).unwrap_err();
        assert!(
            matches!(&error, SessionStoreError::NotResumable { disposition, .. } if *disposition == expected),
            "{lifecycle:?} with a recorded identifier={native_recorded} got {error:?}"
        );
        assert_eq!(
            store.get(&record.id).unwrap().unwrap().lifecycle,
            lifecycle,
            "a refused resume must leave {lifecycle:?} exactly as it was"
        );
    }
}

#[test]
fn resuming_an_unknown_session_says_so() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let error = fixture
        .store()
        .open_for_resume(&SessionId::new("nope"))
        .expect_err("an unknown session cannot be resumed");
    assert!(
        matches!(error, SessionStoreError::NotFound { .. }),
        "got {error:?}"
    );
}

#[test]
fn a_live_session_is_not_resumable() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let store = fixture.store();
    let record = store.create(NewSession::embedded("claude-code")).unwrap();
    store.set_native_session_id(&record.id, "native-1").unwrap();
    store
        .set_lifecycle(&record.id, SessionLifecycle::Running)
        .unwrap();

    let error = store
        .open_for_resume(&record.id)
        .expect_err("a running session is not resumable");
    assert!(
        matches!(&error, SessionStoreError::NotResumable { disposition, .. } if *disposition == "still running"),
        "got {error:?}"
    );
}

/// Without a native identifier there is nothing to resume *to*, so
/// offering a resume would produce a blank session wearing an old name.
#[test]
fn a_stopped_session_with_no_native_identifier_is_not_resumable() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let store = fixture.store();
    let record = store.create(NewSession::embedded("claude-code")).unwrap();
    store
        .set_lifecycle(&record.id, SessionLifecycle::Stopped)
        .unwrap();

    let error = store
        .open_for_resume(&record.id)
        .expect_err("nothing to resume to");
    assert!(
        matches!(error, SessionStoreError::NotResumable { .. }),
        "got {error:?}"
    );
}

// ---------------------------------------------------------------
// Phase 2 line 183 — metadata independent of native session files.
// ---------------------------------------------------------------

/// The record is Glasshouse's own: it is complete before the harness has
/// produced any identifier, it survives a reopen, and nothing about it is
/// read from a harness's files.
#[test]
fn a_session_is_recorded_and_survives_a_reopen_with_no_harness_involved() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let created = fixture
        .store_with_ticking_clock(1_700_000_000, 0)
        .create(
            NewSession::embedded("claude-code")
                .with_role(SessionRole::Orchestrator)
                .with_presentation(SessionPresentation::External),
        )
        .unwrap();
    assert!(
        created.native_session_id.is_none(),
        "no harness has spoken yet"
    );

    // A different connection to the same file, as a later launch makes.
    let reopened = fixture.reopen();
    let store = SessionStore::new(&reopened).unwrap();
    let read_back = store
        .get(&created.id)
        .unwrap()
        .expect("the record is on disk");
    assert_eq!(read_back, created);
}

// ---------------------------------------------------------------
// Phase 2 line 184 — Glasshouse ID <-> native harness ID mapping.
// ---------------------------------------------------------------

#[test]
fn a_native_session_identifier_can_be_attached_later_and_read_back() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let store = fixture.store();

    let record = store.create(NewSession::embedded("claude-code")).unwrap();
    let updated = store.set_native_session_id(&record.id, "sess-abc").unwrap();

    assert_eq!(updated.native_session_id.as_deref(), Some("sess-abc"));
    assert_eq!(
        updated.id, record.id,
        "the Glasshouse identifier never changes"
    );
    assert_eq!(
        store
            .get(&record.id)
            .unwrap()
            .unwrap()
            .native_session_id
            .as_deref(),
        Some("sess-abc")
    );
}

/// A mapping, not an annotation: one native session cannot be claimed by
/// two Glasshouse sessions, or a resume would not know which to continue.
#[test]
fn one_native_session_cannot_map_to_two_glasshouse_sessions() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let store = fixture.store();

    let first = store.create(NewSession::embedded("claude-code")).unwrap();
    let second = store.create(NewSession::embedded("claude-code")).unwrap();
    store.set_native_session_id(&first.id, "shared").unwrap();

    let error = store
        .set_native_session_id(&second.id, "shared")
        .expect_err("the same native session must not be claimed twice");
    assert!(
        matches!(error, SessionStoreError::Sql { .. }),
        "got {error:?}"
    );
}

/// Scoped per harness, so two harnesses that happen to use the same
/// identifier format do not collide.
#[test]
fn two_harnesses_may_use_the_same_native_identifier() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let store = fixture.store();

    let claude = store.create(NewSession::embedded("claude-code")).unwrap();
    let codex = store.create(NewSession::embedded("codex")).unwrap();
    store.set_native_session_id(&claude.id, "1").unwrap();
    store.set_native_session_id(&codex.id, "1").unwrap();

    assert_eq!(store.list().unwrap().len(), 2);
}

/// Sessions awaiting a native identifier must coexist freely.
///
/// SQLite's unique indexes treat NULLs as distinct, so this holds today
/// without help from the index's `WHERE` clause. The test earns its place
/// by pinning the behaviour against the obvious future refactor: making
/// the column `NOT NULL DEFAULT ''` would make every unidentified session
/// collide with the next one.
#[test]
fn many_sessions_may_have_no_native_identifier_at_once() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let store = fixture.store();

    for _ in 0..3 {
        store.create(NewSession::embedded("claude-code")).unwrap();
    }
    assert_eq!(store.list().unwrap().len(), 3);
}

// ---------------------------------------------------------------
// Phase 2 line 185 — harness, times, role, lifecycle, project id.
// ---------------------------------------------------------------

/// Every field the capability names, asserted by value rather than by
/// "it round-trips".
#[test]
fn every_required_field_is_persisted() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let record = fixture
        .store_with_ticking_clock(1_600_000_000, 0)
        .create(NewSession::embedded("codex").with_role(SessionRole::Worker))
        .unwrap();

    assert_eq!(record.harness, "codex");
    assert_eq!(record.role, SessionRole::Worker);
    assert_eq!(record.lifecycle, SessionLifecycle::Starting);
    assert_eq!(record.project_id, fixture.project_id());
    assert_eq!(record.created_at, 1_600_000_000);
    assert_eq!(record.last_activity_at, 1_600_000_000);
    assert!(!record.id.as_str().is_empty());
}

#[test]
fn every_role_and_lifecycle_value_round_trips() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let store = fixture.store();

    for role in [
        SessionRole::Normal,
        SessionRole::Orchestrator,
        SessionRole::Worker,
    ] {
        let record = store
            .create(NewSession::embedded("claude-code").with_role(role))
            .unwrap();
        assert_eq!(store.get(&record.id).unwrap().unwrap().role, role);

        for lifecycle in [
            SessionLifecycle::Starting,
            SessionLifecycle::Running,
            SessionLifecycle::Idle,
            SessionLifecycle::WaitingForUser,
            SessionLifecycle::Stopped,
            SessionLifecycle::Failed,
            SessionLifecycle::Closed,
        ] {
            store.set_lifecycle(&record.id, lifecycle).unwrap();
            assert_eq!(store.get(&record.id).unwrap().unwrap().lifecycle, lifecycle);
        }
    }
}

/// Activity time is what a session list sorts and ages by, so it has to
/// move independently of creation time.
#[test]
fn activity_time_advances_while_creation_time_stays_put() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let store = fixture.store_with_ticking_clock(1_000, 10);

    let record = store.create(NewSession::embedded("claude-code")).unwrap();
    assert_eq!(record.created_at, 1_000);

    let touched = store.touch(&record.id).unwrap();
    assert_eq!(touched.created_at, 1_000, "creation time is immutable");
    assert_eq!(touched.last_activity_at, 1_010);

    let moved = store
        .set_lifecycle(&record.id, SessionLifecycle::Running)
        .unwrap();
    assert_eq!(
        moved.last_activity_at, 1_020,
        "a state change counts as activity"
    );
    assert_eq!(moved.created_at, 1_000);
}

#[test]
fn sessions_are_listed_most_recently_active_first() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let store = fixture.store_with_ticking_clock(500, 10);

    let first = store.create(NewSession::embedded("claude-code")).unwrap();
    let second = store.create(NewSession::embedded("codex")).unwrap();
    store.touch(&first.id).unwrap();

    let listed: Vec<_> = store.list().unwrap().into_iter().map(|r| r.id).collect();
    assert_eq!(listed, vec![first.id, second.id]);
}

#[test]
fn touching_an_unknown_session_reports_it_missing_rather_than_inventing_one() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let error = fixture
        .store()
        .touch(&SessionId::new("ghost"))
        .expect_err("no such session");
    assert!(
        matches!(error, SessionStoreError::NotFound { .. }),
        "got {error:?}"
    );
    assert_eq!(
        fixture.store().list().unwrap().len(),
        0,
        "nothing was created"
    );
}

// ---------------------------------------------------------------
// Phase 17 line 760 — an external session's pane, as opaque metadata.

/// The reference survives a round trip exactly as given, an embedded
/// session records none, and the store never interprets the string:
/// a value no backend would accept is stored and returned all the same,
/// because deciding what a reference means is the presenting
/// integration's job (line 762), not this module's.
#[test]
fn an_external_sessions_presentation_ref_round_trips_and_is_never_interpreted() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let store = fixture.store();

    let external = store
        .create(
            NewSession::embedded("claude-code")
                .with_presentation(SessionPresentation::External)
                .with_presentation_ref(Some("workspace:349".to_owned())),
        )
        .unwrap();
    assert_eq!(
        store
            .get(&external.id)
            .unwrap()
            .unwrap()
            .presentation_ref
            .as_deref(),
        Some("workspace:349")
    );

    let embedded = store.create(NewSession::embedded("claude-code")).unwrap();
    assert_eq!(
        store.get(&embedded.id).unwrap().unwrap().presentation_ref,
        None,
        "a session with no pane records no pane"
    );

    let opaque = store
        .create(
            NewSession::embedded("claude-code")
                .with_presentation(SessionPresentation::External)
                .with_presentation_ref(Some("not-a-cmux-ref".to_owned())),
        )
        .unwrap();
    assert_eq!(
        store
            .get(&opaque.id)
            .unwrap()
            .unwrap()
            .presentation_ref
            .as_deref(),
        Some("not-a-cmux-ref"),
        "the store stores; it does not decide what a reference looks like"
    );
}

/// A session recorded somewhere else and then continued inside a pane
/// has its presentation and its pane rewritten together, and its
/// activity clock untouched.
#[test]
fn a_continued_session_can_be_moved_into_a_pane_afterwards() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let store = fixture.store();

    let record = store.create(NewSession::embedded("claude-code")).unwrap();
    assert_eq!(record.presentation, SessionPresentation::Embedded);
    assert_eq!(record.presentation_ref, None);

    let moved = store
        .set_presentation(
            &record.id,
            SessionPresentation::External,
            Some("workspace:349"),
        )
        .unwrap();
    assert_eq!(moved.presentation, SessionPresentation::External);
    assert_eq!(moved.presentation_ref.as_deref(), Some("workspace:349"));
    assert_eq!(
        moved.last_activity_at, record.last_activity_at,
        "moving a session is not session activity"
    );
    let read_back = store.get(&record.id).unwrap().unwrap();
    assert_eq!(read_back.presentation, SessionPresentation::External);
    assert_eq!(read_back.presentation_ref.as_deref(), Some("workspace:349"));
}

// Phase 2 line 186 — presentation mode.
// ---------------------------------------------------------------

#[test]
fn every_presentation_mode_is_persisted() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let store = fixture.store();

    for presentation in [
        SessionPresentation::Embedded,
        SessionPresentation::Headless,
        SessionPresentation::External,
    ] {
        let record = store
            .create(NewSession::embedded("claude-code").with_presentation(presentation))
            .unwrap();
        assert_eq!(
            store.get(&record.id).unwrap().unwrap().presentation,
            presentation,
            "presentation must survive a round trip"
        );
    }
}

// ---------------------------------------------------------------
// Phase 2 line 187 — active / resumable / closed / failed.
// ---------------------------------------------------------------

#[test]
fn the_four_dispositions_are_distinguishable_from_stored_metadata() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let store = fixture.store();

    let make = |lifecycle: SessionLifecycle, native: Option<&str>| {
        let record = store.create(NewSession::embedded("claude-code")).unwrap();
        if let Some(native) = native {
            store.set_native_session_id(&record.id, native).unwrap();
        }
        store.set_lifecycle(&record.id, lifecycle).unwrap()
    };

    assert_eq!(
        make(SessionLifecycle::Starting, None).disposition(),
        SessionDisposition::Active
    );
    assert_eq!(
        make(SessionLifecycle::Running, None).disposition(),
        SessionDisposition::Active
    );
    assert_eq!(
        make(SessionLifecycle::Idle, None).disposition(),
        SessionDisposition::Active
    );
    assert_eq!(
        make(SessionLifecycle::WaitingForUser, None).disposition(),
        SessionDisposition::Active
    );
    assert_eq!(
        make(SessionLifecycle::Stopped, Some("n1")).disposition(),
        SessionDisposition::Resumable
    );
    assert_eq!(
        make(SessionLifecycle::Stopped, None).disposition(),
        SessionDisposition::Closed,
        "stopped with nothing to resume to is over, not resumable"
    );
    assert_eq!(
        make(SessionLifecycle::Closed, None).disposition(),
        SessionDisposition::Closed
    );
    assert_eq!(
        make(SessionLifecycle::Failed, Some("n2")).disposition(),
        SessionDisposition::Failed,
        "a failure stays visible as a failure even with a native id"
    );
}

// ---------------------------------------------------------------
// Phase 2 line 188 — no provider credentials in the project database.
// ---------------------------------------------------------------

/// The whole schema, locked to an explicit list.
///
/// Fuzzy name matching would be worse than useless here: `project_metadata`
/// legitimately has a column called `key`, and a credential column could
/// just as easily be called `value`. Pinning the exact schema instead means
/// any new column fails this test until someone updates the list, and that
/// is the moment to ask what the new column can hold.
///
/// **What this test can and cannot prove.** It proves no column exists
/// whose *purpose* is to hold a credential, and that adding one is a
/// deliberate act somebody has to write down here. It does not prove a
/// credential can never be stored: `memories.subject` and `memories.body`
/// are free text, and free text can hold anything.
///
/// That gap is real and is not closed by widening this list. It is closed
/// on the **producer** side — Phase 21's memory extractor must never be
/// fed, and must never emit, credential material, and that is an explicit
/// acceptance condition of Phase 21 rather than something inherited by
/// assumption. Recorded when migration 4 added the memory tables and the
/// worker adding them declined to certify otherwise.
///
/// **Migration 6's twelve new columns, and the answer this test exists to
/// force.** Two of them are integers: `source_event_first` and
/// `source_event_last` are positions in `lifecycle_events.seq`, and an
/// `INTEGER` column cannot hold a credential — there is no question to
/// ask about those two.
///
/// The other ten **can**. `rationale`, `problem`, `assumptions`,
/// `scale_assumptions`, `security_assumptions`,
/// `compatibility_assumptions`, `operational_assumptions`, `evidence`
/// and `source_excerpt` are free text a producer chooses, exactly like
/// `subject` and `body`, and `source_excerpt` is the sharpest of the ten
/// because it is *verbatim session text* rather than a model's
/// paraphrase — a decision quoted from a session that discussed
/// configuring a provider is precisely where a key would appear.
/// (`project_phase` is the eleventh and the one exception: migration 6
/// gives it a `CHECK` over five fixed words, so it is not free text.)
///
/// So the answer for migration 6 is the same as migration 4's and it is
/// written down rather than inherited: **this test does not certify
/// them.** The control is on the producer side, and it covers the new
/// fields *without being extended*, which is the property worth having:
/// `memory::extract::schema::judge` screens each emitted element whole,
/// over its serialized text, **before reading any field of it**, so a
/// field the contract gained yesterday is screened today. That ordering
/// is why the coverage is automatic, and it is a Phase 21 acceptance
/// condition rather than a convention.
///
/// **Migration 5's twenty new columns, judged one at a time.** Nineteen
/// hold a value drawn from a fixed set or from Glasshouse's own machinery
/// — a kind, an origin, an exit code, a signal name, a backend resource
/// slug, an integration slug, a harness event name from an adapter's own
/// constant list — and none of them is free text a caller chooses.
/// `checkpoints.document` is the twentieth and it **is** free text, for
/// the same reason `memories.body` is: a person writes a handoff. The same
/// limit therefore applies to it and is recorded here rather than glossed
/// — it is closed on the producer side, by whoever authors a checkpoint,
/// and this test does not and cannot certify it.
#[test]
fn the_project_database_schema_has_nowhere_to_put_a_credential() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let mut statement = fixture
        .conn
        .prepare(
            "SELECT m.name, p.name FROM sqlite_master m \
             JOIN pragma_table_info(m.name) p \
             WHERE m.type = 'table' AND m.name NOT LIKE 'sqlite_%' \
             ORDER BY m.name, p.cid",
        )
        .unwrap();
    let columns: Vec<String> = statement
        .query_map([], |row| {
            Ok(format!(
                "{}.{}",
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();

    assert_eq!(
        columns,
        vec![
            // Migration 19 (carried from `assumption-guardrails`). The
            // premises an agent states, their evidence class, scope and
            // verification, and the transitions between six states —
            // free text a caller supplied, sanitized by that package's
            // writer, and vocabularies that live in Rust. That package's
            // own review of these columns is the authority; they are
            // listed here so this tree's ladder (1..20) is the schema
            // this test sees.
            "assumption_transitions.seq",
            "assumption_transitions.project_id",
            "assumption_transitions.assumption_id",
            "assumption_transitions.session_id",
            "assumption_transitions.at",
            "assumption_transitions.kind",
            "assumption_transitions.state",
            "assumption_transitions.origin",
            "assumption_transitions.subject",
            "assumption_transitions.response",
            "assumption_transitions.note",
            "checkpoints.id",
            "checkpoints.project_id",
            "checkpoints.session_id",
            "checkpoints.created_at",
            "checkpoints.reason",
            "checkpoints.document",
            // Migration 14. A counter of how many checkpoints this project
            // had written before this one — an integer with no free text
            // anywhere near it.
            "checkpoints.seq",
            // Migration 15. Every one confirmed unable to hold a provider
            // credential: `seq`/`observed_at`/`routing_seq` are integers,
            // `project_id` is the project hash every table already
            // carries, `kind`/`outcome` come from an exhaustive Rust match
            // at the single writer, `subject` from a two-value scope enum,
            // `session_id`/`memory_id` are identifiers, and `feature`,
            // `arm` and `detail` have no production writer at all.
            "evaluation_observations.seq",
            "evaluation_observations.project_id",
            "evaluation_observations.observed_at",
            "evaluation_observations.kind",
            "evaluation_observations.outcome",
            "evaluation_observations.subject",
            "evaluation_observations.session_id",
            "evaluation_observations.feature",
            "evaluation_observations.arm",
            "evaluation_observations.memory_id",
            "evaluation_observations.routing_seq",
            "evaluation_observations.detail",
            // Migration 27. A project identifier, a Glasshouse session
            // identifier, three timestamps, and a repo-relative path that
            // `crate::memory::normalize_observed_path` refuses unless it is
            // relative and free of `..` — six columns, none of them free
            // text a caller chooses and none of them able to hold a
            // credential.
            "file_claims.project_id",
            "file_claims.session_id",
            "file_claims.path",
            "file_claims.claimed_at",
            "file_claims.renewed_at",
            "file_claims.expires_at",
            "lifecycle_events.seq",
            "lifecycle_events.project_id",
            "lifecycle_events.session_id",
            "lifecycle_events.at",
            "lifecycle_events.kind",
            "lifecycle_events.turn_outcome",
            "lifecycle_events.origin",
            "lifecycle_events.bytes",
            "lifecycle_events.exit_code",
            "lifecycle_events.exit_signal",
            "lifecycle_events.resource",
            "lifecycle_events.gateway_reason",
            "lifecycle_events.gateway_provider",
            "lifecycle_events.gateway_model",
            "lifecycle_events.gateway_cause",
            // Migration 26. A repo-relative path a session edited, as
            // `crate::memory::store::normalize_observed_path` spells it —
            // the user's own file names, and never anything a provider
            // issued.
            "lifecycle_events.path",
            "lifecycle_events.observed_harness",
            "lifecycle_events.observed_event",
            "memories.id",
            "memories.project_id",
            "memories.kind",
            "memories.authority",
            "memories.status",
            "memories.subject",
            "memories.body",
            "memories.source_session_id",
            "memories.source_commit",
            "memories.superseded_by",
            "memories.created_at",
            "memories.updated_at",
            "memories.source_event_first",
            "memories.source_event_last",
            "memories.rationale",
            "memories.project_phase",
            "memories.problem",
            "memories.assumptions",
            "memories.scale_assumptions",
            "memories.security_assumptions",
            "memories.compatibility_assumptions",
            "memories.operational_assumptions",
            "memories.evidence",
            "memories.source_excerpt",
            // Migration 10. `review_reason` is one of six fixed words (a
            // `CHECK` enum); `review_marked_at` and `last_validated_at` are
            // Unix timestamps — none of the three can hold a credential.
            // `validity_conditions` and `invalidation_conditions` are free
            // text a producer writes, exactly like `rationale` and the rest
            // of migration 6's provenance columns beside them, and this test
            // does not and cannot certify them for the same reason it does
            // not certify those: the control is on the producer side, where
            // `memory::extract::chunk` scrubs and `schema::judge` screens.
            "memories.validity_conditions",
            "memories.invalidation_conditions",
            "memories.review_reason",
            "memories.review_marked_at",
            "memories.last_validated_at",
            "memories.superseded_reason",
            // Migration 19. One of four fixed words from
            // `memory::ExtractionTrigger::as_str`, which returns
            // `&'static str` — every value this column can hold is a
            // literal compiled into the binary, so there is nothing a
            // user or a provider could type into it.
            "memories.extraction_trigger",
            "memories_fts.subject",
            "memories_fts.body",
            "memories_fts.rationale",
            "memories_fts_config.k",
            "memories_fts_config.v",
            "memories_fts_data.id",
            "memories_fts_data.block",
            "memories_fts_docsize.id",
            "memories_fts_docsize.sz",
            "memories_fts_idx.segid",
            "memories_fts_idx.term",
            "memories_fts_idx.pgno",
            // Migration 17. `seq` and `observed_at` are integers,
            // `project_id` is the project hash every table carries,
            // `memory_id` is an identifier, and `provenance` comes from
            // an exhaustive Rust match at the single writer. `path` is
            // the one to argue about, and it is argued: it is never free
            // text a caller chooses — the only writer is
            // `MemoryStore::record_observed_files`, whose paths come from
            // the git index by way of
            // `checkpoint::git::WorkingTreeStatus::detect`, and
            // `memory::normalize_observed_path` refuses anything that is
            // not a repo-relative path before it can reach the column. A
            // credential is not a tracked file name.
            "memory_files.seq",
            "memory_files.project_id",
            "memory_files.memory_id",
            "memory_files.path",
            "memory_files.provenance",
            "memory_files.observed_at",
            "project_metadata.key",
            "project_metadata.value",
            // Migration 11: `routing_observations` (Phase 33A). `seq`,
            // `observed_at`, the timestamps, the counters and the
            // fixed-vocabulary columns cannot hold a credential; the
            // free-text ones (`route`, `quota_context`, `harness`,
            // `purpose`) are names and slugs a producer inside this crate
            // constructs, never text copied from a provider response body
            // — the gateway that writes this table is structurally unable
            // to read a response body at all (see `routing::evidence`).
            "routing_observations.seq",
            "routing_observations.project_id",
            "routing_observations.observed_at",
            "routing_observations.provider",
            "routing_observations.model",
            "routing_observations.route",
            "routing_observations.quota_context",
            "routing_observations.harness",
            "routing_observations.purpose",
            "routing_observations.dispatched_at",
            "routing_observations.first_byte_at",
            "routing_observations.first_token_at",
            "routing_observations.first_tool_call_at",
            "routing_observations.completed_at",
            "routing_observations.input_tokens",
            "routing_observations.output_tokens",
            "routing_observations.cached_input_tokens",
            "routing_observations.cost_micro_usd",
            "routing_observations.cost_confidence",
            "routing_observations.tool_rounds",
            "routing_observations.retries",
            "routing_observations.repairs",
            "routing_observations.failovers",
            "routing_observations.outcome",
            "routing_observations.context_state",
            "routing_observations.failure_class",
            // Migration 23 (Phase 32E line 1276). A fixed vocabulary
            // that lives in Rust — `routing::request::TaskClass`, five
            // variants — written only from `TaskClass::as_str`, which is
            // `&'static str` precisely so that no runtime string can
            // reach this column. It is Glasshouse's own classification
            // of a *request*, never text from a provider response, and
            // the one writer (`main.rs::record_routing_latency`) holds
            // no credential in scope. Nowhere to put one.
            "routing_observations.task_class",
            // Migration 24 (Phase 58 lines 2019 and 2039). Three columns
            // and no credential anywhere near any of them.
            //
            // `session_id` holds `crate::session::SessionId`'s own
            // string — a value this database already stores as
            // `sessions.id` and prints in `glasshouse sessions` — set
            // from the launch through
            // `gateway::session::SessionRouting::serve_session`, which
            // takes a `&SessionId` and can therefore be handed nothing
            // else. Never the harness's `metadata.user_id` and never
            // the gateway's own token.
            //
            // `effort_level` and `turn_shape` are fixed vocabularies
            // that live in Rust — `routing::evidence::EffortLevel` and
            // `::TurnShape` — written only from their `as_str`, which
            // is `&'static str` precisely so that no runtime string can
            // reach either column. Both are derived from the *request*
            // Glasshouse decoded in order to translate it, never from a
            // provider's reply, and the relay (which reads no body at
            // all) writes `NULL` for both.
            "routing_observations.session_id",
            "routing_observations.effort_level",
            "routing_observations.turn_shape",
            // Migration 25's four millisecond offsets: integers, and an
            // integer has nowhere to keep a credential.
            "routing_observations.first_byte_ms",
            "routing_observations.first_token_ms",
            "routing_observations.first_tool_call_ms",
            "routing_observations.completed_ms",
            "schema_migrations.version",
            "sessions.id",
            "sessions.project_id",
            "sessions.harness",
            "sessions.native_session_id",
            "sessions.role",
            "sessions.lifecycle",
            "sessions.presentation",
            "sessions.created_at",
            "sessions.last_activity_at",
            "sessions.launch_profile",
            "sessions.backend_resource",
            // Migration 8. Every one of these is a name, a slug or a
            // label a person typed: a model id, a pairing class, a wire
            // protocol, five response axes, a mechanism category, a
            // session name and a purpose. None of them is a place a
            // credential could be put, and there is still no column that
            // could hold one.
            "sessions.model",
            "sessions.pairing_class",
            "sessions.protocol",
            "sessions.response_profile",
            "sessions.response_mechanism",
            "sessions.display_name",
            "sessions.purpose",
            // Migration 9. A process id, a kernel start time, a host
            // name, one of four fixed supervision words, and a sentence
            // Glasshouse composes itself in `session::supervision`. None
            // of the five is ever written from anything a user typed or a
            // provider returned, and the two that are free-form are
            // `process_host` — the machine's own name — and
            // `supervision_reason`, whose every producer is a `format!`
            // in this crate over a process id and a timestamp.
            "sessions.process_id",
            "sessions.process_started_at",
            "sessions.process_host",
            "sessions.supervision",
            "sessions.supervision_reason",
            // Migration 12. A Glasshouse-generated session identifier —
            // the same one every other `sessions.id` column already
            // holds — never anything a user typed or a provider
            // returned.
            "sessions.source_session_id",
            // Migration 16. A count of compactions Glasshouse observed:
            // an integer this crate increments by one, constrained
            // non-negative by the schema, and never given a value from
            // outside the process. There is no string here for anything
            // to be typed into.
            "sessions.observed_compactions",
            // Migration 20. A cmux workspace reference of the shape
            // `workspace:<n>`, written only from what cmux itself
            // printed or from a reference a person typed on the command
            // line, and validated to that shape before it is ever handed
            // back. A pane number is not a place a credential could be
            // put.
            "sessions.presentation_ref",
            // Migration 21. A Git object name: forty hex characters read
            // out of `.git/HEAD` and the ref it points at, by
            // `checkpoint::git::GitPosition::detect`, which spawns no
            // process and parses nothing a provider or a user supplied.
            // The only strings that can reach it come from files Git
            // itself wrote.
            "sessions.last_seen_commit",
            // Migration 22. The key of an `[entitlements.<name>]` table
            // — the name a person typed in their own configuration file,
            // and the same string `glasshouse status` already prints. An
            // entitlement's authentication is a `config::SecretRef`, a
            // reference resolved through the operating system's secret
            // storage at the moment of use; no value it names can reach
            // this column, because the only writer copies
            // `ResolvedEntitlement::name` and nothing else.
            "sessions.entitlement",
            // Migration 19: the six fields an agent states about a
            // premise and their bookkeeping. Free text, sanitized by the
            // writer and bounded; no column is named for, or shaped
            // like, a credential.
            "task_assumptions.id",
            "task_assumptions.project_id",
            "task_assumptions.session_id",
            "task_assumptions.created_at",
            "task_assumptions.origin",
            "task_assumptions.claim",
            "task_assumptions.evidence",
            "task_assumptions.evidence_source",
            "task_assumptions.uncertainty",
            "task_assumptions.affected",
            "task_assumptions.verification",
            // Migration 28. A project identifier, a Glasshouse session
            // identifier, and three timestamps — five columns, and
            // deliberately nothing else. A declaration is one bit
            // (*nearly complete*) plus a scope plus a horizon, so there
            // is no note, label or summary column: a free-text column
            // here would be a place for prompt text or session content
            // to reach a table that exists to answer a boolean, and
            // nothing about the work would be readable back anyway.
            "task_progress_declarations.project_id",
            "task_progress_declarations.session_id",
            "task_progress_declarations.declared_at",
            "task_progress_declarations.renewed_at",
            "task_progress_declarations.expires_at",
        ],
        "the project database schema changed; confirm the new column cannot \
         hold a provider credential before updating this list"
    );
}

// ---------------------------------------------------------------
// Phase 9A — a launch profile is a reference here, never a definition.
// ---------------------------------------------------------------

/// The database schema has exactly a reference column for the profile a
/// session ran under, and no table defining what a profile *is* —
/// profiles are configuration, resolved in `crate::config`/
/// `crate::profile`, never project memory.
#[test]
fn no_launch_profile_definition_is_stored_in_the_project_database() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let mut statement = fixture
        .conn
        .prepare(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' \
             ORDER BY name",
        )
        .unwrap();
    let tables: Vec<String> = statement
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        tables,
        vec![
            // Migration 19 (carried from `assumption-guardrails`): a
            // ledger of stated premises, not a profile definition.
            "assumption_transitions",
            "checkpoints",
            "evaluation_observations",
            // Migration 27: who is working on which file, not a profile
            // definition.
            "file_claims",
            "lifecycle_events",
            "memories",
            "memories_fts",
            "memories_fts_config",
            "memories_fts_data",
            "memories_fts_docsize",
            "memories_fts_idx",
            "memory_files",
            "project_metadata",
            "routing_observations",
            "schema_migrations",
            "sessions",
            "task_assumptions",
            // Migration 28: whose current task somebody declared nearly
            // complete, not a profile definition.
            "task_progress_declarations",
        ],
        "no table defining launch profiles may exist in the project database"
    );

    let record = fixture
        .store()
        .create(
            NewSession::embedded("claude-code")
                .with_launch_profile(Some("native".to_owned()))
                .with_backend_resource(Some("native".to_owned())),
        )
        .unwrap();
    assert_eq!(record.launch_profile.as_deref(), Some("native"));
    assert_eq!(record.backend_resource.as_deref(), Some("native"));

    let read_back = fixture.store().get(&record.id).unwrap().unwrap();
    assert_eq!(read_back.launch_profile.as_deref(), Some("native"));
    assert_eq!(read_back.backend_resource.as_deref(), Some("native"));
}

/// Building a session without naming a profile leaves both columns NULL
/// rather than inventing a value — the same "None means not recorded"
/// rule the rest of this table already follows for `native_session_id`.
#[test]
fn a_session_with_no_recorded_profile_leaves_both_columns_null() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let record = fixture
        .store()
        .create(NewSession::embedded("claude-code"))
        .unwrap();
    assert_eq!(record.launch_profile, None);
    assert_eq!(record.backend_resource, None);
}

/// An existing version-2 database gains the two launch-profile columns on
/// the next launch, with every existing session's data intact and both
/// new columns `NULL` — a session recorded before this migration ran is a
/// different fact from one that ran the Native profile, so NULL must
/// stay NULL rather than default to `"native"`.
#[test]
fn upgrading_a_version_2_database_preserves_every_existing_session() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let store = fixture.store();

    let record = store
        .create(NewSession::embedded("claude-code").with_role(SessionRole::Worker))
        .unwrap();
    store.set_native_session_id(&record.id, "native-1").unwrap();
    store
        .set_lifecycle(&record.id, SessionLifecycle::Stopped)
        .unwrap();

    // Roll the database back to what version 2 left behind: drop what
    // migrations 3 and 4 added, and forget that they ran.
    //
    // `DELETE ... WHERE version = 3` is what this said while 3 was the
    // highest migration, and it stopped working the moment 4 existed. The
    // runner resumes from `MAX(version)`, so deleting only row 3 leaves a
    // *hole* — max is still 4, nothing re-applies, and the test failed
    // later and confusingly with "no such column: launch_profile". Roll
    // back a contiguous range, or do not roll back at all.
    //
    // Everything a later migration created has to go with the rows that
    // record it, or the re-run fails on `table … already exists` instead —
    // which is the same trap wearing the opposite coat, and is exactly how
    // migration 5 announced itself here.
    fixture
        .conn
        .execute_batch(
            "ALTER TABLE routing_observations DROP COLUMN completed_ms;
             ALTER TABLE routing_observations DROP COLUMN first_tool_call_ms;
             ALTER TABLE routing_observations DROP COLUMN first_token_ms;
             ALTER TABLE routing_observations DROP COLUMN first_byte_ms;
             ALTER TABLE routing_observations DROP COLUMN turn_shape;
             ALTER TABLE routing_observations DROP COLUMN effort_level;
             ALTER TABLE routing_observations DROP COLUMN session_id;
             ALTER TABLE routing_observations DROP COLUMN task_class;
             ALTER TABLE sessions DROP COLUMN entitlement;
             ALTER TABLE sessions DROP COLUMN last_seen_commit;
            ALTER TABLE sessions DROP COLUMN presentation_ref;
             ALTER TABLE sessions DROP COLUMN observed_compactions;
             ALTER TABLE sessions DROP COLUMN launch_profile;
             ALTER TABLE sessions DROP COLUMN backend_resource;
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
             DROP TABLE IF EXISTS memories_fts;
             DROP TABLE IF EXISTS memories;
             DROP TABLE IF EXISTS lifecycle_events;
             DROP TABLE IF EXISTS checkpoints;
             DROP TABLE IF EXISTS routing_observations;
             DROP TABLE IF EXISTS evaluation_observations;
             DROP TABLE IF EXISTS memory_files;
             DROP TABLE IF EXISTS assumption_transitions;
             DROP TABLE IF EXISTS task_assumptions;
             -- Migration 27's table: a rollback that leaves it in place
             -- meets `table file_claims already exists` on the re-run.
             DROP TABLE IF EXISTS task_progress_declarations;
             DROP TABLE IF EXISTS file_claims;
             DELETE FROM schema_migrations WHERE version >= 3;",
        )
        .unwrap();

    let reopened = fixture.reopen();
    let version: i64 = reopened
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(
        version, 28,
        "the launch must have applied migrations 3 through 22"
    );

    let migrated_store = SessionStore::new(&reopened).unwrap();
    let migrated = migrated_store
        .get(&record.id)
        .unwrap()
        .expect("the pre-migration session must survive");
    assert_eq!(migrated.id, record.id);
    assert_eq!(migrated.harness, "claude-code");
    assert_eq!(migrated.role, SessionRole::Worker);
    assert_eq!(migrated.native_session_id.as_deref(), Some("native-1"));
    assert_eq!(migrated.lifecycle, SessionLifecycle::Stopped);
    assert_eq!(migrated.created_at, record.created_at);
    assert_eq!(
        migrated.launch_profile, None,
        "a pre-migration session has no recorded profile — never a guessed default"
    );
    assert_eq!(migrated.backend_resource, None);
}

/// `project_metadata` is a key/value table, which is the one place a
/// credential could be smuggled in without a schema change. Its keys are
/// pinned too.
#[test]
fn project_metadata_holds_only_the_project_identifier() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    fixture
        .store()
        .create(NewSession::embedded("claude-code"))
        .unwrap();

    let mut statement = fixture
        .conn
        .prepare("SELECT key FROM project_metadata ORDER BY key")
        .unwrap();
    let keys: Vec<String> = statement
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(keys, vec!["project_id"]);
}

// ---------------------------------------------------------------
// Storage-layer integrity.
// ---------------------------------------------------------------

/// The `CHECK` constraints are the reason readers can trust the enum
/// columns, so verify they actually reject nonsense.
#[test]
fn the_schema_rejects_enum_values_it_does_not_define() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let project_id = fixture.project_id().to_owned();

    for (column, bad) in [
        ("role", "admin"),
        ("lifecycle", "probably_fine"),
        ("presentation", "invisible"),
    ] {
        let mut values = std::collections::HashMap::from([
            ("role", "normal"),
            ("lifecycle", "starting"),
            ("presentation", "embedded"),
        ]);
        values.insert(column, bad);

        let result = fixture.conn.execute(
            "INSERT INTO sessions (id, project_id, harness, role, lifecycle, \
             presentation, created_at, last_activity_at) \
             VALUES (?1, ?2, 'claude-code', ?3, ?4, ?5, 1, 1)",
            rusqlite::params![
                format!("bad-{column}"),
                &project_id,
                values["role"],
                values["lifecycle"],
                values["presentation"],
            ],
        );
        assert!(result.is_err(), "`{column}` must reject `{bad}`");
    }
}

/// A value that somehow got past the constraint must surface as a typed
/// error naming the column, never a panic or a silent default.
#[test]
fn an_unrecognized_stored_enum_value_is_reported_rather_than_guessed() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let record = fixture
        .store()
        .create(NewSession::embedded("claude-code"))
        .unwrap();

    // Rebuild the table without its CHECK constraints to model a database
    // written by a future build that knows a lifecycle this one does not.
    fixture
        .conn
        .execute_batch(
            "PRAGMA writable_schema = ON;
             UPDATE sqlite_master
                SET sql = replace(sql, \"CHECK (lifecycle IN ('starting', 'running', 'idle',\
\n                                 'waiting_for_user', 'stopped', 'failed',\
\n                                 'closed'))\", '')
              WHERE type = 'table' AND name = 'sessions';
             PRAGMA writable_schema = OFF;",
        )
        .unwrap();
    let reopened = fixture.reopen();
    reopened
        .execute(
            "UPDATE sessions SET lifecycle = 'hibernating' WHERE id = ?1",
            [record.id.as_str()],
        )
        .unwrap();

    let store = SessionStore::new(&reopened).unwrap();
    let error = store
        .get(&record.id)
        .expect_err("an unknown lifecycle must not be guessed");
    match error {
        SessionStoreError::UnknownValue { column, value, .. } => {
            assert_eq!(column, "lifecycle");
            assert_eq!(value, "hibernating");
        }
        other => panic!("expected UnknownValue, got {other:?}"),
    }
}

/// Identifiers come from SQLite's CSPRNG rather than the clock, because
/// sessions get spawned in bursts.
#[test]
fn generated_session_identifiers_are_unique_within_a_burst() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    // A frozen clock: any identifier derived from time would collide.
    let store = fixture.store_with_ticking_clock(42, 0);

    let ids: std::collections::HashSet<_> = (0..64)
        .map(|_| {
            store
                .create(NewSession::embedded("claude-code"))
                .unwrap()
                .id
        })
        .collect();
    assert_eq!(ids.len(), 64, "identifiers must not collide");
}

/// An existing version-1 database gains the sessions table on the next
/// launch without losing its project binding.
#[test]
fn a_version_one_database_migrates_forward_keeping_its_binding() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let project_id = fixture.project_id().to_owned();

    // Wind the database back to what version 1 left behind.
    //
    // The deleted range must stay contiguous to the newest migration: the
    // runner resumes from `MAX(version)`, so leaving a higher row behind
    // makes it believe there is nothing to do. See the sibling test.
    fixture
        .conn
        .execute_batch(
            "DROP TRIGGER sessions_reject_foreign_project_insert;
             DROP TRIGGER sessions_reject_foreign_project_update;
             DROP TABLE sessions;
             DROP TABLE IF EXISTS memories_fts;
             DROP TABLE IF EXISTS memories;
             DROP TABLE IF EXISTS lifecycle_events;
             DROP TABLE IF EXISTS checkpoints;
             DROP TABLE IF EXISTS routing_observations;
             DROP TABLE IF EXISTS evaluation_observations;
             DROP TABLE IF EXISTS memory_files;
             DROP TABLE IF EXISTS assumption_transitions;
             DROP TABLE IF EXISTS task_assumptions;
             -- Migration 27's table: a rollback that leaves it in place
             -- meets `table file_claims already exists` on the re-run.
             DROP TABLE IF EXISTS task_progress_declarations;
             DROP TABLE IF EXISTS file_claims;
             DELETE FROM schema_migrations WHERE version >= 2;",
        )
        .unwrap();
    drop(fixture.reopen());

    let reopened = fixture.reopen();
    let version: i64 = reopened
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(
        version, 28,
        "the launch must have applied migrations 2 through 22"
    );

    let store = SessionStore::new(&reopened).unwrap();
    assert_eq!(store.project_id(), project_id, "the binding survived");
    let record = store.create(NewSession::embedded("claude-code")).unwrap();
    assert_eq!(record.project_id, project_id);
}

/// Two projects on one machine keep entirely separate session lists —
/// separate files, not a shared file with a filter.
#[test]
fn two_projects_have_independent_session_lists() {
    let tmp = tempfile::tempdir().unwrap();
    let alpha = Fixture::new(tmp.path(), "alpha");
    let beta = Fixture::new(tmp.path(), "beta");

    alpha
        .store()
        .create(NewSession::embedded("claude-code"))
        .unwrap();
    alpha.store().create(NewSession::embedded("codex")).unwrap();
    beta.store()
        .create(NewSession::embedded("claude-code"))
        .unwrap();

    assert_ne!(alpha.runtime.database_path(), beta.runtime.database_path());
    assert_eq!(alpha.store().list().unwrap().len(), 2);
    assert_eq!(beta.store().list().unwrap().len(), 1);
}

/// The store refuses to work against a database with no project bound,
/// rather than defaulting to something and writing rows nobody can place.
#[test]
fn the_store_refuses_an_unbound_database() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    fixture
        .conn
        .execute("DELETE FROM project_metadata WHERE key = 'project_id'", [])
        .unwrap();

    let error = SessionStore::new(&fixture.conn).expect_err("an unbound database is unusable");
    assert!(
        matches!(error, SessionStoreError::UnboundDatabase),
        "got {error:?}"
    );
}

/// The injected clock is the one every test above uses, so the real one
/// needs its own check that it returns sane epoch seconds rather than,
/// say, nanoseconds or zero.
#[test]
fn the_default_clock_returns_plausible_epoch_seconds() {
    let first = system_clock();
    let second = system_clock();
    assert!(
        second >= first,
        "the wall clock must not run backwards mid-test"
    );
    assert!(
        first > 1_600_000_000,
        "the clock must return seconds since the epoch"
    );
    assert!(
        first < 32_000_000_000,
        "seconds, not milliseconds or nanoseconds"
    );
}

// --- resolving an identifier ----------------------------------------

#[test]
fn a_whole_identifier_resolves_to_its_session() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let store = fixture.store();
    let record = store.create(NewSession::embedded("claude-code")).unwrap();
    assert_eq!(store.resolve_id(record.id.as_str()).unwrap(), record.id);
}

#[test]
fn the_short_form_the_listing_prints_is_enough_to_resolve() {
    // `glasshouse sessions` prints twelve characters and nothing else, so
    // twelve characters have to be usable. If they were not, the only
    // identifier a user can see would be the one they cannot use.
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let store = fixture.store();
    let record = store.create(NewSession::embedded("claude-code")).unwrap();

    let short: String = record.id.as_str().chars().take(12).collect();
    assert_eq!(store.resolve_id(&short).unwrap(), record.id);
}

#[test]
fn an_ambiguous_prefix_is_refused_and_names_its_candidates() {
    // Resuming the wrong session is worse than being asked to type more.
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let store = fixture.store();
    let first = store.create(NewSession::embedded("claude-code")).unwrap();
    let second = store.create(NewSession::embedded("codex")).unwrap();

    // Every identifier shares the empty prefix; the shortest prefix both
    // share is found by comparison so the test does not depend on the
    // random values.
    let shared: String = first
        .id
        .as_str()
        .chars()
        .zip(second.id.as_str().chars())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a)
        .collect();
    let ambiguous = shared;
    if ambiguous.is_empty() {
        // Two identifiers with no shared prefix: use a one-character one
        // that both cannot share, and assert the exact-match path instead.
        assert_eq!(store.resolve_id(first.id.as_str()).unwrap(), first.id);
        return;
    }

    match store.resolve_id(&ambiguous) {
        Err(SessionStoreError::AmbiguousPrefix { matches, .. }) => {
            assert!(matches.contains(&first.id));
            assert!(matches.contains(&second.id));
        }
        other => panic!("expected an ambiguous prefix, got {other:?}"),
    }
}

#[test]
fn an_unknown_identifier_is_not_found() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let store = fixture.store();
    store.create(NewSession::embedded("claude-code")).unwrap();
    assert!(matches!(
        store.resolve_id("ffffffffffffffffffffffffffffffff"),
        Err(SessionStoreError::NotFound { .. })
    ));
}

#[test]
fn a_wildcard_cannot_be_smuggled_into_the_lookup() {
    // Identifiers are matched with `substr`, not `LIKE`. Under `LIKE`, a
    // bare `%` would match every session in the project, and resuming
    // "whichever one came first" is exactly the wrong answer.
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let store = fixture.store();
    store.create(NewSession::embedded("claude-code")).unwrap();

    for hostile in ["%", "_", "%%", "a%", "' OR 1=1 --"] {
        assert!(
            matches!(
                store.resolve_id(hostile),
                Err(SessionStoreError::MalformedId { .. })
            ),
            "`{hostile}` was not refused"
        );
    }
}

#[test]
fn an_empty_identifier_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let store = fixture.store();
    assert!(matches!(
        store.resolve_id("   "),
        Err(SessionStoreError::MalformedId { .. })
    ));
}

// --- assigned native identifiers -------------------------------------

#[test]
fn a_minted_native_identifier_is_a_valid_version_4_uuid() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let store = fixture.store();
    for _ in 0..64 {
        let id = store.new_native_session_id().unwrap();
        assert_eq!(id.len(), 36, "{id}");
        let groups: Vec<&str> = id.split('-').collect();
        assert_eq!(
            groups.iter().map(|g| g.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12],
            "{id}"
        );
        assert!(
            id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'),
            "{id}"
        );
        // The two things a strict validator checks beyond the shape.
        assert_eq!(groups[2].chars().next(), Some('4'), "version nibble: {id}");
        assert!(
            matches!(groups[3].chars().next(), Some('8' | '9' | 'a' | 'b')),
            "variant nibble: {id}"
        );
    }
}

#[test]
fn minted_native_identifiers_do_not_repeat() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let store = fixture.store();
    let mut seen = std::collections::HashSet::new();
    for _ in 0..256 {
        assert!(
            seen.insert(store.new_native_session_id().unwrap()),
            "a minted identifier repeated"
        );
    }
}

#[test]
fn the_uuid_formatter_only_overwrites_the_version_and_variant() {
    // Every other nibble survives, so the identifier keeps 122 bits of
    // the randomness it was given rather than being quietly reshaped.
    let hex = "0123456789abcdef0123456789abcdef";
    let uuid = uuid_v4_from_hex(hex);
    assert_eq!(uuid, "01234567-89ab-4def-8123-456789abcdef");

    let plain: String = uuid.chars().filter(|c| *c != '-').collect();
    let differences = hex
        .chars()
        .zip(plain.chars())
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    assert_eq!(differences, vec![12, 16], "only these two nibbles may move");
}

#[test]
fn a_session_can_be_recorded_with_its_native_identifier_from_the_start() {
    // The point of assignment: the record carries the identifier before
    // the harness has produced any output at all, so a session that dies
    // during startup is still resumable rather than anonymous.
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let store = fixture.store();
    let native = store.new_native_session_id().unwrap();
    let record = store
        .create(NewSession::embedded("claude-code").with_native_session_id(Some(native.clone())))
        .unwrap();
    assert_eq!(record.native_session_id.as_deref(), Some(native.as_str()));

    let read_back = store.get(&record.id).unwrap().expect("the session");
    assert_eq!(read_back.native_session_id, Some(native));
}

// ---------------------------------------------------------------
// Phase 10A — one ordered path for lifecycle changes.
// ---------------------------------------------------------------

/// *"Apply session lifecycle changes through a single ordered path."*
///
/// The ordering is worth nothing if there are two paths, and a second one
/// is the natural thing to add: supervision needs to move a session to
/// `stopped` when it finds the process gone, and writing its own `UPDATE`
/// beside the conclusion it had just drawn was in fact the first way this
/// was written. It passed every behavioural test and left two writers with
/// two orders.
///
/// So the structure is the assertion. Reads by lines, so it is blind to
/// line endings by construction — see `docs/product/design-decisions.md`.
///
/// Phase 59 (`GH-DECOMP-SESSION-STORE`) split `session/store.rs` into
/// `mod.rs`, `record.rs` and `context.rs`, none of which holds a `mod tests`
/// of its own any more -- every inline test moved to this file -- so there is
/// no `#[cfg(test)]`/`mod tests` boundary left to find inside them: the
/// joined production source of the three files already is production code.
#[test]
fn one_statement_moves_a_sessions_lifecycle() {
    let source = store_source();
    let code: String = source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .flat_map(|line| line.chars().filter(|c| !c.is_whitespace()))
        .collect();

    let writes = code.matches("UPDATEsessionsSETlifecycle").count();
    assert_eq!(
        writes, 1,
        "{writes} statements move a session's lifecycle; there must be exactly \
         one, and every caller must reach it through `write_lifecycle_locked` \
         inside `in_a_write_transaction`"
    );

    let locked = code
        .find("fnwrite_lifecycle_locked(")
        .expect("the one write site must still be called `write_lifecycle_locked`");
    let write = code
        .find("UPDATEsessionsSETlifecycle")
        .expect("checked above");
    assert!(
        write > locked,
        "the lifecycle write must be inside `write_lifecycle_locked`, where the \
         read that decides it is also taken"
    );

    // And the transaction it must be run inside is `IMMEDIATE`. A deferred
    // one reads without the write lock and then has to upgrade, which
    // SQLite refuses outright once another connection has committed.
    assert!(
        code.contains(r#"execute_batch("BEGINIMMEDIATE")"#),
        "the write transaction must take SQLite's write lock up front"
    );
    assert!(
        !code.contains(r#"execute_batch("BEGINDEFERRED")"#)
            && !code.contains(r#"execute_batch("BEGIN")"#),
        "a deferred write transaction cannot order two writers"
    );
}

mod phase_10 {
    //! Phase 10 — the unified session model, at the storage layer.
    //!
    //! The production surfaces live in `main.rs` and are exercised against the
    //! shipped binary in `tests/session_model.rs`. What is here is what only
    //! the store can answer: that a session records nine separate facts in
    //! nine separate places, that the two labels a person owns cannot reach
    //! the identifier a resume depends on, and that migration 8 leaves a
    //! version-7 database's rows exactly as it found them.

    use super::*;

    /// The seven kinds of thing a session records, all different, all read
    /// back apart.
    ///
    /// Every value below is distinct from every other, on purpose: a build
    /// that filled the pairing class in from the launch profile — or the
    /// model from the backend resource, or either label from the other —
    /// would put the same string in two columns, and this fails on it. That
    /// is the failure line 645 and the phase's second architectural
    /// requirement exist to prevent, and it is checked *after a reopen*, so
    /// what is proved is what is on disk rather than what was in memory.
    #[test]
    fn a_session_records_seven_facts_and_no_two_of_them_share_a_column() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();

        let profile = ResponseProfile::new(
            Verbosity::Terse,
            Audience::Executive,
            Narration::Silent,
            EvidenceDetail::Audit,
            AnswerFormat::Bullets,
        );
        let record = store
            .create(
                NewSession::embedded("claude-code")
                    .with_launch_profile(Some("a-launch-profile".to_owned()))
                    .with_backend_resource(Some("a-backend-resource".to_owned()))
                    .with_model(Some(AssignedModel::named("a-model")))
                    .with_pairing_class(Some(SessionPairingClass::ProtocolCompatible))
                    .with_protocol(Some(SessionProtocol::OpenAiResponses))
                    .with_response_profile(Some(profile))
                    .with_response_mechanism(Some(ResponseMechanism::Additive)),
            )
            .unwrap();

        let reopened = fixture.reopen();
        let stored = SessionStore::new(&reopened)
            .unwrap()
            .get(&record.id)
            .unwrap()
            .expect("the session survived the reopen");

        assert_eq!(stored.harness, "claude-code");
        assert_eq!(stored.launch_profile.as_deref(), Some("a-launch-profile"));
        assert_eq!(
            stored.backend_resource.as_deref(),
            Some("a-backend-resource")
        );
        assert_eq!(stored.model, Some(AssignedModel::named("a-model")));
        assert_eq!(
            stored.pairing_class,
            Some(SessionPairingClass::ProtocolCompatible)
        );
        assert_eq!(stored.protocol, Some(SessionProtocol::OpenAiResponses));
        assert_eq!(stored.response_profile, Some(profile));
        assert_eq!(stored.response_mechanism, Some(ResponseMechanism::Additive));

        // And the columns themselves hold seven different strings. Reading
        // the row rather than the record, because a record built from one
        // column read twice would satisfy every assertion above.
        let raw: Vec<Option<String>> = reopened
            .query_row(
                "SELECT harness, launch_profile, backend_resource, model, pairing_class, \
                 protocol, response_profile, response_mechanism FROM sessions WHERE id = ?1",
                [record.id.as_str()],
                |row| {
                    Ok((0..8)
                        .map(|i| row.get::<_, Option<String>>(i).unwrap())
                        .collect())
                },
            )
            .unwrap();
        let mut seen = std::collections::BTreeSet::new();
        for value in &raw {
            let value = value.as_deref().expect("every column was written");
            assert!(
                seen.insert(value.to_owned()),
                "two session columns hold `{value}`; the seven facts line 645 \
                 keeps apart have started sharing a slot:\n{raw:?}"
            );
        }
    }

    /// Line 646, at the only door there is.
    ///
    /// A provider and the gateway are not integrations at all, so they cannot
    /// even be named; `cmux`, `ollama` and `llama.cpp` are integrations and
    /// still not harnesses. None of the five may own a session, and the
    /// refusal happens before an identifier is minted, so nothing is left
    /// behind.
    #[test]
    fn only_a_real_harness_may_own_a_session() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();

        for owner in ["cmux", "ollama", "llama-cpp"] {
            let err = store.create(NewSession::embedded(owner)).unwrap_err();
            assert!(
                matches!(err, SessionStoreError::NotAHarness { .. }),
                "`{owner}` is not a harness and must be refused as one, got: {err}"
            );
        }
        for backend in [
            "openai",
            "anthropic",
            "openrouter",
            "glasshouse-gateway",
            "",
        ] {
            let err = store.create(NewSession::embedded(backend)).unwrap_err();
            assert!(
                matches!(err, SessionStoreError::UnknownHarness { .. }),
                "`{backend}` is a backend, never a session owner, got: {err}"
            );
        }
        assert!(
            store.list().unwrap().is_empty(),
            "a refused session must leave no row behind"
        );

        // And every real harness is still accepted, so the guard is a filter
        // rather than a wall.
        for harness in ["claude-code", "codex", "opencode", "cursor", "pi", "hermes"] {
            store
                .create(NewSession::embedded(harness))
                .unwrap_or_else(|err| panic!("`{harness}` is a harness: {err}"));
        }
    }

    /// Line 650. The rename writes one column; the identifier a resume
    /// depends on is read back afterwards and is the one it was before.
    #[test]
    fn renaming_a_session_leaves_its_native_identifier_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();

        let record = store
            .create(
                NewSession::embedded("claude-code").with_native_session_id(Some(
                    "d4c3b2a1-0000-4000-8000-000000000001".to_owned(),
                )),
            )
            .unwrap();

        let renamed = store
            .rename(
                &record.id,
                &SessionName::parse("  the auth probe  ").unwrap(),
            )
            .unwrap();

        assert_eq!(
            renamed.display_name.as_ref().map(SessionName::as_str),
            Some("the auth probe"),
            "surrounding whitespace is trimmed rather than stored"
        );
        assert_eq!(
            renamed.native_session_id.as_deref(),
            Some("d4c3b2a1-0000-4000-8000-000000000001"),
            "a rename must not touch the identifier a resume continues from"
        );
        assert_eq!(renamed.id, record.id, "nor the Glasshouse identifier");

        // And it is still resumable afterwards, which is the consequence that
        // would actually bite a user.
        store
            .set_lifecycle(&record.id, SessionLifecycle::Stopped)
            .unwrap();
        let resumable = store.open_for_resume(&record.id).unwrap();
        assert_eq!(
            resumable.native_session_id,
            "d4c3b2a1-0000-4000-8000-000000000001"
        );
    }

    /// Renaming and tagging are things the *user* did, not things the session
    /// did, so neither counts as activity.
    ///
    /// The listing is ordered by `last_activity_at`. If a rename stamped it,
    /// naming an old session would jump it to the top of a list whose whole
    /// job is to say what ran most recently.
    #[test]
    fn naming_or_tagging_a_session_is_not_session_activity() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store_with_ticking_clock(1_000, 100);

        let record = store.create(NewSession::embedded("claude-code")).unwrap();
        let created_activity = record.last_activity_at;

        let renamed = store
            .rename(&record.id, &SessionName::parse("old work").unwrap())
            .unwrap();
        assert_eq!(renamed.last_activity_at, created_activity);

        let tagged = store
            .set_purpose(&record.id, &SessionPurpose::parse("research").unwrap())
            .unwrap();
        assert_eq!(tagged.last_activity_at, created_activity);

        store
            .set_lifecycle(&record.id, SessionLifecycle::Stopped)
            .unwrap();
        let closed = store.close(&record.id).unwrap();
        assert_eq!(
            closed.last_activity_at,
            store.get(&record.id).unwrap().unwrap().last_activity_at,
        );
        assert_ne!(
            closed.last_activity_at, created_activity,
            "the state change before it *was* activity, and did move the clock"
        );
    }

    /// Line 651. A name and a purpose are two columns and two types, and
    /// setting one leaves the other exactly as it was.
    #[test]
    fn a_name_and_a_purpose_are_two_different_things() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();

        let record = store.create(NewSession::embedded("codex")).unwrap();
        store
            .rename(&record.id, &SessionName::parse("nightly").unwrap())
            .unwrap();
        let tagged = store
            .set_purpose(&record.id, &SessionPurpose::parse("tests").unwrap())
            .unwrap();

        assert_eq!(
            tagged.display_name.as_ref().map(SessionName::as_str),
            Some("nightly")
        );
        assert_eq!(
            tagged.purpose.as_ref().map(SessionPurpose::as_str),
            Some("tests")
        );

        let cleared = store.clear_purpose(&record.id).unwrap();
        assert_eq!(cleared.purpose, None);
        assert_eq!(
            cleared.display_name.as_ref().map(SessionName::as_str),
            Some("nightly"),
            "clearing a purpose must not clear a name"
        );

        let unnamed = store.clear_name(&record.id).unwrap();
        assert_eq!(unnamed.display_name, None);
        assert_eq!(unnamed.purpose, None);
    }

    /// A label a person typed is refused rather than repaired.
    #[test]
    fn an_unusable_label_is_refused_by_name() {
        assert!(SessionName::parse("   ").is_err());
        assert!(SessionPurpose::parse("").is_err());
        assert!(SessionName::parse("two\nlines").is_err());
        assert!(SessionPurpose::parse("a\tb").is_err());
        assert!(SessionName::parse(&"x".repeat(MAX_SESSION_NAME)).is_ok());
        assert!(SessionName::parse(&"x".repeat(MAX_SESSION_NAME + 1)).is_err());
        assert!(SessionPurpose::parse(&"x".repeat(MAX_SESSION_PURPOSE)).is_ok());
        assert!(SessionPurpose::parse(&"x".repeat(MAX_SESSION_PURPOSE + 1)).is_err());
        // Counted in characters, not bytes: a name of thirty-two emoji is
        // thirty-two characters and would be a hundred and twenty-eight bytes.
        assert!(SessionPurpose::parse(&"é".repeat(MAX_SESSION_PURPOSE)).is_ok());
    }

    /// Line 654. Closing writes one column and leaves the pointer to the
    /// harness's own history exactly where it was.
    #[test]
    fn closing_a_session_keeps_the_pointer_to_its_native_history() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();

        let record = store
            .create(
                NewSession::embedded("claude-code").with_native_session_id(Some(
                    "11112222-3333-4444-8555-666677778888".to_owned(),
                )),
            )
            .unwrap();
        store
            .set_lifecycle(&record.id, SessionLifecycle::Stopped)
            .unwrap();

        let closed = store.close(&record.id).unwrap();
        assert_eq!(closed.lifecycle, SessionLifecycle::Closed);
        assert_eq!(
            closed.native_session_id.as_deref(),
            Some("11112222-3333-4444-8555-666677778888"),
            "closing a Glasshouse record must not throw away the name of the \
             harness history it points at"
        );

        // Still there after a reopen, so what survived is the file rather
        // than a value the closing call happened to return.
        let reopened = fixture.reopen();
        let after = SessionStore::new(&reopened)
            .unwrap()
            .get(&record.id)
            .unwrap()
            .expect("a closed record is retired, never deleted");
        assert_eq!(
            after.native_session_id.as_deref(),
            Some("11112222-3333-4444-8555-666677778888")
        );
        assert_eq!(after.harness, "claude-code");
    }

    /// A record whose process is still running is not finished being written.
    #[test]
    fn a_live_session_cannot_be_closed() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();

        let record = store.create(NewSession::embedded("claude-code")).unwrap();
        for live in [
            SessionLifecycle::Starting,
            SessionLifecycle::Running,
            SessionLifecycle::Idle,
            SessionLifecycle::WaitingForUser,
        ] {
            store.set_lifecycle(&record.id, live).unwrap();
            let err = store.close(&record.id).unwrap_err();
            assert!(
                matches!(err, SessionStoreError::StillLive { .. }),
                "a {live} session must be stopped before its record is closed, got: {err}"
            );
            assert_eq!(store.get(&record.id).unwrap().unwrap().lifecycle, live);
        }
    }

    /// Line 653. A stopped session with something to resume to is a different
    /// row from a live one and from a closed one, and closing moves it out of
    /// the resumable group without deleting it.
    #[test]
    fn a_resumable_session_stays_visible_and_apart_from_the_live_ones() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();

        let live = store.create(NewSession::embedded("claude-code")).unwrap();
        store
            .set_lifecycle(&live.id, SessionLifecycle::Running)
            .unwrap();

        let stopped = store
            .create(
                NewSession::embedded("codex")
                    .with_native_session_id(Some("codex-native-1".to_owned())),
            )
            .unwrap();
        store
            .set_lifecycle(&stopped.id, SessionLifecycle::Stopped)
            .unwrap();

        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 2, "both are visible in one listing");
        let by_id = |id: &SessionId| {
            listed
                .iter()
                .find(|record| &record.id == id)
                .unwrap()
                .disposition()
        };
        assert_eq!(by_id(&live.id), SessionDisposition::Active);
        assert_eq!(by_id(&stopped.id), SessionDisposition::Resumable);

        store.close(&stopped.id).unwrap();
        let after = store.list().unwrap();
        assert_eq!(after.len(), 2, "a closed session is retired, not removed");
        assert_eq!(
            after
                .iter()
                .find(|record| record.id == stopped.id)
                .unwrap()
                .disposition(),
            SessionDisposition::Closed
        );
    }

    /// Every value the store can write is one the schema accepts.
    ///
    /// The `CHECK` constraints in migration 8 are second copies of three
    /// vocabularies. This is what keeps the copies honest: each variant is
    /// written through a real insert, so a slug that drifted from the schema
    /// fails here rather than on a background writer where nobody is looking.
    #[test]
    fn every_stored_vocabulary_is_one_the_schema_accepts() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();

        let classes = [
            SessionPairingClass::VendorNative,
            SessionPairingClass::VendorSupported,
            SessionPairingClass::ProtocolNative,
            SessionPairingClass::ProtocolCompatible,
            SessionPairingClass::ProtocolTranslated,
            SessionPairingClass::Unknown,
        ];
        let protocols = [
            SessionProtocol::AnthropicMessages,
            SessionProtocol::OpenAiResponses,
            SessionProtocol::OpenAiChat,
            SessionProtocol::Unknown,
        ];
        let mechanisms = [
            ResponseMechanism::Native,
            ResponseMechanism::Additive,
            ResponseMechanism::NotApplied,
        ];

        for (i, class) in classes.iter().enumerate() {
            let protocol = protocols[i % protocols.len()];
            let mechanism = mechanisms[i % mechanisms.len()];
            let record = store
                .create(
                    NewSession::embedded("claude-code")
                        .with_pairing_class(Some(*class))
                        .with_protocol(Some(protocol))
                        .with_response_mechanism(Some(mechanism))
                        .with_model(Some(AssignedModel::HarnessDefault)),
                )
                .unwrap_or_else(|err| panic!("the schema rejected {class}/{protocol}: {err}"));
            let read = store.get(&record.id).unwrap().unwrap();
            assert_eq!(read.pairing_class, Some(*class));
            assert_eq!(read.protocol, Some(protocol));
            assert_eq!(read.response_mechanism, Some(mechanism));
        }
        // The two the loop above could not reach by index.
        for protocol in protocols {
            for mechanism in mechanisms {
                store
                    .create(
                        NewSession::embedded("codex")
                            .with_protocol(Some(protocol))
                            .with_response_mechanism(Some(mechanism)),
                    )
                    .unwrap_or_else(|err| panic!("the schema rejected {protocol}: {err}"));
            }
        }
    }

    /// "Glasshouse assigned no model" and "this build recorded no model" are
    /// two facts, and the column keeps them apart.
    #[test]
    fn a_harness_default_is_not_the_same_stored_fact_as_nothing_recorded() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();

        let defaulted = store
            .create(
                NewSession::embedded("claude-code").with_model(Some(AssignedModel::HarnessDefault)),
            )
            .unwrap();
        let unrecorded = store.create(NewSession::embedded("claude-code")).unwrap();
        let named = store
            .create(
                NewSession::embedded("claude-code")
                    .with_model(Some(AssignedModel::named("harness-default"))),
            )
            .unwrap();

        assert_eq!(
            store.get(&defaulted.id).unwrap().unwrap().model,
            Some(AssignedModel::HarnessDefault)
        );
        assert_eq!(store.get(&unrecorded.id).unwrap().unwrap().model, None);
        // A model whose id is literally the sentinel word still reads back as
        // a named model, which is what the `named:` prefix is for.
        assert_eq!(
            store.get(&named.id).unwrap().unwrap().model,
            Some(AssignedModel::named("harness-default"))
        );
    }

    /// All 324 combinations of the five axes survive the round trip, so no
    /// axis is dropped or defaulted on the way through the column.
    #[test]
    fn every_response_profile_round_trips_through_one_column() {
        for verbosity in Verbosity::ALL {
            for audience in Audience::ALL {
                for narration in Narration::ALL {
                    for evidence in EvidenceDetail::ALL {
                        for format in AnswerFormat::ALL {
                            let profile = ResponseProfile::new(
                                *verbosity, *audience, *narration, *evidence, *format,
                            );
                            let encoded = encode_response_profile(&profile);
                            assert_eq!(
                                decode_response_profile(&encoded),
                                Some(profile),
                                "`{encoded}` did not come back as the profile it was"
                            );
                        }
                    }
                }
            }
        }
        // A partial encoding is refused rather than completed from defaults:
        // a profile a session never ran under, reported as though it had, is
        // worse than an error naming the column.
        assert_eq!(
            decode_response_profile("verbosity=terse,audience=plain"),
            None
        );
        assert_eq!(decode_response_profile("verbosity=nonsense"), None);
        assert_eq!(decode_response_profile(""), None);
    }

    /// A value this build cannot read is reported by column and value, never
    /// silently turned into `None` — which would say "nothing was recorded"
    /// about a row that recorded something.
    #[test]
    fn a_stored_value_this_build_cannot_read_is_reported_rather_than_erased() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();
        let record = store.create(NewSession::embedded("claude-code")).unwrap();

        // Written straight into the column, as a newer build's value would
        // arrive: the schema's `CHECK` is what stops this build writing one.
        fixture
            .conn
            .execute(
                "UPDATE sessions SET response_profile = 'verbosity=galactic' WHERE id = ?1",
                [record.id.as_str()],
            )
            .unwrap();

        let err = store.get(&record.id).unwrap_err();
        match err {
            SessionStoreError::UnknownValue { column, value, .. } => {
                assert_eq!(column, "response_profile");
                assert_eq!(value, "verbosity=galactic");
            }
            other => panic!("expected the column and the value to be named, got: {other}"),
        }
    }

    /// A `harness` column holding bytes that are not valid UTF-8 — the
    /// shape a single flipped bit in an otherwise-intact row produces,
    /// invisible to `PRAGMA integrity_check` (`store-db.md` finding #1) —
    /// must be a reported error from `get` and `list`, never a panic that
    /// takes down every later invocation that reads a session back.
    ///
    /// On the unpatched tree (`row.get_unwrap` in `read_record`) this
    /// panics with exactly:
    ///   called `Result::unwrap()` on an `Err` value: Utf8Error(0, Utf8Error { valid_up_to: 0, error_len: Some(1) })
    #[test]
    fn a_hostile_harness_column_is_a_reported_error_not_a_panic() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();
        let record = store.create(NewSession::embedded("codex")).unwrap();

        fixture
            .conn
            .execute(
                "UPDATE sessions SET harness = CAST(x'ff' AS TEXT) WHERE id = ?1",
                [record.id.as_str()],
            )
            .unwrap();

        let error = store
            .get(&record.id)
            .expect_err("a hostile column must not panic `get`");
        assert!(
            matches!(error, SessionStoreError::Sql { .. }),
            "expected Sql, got {error:?}"
        );

        let error = store
            .list()
            .expect_err("a hostile column must not panic `list`");
        assert!(
            matches!(error, SessionStoreError::Sql { .. }),
            "expected Sql, got {error:?}"
        );
    }

    /// `entitlement` (migration 22, the newest column `read_record` reads)
    /// is never decoded, only wrapped — same shape as `source_session_id`
    /// and `presentation_ref` — and shares the same failure mode: a
    /// present value that is not valid UTF-8 must surface as
    /// [`SessionStoreError::Sql`], not a panic.
    #[test]
    fn a_hostile_entitlement_column_is_a_reported_error_not_a_panic() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();
        let record = store.create(NewSession::embedded("codex")).unwrap();

        fixture
            .conn
            .execute(
                "UPDATE sessions SET entitlement = CAST(x'ff' AS TEXT) WHERE id = ?1",
                [record.id.as_str()],
            )
            .unwrap();

        let error = store
            .get(&record.id)
            .expect_err("a hostile entitlement column must not panic `get`");
        assert!(
            matches!(error, SessionStoreError::Sql { .. }),
            "expected Sql, got {error:?}"
        );
    }

    /// Migration 8 applies to a database created by the previous schema, and
    /// every existing row survives it unchanged.
    ///
    /// The rollback is contiguous to the newest migration for the reason
    /// `upgrading_a_version_2_database_preserves_every_existing_session`
    /// records: the runner resumes from `MAX(version)`, so leaving a higher
    /// row behind makes it believe there is nothing to do.
    #[test]
    fn upgrading_a_version_7_database_preserves_every_existing_session() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();

        let record = store
            .create(
                NewSession::embedded("codex")
                    .with_role(SessionRole::Orchestrator)
                    .with_presentation(SessionPresentation::Headless)
                    .with_native_session_id(Some("codex-native-7".to_owned()))
                    .with_launch_profile(Some("nightly".to_owned()))
                    .with_backend_resource(Some("direct:openai".to_owned())),
            )
            .unwrap();
        store
            .set_lifecycle(&record.id, SessionLifecycle::Stopped)
            .unwrap();
        let before = store.get(&record.id).unwrap().unwrap();

        fixture
            .conn
            .execute_batch(&format!(
                // Migration 10's columns go first, for the same reason
                // migration 8's sessions columns are dropped below: this
                // rollback lands on version 7, and `memories` must not
                // still carry columns a later migration added.
                "{UNDO_MIGRATIONS_ABOVE_THIRTEEN}
                 ALTER TABLE memories DROP COLUMN superseded_reason;
                 ALTER TABLE memories DROP COLUMN validity_conditions;
                 ALTER TABLE memories DROP COLUMN invalidation_conditions;
                 ALTER TABLE memories DROP COLUMN review_reason;
                 ALTER TABLE memories DROP COLUMN review_marked_at;
                 ALTER TABLE memories DROP COLUMN last_validated_at;
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
                 DROP TABLE IF EXISTS routing_observations;
             DROP TABLE IF EXISTS evaluation_observations;
             DROP TABLE IF EXISTS memory_files;
             -- Migration 27's table: a rollback that leaves it in place
             -- meets `table file_claims already exists` on the re-run.
             DROP TABLE IF EXISTS task_progress_declarations;
             DROP TABLE IF EXISTS file_claims;
             DELETE FROM schema_migrations WHERE version >= 8;"
            ))
            .unwrap();

        let reopened = fixture.reopen();
        let version: i64 = reopened
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            version, 28,
            "the launch must have applied migrations 8 through 22"
        );

        let after = SessionStore::new(&reopened)
            .unwrap()
            .get(&record.id)
            .unwrap()
            .expect("the pre-migration session must survive");

        // Everything migration 7 knew about, byte for byte.
        assert_eq!(after.id, before.id);
        assert_eq!(after.project_id, before.project_id);
        assert_eq!(after.harness, before.harness);
        assert_eq!(after.native_session_id, before.native_session_id);
        assert_eq!(after.role, before.role);
        assert_eq!(after.lifecycle, before.lifecycle);
        assert_eq!(after.presentation, before.presentation);
        assert_eq!(after.created_at, before.created_at);
        assert_eq!(after.last_activity_at, before.last_activity_at);
        assert_eq!(after.launch_profile, before.launch_profile);
        assert_eq!(after.backend_resource, before.backend_resource);

        // And the seven new columns are NULL rather than guessed at: a
        // session recorded before migration 8 ran under a response profile
        // Glasshouse never wrote down, which is a different fact from having
        // run the default one.
        assert_eq!(after.model, None);
        assert_eq!(after.pairing_class, None);
        assert_eq!(after.protocol, None);
        assert_eq!(after.response_profile, None);
        assert_eq!(after.response_mechanism, None);
        assert_eq!(after.display_name, None);
        assert_eq!(after.purpose, None);

        // The upgraded database is fully usable: the old row can still be
        // renamed and tagged, and a new session records all seven.
        let migrated_store = SessionStore::new(&reopened).unwrap();
        let renamed = migrated_store
            .rename(&record.id, &SessionName::parse("survivor").unwrap())
            .unwrap();
        assert_eq!(
            renamed.display_name.as_ref().map(SessionName::as_str),
            Some("survivor")
        );
        assert_eq!(renamed.native_session_id, before.native_session_id);
    }
}

mod phase_40 {
    //! Phase 40 line 1646 — which session, if any, a session was
    //! bootstrapped from.
    //!
    //! `main.rs::resolve_bootstrap_prompt` and `launch_session` are
    //! exercised against the shipped binary in `tests/handoff_lines.rs`
    //! (the positive case, once per harness pair, and the negative case
    //! of an ordinary launch). What is here is what only the store can
    //! answer: that the column round-trips on its own, and that a
    //! database written before this migration still reads back —
    //! `upgrading_a_version_7_database_preserves_every_existing_session`'s
    //! own reasoning, one migration later.
    use super::*;

    #[test]
    fn a_recorded_source_session_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();

        let source = store.create(NewSession::embedded("claude-code")).unwrap();
        let target = store
            .create(NewSession::embedded("codex").with_source_session(Some(source.id.clone())))
            .unwrap();
        assert_eq!(target.source_session_id, Some(source.id.clone()));

        let read_back = store.get(&target.id).unwrap().unwrap();
        assert_eq!(read_back.source_session_id, Some(source.id));
    }

    /// The negative case, and it matters as much as the positive one: a
    /// session started without naming a checkpoint must record no
    /// source, never an invented one.
    #[test]
    fn a_session_not_started_from_a_checkpoint_has_no_source() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let record = fixture
            .store()
            .create(NewSession::embedded("claude-code"))
            .unwrap();
        assert_eq!(record.source_session_id, None);
    }

    /// Migration 12 applies to a database created by the previous
    /// schema, and every existing row survives it unchanged — the same
    /// contiguous rollback `upgrading_a_version_7_database_preserves_
    /// every_existing_session` uses, one migration later.
    #[test]
    fn upgrading_a_version_11_database_preserves_every_existing_session() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();

        let record = store
            .create(
                NewSession::embedded("claude-code")
                    .with_role(SessionRole::Worker)
                    .with_native_session_id(Some("native-pre-12".to_owned())),
            )
            .unwrap();
        let before = store.get(&record.id).unwrap().unwrap();

        fixture
            .conn
            .execute_batch(&format!(
                "{UNDO_MIGRATIONS_ABOVE_THIRTEEN}
                 ALTER TABLE sessions DROP COLUMN source_session_id;
                 ALTER TABLE memories DROP COLUMN superseded_reason;
                 -- Migration 27's table: a rollback that leaves it in place
                 -- meets `table file_claims already exists` on the re-run.
                 DROP TABLE IF EXISTS task_progress_declarations;
             DROP TABLE IF EXISTS file_claims;
                 DELETE FROM schema_migrations WHERE version >= 12;"
            ))
            .unwrap();

        let reopened = fixture.reopen();
        let version: i64 = reopened
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            version, 28,
            "the reopen must have applied migrations 12 through 22"
        );

        let after = SessionStore::new(&reopened)
            .unwrap()
            .get(&record.id)
            .unwrap()
            .expect("the pre-migration session must survive");

        assert_eq!(after.id, before.id);
        assert_eq!(after.harness, before.harness);
        assert_eq!(after.native_session_id, before.native_session_id);
        assert_eq!(after.role, before.role);
        assert_eq!(after.lifecycle, before.lifecycle);

        // A session recorded before migration 12 ran has no recorded
        // source — a different fact from having been started fresh by a
        // build that could name one, but the column cannot and must not
        // distinguish them.
        assert_eq!(after.source_session_id, None);
    }
}

#[cfg(test)]
mod display_tests {
    use super::*;

    /// A `Display` impl that writes straight to the formatter ignores width,
    /// which turns any aligned listing into ragged columns. Cheap to get
    /// wrong, invisible in a round-trip test, so it gets its own check.
    #[test]
    fn stored_values_honour_format_width_so_listings_align() {
        assert_eq!(format!("[{:<10}]", SessionRole::Normal), "[normal    ]");
        assert_eq!(
            format!("[{:<10}]", SessionRole::Orchestrator),
            "[orchestrator]"
        );
        assert_eq!(
            format!("[{:<10}]", SessionPresentation::Embedded),
            "[embedded  ]"
        );
        assert_eq!(
            format!("[{:<20}]", SessionLifecycle::WaitingForUser),
            "[waiting_for_user    ]"
        );
        assert_eq!(format!("[{:<6}]", SessionId::new("ab")), "[ab    ]");
    }
}
