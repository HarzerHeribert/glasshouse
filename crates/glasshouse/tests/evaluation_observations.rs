//! Phase 51 — `evaluation_observations`, migration 15, and the three map
//! lines it closes.
//!
//! - **1822** *"Measure how often stale or incorrect memory is retrieved."*
//! - **1826** *"Measure how often superseded memories are incorrectly
//!   resurfaced as current guidance."*
//! - **1856** *"Keep evaluation data local and project-scoped unless the user
//!   explicitly exports it."*
//!
//! The 1822/1826 evidence goes **through the shipped binary**, not through the
//! writer function. Practice §35: a caller every test bypasses is not a
//! caller, and the whole claim of these two lines is that the retrieval path
//! Glasshouse actually runs now leaves a trace. Asserting that
//! `EvaluationObservations::record` inserts a row proves nothing about that.

use std::path::{Path, PathBuf};
use std::process::Command;

use clap::Parser;
use rusqlite::Connection;

use glasshouse::evaluation::{
    EvaluationKind, EvaluationObservations, EvaluationOutcome, NewObservation, Retention,
    RetrievalScope,
};
use glasshouse::memory::{MemoryKind, MemoryStatus, NewMemory, ProjectMemory, ReviewReason};
use glasshouse::{Cli, Runtime, bootstrap};

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// A bootstrapped project inside `base`, sharing `base`'s data and config
/// roots — the shape `tests/memory_project_scope.rs` uses, so two fixtures
/// over one `base` are two real projects on one machine, each with its own
/// canonicalised root and its own `glasshouse.db`.
struct Fixture {
    base: PathBuf,
    runtime: Runtime,
}

impl Fixture {
    fn new(base: &Path, name: &str) -> Self {
        let root: PathBuf = base.join("workspace").join(name);
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
        let runtime = bootstrap(&cli, &root).unwrap();
        Fixture {
            base: base.to_path_buf(),
            runtime,
        }
    }

    fn memory(&self) -> ProjectMemory {
        ProjectMemory::open(&self.runtime).unwrap()
    }

    fn ledger(&self) -> EvaluationObservations {
        EvaluationObservations::open(&self.runtime).unwrap()
    }

    fn ledger_with(&self, retention: Retention) -> EvaluationObservations {
        EvaluationObservations::open_with_retention(&self.runtime, retention).unwrap()
    }

    fn db(&self) -> Connection {
        Connection::open(self.runtime.database_path()).unwrap()
    }

    /// Run the shipped binary in this project, exactly as a user would.
    fn run(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .current_dir(self.runtime.project().root())
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args)
            .output()
            .expect("the glasshouse binary must run")
    }

    /// `glasshouse memory search <query>`, asserted to have succeeded, with
    /// its stdout returned.
    fn memory_search(&self, query: &str, history: bool) -> String {
        let mut args = vec!["memory", "search", query];
        if history {
            args.push("--history");
        }
        let output = self.run(&args);
        assert!(
            output.status.success(),
            "`glasshouse memory search` failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }
}

/// Every window, for a read that must not be refused for reaching too far
/// back.
const ALL_TIME: (i64, i64) = (0, i64::MAX);

// -------------------------------------------------------------------------
// 1822 / 1826 — the shipped binary's own retrievals become countable
// -------------------------------------------------------------------------

/// **Map line 1822, and the mutation target for this package.**
///
/// Three memories are planted: one current, one superseded, one marked for
/// review. `glasshouse memory search --history` is then run *as a process* —
/// the binary a user runs, resolving its own project, opening its own
/// database — and afterwards the ledger must be able to say how many of the
/// memories that search handed back were not current knowledge.
///
/// Deleting the `record_memory_retrieval` call in `main.rs` kills this test,
/// which is the point: nothing else in the suite enters through
/// `memory_search_grouped`.
#[test]
fn a_search_that_returns_a_superseded_memory_is_countable_afterwards() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let (current, superseded, needs_review) = {
        let memory = fixture.memory();
        let store = memory.store();
        let current = store
            .record(NewMemory::new(
                MemoryKind::Decision,
                "topaz indexing is done at write time",
            ))
            .unwrap();
        let superseded = store
            .record(NewMemory::new(
                MemoryKind::Decision,
                "topaz indexing was done nightly by a cron job",
            ))
            .unwrap();
        store
            .set_status(&superseded.id, MemoryStatus::Superseded)
            .unwrap();
        let needs_review = store
            .record(NewMemory::new(
                MemoryKind::Decision,
                "topaz shards are sized by hand",
            ))
            .unwrap();
        store
            .mark_for_review(&needs_review.id, ReviewReason::ArchitectureDrift)
            .unwrap();
        (current, superseded, needs_review)
    };

    // Nothing has retrieved anything yet.
    assert_eq!(
        fixture
            .ledger()
            .stale_retrievals(ALL_TIME.0, ALL_TIME.1)
            .unwrap()
            .retrievals,
        0,
        "no retrieval has happened, so nothing may be counted"
    );

    let report = fixture.memory_search("topaz", true);
    assert!(
        report.contains("topaz indexing was done nightly"),
        "the search must actually have returned the superseded memory; got:\n{report}"
    );

    let counts = fixture
        .ledger()
        .stale_retrievals(ALL_TIME.0, ALL_TIME.1)
        .unwrap();
    assert_eq!(
        counts.retrievals, 3,
        "one row per returned memory, and the search returned three"
    );
    assert_eq!(counts.superseded, 1, "map line 1826's own count");
    assert_eq!(counts.needs_review, 1);
    assert_eq!(counts.stale, 2, "map line 1822's own count");
    assert_eq!(
        counts.unresolved, 0,
        "every recorded memory_id must still resolve in `memories`"
    );
    assert_eq!(
        counts.stale_under_history, 2,
        "this search asked for history, so both stale hits were asked for"
    );

    // The rows carry ids and a scope, and nothing of the memories themselves.
    let ledger = fixture.ledger();
    let rows = ledger.recent(10).unwrap();
    assert_eq!(rows.len(), 3);
    let mut recorded: Vec<String> = rows
        .iter()
        .map(|row| row.memory_id.clone().expect("a retrieval names its memory"))
        .collect();
    recorded.sort();
    let mut expected = vec![
        current.id.as_str().to_owned(),
        superseded.id.as_str().to_owned(),
        needs_review.id.as_str().to_owned(),
    ];
    expected.sort();
    assert_eq!(recorded, expected);

    for row in &rows {
        assert_eq!(row.kind, EvaluationKind::MemoryRetrieved);
        assert_eq!(
            row.outcome,
            EvaluationOutcome::Unknown,
            "nothing knows yet whether a retrieval helped"
        );
        assert_eq!(row.subject.as_deref(), Some("historical"));
        assert_eq!(row.detail, None, "no observation stores memory content");
    }
}

