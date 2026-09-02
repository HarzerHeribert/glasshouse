//! Phase 30 — session context metadata, migration 16, and the seven of its
//! eight lines this build can honestly close.
//!
//! - **1158** *"Track an estimated context-size value for a session when the
//!   harness exposes enough information."* — **refused**, and the refusal is
//!   proven here rather than only argued: `a_context_size_has_no_producer_in_
//!   this_build` reads the two places a token count could come from and shows
//!   both are empty. There is no field for it in
//!   [`glasshouse::session::SessionContext`].
//! - **1159** the compaction count — migration 16's one column.
//! - **1160** the most recent request or turn time — `last_activity_at`,
//!   which already carried it.
//! - **1161/1162/1163** the advisory prompt-cache estimate.
//! - **1164** whether a recent portable checkpoint exists.
//! - **1165** the lightweight task-continuity flag.
//!
//! # Everything a harness does here goes through the shipped binary
//!
//! Practice §35: a caller every test bypasses is not a caller. The whole
//! claim of 1159 is that the compaction the *binary* observes is now written
//! down, and of 1165 that the task boundaries the *binary* acts on are
//! countable. So `glasshouse hook` is spawned as a process, exactly as a
//! harness spawns it, and no seam is used for either.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use clap::Parser;
use rusqlite::Connection;

use glasshouse::checkpoint::{Checkpoint, CheckpointReason, Handoff, ProjectCheckpoints};
use glasshouse::config::UserConfig;
use glasshouse::session::{
    AdvisoryCacheState, CacheState, CheckpointRecency, NewSession, ProjectSessions, SessionId,
    SessionLifecycle, SessionStore, TaskContinuity,
};
use glasshouse::{Cli, Runtime};

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

/// A bootstrapped project sharing `base`'s data and config roots, so two
/// fixtures over one `base` are two real projects on one machine.
struct Fixture {
    base: PathBuf,
    root: PathBuf,
    runtime: Runtime,
}

impl Fixture {
    fn new(base: &Path, name: &str) -> Self {
        let root = base.join("workspace").join(name);
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        let runtime = bootstrap(base, &root);
        Self {
            base: base.to_path_buf(),
            root,
            runtime,
        }
    }

    fn db(&self) -> Connection {
        Connection::open(self.runtime.database_path()).unwrap()
    }

    fn sessions(&self) -> ProjectSessions {
        ProjectSessions::open(&self.runtime).unwrap()
    }

    /// Run `glasshouse hook`, exactly as a harness runs it.
    ///
    /// The payload is written to the child's stdin because a harness writes
    /// one, and the handler drains it unread; a test that closed the pipe
    /// instead would not be running the production path.
    fn hook(&self, session: &SessionId, event: &str) {
        let mut child = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .arg("hook")
            .arg("--session")
            .arg(session.as_str())
            .arg("--event")
            .arg(event)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the glasshouse binary must be runnable");
        child
            .stdin
            .as_mut()
            .expect("stdin was piped")
            .write_all(PAYLOAD.as_bytes())
            .expect("the handler must read its payload rather than closing the pipe");
        let output = child.wait_with_output().expect("the hook must finish");
        assert!(
            output.status.success(),
            "a hook must exit zero: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn observed_compactions(&self, id: &SessionId) -> Option<i64> {
        self.sessions()
            .store()
            .get(id)
            .unwrap()
            .expect("the session must still be recorded")
            .observed_compactions
    }

    fn recorded_events(&self, id: &SessionId) -> i64 {
        self.db()
            .query_row(
                "SELECT COUNT(*) FROM lifecycle_events WHERE session_id = ?1",
                [id.as_str()],
                |row| row.get(0),
            )
            .unwrap()
    }
}

const PAYLOAD: &str = r#"{"session_id":"native-1","hook_event_name":"PreCompact"}"#;

fn bootstrap(base: &Path, root: &Path) -> Runtime {
    let cli = Cli::try_parse_from([
        "glasshouse",
        "--data-dir",
        base.join("data").to_str().unwrap(),
        "--config-dir",
        base.join("config").to_str().unwrap(),
    ])
    .unwrap();
    glasshouse::bootstrap(&cli, root).unwrap()
}

/// The wall clock, which is the only clock the hook process has.
fn system_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn schema_version(conn: &Connection) -> i64 {
    conn.query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
        row.get(0)
    })
    .unwrap()
}

