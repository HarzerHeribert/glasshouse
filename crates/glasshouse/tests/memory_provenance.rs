//! Migration 6: event provenance, decision provenance, and the rebuilt FTS5
//! index — proved from the outside, the way `memory_store.rs` proves the
//! rest of the table.

use std::path::Path;

use clap::Parser;

use glasshouse::memory::search::SearchScope;
use glasshouse::memory::{
    DecisionProvenance, MemoryAuthority, MemoryKind, NewMemory, ProjectMemory, ProjectPhase,
    SourceEvents,
};
use glasshouse::{Cli, Runtime};

/// A bootstrapped project inside `base`, sharing `base`'s data and config
/// roots. Two fixtures over one `base` are two real projects on one machine.
///
/// Copied from `tests/memory_store.rs` rather than reinvented, per the
/// packet.
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
}

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

// -------------------------------------------------------------------------
// Round trip and absence.
// -------------------------------------------------------------------------

/// Every one of Phase 21B's ten `DecisionProvenance` fields, plus a
/// `SourceEvents` range, round-trips through SQLite exactly. A memory
/// recorded with none of them comes back with every one absent — the
/// absent/empty distinction migration 6 exists to preserve.
#[test]
fn every_provenance_field_round_trips_and_absence_stays_none() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let provenance = DecisionProvenance {
        rationale: Some(
            "chose SQLite triggers over foreign keys because PRAGMA foreign_keys \
             defaults off"
                .to_owned(),
        ),
        project_phase: Some(ProjectPhase::Beta),
        problem: Some("needed project isolation that no query could accidentally skip".to_owned()),
        assumptions: Some("assumed every connection goes through database::open".to_owned()),
        scale_assumptions: Some("assumed under ten thousand memories per project".to_owned()),
        security_assumptions: Some(
            "assumed the database file itself is the trust boundary".to_owned(),
        ),
        compatibility_assumptions: Some(
            "assumed SQLite's bundled FTS5 module is always available".to_owned(),
        ),
        operational_assumptions: Some(
            "assumed a single Glasshouse process writes at a time".to_owned(),
        ),
        evidence: Some(
            "verified in concurrent_first_bootstraps_serialize_on_one_database".to_owned(),
        ),
        source_excerpt: Some("\"a query can forget to filter by project_id\"".to_owned()),
    };
    let events = SourceEvents::new(10, 25).unwrap();

    let recorded = store
        .record(
            NewMemory::new(
                MemoryKind::Decision,
                "project isolation is enforced by triggers, not foreign keys",
            )
            .with_provenance(provenance.clone())
            .with_source_events(Some(events)),
        )
        .unwrap();
    assert_eq!(recorded.provenance, provenance);
    assert_eq!(recorded.source_events, Some(events));

    let read_back = store.get(&recorded.id).unwrap().unwrap();
    assert_eq!(
        read_back.provenance, provenance,
        "every provenance field must survive the round trip exactly"
    );
    assert_eq!(read_back.source_events, Some(events));

    // The absent case: nothing recorded, everything reads back `None`. A
    // store that quietly wrote a default here would still pass the filled
    // case above, which is why this half of the test exists.
    let bare = store
        .record(NewMemory::new(
            MemoryKind::Decision,
            "a decision recorded with nothing else known about it",
        ))
        .unwrap();
    let read_back = store.get(&bare.id).unwrap().unwrap();
    assert_eq!(read_back.provenance, DecisionProvenance::default());
    assert_eq!(read_back.source_events, None);
}

/// `with_provenance` stores a whitespace-only field as `None`, the same rule
/// `NewMemory::with_subject` already applies — "nobody recorded this" and
/// "this is the empty string" must not become the same fact.
#[test]
fn whitespace_only_provenance_fields_are_stored_as_absence() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let provenance = DecisionProvenance {
        rationale: Some("   ".to_owned()),
        security_assumptions: Some("  \n\t ".to_owned()),
        ..DecisionProvenance::default()
    };

    let recorded = store
        .record(
            NewMemory::new(
                MemoryKind::Decision,
                "a decision whose rationale and security assumption were blank",
            )
            .with_provenance(provenance),
        )
        .unwrap();
    assert_eq!(recorded.provenance.rationale, None);
    assert_eq!(recorded.provenance.security_assumptions, None);

    let read_back = store.get(&recorded.id).unwrap().unwrap();
    assert_eq!(read_back.provenance.rationale, None);
    assert_eq!(read_back.provenance.security_assumptions, None);
}

