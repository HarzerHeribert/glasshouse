use super::bootstrap::{
    CreatorLiveness, PRIVATE_INFIX, creator_is_gone, install_private_file_hold, journal_beside,
    parse_private_name,
};
use super::migrations::MIGRATIONS_V1_TO_V13;
use super::schema::{FAILURE_CLASSES, SUPPORTED_SCHEMA_VERSION, TASK_CLASSES};
use super::*;
use crate::{Cli, Runtime};
use clap::Parser;
use rusqlite::OptionalExtension;
// Only the unix permission tests below name `fs` unqualified; on Windows the
// import would be unused and `-D warnings` refuses it (Windows VM run 12).
#[cfg(unix)]
use std::fs;
use std::path::PathBuf;

/// A project rooted inside `base`'s `workspace/`, bootstrapped against
/// `base`'s `data/` and `config/`. Fixtures sharing one `base` therefore
/// share one GLASSHOUSE data/config root, like two real projects on one
/// machine.
struct Fixture {
    base: PathBuf,
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
        let runtime = crate::bootstrap(&cli, &root).unwrap();
        Fixture {
            base: base.to_path_buf(),
            runtime,
        }
    }

    /// Bootstrap the same project again, exactly as a later launch would.
    fn rebootstrap(&self) -> anyhow::Result<Runtime> {
        let cli = Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            self.base.join("data").to_str().unwrap(),
            "--config-dir",
            self.base.join("config").to_str().unwrap(),
        ])
        .unwrap();
        crate::bootstrap(&cli, self.runtime.project().root())
    }
}

/// Undo every migration above 13, for a fixture that claims to be an older
/// database.
///
/// A rollback fixture has to undo **every** migration above the version it
/// claims to be, not only the one it is about. Migration 14 arrived after
/// three of these were written, and each of them failed the re-run with
/// `duplicate column name: seq` until it was added here — which is why
/// this is one constant rather than three copies for the next migration to
/// miss. Migration 15 was appended for the same reason and cost nothing,
/// which is the point of the constant existing.
///
/// Dropping a table takes its indexes and triggers with it, so migration
/// 15 is one statement. Migration 14 is not: SQLite refuses to drop a
/// column an index mentions, so its indexes go first, and
/// `checkpoints_by_session` is put back the way migration 5 left it.
/// Migration 16 is one statement for the opposite reason: nothing indexes
/// `observed_compactions`, and a column-scoped `CHECK` goes with the
/// column it is written on. Migration 17 is one statement for migration
/// 15's reason — dropping `memory_files` takes its index and its two
/// triggers with it — and it goes first among the tables, because the
/// rollback runs newest-migration-first for the same reason the ladder
/// runs oldest-first. Migration 18 is one statement for migration 16's
/// reason — nothing indexes `failure_class` and it carries no `CHECK` —
/// and it goes before all of them, being the newest. Migration 19 is two
/// statements for migration 15's reason — each table takes its indexes
/// and triggers with it — and migration 20 is one for migration 16's:
/// nothing indexes `presentation_ref` and it carries no `CHECK`.
/// Migrations 21 and 22 are each one statement for the same reason —
/// nothing indexes `last_seen_commit`, `extraction_trigger` or
/// `entitlement` and none of the three carries a `CHECK`. Migration 23 is
/// one statement for the same reason — nothing indexes `task_class` and
/// it carries no `CHECK`. Migration 24 is three for the same reason
/// again — nothing indexes `session_id`, `effort_level` or `turn_shape`
/// and none of the three carries a `CHECK` or a `REFERENCES`. Migration
/// 25 is four statements, and it is migration 16's reason rather than
/// 23's: nothing indexes the four millisecond offsets, and each of them
/// *does* carry a `CHECK` — a column-scoped one, which SQLite drops with
/// the column it is written on. Newest first, so 25's four lead and
/// 24's three follow, each set in the reverse of the order it was
/// added.
///
/// Migration 26 is the only one here that is none of those shapes, and it
/// leads because it is the newest. It rebuilt `lifecycle_events`, so
/// undoing it is a rebuild back to migration 7's exact table — an
/// `ALTER TABLE ... DROP COLUMN` cannot do it, because `path` is named in
/// a **table**-scoped `CHECK` and SQLite refuses to drop a column a table
/// constraint mentions, and dropping the column alone would leave `kind`'s
/// own `CHECK` still admitting `file_touched`. `seq` is named explicitly
/// on the way back for exactly the reason migration 26 names it on the way
/// out: `memories.source_event_first`/`.source_event_last` point at it.
const UNDO_MIGRATIONS_ABOVE_THIRTEEN: &str = "
        CREATE TABLE lifecycle_events_pre26 (
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
            observed_harness TEXT,
            observed_event   TEXT,
            CHECK ((observed_harness IS NULL) = (observed_event IS NULL))
        );
        INSERT INTO lifecycle_events_pre26 (
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
        FROM lifecycle_events
        WHERE kind <> 'file_touched';
        DROP TABLE lifecycle_events;
        ALTER TABLE lifecycle_events_pre26 RENAME TO lifecycle_events;
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

        DROP TABLE evaluation_observations;

        DROP INDEX checkpoints_by_seq;
        DROP INDEX checkpoints_by_session;
        ALTER TABLE checkpoints DROP COLUMN seq;
        CREATE INDEX checkpoints_by_session
            ON checkpoints (session_id, created_at DESC);
    ";

fn stored_project_id(db_path: &Path) -> String {
    let conn = Connection::open(db_path).unwrap();
    conn.query_row(
        "SELECT value FROM project_metadata WHERE key = 'project_id'",
        [],
        |row| row.get(0),
    )
    .unwrap()
}

fn schema_version(db_path: &Path) -> i64 {
    let conn = Connection::open(db_path).unwrap();
    conn.query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
        row.get(0)
    })
    .unwrap()
}

/// A phase the type can produce and the schema will not accept is a
/// constraint violation at the moment a memory is stored, on whichever
/// thread happens to be extracting. Migration 6's `CHECK` is the
/// authority, so this reads the list **out of the migration itself**
/// rather than out of a second constant beside it: a constant can drift
/// from the SQL, and then the pin proves only that two Rust literals
/// still agree.
#[test]
fn every_project_phase_the_type_supports_is_one_the_schema_accepts() {
    use crate::memory::ProjectPhase;

    let migration = MIGRATIONS_V1_TO_V13[5];
    let marker = "project_phase IN";
    let open = migration
        .find(marker)
        .expect("migration 6 checks the phase")
        + marker.len();
    let list = &migration[open..];
    let list = &list[..list.find(')').expect("the CHECK's list is parenthesised")];
    let accepted: Vec<String> = list
        .split(',')
        .map(|value| value.trim().trim_matches(['(', ' ', '\n', '\'']).to_owned())
        .filter(|value| !value.is_empty())
        .collect();

    let declared: Vec<String> = ProjectPhase::ALL
        .iter()
        .map(|phase| phase.as_str().to_owned())
        .collect();

    assert_eq!(
        declared, accepted,
        "a project phase was added or renamed without migration 6's CHECK"
    );

    // And the parse has to be able to fail, or it asserts nothing: the
    // map's own list is five long, so a `CHECK` this failed to read
    // would show up here as an empty vector rather than as a pass.
    assert_eq!(accepted.len(), 5, "the CHECK's list was not read correctly");
}

/// Migration 17's `provenance` carries **no** `CHECK`, so nothing in SQL
/// pins it — this test is the guarantee, exactly as
/// `EVALUATION_KINDS`' own pinning test is for
/// `evaluation_observations.kind`.
///
/// Two independently written spellings: [`MEMORY_FILE_PROVENANCE`], which
/// sits beside the migration where a schema reader looks, and
/// [`crate::memory::FileAssociation`], which is what the writer actually
/// stores. Neither is derived from the other — that is the whole point,
/// and it is why this is not a tautology.
///
/// **The one that must never appear here is `referenced`.** Migration
/// 17's own text and `FileAssociation::Observed`'s both say why: this
/// build observes which files were dirty, and calling that a reference
/// would close capability-map line 1139 on a producer that does not
/// exist. A future package may add the value — beside this one, with its
/// own producer — and this test is where it has to be declared.
#[test]
fn every_file_association_the_type_supports_is_one_the_schema_records() {
    use crate::memory::FileAssociation;

    let declared: Vec<&str> = FileAssociation::ALL
        .iter()
        .map(|association| association.as_str())
        .collect();
    assert_eq!(
        declared,
        MEMORY_FILE_PROVENANCE.to_vec(),
        "a memory-file provenance was added or renamed on one side only"
    );

    // Every declared value must survive a round trip, or a row this build
    // wrote is a row it cannot read back.
    for association in FileAssociation::ALL {
        assert_eq!(
            FileAssociation::from_stored(association.as_str()),
            Some(*association)
        );
    }
    // A word no build has ever written. `referenced` used to stand here
    // for exactly that reason and stopped being the right example when
    // migration 26 gave it a producer — which is the whole content of
    // this assertion: `from_stored` drops what it does not know rather
    // than defaulting, so a row from a later build reads as no
    // association instead of as the weaker of the two this one has.
    assert_eq!(FileAssociation::from_stored("mentioned"), None);
    assert_eq!(FileAssociation::from_stored(""), None);

    // Map line 1139's ordering, pinned where the vocabulary is: a memory
    // carrying both rows for one file reports the claim, not the
    // correlation. `FileAssociation::strongest` is what
    // `MemoryStore::for_path` folds its `group_concat` with, and getting
    // this backwards would label every referenced row `observed`.
    use FileAssociation::{Observed, Referenced};
    assert_eq!(Observed.strongest(Referenced), Referenced);
    assert_eq!(Referenced.strongest(Observed), Referenced);
    assert_eq!(Observed.strongest(Observed), Observed);
    assert_eq!(Referenced.strongest(Referenced), Referenced);
}

/// Migration 18's `failure_class` carries **no** `CHECK`, so nothing in
/// SQL pins it — this test is the guarantee, exactly as
/// `EVALUATION_KINDS`' and [`MEMORY_FILE_PROVENANCE`]'s own are.
///
/// Two independently written spellings: [`FAILURE_CLASSES`], beside the
/// migration where a schema reader looks, and
/// [`crate::routing::evidence::FailureClass`], which the writer stores.
/// Neither is derived from the other, which is why this is not a
/// tautology. Nine, in capability map line 1364's own order.
#[test]
fn every_failure_class_the_type_supports_is_one_the_schema_records() {
    use crate::routing::evidence::FailureClass;

    let declared: Vec<&str> = FailureClass::ALL
        .iter()
        .map(|class| class.as_str())
        .collect();
    assert_eq!(
        declared,
        FAILURE_CLASSES.to_vec(),
        "a failure class was added, renamed or reordered on one side only"
    );
    assert_eq!(FAILURE_CLASSES.len(), 9, "the map line names nine");

    for class in FailureClass::ALL {
        assert_eq!(FailureClass::from_stored(class.as_str()), Some(class));
    }
    // A spelling nothing writes reads as unrecognised, never as a class.
    assert_eq!(FailureClass::from_stored("rate_limited"), None);
    assert_eq!(FailureClass::from_stored(""), None);
}

/// Migration 23's `task_class` carries **no** `CHECK`, so nothing in SQL
/// pins it — this test is the guarantee, exactly as
/// `every_failure_class_the_type_supports_is_one_the_schema_records` is
/// for migration 18.
///
/// Two independently written spellings: [`TASK_CLASSES`], beside the
/// migration where a schema reader looks, and
/// [`crate::routing::request::TaskClass`], which the writer stores.
/// Neither is derived from the other.
#[test]
fn every_task_class_the_type_supports_is_one_the_schema_records() {
    use crate::routing::request::TaskClass;

    let declared: Vec<&str> = TaskClass::ALL.iter().map(|class| class.as_str()).collect();
    assert_eq!(
        declared,
        TASK_CLASSES.to_vec(),
        "a task class was added, renamed or reordered on one side only"
    );
    assert_eq!(TASK_CLASSES.len(), 5, "the type declares five");

    for class in TaskClass::ALL {
        assert_eq!(TaskClass::from_stored(class.as_str()), Some(class));
    }
    // An unrecognised word reads as no class — never an error, and never
    // a class this build invented. Migration 23's own doc comment says
    // why this differs from `failure_class`.
    assert_eq!(TaskClass::from_stored("code_modification"), None);
    assert_eq!(TaskClass::from_stored(""), None);
}