/// A store over this project's database whose clock answers `now`.
///
/// A raw connection rather than [`ProjectSessions`], so that the reads under
/// test are the only thing that happens: opening a `ProjectSessions` also
/// supervises, which writes.
fn store_at(conn: &Connection, now: i64) -> SessionStore<'_> {
    SessionStore::with_clock(conn, Arc::new(move || now)).unwrap()
}

// ---------------------------------------------------------------------------
// Acceptance 1 — forward migration on a populated database
// ---------------------------------------------------------------------------

/// **Migration 16 on a database that already holds sessions.**
///
/// Two sessions are recorded, the database is wound back to schema 15, and an
/// ordinary launch migrates it forward. Every pre-existing row survives
/// untouched and its new column reads as **unknown** — `None`, not `Some(0)`.
///
/// That last assertion is the one this package most needs. A migration
/// written `NOT NULL DEFAULT 0` would pass every other test in this file and
/// would quietly tell a router that every session recorded before the upgrade
/// had been watched from the start and had compacted nothing.
#[test]
fn a_schema_fifteen_database_migrates_forward_and_its_sessions_read_as_uncounted() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let (first, second) = {
        let sessions = fixture.sessions();
        let store = sessions.store();
        let first = store
            .create(
                NewSession::embedded("claude-code")
                    .with_native_session_id(Some("native-pre-16".to_owned())),
            )
            .unwrap();
        let second = store.create(NewSession::embedded("codex")).unwrap();
        (first, second)
    };

    {
        let conn = fixture.db();
        // Every migration above 15 is undone, newest first: 24's three
        // columns, 23's column, 22's column, 21's two, 20's column, 19's two
        // tables, 18's column, 17's table, then 16's.
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
             ALTER TABLE sessions DROP COLUMN observed_compactions;
             DELETE FROM schema_migrations WHERE version >= 16;",
        )
        .unwrap();
        assert_eq!(
            schema_version(&conn),
            15,
            "the rollback must land on version 15"
        );
    }

    let migrated = bootstrap(tmp.path(), &fixture.root);
    let conn = Connection::open(migrated.database_path()).unwrap();
    assert_eq!(
        schema_version(&conn),
        26,
        "the launch must have applied migrations 16 through 26"
    );
    drop(conn);

    let sessions = ProjectSessions::open(&migrated).unwrap();
    let store = sessions.store();
    for before in [&first, &second] {
        let after = store
            .get(&before.id)
            .unwrap()
            .expect("a session recorded before the migration must survive it");

        assert_eq!(after.harness, before.harness);
        assert_eq!(after.native_session_id, before.native_session_id);
        assert_eq!(after.created_at, before.created_at);
        assert_eq!(after.last_activity_at, before.last_activity_at);
        assert_eq!(after.role, before.role);
        assert_eq!(after.presentation, before.presentation);
        assert_eq!(after.source_session_id, before.source_session_id);

        assert_eq!(
            after.observed_compactions, None,
            "a session recorded before migration 16 must read as `nobody was counting`, \
             never as a measured zero"
        );
        assert_ne!(
            after.observed_compactions,
            Some(0),
            "unknown and zero are different facts and the column must keep them apart"
        );
    }
}

/// The other half of the same distinction: a session **this** build starts
/// has a measured zero, so `None` and `Some(0)` are both reachable and mean
/// different things.
#[test]
fn a_session_this_build_starts_counts_from_a_measured_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let sessions = fixture.sessions();
    let store = sessions.store();

    let record = store.create(NewSession::embedded("claude-code")).unwrap();
    assert_eq!(record.observed_compactions, Some(0));
    assert_eq!(
        store.get(&record.id).unwrap().unwrap().observed_compactions,
        Some(0),
        "the measured zero must survive the round trip through the database"
    );
}

// ---------------------------------------------------------------------------
// Acceptance 4 — line 1159 through the shipped binary
// ---------------------------------------------------------------------------

