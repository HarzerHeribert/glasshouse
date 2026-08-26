//! `memory::snapshot` against a real project database — Phase 26's
//! behavioral contract, proved end to end rather than against a mock store.
//!
//! Given a project whose SQLite database holds durable memories, when an
//! agent asks for the current-project snapshot, Glasshouse returns a short
//! structured result grouped by kind, each entry carrying its own
//! provenance, never exceeding its budget, and never presenting resolved or
//! superseded memories as current.

use clap::Parser;

use glasshouse::memory::snapshot::{SnapshotBudget, snapshot};
use glasshouse::memory::{
    ConflictResolver, MemoryAuthority, MemoryKind, MemoryStatus, NewMemory, ProjectMemory,
};
use glasshouse::{Cli, Runtime, bootstrap};

fn bootstrap_at(base: &std::path::Path, name: &str) -> Runtime {
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
    bootstrap(&cli, &root).unwrap()
}

/// A bootstrapped project with an open, project-bound memory store — what
/// every caller of `memory::snapshot` actually has.
struct Fixture {
    _tmp: tempfile::TempDir,
    memory: ProjectMemory,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = bootstrap_at(tmp.path(), name);
        let memory = ProjectMemory::open(&runtime).unwrap();
        Self { _tmp: tmp, memory }
    }
}

/// The capability, stated as a contract: every one of the six kinds lands in
/// its own section, and only there.
#[test]
fn every_one_of_the_six_kinds_lands_in_its_own_section_and_only_there() {
    let fixture = Fixture::new("alpha");
    let store = fixture.memory.store();

    let by_kind = [
        (MemoryKind::Decision, "a decision"),
        (MemoryKind::Constraint, "a constraint"),
        (MemoryKind::Feature, "a feature"),
        (MemoryKind::Finding, "a finding"),
        (MemoryKind::FailedAttempt, "a failed attempt"),
        (MemoryKind::Todo, "a todo"),
    ];
    for (kind, body) in by_kind {
        store.record(NewMemory::new(kind, body)).unwrap();
    }

    let result = snapshot(&store, &SnapshotBudget::default()).unwrap();
    assert_eq!(result.sections.len(), 6, "one section per kind, no fewer");

    for (kind, body) in by_kind {
        let section = result.section(kind).unwrap();
        assert_eq!(
            section.entries.len(),
            1,
            "kind {kind} should hold exactly the one memory recorded for it"
        );
        assert_eq!(section.entries[0].body, body);

        for (other_kind, _) in by_kind {
            if other_kind == kind {
                continue;
            }
            let other_section = result.section(other_kind).unwrap();
            assert!(
                other_section.entries.iter().all(|entry| entry.body != body),
                "{body} leaked into the {other_kind} section"
            );
        }
    }
}

/// A resolved todo remains queryable by id, but a snapshot must never
/// present it as open work — Phase 22's distinction, and the one this packet
/// calls out as most easily got wrong.
#[test]
fn a_resolved_todo_is_absent_from_open_work_but_still_retrievable_by_id() {
    let fixture = Fixture::new("alpha");
    let store = fixture.memory.store();

    let recorded = store
        .record(NewMemory::new(MemoryKind::Todo, "ship the thing"))
        .unwrap();
    store
        .set_status(&recorded.id, MemoryStatus::Resolved)
        .unwrap();

    let result = snapshot(&store, &SnapshotBudget::default()).unwrap();
    let todo_section = result.section(MemoryKind::Todo).unwrap();
    assert!(
        todo_section.entries.is_empty(),
        "a resolved todo must not appear as open work"
    );

    let fetched = store
        .get(&recorded.id)
        .unwrap()
        .expect("still queryable by id");
    assert_eq!(fetched.status, MemoryStatus::Resolved);
}

/// A superseded memory is absent from every current section, even the
/// section for its own kind.
#[test]
fn a_superseded_memory_is_absent_from_every_current_section() {
    let fixture = Fixture::new("alpha");
    let store = fixture.memory.store();

    let old = store
        .record(NewMemory::new(MemoryKind::Decision, "use approach A"))
        .unwrap();
    let replacement = store
        .record(NewMemory::new(
            MemoryKind::Decision,
            "use approach B instead",
        ))
        .unwrap();
    store.supersede(&old.id, &replacement.id).unwrap();

    let result = snapshot(&store, &SnapshotBudget::default()).unwrap();
    let ids: Vec<_> = result
        .sections
        .iter()
        .flat_map(|section| section.entries.iter())
        .map(|entry| &entry.id)
        .collect();
    assert!(
        !ids.contains(&&old.id),
        "the superseded memory must not appear as current"
    );
    assert!(
        ids.contains(&&replacement.id),
        "its replacement must appear as current"
    );
}