/// The column names of `table`, in declaration order.
fn columns_of(conn: &Connection, table: &str) -> Vec<String> {
    let mut statement = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .unwrap();
    statement
        .query_map([], |row| row.get::<_, String>("name"))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// Everything `sqlite_master` holds, in a stable order — the whole
/// schema as one comparable value.
fn whole_schema(conn: &Connection) -> Vec<(String, String, Option<String>)> {
    let mut statement = conn
        .prepare("SELECT type, name, sql FROM sqlite_master ORDER BY type, name")
        .unwrap();
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// Migration proof for 18: a version-17 database that already holds a
/// routing observation opens, migrates to 18 adding exactly one column,
/// reads the old row's `failure_class` as unknown rather than as a class,
/// records a classified row through the real writer, and the undo takes
/// the whole schema back to exactly what it was — every table, index and
/// trigger.
///
/// One connection at a time throughout (practice §65): every handle is
/// dropped before the next is opened and before the re-bootstrap.
#[test]
fn migration_18_adds_failure_class_and_undoes_cleanly() {
    use crate::routing::evidence::{
        EvidenceLedger, FailureClass, NewObservation, ObservationQuery, Outcome,
    };

    // Migrations 20 and 19 are undone first: a rollback undoes every
    // migration above the version it claims, or the re-run fails with
    // `duplicate column name` — `UNDO_MIGRATIONS_ABOVE_THIRTEEN`'s own
    // lesson.
    const UNDO_18: &str = "
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
            -- Migration 27's table: a rollback that leaves it in place
            -- meets `table file_claims already exists` on the re-run.
            DROP TABLE IF EXISTS file_claims;
            DELETE FROM schema_migrations WHERE version >= 18;
        ";

    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let db_path = fixture.runtime.database_path();
    let project_id = stored_project_id(&db_path);

    // Back to 17, with a row written the way a version-17 build wrote
    // them — no `failure_class` to name.
    let (schema_at_17, columns_at_17) = {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(UNDO_18).unwrap();
        conn.execute(
            "INSERT INTO routing_observations (project_id, observed_at, provider, model, outcome)
                 VALUES (?1, 1, 'pre-migration', 'm', 'failed')",
            [&project_id],
        )
        .unwrap();
        (
            whole_schema(&conn),
            columns_of(&conn, "routing_observations"),
        )
    };
    assert_eq!(schema_version(&db_path), 17, "the rollback must land on 17");
    assert!(
        !columns_at_17.iter().any(|column| column == "failure_class"),
        "{columns_at_17:?}"
    );

    // Forward: an ordinary bootstrap, exactly as a real upgrade happens.
    let migrated = fixture.rebootstrap().unwrap();
    assert_eq!(
        schema_version(&migrated.database_path()),
        SUPPORTED_SCHEMA_VERSION,
        "the launch must have applied migration 18 and everything above it"
    );
    {
        let conn = Connection::open(&db_path).unwrap();
        let columns = columns_of(&conn, "routing_observations");
        let mut expected = columns_at_17.clone();
        expected.push("failure_class".to_owned());
        // Migrations 23, 24 and 25 append to the same table, so a
        // forward run from 17 now lands nine columns rather than one.
        // All nine are asserted by name and in order, which is the
        // property this test was always about — migration 18 appended
        // `failure_class` and rebuilt nothing.
        expected.push("task_class".to_owned());
        expected.push("session_id".to_owned());
        expected.push("effort_level".to_owned());
        expected.push("turn_shape".to_owned());
        expected.push("first_byte_ms".to_owned());
        expected.push("first_token_ms".to_owned());
        expected.push("first_tool_call_ms".to_owned());
        expected.push("completed_ms".to_owned());
        assert_eq!(columns, expected, "exactly nine columns, all appended");
    }

    // The pre-migration row reads as *unknown kind*, never as a class,
    // and a row written now carries the class it was given.
    {
        let ledger = EvidenceLedger::open(&migrated).unwrap();
        let pre = ledger
            .recent(
                ObservationQuery {
                    provider: "pre-migration",
                    model: "m",
                    route: None,
                    harness: None,
                },
                1,
            )
            .unwrap();
        assert_eq!(pre.len(), 1);
        assert_eq!(pre[0].outcome, Some(Outcome::Failed));
        assert_eq!(
            pre[0].failure_class, None,
            "a row from before the column existed has no kind, not an `unknown` kind"
        );

        ledger
            .record(
                NewObservation::new("post-migration", "m")
                    .with_outcome(Outcome::Failed)
                    .with_failure_class(Some(FailureClass::Throttle)),
                2,
            )
            .unwrap();
        let post = ledger
            .recent(
                ObservationQuery {
                    provider: "post-migration",
                    model: "m",
                    route: None,
                    harness: None,
                },
                1,
            )
            .unwrap();
        assert_eq!(post[0].failure_class, Some(FailureClass::Throttle));
    }

    // Back again: the whole schema is what it was at 17, byte for byte,
    // and the rows are still there.
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(UNDO_18).unwrap();
        assert_eq!(whole_schema(&conn), schema_at_17);
        assert_eq!(columns_of(&conn, "routing_observations"), columns_at_17);
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM routing_observations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(rows, 2, "dropping the column drops no rows");
    }
    assert_eq!(schema_version(&db_path), 17);
}

/// Migration proof for 23: a version-22 database that already holds a
/// routing observation opens, migrates to 23 adding exactly one column,
/// reads the old row's `task_class` as unnamed rather than as a class,
/// records a classified row through the real writer, reads an
/// unrecognised stored word back as `None` **rather than as an error**
/// (migration 18's one deliberate difference), and the undo takes the
/// whole schema back to exactly what it was — every table, index and
/// trigger, the two project-scope triggers included.
///
/// One connection at a time throughout (practice §65): every handle is
/// dropped before the next is opened and before the re-bootstrap.
#[test]
fn migration_23_adds_task_class_and_undoes_cleanly() {
    use crate::routing::evidence::{EvidenceLedger, NewObservation, ObservationQuery, Outcome};
    use crate::routing::request::TaskClass;

    const UNDO_23: &str = "
            ALTER TABLE routing_observations DROP COLUMN completed_ms;
            ALTER TABLE routing_observations DROP COLUMN first_tool_call_ms;
            ALTER TABLE routing_observations DROP COLUMN first_token_ms;
            ALTER TABLE routing_observations DROP COLUMN first_byte_ms;
            ALTER TABLE routing_observations DROP COLUMN turn_shape;
            ALTER TABLE routing_observations DROP COLUMN effort_level;
            ALTER TABLE routing_observations DROP COLUMN session_id;
            ALTER TABLE routing_observations DROP COLUMN task_class;
            -- Migration 27's table: a rollback that leaves it in place
            -- meets `table file_claims already exists` on the re-run.
            DROP TABLE IF EXISTS file_claims;
            DELETE FROM schema_migrations WHERE version >= 23;
        ";

    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let db_path = fixture.runtime.database_path();
    let project_id = stored_project_id(&db_path);

    // Back to 22, with a row written the way a version-22 build wrote
    // them — no `task_class` to name.
    let (schema_at_22, columns_at_22) = {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(UNDO_23).unwrap();
        conn.execute(
            "INSERT INTO routing_observations (project_id, observed_at, provider, model, outcome)
                 VALUES (?1, 1, 'pre-migration', 'm', 'succeeded')",
            [&project_id],
        )
        .unwrap();
        (
            whole_schema(&conn),
            columns_of(&conn, "routing_observations"),
        )
    };
    assert_eq!(schema_version(&db_path), 22, "the rollback must land on 22");
    assert!(
        !columns_at_22.iter().any(|column| column == "task_class"),
        "{columns_at_22:?}"
    );

    // Forward: an ordinary bootstrap, exactly as a real upgrade happens.
    let migrated = fixture.rebootstrap().unwrap();
    assert_eq!(
        schema_version(&migrated.database_path()),
        SUPPORTED_SCHEMA_VERSION,
        "the launch must have applied migration 23"
    );
    assert_eq!(
        SUPPORTED_SCHEMA_VERSION, 27,
        "a fresh database reports the version the newest migration ships"
    );
    {
        let conn = Connection::open(&db_path).unwrap();
        let columns = columns_of(&conn, "routing_observations");
        let mut expected = columns_at_22.clone();
        expected.push("task_class".to_owned());
        // Migration 24's own three and migration 25's own four ride
        // along on this bootstrap, in the order they add them; 23's
        // column is still the first appended.
        expected.push("session_id".to_owned());
        expected.push("effort_level".to_owned());
        expected.push("turn_shape".to_owned());
        expected.push("first_byte_ms".to_owned());
        expected.push("first_token_ms".to_owned());
        expected.push("first_tool_call_ms".to_owned());
        expected.push("completed_ms".to_owned());
        assert_eq!(
            columns, expected,
            "23's column, then 24's three, then 25's four, appended"
        );
    }

    // The pre-migration row names no class; a row written now carries
    // the class it was given.
    {
        let ledger = EvidenceLedger::open(&migrated).unwrap();
        let query = |provider| ObservationQuery {
            provider,
            model: "m",
            route: None,
            harness: None,
        };
        let pre = ledger.recent(query("pre-migration"), 1).unwrap();
        assert_eq!(pre.len(), 1);
        assert_eq!(
            pre[0].task_class, None,
            "a row from before the column existed names no class, not an `unknown` class"
        );

        ledger
            .record(
                NewObservation::new("post-migration", "m")
                    .with_outcome(Outcome::Succeeded)
                    .with_task_class(Some(TaskClass::CodeModification)),
                2,
            )
            .unwrap();
        let post = ledger.recent(query("post-migration"), 1).unwrap();
        assert_eq!(post[0].task_class, Some(TaskClass::CodeModification));
    }

    // A word this build does not recognise reads back as *no class*, and
    // — the property that separates this column from `failure_class` —
    // the row itself still reads. An `UnknownValue` here would let a
    // future build's sixth class break this build's burn rate.
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute(
            "INSERT INTO routing_observations
                     (project_id, observed_at, provider, model, outcome, task_class)
                 VALUES (?1, 3, 'future-build', 'm', 'succeeded', 'quantum tinkering')",
            [&project_id],
        )
        .unwrap();
    }
    {
        let ledger = EvidenceLedger::open(&migrated).unwrap();
        let future = ledger
            .recent(
                ObservationQuery {
                    provider: "future-build",
                    model: "m",
                    route: None,
                    harness: None,
                },
                1,
            )
            .unwrap();
        assert_eq!(future.len(), 1, "the row reads, it does not error");
        assert_eq!(future[0].task_class, None);
        assert_eq!(future[0].outcome, Some(Outcome::Succeeded));
    }

    // Project isolation survives 23 → 22 → 23: `ADD COLUMN` does not
    // drop a trigger, and neither does `DROP COLUMN`, but the schema
    // comparison below is what proves it rather than the claim.
    {
        let conn = Connection::open(&db_path).unwrap();
        let refused = conn.execute(
            "INSERT INTO routing_observations (project_id, observed_at, provider, model)
                 VALUES ('another-project', 4, 'p', 'm')",
            [],
        );
        assert!(
            refused.is_err(),
            "the foreign-project trigger must still refuse after 22 → 23"
        );
    }

    // Back again: the whole schema is what it was at 22, byte for byte,
    // and the rows are still there.
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(UNDO_23).unwrap();
        assert_eq!(whole_schema(&conn), schema_at_22);
        assert_eq!(columns_of(&conn, "routing_observations"), columns_at_22);
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM routing_observations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(rows, 3, "dropping the column drops no rows");
        let refused = conn.execute(
            "INSERT INTO routing_observations (project_id, observed_at, provider, model)
                 VALUES ('another-project', 5, 'p', 'm')",
            [],
        );
        assert!(
            refused.is_err(),
            "the foreign-project trigger must still refuse after 23 → 22"
        );
    }
    assert_eq!(schema_version(&db_path), 22);
}

