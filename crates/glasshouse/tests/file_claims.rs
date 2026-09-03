//! Capability map lines 2392–2398 — Phase 60's A+F slice: soft,
//! project-scoped, turn-scoped file claims.
//!
//! # Everything here drives the shipped binary
//!
//! Practice §35: a caller every test bypasses is not a caller. A claim is
//! taken by running `glasshouse claim`, released by running `glasshouse hook
//! --event Stop` exactly as a harness runs it, and read back through
//! `glasshouse sessions` — the three surfaces a person or a harness actually
//! reaches. The store is opened directly only to assert on the row a surface
//! is not meant to print, and to drive the clock forward for the stale
//! timeout, which no surface can do.
//!
//! # What this file does not prove
//!
//! Nothing here consults a claim before deciding anything, because nothing in
//! this build does: edit-intent detection, conflict prediction and
//! orchestrator notification are the three packages after this one. A claim
//! is metadata, and the tests below assert exactly that — including that a
//! second session claiming the same file succeeds.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use clap::Parser;
use rusqlite::Connection;

use glasshouse::session::store::Clock;
use glasshouse::session::{
    FileClaim, NewSession, ProjectSessions, SessionId, SessionLifecycle, SessionStore,
};
use glasshouse::{Cli, Runtime};

/// Two projects under one data directory, because line 2397 is about two
/// projects and a single-project fixture cannot see it.
struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
    other_root: PathBuf,
    runtime: Runtime,
    other_runtime: Runtime,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();

        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();

        let other_root = base.join("other");
        std::fs::create_dir_all(other_root.join(".git")).unwrap();
        std::fs::create_dir_all(other_root.join("src")).unwrap();
        let other_root = std::fs::canonicalize(&other_root).unwrap();

        let runtime = bootstrap(&base, &root);
        let other_runtime = bootstrap(&base, &other_root);
        Self {
            _tmp: tmp,
            base,
            root,
            other_root,
            runtime,
            other_runtime,
        }
    }

    /// Run the built binary against one of the two projects, from inside that
    /// project's own directory — which is where a person types this.
    fn run(&self, root: &Path, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .current_dir(root)
            .arg("--scope")
            .arg(root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args)
            .output()
            .expect("the glasshouse binary must be runnable")
    }

    fn ok(&self, root: &Path, args: &[&str]) -> String {
        let output = self.run(root, args);
        assert!(
            output.status.success(),
            "`glasshouse {}` failed: {}{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        String::from_utf8(output.stdout).expect("the report is text")
    }

    /// `glasshouse hook`, exactly as a harness runs it: its own process, the
    /// event on argv, a payload on standard input.
    fn hook(&self, session: &SessionId, event: &str) {
        let mut child = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .current_dir(&self.root)
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(["hook", "--session", session.as_str(), "--event", event])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the glasshouse binary must be runnable");
        child
            .stdin
            .as_mut()
            .expect("stdin was piped")
            .write_all(b"{}")
            .expect("the handler must drain its payload rather than closing the pipe");
        let output = child.wait_with_output().expect("the hook must exit");
        assert!(
            output.status.success(),
            "a hook always exits zero: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn claims(&self, runtime: &Runtime) -> Vec<FileClaim> {
        ProjectSessions::open(runtime)
            .unwrap()
            .store()
            .active_claims()
            .unwrap()
    }

    fn db(&self, runtime: &Runtime) -> Connection {
        Connection::open(runtime.database_path()).unwrap()
    }
}

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

/// A live session in one project, which is the only kind that can hold a
/// claim.
fn running_session(runtime: &Runtime) -> SessionId {
    let sessions = ProjectSessions::open(runtime).unwrap();
    let store = sessions.store();
    let record = store.create(NewSession::embedded("claude-code")).unwrap();
    store
        .set_lifecycle(&record.id, SessionLifecycle::Running)
        .unwrap();
    record.id
}

/// The number of `file_claims` rows actually in a project's table, whatever a
/// read would choose to report.
fn stored_rows(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM file_claims", [], |row| row.get(0))
        .unwrap()
}

// ---------------------------------------------------------------------------
// 2392 — a session claims a file.
// ---------------------------------------------------------------------------

#[test]
fn claiming_a_file_records_it_against_the_session_that_asked() {
    let fixture = Fixture::new();
    let id = running_session(&fixture.runtime);

    let report = fixture.ok(
        &fixture.root,
        &["claim", "src/main.rs", "--session", id.as_str()],
    );
    assert!(
        report.contains("src/main.rs"),
        "the verb must say what it claimed: {report}"
    );

    let claims = fixture.claims(&fixture.runtime);
    assert_eq!(claims.len(), 1, "{claims:?}");
    assert_eq!(claims[0].path, "src/main.rs");
    assert_eq!(claims[0].session_id, id);
}

/// The path is stored in `memory_files.path`'s spelling, whatever spelling
/// was typed: claims are compared as strings, so two spellings of one file
/// would be two claims and the overlap a later package looks for would be
/// missed in silence.
#[test]
fn a_claimed_path_is_stored_repo_relative_however_it_was_typed() {
    let fixture = Fixture::new();
    let id = running_session(&fixture.runtime);
    let absolute = fixture.root.join("src").join("main.rs");

    fixture.ok(
        &fixture.root,
        &["claim", "./src/main.rs", "--session", id.as_str()],
    );
    fixture.ok(
        &fixture.root,
        &[
            "claim",
            absolute.to_str().unwrap(),
            "--session",
            id.as_str(),
        ],
    );

    let claims = fixture.claims(&fixture.runtime);
    assert_eq!(
        claims.len(),
        1,
        "`./src/main.rs` and the absolute path are one file and must be one claim: {claims:?}"
    );
    assert_eq!(claims[0].path, "src/main.rs");
}

/// A claim is project-scoped in the plainest sense: there is no path outside
/// the project that could be recorded at all.
#[test]
fn a_path_outside_this_project_cannot_be_claimed() {
    let fixture = Fixture::new();
    let id = running_session(&fixture.runtime);
    let outside = fixture.other_root.join("src").join("main.rs");

    let refused = fixture.run(
        &fixture.root,
        &["claim", outside.to_str().unwrap(), "--session", id.as_str()],
    );
    assert!(
        !refused.status.success(),
        "a path in another project must be refused, not recorded"
    );
    assert!(fixture.claims(&fixture.runtime).is_empty());
}

// ---------------------------------------------------------------------------
// 2393 — the turn ends, the claim goes. This is the package's decision.
// ---------------------------------------------------------------------------

/// Line 2393, through the real hook process.
///
/// `Stop` and `StopFailure` both release: `TurnOutcome` is the harness's
/// verdict on its own turn, and a turn that ended badly is a turn that
/// finished. A claim outliving it would describe work nobody is doing.
#[test]
fn a_turn_ending_releases_every_claim_that_session_held() {
    for event in ["Stop", "StopFailure"] {
        let fixture = Fixture::new();
        let id = running_session(&fixture.runtime);

        fixture.ok(
            &fixture.root,
            &["claim", "src/main.rs", "--session", id.as_str()],
        );
        fixture.ok(
            &fixture.root,
            &["claim", "src/other.rs", "--session", id.as_str()],
        );
        assert_eq!(fixture.claims(&fixture.runtime).len(), 2);

        fixture.hook(&id, event);

        assert!(
            fixture.claims(&fixture.runtime).is_empty(),
            "`{event}` ends the turn, so the claims must be gone"
        );
        assert_eq!(
            stored_rows(&fixture.db(&fixture.runtime)),
            0,
            "`{event}` must delete the rows, not merely stop reporting them"
        );
    }
}

/// The discriminating half: a turn *starting* is not a turn ending, and a
/// release that ran on any event at all would be a release on a schedule.
#[test]
fn an_event_that_is_not_a_turn_ending_releases_nothing() {
    let fixture = Fixture::new();
    let id = running_session(&fixture.runtime);
    fixture.ok(
        &fixture.root,
        &["claim", "src/main.rs", "--session", id.as_str()],
    );

    fixture.hook(&id, "UserPromptSubmit");

    assert_eq!(
        fixture.claims(&fixture.runtime).len(),
        1,
        "a turn starting must leave the claim alone"
    );
}

/// Another session's turn ending is not this session's turn ending.
#[test]
fn a_turn_ending_releases_only_the_session_whose_turn_it_was() {
    let fixture = Fixture::new();
    let mine = running_session(&fixture.runtime);
    let theirs = running_session(&fixture.runtime);

    fixture.ok(
        &fixture.root,
        &["claim", "src/main.rs", "--session", mine.as_str()],
    );
    fixture.ok(
        &fixture.root,
        &["claim", "src/main.rs", "--session", theirs.as_str()],
    );

    fixture.hook(&mine, "Stop");

    let claims = fixture.claims(&fixture.runtime);
    assert_eq!(claims.len(), 1, "{claims:?}");
    assert_eq!(claims[0].session_id, theirs);
}

// ---------------------------------------------------------------------------
// 2394 — abandoned claims.
// ---------------------------------------------------------------------------

/// A session that stopped without its hook arriving — the machine went away,
/// or the harness was killed hard. The claim is neither reported nor kept.
///
/// Two halves, and the second is the one a reader should not take on trust:
/// the read stops reporting it immediately, and the row itself goes when the
/// next claim is written to this project, which is the production sweep and
/// not a test-only entry point.
#[test]
fn a_claim_whose_session_is_no_longer_live_is_released() {
    for ending in [SessionLifecycle::Stopped, SessionLifecycle::Failed] {
        let fixture = Fixture::new();
        let id = running_session(&fixture.runtime);
        let other = running_session(&fixture.runtime);
        fixture.ok(
            &fixture.root,
            &["claim", "src/main.rs", "--session", id.as_str()],
        );

        {
            let sessions = ProjectSessions::open(&fixture.runtime).unwrap();
            sessions.store().set_lifecycle(&id, ending).unwrap();
        }

        assert!(
            fixture.claims(&fixture.runtime).is_empty(),
            "a {ending} session's claim must not be reported"
        );

        fixture.ok(
            &fixture.root,
            &["claim", "src/other.rs", "--session", other.as_str()],
        );
        let rows: Vec<String> = fixture
            .db(&fixture.runtime)
            .prepare("SELECT path FROM file_claims")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            rows,
            vec!["src/other.rs".to_owned()],
            "the {ending} session's row must be gone, not merely unreported"
        );
    }
}