/// A conflicted memory is absent from every current section — a conflict is
/// exactly the opposite of settled knowledge.
#[test]
fn a_conflicted_memory_is_absent_from_every_current_section() {
    let fixture = Fixture::new("alpha");
    let store = fixture.memory.store();

    let one = store
        .record(NewMemory::new(MemoryKind::Constraint, "limit is 10MB"))
        .unwrap();
    let other = store
        .record(NewMemory::new(MemoryKind::Constraint, "limit is 50MB"))
        .unwrap();
    store.mark_conflicted(&one.id, &other.id).unwrap();

    let result = snapshot(&store, &SnapshotBudget::default()).unwrap();
    let ids: Vec<_> = result
        .sections
        .iter()
        .flat_map(|section| section.entries.iter())
        .map(|entry| &entry.id)
        .collect();
    assert!(!ids.contains(&&one.id));
    assert!(!ids.contains(&&other.id));

    // Still real, resolvable rows — review, not deletion.
    store
        .resolve_conflict(&one.id, MemoryStatus::Active, ConflictResolver::Reviewed)
        .unwrap();
}

/// Recording far more memories than the per-section cap must both cap the
/// returned entries AND report how many were left out — no silent
/// truncation.
#[test]
fn the_entry_cap_holds_and_the_elision_count_reports_the_rest() {
    let fixture = Fixture::new("alpha");
    let store = fixture.memory.store();

    let budget = SnapshotBudget::new(3, 280);
    for i in 0..10 {
        store
            .record(NewMemory::new(MemoryKind::Finding, format!("finding {i}")))
            .unwrap();
    }

    let result = snapshot(&store, &budget).unwrap();
    let section = result.section(MemoryKind::Finding).unwrap();
    assert_eq!(
        section.entries.len(),
        3,
        "capped at the budget's per-section limit"
    );
    assert_eq!(
        section.omitted, 7,
        "the other seven must be counted, not silently dropped"
    );
}

/// A body longer than the budget is cut to it, and the cut is visible on the
/// entry rather than indistinguishable from a naturally short body.
#[test]
fn a_long_body_is_shortened_to_the_budget_and_the_shortening_is_visible() {
    let fixture = Fixture::new("alpha");
    let store = fixture.memory.store();

    let long_body = "x".repeat(500);
    store
        .record(NewMemory::new(MemoryKind::Finding, long_body.clone()))
        .unwrap();
    let short_body = "short";
    store
        .record(NewMemory::new(MemoryKind::Finding, short_body))
        .unwrap();

    let budget = SnapshotBudget::new(10, 50);
    let result = snapshot(&store, &budget).unwrap();
    let section = result.section(MemoryKind::Finding).unwrap();

    let long_entry = section
        .entries
        .iter()
        .find(|entry| entry.body.starts_with('x'))
        .unwrap();
    assert_eq!(long_entry.body.chars().count(), 50);
    assert!(long_entry.body_truncated);

    let short_entry = section
        .entries
        .iter()
        .find(|entry| entry.body == short_body)
        .unwrap();
    assert!(!short_entry.body_truncated);
}

/// Provenance travels with every entry, and its absence is reported as
/// absence rather than as an empty string.
#[test]
fn provenance_round_trips_and_absent_provenance_is_absent_rather_than_empty() {
    let fixture = Fixture::new("alpha");
    let store = fixture.memory.store();

    store
        .record(
            NewMemory::new(MemoryKind::Finding, "with provenance")
                .with_source_session(Some("session-123"))
                .with_source_commit(Some("abc1234")),
        )
        .unwrap();
    store
        .record(NewMemory::new(MemoryKind::Finding, "without provenance"))
        .unwrap();

    let result = snapshot(&store, &SnapshotBudget::default()).unwrap();
    let section = result.section(MemoryKind::Finding).unwrap();

    let with_provenance = section
        .entries
        .iter()
        .find(|e| e.body == "with provenance")
        .unwrap();
    assert_eq!(
        with_provenance.source_session_id.as_deref(),
        Some("session-123")
    );
    assert_eq!(with_provenance.source_commit.as_deref(), Some("abc1234"));

    let without_provenance = section
        .entries
        .iter()
        .find(|e| e.body == "without provenance")
        .unwrap();
    assert_eq!(without_provenance.source_session_id, None);
    assert_eq!(without_provenance.source_commit, None);
}

/// An unclassified authority stays `None` through the snapshot — never
/// promoted to a class nobody assigned.
#[test]
fn an_unclassified_authority_stays_none_through_the_snapshot() {
    let fixture = Fixture::new("alpha");
    let store = fixture.memory.store();

    store
        .record(NewMemory::new(
            MemoryKind::Decision,
            "unclassified decision",
        ))
        .unwrap();
    store
        .record(
            NewMemory::new(MemoryKind::Decision, "classified decision")
                .with_authority(Some(MemoryAuthority::Decision)),
        )
        .unwrap();

    let result = snapshot(&store, &SnapshotBudget::default()).unwrap();
    let section = result.section(MemoryKind::Decision).unwrap();

    let unclassified = section
        .entries
        .iter()
        .find(|e| e.body == "unclassified decision")
        .unwrap();
    assert_eq!(unclassified.authority, None);

    let classified = section
        .entries
        .iter()
        .find(|e| e.body == "classified decision")
        .unwrap();
    assert_eq!(classified.authority, Some(MemoryAuthority::Decision));
}
