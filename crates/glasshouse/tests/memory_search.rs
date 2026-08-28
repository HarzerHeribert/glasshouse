//! Acceptance tests for Phase 23's free-text memory search
//! (`glasshouse::memory::search`).
//!
//! Exercises the crate exactly as a caller does: through
//! `glasshouse::memory::ProjectMemory::open`, never through anything private
//! to the crate.

use std::sync::{Arc, Mutex};

use clap::Parser;

use glasshouse::memory::search::SearchScope;
use glasshouse::memory::{
    DecisionProvenance, MemoryAuthority, MemoryKind, MemoryStatus, NewMemory, ProjectMemory,
    ProjectPhase,
};
use glasshouse::{Cli, Runtime, bootstrap};

/// A bootstrapped project, with its temp directories kept alive for the
/// duration of the test.
struct Fixture {
    _workspace: tempfile::TempDir,
    _data: tempfile::TempDir,
    runtime: Runtime,
}

impl Fixture {
    fn new() -> Self {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(workspace.path().join(".git")).unwrap();
        let data = tempfile::tempdir().unwrap();

        let cli = Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            data.path().to_str().unwrap(),
            "--config-dir",
            data.path().to_str().unwrap(),
        ])
        .unwrap();
        let runtime = bootstrap(&cli, workspace.path()).unwrap();

        Self {
            _workspace: workspace,
            _data: data,
            runtime,
        }
    }

    fn open(&self) -> ProjectMemory {
        ProjectMemory::open(&self.runtime).unwrap()
    }
}

/// Phase 23 — only a memory whose text matches the query comes back.
#[test]
fn a_search_returns_only_the_memory_that_contains_the_term() {
    let fixture = Fixture::new();
    let project = fixture.open();
    let store = project.store();

    let matching = store
        .record(NewMemory::new(
            MemoryKind::Finding,
            "The billing worker retries a failed webhook three times before giving up.",
        ))
        .unwrap();
    store
        .record(NewMemory::new(
            MemoryKind::Finding,
            "The onboarding email is sent from a queue that drains every minute.",
        ))
        .unwrap();

    let results = store.search("webhook", SearchScope::Current, 10).unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, matching.id);
}

/// Phase 23 — BM25 order, not just membership. A memory whose subject and
/// body both carry the term outranks one that mentions it once in a long
/// body, and `bm25()`'s negative-is-better direction must be handled the
/// right way round to get this order at all.
#[test]
fn results_are_ordered_by_bm25_relevance_best_first() {
    let fixture = Fixture::new();
    let project = fixture.open();
    let store = project.store();

    let strong = store
        .record(
            NewMemory::new(
                MemoryKind::Decision,
                "We chose postgres for the primary store because postgres already \
                 backs three other services and postgres's replication tooling is \
                 something the team already operates.",
            )
            .with_subject(Some("postgres as the primary store")),
        )
        .unwrap();

    let weak = store
        .record(NewMemory::new(
            MemoryKind::Finding,
            "This note wanders across a dozen unrelated topics: the release \
             calendar, a flaky end-to-end test, the design of the settings \
             overlay, a rename of an internal crate, the on-call rotation, a \
             typo in the changelog, the icon set for the TUI, and, in passing, \
             a single mention of postgres somewhere in the middle of it all.",
        ))
        .unwrap();

    let results = store.search("postgres", SearchScope::Current, 10).unwrap();

    assert_eq!(
        results.iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
        vec![strong.id, weak.id],
        "the memory carrying the term in both subject and body must rank first"
    );
}