/// The backstop for the case both other releases miss: nothing ever came
/// back to say the session was gone.
#[test]
fn a_claim_older_than_the_stale_timeout_is_released() {
    let fixture = Fixture::new();
    let id = running_session(&fixture.runtime);
    fixture.ok(
        &fixture.root,
        &["claim", "src/main.rs", "--session", id.as_str()],
    );
    assert_eq!(fixture.claims(&fixture.runtime).len(), 1);

    let later: Clock = {
        let at = now() + glasshouse::session::STALE_CLAIM_AFTER + 1;
        Arc::new(move || at)
    };
    let sessions = ProjectSessions::open_with_clock(&fixture.runtime, later).unwrap();
    let store = sessions.store();
    assert!(
        store.active_claims().unwrap().is_empty(),
        "a claim past `STALE_CLAIM_AFTER` must not be reported"
    );

    // And the row goes with the next claim written to this project — the
    // production sweep, not a test-only entry point.
    store.claim_file(&id, "src/other.rs").unwrap();
    let rows: Vec<String> = fixture
        .db(&fixture.runtime)
        .prepare("SELECT path FROM file_claims")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        rows,
        vec!["src/other.rs".to_owned()],
        "the stale row must be gone, not merely unreported"
    );
}

/// The same session is still live and still working: the claim stands, and a
/// timeout that expired it early would be a claim that stopped describing
/// real work while the work went on.
#[test]
fn a_claim_inside_the_stale_timeout_stands() {
    let fixture = Fixture::new();
    let id = running_session(&fixture.runtime);
    fixture.ok(
        &fixture.root,
        &["claim", "src/main.rs", "--session", id.as_str()],
    );

    let nearly: Clock = {
        let at = now() + glasshouse::session::STALE_CLAIM_AFTER - 60;
        Arc::new(move || at)
    };
    let sessions = ProjectSessions::open_with_clock(&fixture.runtime, nearly).unwrap();
    assert_eq!(sessions.store().active_claims().unwrap().len(), 1);
}

