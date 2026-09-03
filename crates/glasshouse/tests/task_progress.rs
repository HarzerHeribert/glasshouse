//! Capability map lines 1294 and 1610 — a task's progress is **declared**,
//! never guessed.
//!
//! # The two lines are one mechanism seen from two phases
//!
//! 1294 is *"avoid moving an almost-complete high-value task to another
//! session solely because a reserve threshold was crossed"*; 1610 is *"avoid
//! migrating a nearly completed task solely to preserve a small amount of
//! quota"*. Both turn on **solely**: the guard stops a threshold being the
//! whole reason work moves. Both are answered by the same field,
//! `provider::quota::ReserveDecisionInputs::task_nearly_complete`, which has
//! two production construction sites and — until this package — no producer.
//!
//! # Everything here drives the shipped binary or the policy itself
//!
//! Practice §35: a caller every test bypasses is not a caller. A declaration
//! is made by running `glasshouse task-progress`, read back through the same
//! verb's `--list`, and its effect on routing is asserted against the
//! policies that actually consume it. The store is opened directly only to
//! drive the clock forward for the horizon, which no surface can do.
//!
//! # What this file does not prove
//!
//! It does not prove that Glasshouse can tell how far through a task a
//! session is. It cannot, and the design says so: this suite's whole subject
//! is a statement somebody made. The one assertion in that direction is
//! negative and lives in `tests/subscription_pressure.rs` — neither
//! construction site writes a literal, so neither can fabricate one.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use clap::Parser;
use rusqlite::Connection;

use glasshouse::provider::quota::{CapacityBand, ReserveDecisionInputs, evaluate_reserve_spend};
use glasshouse::routing::classify::WorkloadTier;
use glasshouse::routing::disposable::DeclaredTaskProgress;
use glasshouse::routing::pressure::reserve_verdict;
use glasshouse::session::store::Clock;
use glasshouse::session::{
    NewSession, ProjectSessions, SessionId, SessionLifecycle, TaskProgressDeclaration,
};
use glasshouse::{Cli, Runtime};