/// Migration proof for 24: a version-23 database with a row written the
/// way a version-23 build wrote them opens, migrates to 24 adding
/// exactly three columns in the order the migration names them, reads
/// that pre-migration row back with `NULL` in all three, records a
/// stamped row through the real writer, reads an unrecognised
/// `effort_level` and an unrecognised `turn_shape` back as `None`
/// **rather than as an error** (migration 23's rule, not 18's), and the
/// undo takes the whole schema back to exactly what it was — every
/// table, index and trigger, the two project-scope triggers included.
///
/// One connection at a time throughout (practice §65).
#[test]
fn migration_24_adds_the_session_columns_and_undoes_cleanly() {
    use crate::routing::evidence::{
        EffortLevel, EvidenceLedger, NewObservation, ObservationQuery, Outcome, TurnShape,
    };

    const UNDO_24: &str = "
            ALTER TABLE routing_observations DROP COLUMN completed_ms;
            ALTER TABLE routing_observations DROP COLUMN first_tool_call_ms;
            ALTER TABLE routing_observations DROP COLUMN first_token_ms;
            ALTER TABLE routing_observations DROP COLUMN first_byte_ms;
            ALTER TABLE routing_observations DROP COLUMN turn_shape;
            ALTER TABLE routing_observations DROP COLUMN effort_level;
            ALTER TABLE routing_observations DROP COLUMN session_id;
            -- Migration 27's table: a rollback that leaves it in place
            -- meets `table file_claims already exists` on the re-run.
            DROP TABLE IF EXISTS file_claims;
            DELETE FROM schema_migrations WHERE version >= 24;
        ";

    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let db_path = fixture.runtime.database_path();
    let project_id = stored_project_id(&db_path);

    // Back to 23, with a row written the way a version-23 build wrote
    // them — no session, no effort and no shape to name.
    let (schema_at_23, columns_at_23) = {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(UNDO_24).unwrap();
        conn.execute(
            "INSERT INTO routing_observations (project_id, observed_at, provider, model, outcome)
                 VALUES (?1, 1, 'pre-migration', 'm', 'succeeded')",
            [&project_id],
        )
        .unwrap();
        (
            whole_schema(&conn),
            columns_of(&conn, "routing_observations"),
        )
    };
    assert_eq!(schema_version(&db_path), 23, "the rollback must land on 23");
    for column in ["session_id", "effort_level", "turn_shape"] {
        assert!(
            !columns_at_23.iter().any(|held| held == column),
            "{columns_at_23:?}"
        );
    }

    // Forward: an ordinary bootstrap, exactly as a real upgrade happens.
    let migrated = fixture.rebootstrap().unwrap();
    assert_eq!(
        schema_version(&migrated.database_path()),
        SUPPORTED_SCHEMA_VERSION,
        "the launch must have applied migration 24"
    );
    assert_eq!(
        SUPPORTED_SCHEMA_VERSION, 27,
        "a fresh database reports the version the newest migration ships"
    );
    {
        let conn = Connection::open(&db_path).unwrap();
        let columns = columns_of(&conn, "routing_observations");
        let mut expected = columns_at_23.clone();
        expected.push("session_id".to_owned());
        expected.push("effort_level".to_owned());
        expected.push("turn_shape".to_owned());
        // Migration 25's own four ride along on this bootstrap, exactly
        // as 24's three ride along on migration 23's proof above.
        expected.push("first_byte_ms".to_owned());
        expected.push("first_token_ms".to_owned());
        expected.push("first_tool_call_ms".to_owned());
        expected.push("completed_ms".to_owned());
        assert_eq!(
            columns, expected,
            "exactly three columns, appended in order, then 25's four"
        );
    }

    // The pre-migration row names none of the three; a row written now
    // carries what it was given.
    {
        let ledger = EvidenceLedger::open(&migrated).unwrap();
        let query = |provider| ObservationQuery {
            provider,
            model: "m",
            route: None,
            harness: None,
        };
        let pre = ledger.recent(query("pre-migration"), 1).unwrap();
        assert_eq!(pre.len(), 1);
        assert_eq!(
            pre[0].session_id, None,
            "a row from before the column existed names no session, not an invented id"
        );
        assert_eq!(pre[0].effort_level, None);
        assert_eq!(pre[0].turn_shape, None);

        ledger
            .record(
                NewObservation::new("post-migration", "m")
                    .with_outcome(Outcome::Succeeded)
                    .with_session_id(Some("ses_planted"))
                    .with_effort_level(Some(EffortLevel::Medium))
                    .with_turn_shape(Some(TurnShape::ToolResume)),
                2,
            )
            .unwrap();
        let post = ledger.recent(query("post-migration"), 1).unwrap();
        assert_eq!(post[0].session_id.as_deref(), Some("ses_planted"));
        assert_eq!(post[0].effort_level, Some(EffortLevel::Medium));
        assert_eq!(post[0].turn_shape, Some(TurnShape::ToolResume));
    }

    // Words this build does not recognise read back as *nothing
    // recorded*, and — the property that separates these columns from
    // `failure_class` — the row itself still reads. An `UnknownValue`
    // here would let a future build's fifth effort word break this
    // build's savings readout.
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute(
            "INSERT INTO routing_observations
                     (project_id, observed_at, provider, model, outcome,
                      session_id, effort_level, turn_shape)
                 VALUES (?1, 3, 'future-build', 'm', 'succeeded',
                         'ses_future', 'transcendent', 'interpretive dance')",
            [&project_id],
        )
        .unwrap();
    }
    {
        let ledger = EvidenceLedger::open(&migrated).unwrap();
        let future = ledger
            .recent(
                ObservationQuery {
                    provider: "future-build",
                    model: "m",
                    route: None,
                    harness: None,
                },
                1,
            )
            .unwrap();
        assert_eq!(future.len(), 1, "the row reads, it does not error");
        assert_eq!(future[0].effort_level, None);
        assert_eq!(future[0].turn_shape, None);
        assert_eq!(
            future[0].session_id.as_deref(),
            Some("ses_future"),
            "the session id has no vocabulary to fail against and is returned as stored"
        );
        assert_eq!(future[0].outcome, Some(Outcome::Succeeded));
    }

    // Project isolation survives 24 → 23 → 24.
    {
        let conn = Connection::open(&db_path).unwrap();
        let refused = conn.execute(
            "INSERT INTO routing_observations (project_id, observed_at, provider, model)
                 VALUES ('another-project', 4, 'p', 'm')",
            [],
        );
        assert!(
            refused.is_err(),
            "the foreign-project trigger must still refuse after 23 → 24"
        );
    }

    // Back again: the whole schema is what it was at 23, byte for byte,
    // and the rows are still there.
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(UNDO_24).unwrap();
        assert_eq!(whole_schema(&conn), schema_at_23);
        assert_eq!(columns_of(&conn, "routing_observations"), columns_at_23);
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM routing_observations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(rows, 3, "dropping the columns drops no rows");
        let refused = conn.execute(
            "INSERT INTO routing_observations (project_id, observed_at, provider, model)
                 VALUES ('another-project', 5, 'p', 'm')",
            [],
        );
        assert!(
            refused.is_err(),
            "the foreign-project trigger must still refuse after 24 → 23"
        );
    }
    assert_eq!(schema_version(&db_path), 23);
}

/// Migration proof for 25: a version-24 database with a row written the
/// way a version-24 build wrote them opens, migrates to 25 adding
/// exactly four columns in the order the migration names them, reads
/// that pre-migration row back with `None` in all four **and with
/// `duration_ms` still answering from the seconds it does have**,
/// records a measured row through the real writer, refuses a negative
/// offset at the schema, and the undo takes the whole schema back to
/// exactly what it was — every table, index and trigger, the two
/// project-scope triggers included.
///
/// One connection at a time throughout (practice §65).
///
/// Mutation targets. `fallback-dropped`: making
/// `RoutingObservation::duration_ms` answer `None` when `completed_ms`
/// is `None` must fail the pre-migration row's assertion below.
/// `migration-missing-check`: dropping the `CHECK` from any one of the
/// four columns must fail the negative-offset refusal below.
#[test]
fn migration_25_adds_the_millisecond_offsets_and_undoes_cleanly() {
    use crate::routing::evidence::{EvidenceLedger, NewObservation, ObservationQuery, Outcome};

    const UNDO_25: &str = "
            ALTER TABLE routing_observations DROP COLUMN completed_ms;
            ALTER TABLE routing_observations DROP COLUMN first_tool_call_ms;
            ALTER TABLE routing_observations DROP COLUMN first_token_ms;
            ALTER TABLE routing_observations DROP COLUMN first_byte_ms;
            -- Migration 27's table: a rollback that leaves it in place
            -- meets `table file_claims already exists` on the re-run.
            DROP TABLE IF EXISTS file_claims;
            DELETE FROM schema_migrations WHERE version >= 25;
        ";

    const OFFSETS: [&str; 4] = [
        "first_byte_ms",
        "first_token_ms",
        "first_tool_call_ms",
        "completed_ms",
    ];

    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let db_path = fixture.runtime.database_path();
    let project_id = stored_project_id(&db_path);

    // Back to 24, with a row written the way a version-24 build wrote
    // them: both ends of the exchange in unix seconds and no offset
    // anywhere, because the columns did not exist.
    let (schema_at_24, columns_at_24) = {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(UNDO_25).unwrap();
        conn.execute(
            "INSERT INTO routing_observations
                     (project_id, observed_at, provider, model, outcome,
                      dispatched_at, completed_at)
                 VALUES (?1, 1, 'pre-migration', 'm', 'succeeded', 1000, 1007)",
            [&project_id],
        )
        .unwrap();
        (
            whole_schema(&conn),
            columns_of(&conn, "routing_observations"),
        )
    };
    assert_eq!(schema_version(&db_path), 24, "the rollback must land on 24");
    for column in OFFSETS {
        assert!(
            !columns_at_24.iter().any(|held| held == column),
            "{columns_at_24:?}"
        );
    }

    // Forward: an ordinary bootstrap, exactly as a real upgrade happens.
    let migrated = fixture.rebootstrap().unwrap();
    assert_eq!(
        schema_version(&migrated.database_path()),
        SUPPORTED_SCHEMA_VERSION,
        "the launch must have applied migration 25"
    );
    assert_eq!(
        SUPPORTED_SCHEMA_VERSION, 27,
        "a fresh database reports the version the newest migration ships"
    );
    {
        let conn = Connection::open(&db_path).unwrap();
        let columns = columns_of(&conn, "routing_observations");
        let mut expected = columns_at_24.clone();
        for column in OFFSETS {
            expected.push(column.to_owned());
        }
        assert_eq!(columns, expected, "exactly four columns, appended in order");
    }

    // The pre-migration row names none of the four — and still answers
    // `duration_ms` from the seconds it does carry, which is the whole
    // point of the fallback: every existing reader improves silently
    // where the offset exists and is unchanged where it does not.
    {
        let ledger = EvidenceLedger::open(&migrated).unwrap();
        let query = |provider| ObservationQuery {
            provider,
            model: "m",
            route: None,
            harness: None,
        };
        let pre = ledger.recent(query("pre-migration"), 1).unwrap();
        assert_eq!(pre.len(), 1);
        assert_eq!(
            pre[0].first_byte_ms, None,
            "a row from before the column existed measured nothing, and invents nothing"
        );
        assert_eq!(pre[0].first_token_ms, None);
        assert_eq!(pre[0].first_tool_call_ms, None);
        assert_eq!(pre[0].completed_ms, None);
        assert_eq!(
            pre[0].duration_ms(),
            Some(7_000),
            "with no measured completion the seconds difference is still the answer"
        );

        ledger
            .record(
                NewObservation::new("post-migration", "m")
                    .with_outcome(Outcome::Succeeded)
                    .with_timing(Some(2_000), Some(2_009))
                    .with_first_byte_ms(Some(120))
                    .with_first_token_ms(Some(1_450))
                    .with_first_tool_call_ms(Some(2_600))
                    .with_completed_ms(Some(8_910)),
                2,
            )
            .unwrap();
        let post = ledger.recent(query("post-migration"), 1).unwrap();
        assert_eq!(post[0].first_byte_ms, Some(120));
        assert_eq!(post[0].first_token_ms, Some(1_450));
        assert_eq!(post[0].first_tool_call_ms, Some(2_600));
        assert_eq!(post[0].completed_ms, Some(8_910));
        assert_eq!(
            post[0].duration_ms(),
            Some(8_910),
            "a measured completion is preferred over the 9,000ms the seconds would give"
        );
    }

    // The `CHECK` is the whole difference between these columns and
    // migrations 23 and 24's: a negative offset is not an unrecognised
    // word a later build might have meant, it is a reading no monotonic
    // clock can produce, and the schema refuses it one column at a time.
    {
        let conn = Connection::open(&db_path).unwrap();
        for column in OFFSETS {
            let refused = conn.execute(
                &format!(
                    "INSERT INTO routing_observations
                             (project_id, observed_at, provider, model, {column})
                         VALUES (?1, 3, 'negative', 'm', -1)"
                ),
                [&project_id],
            );
            assert!(
                refused.is_err(),
                "`{column}` must refuse a negative offset at the schema"
            );
        }
    }

    // Project isolation survives 24 → 25.
    {
        let conn = Connection::open(&db_path).unwrap();
        let refused = conn.execute(
            "INSERT INTO routing_observations (project_id, observed_at, provider, model)
                 VALUES ('another-project', 4, 'p', 'm')",
            [],
        );
        assert!(
            refused.is_err(),
            "the foreign-project trigger must still refuse after 24 → 25"
        );
    }

    // Back again: the whole schema is what it was at 24, byte for byte —
    // which is also the proof that each column-scoped `CHECK` went with
    // the column it was written on, migration 16's rule.
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(UNDO_25).unwrap();
        assert_eq!(whole_schema(&conn), schema_at_24);
        assert_eq!(columns_of(&conn, "routing_observations"), columns_at_24);
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM routing_observations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(rows, 2, "dropping the columns drops no rows");
        let refused = conn.execute(
            "INSERT INTO routing_observations (project_id, observed_at, provider, model)
                 VALUES ('another-project', 5, 'p', 'm')",
            [],
        );
        assert!(
            refused.is_err(),
            "the foreign-project trigger must still refuse after 25 → 24"
        );
    }
    assert_eq!(schema_version(&db_path), 24);
}