/// The default search does not ask for history, and the count must say so.
///
/// A superseded memory returned by `--history` is the tool doing what it was
/// told; map line 1826 is about one *"incorrectly resurfaced as current
/// guidance"*, which is the case where the scope was `current`. Folding the
/// two together would report the history command itself as a defect.
#[test]
fn the_default_search_records_a_current_scope_and_returns_no_superseded_memory() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    {
        let memory = fixture.memory();
        let store = memory.store();
        store
            .record(NewMemory::new(
                MemoryKind::Decision,
                "jade caching is keyed by content hash",
            ))
            .unwrap();
        let old = store
            .record(NewMemory::new(
                MemoryKind::Decision,
                "jade caching was keyed by file path",
            ))
            .unwrap();
        store.set_status(&old.id, MemoryStatus::Superseded).unwrap();
    }

    fixture.memory_search("jade", false);

    let counts = fixture
        .ledger()
        .stale_retrievals(ALL_TIME.0, ALL_TIME.1)
        .unwrap();
    assert_eq!(
        counts.retrievals, 1,
        "the default scope returns current knowledge only"
    );
    assert_eq!(counts.stale, 0);
    assert_eq!(counts.stale_under_history, 0);

    let ledger = fixture.ledger();
    let rows = ledger.recent(10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].subject.as_deref(), Some("current"));
}

/// A search that matched nothing writes nothing, and opens no database to do
/// it.
///
/// This ledger counts *retrieved memories*, so an empty result has nothing to
/// count. Practice §65's rule is the other half: the handle is acquired where
/// its consumer starts, so the path that has nothing to record does not pay
/// for a second SQLite handle on any platform.
#[test]
fn a_search_that_returns_nothing_records_nothing() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let report = fixture.memory_search("nothing-matches-this", false);
    assert!(report.contains("No current memories match"));

    assert_eq!(fixture.ledger().recent(10).unwrap(), Vec::new());
}

/// **Bookkeeping may not break a search.**
///
/// The table is dropped while `schema_migrations` still claims the current
/// schema version, so the next launch runs no migration, does not rebuild it,
/// and every observation write fails at the SQL layer. The search must still find the memory and still print it: memory
/// retrieval is on the user's path and this ledger is not.
#[test]
fn a_ledger_that_cannot_be_written_does_not_fail_the_retrieval() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    {
        let memory = fixture.memory();
        memory
            .store()
            .record(NewMemory::new(
                MemoryKind::Decision,
                "cinnabar builds are reproducible",
            ))
            .unwrap();
    }

    {
        let conn = fixture.db();
        conn.execute_batch("DROP TABLE evaluation_observations;")
            .unwrap();
        let version: i64 = conn
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            version, 19,
            "the database must still claim the current schema version, so nothing \
             rebuilds the table"
        );
    }

    let output = fixture.run(&["memory", "search", "cinnabar"]);
    assert!(
        output.status.success(),
        "the search must succeed even though the ledger cannot be written: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("cinnabar builds are reproducible"),
        "the user's results must be unaffected"
    );
}

// -------------------------------------------------------------------------
// 1856 — local, project-scoped, and with no way out
// -------------------------------------------------------------------------

/// **Map line 1856, the structural half.** A row naming another project is
/// refused by the database itself, not by a caller remembering to check.
///
/// Both directions: an `INSERT` that names a foreign project, and an `UPDATE`
/// that tries to move an already-stored row into one. Removing either trigger
/// from migration 15 fails this test.
#[test]
fn a_foreign_projects_evaluation_row_is_refused_by_the_schema() {
    let tmp = tempdir();
    let alpha = Fixture::new(tmp.path(), "alpha");
    let beta = Fixture::new(tmp.path(), "beta");
    assert_ne!(
        alpha.runtime.project().id().as_str(),
        beta.runtime.project().id().as_str(),
        "the two fixtures must be two real projects"
    );

    // A row that belongs here is fine.
    alpha
        .ledger()
        .record(
            NewObservation::new(EvaluationKind::MemoryRetrieved).with_memory_id("m-local"),
            1_000,
        )
        .unwrap();

    let conn = alpha.db();
    let err = conn
        .execute(
            "INSERT INTO evaluation_observations
                 (project_id, observed_at, kind, outcome, memory_id)
             VALUES (?1, 1001, 'memory_retrieved', 'unknown', 'm-foreign')",
            [beta.runtime.project().id().as_str()],
        )
        .expect_err("the schema's own trigger must refuse a foreign project_id");
    assert!(err.to_string().contains("different project"), "got: {err}");

    let err = conn
        .execute(
            "UPDATE evaluation_observations SET project_id = ?1",
            [beta.runtime.project().id().as_str()],
        )
        .expect_err("the schema's own trigger must refuse moving a row to another project");
    assert!(err.to_string().contains("different project"), "got: {err}");

    // Nothing leaked either way.
    let alpha_rows = alpha.ledger().recent(10).unwrap();
    assert_eq!(alpha_rows.len(), 1);
    assert_eq!(alpha_rows[0].memory_id.as_deref(), Some("m-local"));
    assert_eq!(beta.ledger().recent(10).unwrap(), Vec::new());
}

