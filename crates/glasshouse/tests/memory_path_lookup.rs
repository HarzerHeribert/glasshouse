//! Acceptance tests for `MemoryStore::for_path` — the read door onto
//! migration 17's `memory_files` rows.
//!
//! The producer landed with no reader: `record_observed_files` wrote the
//! associations and nothing could ask for them back. These tests hold the
//! door to five claims — it returns the memories observed against a path and
//! only those, it reports **no relevance** for memories no query ever
//! matched, it left a real search's behaviour untouched, it never crosses a
//! project boundary, and it ranks through the same `group()` the other two
//! doors rank through.
//!
//! # The second claim is the one this file exists for
//!
//! `RetrievalResult`'s relevance map is private so that *every entry was
//! produced by an actual retrieval*. A path lookup runs no query, so it has
//! no relevance to report, and `Some(0.0)` would be a fabrication that reads
//! as "matched as badly as possible" rather than "was not asked about". The
//! test that watches this is
//! `a_path_lookup_reports_no_relevance_rather_than_a_zero`.
//!
//! Exercises the crate exactly as a caller does: through
//! `glasshouse::memory::ProjectMemory::open`, never through anything private
//! to the crate.

use std::path::{Path, PathBuf};

use clap::Parser;
use rusqlite::Connection;

use glasshouse::memory::search::{RetrievalResult, SearchScope};
use glasshouse::memory::{
    DecisionProvenance, MemoryAuthority, MemoryId, MemoryKind, MemoryRecord, MemoryStore,
    NewMemory, ProjectMemory,
};
use glasshouse::{Cli, Runtime, bootstrap};

/// A bootstrapped project inside `base`, sharing `base`'s data and config
/// roots — the same shape `tests/memory_project_scope.rs` uses, so that two
/// fixtures over one `base` are two real projects on one machine, each with
/// its own canonicalised root and its own `glasshouse.db`.
struct Fixture {
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
        Fixture { runtime }
    }

    fn memory(&self) -> ProjectMemory {
        ProjectMemory::open(&self.runtime).unwrap()
    }

    fn raw_connection(&self) -> Connection {
        Connection::open(self.runtime.database_path()).unwrap()
    }
}

/// Six memories that all match `marmot`, spread across three ladder rungs and
/// two authority classes, so that every stage of the ranking has something to
/// do: the rung comparison separates the invariant and the idea from the
/// middle, the blended relevance/decay weight orders the middle rung, and
/// `demote_thin_decisions` puts the well-proven decision ahead of the thin
/// one.
///
/// Returned in declaration order, which is deliberately **not** the order a
/// search returns them in.
fn seed_ranking_corpus(store: &MemoryStore<'_>) -> Vec<MemoryId> {
    let record = |memory: NewMemory| store.record(memory).unwrap().id;

    vec![
        record(
            NewMemory::new(
                MemoryKind::Constraint,
                "a marmot credential is never written to a log",
            )
            .with_subject(Some("marmot credential handling"))
            .with_authority(Some(MemoryAuthority::Invariant)),
        ),
        record(
            NewMemory::new(MemoryKind::Constraint, "marmot requests time out at 30s")
                .with_subject(Some("marmot timeout"))
                .with_authority(Some(MemoryAuthority::Constraint)),
        ),
        record(
            NewMemory::new(
                MemoryKind::Constraint,
                "a marmot payload is never larger than one megabyte",
            )
            .with_subject(Some("marmot payload size"))
            .with_authority(Some(MemoryAuthority::Constraint)),
        ),
        record(
            NewMemory::new(MemoryKind::Decision, "marmot retries three times")
                .with_subject(Some("marmot retry policy"))
                .with_authority(Some(MemoryAuthority::Decision))
                .with_provenance(DecisionProvenance {
                    rationale: Some("three covers a transient upstream outage".into()),
                    assumptions: Some("the upstream recovers inside a minute".into()),
                    ..DecisionProvenance::default()
                }),
        ),
        record(
            NewMemory::new(MemoryKind::Decision, "marmot batches on write")
                .with_subject(Some("marmot batching"))
                .with_authority(Some(MemoryAuthority::Decision)),
        ),
        record(
            NewMemory::new(MemoryKind::Finding, "marmot could maybe use a cache")
                .with_subject(Some("marmot caching idea"))
                .with_authority(Some(MemoryAuthority::Idea)),
        ),
    ]
}

/// Every subject a result carries, in the order it carries them.
fn subjects(records: &[MemoryRecord]) -> Vec<String> {
    records
        .iter()
        .map(|record| record.subject.clone().unwrap_or_default())
        .collect()
}