/// **Line 1159, through the process a harness actually spawns.**
///
/// `PreCompact` is Codex's own name for "I am about to compact", and
/// `session::lifecycle::precedes_native_compaction` is the only thing in the
/// binary that recognises it. Two of them arrive; the count goes to two.
///
/// And **no `lifecycle_events` row is written** — the refusal register's
/// Cluster G, asserted rather than assumed. A twelfth `lifecycle_events.kind`
/// would need that table's `CHECK` widened, which SQLite cannot do in place.
#[test]
fn an_observed_compaction_is_counted_by_the_shipped_binary_and_writes_no_event() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let id = {
        let sessions = fixture.sessions();
        let store = sessions.store();
        let record = store.create(NewSession::embedded("codex")).unwrap();
        store
            .set_lifecycle(&record.id, SessionLifecycle::Running)
            .unwrap();
        record.id
    };

    assert_eq!(fixture.observed_compactions(&id), Some(0));
    let before = fixture
        .sessions()
        .store()
        .get(&id)
        .unwrap()
        .unwrap()
        .last_activity_at;

    fixture.hook(&id, "PreCompact");
    assert_eq!(
        fixture.observed_compactions(&id),
        Some(1),
        "the one production site that sees a compaction must count it"
    );

    fixture.hook(&id, "PreCompact");
    assert_eq!(fixture.observed_compactions(&id), Some(2));

    assert_eq!(
        fixture.recorded_events(&id),
        0,
        "a compaction must leave the event log exactly as narrow as it was"
    );

    let after = fixture.sessions().store().get(&id).unwrap().unwrap();
    assert_eq!(
        after.last_activity_at, before,
        "a compaction is the harness reorganising what it holds, not the session \
         doing work, so it must not move the activity stamp"
    );
    assert_eq!(
        after.lifecycle,
        SessionLifecycle::Running,
        "and it must not move the session's state either"
    );
}

/// A row from before migration 16 begins counting at its first observation
/// rather than staying unknowable, and what that costs is stated: the number
/// is then a lower bound.
#[test]
fn a_session_recorded_before_the_migration_starts_counting_at_its_first_compaction() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let id = {
        let sessions = fixture.sessions();
        let store = sessions.store();
        store.create(NewSession::embedded("codex")).unwrap().id
    };
    // Exactly what migration 16 leaves behind for a row it did not write.
    fixture
        .db()
        .execute(
            "UPDATE sessions SET observed_compactions = NULL WHERE id = ?1",
            [id.as_str()],
        )
        .unwrap();
    assert_eq!(fixture.observed_compactions(&id), None);

    fixture.hook(&id, "PreCompact");
    assert_eq!(
        fixture.observed_compactions(&id),
        Some(1),
        "an uncounted row must start counting rather than stay uncountable for ever"
    );
}

// ---------------------------------------------------------------------------
// Map line 1171 — refresh a portable checkpoint before intentional compaction
// ---------------------------------------------------------------------------

/// **Line 1171, acceptance 1.** A session with an existing checkpoint gets it
/// refreshed — `created_at` moves forward — when `PreCompact` fires, through
/// the shipped binary.
#[test]
fn a_precompact_hook_refreshes_an_existing_checkpoints_created_at() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let id = {
        let sessions = fixture.sessions();
        let store = sessions.store();
        store.create(NewSession::embedded("codex")).unwrap().id
    };

    store_checkpoint(&fixture, &id, 1_700_000_000);
    let before = latest_checkpoint(&fixture, &id).created_at;

    fixture.hook(&id, "PreCompact");

    let after = latest_checkpoint(&fixture, &id).created_at;
    assert!(
        after > before,
        "a refreshed checkpoint must move forward in time: before {before}, after {after}"
    );
}

/// **Line 1171, acceptance 2 — the ruling's own test.** The refreshed
/// checkpoint keeps the reason the previous one recorded rather than
/// stamping `TaskBoundary`: a compaction is not a turn ending, and
/// `CheckpointReason` has no third value honest enough to invent instead.
#[test]
fn a_precompact_hook_preserves_a_manual_checkpoints_reason() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let id = {
        let sessions = fixture.sessions();
        let store = sessions.store();
        store.create(NewSession::embedded("codex")).unwrap().id
    };

    store_checkpoint_with_reason(&fixture, &id, 1_700_000_000, CheckpointReason::Manual);

    fixture.hook(&id, "PreCompact");

    assert_eq!(
        latest_checkpoint(&fixture, &id).reason,
        CheckpointReason::Manual,
        "compaction must never restamp a manual checkpoint as a task boundary"
    );
}