/// **Map line 1856, the other half:** *"unless the user explicitly exports
/// it"* is carried by there being no export at all.
///
/// A capability that does not exist is a stronger guarantee than one behind a
/// flag, and this is what stops the next author adding one without noticing
/// that the line depends on its absence. Substring checks, not line splitting
/// — practice §14's line-ending trap does not reach a search that never asks
/// where a line ends.
#[test]
fn the_evaluation_module_has_no_path_out_of_the_project() {
    let source = include_str!("../src/evaluation/mod.rs");

    for forbidden in [
        "fn export",
        "fn to_json",
        "fn write_to",
        "fn dump",
        "serde::Serialize",
        "#[derive(Serialize",
        "serde_json",
        "std::fs::",
        "File::create",
        "ureq",
        "TcpStream",
    ] {
        assert!(
            !source.contains(forbidden),
            "`{forbidden}` appears in the evaluation module; map line 1856 \
             depends on this ledger having no way out of the project, so an \
             export needs the line re-argued rather than a test relaxed"
        );
    }

    // And no accessor hands out the connection itself.
    assert!(
        !source.contains("pub fn connection"),
        "handing out the Connection would make every guarantee above advisory"
    );
}

/// No observation stores memory *content* — ids and counts only.
///
/// The one column that could hold a sentence, `detail`, is never written by
/// the retrieval producer, and the `subject` it does write is a two-value
/// scope rather than the user's query text.
#[test]
fn a_recorded_retrieval_stores_no_memory_content() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let body = "malachite deploys are gated on a green pipeline";
    let subject_line = "malachite deploy gate";
    {
        let memory = fixture.memory();
        let mut new = NewMemory::new(MemoryKind::Decision, body);
        new.subject = Some(subject_line.to_owned());
        memory.store().record(new).unwrap();
    }

    fixture.memory_search("malachite", false);

    let conn = fixture.db();
    let mut statement = conn
        .prepare("SELECT * FROM evaluation_observations")
        .unwrap();
    let column_count = statement.column_count();
    let stored: Vec<String> = statement
        .query_map([], |row| {
            let mut cells = Vec::new();
            for index in 0..column_count {
                cells.push(match row.get_ref(index)? {
                    rusqlite::types::ValueRef::Text(text) => {
                        String::from_utf8_lossy(text).into_owned()
                    }
                    _ => String::new(),
                });
            }
            Ok(cells.join("\u{1f}"))
        })
        .unwrap()
        .map(|row| row.unwrap())
        .collect();

    assert_eq!(stored.len(), 1, "the search returned one memory");
    for cell in &stored {
        assert!(
            !cell.contains("malachite deploys"),
            "a memory's body reached the evaluation ledger: {cell}"
        );
        assert!(
            !cell.contains(subject_line),
            "a memory's subject reached the evaluation ledger: {cell}"
        );
        assert!(
            !cell.contains("malachite"),
            "the search query reached the evaluation ledger: {cell}"
        );
    }
}

// -------------------------------------------------------------------------
// Retention — part of migration 15's contract, not a follow-up
// -------------------------------------------------------------------------

/// The row bound trims oldest-first, and it is observable rather than merely
/// written.
#[test]
fn the_row_bound_trims_the_oldest_rows_first() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let ledger = fixture.ledger_with(Retention {
        max_age_secs: i64::MAX,
        max_rows: 3,
        trim_every: 1,
    });

    for n in 1..=6 {
        ledger
            .record(
                NewObservation::new(EvaluationKind::MemoryRetrieved)
                    .with_memory_id(format!("m-{n}")),
                1_000 + n,
            )
            .unwrap();
    }

    let kept: Vec<String> = ledger
        .recent(10)
        .unwrap()
        .into_iter()
        .map(|row| row.memory_id.unwrap())
        .collect();
    assert_eq!(
        kept,
        vec!["m-6".to_owned(), "m-5".to_owned(), "m-4".to_owned()],
        "the newest three rows survive and the oldest three go"
    );
}

/// The age bound trims by `observed_at`, and it binds independently of the row
/// bound.
#[test]
fn the_age_bound_trims_rows_older_than_the_window() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let ledger = fixture.ledger_with(Retention {
        max_age_secs: 100,
        max_rows: i64::MAX,
        // Never on the append cadence, so the trim under test is the explicit
        // one and nothing else can have done the work.
        trim_every: i64::MAX,
    });

    for at in [1_000, 1_050, 1_100] {
        ledger
            .record(
                NewObservation::new(EvaluationKind::MemoryRetrieved)
                    .with_memory_id(format!("m-{at}")),
                at,
            )
            .unwrap();
    }
    assert_eq!(ledger.recent(10).unwrap().len(), 3);

    // "Now" is 1_120, so the cutoff is 1_020 and only the first row is older.
    let removed = ledger.trim(1_120).unwrap();
    assert_eq!(removed, 1);
    let kept: Vec<String> = ledger
        .recent(10)
        .unwrap()
        .into_iter()
        .map(|row| row.memory_id.unwrap())
        .collect();
    assert_eq!(kept, vec!["m-1100".to_owned(), "m-1050".to_owned()]);
}