/// A real search's behaviour is **identical** — same records, same order, and
/// every relevance still `Some(_)`.
///
/// The three expected orders below are **literals captured from the tree
/// before this change** by running this same fixture against `main`. They are
/// deliberately not computed from `ladder_rung`, `retrieval_weight` or the
/// comparator: a test that derives its expectation from the thing under test
/// cannot detect that thing changing.
#[test]
fn a_real_search_returns_the_same_records_in_the_same_order_with_the_same_relevances() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let project = fixture.memory();
    let store = project.store();
    seed_ranking_corpus(&store);

    let flat = store.search("marmot", SearchScope::Current, 10).unwrap();
    assert_eq!(
        subjects(&flat),
        [
            "marmot credential handling",
            "marmot retry policy",
            "marmot timeout",
            "marmot payload size",
            "marmot batching",
            "marmot caching idea",
        ],
        "FLAT ORDER"
    );

    let grouped = store
        .search_grouped("marmot", SearchScope::Current, 10)
        .unwrap();
    assert_eq!(
        subjects(&grouped.invariants_and_constraints),
        [
            "marmot credential handling",
            "marmot timeout",
            "marmot payload size",
        ],
        "GROUPED INVARIANTS"
    );
    assert_eq!(
        subjects(&grouped.other),
        [
            "marmot retry policy",
            "marmot batching",
            "marmot caching idea"
        ],
        "GROUPED OTHER"
    );

    for record in grouped
        .invariants_and_constraints
        .iter()
        .chain(grouped.other.iter())
    {
        assert!(
            grouped.relevance(&record.id).is_some_and(f64::is_finite),
            "a memory an actual query returned must still carry the relevance it earned, \
             but {:?} carried {:?}",
            record.subject,
            grouped.relevance(&record.id),
        );
    }
}

/// Every memory a retrieval returned, invariants first, as one list.
fn returned(result: &RetrievalResult) -> Vec<&MemoryRecord> {
    result
        .invariants_and_constraints
        .iter()
        .chain(result.other.iter())
        .collect()
}

/// A path returns the memories observed against it, and **only** those.
///
/// Does not compile against `main` — `for_path` does not exist there — which
/// is the strongest form of "fails before the change".
#[test]
fn a_path_returns_the_memories_observed_against_it_and_only_those() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let project = fixture.memory();
    let store = project.store();

    let record = |body: &str| {
        store
            .record(NewMemory::new(MemoryKind::Finding, body).with_subject(Some(body)))
            .unwrap()
            .id
    };
    let parser = record("the parser rejects a trailing comma");
    let renderer = record("the renderer pads to the viewport");
    let unrelated = record("the release notes live in the wiki");

    store
        .record_observed_files(
            &[parser.clone(), renderer.clone()],
            &["src/parser.rs".to_owned()],
        )
        .unwrap();
    store
        .record_observed_files(
            std::slice::from_ref(&unrelated),
            &["docs/release.md".to_owned()],
        )
        .unwrap();

    let hit = store
        .for_path("src/parser.rs", SearchScope::Current, 10)
        .unwrap();
    let mut ids: Vec<&MemoryId> = returned(&hit)
        .into_iter()
        .map(|record| &record.id)
        .collect();
    ids.sort_by_key(|id| id.as_str().to_owned());
    let mut expected = vec![&parser, &renderer];
    expected.sort_by_key(|id| id.as_str().to_owned());
    assert_eq!(
        ids, expected,
        "a path lookup must return exactly the memories observed against that path"
    );

    let other = store
        .for_path("docs/release.md", SearchScope::Current, 10)
        .unwrap();
    assert_eq!(
        returned(&other)
            .into_iter()
            .map(|record| record.id.clone())
            .collect::<Vec<MemoryId>>(),
        vec![unrelated],
        "the premise: a second path returns its own memory, so an empty result above \
         could not have passed by accident"
    );

    let absent = store
        .for_path("src/never-touched.rs", SearchScope::Current, 10)
        .unwrap();
    assert!(
        returned(&absent).is_empty(),
        "a path nothing was observed against returns nothing"
    );
}

/// **The ruling, watched.** A memory a path lookup returned was never
/// queried, so it has no relevance — and `None` is the answer, never
/// `Some(0.0)`.
///
/// `RetrievalResult`'s relevance map is private precisely so that every entry
/// in it was produced by an actual retrieval. A zero here would be a
/// manufactured relevance for a memory no query ever matched: a number that
/// reads as "matched as badly as possible" rather than "was not asked about",
/// which is the fabrication `memory::inject::briefing`'s refusal exists to
/// prevent.
#[test]
fn a_path_lookup_reports_no_relevance_rather_than_a_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let project = fixture.memory();
    let store = project.store();
    let ids = seed_ranking_corpus(&store);
    store
        .record_observed_files(&ids, &["src/marmot.rs".to_owned()])
        .unwrap();

    let hit = store
        .for_path("src/marmot.rs", SearchScope::Current, 10)
        .unwrap();
    assert_eq!(
        returned(&hit).len(),
        ids.len(),
        "the premise: every seeded memory came back, so the assertion below is about \
         all six of them and not about an empty result"
    );

    for record in returned(&hit) {
        assert_eq!(
            hit.relevance(&record.id),
            None,
            "no query ran, so {:?} has no relevance to report; a zero would be a \
             fabricated number for a memory nothing ever scored",
            record.subject
        );
    }
}