/// **Line 1171, acceptance 3.** A session with no checkpoint gets none
/// invented, and the hook still succeeds: `store.latest_for(id)? == None` is
/// the whole of "when practical".
#[test]
fn a_precompact_hook_invents_no_checkpoint_for_a_session_that_never_had_one() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let id = {
        let sessions = fixture.sessions();
        let store = sessions.store();
        store.create(NewSession::embedded("codex")).unwrap().id
    };

    fixture.hook(&id, "PreCompact");

    let checkpoints = ProjectCheckpoints::open(&fixture.runtime).unwrap();
    assert!(
        checkpoints.store().latest_for(&id).unwrap().is_none(),
        "a session that never had a checkpoint must not get one invented at compaction time"
    );
}

/// **Line 1171, acceptance 4 — pins the gating ruling.** With automatic
/// checkpoints disabled, `PreCompact` refreshes nothing, but the compaction
/// count still increments: the count sits outside the `automatic_checkpoint`
/// gate, deliberately, the same way it already sits outside
/// `memory_extraction`.
#[test]
fn a_precompact_hook_with_automatic_checkpoints_disabled_refreshes_nothing_but_still_counts() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let id = {
        let sessions = fixture.sessions();
        let store = sessions.store();
        store.create(NewSession::embedded("codex")).unwrap().id
    };
    store_checkpoint(&fixture, &id, 1_700_000_000);
    let before = latest_checkpoint(&fixture, &id).created_at;

    let mut user = UserConfig::load(fixture.runtime.paths())
        .expect("a fresh fixture has no config file yet, which loads as the default");
    user.set_automatic_checkpoint(Some(false));
    user.save(fixture.runtime.paths())
        .expect("the user config layer must be writable in the fixture's own tempdir");

    fixture.hook(&id, "PreCompact");

    assert_eq!(
        latest_checkpoint(&fixture, &id).created_at,
        before,
        "automatic_checkpoint=false must leave an existing checkpoint untouched"
    );
    assert_eq!(
        fixture.observed_compactions(&id),
        Some(1),
        "the compaction count is gated independently and must still increment"
    );
}

/// Write, read, and read again through a connection opened from nothing:
/// the count is durable, not a property of one open store.
#[test]
fn the_compaction_count_survives_a_write_a_read_and_a_reopen() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let id = {
        let sessions = fixture.sessions();
        let store = sessions.store();
        let record = store.create(NewSession::embedded("codex")).unwrap();
        for _ in 0..3 {
            store.record_observed_compaction(&record.id).unwrap();
        }
        assert_eq!(
            store.get(&record.id).unwrap().unwrap().observed_compactions,
            Some(3)
        );
        record.id
    };

    let reopened = ProjectSessions::open(&fixture.runtime).unwrap();
    assert_eq!(
        reopened
            .store()
            .get(&id)
            .unwrap()
            .unwrap()
            .observed_compactions,
        Some(3),
        "the count must be in the database, not in the store that wrote it"
    );
}

// ---------------------------------------------------------------------------
// Line 1160 — the most recent request or turn time
// ---------------------------------------------------------------------------