/// The trim runs on the append path, on a cadence that survives a process
/// which appends a handful of rows and exits.
///
/// The cadence is counted on `seq`, which is durable, and not on an in-memory
/// counter, which would reset on every `glasshouse memory search` and so would
/// never reach 256 in the usage this ledger's rows actually come from. Two
/// separately opened ledgers here stand in for two processes.
#[test]
fn the_append_cadence_is_durable_across_handles() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let retention = Retention {
        max_age_secs: i64::MAX,
        max_rows: 2,
        trim_every: 4,
    };

    // Three separate handles, one row each: an in-memory counter would sit at
    // 1 forever and never trim.
    for n in 1..=3 {
        fixture
            .ledger_with(retention)
            .record(
                NewObservation::new(EvaluationKind::MemoryRetrieved)
                    .with_memory_id(format!("m-{n}")),
                1_000 + n,
            )
            .unwrap();
    }
    assert_eq!(
        fixture.ledger_with(retention).recent(10).unwrap().len(),
        3,
        "no boundary has been crossed yet"
    );

    // The fourth row crosses `seq % 4 == 0`, and the trim runs in that same
    // transaction.
    fixture
        .ledger_with(retention)
        .record(
            NewObservation::new(EvaluationKind::MemoryRetrieved).with_memory_id("m-4"),
            1_004,
        )
        .unwrap();

    let kept: Vec<String> = fixture
        .ledger_with(retention)
        .recent(10)
        .unwrap()
        .into_iter()
        .map(|row| row.memory_id.unwrap())
        .collect();
    assert_eq!(kept, vec!["m-4".to_owned(), "m-3".to_owned()]);
}

/// **A count over a window older than what was kept refuses rather than
/// undercounting.**
///
/// The whole value of this ledger is a rate, and a rate whose denominator was
/// silently trimmed is a wrong answer wearing the costume of a right one.
#[test]
fn a_count_over_a_pruned_window_refuses_rather_than_undercounting() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let ledger = fixture.ledger_with(Retention {
        max_age_secs: i64::MAX,
        max_rows: 2,
        trim_every: 1,
    });

    // Before anything is trimmed, a window reaching back to the epoch is a
    // perfectly good question and gets a number. Refusing it would make the
    // most natural query unaskable and prove nothing.
    ledger
        .record(
            NewObservation::new(EvaluationKind::MemoryRetrieved).with_memory_id("m-0"),
            1_000,
        )
        .unwrap();
    assert_eq!(
        ledger
            .count(EvaluationKind::MemoryRetrieved, 0, i64::MAX)
            .unwrap(),
        1,
        "an unpruned ledger answers any window"
    );

    for n in 1..=4 {
        ledger
            .record(
                NewObservation::new(EvaluationKind::MemoryRetrieved)
                    .with_memory_id(format!("m-{n}")),
                1_000 + n,
            )
            .unwrap();
    }

    assert_eq!(ledger.oldest_retained_at().unwrap(), Some(1_003));

    // A window that starts inside what was kept is answerable.
    assert_eq!(
        ledger
            .count(EvaluationKind::MemoryRetrieved, 1_003, 1_100)
            .unwrap(),
        2
    );

    // One that reaches back past the trim is refused, in both readers.
    let err = ledger
        .count(EvaluationKind::MemoryRetrieved, 1_000, 1_100)
        .expect_err("a count over a pruned window must refuse");
    assert!(
        err.to_string().contains("oldest retained observation"),
        "got: {err}"
    );
    let err = ledger
        .stale_retrievals(1_000, 1_100)
        .expect_err("a stale-retrieval count over a pruned window must refuse");
    assert!(
        err.to_string().contains("oldest retained observation"),
        "got: {err}"
    );

    // And the case an oldest-row test cannot see at all: a ledger trimmed
    // empty has no oldest row, and a zero would read as "this never happened".
    let emptied = fixture.ledger_with(Retention {
        max_age_secs: 1,
        max_rows: i64::MAX,
        trim_every: i64::MAX,
    });
    assert_eq!(emptied.trim(i64::MAX / 2).unwrap(), 2);
    let err = emptied
        .count(EvaluationKind::MemoryRetrieved, 0, i64::MAX)
        .expect_err("a ledger trimmed empty must refuse rather than answer zero");
    assert!(err.to_string().contains("trimmed empty"), "got: {err}");
}

/// A ledger that never held anything answers zero, and does not pretend to
/// have been trimmed.
///
/// The distinction matters because it is the one an oldest-row test gets
/// wrong in the other direction: refusing here would make a fresh project
/// unable to ask its first question.
#[test]
fn a_ledger_that_never_held_anything_answers_zero() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    assert_eq!(
        fixture
            .ledger()
            .count(EvaluationKind::MemoryRetrieved, 0, i64::MAX)
            .unwrap(),
        0
    );
    assert_eq!(
        fixture
            .ledger()
            .stale_retrievals(0, i64::MAX)
            .unwrap()
            .retrievals,
        0
    );
}

// -------------------------------------------------------------------------
// Migration 15 itself
// -------------------------------------------------------------------------