// -------------------------------------------------------------------------
// What the database refuses, at the row.
// -------------------------------------------------------------------------

/// A row naming one end of its source event range without the other, or
/// naming them out of order, is refused by migration 6's `INSERT` trigger —
/// not merely avoided by `NewMemory`'s own API. Legitimate rows still write.
#[test]
fn inserting_a_memory_with_a_half_filled_event_range_is_refused_by_the_database() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();
    let project_id = store.project_id().to_owned();
    drop(store);
    drop(memory);

    let conn = rusqlite::Connection::open(fixture.runtime.database_path()).unwrap();

    let err = conn
        .execute(
            "INSERT INTO memories (id, project_id, kind, status, body, created_at, \
             updated_at, source_event_first, source_event_last) \
             VALUES ('e1', ?1, 'finding', 'active', 'a half filled event range', 1, 1, 5, NULL)",
            [&project_id],
        )
        .expect_err("naming only the first end of the range must be refused");
    assert!(err.to_string().contains("names both ends"), "{err}");

    let err = conn
        .execute(
            "INSERT INTO memories (id, project_id, kind, status, body, created_at, \
             updated_at, source_event_first, source_event_last) \
             VALUES ('e2', ?1, 'finding', 'active', 'a reversed event range', 1, 1, 10, 5)",
            [&project_id],
        )
        .expect_err("first > last must be refused");
    assert!(err.to_string().contains("names both ends"), "{err}");

    conn.execute(
        "INSERT INTO memories (id, project_id, kind, status, body, created_at, updated_at) \
         VALUES ('e3', ?1, 'finding', 'active', 'no event range at all', 1, 1)",
        [&project_id],
    )
    .expect("a memory naming neither end of the range must still write");

    conn.execute(
        "INSERT INTO memories (id, project_id, kind, status, body, created_at, \
         updated_at, source_event_first, source_event_last) \
         VALUES ('e4', ?1, 'finding', 'active', 'a real event range', 1, 1, 3, 8)",
        [&project_id],
    )
    .expect("a well-formed range must still write");

    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 2, "only the two legitimate inserts may have landed");
}

/// The same refusal applies to `UPDATE`, not only `INSERT` — migration 6
/// ships two triggers for exactly this reason.
#[test]
fn updating_a_memory_to_a_half_filled_event_range_is_refused_by_the_database() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();
    let recorded = store
        .record(NewMemory::new(
            MemoryKind::Finding,
            "a memory that will be edited with raw SQL",
        ))
        .unwrap();
    drop(store);
    drop(memory);

    let conn = rusqlite::Connection::open(fixture.runtime.database_path()).unwrap();

    let err = conn
        .execute(
            "UPDATE memories SET source_event_first = 5, source_event_last = NULL \
             WHERE id = ?1",
            [recorded.id.as_str()],
        )
        .expect_err("updating to only one end of the range must be refused");
    assert!(err.to_string().contains("names both ends"), "{err}");

    let err = conn
        .execute(
            "UPDATE memories SET source_event_first = 10, source_event_last = 5 \
             WHERE id = ?1",
            [recorded.id.as_str()],
        )
        .expect_err("updating to a reversed range must be refused");
    assert!(err.to_string().contains("names both ends"), "{err}");

    conn.execute(
        "UPDATE memories SET source_event_first = 3, source_event_last = 8 WHERE id = ?1",
        [recorded.id.as_str()],
    )
    .expect("updating to a well-formed range must succeed");

    let (first, last): (Option<i64>, Option<i64>) = conn
        .query_row(
            "SELECT source_event_first, source_event_last FROM memories WHERE id = ?1",
            [recorded.id.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((first, last), (Some(3), Some(8)));
}

/// `project_phase` is a fixed vocabulary enforced by the `CHECK` at the
/// storage layer, not merely by the Rust enum, and every `ProjectPhase`
/// variant the type supports writes and reads back through the store.
#[test]
fn project_phase_is_a_fixed_vocabulary_enforced_by_the_database() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();
    let project_id = store.project_id().to_owned();
    drop(store);
    drop(memory);

    let conn = rusqlite::Connection::open(fixture.runtime.database_path()).unwrap();
    let err = conn
        .execute(
            "INSERT INTO memories (id, project_id, kind, status, body, created_at, \
             updated_at, project_phase) \
             VALUES ('phase-invalid', ?1, 'decision', 'active', 'a phase nobody named', \
             1, 1, 'general-availability')",
            [&project_id],
        )
        .expect_err("an unrecognized project phase must be refused by the CHECK");
    assert!(err.to_string().to_lowercase().contains("check"), "{err}");
    drop(conn);

    let memory = fixture.memory();
    let store = memory.store();
    assert_eq!(
        ProjectPhase::ALL.len(),
        5,
        "Phase 21B names exactly five project phases"
    );
    for phase in ProjectPhase::ALL {
        let recorded = store
            .record(
                NewMemory::new(
                    MemoryKind::Decision,
                    format!("a decision made during the {phase} phase"),
                )
                .with_provenance(DecisionProvenance {
                    project_phase: Some(*phase),
                    ..Default::default()
                }),
            )
            .unwrap_or_else(|error| panic!("recording a {phase}-phase decision failed: {error}"));
        let read_back = store.get(&recorded.id).unwrap().unwrap();
        assert_eq!(read_back.provenance.project_phase, Some(*phase));
    }
}