/// **Line 1160 closes on `last_activity_at`, and this is why no second column
/// was added.**
///
/// The line names two things: a request, and a turn. `UserPromptSubmit` is
/// the first and `Stop` is the second, and each moves the existing column on
/// its own, through the single `UPDATE` that moves a session's lifecycle. A
/// duplicate timestamp meaning almost the same thing would be a defect, not a
/// closure.
///
/// # Why the session is created an hour in the past
///
/// The database stores whole seconds and the hook runs in another process on
/// the real clock, so a session created *now* and hooked *now* would satisfy
/// a `>=` assertion whether or not anything was stamped — which is practice
/// §41's weak mutation wearing a test's clothes. Creating it an hour back
/// makes every assertion here strictly greater, and the stamp is wound back
/// between the two events so that each is shown to move it **alone**.
#[test]
fn a_request_and_a_turn_ending_each_move_the_existing_activity_stamp() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let conn = fixture.db();

    let an_hour_ago = system_now() - 60 * 60;
    let id = {
        let store = store_at(&conn, an_hour_ago);
        store
            .create(NewSession::embedded("claude-code"))
            .unwrap()
            .id
    };
    let created_at = store_at(&conn, an_hour_ago)
        .get(&id)
        .unwrap()
        .unwrap()
        .created_at;
    assert_eq!(
        created_at, an_hour_ago,
        "the fixture's clock must have been used"
    );

    // A request.
    fixture.hook(&id, "UserPromptSubmit");
    let after_request = store_at(&conn, an_hour_ago)
        .context(&id)
        .unwrap()
        .expect("the session exists")
        .last_activity_at;
    assert!(
        after_request > created_at,
        "a prompt is a request, and line 1160 asks for its time: {after_request} vs {created_at}"
    );

    // Wind the stamp back, so that what follows can only have been moved by
    // the turn ending.
    conn.execute(
        "UPDATE sessions SET last_activity_at = ?2 WHERE id = ?1",
        rusqlite::params![id.as_str(), an_hour_ago],
    )
    .unwrap();

    fixture.hook(&id, "Stop");
    let after_turn = store_at(&conn, an_hour_ago)
        .context(&id)
        .unwrap()
        .unwrap()
        .last_activity_at;
    assert!(
        after_turn > an_hour_ago,
        "a turn ending must move the stamp on its own: {after_turn} vs {an_hour_ago}"
    );

    assert_eq!(
        after_turn,
        store_at(&conn, an_hour_ago)
            .get(&id)
            .unwrap()
            .unwrap()
            .last_activity_at,
        "line 1160's answer is `sessions.last_activity_at` itself and not a copy \
         of it that could drift"
    );
}

// ---------------------------------------------------------------------------
// Lines 1161, 1162, 1163 — the advisory prompt-cache estimate
// ---------------------------------------------------------------------------

/// **Line 1162** — all four states are reachable, and `Unknown` is not one of
/// the three times.
#[test]
fn every_prompt_cache_state_the_map_requires_is_reachable() {
    let now = 1_700_000_000;
    assert_eq!(
        AdvisoryCacheState::estimate(now, now).state(),
        CacheState::Hot
    );
    assert_eq!(
        AdvisoryCacheState::estimate(now, now - 30 * 60).state(),
        CacheState::Warm
    );
    assert_eq!(
        AdvisoryCacheState::estimate(now, now - 2 * 24 * 60 * 60).state(),
        CacheState::Cold
    );
    // A clock that stepped backwards. Not clamped to zero, because reporting
    // `Hot` because the clock moved is the one answer this type is least
    // entitled to give.
    assert_eq!(
        AdvisoryCacheState::estimate(now, now + 60).state(),
        CacheState::Unknown
    );
    assert_eq!(
        AdvisoryCacheState::unknown().state(),
        CacheState::Unknown,
        "and an estimate that declines to guess must be expressible without a clock"
    );
}

/// **Line 1163** — the estimate carries the word "estimated" wherever it is
/// rendered, on top of being a type nothing outside the store can construct
/// from an authority it claims to have.
#[test]
fn a_cache_estimate_says_it_is_an_estimate_when_it_is_printed() {
    let now = 1_700_000_000;
    assert_eq!(
        AdvisoryCacheState::estimate(now, now).to_string(),
        "hot (estimated)"
    );
    assert_eq!(
        AdvisoryCacheState::unknown().to_string(),
        "unknown (estimated)"
    );
}