// ---------------------------------------------------------------------------
// 2395 — renew.
// ---------------------------------------------------------------------------

/// Line 2395: the next turn continues work on the same file. One row moves;
/// no second row appears, and `claimed_at` still says when the work started.
#[test]
fn claiming_a_file_the_session_already_holds_renews_it() {
    let fixture = Fixture::new();
    let id = running_session(&fixture.runtime);

    fixture.ok(
        &fixture.root,
        &["claim", "src/main.rs", "--session", id.as_str()],
    );
    let first = fixture.claims(&fixture.runtime)[0].clone();

    // A clock two minutes on, so the renew's own stamps are distinguishable
    // from the original's without sleeping.
    let later_at = first.claimed_at + 120;
    {
        let later: Clock = Arc::new(move || later_at);
        let sessions = ProjectSessions::open_with_clock(&fixture.runtime, later).unwrap();
        sessions.store().claim_file(&id, "src/main.rs").unwrap();
    }

    let claims = fixture.claims(&fixture.runtime);
    assert_eq!(claims.len(), 1, "a renew must not create a second claim");
    assert_eq!(
        claims[0].claimed_at, first.claimed_at,
        "a renew does not change when the work started"
    );
    assert_eq!(claims[0].renewed_at, later_at);
    assert_eq!(
        claims[0].expires_at,
        later_at + glasshouse::session::STALE_CLAIM_AFTER,
        "a renew extends the claim"
    );
}