/// A path lookup normalises its argument the same way the writer normalised
/// the column.
///
/// `memory_files.path` is repo-relative, `/`-separated and canonical because
/// `normalize_observed_path` made it so on the way in. A lookup that
/// normalised differently — or not at all — would match nothing and say so
/// silently, which is the failure mode migration 17's own text calls
/// invisible.
#[test]
fn a_lookup_and_a_write_agree_on_how_a_path_is_spelled() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let project = fixture.memory();
    let store = project.store();

    let id = store
        .record(
            NewMemory::new(MemoryKind::Finding, "the loader mmaps the index")
                .with_subject(Some("index loading")),
        )
        .unwrap()
        .id;
    store
        .record_observed_files(
            std::slice::from_ref(&id),
            &["./src//index/loader.rs".to_owned()],
        )
        .unwrap();

    for spelling in [
        "src/index/loader.rs",
        "./src/index/loader.rs",
        "src\\index\\loader.rs",
        "  src/index//loader.rs  ",
    ] {
        let hit = store.for_path(spelling, SearchScope::Current, 10).unwrap();
        assert_eq!(
            returned(&hit)
                .into_iter()
                .map(|record| record.id.clone())
                .collect::<Vec<MemoryId>>(),
            vec![id.clone()],
            "{spelling:?} names the same file the writer canonicalised, so the lookup \
             must find it"
        );
    }

    let refused = store
        .for_path("../outside/loader.rs", SearchScope::Current, 10)
        .unwrap();
    assert!(
        returned(&refused).is_empty(),
        "a path the writer would have refused cannot match a row, and the lookup says \
         so with an empty result rather than an error"
    );
}

/// Cross-project isolation: a memory belonging to another project is never
/// returned, however its association row got into the file.
///
/// Both rows are planted with the triggers dropped — the only way to model a
/// row that reached the file by a route the guard never saw: a restored
/// backup, a hand-edited file, a build whose schema predates it. The `WHERE`
/// clause is the entire boundary at read time, and a file path is user data:
/// if scoping is omitted, another project's memory body is what a reader
/// gets.
#[test]
fn a_memory_planted_from_another_project_is_never_returned_by_a_path_lookup() {
    let tmp = tempfile::tempdir().unwrap();
    let alpha = Fixture::new(tmp.path(), "alpha");
    let project = alpha.memory();
    let store = project.store();

    let mine = store
        .record(
            NewMemory::new(MemoryKind::Finding, "alpha writes its own audit line")
                .with_subject(Some("alpha audit")),
        )
        .unwrap()
        .id;
    store
        .record_observed_files(std::slice::from_ref(&mine), &["src/shared.rs".to_owned()])
        .unwrap();

    let conn = alpha.raw_connection();
    // Shape one: both rows are beta's, as a restored backup of another
    // project would leave them.
    plant_foreign_association(
        &conn,
        "beta-memory-id",
        "beta-project-id",
        "beta-project-id",
        "src/shared.rs",
    );
    // Shape two: beta's memory reached through an association row that looks
    // local. `memory_files.project_id` cannot exclude this one, so the
    // memory row's own scoping is the only thing standing between beta's
    // body and alpha's reader.
    plant_foreign_association(
        &conn,
        "beta-memory-through-a-local-looking-row",
        "beta-project-id",
        project.store().project_id(),
        "src/shared.rs",
    );

    let hit = store
        .for_path("src/shared.rs", SearchScope::Current, 10)
        .unwrap();
    assert_eq!(
        returned(&hit)
            .into_iter()
            .map(|record| record.id.clone())
            .collect::<Vec<MemoryId>>(),
        vec![mine],
        "the premise is alpha's own memory coming back; the foreign one shares the \
         path and the file, and only the project scoping can exclude it"
    );
}