/// **Line 1161 — independence from resumability, proven by making the two
/// answers disagree in both directions.**
///
/// A **closed** session with no native identifier cannot be resumed at all,
/// and a **stopped** one with an identifier can. If the estimate were derived
/// from resumability, the first would never be `Hot` and the second would
/// never be `Cold`. Both happen here, in the same database, at the same
/// instant.
#[test]
fn a_prompt_cache_estimate_is_independent_of_whether_a_session_can_be_resumed() {
    use glasshouse::session::SessionDisposition;

    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let conn = fixture.db();

    let now = 1_700_000_000;
    let a_day = 24 * 60 * 60;

    // Not resumable, and active this second.
    let unresumable = {
        let store = store_at(&conn, now);
        let record = store.create(NewSession::embedded("claude-code")).unwrap();
        store
            .set_lifecycle(&record.id, SessionLifecycle::Stopped)
            .unwrap();
        store.close(&record.id).unwrap();
        record.id
    };

    // Resumable, and idle since yesterday.
    let resumable = {
        let store = store_at(&conn, now - a_day);
        let record = store
            .create(
                NewSession::embedded("claude-code")
                    .with_native_session_id(Some("native-resumable".to_owned())),
            )
            .unwrap();
        store
            .set_lifecycle(&record.id, SessionLifecycle::Stopped)
            .unwrap();
        record.id
    };

    let store = store_at(&conn, now);
    let unresumable_record = store.get(&unresumable).unwrap().unwrap();
    let resumable_record = store.get(&resumable).unwrap().unwrap();
    assert_eq!(
        unresumable_record.disposition(),
        SessionDisposition::Closed,
        "the first session must genuinely not be resumable"
    );
    assert_eq!(
        resumable_record.disposition(),
        SessionDisposition::Resumable,
        "and the second must genuinely be"
    );

    assert_eq!(
        store
            .context(&unresumable)
            .unwrap()
            .unwrap()
            .prompt_cache
            .state(),
        CacheState::Hot,
        "a session that cannot be resumed can still have a warm provider cache"
    );
    assert_eq!(
        store
            .context(&resumable)
            .unwrap()
            .unwrap()
            .prompt_cache
            .state(),
        CacheState::Cold,
        "and one that can be resumed can have none at all"
    );
}

// ---------------------------------------------------------------------------
// Line 1164 — a recent portable checkpoint
// ---------------------------------------------------------------------------

/// **Line 1164**, with "recent" measured against the session rather than
/// against a threshold nobody could defend.
///
/// Three states, all reached through the real checkpoint store: no checkpoint
/// at all; one written after the session's last activity; and one the session
/// has since worked past.
#[test]
fn a_checkpoint_written_after_the_last_activity_is_current_and_an_older_one_is_not() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let conn = fixture.db();

    let now = 1_700_000_000;
    let id = {
        let store = store_at(&conn, now);
        store.create(NewSession::embedded("codex")).unwrap().id
    };

    assert_eq!(
        store_at(&conn, now)
            .context(&id)
            .unwrap()
            .unwrap()
            .checkpoint,
        CheckpointRecency::Never,
        "a session with no checkpoint has none, which is not the same as a stale one"
    );

    // A checkpoint stored a minute after the session's last activity.
    store_checkpoint(&fixture, &id, now + 60);
    assert_eq!(
        store_at(&conn, now)
            .context(&id)
            .unwrap()
            .unwrap()
            .checkpoint,
        CheckpointRecency::Current(now + 60),
        "nothing has happened in the session since it was written"
    );

    // The session then does recorded work, which overtakes the checkpoint.
    store_at(&conn, now + 600)
        .set_lifecycle(&id, SessionLifecycle::Running)
        .unwrap();
    assert_eq!(
        store_at(&conn, now + 600)
            .context(&id)
            .unwrap()
            .unwrap()
            .checkpoint,
        CheckpointRecency::Stale(now + 60),
        "a checkpoint the session has worked past no longer describes where it is"
    );
    assert!(
        !store_at(&conn, now + 600)
            .context(&id)
            .unwrap()
            .unwrap()
            .checkpoint
            .is_current()
    );

    // A newer checkpoint restores the answer, and the newest is the one read.
    store_checkpoint(&fixture, &id, now + 900);
    assert_eq!(
        store_at(&conn, now + 900)
            .context(&id)
            .unwrap()
            .unwrap()
            .checkpoint,
        CheckpointRecency::Current(now + 900)
    );
}