// ---------------------------------------------------------------------------
// 2396 — the owner is a Glasshouse session, never a process.
// ---------------------------------------------------------------------------

/// Line 2396. The stored owner is the Glasshouse session identifier, and
/// there is no process identifier in the table to be recycled into a live
/// claim.
#[test]
fn a_claim_is_owned_by_a_glasshouse_session_and_no_process_identifier() {
    let fixture = Fixture::new();
    let id = running_session(&fixture.runtime);
    fixture.ok(
        &fixture.root,
        &["claim", "src/main.rs", "--session", id.as_str()],
    );

    let conn = fixture.db(&fixture.runtime);
    let owner: String = conn
        .query_row("SELECT session_id FROM file_claims", [], |row| row.get(0))
        .unwrap();
    assert_eq!(owner, id.as_str());

    let columns: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('file_claims')")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(
        !columns
            .iter()
            .any(|name| name.contains("process") || name == "pid" || name.ends_with("_pid")),
        "a claim must carry no process identity: {columns:?}"
    );
}

/// A session this project does not have cannot hold a claim, so nothing can
/// be recorded against an identifier nobody minted.
#[test]
fn a_session_this_project_does_not_have_cannot_claim() {
    let fixture = Fixture::new();
    let _mine = running_session(&fixture.runtime);
    let theirs = running_session(&fixture.other_runtime);

    let refused = fixture.run(
        &fixture.root,
        &["claim", "src/main.rs", "--session", theirs.as_str()],
    );
    assert!(
        !refused.status.success(),
        "another project's session is not a session here"
    );
    assert_eq!(stored_rows(&fixture.db(&fixture.runtime)), 0);
}

// ---------------------------------------------------------------------------
// 2397 — project scope. The security invariant.
// ---------------------------------------------------------------------------