/// Phase 23 — a superseded memory is invisible to a default search and
/// visible only once history is asked for explicitly.
#[test]
fn a_superseded_memory_is_hidden_by_default_and_found_under_historical_scope() {
    let fixture = Fixture::new();
    let project = fixture.open();
    let store = project.store();

    let old = store
        .record(NewMemory::new(
            MemoryKind::Decision,
            "The zephyr cache was configured with a five minute TTL.",
        ))
        .unwrap();
    let new = store
        .record(NewMemory::new(
            MemoryKind::Decision,
            "The zephyr cache TTL was raised to one hour after profiling.",
        ))
        .unwrap();
    store.supersede(&old.id, &new.id).unwrap();

    let current = store.search("zephyr", SearchScope::Current, 10).unwrap();
    assert!(
        current.iter().all(|r| r.id != old.id),
        "a superseded memory must not appear in a default search"
    );

    let historical = store.search("zephyr", SearchScope::Historical, 10).unwrap();
    assert!(
        historical.iter().any(|r| r.id == old.id),
        "a superseded memory must be findable once history is asked for"
    );
    let found = historical.iter().find(|r| r.id == old.id).unwrap();
    assert_eq!(found.status, MemoryStatus::Superseded);
}

/// Phase 23 — provenance round-trips as `Option`, never invented.
#[test]
fn provenance_round_trips_as_present_or_absent_never_invented() {
    let fixture = Fixture::new();
    let project = fixture.open();
    let store = project.store();

    let with_provenance = store
        .record(
            NewMemory::new(MemoryKind::Finding, "The gizmo export job is idempotent.")
                .with_source_session(Some("session-42"))
                .with_source_commit(Some("abc1234")),
        )
        .unwrap();
    let without_provenance = store
        .record(NewMemory::new(
            MemoryKind::Finding,
            "The gizmo import job is not idempotent and must not be retried blindly.",
        ))
        .unwrap();

    let results = store.search("gizmo", SearchScope::Current, 10).unwrap();

    let with_result = results
        .iter()
        .find(|r| r.id == with_provenance.id)
        .expect("the memory recorded with provenance must be found");
    assert_eq!(with_result.source_session_id.as_deref(), Some("session-42"));
    assert_eq!(with_result.source_commit.as_deref(), Some("abc1234"));

    let without_result = results
        .iter()
        .find(|r| r.id == without_provenance.id)
        .expect("the memory recorded without provenance must be found");
    assert_eq!(without_result.source_session_id, None);
    assert_eq!(without_result.source_commit, None);
}

/// Phase 23 — free-form text is not FTS5 syntax. None of these must ever
/// produce an `Err`, even though every one of them is FTS5 operator syntax on
/// its own.
#[test]
fn fts5_operator_characters_in_a_query_never_produce_an_error() {
    let fixture = Fixture::new();
    let project = fixture.open();
    let store = project.store();

    store
        .record(NewMemory::new(
            MemoryKind::Finding,
            "The search index tokenizes on unicode word boundaries.",
        ))
        .unwrap();

    for query in ["\"", "*", "AND", "NEAR(", "a:b", "-x"] {
        let result = store.search(query, SearchScope::Current, 10);
        assert!(
            result.is_ok(),
            "query {query:?} must not error, got {result:?}"
        );
    }
}

/// Phase 23 — a query with nothing searchable in it returns no results and no
/// error, rather than reaching SQLite at all.
#[test]
fn an_empty_or_punctuation_only_query_returns_no_results_and_no_error() {
    let fixture = Fixture::new();
    let project = fixture.open();
    let store = project.store();

    store
        .record(NewMemory::new(
            MemoryKind::Finding,
            "This memory exists so the table is not itself empty.",
        ))
        .unwrap();

    for query in ["", "   ", "...", "---", "??? !!!"] {
        let results = store
            .search(query, SearchScope::Current, 10)
            .unwrap_or_else(|error| panic!("query {query:?} must not error: {error}"));
        assert!(
            results.is_empty(),
            "query {query:?} must return no results, got {results:?}"
        );
    }
}