/// The tie, which both columns being whole seconds makes reachable in
/// ordinary use: a checkpoint written in the same second as the session's
/// last activity counts as current.
///
/// Asserted rather than left to the doc comment, because the alternative
/// rule — strictly newer — is one character away and would tell a user their
/// checkpoint was stale on the strength of a rounding boundary.
#[test]
fn a_checkpoint_written_in_the_same_second_as_the_last_activity_is_current() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let conn = fixture.db();

    let now = 1_700_000_000;
    let id = {
        let store = store_at(&conn, now);
        store.create(NewSession::embedded("codex")).unwrap().id
    };
    let last_activity = store_at(&conn, now)
        .get(&id)
        .unwrap()
        .unwrap()
        .last_activity_at;

    store_checkpoint(&fixture, &id, last_activity);
    assert_eq!(
        store_at(&conn, now)
            .context(&id)
            .unwrap()
            .unwrap()
            .checkpoint,
        CheckpointRecency::Current(last_activity),
        "within one second the checkpoint is at least as new as the activity, and \
         reporting it stale would cost a user a checkpoint they have"
    );
}

fn store_checkpoint(fixture: &Fixture, session: &SessionId, created_at: i64) {
    store_checkpoint_with_reason(fixture, session, created_at, CheckpointReason::TaskBoundary);
}

fn store_checkpoint_with_reason(
    fixture: &Fixture,
    session: &SessionId,
    created_at: i64,
    reason: CheckpointReason,
) {
    let checkpoints = ProjectCheckpoints::open(&fixture.runtime).unwrap();
    checkpoints
        .store()
        .save(Checkpoint::capture(
            session,
            "codex",
            reason,
            created_at,
            &fixture.root,
            Handoff {
                objective: "prove a checkpoint's recency is readable".to_owned(),
                implementation_state: "written".to_owned(),
                ..Handoff::default()
            },
        ))
        .unwrap();
}

/// The most recent stored checkpoint for `session`, unwrapped: every caller
/// in this file uses it after seeding one, so a missing checkpoint is a test
/// bug, not a case to handle.
fn latest_checkpoint(fixture: &Fixture, session: &SessionId) -> Checkpoint {
    ProjectCheckpoints::open(&fixture.runtime)
        .unwrap()
        .store()
        .latest_for(session)
        .unwrap()
        .expect("a checkpoint must be stored for this session")
        .checkpoint
}

// ---------------------------------------------------------------------------
// Line 1165 — the task-continuity flag
// ---------------------------------------------------------------------------

/// **Line 1165**, and its three states are genuinely three.
///
/// A session nobody has observed is not a session seen doing one thing, and
/// a session that has finished two tasks is not one still inside its first.
/// Every boundary here is written by the shipped binary's own hook handler,
/// which is what makes the count a record of the boundaries Glasshouse acted
/// on rather than a new interpretation of the log.
#[test]
fn task_continuity_separates_nothing_observed_from_one_task_from_several() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let id = {
        let sessions = fixture.sessions();
        sessions
            .store()
            .create(NewSession::embedded("claude-code"))
            .unwrap()
            .id
    };

    let continuity = |fixture: &Fixture| {
        fixture
            .sessions()
            .store()
            .context(&id)
            .unwrap()
            .expect("the session exists")
            .task_continuity
    };

    assert_eq!(
        continuity(&fixture),
        TaskContinuity::Unknown,
        "a session whose harness has reported nothing has told us nothing"
    );

    fixture.hook(&id, "UserPromptSubmit");
    assert_eq!(
        continuity(&fixture),
        TaskContinuity::OneTask,
        "work has been observed and no boundary among it"
    );

    fixture.hook(&id, "Stop");
    assert_eq!(
        continuity(&fixture),
        TaskContinuity::BoundariesCrossed(1),
        "the task the session began is finished"
    );

    fixture.hook(&id, "UserPromptSubmit");
    fixture.hook(&id, "Stop");
    assert_eq!(
        continuity(&fixture),
        TaskContinuity::BoundariesCrossed(2),
        "and a second finished task is a session carrying two tasks' context"
    );
}

// ---------------------------------------------------------------------------
// Line 1158 — the refusal, proven rather than asserted
// ---------------------------------------------------------------------------