// -------------------------------------------------------------------------
// The rebuilt FTS5 index.
// -------------------------------------------------------------------------

/// A rationale is findable by full-text search even when the body it sits
/// beside does not contain the word — the whole point of rebuilding
/// `memories_fts` over three columns instead of two.
#[test]
fn a_rationale_is_findable_by_full_text_search_even_when_the_body_is_not() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let recorded = store
        .record(
            NewMemory::new(
                MemoryKind::Decision,
                "the gateway streams responses through untouched",
            )
            .with_provenance(DecisionProvenance {
                rationale: Some("zorbatron compression would break streaming clients".to_owned()),
                ..Default::default()
            }),
        )
        .unwrap();

    let hits = store.search("zorbatron", SearchScope::Current, 10).unwrap();
    assert_eq!(hits.len(), 1, "the rationale-only word must be found");
    assert_eq!(hits[0].id, recorded.id);
}

/// Editing a stored rationale updates the full-text index: the old wording
/// stops being found and the new wording starts. This is what proves the
/// rebuilt `AFTER UPDATE` trigger actually carries the third column — a
/// trigger that forgot it would leave the index silently stale.
#[test]
fn editing_a_rationale_updates_the_full_text_index() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let recorded = store
        .record(
            NewMemory::new(
                MemoryKind::Decision,
                "a decision whose rationale gets edited",
            )
            .with_provenance(DecisionProvenance {
                rationale: Some("quixotically chosen for its simplicity".to_owned()),
                ..Default::default()
            }),
        )
        .unwrap();
    assert_eq!(
        store
            .search("quixotically", SearchScope::Current, 10)
            .unwrap()
            .len(),
        1,
        "the original rationale must be findable before the edit"
    );

    // The store has no setter for the rationale; edit it with raw SQL, as
    // the packet directs.
    let conn = rusqlite::Connection::open(fixture.runtime.database_path()).unwrap();
    conn.execute(
        "UPDATE memories SET rationale = 'zylophonic instead' WHERE id = ?1",
        [recorded.id.as_str()],
    )
    .unwrap();
    drop(conn);

    assert!(
        store
            .search("quixotically", SearchScope::Current, 10)
            .unwrap()
            .is_empty(),
        "the old rationale must be gone from the index"
    );
    let hits = store
        .search("zylophonic", SearchScope::Current, 10)
        .unwrap();
    assert_eq!(hits.len(), 1, "the new rationale must be found");
    assert_eq!(hits[0].id, recorded.id);
}