/// The ladder ordering holds for a path lookup, exactly as it does for a
/// query: currently active invariants and constraints are their own group,
/// and inside the remainder a current decision outranks an idea.
///
/// This is what reusing `group()` and the shared ranking buys — a path lookup
/// that ordered its own way would let the same memories rank differently
/// depending on which door asked.
#[test]
fn a_path_lookup_ranks_through_the_same_ladder_a_query_does() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let project = fixture.memory();
    let store = project.store();
    let ids = seed_ranking_corpus(&store);
    store
        .record_observed_files(&ids, &["src/marmot.rs".to_owned()])
        .unwrap();

    let hit = store
        .for_path("src/marmot.rs", SearchScope::Current, 10)
        .unwrap();

    let mut invariants = subjects(&hit.invariants_and_constraints);
    invariants.sort();
    assert_eq!(
        invariants,
        [
            "marmot credential handling",
            "marmot payload size",
            "marmot timeout",
        ],
        "the invariant and the two constraints are currently active, so they are the \
         group line 929 asks to be kept apart"
    );

    let other = subjects(&hit.other);
    assert_eq!(
        other.len(),
        3,
        "the two decisions and the idea are everything else"
    );
    assert_eq!(
        other.last().map(String::as_str),
        Some("marmot caching idea"),
        "an idea is on the lowest rung and never outranks a current decision, whichever \
         door returned it: got {other:?}"
    );
}

/// History is only ever visible when it is explicitly asked for, on this door
/// as on the other two.
///
/// `SearchScope` exists so that a superseded memory cannot reach a caller who
/// asked for current project knowledge. A path lookup that ignored its scope
/// would return a decision the project has already replaced, presented beside
/// the one that replaced it — and the association row survives the
/// supersession, so this is not a hypothetical branch.
#[test]
fn a_path_lookup_shows_history_only_when_history_is_asked_for() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let project = fixture.memory();
    let store = project.store();

    let record = |body: &str| {
        store
            .record(NewMemory::new(MemoryKind::Decision, body).with_subject(Some(body)))
            .unwrap()
            .id
    };
    let old = record("the cache is written through");
    let new = record("the cache is written back");
    store
        .record_observed_files(&[old.clone(), new.clone()], &["src/cache.rs".to_owned()])
        .unwrap();
    store.supersede(&old, &new).unwrap();

    let current = store
        .for_path("src/cache.rs", SearchScope::Current, 10)
        .unwrap();
    assert_eq!(
        returned(&current)
            .into_iter()
            .map(|record| record.id.clone())
            .collect::<Vec<MemoryId>>(),
        vec![new.clone()],
        "a superseded memory is history, and a default lookup returns current project \
         knowledge only"
    );

    let historical = store
        .for_path("src/cache.rs", SearchScope::Historical, 10)
        .unwrap();
    let mut ids: Vec<String> = returned(&historical)
        .into_iter()
        .map(|record| record.id.as_str().to_owned())
        .collect();
    ids.sort();
    let mut expected = vec![old.as_str().to_owned(), new.as_str().to_owned()];
    expected.sort();
    assert_eq!(
        ids, expected,
        "the explicit ask is what makes history visible, and the association row \
         outlives the supersession"
    );
}

/// Insert a memory belonging to `memory_project` and an association row
/// belonging to `association_project`, bypassing both project-id triggers —
/// the only way to plant rows belonging to another project, which is exactly
/// what those triggers exist to prevent.
///
/// The two projects are separate arguments because the read door carries two
/// predicates and they guard different accidents: a wholly foreign pair (a
/// restored backup of another project) and a foreign memory reached through
/// an association row that looks local (a hand-edited file, a partial
/// restore). The second shape is the one only `memories.project_id` excludes.
fn plant_foreign_association(
    conn: &Connection,
    memory_id: &str,
    memory_project: &str,
    association_project: &str,
    path: &str,
) {
    conn.execute_batch(
        "DROP TRIGGER memories_reject_foreign_project_insert;
         DROP TRIGGER memory_files_reject_foreign_project_insert;",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO memories (id, project_id, kind, status, subject, body, \
         created_at, updated_at) \
         VALUES (?1, ?2, 'finding', 'active', 'beta secret', 'beta knows something', 0, 0)",
        rusqlite::params![memory_id, memory_project],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO memory_files (project_id, memory_id, path, provenance, observed_at) \
         VALUES (?1, ?2, ?3, 'observed', 0)",
        rusqlite::params![association_project, memory_id, path],
    )
    .unwrap();
    conn.execute_batch(
        "CREATE TRIGGER memories_reject_foreign_project_insert
         BEFORE INSERT ON memories
         FOR EACH ROW
         WHEN NEW.project_id IS NOT (
             SELECT value FROM project_metadata WHERE key = 'project_id'
         )
         BEGIN
             SELECT RAISE(ABORT, 'memory belongs to a different project');
         END;
         CREATE TRIGGER memory_files_reject_foreign_project_insert
         BEFORE INSERT ON memory_files
         FOR EACH ROW
         WHEN NEW.project_id IS NOT (
             SELECT value FROM project_metadata WHERE key = 'project_id'
         )
         BEGIN
             SELECT RAISE(ABORT, 'memory file association belongs to a different project');
         END;",
    )
    .unwrap();
}