/// A version-14 database migrates forward keeping every row.
///
/// Migration 15 is `CREATE TABLE` only — no `ALTER`, no rebuild, no existing
/// `CHECK` touched — and this is the proof that the claim holds against real
/// rows rather than an empty file.
#[test]
fn a_version_fourteen_database_migrates_forward_keeping_every_row() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let planted = {
        let memory = fixture.memory();
        memory
            .store()
            .record(NewMemory::new(
                MemoryKind::Decision,
                "obsidian rollouts predate migration 15",
            ))
            .unwrap()
    };

    let db_path = fixture.runtime.database_path();
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            // Every migration above 14, newest first: 19's two tables,
            // 18's column, 17's table, 16's column, then 15's table.
            "DROP TABLE assumption_transitions;
             DROP TABLE task_assumptions;
             ALTER TABLE routing_observations DROP COLUMN failure_class;
             DROP TABLE memory_files;
             ALTER TABLE sessions DROP COLUMN observed_compactions;
             DROP TABLE evaluation_observations;
             DELETE FROM schema_migrations WHERE version >= 15;",
        )
        .unwrap();
        let version: i64 = conn
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, 14, "the rollback must land on version 14");
    }

    // An ordinary launch, which is the only thing that ever runs a migration.
    let migrated = Fixture::new(tmp.path(), "alpha");
    let version: i64 = Connection::open(migrated.runtime.database_path())
        .unwrap()
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(
        version, 19,
        "the launch must have applied migrations 15 through 19"
    );

    let memory = migrated.memory();
    let intact = memory
        .store()
        .get(&planted.id)
        .unwrap()
        .expect("the pre-existing memory must survive the migration");
    assert_eq!(intact.body, planted.body);
    assert_eq!(intact.status, planted.status);

    // And the new table is there, empty, and writable.
    migrated
        .ledger()
        .record(
            NewObservation::new(EvaluationKind::MemoryRetrieved).with_memory_id("m-after"),
            2_000,
        )
        .unwrap();
    assert_eq!(migrated.ledger().recent(10).unwrap().len(), 1);
}

/// Every `(kind, outcome)` pair the Rust vocabulary can produce is one the
/// real schema accepts.
///
/// Migration 15 gives `kind` and `outcome` no `CHECK`, on the argument that a
/// SQL vocabulary in the one table certain to need widening is manufacturing
/// migration 7's problem deliberately. That argument is only honest if the
/// Rust side is actually pinned, through the real schema, which is what this
/// does.
#[test]
fn every_stored_vocabulary_is_one_the_schema_accepts() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let ledger = fixture.ledger();

    let mut at = 1_000;
    for kind in [EvaluationKind::MemoryRetrieved] {
        for outcome in [EvaluationOutcome::Unknown] {
            for scope in [RetrievalScope::Current, RetrievalScope::Historical] {
                let mut new = NewObservation::new(kind).with_subject(scope.as_str());
                new.outcome = outcome;
                at += 1;
                ledger
                    .record(new, at)
                    .unwrap_or_else(|err| panic!("{kind:?}/{outcome:?} was refused: {err}"));
            }
        }
    }

    // And every one reads back as itself rather than as a neighbour.
    for row in ledger.recent(100).unwrap() {
        assert_eq!(row.kind, EvaluationKind::MemoryRetrieved);
        assert_eq!(row.outcome, EvaluationOutcome::Unknown);
    }
}

/// A kind this build does not know is reported, not bucketed.
#[test]
fn a_kind_this_build_does_not_know_is_reported_rather_than_absorbed() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    fixture
        .db()
        .execute(
            "INSERT INTO evaluation_observations
                 (project_id, observed_at, kind, outcome)
             VALUES (?1, 1000, 'route_preferred', 'unknown')",
            [fixture.runtime.project().id().as_str()],
        )
        .expect("the schema has no `kind` CHECK, so this row stores");

    let err = fixture
        .ledger()
        .recent(10)
        .expect_err("a kind this build cannot decode must be reported");
    let message = err.to_string();
    assert!(message.contains("route_preferred"), "got: {message}");
    assert!(
        message.contains("memory_retrieved"),
        "the error must name the vocabulary it does read; got: {message}"
    );
}

/// This ledger is prunable **by construction**, which is the difference
/// between it and `lifecycle_events`.
///
/// Migration 5's append-only `DELETE` trigger is why `lifecycle_events` cannot
/// be trimmed even deliberately; copying it here would have repeated a known
/// defect, so migration 15 copies migration 11's two triggers and not
/// migration 5's three. If a future migration adds one, this fails.
#[test]
fn the_evaluation_ledger_carries_no_append_only_trigger() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let conn = fixture.db();
    let mut statement = conn
        .prepare(
            "SELECT name FROM sqlite_master
              WHERE type = 'trigger' AND tbl_name = 'evaluation_observations'
              ORDER BY name",
        )
        .unwrap();
    let triggers: Vec<String> = statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|name| name.unwrap())
        .collect();
    assert_eq!(
        triggers,
        vec![
            "evaluation_observations_reject_foreign_project_insert".to_owned(),
            "evaluation_observations_reject_foreign_project_update".to_owned(),
        ],
        "migration 11's two project-scope triggers, and nothing else"
    );

    // The proof that matters is behavioural: a delete goes through.
    fixture
        .ledger()
        .record(
            NewObservation::new(EvaluationKind::MemoryRetrieved).with_memory_id("m-1"),
            1_000,
        )
        .unwrap();
    let removed = conn
        .execute("DELETE FROM evaluation_observations", [])
        .expect("this ledger must be prunable, unlike `lifecycle_events`");
    assert_eq!(removed, 1);
}

// -------------------------------------------------------------------------
// The `routing_seq` pointer, and the promise it makes about a future rebuild
// -------------------------------------------------------------------------