/// Migration proof for 19: a version-18 database opens, migrates to 19
/// adding exactly two tables with their indexes and triggers, accepts an
/// assumption and a transition through the real writer, refuses an edit
/// to either, and the undo takes the whole schema back to exactly what
/// it was — every table, index and trigger — with `schema_migrations`
/// at 18.
///
/// The trigger check is by name: migration 15's two scope triggers and
/// one append-only trigger per table, and **no `DELETE` trigger** — a
/// future migration that added one would quietly make a prunable ledger
/// permanent, which is the defect migration 5 documents.
///
/// One connection at a time throughout (practice §65).
#[test]
fn migration_19_adds_the_assumption_tables_and_undoes_cleanly() {
    use crate::guardrails::{
        AssumptionState, AssumptionStore, EvidenceSource, NewAssumption, NewTransition, Origin,
        Uncertainty,
    };

    // Reaches past 19, for `UNDO_18`'s reason exactly: this test rolls
    // back to 18 and lets an ordinary bootstrap migrate forward again, so
    // it must undo EVERY migration above 18. Leaving 20's column standing
    // lands an "18" that still has `presentation_ref`, and the re-bootstrap
    // fails with `duplicate column name` instead of proving anything about
    // 19. A migration 21 owes this constant its own line.
    const UNDO_19: &str = "
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
            -- Migration 27's table: a rollback that leaves it in place
            -- meets `table file_claims already exists` on the re-run.
            DROP TABLE IF EXISTS file_claims;
            DELETE FROM schema_migrations WHERE version >= 19;
        ";

    fn schema_of(conn: &Connection) -> Vec<(String, String, Option<String>)> {
        let mut statement = conn
            .prepare("SELECT type, name, sql FROM sqlite_master ORDER BY type, name")
            .unwrap();
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }
    fn names_of(conn: &Connection, kind: &str, table: &str) -> Vec<String> {
        let mut statement = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type = ?1 AND tbl_name = ?2 \
                     ORDER BY name",
            )
            .unwrap();
        statement
            .query_map([kind, table], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let db_path = fixture.runtime.database_path();

    // Back to 18.
    let schema_at_18 = {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(UNDO_19).unwrap();
        schema_of(&conn)
    };
    assert_eq!(schema_version(&db_path), 18, "the rollback must land on 18");
    assert!(
        !schema_at_18
            .iter()
            .any(|(_, name, _)| name.starts_with("task_assumptions")
                || name.starts_with("assumption_transitions")),
        "{schema_at_18:?}"
    );

    // Forward: an ordinary bootstrap, exactly as a real upgrade happens.
    let migrated = fixture.rebootstrap().unwrap();
    assert_eq!(
        schema_version(&migrated.database_path()),
        SUPPORTED_SCHEMA_VERSION,
        "the launch must have applied migration 19"
    );
    {
        let conn = Connection::open(&db_path).unwrap();
        assert_eq!(
            names_of(&conn, "table", "task_assumptions"),
            ["task_assumptions"]
        );
        assert_eq!(
            names_of(&conn, "table", "assumption_transitions"),
            ["assumption_transitions"]
        );
        assert_eq!(
            names_of(&conn, "index", "task_assumptions"),
            [
                "sqlite_autoindex_task_assumptions_1",
                "task_assumptions_by_session"
            ]
        );
        assert_eq!(
            names_of(&conn, "index", "assumption_transitions"),
            [
                "assumption_transitions_by_assumption",
                "assumption_transitions_by_session"
            ]
        );
        assert_eq!(
            names_of(&conn, "trigger", "task_assumptions"),
            [
                "task_assumptions_never_edited",
                "task_assumptions_reject_foreign_project_insert",
                "task_assumptions_reject_foreign_project_update"
            ],
            "two scope triggers and one append-only trigger, and no DELETE trigger"
        );
        assert_eq!(
            names_of(&conn, "trigger", "assumption_transitions"),
            [
                "assumption_transitions_append_only",
                "assumption_transitions_reject_foreign_project_insert",
                "assumption_transitions_reject_foreign_project_update"
            ]
        );
    }

    // The real writer, through the migrated schema.
    let recorded = {
        let mut store = AssumptionStore::open(&migrated).unwrap();
        let record = store
            .record(NewAssumption {
                session: Some("s1".to_owned()),
                claim: "written through migration 19".to_owned(),
                evidence: "this test".to_owned(),
                evidence_source: EvidenceSource::Experiment,
                uncertainty: Uncertainty::Low,
                affected: "database.rs".to_owned(),
                verification: "the undo below".to_owned(),
                origin: Origin::Agent,
            })
            .unwrap();
        let moved = store
            .transition(
                &record.id,
                NewTransition::to(AssumptionState::Refuted, Origin::Agent),
            )
            .unwrap();
        assert_eq!(moved.state, Some(AssumptionState::Refuted));
        assert_eq!(
            store.get(&record.id).unwrap().unwrap().state,
            AssumptionState::Refuted
        );
        record
    };
    {
        let conn = Connection::open(&db_path).unwrap();
        let err = conn
            .execute(
                "UPDATE assumption_transitions SET state = 'supported' WHERE assumption_id = ?1",
                [recorded.id.as_str()],
            )
            .unwrap_err();
        assert!(err.to_string().contains("append-only"), "{err}");
        let err = conn
            .execute(
                "UPDATE task_assumptions SET claim = 'edited' WHERE id = ?1",
                [recorded.id.as_str()],
            )
            .unwrap_err();
        assert!(err.to_string().contains("never edited"), "{err}");
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM assumption_transitions", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(rows, 2, "the first state and the move");
    }

    // Back again: the whole schema is what it was at 18, byte for byte.
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(UNDO_19).unwrap();
        assert_eq!(schema_of(&conn), schema_at_18);
    }
    assert_eq!(schema_version(&db_path), 18);
}

/// Migration proof for 20: a version-19 database that already holds a
/// session written the way a version-19 build wrote it migrates forward
/// adding exactly one column, appended; the old row reads as *no pane
/// recorded*, never as an empty or invented reference; a row written now
/// carries the reference it was given; and the undo takes the whole
/// schema back to exactly what it was at 19, keeping every row.
///
/// The `None` assertion is the one this migration most needs: a column
/// written `NOT NULL DEFAULT ''` would pass every other check here and
/// would hand `integrations::cmux::PaneRef::parse` an empty string for
/// every session recorded before the upgrade.
#[test]
fn migration_20_adds_presentation_ref_and_undoes_cleanly() {
    use crate::session::{NewSession, ProjectSessions, SessionId, SessionPresentation};

    const UNDO_20: &str = "
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
            -- Migration 27's table: a rollback that leaves it in place
            -- meets `table file_claims already exists` on the re-run.
            DROP TABLE IF EXISTS file_claims;
            DELETE FROM schema_migrations WHERE version >= 20;
        ";

    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let db_path = fixture.runtime.database_path();
    let project_id = stored_project_id(&db_path);

    // Back to 19, with a row written the way a version-19 build wrote
    // them — no `presentation_ref` to name.
    let (schema_at_19, columns_at_19) = {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(UNDO_20).unwrap();
        conn.execute(
            "INSERT INTO sessions (id, project_id, harness, role, lifecycle, \
                 presentation, created_at, last_activity_at) \
                 VALUES ('pre-migration', ?1, 'claude-code', 'normal', 'stopped', \
                 'external', 1, 1)",
            [&project_id],
        )
        .unwrap();
        (whole_schema(&conn), columns_of(&conn, "sessions"))
    };
    assert_eq!(schema_version(&db_path), 19, "the rollback must land on 19");
    assert!(
        !columns_at_19
            .iter()
            .any(|column| column == "presentation_ref"),
        "{columns_at_19:?}"
    );

    // Forward: an ordinary bootstrap, exactly as a real upgrade happens.
    let migrated = fixture.rebootstrap().unwrap();
    assert_eq!(
        schema_version(&migrated.database_path()),
        SUPPORTED_SCHEMA_VERSION,
        "the launch must have applied migration 20"
    );
    {
        let conn = Connection::open(&db_path).unwrap();
        let columns = columns_of(&conn, "sessions");
        let mut expected = columns_at_19.clone();
        expected.push("presentation_ref".to_owned());
        // A prefix, not an equality: the bootstrap runs every migration
        // above 19, and 21 appends `last_seen_commit` after this one. What
        // this migration owns is that ITS column is the first appended.
        assert_eq!(
            &columns[..expected.len()],
            &expected[..],
            "exactly one column from this migration, appended"
        );
    }

    // The pre-migration row reads as *no pane recorded*, and a row
    // written now carries the reference it was given — through the real
    // store, which is the only writer.
    {
        let sessions = ProjectSessions::open(&migrated).unwrap();
        let store = sessions.store();
        let pre = store
            .get(&SessionId::new("pre-migration"))
            .unwrap()
            .expect("the pre-migration row survives");
        assert_eq!(pre.presentation, SessionPresentation::External);
        assert_eq!(
            pre.presentation_ref, None,
            "a row from before the column existed has no pane, not an empty one"
        );

        let post = store
            .create(
                NewSession::embedded("claude-code")
                    .with_presentation(SessionPresentation::External)
                    .with_presentation_ref(Some("workspace:349".to_owned())),
            )
            .unwrap();
        let read_back = store.get(&post.id).unwrap().unwrap();
        assert_eq!(read_back.presentation_ref.as_deref(), Some("workspace:349"));
    }

    // Back again: the whole schema is what it was at 19, byte for byte,
    // and both rows are still there.
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(UNDO_20).unwrap();
        assert_eq!(whole_schema(&conn), schema_at_19);
        assert_eq!(columns_of(&conn, "sessions"), columns_at_19);
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 2, "dropping the column drops no rows");
    }
    assert_eq!(schema_version(&db_path), 19);
}

/// Migration proof for 21: a version-20 database holding a session and a
/// memory opens, migrates to 21 adding exactly one column to each of two
/// tables, reads both pre-migration rows as *nothing recorded* rather
/// than as a value, accepts a position and a trigger written through the
/// real writers, and the undo takes the whole schema back to exactly what
/// it was — every table, index and trigger.
///
/// Named for what the migration does rather than for its number: the
/// number is whatever position this script ends up in once the migrations
/// being written beside it land, and a test name that had to be renumbered
/// to stay true is a name nobody would renumber.
///
/// One connection at a time throughout (practice §65): every handle is
/// dropped before the next is opened and before the re-bootstrap.
#[test]
fn the_memory_commit_migration_adds_its_two_columns_and_undoes_cleanly() {
    use crate::memory::{MemoryKind, NewMemory, ProjectMemory};
    use crate::session::{NewSession, ProjectSessions};

    const UNDO: &str = "
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
            -- Migration 27's table: a rollback that leaves it in place
            -- meets `table file_claims already exists` on the re-run.
            DROP TABLE IF EXISTS file_claims;
            DELETE FROM schema_migrations WHERE version >= 21;
        ";

    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let db_path = fixture.runtime.database_path();
    let project_id = stored_project_id(&db_path);

    // Back to 20, with a session row and a memory row written the way a
    // version-18 build wrote them — neither has a column to name.
    let (schema_at_18, session_columns_at_18, memory_columns_at_18) = {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(UNDO).unwrap();
        conn.execute(
            "INSERT INTO sessions (id, project_id, harness, role, lifecycle, \
                 presentation, created_at, last_activity_at) \
                 VALUES (\'pre-migration\', ?1, \'claude-code\', \'normal\', \'idle\', \
                 \'embedded\', 1, 1)",
            [&project_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO memories (id, project_id, kind, status, body, created_at, updated_at) \
                 VALUES (\'premigration0000\', ?1, \'finding\', \'active\', \
                 \'learned before the column existed\', 1, 1)",
            [&project_id],
        )
        .unwrap();
        (
            whole_schema(&conn),
            columns_of(&conn, "sessions"),
            columns_of(&conn, "memories"),
        )
    };
    assert_eq!(schema_version(&db_path), 20, "the rollback must land on 20");
    assert!(
        !session_columns_at_18
            .iter()
            .any(|column| column == "last_seen_commit"),
        "{session_columns_at_18:?}"
    );
    assert!(
        !memory_columns_at_18
            .iter()
            .any(|column| column == "extraction_trigger"),
        "{memory_columns_at_18:?}"
    );

    // Forward: an ordinary bootstrap, exactly as a real upgrade happens.
    let migrated = fixture.rebootstrap().unwrap();
    assert_eq!(
        schema_version(&migrated.database_path()),
        SUPPORTED_SCHEMA_VERSION,
        "the launch must have applied the memory-commit migration"
    );
    {
        let conn = Connection::open(&db_path).unwrap();
        let mut expected = session_columns_at_18.clone();
        expected.push("last_seen_commit".to_owned());
        // Migration 22 runs in the same forward pass and appends its own
        // column after this one. Named here rather than left out: this
        // assertion is about *append-only*, and a migration that
        // reordered or rebuilt the table is exactly what it must catch.
        expected.push("entitlement".to_owned());
        assert_eq!(
            columns_of(&conn, "sessions"),
            expected,
            "one column per migration, each appended"
        );
        let mut expected = memory_columns_at_18.clone();
        expected.push("extraction_trigger".to_owned());
        assert_eq!(
            columns_of(&conn, "memories"),
            expected,
            "exactly one column, appended"
        );
    }

    // Both pre-migration rows read as *nothing recorded*, never as a
    // value, and rows written now carry what the real writers gave them.
    {
        let sessions = ProjectSessions::open(&migrated).unwrap();
        let store = sessions.store();
        let pre = store
            .get(&crate::session::SessionId::new("pre-migration"))
            .unwrap()
            .expect("the pre-migration session survived");
        assert_eq!(
            pre.last_seen_commit, None,
            "a row from before the column existed has seen no HEAD, \
                 not an empty one"
        );

        let fresh = store.create(NewSession::embedded("claude-code")).unwrap();
        assert_eq!(
            fresh.last_seen_commit, None,
            "a session Glasshouse just created has not looked at HEAD either"
        );
        let noted = store
            .record_seen_commit(&fresh.id, "0123456789abcdef0123456789abcdef01234567")
            .unwrap();
        assert_eq!(
            noted.last_seen_commit.as_deref(),
            Some("0123456789abcdef0123456789abcdef01234567")
        );
    }
    {
        let memory = ProjectMemory::open(&migrated).unwrap();
        let store = memory.store();
        let pre = store
            .get(&crate::memory::MemoryId::new("premigration0000"))
            .unwrap()
            .expect("the pre-migration memory survived");
        assert_eq!(
            pre.extraction_trigger, None,
            "a row from before the column existed has no trigger, \
                 not an `unknown` one"
        );

        let recorded = store
            .record(
                NewMemory::new(MemoryKind::Finding, "learned at a code-change boundary")
                    .with_extraction_trigger(Some("git_commit")),
            )
            .unwrap();
        assert_eq!(recorded.extraction_trigger.as_deref(), Some("git_commit"));
    }

    // Back again: the whole schema is what it was at 20, and the rows are
    // still there.
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(UNDO).unwrap();
        assert_eq!(whole_schema(&conn), schema_at_18);
        assert_eq!(columns_of(&conn, "sessions"), session_columns_at_18);
        assert_eq!(columns_of(&conn, "memories"), memory_columns_at_18);
        let sessions: i64 = conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
            .unwrap();
        let memories: i64 = conn
            .query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))
            .unwrap();
        assert_eq!(sessions, 2, "dropping the column drops no rows");
        assert_eq!(memories, 2, "dropping the column drops no rows");
    }
    assert_eq!(schema_version(&db_path), 20);
}