/// Phase 23 — the result count honours the requested limit.
#[test]
fn the_result_count_honours_the_requested_limit() {
    let fixture = Fixture::new();
    let project = fixture.open();
    let store = project.store();

    for n in 0..5 {
        store
            .record(NewMemory::new(
                MemoryKind::Finding,
                format!("The turnip counter reached value number {n} during the test run."),
            ))
            .unwrap();
    }

    let results = store.search("turnip", SearchScope::Current, 2).unwrap();
    assert_eq!(results.len(), 2);

    let unbounded = store.search("turnip", SearchScope::Current, 10).unwrap();
    assert_eq!(unbounded.len(), 5);
}

// -------------------------------------------------------------------------
// Phase 21F — memory retrieval quality.
// -------------------------------------------------------------------------

/// Line 929 — current invariants and constraints come back distinguishably
/// from everything else a search matched, through the exact grouping
/// `main.rs`'s `memory_report` and the control API's `query_memory` both
/// render from.
#[test]
fn search_grouped_separates_current_invariants_and_constraints_from_everything_else() {
    let fixture = Fixture::new();
    let project = fixture.open();
    let store = project.store();

    let invariant = store
        .record(
            NewMemory::new(MemoryKind::Constraint, "sparrow tokens are never logged")
                .with_subject(Some("sparrow token handling"))
                .with_authority(Some(MemoryAuthority::Invariant)),
        )
        .unwrap();
    let constraint = store
        .record(
            NewMemory::new(MemoryKind::Constraint, "sparrow requests time out at 30s")
                .with_subject(Some("sparrow timeout"))
                .with_authority(Some(MemoryAuthority::Constraint)),
        )
        .unwrap();
    let decision = store
        .record(
            NewMemory::new(MemoryKind::Decision, "sparrow retries three times")
                .with_subject(Some("sparrow retries"))
                .with_authority(Some(MemoryAuthority::Decision)),
        )
        .unwrap();
    let idea = store
        .record(
            NewMemory::new(MemoryKind::Finding, "sparrow could maybe batch requests")
                .with_subject(Some("sparrow batching idea"))
                .with_authority(Some(MemoryAuthority::Idea)),
        )
        .unwrap();

    let grouped = store
        .search_grouped("sparrow", SearchScope::Current, 10)
        .unwrap();

    let rule_ids: Vec<_> = grouped
        .invariants_and_constraints
        .iter()
        .map(|r| r.id.clone())
        .collect();
    assert!(
        rule_ids.contains(&invariant.id),
        "an active invariant must land in the rules group: {rule_ids:?}"
    );
    assert!(
        rule_ids.contains(&constraint.id),
        "an active constraint must land in the rules group: {rule_ids:?}"
    );
    assert!(
        !rule_ids.contains(&decision.id),
        "an ordinary decision must not land in the rules group: {rule_ids:?}"
    );
    assert!(
        !rule_ids.contains(&idea.id),
        "an idea must not land in the rules group: {rule_ids:?}"
    );

    let other_ids: Vec<_> = grouped.other.iter().map(|r| r.id.clone()).collect();
    assert!(other_ids.contains(&decision.id));
    assert!(other_ids.contains(&idea.id));
    assert!(!other_ids.contains(&invariant.id));
    assert!(!other_ids.contains(&constraint.id));
}

/// Line 929 — a memory whose authority is invariant or constraint but that
/// is no longer active is history, not a current rule, even when
/// `SearchScope::Historical` returns it at all.
#[test]
fn search_grouped_excludes_an_invalidated_invariant_from_the_rules_group_even_under_history() {
    let fixture = Fixture::new();
    let project = fixture.open();
    let store = project.store();

    let invariant = store
        .record(
            NewMemory::new(MemoryKind::Constraint, "kiwi backups run nightly")
                .with_authority(Some(MemoryAuthority::Invariant)),
        )
        .unwrap();
    store
        .set_status(&invariant.id, MemoryStatus::Invalidated)
        .unwrap();

    let grouped = store
        .search_grouped("kiwi", SearchScope::Historical, 10)
        .unwrap();

    assert!(
        !grouped
            .invariants_and_constraints
            .iter()
            .any(|r| r.id == invariant.id),
        "an invalidated invariant must not be presented as a current rule"
    );
    assert!(
        grouped.other.iter().any(|r| r.id == invariant.id),
        "an invalidated invariant must still be reachable as history"
    );
}