/// Line 2397, from the outside: a claim taken in project A is not in project
/// B's listing, its table, or anything B's binary prints.
#[test]
fn a_claim_in_one_project_is_invisible_to_another() {
    let fixture = Fixture::new();
    let mine = running_session(&fixture.runtime);
    let _theirs = running_session(&fixture.other_runtime);

    fixture.ok(
        &fixture.root,
        &["claim", "src/main.rs", "--session", mine.as_str()],
    );

    assert!(
        fixture.claims(&fixture.other_runtime).is_empty(),
        "project B must see none of project A's claims"
    );
    assert_eq!(stored_rows(&fixture.db(&fixture.other_runtime)), 0);

    let listing = fixture.ok(&fixture.other_root, &["claim", "--list"]);
    assert!(
        !listing.contains("src/main.rs"),
        "project B's listing named project A's claim: {listing}"
    );
    let overview = fixture.ok(&fixture.other_root, &["sessions"]);
    assert!(
        !overview.contains("CLAIMED BY"),
        "project B's overview named project A's claim: {overview}"
    );
}

/// The schema's own half of line 2397: a row carrying another project's
/// identifier is refused before it exists, so no reader has to remember to
/// filter.
#[test]
fn the_database_refuses_a_claim_belonging_to_another_project() {
    let fixture = Fixture::new();
    let id = running_session(&fixture.runtime);
    let conn = fixture.db(&fixture.runtime);

    let refused = conn.execute(
        "INSERT INTO file_claims \
         (project_id, session_id, path, claimed_at, renewed_at, expires_at) \
         VALUES ('another-project', ?1, 'src/main.rs', 1, 1, 9999999999)",
        [id.as_str()],
    );
    let err = refused.expect_err("the trigger must abort a foreign project's claim");
    assert!(
        err.to_string().contains("different project"),
        "unexpected refusal: {err}"
    );
    assert_eq!(stored_rows(&conn), 0);
}

/// And the query's half, which the trigger hides: a foreign row that reached
/// the table anyway — a build whose triggers differed, a restored backup — is
/// still not something a read will report.
///
/// The triggers are dropped for exactly as long as it takes to write the row,
/// and the row names a **live session of this project**, so nothing but the
/// `project_id` predicate can be what excludes it.
#[test]
fn a_read_never_reports_a_row_belonging_to_another_project() {
    let fixture = Fixture::new();
    let id = running_session(&fixture.runtime);

    {
        let conn = fixture.db(&fixture.runtime);
        conn.execute_batch(
            "DROP TRIGGER file_claims_reject_foreign_project_insert;
             DROP TRIGGER file_claims_reject_foreign_project_update;",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO file_claims \
             (project_id, session_id, path, claimed_at, renewed_at, expires_at) \
             VALUES ('another-project', ?1, 'src/smuggled.rs', 1, 1, 9999999999)",
            [id.as_str()],
        )
        .unwrap();
        assert_eq!(stored_rows(&conn), 1, "the row really is in the table");
    }

    let claims = fixture.claims(&fixture.runtime);
    assert!(
        claims.is_empty(),
        "a row carrying another project's identifier was reported: {claims:?}"
    );
    let listing = fixture.ok(&fixture.root, &["claim", "--list"]);
    assert!(
        !listing.contains("smuggled"),
        "the listing reported a foreign row: {listing}"
    );
}

// ---------------------------------------------------------------------------
// 2398 — the overview.
// ---------------------------------------------------------------------------

/// Line 2398. Two sessions on one file stand next to each other, which is the
/// adjacency that makes a claim useful to parallel work. It is not a warning
/// and it is not a verdict: this package predicts nothing.
#[test]
fn the_session_overview_shows_active_claims() {
    let fixture = Fixture::new();
    let mine = running_session(&fixture.runtime);
    let theirs = running_session(&fixture.runtime);

    fixture.ok(
        &fixture.root,
        &["claim", "src/main.rs", "--session", mine.as_str()],
    );
    fixture.ok(
        &fixture.root,
        &["claim", "src/main.rs", "--session", theirs.as_str()],
    );

    let overview = fixture.ok(&fixture.root, &["sessions"]);
    assert!(overview.contains("CLAIMED BY"), "{overview}");
    assert!(overview.contains("src/main.rs"), "{overview}");
    let short = |id: &SessionId| id.as_str().chars().take(12).collect::<String>();
    assert!(overview.contains(&short(&mine)), "{overview}");
    assert!(overview.contains(&short(&theirs)), "{overview}");

    let claimed: Vec<&str> = overview
        .lines()
        .skip_while(|line| !line.starts_with("CLAIMED BY"))
        .skip(1)
        .collect();
    assert_eq!(claimed.len(), 2, "{overview}");
}