/// Migration proof for migration 22: a version-21 database holding a
/// session written the way a version-21 build wrote it opens, migrates to
/// 22 adding exactly one column — appended, never a rebuild — reads the
/// pre-migration row as *no account recorded* rather than as a name,
/// carries a name written through the real writer, and the undo takes the
/// whole schema back to exactly what it was at 21, keeping every row.
///
/// The `None` assertion is the one this migration most needs, and it is
/// migration 20's lesson repeated: a column written `NOT NULL DEFAULT ''`
/// would pass every other check here and would then tell `glasshouse
/// entitlements` that every session recorded before the upgrade was
/// served by an entitlement named by the empty string. *Nothing recorded*
/// and *an account* are different facts, and only `NULL` keeps them apart.
///
/// Named for what the migration does rather than for its number, for the
/// reason the memory-commit proof above states.
///
/// One connection at a time throughout (practice §65): every handle is
/// dropped before the next is opened and before the re-bootstrap.
#[test]
fn the_entitlement_migration_adds_its_column_and_undoes_cleanly() {
    use crate::session::{NewSession, ProjectSessions};

    const UNDO: &str = "
            ALTER TABLE routing_observations DROP COLUMN completed_ms;
            ALTER TABLE routing_observations DROP COLUMN first_tool_call_ms;
            ALTER TABLE routing_observations DROP COLUMN first_token_ms;
            ALTER TABLE routing_observations DROP COLUMN first_byte_ms;
            ALTER TABLE routing_observations DROP COLUMN turn_shape;
            ALTER TABLE routing_observations DROP COLUMN effort_level;
            ALTER TABLE routing_observations DROP COLUMN session_id;
            ALTER TABLE routing_observations DROP COLUMN task_class;
            ALTER TABLE sessions DROP COLUMN entitlement;
            -- Migration 27's table: a rollback that leaves it in place
            -- meets `table file_claims already exists` on the re-run.
            DROP TABLE IF EXISTS file_claims;
            DELETE FROM schema_migrations WHERE version >= 22;
        ";

    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let db_path = fixture.runtime.database_path();
    let project_id = stored_project_id(&db_path);

    // Back to 21, with a session row written the way a version-21 build
    // wrote it: it has no column to name an account in.
    let (schema_at_21, session_columns_at_21) = {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(UNDO).unwrap();
        conn.execute(
            "INSERT INTO sessions (id, project_id, harness, role, lifecycle, \
                 presentation, created_at, last_activity_at, backend_resource) \
                 VALUES (\'pre-migration\', ?1, \'claude-code\', \'normal\', \'idle\', \
                 \'embedded\', 1, 1, \'direct-provider:alpha-probe\')",
            [&project_id],
        )
        .unwrap();
        (whole_schema(&conn), columns_of(&conn, "sessions"))
    };
    assert_eq!(schema_version(&db_path), 21, "the rollback must land on 21");
    assert!(
        !session_columns_at_21
            .iter()
            .any(|column| column == "entitlement"),
        "{session_columns_at_21:?}"
    );

    // Forward: an ordinary bootstrap, exactly as a real upgrade happens.
    let migrated = fixture.rebootstrap().unwrap();
    assert_eq!(
        schema_version(&migrated.database_path()),
        SUPPORTED_SCHEMA_VERSION,
        "the launch must have applied the entitlement migration"
    );
    assert_eq!(
        SUPPORTED_SCHEMA_VERSION, 27,
        "a fresh database reports the version the newest migration ships"
    );
    {
        let conn = Connection::open(&db_path).unwrap();
        let mut expected = session_columns_at_21.clone();
        expected.push("entitlement".to_owned());
        assert_eq!(
            columns_of(&conn, "sessions"),
            expected,
            "exactly one column, appended"
        );
    }

    // The pre-migration row reads as *nothing recorded*, and a row
    // written now carries what the real writer gave it.
    {
        let sessions = ProjectSessions::open(&migrated).unwrap();
        let store = sessions.store();
        let pre = store
            .get(&crate::session::SessionId::new("pre-migration"))
            .unwrap()
            .expect("the pre-migration session survived");
        assert_eq!(
            pre.entitlement, None,
            "a row from before the column existed was served by no account                  this build can name — not by one named the empty string"
        );
        assert_eq!(
            pre.backend_resource.as_deref(),
            Some("direct-provider:alpha-probe"),
            "and everything the old row did hold survived the upgrade"
        );

        let fresh = store.create(NewSession::embedded("claude-code")).unwrap();
        assert_eq!(
            fresh.entitlement, None,
            "a session created without one records no account, never a guess"
        );
        let named = store
            .create(
                NewSession::embedded("claude-code").with_entitlement(Some("claude-b".to_owned())),
            )
            .unwrap();
        assert_eq!(named.entitlement.as_deref(), Some("claude-b"));
        assert_eq!(
            store
                .get(&named.id)
                .unwrap()
                .expect("the session was recorded")
                .entitlement
                .as_deref(),
            Some("claude-b"),
            "and it survives the round trip through the column"
        );
    }

    // Back again: the whole schema is what it was at 21, and every row is
    // still there.
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(UNDO).unwrap();
        assert_eq!(whole_schema(&conn), schema_at_21);
        assert_eq!(columns_of(&conn, "sessions"), session_columns_at_21);
        let sessions: i64 = conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(sessions, 3, "dropping the column drops no rows");
    }
    assert_eq!(schema_version(&db_path), 21);
}

/// Migration proof for migration 17: a version-16 database opens,
/// migrates to 17, keeps every memory it had, and comes out with a table
/// that accepts an association — plus the index and the two triggers, and
/// nothing else.
///
/// The trigger check is by name and by behaviour: migration 5's three
/// append-only triggers are deliberately **not** copied here, so a future
/// migration that adds one fails this rather than quietly making a
/// prunable table permanent.
#[test]
fn a_version_sixteen_database_migrates_forward_keeping_its_memories() {
    use crate::memory::{MemoryKind, NewMemory, ProjectMemory};

    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let recorded = {
        let memory = ProjectMemory::open(&fixture.runtime).unwrap();
        memory
            .store()
            .record(NewMemory::new(
                MemoryKind::Finding,
                "a memory written before migration 17 existed",
            ))
            .unwrap()
    };

    let db_path = fixture.runtime.database_path();
    {
        let conn = Connection::open(&db_path).unwrap();
        // Migrations 19 and 18 are undone first: a rollback undoes
        // **every** migration above the version it claims, or the
        // re-run fails — `UNDO_MIGRATIONS_ABOVE_THIRTEEN`'s own lesson.
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
                 -- Migration 27's table: a rollback that leaves it in place
                 -- meets `table file_claims already exists` on the re-run.
                 DROP TABLE IF EXISTS file_claims;
                 DELETE FROM schema_migrations WHERE version >= 17;",
        )
        .unwrap();
    }
    assert_eq!(
        schema_version(&db_path),
        16,
        "the rollback must land on version 16"
    );

    let migrated = fixture.rebootstrap().unwrap();
    assert_eq!(
        schema_version(&migrated.database_path()),
        SUPPORTED_SCHEMA_VERSION,
        "the launch must have applied migration 17"
    );

    // The memory that predates the table is still there, and reads back
    // with no associations — which is the truth about it.
    let memory = ProjectMemory::open(&migrated).unwrap();
    let store = memory.store();
    assert_eq!(
        store.get(&recorded.id).unwrap().map(|found| found.body),
        Some(recorded.body.clone())
    );

    let conn = Connection::open(migrated.database_path()).unwrap();
    let associations: i64 = conn
        .query_row("SELECT COUNT(*) FROM memory_files", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        associations, 0,
        "a memory recorded before this migration has no associations to invent"
    );

    // The index the table exists for.
    let index: Option<String> = conn
            .query_row(
                "SELECT name FROM sqlite_master                   WHERE type = 'index' AND name = 'memory_files_by_path'",
                [],
                |row| row.get(0),
            )
            .optional()
            .unwrap();
    assert_eq!(index.as_deref(), Some("memory_files_by_path"));

    // Migration 11's two triggers, and nothing else — no append-only
    // trigger, so this table stays prunable.
    let mut statement = conn
            .prepare(
                "SELECT name FROM sqlite_master                   WHERE type = 'trigger' AND tbl_name = 'memory_files' ORDER BY name",
            )
            .unwrap();
    let triggers: Vec<String> = statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        triggers,
        vec![
            "memory_files_reject_foreign_project_insert".to_owned(),
            "memory_files_reject_foreign_project_update".to_owned(),
        ],
        "migration 11's two project-scope triggers, and nothing else"
    );
    drop(statement);

    // And the table really is prunable, behaviourally — unlike
    // `lifecycle_events`, whose BEFORE DELETE trigger aborts.
    store
        .record_observed_files(
            std::slice::from_ref(&recorded.id),
            &["src/example.rs".to_owned()],
        )
        .unwrap();
    let removed = conn.execute("DELETE FROM memory_files", []).unwrap();
    assert_eq!(removed, 1);
}

/// Migration 10's `CHECK` on `review_reason` is the only definition of
/// Phase 21C's six review reasons — map lines 885-890, one value per
/// line, in order. Modeled on
/// `every_project_phase_the_type_supports_is_one_the_schema_accepts`,
/// reading the list **out of the migration itself** rather than out of a
/// second constant that could drift from it.
#[test]
fn every_review_reason_the_type_supports_is_one_the_schema_accepts() {
    use crate::memory::ReviewReason;

    let migration = MIGRATIONS_V1_TO_V13[9];
    let marker = "review_reason IN";
    let open = migration
        .find(marker)
        .expect("migration 10 checks review_reason")
        + marker.len();
    let list = &migration[open..];
    let list = &list[..list.find(')').expect("the CHECK's list is parenthesised")];
    let accepted: Vec<String> = list
        .split(',')
        .map(|value| value.trim().trim_matches(['(', ' ', '\n', '\'']).to_owned())
        .filter(|value| !value.is_empty())
        .collect();

    let declared: Vec<String> = ReviewReason::ALL
        .iter()
        .map(|reason| reason.as_str().to_owned())
        .collect();

    assert_eq!(
        declared, accepted,
        "a review reason was added or renamed without migration 10's CHECK, \
             or the two fell out of the map's own order"
    );
    assert_eq!(accepted.len(), 6, "the CHECK's list was not read correctly");
}

/// Migration proof (a): a version-9 database opens, migrates to 10, and
/// keeps every existing row — including a memory recorded before any of
/// Phase 21C's columns existed.
#[test]
fn a_version_nine_database_migrates_forward_keeping_its_memories() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = crate::memory::ProjectMemory::open(&fixture.runtime).unwrap();
    let store = memory.store();

    let pre_existing = store
        .record(crate::memory::NewMemory::new(
            crate::memory::MemoryKind::Decision,
            "amethyst decisions predate migration 10",
        ))
        .unwrap();
    drop(store);
    drop(memory);

    let db_path = fixture.runtime.database_path();
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(&format!(
            "{UNDO_MIGRATIONS_ABOVE_THIRTEEN}
                 ALTER TABLE sessions DROP COLUMN source_session_id;
                 DROP TABLE routing_observations;

                 ALTER TABLE memories DROP COLUMN superseded_reason;
                 ALTER TABLE memories DROP COLUMN validity_conditions;
                 ALTER TABLE memories DROP COLUMN invalidation_conditions;
                 ALTER TABLE memories DROP COLUMN review_reason;
                 ALTER TABLE memories DROP COLUMN review_marked_at;
                 ALTER TABLE memories DROP COLUMN last_validated_at;

                 -- Migration 27's table: a rollback that leaves it in place
                 -- meets `table file_claims already exists` on the re-run.

                 DROP TABLE IF EXISTS file_claims;

                 DELETE FROM schema_migrations WHERE version >= 10;"
        ))
        .unwrap();
    }

    assert_eq!(
        schema_version(&db_path),
        9,
        "the rollback must land on version 9"
    );

    // The next launch is an ordinary bootstrap; nothing special is asked
    // of it, matching the way a real upgrade happens.
    let migrated = fixture.rebootstrap().unwrap();
    assert_eq!(
        schema_version(&migrated.database_path()),
        SUPPORTED_SCHEMA_VERSION,
        "the launch must have applied migration 10"
    );

    let reopened = crate::memory::ProjectMemory::open(&migrated).unwrap();
    let intact = reopened
        .store()
        .get(&pre_existing.id)
        .unwrap()
        .expect("the pre-migration memory must survive the upgrade");
    assert_eq!(intact.body, pre_existing.body);

    // Migration proof (c): a pre-migration row's `last_validated_at`
    // reads as unknown, not as zero — the row existed before the column
    // did, so `ALTER TABLE ADD COLUMN` backfills it with `NULL`, and
    // `row_to_record` must not substitute a default for that `NULL`.
    assert_eq!(
        intact.last_validated_at, None,
        "a pre-migration memory's last_validated_at must read as unknown, not as zero"
    );
    assert_eq!(intact.review_reason, None);
    assert_eq!(intact.review_marked_at, None);
}

/// Migration proof (a) for migration 13: a version-12 database opens,
/// migrates to 13, and keeps every existing row — including a memory that
/// was **already superseded** before the reason column existed.
///
/// That last part is the one worth having. A pre-migration supersession is
/// the population line 925's column can never fill in, so it has to read
/// back as *"no reason recorded"* rather than as anything invented, and
/// the supersession itself has to survive intact.
#[test]
fn a_version_twelve_database_migrates_forward_keeping_a_supersession_it_could_not_explain() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = crate::memory::ProjectMemory::open(&fixture.runtime).unwrap();
    let store = memory.store();

    let old = store
        .record(crate::memory::NewMemory::new(
            crate::memory::MemoryKind::Decision,
            "obsidian decisions predate migration 13",
        ))
        .unwrap();
    let replacement = store
        .record(crate::memory::NewMemory::new(
            crate::memory::MemoryKind::Decision,
            "obsidian's successor",
        ))
        .unwrap();
    let superseded = store.supersede(&old.id, &replacement.id).unwrap();
    assert_eq!(superseded.superseded_by.as_ref(), Some(&replacement.id));
    drop(store);
    drop(memory);

    let db_path = fixture.runtime.database_path();
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(&format!(
            "{UNDO_MIGRATIONS_ABOVE_THIRTEEN}
                 ALTER TABLE memories DROP COLUMN superseded_reason;
                 -- Migration 27's table: a rollback that leaves it in place
                 -- meets `table file_claims already exists` on the re-run.
                 DROP TABLE IF EXISTS file_claims;
                 DELETE FROM schema_migrations WHERE version >= 13;"
        ))
        .unwrap();
    }

    assert_eq!(
        schema_version(&db_path),
        12,
        "the rollback must land on version 12"
    );

    // The next launch is an ordinary bootstrap; nothing special is asked
    // of it, matching the way a real upgrade happens.
    let migrated = fixture.rebootstrap().unwrap();
    assert_eq!(
        schema_version(&migrated.database_path()),
        SUPPORTED_SCHEMA_VERSION,
        "the launch must have applied migration 13"
    );

    let reopened = crate::memory::ProjectMemory::open(&migrated).unwrap();
    let intact = reopened
        .store()
        .get(&old.id)
        .unwrap()
        .expect("the pre-migration memory must survive the upgrade");
    assert_eq!(intact.body, old.body);
    assert_eq!(
        intact.status,
        crate::memory::MemoryStatus::Superseded,
        "the supersession recorded before the column existed must survive it"
    );
    assert_eq!(intact.superseded_by.as_ref(), Some(&replacement.id));
    assert_eq!(
        intact.superseded_reason, None,
        "a supersession recorded before migration 13 has no reason, and must not acquire an \
             invented one"
    );
    // The successor is untouched by any of it.
    let successor = reopened
        .store()
        .get(&replacement.id)
        .unwrap()
        .expect("the successor must survive the upgrade");
    assert_eq!(successor.superseded_reason, None);
    assert_eq!(successor.status, crate::memory::MemoryStatus::Active);
}