// -------------------------------------------------------------------------
// The lower-confidence rule (`demote_thin_decisions`).
// -------------------------------------------------------------------------

/// Two decisions of the same authority class: the well-proven one comes
/// first even when the thin one is the denser (and so, by BM25 alone, the
/// better) match.
#[test]
fn a_well_proven_decision_precedes_a_better_matching_thin_decision_of_the_same_authority() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let thin = store
        .record(
            NewMemory::new(
                MemoryKind::Decision,
                "sapphire sapphire sapphire sapphire a decision with no reason recorded",
            )
            .with_authority(Some(MemoryAuthority::Decision)),
        )
        .unwrap();
    let well_proven = store
        .record(
            NewMemory::new(
                MemoryKind::Decision,
                "sapphire a decision that explains itself",
            )
            .with_authority(Some(MemoryAuthority::Decision))
            .with_provenance(DecisionProvenance {
                rationale: Some("chosen for its simplicity, not its speed".to_owned()),
                ..Default::default()
            }),
        )
        .unwrap();

    let hits = store.search("sapphire", SearchScope::Current, 10).unwrap();
    assert_eq!(hits.len(), 2);
    assert_eq!(
        hits[0].id, well_proven.id,
        "the well-proven decision must be first despite matching less densely"
    );
    assert_eq!(hits[1].id, thin.id);
}

/// A thin decision that matches better than a finding is not overtaken by
/// it: the rule compares a decision to a decision, never a decision to a
/// finding.
#[test]
fn a_thin_decision_matching_better_is_not_jumped_by_a_finding() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let thin_decision = store
        .record(NewMemory::new(
            MemoryKind::Decision,
            "opal opal opal opal a decision made without explanation",
        ))
        .unwrap();
    let finding = store
        .record(NewMemory::new(
            MemoryKind::Finding,
            "opal a fact worth remembering",
        ))
        .unwrap();

    let hits = store.search("opal", SearchScope::Current, 10).unwrap();
    assert_eq!(hits.len(), 2);
    assert_eq!(
        hits[0].id, thin_decision.id,
        "the denser match must still win: a finding does not jump a decision \
         merely because the decision is thin"
    );
    assert_eq!(hits[1].id, finding.id);
}

/// A thin decision and a well-proven decision of *different* authority
/// classes keep whatever order BM25 gave them: the rule only reorders within
/// one authority class.
#[test]
fn thin_and_well_proven_decisions_of_different_authority_classes_keep_bm25_order() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let thin_preference = store
        .record(
            NewMemory::new(
                MemoryKind::Decision,
                "amber amber amber amber a preference decision with no reason",
            )
            .with_authority(Some(MemoryAuthority::Preference)),
        )
        .unwrap();
    let well_proven_constraint = store
        .record(
            NewMemory::new(
                MemoryKind::Decision,
                "amber a constraint decision with a reason",
            )
            .with_authority(Some(MemoryAuthority::Constraint))
            .with_provenance(DecisionProvenance {
                rationale: Some("locked in by the platform's own limit".to_owned()),
                ..Default::default()
            }),
        )
        .unwrap();

    let hits = store.search("amber", SearchScope::Current, 10).unwrap();
    assert_eq!(hits.len(), 2);
    assert_eq!(
        hits[0].id, thin_preference.id,
        "different authority classes must not be compared against each other"
    );
    assert_eq!(hits[1].id, well_proven_constraint.id);
}