/// **Line 1158 is refused, and this is the evidence.**
///
/// The line's own condition is *"when the harness exposes enough
/// information"*. Two channels could carry it and both are empty:
///
/// - the **hook**, which is the only way a harness reports anything, carries
///   an event name and a session identifier and nothing else. Its payload is
///   drained into `io::sink()` unread, so a compaction the binary observes
///   leaves no size behind — proven here by observing one and finding the
///   token columns still empty;
/// - the **gateway**, whose `routing_observations` row is the only place in
///   this schema with token counts. Its own module documentation says they
///   are "not supplied", because reading them means parsing a response body
///   the gateway is forbidden to parse.
///
/// So there is no field for a context size in
/// `glasshouse::session::SessionContext`. An estimator built from message
/// counts would produce a number a future router would read as telemetry,
/// and that is a worse outcome than the absence.
#[test]
fn a_context_size_has_no_producer_in_this_build() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let id = {
        let sessions = fixture.sessions();
        let store = sessions.store();
        let record = store.create(NewSession::embedded("codex")).unwrap();
        store
            .set_lifecycle(&record.id, SessionLifecycle::Running)
            .unwrap();
        record.id
    };

    // A full turn plus a compaction: every harness report this build
    // understands, through the process a harness spawns.
    fixture.hook(&id, "UserPromptSubmit");
    fixture.hook(&id, "PreCompact");
    fixture.hook(&id, "Stop");

    let conn = fixture.db();
    let with_tokens: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM routing_observations \
             WHERE input_tokens IS NOT NULL \
                OR output_tokens IS NOT NULL \
                OR cached_input_tokens IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        with_tokens, 0,
        "nothing in this build writes a token count, so nothing can estimate a \
         context size from one"
    );

    // And the whole of what the harness did reach the database with: an event
    // name, a session, a timestamp. No size, and nowhere to put one.
    let recorded: Vec<(String, Option<String>)> = conn
        .prepare(
            "SELECT kind, observed_event FROM lifecycle_events \
             WHERE session_id = ?1 ORDER BY seq",
        )
        .unwrap()
        .query_map([id.as_str()], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        recorded,
        vec![
            (
                "turn_started".to_owned(),
                Some("UserPromptSubmit".to_owned())
            ),
            ("turn_ended".to_owned(), Some("Stop".to_owned())),
        ],
        "the hook channel carries event names, and the compaction between them \
         carried no row at all"
    );
}

// ---------------------------------------------------------------------------
// Project scope
// ---------------------------------------------------------------------------

/// Every read [`SessionStore::context`] adds names `project_id` beside
/// `session_id`, and two projects on one machine see only their own.
///
/// `tests/project_isolation.rs`'s shape: two real projects sharing a data
/// directory, each with its own canonicalised root.
#[test]
fn a_session_context_never_reaches_across_the_project_boundary() {
    let tmp = tempfile::tempdir().unwrap();
    let alpha = Fixture::new(tmp.path(), "alpha");
    let beta = Fixture::new(tmp.path(), "beta");
    assert_ne!(
        alpha.runtime.database_path(),
        beta.runtime.database_path(),
        "two projects must not share a database"
    );

    let in_alpha = {
        let sessions = alpha.sessions();
        let store = sessions.store();
        let record = store.create(NewSession::embedded("codex")).unwrap();
        store.record_observed_compaction(&record.id).unwrap();
        record.id
    };
    store_checkpoint(&alpha, &in_alpha, 1_700_000_000);
    alpha.hook(&in_alpha, "UserPromptSubmit");
    alpha.hook(&in_alpha, "Stop");

    let context = alpha
        .sessions()
        .store()
        .context(&in_alpha)
        .unwrap()
        .expect("its own project can read it");
    assert_eq!(context.observed_compactions, Some(1));
    assert!(context.checkpoint.stored_at().is_some());
    assert_eq!(
        context.task_continuity,
        TaskContinuity::BoundariesCrossed(1)
    );

    assert_eq!(
        beta.sessions().store().context(&in_alpha).unwrap(),
        None,
        "the other project must not be able to read it at all"
    );

    // And beta's own session sees none of alpha's rows.
    let in_beta = {
        let sessions = beta.sessions();
        sessions
            .store()
            .create(NewSession::embedded("codex"))
            .unwrap()
            .id
    };
    let beta_context = beta.sessions().store().context(&in_beta).unwrap().unwrap();
    assert_eq!(beta_context.observed_compactions, Some(0));
    assert_eq!(beta_context.checkpoint, CheckpointRecency::Never);
    assert_eq!(beta_context.task_continuity, TaskContinuity::Unknown);
}