/// Migration proof for migration 14: a version-13 database opens, migrates
/// to 14, and keeps every checkpoint it had — in the order it could
/// actually record, and admitting the order it never could.
///
/// The three checkpoints are written into **two seconds**: two into the
/// first and one into the second. That split is the whole test. The
/// between-second order was recorded in `created_at` and must survive
/// exactly; the within-second order was recorded nowhere, so the backfill
/// cannot recover it and must not invent it — what it owes instead is the
/// answer the old query already gave, which is `id` order, so that a
/// database that migrates does not silently change an answer it had
/// already given the user.
#[test]
fn a_version_thirteen_database_migrates_forward_keeping_the_order_it_could_record() {
    use crate::checkpoint::{CheckpointReason, ProjectCheckpoints};
    use crate::session::SessionId;

    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let session = SessionId::new("session-a");

    // Two in the first second, one in the second — through `save`, so the
    // rows are exactly what a version-13 build would have left behind
    // apart from the column that is about to be removed.
    let checkpoints = ProjectCheckpoints::open(&fixture.runtime).unwrap();
    let store = checkpoints.store();
    let earlier_a = store
        .save(sample_checkpoint(&session, 1_000, CheckpointReason::Manual))
        .unwrap();
    let earlier_b = store
        .save(sample_checkpoint(&session, 1_000, CheckpointReason::Manual))
        .unwrap();
    let later = store
        .save(sample_checkpoint(
            &session,
            2_000,
            CheckpointReason::TaskBoundary,
        ))
        .unwrap();
    drop(store);
    drop(checkpoints);

    let db_path = fixture.runtime.database_path();
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(&format!(
            "{UNDO_MIGRATIONS_ABOVE_THIRTEEN}
                 -- Migration 27's table: a rollback that leaves it in place
                 -- meets `table file_claims already exists` on the re-run.
                 DROP TABLE IF EXISTS file_claims;
                 DELETE FROM schema_migrations WHERE version >= 14;"
        ))
        .unwrap();
    }
    assert_eq!(
        schema_version(&db_path),
        13,
        "the rollback must land on version 13"
    );

    let migrated = fixture.rebootstrap().unwrap();
    assert_eq!(
        schema_version(&migrated.database_path()),
        SUPPORTED_SCHEMA_VERSION,
        "the launch must have applied migration 14"
    );

    let reopened = ProjectCheckpoints::open(&migrated).unwrap();
    let store = reopened.store();

    // Nothing was lost, and every document still parses.
    let listed = store.list().unwrap();
    assert_eq!(listed.len(), 3, "the migration must keep every checkpoint");

    // The between-second order survives: the later checkpoint is still
    // the later one, both in the listing and in the resolution.
    assert_eq!(
        store.latest_for(&session).unwrap().unwrap().id,
        later.id,
        "a checkpoint written a second after the others must still resolve as the latest"
    );
    assert_eq!(store.latest().unwrap().unwrap().id, later.id);
    assert_eq!(listed[0].id, later.id);

    // The within-second order is the one the old query reported — `id`
    // order — because nothing else about it was ever recorded. Asserting
    // that rather than a write order is the honest claim: the two rows
    // tied on `created_at`, and this is what the migration promises for
    // them.
    let mut tied = [earlier_a.id.clone(), earlier_b.id.clone()];
    tied.sort();
    assert_eq!(
        [listed[2].id.clone(), listed[1].id.clone()],
        tied,
        "rows tied on created_at must keep the order the old query gave them"
    );

    // And a checkpoint written *after* the migration outranks every
    // backfilled row, which is what stops the counter restarting inside
    // the population it just numbered.
    let after = store
        .save(sample_checkpoint(
            &session,
            // Deliberately *earlier* than everything already stored: the
            // counter is a write order, not a clock reading, so a clock
            // that stepped backwards must not resurrect an older row.
            500,
            CheckpointReason::Manual,
        ))
        .unwrap();
    assert_eq!(
        store.latest_for(&session).unwrap().unwrap().id,
        after.id,
        "the checkpoint written last must win even when its timestamp is the oldest"
    );
    assert_eq!(store.latest().unwrap().unwrap().id, after.id);
}

/// A checkpoint with just enough in it to render, parse and be told apart.
fn sample_checkpoint(
    session: &crate::session::SessionId,
    at: i64,
    reason: crate::checkpoint::CheckpointReason,
) -> crate::checkpoint::Checkpoint {
    crate::checkpoint::Checkpoint {
        session: session.clone(),
        harness: "a-harness".to_owned(),
        reason,
        created_at: at,
        git: None,
        working_tree: None,
        handoff: crate::checkpoint::Handoff {
            objective: format!("the objective at {at}"),
            implementation_state: "the state".to_owned(),
            next_actions: vec!["carry on".to_owned()],
            ..crate::checkpoint::Handoff::default()
        },
        trimmed: false,
    }
}

/// Migration proof (b) for migration 13: its `CHECK` refuses an empty
/// reason, so `''` can never read back as *"a reason was recorded"* even
/// from a hand-edited database.
#[test]
fn migration_thirteen_rejects_an_empty_supersession_reason() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = crate::memory::ProjectMemory::open(&fixture.runtime).unwrap();
    let recorded = memory
        .store()
        .record(crate::memory::NewMemory::new(
            crate::memory::MemoryKind::Finding,
            "onyx needs a supersession reason that is not one",
        ))
        .unwrap();
    drop(memory);

    let conn = Connection::open(fixture.runtime.database_path()).unwrap();
    assert!(
        conn.execute(
            "UPDATE memories SET superseded_reason = '' WHERE id = ?1",
            [recorded.id.as_str()],
        )
        .is_err(),
        "an empty supersession reason must be rejected by the CHECK constraint"
    );
    assert!(
        conn.execute(
            "UPDATE memories SET superseded_reason = ?2 WHERE id = ?1",
            rusqlite::params![recorded.id.as_str(), "x".repeat(513)],
        )
        .is_err(),
        "a supersession reason past the bound must be rejected by the CHECK constraint"
    );
}

/// Migration proof (b): migration 10's new `CHECK` rejects a
/// `review_reason` outside the six the map names.
#[test]
fn migration_ten_rejects_an_unrecognized_review_reason() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = crate::memory::ProjectMemory::open(&fixture.runtime).unwrap();
    let recorded = memory
        .store()
        .record(crate::memory::NewMemory::new(
            crate::memory::MemoryKind::Finding,
            "beryl needs a review reason that does not exist",
        ))
        .unwrap();
    drop(memory);

    let conn = Connection::open(fixture.runtime.database_path()).unwrap();
    let result = conn.execute(
        "UPDATE memories SET review_reason = 'not-a-real-reason' WHERE id = ?1",
        [recorded.id.as_str()],
    );
    assert!(
        result.is_err(),
        "an unrecognized review_reason must be rejected by the CHECK constraint"
    );
}

/// Migration proof (a): a version-10 database migrates to 11 keeping
/// every existing row, and gains a working `routing_observations` table.
#[test]
fn a_version_ten_database_migrates_forward_keeping_its_memories() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = crate::memory::ProjectMemory::open(&fixture.runtime).unwrap();
    let pre_existing = memory
        .store()
        .record(crate::memory::NewMemory::new(
            crate::memory::MemoryKind::Decision,
            "citrine decisions predate migration 11",
        ))
        .unwrap();
    drop(memory);

    let db_path = fixture.runtime.database_path();
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(&format!(
            "{UNDO_MIGRATIONS_ABOVE_THIRTEEN}
                 ALTER TABLE sessions DROP COLUMN source_session_id;
                 ALTER TABLE memories DROP COLUMN superseded_reason;
                 DROP TABLE routing_observations;
                 -- Migration 27's table: a rollback that leaves it in place
                 -- meets `table file_claims already exists` on the re-run.
                 DROP TABLE IF EXISTS file_claims;
                 DELETE FROM schema_migrations WHERE version >= 11;"
        ))
        .unwrap();
    }

    assert_eq!(
        schema_version(&db_path),
        10,
        "the rollback must land on version 10"
    );

    // The next launch is an ordinary bootstrap; nothing special is asked
    // of it, matching the way a real upgrade happens.
    let migrated = fixture.rebootstrap().unwrap();
    assert_eq!(
        schema_version(&migrated.database_path()),
        SUPPORTED_SCHEMA_VERSION,
        "the launch must have applied migration 11"
    );

    let reopened = crate::memory::ProjectMemory::open(&migrated).unwrap();
    let intact = reopened
        .store()
        .get(&pre_existing.id)
        .unwrap()
        .expect("the pre-migration memory must survive the upgrade");
    assert_eq!(intact.body, pre_existing.body);

    let conn = Connection::open(migrated.database_path()).unwrap();
    let project_id = stored_project_id(&migrated.database_path());
    conn.execute(
        "INSERT INTO routing_observations (project_id, observed_at, provider, model) \
             VALUES (?1, 1000, 'fixture', 'fixture-model')",
        [project_id.as_str()],
    )
    .expect("a freshly migrated database must accept a routing observation");
}

/// Migration proof (b): the isolation trigger really aborts a foreign
/// `project_id`, migration 4's own pair applied to `routing_observations`.
#[test]
fn migration_eleven_rejects_a_routing_observation_from_a_foreign_project() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let conn = Connection::open(fixture.runtime.database_path()).unwrap();

    let result = conn.execute(
        "INSERT INTO routing_observations (project_id, observed_at, provider, model) \
             VALUES ('a-different-project-entirely', 1000, 'fixture', 'fixture-model')",
        [],
    );
    assert!(
        result.is_err(),
        "an insert naming a foreign project_id must be rejected by the isolation trigger"
    );
}

/// Migration proof (c): the `cost_micro_usd`/`cost_confidence` `CHECK`
/// refuses a cost with no confidence label.
#[test]
fn migration_eleven_refuses_a_cost_with_no_confidence_label() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let conn = Connection::open(fixture.runtime.database_path()).unwrap();
    let project_id = stored_project_id(&fixture.runtime.database_path());

    let result = conn.execute(
        "INSERT INTO routing_observations \
             (project_id, observed_at, provider, model, cost_micro_usd) \
             VALUES (?1, 1000, 'fixture', 'fixture-model', 500)",
        [project_id.as_str()],
    );
    assert!(
        result.is_err(),
        "a stored cost with no cost_confidence must be rejected by the CHECK constraint"
    );

    // The paired value is accepted, so the failure above is about the
    // missing label and not about the column existing at all.
    conn.execute(
        "INSERT INTO routing_observations \
             (project_id, observed_at, provider, model, cost_micro_usd, cost_confidence) \
             VALUES (?1, 1000, 'fixture', 'fixture-model', 500, 'estimated')",
        [project_id.as_str()],
    )
    .expect("a cost paired with a confidence label must be accepted");
}

#[test]
fn bootstrap_creates_the_project_database() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let db = fixture.runtime.database_path();

    assert_eq!(db.file_name().unwrap(), DATABASE_FILE_NAME);
    assert!(db.is_file());
    assert_eq!(
        stored_project_id(&db),
        fixture.runtime.project().id().as_str()
    );
    assert_eq!(schema_version(&db), SUPPORTED_SCHEMA_VERSION);
}

#[test]
fn two_projects_sharing_one_data_root_get_separate_databases_and_ids() {
    let tmp = tempfile::tempdir().unwrap();
    // alpha and beta resolve against the SAME GLASSHOUSE data/config
    // root; only their project identities differ.
    let alpha = Fixture::new(tmp.path(), "alpha");
    let beta = Fixture::new(tmp.path(), "beta");

    let alpha_db = alpha.runtime.database_path();
    let beta_db = beta.runtime.database_path();

    // Both databases live under the one shared projects root...
    let projects_root = tmp.path().join("data").join("projects");
    assert_eq!(alpha_db.parent().unwrap().parent().unwrap(), projects_root);
    assert_eq!(beta_db.parent().unwrap().parent().unwrap(), projects_root);
    // ...yet in physically different files and directories.
    assert_ne!(alpha_db.parent(), beta_db.parent());
    assert_ne!(alpha_db, beta_db);

    // And each file records its own project, not its neighbour's.
    let alpha_id = stored_project_id(&alpha_db);
    let beta_id = stored_project_id(&beta_db);
    assert_ne!(alpha_id, beta_id);
    assert_eq!(alpha_id, alpha.runtime.project().id().as_str());
    assert_eq!(beta_id, beta.runtime.project().id().as_str());
}