/// A decision with an assumption recorded but no rationale is not thin — the
/// rule is `rationale.is_none() AND !has_assumptions()`, not `OR` — so it is
/// not demoted even when it out-matches a decision that recorded a
/// rationale.
#[test]
fn a_decision_with_an_assumption_but_no_rationale_is_not_thin_and_is_not_demoted() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let with_assumption_only = store
        .record(
            NewMemory::new(
                MemoryKind::Decision,
                "garnet garnet garnet garnet a decision resting on a recorded assumption",
            )
            .with_authority(Some(MemoryAuthority::Decision))
            .with_provenance(DecisionProvenance {
                assumptions: Some("assumed load stays under one request per second".to_owned()),
                ..Default::default()
            }),
        )
        .unwrap();
    let well_proven = store
        .record(
            NewMemory::new(
                MemoryKind::Decision,
                "garnet a decision that explains itself",
            )
            .with_authority(Some(MemoryAuthority::Decision))
            .with_provenance(DecisionProvenance {
                rationale: Some("chosen because garnet was the cheapest option".to_owned()),
                ..Default::default()
            }),
        )
        .unwrap();

    assert!(
        !with_assumption_only.is_lower_confidence_decision(),
        "recording an assumption alone must already keep a decision out of the thin case"
    );

    let hits = store.search("garnet", SearchScope::Current, 10).unwrap();
    assert_eq!(hits.len(), 2);
    assert_eq!(
        hits[0].id, with_assumption_only.id,
        "a decision with an assumption but no rationale must not be demoted \
         below a decision that only out-matched it, not out-proved it"
    );
    assert_eq!(hits[1].id, well_proven.id);
}

// -------------------------------------------------------------------------
// The migration itself.
// -------------------------------------------------------------------------

/// A version-5 database migrates forward to version 6, keeping its existing
/// memory intact and still findable by search.
///
/// Modeled on `a_version_three_database_gains_the_memory_table_with_its_sessions_intact`
/// in `tests/memory_store.rs`.
#[test]
fn a_version_five_database_migrates_forward_keeping_its_memories() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let pre_existing = store
        .record(
            NewMemory::new(MemoryKind::Decision, "topaz decisions predate migration 6")
                .with_subject(Some("pre-migration decision")),
        )
        .unwrap();
    drop(store);
    drop(memory);

    let db_path = fixture.runtime.database_path();
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "DROP TRIGGER memories_reject_broken_event_range_insert;
             DROP TRIGGER memories_reject_broken_event_range_update;
             DROP INDEX memories_by_source_session;

             DROP TRIGGER memories_fts_after_insert;
             DROP TRIGGER memories_fts_after_delete;
             DROP TRIGGER memories_fts_after_update;
             DROP TABLE memories_fts;

             ALTER TABLE memories DROP COLUMN source_event_first;
             ALTER TABLE memories DROP COLUMN source_event_last;
             ALTER TABLE memories DROP COLUMN rationale;
             ALTER TABLE memories DROP COLUMN project_phase;
             ALTER TABLE memories DROP COLUMN problem;
             ALTER TABLE memories DROP COLUMN assumptions;
             ALTER TABLE memories DROP COLUMN scale_assumptions;
             ALTER TABLE memories DROP COLUMN security_assumptions;
             ALTER TABLE memories DROP COLUMN compatibility_assumptions;
             ALTER TABLE memories DROP COLUMN operational_assumptions;
             ALTER TABLE memories DROP COLUMN evidence;
             ALTER TABLE memories DROP COLUMN source_excerpt;

             CREATE VIRTUAL TABLE memories_fts USING fts5(
                 subject,
                 body,
                 content = 'memories',
                 content_rowid = 'rowid',
                 tokenize = 'unicode61 remove_diacritics 2'
             );
             INSERT INTO memories_fts (memories_fts) VALUES ('rebuild');

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

             DELETE FROM schema_migrations WHERE version >= 6;",
        )
        .unwrap();

        let version: i64 = conn
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, 5, "the rollback must land on version 5");
    }

    // The next launch is an ordinary bootstrap; nothing special is asked of
    // it.
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
    assert_eq!(version, 6, "the launch must have applied migration 6");
    drop(conn);

    let reopened = ProjectMemory::open(&migrated).unwrap();
    let store = reopened.store();
    let intact = store
        .get(&pre_existing.id)
        .unwrap()
        .expect("the pre-existing memory must survive the migration");
    assert_eq!(intact.body, pre_existing.body);
    assert_eq!(intact.subject, pre_existing.subject);
    assert_eq!(
        intact.provenance,
        DecisionProvenance::default(),
        "migration 6 must not invent provenance for a row that predates it"
    );

    let hits = store.search("topaz", SearchScope::Current, 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].id, pre_existing.id,
        "the pre-existing memory must still be findable by search after migration"
    );
}