/// `routing_seq` puts `routing_observations` into the category
/// `lifecycle_events` is already in: **a table whose `seq` a future migration
/// may not renumber.**
///
/// `AUTOINCREMENT` protects against pruning, not against a rebuild. Migration
/// 7 documents that hazard at length for `lifecycle_events`, and
/// `a_memorys_provenance_survives_the_seq_rebuild` is the test that holds it.
/// This is the same test for the pointer this package introduces, and it is
/// written in the same change as the pointer rather than after the first
/// rebuild has already broken it.
///
/// It proves both halves. A **naive** rebuild — copy the surviving rows and
/// let the new table's own `AUTOINCREMENT` number them — silently re-points
/// the observation at a different turn, and this test watches that happen, so
/// the second half is not asserting against a rebuild that could not have
/// failed. A rebuild that copies `seq` explicitly keeps the pointer meaning
/// what it meant.
#[test]
fn an_evaluation_rows_provenance_survives_a_routing_observations_rebuild() {
    let tmp = tempdir();

    for naive in [true, false] {
        let fixture = Fixture::new(tmp.path(), if naive { "naive" } else { "faithful" });
        let project_id = fixture.runtime.project().id().as_str().to_owned();

        // Three routed turns, distinguishable by model.
        {
            let conn = fixture.db();
            for (n, model) in ["m-one", "m-two", "m-three"].iter().enumerate() {
                conn.execute(
                    "INSERT INTO routing_observations
                         (project_id, observed_at, provider, model, context_state)
                     VALUES (?1, ?2, 'anyrouter', ?3, 'unknown')",
                    rusqlite::params![project_id, 1_000 + n as i64, model],
                )
                .unwrap();
            }
        }

        let pointed_at: i64 = fixture
            .db()
            .query_row(
                "SELECT seq FROM routing_observations WHERE model = 'm-three'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        fixture
            .ledger()
            .record(
                NewObservation::new(EvaluationKind::MemoryRetrieved)
                    .with_memory_id("m-x")
                    .with_routing_seq(pointed_at),
                2_000,
            )
            .unwrap();

        // Rebuild `routing_observations` the way a future migration would,
        // dropping the oldest row as a retention policy might have. The DDL is
        // read back out of the schema so this is the real table's shape and
        // not a transcription of it.
        {
            let conn = fixture.db();
            let ddl: String = conn
                .query_row(
                    "SELECT sql FROM sqlite_master
                      WHERE type = 'table' AND name = 'routing_observations'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            let rebuilt = ddl.replacen(
                "CREATE TABLE routing_observations",
                "CREATE TABLE routing_observations_rebuilt",
                1,
            );
            assert!(
                rebuilt.contains("routing_observations_rebuilt"),
                "the rebuild fixture must have renamed the table it creates"
            );
            conn.execute_batch(&rebuilt).unwrap();

            let copy = if naive {
                // The mistake: let the new table number the rows itself.
                "INSERT INTO routing_observations_rebuilt
                     (project_id, observed_at, provider, model, context_state)
                 SELECT project_id, observed_at, provider, model, context_state
                   FROM routing_observations WHERE seq > 1;"
            } else {
                // Migration 7's fix: copy `seq` explicitly.
                "INSERT INTO routing_observations_rebuilt
                     (seq, project_id, observed_at, provider, model, context_state)
                 SELECT seq, project_id, observed_at, provider, model, context_state
                   FROM routing_observations WHERE seq > 1;"
            };
            conn.execute_batch(&format!(
                "{copy}
                 DROP TABLE routing_observations;
                 ALTER TABLE routing_observations_rebuilt
                     RENAME TO routing_observations;"
            ))
            .unwrap();
        }

        let still_names: Option<String> = fixture
            .db()
            .query_row(
                "SELECT r.model
                   FROM evaluation_observations AS e
                   JOIN routing_observations AS r ON r.seq = e.routing_seq
                  WHERE e.memory_id = 'm-x'",
                [],
                |row| row.get(0),
            )
            .ok();

        if naive {
            assert_ne!(
                still_names.as_deref(),
                Some("m-three"),
                "a rebuild that renumbers `seq` must be seen to break the pointer, \
                 or the other half of this test proves nothing"
            );
        } else {
            assert_eq!(
                still_names.as_deref(),
                Some("m-three"),
                "a rebuild of `routing_observations` must copy `seq`, or every \
                 evaluation row's provenance silently names a different turn"
            );
        }
    }
}

/// **The three kinds this build's routing-outcome half adds**, both halves of
/// what makes a new kind real: the Rust vocabulary and the schema constant
/// agree about it, and the schema still refuses a row that names another
/// project under it.
///
/// Migration 15 gives `kind` no `CHECK`, deliberately, so nothing in SQL
/// would have caught a new kind spelled two ways — and nothing in SQL knows
/// these three exist, so nothing would have proved the project triggers still
/// apply to them either. Both are checked here rather than assumed from the
/// kinds that came before.
#[test]
fn the_new_kinds_are_in_the_vocabulary_and_foreign_project_rows_are_refused() {
    let tmp = tempdir();
    let alpha = Fixture::new(tmp.path(), "alpha");
    let beta = Fixture::new(tmp.path(), "beta");

    for kind in [
        EvaluationKind::RoutingCostClassObserved,
        EvaluationKind::RoutingEvidenceObserved,
        EvaluationKind::RoutingOutcomeObserved,
    ] {
        assert_eq!(
            EvaluationKind::from_stored(kind.as_str()),
            Some(kind),
            "{kind:?} must decode back to itself, or a row this build wrote is a row it \
             cannot read"
        );

        alpha
            .ledger()
            .record(
                NewObservation::new(kind)
                    .with_subject("completed")
                    .with_session_id("s-local")
                    .with_detail("fresh:claude-code:native"),
                1_000,
            )
            .expect("a row of a new kind belongs in its own project");

        let err = alpha
            .db()
            .execute(
                "INSERT INTO evaluation_observations
                     (project_id, observed_at, kind, outcome, subject, session_id)
                 VALUES (?1, 1001, ?2, 'unknown', 'completed', 's-foreign')",
                rusqlite::params![beta.runtime.project().id().as_str(), kind.as_str()],
            )
            .expect_err("migration 15's trigger must refuse a foreign project_id for a new kind");
        assert!(err.to_string().contains("different project"), "got: {err}");
    }

    assert_eq!(
        beta.ledger().recent(10).unwrap(),
        Vec::new(),
        "nothing leaked into the other project"
    );
}

// -------------------------------------------------------------------------
// 1829 / 1830 — a launch's routing decision becomes countable
// -------------------------------------------------------------------------

/// A project wired with a fake harness, so `glasshouse launch` runs end to
/// end — the same shape `tests/route_command.rs` uses, and for the same
/// reason (practice §35): lines 1829 and 1830's whole claim is that
/// `main.rs::launch_session`, the production caller, now leaves a trace.
/// Asserting that `record_routing_decision` inserts a row would prove
/// nothing about that.
struct LaunchFixture {
    base: PathBuf,
    runtime: Runtime,
}

impl LaunchFixture {
    fn new(base: &Path) -> Self {
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        let root = std::fs::canonicalize(&root).expect("canonicalize project root");

        let bin_dir = base.join("bin");
        std::fs::create_dir_all(&bin_dir).expect("create bin dir");
        let harness = install_fake_harness(&bin_dir);
        let escaped = harness.display().to_string().replace('\\', "\\\\");

        let config_dir = base.join("config");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        std::fs::write(
            config_dir.join("config.toml"),
            format!(
                "version = 1\n\n[integrations.claude-code]\nenabled = true\nexecutable = \"{escaped}\"\n"
            ),
        )
        .expect("write user config");

        let cli = Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            base.join("data").to_str().unwrap(),
            "--config-dir",
            config_dir.to_str().unwrap(),
        ])
        .unwrap();
        let runtime = bootstrap(&cli, &root).unwrap();

        LaunchFixture {
            base: base.to_path_buf(),
            runtime,
        }
    }

    fn glasshouse(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .current_dir(self.runtime.project().root())
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args)
            .output()
            .expect("the glasshouse binary must run")
    }

    fn both_streams(output: &std::process::Output) -> String {
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    }

    fn ledger(&self) -> EvaluationObservations {
        EvaluationObservations::open(&self.runtime).unwrap()
    }

    /// Every recorded session's id, oldest first — read straight off the
    /// `sessions` table, the same door `route_command.rs`'s
    /// `recorded_sessions` reaches through `glasshouse sessions` instead.
    fn session_ids(&self) -> Vec<String> {
        let conn = Connection::open(self.runtime.database_path()).unwrap();
        let mut statement = conn
            .prepare("SELECT id FROM sessions ORDER BY created_at")
            .unwrap();
        statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|row| row.unwrap())
            .collect()
    }
}