#[test]
fn reopening_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let db = fixture.runtime.database_path();

    let id_before = stored_project_id(&db);
    let version_before = schema_version(&db);

    // Reopen through bootstrap several times: nothing may drift.
    for _ in 0..3 {
        let runtime = fixture.rebootstrap().unwrap();
        assert_eq!(runtime.database_path(), db);
    }

    assert_eq!(stored_project_id(&db), id_before);
    assert_eq!(schema_version(&db), version_before);
}

#[test]
fn concurrent_first_bootstraps_serialize_on_one_database() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("workspace").join("solo");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    let root = std::fs::canonicalize(&root).unwrap();

    const CALLERS: usize = 16;
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(CALLERS));
    let mut handles = Vec::new();
    for _ in 0..CALLERS {
        let barrier = std::sync::Arc::clone(&barrier);
        let root = root.clone();
        let data = tmp.path().join("data");
        let config = tmp.path().join("config");
        handles.push(std::thread::spawn(move || {
            // Release all callers at once so the very first creation of
            // the database file and schema is genuinely contended.
            barrier.wait();
            let cli = Cli::try_parse_from([
                "glasshouse",
                "--data-dir",
                data.to_str().unwrap(),
                "--config-dir",
                config.to_str().unwrap(),
            ])
            .unwrap();
            crate::bootstrap(&cli, &root)
        }));
    }

    let mut results = Vec::new();
    for handle in handles {
        results.push(handle.join().expect("bootstrap thread panicked"));
    }
    for result in &results {
        result
            .as_ref()
            .expect("a concurrent first bootstrap failed");
    }

    // All callers agree on one physical database with one binding.
    let expected_db = results[0].as_ref().unwrap().database_path();
    let expected_id = results[0].as_ref().unwrap().project().id().as_str();
    for result in &results {
        let runtime = result.as_ref().unwrap();
        assert_eq!(runtime.database_path(), expected_db);
    }
    assert_eq!(schema_version(&expected_db), SUPPORTED_SCHEMA_VERSION);
    assert_eq!(stored_project_id(&expected_db), expected_id);

    let conn = Connection::open(&expected_db).unwrap();
    let bindings: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM project_metadata WHERE key = 'project_id'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(bindings, 1);
}

/// The name a private creation file would have if this process made one
/// now, with the pid and start time the caller asks for.
///
/// Built here rather than by calling `private_creation_path` because every
/// test below needs to *choose* the pid and start time — that pair is the
/// whole subject of the sweep.
fn private_file_named(db: &Path, pid: u32, started_at_ms: i64, nonce: u64) -> PathBuf {
    db.parent().unwrap().join(format!(
        "{}{PRIVATE_INFIX}{pid}-{:016x}-{nonce:016x}",
        db.file_name().unwrap().to_str().unwrap(),
        started_at_ms as u64,
    ))
}

/// Every private creation file currently sitting beside `db`.
fn private_files_beside(db: &Path) -> Vec<String> {
    let name = db.file_name().unwrap().to_str().unwrap();
    std::fs::read_dir(db.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|candidate| parse_private_name(name, candidate).is_some())
        .collect()
}

/// The straggler this module used to have to wait for no longer exists: a
/// caller that finds no database creates and migrates its *own* private
/// file and races only for the final directory entry. This is what happens
/// to the one that loses that race.
///
/// It is the wave-108 stress test's successor and it pins the same
/// property from the other side. There, a creator held one shared file and
/// a straggler had to wait however long its migration took; here nothing
/// waits, and the question is whether the loser cleans up after itself and
/// adopts the winner's database rather than damaging it.
///
/// **Deterministic by rendezvous, not by clock.** The creator is held in a
/// test-only hook (`install_private_file_hold`, compiled out of every
/// non-`cfg(test)` build) from the instant its private file exists until
/// this thread has *finished* publishing its own. A fixed sleep would have
/// been a bet that the winner finishes inside it — and losing that bet
/// silently swaps the two roles and passes anyway, which is §60's vacuous
/// pass wearing the opposite face. The `HOLD` below is only a hang guard,
/// and the assertion that the creator's file is still unpublished when the
/// winner returns is what proves the roles did not swap.
#[test]
fn a_creator_that_loses_the_publish_race_discards_its_own_and_opens_the_winners() {
    /// Long enough that it never fires, short enough that a broken
    /// rendezvous fails the test instead of hanging the suite.
    const HOLD: std::time::Duration = std::time::Duration::from_secs(120);

    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let db = fixture.runtime.database_path().to_path_buf();
    let base = fixture.base.clone();
    let root = fixture.runtime.project().root().to_path_buf();

    // Back to the state a first launch starts from.
    std::fs::remove_file(&db).unwrap();

    let (created_tx, created_rx) = std::sync::mpsc::channel::<PathBuf>();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();

    let creator = {
        let base = base.clone();
        let root = root.clone();
        std::thread::spawn(move || {
            install_private_file_hold(Box::new(move |private| {
                created_tx.send(private.to_path_buf()).unwrap();
                let _ = release_rx.recv_timeout(HOLD);
            }));
            let cli = Cli::try_parse_from([
                "glasshouse",
                "--data-dir",
                base.join("data").to_str().unwrap(),
                "--config-dir",
                base.join("config").to_str().unwrap(),
            ])
            .unwrap();
            crate::bootstrap(&cli, &root)
        })
    };

    // The creator is now holding a private file it has not published.
    let creators_private = created_rx.recv().unwrap();
    assert!(
        creators_private.exists(),
        "the hook must run with the private file already created"
    );
    assert!(
        !db.exists(),
        "a creation in flight must be invisible at the final path"
    );

    // The winner: an ordinary production bootstrap, start to finish, while
    // the creator is held.
    let winner = fixture
        .rebootstrap()
        .expect("a caller must not be blocked by a sibling mid-creation");
    assert_eq!(winner.database_path(), db);
    assert_eq!(schema_version(&db), SUPPORTED_SCHEMA_VERSION);

    // The roles cannot have swapped: the creator's own file is still
    // sitting there unpublished, so the database now at the final path is
    // the winner's and not the creator's.
    assert!(
        creators_private.exists(),
        "the creator must still be held; if it published first this test proves nothing"
    );

    // A mark that only survives if the winner's inode is the one that
    // stays at the final path. A rename in place of the link would replace
    // it; nothing legitimate ever can.
    let marked = Connection::open(&db).unwrap();
    marked
        .execute(
            "INSERT INTO project_metadata (key, value) VALUES ('publish_race_marker', 'winner')",
            [],
        )
        .unwrap();
    drop(marked);
    #[cfg(unix)]
    let winners_inode = {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(&db).unwrap().ino()
    };

    release_tx.send(()).unwrap();
    let creators_runtime = creator
        .join()
        .expect("the creator thread panicked")
        .expect("losing the publish race is ordinary, not an error");

    // Both callers ended up on the one database.
    assert_eq!(creators_runtime.database_path(), db);
    assert_eq!(
        creators_runtime.project().id().as_str(),
        winner.project().id().as_str()
    );

    // The loser discarded its own finished database rather than publishing
    // it over the winner's.
    assert!(
        private_files_beside(&db).is_empty(),
        "the loser must leave no private file behind; found {:?}",
        private_files_beside(&db)
    );
    let final_db = Connection::open(&db).unwrap();
    let marker: Option<String> = final_db
        .query_row(
            "SELECT value FROM project_metadata WHERE key = 'publish_race_marker'",
            [],
            |row| row.get(0),
        )
        .optional()
        .unwrap();
    assert_eq!(
        marker.as_deref(),
        Some("winner"),
        "the winner's database must still be the one at the final path"
    );
    let bindings: i64 = final_db
        .query_row(
            "SELECT COUNT(*) FROM project_metadata WHERE key = 'project_id'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(bindings, 1);
    drop(final_db);

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            std::fs::metadata(&db).unwrap().ino(),
            winners_inode,
            "the final path must still be the winner's file, not a replacement"
        );
    }
    assert_eq!(schema_version(&db), SUPPORTED_SCHEMA_VERSION);
    assert_eq!(
        stored_project_id(&db),
        creators_runtime.project().id().as_str()
    );
}

/// A creator killed before it published leaves its private file behind,
/// and the next bootstrap collects it — but only because its process is
/// provably gone.
#[test]
fn a_private_file_from_a_dead_creator_is_swept_on_the_next_bootstrap() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let db = fixture.runtime.database_path().to_path_buf();
    std::fs::remove_file(&db).unwrap();

    // Far above any pid Linux, macOS or Windows hands out, so the liveness
    // probe answers "nothing there" rather than "I cannot tell".
    let leftover = private_file_named(&db, 0x3fff_ffff, 1_700_000_000_000, 0xdead_beef);
    std::fs::write(&leftover, b"a crashed creator's work").unwrap();
    let journal = journal_beside(&leftover);
    std::fs::write(&journal, b"and its journal").unwrap();

    // Neighbours that are not private creation files. The sweep is not
    // entitled to any of these and must not touch one.
    let dir = db.parent().unwrap();
    let name = db.file_name().unwrap().to_str().unwrap();
    let bystanders = [
        dir.join(format!("{name}.backup")),
        dir.join(format!(
            "{name}{PRIVATE_INFIX}notapid-0000000000000000-0000000000000000"
        )),
        dir.join(format!("{name}{PRIVATE_INFIX}12345-short-0000000000000000")),
        dir.join(format!("{name}{PRIVATE_INFIX}12345-0000000000000000")),
    ];
    for bystander in &bystanders {
        std::fs::write(bystander, b"not yours").unwrap();
    }

    fixture.rebootstrap().unwrap();

    assert!(
        !leftover.exists(),
        "a private file whose creator is gone must be collected"
    );
    assert!(!journal.exists(), "and so must its journal");
    for bystander in &bystanders {
        assert!(
            bystander.exists(),
            "the sweep touched a file that is not a private creation file: {}",
            bystander.display()
        );
    }
    assert_eq!(schema_version(&db), SUPPORTED_SCHEMA_VERSION);
}

/// The half that matters more: a private file whose creator is *alive* is
/// a sibling's database mid-migration, and deleting it destroys work in
/// flight. This test's own process stands in for that sibling.
#[test]
fn a_private_file_from_a_live_creator_is_left_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let db = fixture.runtime.database_path().to_path_buf();
    std::fs::remove_file(&db).unwrap();

    let started_at_ms = crate::session::supervision::observe(std::process::id())
        .expect("this process must be observable to its own liveness probe")
        .started_at_ms;
    let live = private_file_named(&db, std::process::id(), started_at_ms, 0x0123_4567);
    std::fs::write(&live, b"a live sibling's work in progress").unwrap();

    fixture.rebootstrap().unwrap();

    assert!(
        live.exists(),
        "a private file whose creator is still running must never be swept"
    );
    assert_eq!(
        std::fs::read(&live).unwrap(),
        b"a live sibling's work in progress",
        "and it must be byte-identical"
    );
    assert_eq!(schema_version(&db), SUPPORTED_SCHEMA_VERSION);
}

/// The leak the design note accepted and the start time removes: a
/// leftover whose pid has been recycled by an unrelated process.
///
/// Without the recorded start time this file would pin itself in place
/// forever, because a live pid is indistinguishable from a live creator.
/// With it, the process answering that pid is *provably* not the one that
/// created the file, so the creator is as gone as a pid that answers
/// nothing — and the file goes.
#[test]
fn a_private_file_whose_pid_was_recycled_is_swept() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let db = fixture.runtime.database_path().to_path_buf();
    std::fs::remove_file(&db).unwrap();

    // This process's pid, with a start time that is not this process's:
    // exactly the shape a recycled pid presents.
    let started_at_ms = crate::session::supervision::observe(std::process::id())
        .unwrap()
        .started_at_ms;
    let recycled = private_file_named(
        &db,
        std::process::id(),
        started_at_ms.wrapping_sub(1_000_000),
        0x89ab_cdef,
    );
    std::fs::write(&recycled, b"a crashed creator, long ago").unwrap();

    fixture.rebootstrap().unwrap();

    assert!(
        !recycled.exists(),
        "a leftover whose pid now belongs to another process must be collected"
    );
    assert_eq!(schema_version(&db), SUPPORTED_SCHEMA_VERSION);
}

/// The name is the whole contract the sweep reads, so it is pinned
/// directly: what parses, what does not, and what the fields mean.
#[test]
fn a_private_creation_name_parses_only_in_its_exact_shape() {
    assert_eq!(
        parse_private_name(
            "glasshouse.db",
            "glasshouse.db.tmp-4321-00000000000004d2-0123456789abcdef"
        ),
        Some((4321, 1234))
    );
    for wrong in [
        "glasshouse.db",
        "glasshouse.db-journal",
        "glasshouse.db.tmp-4321-00000000000004d2",
        "glasshouse.db.tmp-4321-00000000000004d2-0123456789abcdef-extra",
        "glasshouse.db.tmp--00000000000004d2-0123456789abcdef",
        "glasshouse.db.tmp-4321-4d2-0123456789abcdef",
        "glasshouse.db.tmp-4321-00000000000004d2-0123456789abcdeg",
        "other.db.tmp-4321-00000000000004d2-0123456789abcdef",
    ] {
        assert_eq!(
            parse_private_name("glasshouse.db", wrong),
            None,
            "{wrong} must not parse as a private creation file"
        );
    }
}