/// *"When nothing is claimed, print nothing"* — a project that does not use
/// claims sees the listing it always saw.
#[test]
fn the_session_overview_says_nothing_when_nothing_is_claimed() {
    let fixture = Fixture::new();
    let id = running_session(&fixture.runtime);

    let overview = fixture.ok(&fixture.root, &["sessions"]);
    assert!(!overview.contains("CLAIMED BY"), "{overview}");

    // And after a claim is taken and released again, back to nothing.
    fixture.ok(
        &fixture.root,
        &["claim", "src/main.rs", "--session", id.as_str()],
    );
    fixture.ok(
        &fixture.root,
        &[
            "claim",
            "src/main.rs",
            "--session",
            id.as_str(),
            "--release",
        ],
    );
    let after = fixture.ok(&fixture.root, &["sessions"]);
    assert!(!after.contains("CLAIMED BY"), "{after}");
}

// ---------------------------------------------------------------------------
// The scoping rule the whole slice rests on: a claim is soft.
// ---------------------------------------------------------------------------

/// Two sessions, one file, both claims recorded. This is the overlap a later
/// package reports; it is not an error, not a lock, and not a refusal.
#[test]
fn two_sessions_may_claim_one_file_and_neither_is_refused() {
    let fixture = Fixture::new();
    let first = running_session(&fixture.runtime);
    let second = running_session(&fixture.runtime);

    fixture.ok(
        &fixture.root,
        &["claim", "src/main.rs", "--session", first.as_str()],
    );
    fixture.ok(
        &fixture.root,
        &["claim", "src/main.rs", "--session", second.as_str()],
    );

    let claims = fixture.claims(&fixture.runtime);
    assert_eq!(claims.len(), 2, "{claims:?}");
    assert_eq!(claims[0].path, claims[1].path);
    assert_ne!(claims[0].session_id, claims[1].session_id);
}

/// Releasing one path leaves the session's other claims alone, and releasing
/// with no path releases all of them.
#[test]
fn releasing_names_what_it_releases() {
    let fixture = Fixture::new();
    let id = running_session(&fixture.runtime);
    for path in ["src/main.rs", "src/other.rs"] {
        fixture.ok(&fixture.root, &["claim", path, "--session", id.as_str()]);
    }

    fixture.ok(
        &fixture.root,
        &[
            "claim",
            "src/main.rs",
            "--session",
            id.as_str(),
            "--release",
        ],
    );
    let claims = fixture.claims(&fixture.runtime);
    assert_eq!(claims.len(), 1, "{claims:?}");
    assert_eq!(claims[0].path, "src/other.rs");

    fixture.ok(
        &fixture.root,
        &["claim", "--session", id.as_str(), "--release"],
    );
    assert!(fixture.claims(&fixture.runtime).is_empty());
}

/// The store's own API is what the next package calls; the verb is a seam
/// over it. A finished session is refused rather than given a claim that the
/// next read would drop without saying so.
#[test]
fn a_finished_session_cannot_take_a_claim() {
    let fixture = Fixture::new();
    let id = running_session(&fixture.runtime);
    let sessions = ProjectSessions::open(&fixture.runtime).unwrap();
    let store: SessionStore<'_> = sessions.store();
    store.set_lifecycle(&id, SessionLifecycle::Stopped).unwrap();

    let refused = store.claim_file(&id, "src/main.rs");
    assert!(
        refused.is_err(),
        "a session that has finished cannot claim: {refused:?}"
    );
    assert_eq!(stored_rows(&fixture.db(&fixture.runtime)), 0);
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}