#[cfg(unix)]
fn install_fake_harness(bin_dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = bin_dir.join("fake-claude-code");
    std::fs::write(&path, "#!/bin/sh\nexit 0\n").expect("write fake harness");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

#[cfg(windows)]
fn install_fake_harness(bin_dir: &Path) -> PathBuf {
    let path = bin_dir.join("fake-claude-code.cmd");
    std::fs::write(&path, "@echo off\r\nexit /b 0\r\n").expect("write fake harness");
    path
}

/// **Acceptance 1.** A launch with nothing to override records the automatic
/// outcome, and `overrode()`'s `None` must be recorded as *the automatic
/// answer stood* — not folded into an override.
///
/// This is also the mutation target for the producer: a producer that
/// recorded `"overridden"` whenever it was merely called, rather than reading
/// `overrode()`, would pass every other test here and only fail this one.
#[test]
fn a_launch_where_the_ranking_stands_records_the_automatic_outcome_not_an_override() {
    let tmp = tempdir();
    let fixture = LaunchFixture::new(tmp.path());

    let launched = fixture.glasshouse(&["launch", "claude-code", "--headless"]);
    assert!(
        launched.status.success(),
        "the launch must succeed:\n{}",
        LaunchFixture::both_streams(&launched)
    );

    let overrides = fixture
        .ledger()
        .recent_of_kind(EvaluationKind::RoutingOverrideDecided, 10)
        .unwrap();
    assert_eq!(overrides.len(), 1, "one launch, one override outcome");
    assert_eq!(
        overrides[0].subject.as_deref(),
        Some("automatic"),
        "no override was asked for, so `overrode()`'s `None` must be recorded as the automatic \
         answer standing, not as an override"
    );
    assert_eq!(
        overrides[0].detail, None,
        "the automatic case names no destination the ranking would have chosen instead"
    );

    let continuations = fixture
        .ledger()
        .recent_of_kind(EvaluationKind::RoutingContinuationDecided, 10)
        .unwrap();
    assert_eq!(
        continuations.len(),
        1,
        "one launch, one continuation outcome"
    );
    assert_eq!(
        continuations[0].subject.as_deref(),
        Some("fresh"),
        "a first launch has no warm session to continue"
    );
}

/// **Acceptance 2.** An explicit `--fresh` that the ranking disagreed with —
/// because a warm session from the first launch was there to continue — is
/// recorded as an override, naming the destination the ranking would have
/// chosen instead.
#[test]
fn an_override_the_ranking_disagreed_with_names_the_destination_it_would_have_chosen() {
    let tmp = tempdir();
    let fixture = LaunchFixture::new(tmp.path());

    let first = fixture.glasshouse(&["launch", "claude-code", "--headless"]);
    assert!(
        first.status.success(),
        "the first launch must succeed:\n{}",
        LaunchFixture::both_streams(&first)
    );
    let sessions_after_first = fixture.session_ids();
    assert_eq!(
        sessions_after_first.len(),
        1,
        "the first launch records exactly one session"
    );
    let warm_id = sessions_after_first[0].clone();

    // Without `--fresh` the ranking continues that warm session — proven by
    // `route_command.rs`'s `a_second_launch_continues_the_warm_session_...`.
    // `--fresh` overrides that answer.
    let second = fixture.glasshouse(&["launch", "claude-code", "--headless", "--fresh"]);
    assert!(
        second.status.success(),
        "`--fresh` must start a session even over a warm one:\n{}",
        LaunchFixture::both_streams(&second)
    );

    let overrides = fixture
        .ledger()
        .recent_of_kind(EvaluationKind::RoutingOverrideDecided, 10)
        .unwrap();
    assert_eq!(overrides.len(), 2, "one row per launch");
    assert_eq!(
        overrides[0].subject.as_deref(),
        Some("overridden"),
        "`--fresh` changed the answer away from the warm session the ranking would have chosen"
    );
    assert_eq!(
        overrides[0].detail.as_deref(),
        Some(warm_id.as_str()),
        "the row must name the destination the ranking would have chosen instead"
    );
    assert_eq!(
        overrides[1].subject.as_deref(),
        Some("automatic"),
        "the first launch had nothing to override"
    );
}

/// **Acceptance 3.** A launch that continues a warm session and one that
/// starts fresh are distinguishable in the ledger.
#[test]
fn a_continued_warm_session_and_a_fresh_one_are_distinguishable_in_the_ledger() {
    let tmp = tempdir();
    let fixture = LaunchFixture::new(tmp.path());

    let first = fixture.glasshouse(&["launch", "claude-code", "--headless"]);
    assert!(
        first.status.success(),
        "the first launch must succeed:\n{}",
        LaunchFixture::both_streams(&first)
    );

    // No override this time: the router is left to prefer the warm session
    // the first launch just made.
    let second = fixture.glasshouse(&["launch", "claude-code", "--headless"]);
    assert!(
        second.status.success(),
        "the second launch must succeed:\n{}",
        LaunchFixture::both_streams(&second)
    );
    assert_eq!(
        fixture.session_ids().len(),
        1,
        "the second launch must have continued the warm session rather than starting another"
    );

    let continuations = fixture
        .ledger()
        .recent_of_kind(EvaluationKind::RoutingContinuationDecided, 10)
        .unwrap();
    assert_eq!(continuations.len(), 2, "one row per launch");
    assert_eq!(
        continuations[0].subject.as_deref(),
        Some("existing"),
        "the second launch continued the warm session"
    );
    assert_eq!(
        continuations[1].subject.as_deref(),
        Some("fresh"),
        "the first launch had nothing to continue"
    );

    let overrides = fixture
        .ledger()
        .recent_of_kind(EvaluationKind::RoutingOverrideDecided, 10)
        .unwrap();
    assert!(
        overrides
            .iter()
            .all(|row| row.subject.as_deref() == Some("automatic")),
        "neither launch asked for an override:\n{overrides:?}"
    );
}

/// **Acceptance 4.** `glasshouse route` reports without acting; it must
/// record nothing, or the counts these two lines produce would answer a
/// different question than 1829 and 1830 ask.
#[test]
fn glasshouse_route_reports_without_acting_and_records_nothing() {
    let tmp = tempdir();
    let fixture = LaunchFixture::new(tmp.path());

    let launched = fixture.glasshouse(&["launch", "claude-code", "--headless"]);
    assert!(
        launched.status.success(),
        "the launch must succeed:\n{}",
        LaunchFixture::both_streams(&launched)
    );

    let overrides_before = fixture
        .ledger()
        .recent_of_kind(EvaluationKind::RoutingOverrideDecided, 100)
        .unwrap()
        .len();
    let continuations_before = fixture
        .ledger()
        .recent_of_kind(EvaluationKind::RoutingContinuationDecided, 100)
        .unwrap()
        .len();
    assert_eq!(
        overrides_before, 1,
        "the launch recorded exactly one override outcome"
    );
    assert_eq!(
        continuations_before, 1,
        "the launch recorded exactly one continuation outcome"
    );

    let reported = fixture.glasshouse(&["route"]);
    assert!(
        reported.status.success(),
        "`glasshouse route` must succeed:\n{}",
        LaunchFixture::both_streams(&reported)
    );

    let overrides_after = fixture
        .ledger()
        .recent_of_kind(EvaluationKind::RoutingOverrideDecided, 100)
        .unwrap()
        .len();
    let continuations_after = fixture
        .ledger()
        .recent_of_kind(EvaluationKind::RoutingContinuationDecided, 100)
        .unwrap()
        .len();
    assert_eq!(
        overrides_after, overrides_before,
        "`glasshouse route` reports without acting, so it must not add an override row"
    );
    assert_eq!(
        continuations_after, continuations_before,
        "`glasshouse route` reports without acting, so it must not add a continuation row"
    );
}