/// Two projects under one data directory, because project isolation is about
/// two projects and a single-project fixture cannot see it.
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
        let root = std::fs::canonicalize(&root).unwrap();

        let other_root = base.join("other");
        std::fs::create_dir_all(other_root.join(".git")).unwrap();
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

    fn declared(&self, runtime: &Runtime) -> Vec<TaskProgressDeclaration> {
        ProjectSessions::open(runtime)
            .unwrap()
            .store()
            .active_task_progress()
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

/// A live session in one project, which is the only kind that can declare.
fn running_session(runtime: &Runtime) -> SessionId {
    let sessions = ProjectSessions::open(runtime).unwrap();
    let store = sessions.store();
    let record = store.create(NewSession::embedded("claude-code")).unwrap();
    store
        .set_lifecycle(&record.id, SessionLifecycle::Running)
        .unwrap();
    record.id
}

fn stored_rows(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM task_progress_declarations",
        [],
        |row| row.get(0),
    )
    .unwrap()
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// The declaration itself: made on purpose, about one named session.
// ---------------------------------------------------------------------------

#[test]
fn declaring_a_task_nearly_complete_records_it_against_the_session_that_asked() {
    let fixture = Fixture::new();
    let id = running_session(&fixture.runtime);

    let report = fixture.ok(&fixture.root, &["task-progress", "--session", id.as_str()]);
    assert!(
        report.contains("nearly complete"),
        "the verb must say what it recorded: {report}"
    );

    let declared = fixture.declared(&fixture.runtime);
    assert_eq!(declared.len(), 1, "{declared:?}");
    assert_eq!(declared[0].session_id, id);
}

/// One row per session: declaring again renews rather than accumulating, and
/// `declared_at` still says when the operator first said so.
#[test]
fn declaring_again_renews_the_declaration_the_session_already_carries() {
    let fixture = Fixture::new();
    let id = running_session(&fixture.runtime);

    fixture.ok(&fixture.root, &["task-progress", "--session", id.as_str()]);
    let first = fixture.declared(&fixture.runtime).remove(0);

    let later: Clock = {
        let at = now() + 120;
        Arc::new(move || at)
    };
    let sessions = ProjectSessions::open_with_clock(&fixture.runtime, later).unwrap();
    sessions.store().declare_task_nearly_complete(&id).unwrap();

    let declared = fixture.declared(&fixture.runtime);
    assert_eq!(declared.len(), 1, "a renew must not add a second row");
    assert_eq!(
        declared[0].declared_at, first.declared_at,
        "a renew must not move `declared_at`"
    );
    assert!(
        declared[0].expires_at > first.expires_at,
        "a renew must extend the horizon: {:?} then {:?}",
        first,
        declared[0]
    );
}

#[test]
fn a_declaration_can_be_withdrawn_before_it_expires() {
    let fixture = Fixture::new();
    let id = running_session(&fixture.runtime);

    fixture.ok(&fixture.root, &["task-progress", "--session", id.as_str()]);
    assert_eq!(fixture.declared(&fixture.runtime).len(), 1);

    let report = fixture.ok(
        &fixture.root,
        &["task-progress", "--session", id.as_str(), "--withdraw"],
    );
    assert!(report.contains("withdrew"), "{report}");
    assert!(fixture.declared(&fixture.runtime).is_empty());
}

#[test]
fn the_listing_shows_a_declaration_and_says_nothing_when_none_was_made() {
    let fixture = Fixture::new();
    let empty = fixture.ok(&fixture.root, &["task-progress", "--list"]);
    assert!(
        empty.contains("No task declared"),
        "an empty project must say so plainly: {empty}"
    );

    let id = running_session(&fixture.runtime);
    fixture.ok(&fixture.root, &["task-progress", "--session", id.as_str()]);
    let listing = fixture.ok(&fixture.root, &["task-progress", "--list"]);
    assert!(listing.contains("EXPIRES IN"), "{listing}");
    assert!(
        listing.contains(&id.as_str()[..8]),
        "the listing must name the session: {listing}"
    );
}

// ---------------------------------------------------------------------------
// Never sticky: the horizon, and the liveness filter.
// ---------------------------------------------------------------------------

/// The declaration expires on its own. A statement that outlived the task it
/// described would keep the reserve policy's first branch firing for work
/// that had already finished — the inversion the design refuses, arriving by
/// the slower route.
#[test]
fn a_declaration_past_its_horizon_is_neither_reported_nor_honoured() {
    let fixture = Fixture::new();
    let id = running_session(&fixture.runtime);
    fixture.ok(&fixture.root, &["task-progress", "--session", id.as_str()]);
    assert_eq!(fixture.declared(&fixture.runtime).len(), 1);

    let later: Clock = {
        let at = now() + glasshouse::session::TASK_PROGRESS_EXPIRES_AFTER + 1;
        Arc::new(move || at)
    };
    let sessions = ProjectSessions::open_with_clock(&fixture.runtime, later).unwrap();
    let store = sessions.store();
    assert!(
        store.active_task_progress().unwrap().is_empty(),
        "a declaration past its horizon must not be reported"
    );
    assert!(
        store
            .sessions_declaring_task_nearly_complete()
            .unwrap()
            .is_empty(),
        "and the routers must not be told about it either"
    );

    // And the row goes with the next declaration written to this project —
    // the production sweep, not a test-only entry point.
    let other = running_session(&fixture.runtime);
    store.declare_task_nearly_complete(&other).unwrap();
    assert_eq!(
        stored_rows(&fixture.db(&fixture.runtime)),
        1,
        "the expired row must be swept by the next write"
    );
}

#[test]
fn a_declaration_inside_its_horizon_stands() {
    let fixture = Fixture::new();
    let id = running_session(&fixture.runtime);
    fixture.ok(&fixture.root, &["task-progress", "--session", id.as_str()]);

    let nearly: Clock = {
        let at = now() + glasshouse::session::TASK_PROGRESS_EXPIRES_AFTER - 60;
        Arc::new(move || at)
    };
    let sessions = ProjectSessions::open_with_clock(&fixture.runtime, nearly).unwrap();
    assert_eq!(sessions.store().active_task_progress().unwrap().len(), 1);
}

#[test]
fn a_declaration_whose_session_is_no_longer_live_is_not_honoured() {
    let fixture = Fixture::new();
    let id = running_session(&fixture.runtime);
    fixture.ok(&fixture.root, &["task-progress", "--session", id.as_str()]);
    assert_eq!(fixture.declared(&fixture.runtime).len(), 1);

    let sessions = ProjectSessions::open(&fixture.runtime).unwrap();
    sessions
        .store()
        .set_lifecycle(&id, SessionLifecycle::Stopped)
        .unwrap();

    assert!(
        fixture.declared(&fixture.runtime).is_empty(),
        "a stopped session's declaration must not be honoured"
    );
}

#[test]
fn a_finished_session_cannot_declare_its_task_nearly_complete() {
    let fixture = Fixture::new();
    let id = running_session(&fixture.runtime);
    let sessions = ProjectSessions::open(&fixture.runtime).unwrap();
    sessions
        .store()
        .set_lifecycle(&id, SessionLifecycle::Stopped)
        .unwrap();

    let output = fixture.run(&fixture.root, &["task-progress", "--session", id.as_str()]);
    assert!(
        !output.status.success(),
        "a finished session must be refused"
    );
    assert_eq!(stored_rows(&fixture.db(&fixture.runtime)), 0);
}

#[test]
fn a_session_this_project_does_not_have_cannot_declare() {
    let fixture = Fixture::new();
    let output = fixture.run(
        &fixture.root,
        &[
            "task-progress",
            "--session",
            "00000000-0000-4000-8000-000000000000",
        ],
    );
    assert!(!output.status.success());
    assert_eq!(stored_rows(&fixture.db(&fixture.runtime)), 0);
}

// ---------------------------------------------------------------------------
// Project isolation.
// ---------------------------------------------------------------------------

#[test]
fn a_declaration_in_one_project_is_invisible_to_another() {
    let fixture = Fixture::new();
    let id = running_session(&fixture.runtime);
    running_session(&fixture.other_runtime);
    fixture.ok(&fixture.root, &["task-progress", "--session", id.as_str()]);

    assert_eq!(fixture.declared(&fixture.runtime).len(), 1);
    assert!(
        fixture.declared(&fixture.other_runtime).is_empty(),
        "the other project must see nothing"
    );
    let listing = fixture.ok(&fixture.other_root, &["task-progress", "--list"]);
    assert!(listing.contains("No task declared"), "{listing}");
}

#[test]
fn the_database_refuses_a_declaration_belonging_to_another_project() {
    let fixture = Fixture::new();
    let id = running_session(&fixture.runtime);
    let conn = fixture.db(&fixture.runtime);
    let refused = conn.execute(
        "INSERT INTO task_progress_declarations \
         (project_id, session_id, declared_at, renewed_at, expires_at) \
         VALUES ('another-project', ?1, 1, 1, 9999999999)",
        [id.as_str()],
    );
    assert!(
        refused.is_err(),
        "the trigger must refuse a foreign project identifier"
    );
    assert_eq!(stored_rows(&conn), 0);
}

/// The predicate behind the trigger, proven one layer at a time. The triggers
/// are dropped for exactly as long as it takes to write the row, and the row
/// names a **live session of this project**, so nothing but the `project_id`
/// predicate can be what excludes it.
#[test]
fn a_read_never_reports_a_row_belonging_to_another_project() {
    let fixture = Fixture::new();
    let id = running_session(&fixture.runtime);

    {
        let conn = fixture.db(&fixture.runtime);
        conn.execute_batch(
            "DROP TRIGGER task_progress_reject_foreign_project_insert;
             DROP TRIGGER task_progress_reject_foreign_project_update;",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO task_progress_declarations \
             (project_id, session_id, declared_at, renewed_at, expires_at) \
             VALUES ('another-project', ?1, 1, 1, 9999999999)",
            [id.as_str()],
        )
        .unwrap();
        assert_eq!(stored_rows(&conn), 1, "the row really is in the table");
    }

    assert!(
        fixture.declared(&fixture.runtime).is_empty(),
        "a row carrying another project's identifier was reported"
    );
    let listing = fixture.ok(&fixture.root, &["task-progress", "--list"]);
    assert!(listing.contains("No task declared"), "{listing}");
}

// ---------------------------------------------------------------------------
// The scope: a declaration is about one session and no other.
// ---------------------------------------------------------------------------

#[test]
fn a_declaration_applies_only_to_the_session_it_names() {
    let declared = DeclaredTaskProgress::for_sessions(["alpha"]);
    assert!(
        !declared.applies(),
        "a caller that named no session can never match"
    );
    assert!(
        !declared.clone().deciding_for("beta").applies(),
        "a session nobody declared must not be protected"
    );
    assert!(declared.deciding_for("alpha").applies());

    assert!(
        !DeclaredTaskProgress::none().deciding_for("alpha").applies(),
        "nothing declared is nothing declared"
    );
    assert_eq!(
        DeclaredTaskProgress::default(),
        DeclaredTaskProgress::none(),
        "the default must be the empty declaration, so its arrival is a no-op"
    );
    assert_eq!(
        DeclaredTaskProgress::for_sessions(["alpha"])
            .deciding_for("alpha")
            .declared_session(),
        Some("alpha"),
        "an applying declaration names whose task it was, for the explanation"
    );
}

// ---------------------------------------------------------------------------
// 1294 and 1610 — the work stays put, and the reason says why.
// ---------------------------------------------------------------------------

/// The inputs are deliberately the ones that deny without a declaration: the
/// reserve band, a cheaper adequate alternative, a light tier and a distant
/// reset. Only the declaration differs between the two calls, so the change
/// in verdict is about the declaration and nothing else.
fn contested(declared: bool) -> ReserveDecisionInputs {
    ReserveDecisionInputs {
        band: CapacityBand::Reserve,
        tier: WorkloadTier::Leaf,
        cheaper_adequate_resource_exists: true,
        user_override: false,
        seconds_until_reset: Some(7_200),
        task_nearly_complete: declared,
    }
}

#[test]
fn a_declared_task_is_not_moved_by_a_crossed_reserve_threshold_alone() {
    let undeclared = evaluate_reserve_spend(contested(false));
    assert!(
        !undeclared.is_allowed(),
        "without a declaration these inputs must deny, or this test proves nothing: {}",
        undeclared.reason()
    );

    let declared = evaluate_reserve_spend(contested(true));
    assert!(
        declared.is_allowed(),
        "a declared task must keep its resource: {}",
        declared.reason()
    );
}

/// Line 1294's own words are *"solely because a reserve threshold was
/// crossed"*, so the reason a caller receives has to name the second reason —
/// the declaration — rather than only the outcome.
#[test]
fn the_reason_names_the_declaration_and_both_lines_it_answers() {
    let reason = evaluate_reserve_spend(contested(true)).reason().to_owned();
    assert!(reason.contains("declared"), "{reason}");
    assert!(reason.contains("1294"), "{reason}");
    assert!(reason.contains("1610"), "{reason}");
    assert!(
        reason.contains("sole"),
        "the reason must say a threshold was not the sole cause: {reason}"
    );
}

/// The declaration outranks every other signal, including the user's own
/// override — it is the policy's first branch, and the design says why.
#[test]
fn the_declaration_is_decided_before_every_other_signal() {
    let mut inputs = contested(true);
    inputs.user_override = false;
    let reason = evaluate_reserve_spend(inputs).reason().to_owned();
    assert!(
        reason.contains("declared") && !reason.contains("1290"),
        "the declaration must be the branch taken, not the override: {reason}"
    );
}

/// The same guard through `routing::pressure`, which is where line 1610 is
/// seen from. Both the established-tier arm and the unknown-tier copy honour
/// it — the unknown-tier arm is a hand-written copy of the precedence, and a
/// copy that dropped the branch would withhold the protection from exactly
/// the tasks whose tier could not be classified.
#[test]
fn the_pressure_verdict_keeps_a_declared_task_at_either_tier() {
    for tier in [None, Some(WorkloadTier::Leaf)] {
        let undeclared =
            reserve_verdict(CapacityBand::Reserve, tier, true, false, Some(7_200), false);
        assert!(
            !undeclared.is_allowed(),
            "tier {tier:?}: without a declaration this must deny: {}",
            undeclared.reason()
        );

        let declared = reserve_verdict(CapacityBand::Reserve, tier, true, false, Some(7_200), true);
        assert!(
            declared.is_allowed(),
            "tier {tier:?}: a declared task must be kept: {}",
            declared.reason()
        );
        assert!(
            declared.reason().contains("declared"),
            "tier {tier:?}: {}",
            declared.reason()
        );
    }
}

// ---------------------------------------------------------------------------
// Nothing declared ⇒ nothing changes.
// ---------------------------------------------------------------------------

/// The default is a no-op, which is what makes this package's arrival
/// invisible to every existing caller and every existing test.
#[test]
fn nothing_declared_decides_exactly_as_this_build_did_before() {
    for band in [
        CapacityBand::Exhausted,
        CapacityBand::Reserve,
        CapacityBand::Tight,
        CapacityBand::Plenty,
    ] {
        for tier in [
            WorkloadTier::Deterministic,
            WorkloadTier::Leaf,
            WorkloadTier::Heavy,
        ] {
            for cheaper in [false, true] {
                for user_override in [false, true] {
                    for reset in [None, Some(0), Some(1_800), Some(7_200)] {
                        let inputs = ReserveDecisionInputs {
                            band,
                            tier,
                            cheaper_adequate_resource_exists: cheaper,
                            user_override,
                            seconds_until_reset: reset,
                            task_nearly_complete: false,
                        };
                        let undeclared = evaluate_reserve_spend(inputs);
                        // The undeclared verdict never mentions a
                        // declaration: the branch is not merely inert, it is
                        // not taken.
                        assert!(
                            !undeclared.reason().contains("declared"),
                            "band {band}, tier {tier:?}, cheaper {cheaper}, override \
                             {user_override}, reset {reset:?}: an undeclared task was decided \
                             on a declaration: {}",
                            undeclared.reason()
                        );
                    }
                }
            }
        }
    }
}

/// A project where nobody ever ran the verb hands the routers an empty set,
/// which is `applies() == false` for every session — the production reading
/// of "byte-identical behaviour when nothing is declared".
#[test]
fn a_project_that_declared_nothing_hands_the_routers_an_empty_set() {
    let fixture = Fixture::new();
    let id = running_session(&fixture.runtime);
    let sessions = ProjectSessions::open(&fixture.runtime).unwrap();

    let declared = sessions
        .store()
        .sessions_declaring_task_nearly_complete()
        .unwrap();
    assert!(declared.is_empty());
    assert!(
        !DeclaredTaskProgress::for_sessions(declared)
            .deciding_for(id.as_str())
            .applies(),
        "an empty set must not protect the session being decided for"
    );
}

/// And once the verb has run, the same path hands the routers exactly that
/// session — the link between the producer and the two construction sites,
/// asserted end to end rather than assumed.
#[test]
fn the_declared_set_reaches_the_routers_scoped_to_the_session_that_declared() {
    let fixture = Fixture::new();
    let declaring = running_session(&fixture.runtime);
    let other = running_session(&fixture.runtime);
    fixture.ok(
        &fixture.root,
        &["task-progress", "--session", declaring.as_str()],
    );

    let declared = ProjectSessions::open(&fixture.runtime)
        .unwrap()
        .store()
        .sessions_declaring_task_nearly_complete()
        .unwrap();
    assert_eq!(declared.len(), 1, "{declared:?}");

    let progress = DeclaredTaskProgress::for_sessions(declared);
    assert!(
        progress.clone().deciding_for(declaring.as_str()).applies(),
        "the session that declared must be protected"
    );
    assert!(
        !progress.deciding_for(other.as_str()).applies(),
        "a session in the same project that declared nothing must not be"
    );
}