/// The start time is what tells a recycled pid from the creator, and the
/// three answers it produces are what the sweep branches on.
#[test]
fn a_recycled_pid_is_not_the_creator() {
    let me = std::process::id();
    let mine = crate::session::supervision::observe(me)
        .unwrap()
        .started_at_ms;

    assert_eq!(creator_is_gone(me, mine), CreatorLiveness::Working);
    assert_eq!(
        creator_is_gone(me, mine.wrapping_add(1_000_000)),
        CreatorLiveness::Recycled,
        "a live pid whose start time is not the recorded one is provably not the creator"
    );
    assert_eq!(
        creator_is_gone(me, 0),
        CreatorLiveness::Working,
        "an unrecorded start time cannot prove anything, so a live pid is left alone"
    );
    assert_eq!(creator_is_gone(0x3fff_ffff, 0), CreatorLiveness::Gone);
}

/// The same field reproduction, four times as contended.
///
/// Sixteen callers is what the original defect was found at; creation now
/// happens on a private file per caller and they contend only for the
/// final directory entry, so the interesting question is whether that
/// still holds when sixty-three of the sixty-four lose the link and have
/// to discard a finished database each.
#[test]
fn concurrent_first_bootstraps_serialize_on_one_database_at_sixty_four_callers() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("workspace").join("solo");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    let root = std::fs::canonicalize(&root).unwrap();

    const CALLERS: usize = 64;
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(CALLERS));
    let mut handles = Vec::new();
    for _ in 0..CALLERS {
        let barrier = std::sync::Arc::clone(&barrier);
        let root = root.clone();
        let data = tmp.path().join("data");
        let config = tmp.path().join("config");
        handles.push(std::thread::spawn(move || {
            // Release all callers at once so the very first creation of
            // the database file and schema is genuinely contended.
            barrier.wait();
            let cli = Cli::try_parse_from([
                "glasshouse",
                "--data-dir",
                data.to_str().unwrap(),
                "--config-dir",
                config.to_str().unwrap(),
            ])
            .unwrap();
            crate::bootstrap(&cli, &root)
        }));
    }

    let mut results = Vec::new();
    for handle in handles {
        results.push(handle.join().expect("bootstrap thread panicked"));
    }
    for result in &results {
        result
            .as_ref()
            .expect("a concurrent first bootstrap failed");
    }

    // All callers agree on one physical database with one binding.
    let expected_db = results[0].as_ref().unwrap().database_path();
    let expected_id = results[0].as_ref().unwrap().project().id().as_str();
    for result in &results {
        let runtime = result.as_ref().unwrap();
        assert_eq!(runtime.database_path(), expected_db);
    }
    assert_eq!(schema_version(&expected_db), SUPPORTED_SCHEMA_VERSION);
    assert_eq!(stored_project_id(&expected_db), expected_id);

    let conn = Connection::open(&expected_db).unwrap();
    let bindings: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM project_metadata WHERE key = 'project_id'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(bindings, 1);
}

#[test]
fn mismatched_copied_database_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let alpha = Fixture::new(tmp.path(), "alpha");
    let beta = Fixture::new(tmp.path(), "beta");

    // Copy alpha's whole database into beta's slot.
    std::fs::copy(alpha.runtime.database_path(), beta.runtime.database_path()).unwrap();

    let err = beta.rebootstrap().unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("belongs to project"), "{msg}");
    assert!(msg.contains(alpha.runtime.project().id().as_str()), "{msg}");
    assert!(
        msg.contains(beta.runtime.database_path().display().to_string().as_str()),
        "{msg}"
    );

    // The copy is left untouched for the user to decide about.
    assert_eq!(
        stored_project_id(&beta.runtime.database_path()),
        stored_project_id(&alpha.runtime.database_path())
    );
}

#[test]
fn metadata_without_a_project_id_is_rejected_and_not_adopted() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let db = fixture.runtime.database_path();

    {
        let conn = Connection::open(&db).unwrap();
        conn.execute(
            "DELETE FROM project_metadata WHERE key = ?1",
            [PROJECT_ID_KEY],
        )
        .unwrap();
    }

    let err = fixture.rebootstrap().unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("no project identifier"), "{msg}");
    assert!(msg.contains(db.display().to_string().as_str()), "{msg}");

    // Rejection happens during the read-only identity preflight: the
    // missing binding is not silently recreated for the active project.
    let conn = Connection::open(&db).unwrap();
    let bindings: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM project_metadata WHERE key = ?1",
            [PROJECT_ID_KEY],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(bindings, 0);
    assert_eq!(schema_version(&db), SUPPORTED_SCHEMA_VERSION);
}

#[test]
fn too_new_schema_is_rejected_and_not_recreated() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let db = fixture.runtime.database_path();

    {
        // What a newer Glasshouse would leave behind: this build's
        // migrations, plus one it has never heard of. Appending rather
        // than rewriting the existing rows keeps the fixture correct as
        // more migrations are added.
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch("INSERT INTO schema_migrations (version) VALUES (99);")
            .unwrap();
    }

    let err = fixture.rebootstrap().unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("newer"), "{msg}");
    assert!(msg.contains("99"), "{msg}");
    assert!(msg.contains(db.display().to_string().as_str()), "{msg}");

    // Refused, not deleted or recreated: the too-new marker survives.
    assert!(db.is_file());
    assert_eq!(schema_version(&db), 99);
}

#[test]
fn corrupt_database_is_refused_and_never_recreated() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let db = fixture.runtime.database_path();

    std::fs::write(&db, b"definitely not a sqlite database").unwrap();

    let err = fixture.rebootstrap().unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains(db.display().to_string().as_str()), "{msg}");

    // Still the same garbage bytes: nothing was silently wiped.
    assert_eq!(
        std::fs::read(&db).unwrap(),
        b"definitely not a sqlite database"
    );
}

#[test]
fn directory_at_the_database_path_is_rejected_and_not_replaced() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let db = fixture.runtime.database_path();

    // Put a plain directory where the database belongs.
    std::fs::remove_file(&db).unwrap();
    std::fs::create_dir(&db).unwrap();

    let err = fixture.rebootstrap().unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("not a regular file") || msg.contains("a directory"),
        "{msg}"
    );
    assert!(msg.contains(db.display().to_string().as_str()), "{msg}");

    // The directory is still there: nothing deleted or recreated it.
    assert!(db.is_dir());
}

#[test]
fn foreign_database_with_pending_migrations_is_rejected_before_any_writes() {
    let tmp = tempfile::tempdir().unwrap();
    let alpha = Fixture::new(tmp.path(), "alpha");
    let beta = Fixture::new(tmp.path(), "beta");

    // Copy alpha's bound database into beta's slot, then make its
    // migration state look pending by dropping the migration ledger.
    // An implementation that migrated before checking identity would
    // recreate the ledger and write into this foreign database.
    std::fs::copy(alpha.runtime.database_path(), beta.runtime.database_path()).unwrap();
    {
        let conn = Connection::open(beta.runtime.database_path()).unwrap();
        conn.execute_batch("DROP TABLE schema_migrations;").unwrap();
    }

    let err = beta.rebootstrap().unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("belongs to project"), "{msg}");
    assert!(msg.contains(alpha.runtime.project().id().as_str()), "{msg}");
    assert!(
        msg.contains(beta.runtime.database_path().display().to_string().as_str()),
        "{msg}"
    );

    // The refusal happened before any schema work: the ledger is still
    // absent and the foreign binding untouched.
    let conn = Connection::open(beta.runtime.database_path()).unwrap();
    let ledger_present: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type = 'table' AND name = 'schema_migrations'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(ledger_present, 0);
    assert_eq!(
        stored_project_id(&beta.runtime.database_path()),
        alpha.runtime.project().id().as_str()
    );
}

#[cfg(unix)]
#[test]
fn readonly_database_file_is_rejected() {
    use std::os::unix::fs::PermissionsExt;

    // Root can write to 0400 files regardless; the scenario does not
    // exist for that user, so the regression test says nothing there.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let db = fixture.runtime.database_path();

    std::fs::set_permissions(&db, fs::Permissions::from_mode(0o400)).unwrap();

    let err = fixture.rebootstrap().unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("read-only"), "{msg}");
    assert!(msg.contains(db.display().to_string().as_str()), "{msg}");

    // Restore so the temp directory can be cleaned up.
    std::fs::set_permissions(&db, fs::Permissions::from_mode(0o600)).unwrap();
}

/// A zero-byte database file that is also unwritable.
///
/// A zero-byte file at the final path is a truncated database whatever its
/// permissions say, and this is the case that used to need the most care:
/// when deciding meant asking SQLite for the write lock, a file this
/// process could not write was a different answer. It no longer decides
/// anything by opening the file, so the refusal must name the file, be the
/// `EmptyExisting` one, and be immediate.
#[cfg(unix)]
#[test]
fn a_zero_byte_database_that_cannot_be_written_is_still_refused() {
    use std::os::unix::fs::PermissionsExt;

    // Root can write to 0400 files regardless; the scenario does not
    // exist for that user, so the regression test says nothing there.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let db = fixture.runtime.database_path();

    std::fs::write(&db, []).unwrap();
    std::fs::set_permissions(&db, fs::Permissions::from_mode(0o400)).unwrap();

    let started = std::time::Instant::now();
    let err = fixture.rebootstrap().unwrap_err();
    let waited = started.elapsed();
    let msg = format!("{err:#}");

    assert!(msg.contains(db.display().to_string().as_str()), "{msg}");
    // The refusal is the same one a writable empty file gets: a zero-byte
    // file at the final path is a truncated database, full stop, and
    // nothing else -- not a `Sql` error from the read-only connection, not
    // an `Open` failure -- is allowed to stand in for it.
    assert!(
        err.chain().any(|cause| matches!(
            cause.downcast_ref::<DatabaseError>(),
            Some(DatabaseError::EmptyExisting { .. })
        )),
        "expected EmptyExisting, got: {msg}"
    );
    assert!(
        waited < PROMPT_REFUSAL,
        "refusing an unwritable empty file took {waited:?}; the refusal opens \
             no connection, takes no lock and waits for nothing, so it cannot be \
             anywhere near {PROMPT_REFUSAL:?}"
    );
    assert_eq!(
        std::fs::metadata(&db).unwrap().len(),
        0,
        "a refused open must leave the file byte-identical"
    );

    // Restore so the temp directory can be cleaned up.
    std::fs::set_permissions(&db, fs::Permissions::from_mode(0o600)).unwrap();
}

#[cfg(unix)]
#[test]
fn new_database_file_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let mode = std::fs::metadata(fixture.runtime.database_path())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600, "new database must be owner-only");
}

#[cfg(unix)]
#[test]
fn symlinked_final_database_path_is_rejected() {
    use std::os::unix::fs::symlink;

    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let db = fixture.runtime.database_path();

    // Replace the real database with a symlink to an unrelated file.
    let decoy = tmp.path().join("decoy.db");
    std::fs::write(&decoy, b"decoy").unwrap();
    std::fs::remove_file(&db).unwrap();
    symlink(&decoy, &db).unwrap();

    let err = fixture.rebootstrap().unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("symbolic link"), "{msg}");
    assert!(msg.contains(db.display().to_string().as_str()), "{msg}");

    // The symlink itself is left alone; nothing followed or replaced it.
    assert!(
        std::fs::symlink_metadata(&db)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(std::fs::read(&decoy).unwrap(), b"decoy");
}

/// What "immediately" means for the zero-byte refusal, with enough room
/// that a loaded machine cannot make it flake.
///
/// Two orders of magnitude below the 400 ms grace and the 500 ms timer
/// that stood here before, because the refusal now does no I/O beyond the
/// `stat` that found the file: no connection is opened, no lock is taken,
/// nothing is slept on. If this ever fails, something started waiting
/// again.
const PROMPT_REFUSAL: std::time::Duration = std::time::Duration::from_millis(100);

#[test]
fn a_zero_byte_existing_database_is_refused_not_silently_reinitialized() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let db = fixture.runtime.database_path();

    // Truncate an existing, previously-migrated database to zero bytes —
    // the shape a crashed `cp`, an interrupted restore, or a full-disk
    // write leaves behind.
    std::fs::write(&db, []).unwrap();
    assert_eq!(std::fs::metadata(&db).unwrap().len(), 0);

    let started = std::time::Instant::now();
    let err = fixture.rebootstrap().unwrap_err();
    let waited = started.elapsed();
    let msg = format!("{err:#}");

    assert!(
        waited < PROMPT_REFUSAL,
        "refusing a truncated database took {waited:?}; there is nothing left \
             to wait for at this path, so the refusal must be immediate"
    );
    assert!(
        err.chain().any(|cause| matches!(
            cause.downcast_ref::<DatabaseError>(),
            Some(DatabaseError::EmptyExisting { .. })
        )),
        "expected EmptyExisting, got: {msg}"
    );
    assert!(msg.contains("empty"), "{msg}");
    assert!(msg.contains(db.display().to_string().as_str()), "{msg}");

    // The refused open must not have touched the file: still zero bytes,
    // no migration ever ran against it.
    assert_eq!(
        std::fs::metadata(&db).unwrap().len(),
        0,
        "a refused open must leave the file byte-identical"
    );

    // Nor may it have left anything beside the file. The refusal opens no
    // connection at all now, so there is no `-journal` sidecar to avoid
    // and no private creation file either — this filter catches both, and
    // a refusal that started creating one would fail here.
    let leftovers: Vec<String> = std::fs::read_dir(db.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("glasshouse.db") && name != "glasshouse.db")
        .collect();
    assert!(
        leftovers.is_empty(),
        "a refused open must leave no sidecar files behind; found {leftovers:?}"
    );
}

#[test]
fn a_missing_database_file_still_creates_a_fresh_project() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let db = fixture.runtime.database_path();
    let version_before = schema_version(&db);

    // Unlike the zero-byte case, a database that simply does not exist
    // yet must still be created and migrated exactly as a first launch.
    std::fs::remove_file(&db).unwrap();
    assert!(!db.exists());

    let migrated = fixture.rebootstrap().unwrap();
    assert_eq!(migrated.database_path(), db);
    assert!(db.exists());
    assert_eq!(schema_version(&db), version_before);
}