/// Line 931 — acceptance test 2, load-bearing: two memories of equal
/// relevance and authority, the one that has been validated ranks above the
/// one that has not.
#[test]
fn a_validated_memory_outranks_an_equally_relevant_equally_authoritative_unvalidated_one() {
    let fixture = Fixture::new();
    let ticks = Arc::new(Mutex::new(1_000_000i64));
    let clock = Arc::clone(&ticks);
    let project =
        ProjectMemory::open_with_clock(&fixture.runtime, Arc::new(move || *clock.lock().unwrap()))
            .unwrap();
    let store = project.store();

    let unvalidated = store
        .record(
            NewMemory::new(
                MemoryKind::Decision,
                "the wombat cache holds one entry per key",
            )
            .with_authority(Some(MemoryAuthority::Decision)),
        )
        .unwrap();
    let validated = store
        .record(
            NewMemory::new(
                MemoryKind::Decision,
                "the wombat cache holds one entry per key indeed",
            )
            .with_authority(Some(MemoryAuthority::Decision)),
        )
        .unwrap();

    // Both age a hundred days with neither reaffirmed...
    *ticks.lock().unwrap() += 100 * 86_400;
    // ...then one is validated, and another day passes.
    store.reaffirm(&validated.id).unwrap();
    *ticks.lock().unwrap() += 86_400;

    let results = store.search("wombat", SearchScope::Current, 10).unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(
        results[0].id, validated.id,
        "the validated memory must outrank its equally relevant, equally authoritative, \
         unvalidated twin:\n{results:#?}"
    );
    assert_eq!(results[1].id, unvalidated.id);
}

/// Line 933, and its exception — Phase 21D line 901 already requires that
/// reaffirming restores weight, and this must not contradict it: a
/// prototype-phase decision that is never reaffirmed is penalised; the same
/// decision reaffirmed is not.
#[test]
fn a_prototype_phase_decision_is_penalized_until_it_is_reaffirmed() {
    let fixture = Fixture::new();
    let ticks = Arc::new(Mutex::new(1_000_000i64));
    let clock = Arc::clone(&ticks);
    let project =
        ProjectMemory::open_with_clock(&fixture.runtime, Arc::new(move || *clock.lock().unwrap()))
            .unwrap();
    let store = project.store();

    let exploratory = store
        .record(
            NewMemory::new(MemoryKind::Decision, "the toucan queue polls every second")
                .with_authority(Some(MemoryAuthority::Decision))
                .with_provenance(DecisionProvenance {
                    project_phase: Some(ProjectPhase::Prototype),
                    ..DecisionProvenance::default()
                }),
        )
        .unwrap();
    let production = store
        .record(
            NewMemory::new(
                MemoryKind::Decision,
                "the toucan queue polls every second too",
            )
            .with_authority(Some(MemoryAuthority::Decision))
            .with_provenance(DecisionProvenance {
                project_phase: Some(ProjectPhase::Production),
                ..DecisionProvenance::default()
            }),
        )
        .unwrap();

    let before = store.search("toucan", SearchScope::Current, 10).unwrap();
    assert_eq!(before.len(), 2);
    assert_eq!(
        before[0].id, production.id,
        "an exploratory, never-reaffirmed decision must rank below an equally fresh \
         production-phase one:\n{before:#?}"
    );
    assert_eq!(before[1].id, exploratory.id);

    // Reaffirming the exploratory decision lifts the penalty entirely.
    store.reaffirm(&exploratory.id).unwrap();
    *ticks.lock().unwrap() += 1;

    let after = store.search("toucan", SearchScope::Current, 10).unwrap();
    assert_eq!(
        after[0].id, exploratory.id,
        "reaffirming a prototype-phase decision must lift the penalty:\n{after:#?}"
    );
}
