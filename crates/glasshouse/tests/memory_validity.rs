//! Phase 21C: a remembered decision can stop being binding, and Phase 22 line
//! 1063's conflict detection, which lives in the same retrieval path this
//! file exercises (`glasshouse::memory::MemoryStore::search`).
//!
//! An integration test on purpose, for the same reason `memory_store.rs` is:
//! every path here goes through `glasshouse::bootstrap`, so the migration,
//! the project binding and the triggers are all in play.

use std::path::Path;
use std::sync::{Arc, Mutex};

use clap::Parser;

use glasshouse::memory::search::SearchScope;
use glasshouse::memory::{
    MemoryAuthority, MemoryKind, MemoryStatus, NewMemory, ProjectMemory, ReviewReason,
};
use glasshouse::{Cli, Runtime};

/// A bootstrapped project inside `base`, sharing `base`'s data and config
/// roots. Copied from `tests/memory_store.rs`, per the packet.
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

    /// A memory store whose clock is under the test's control.
    fn memory_at(&self, ticks: &Arc<Mutex<i64>>) -> ProjectMemory {
        let ticks = Arc::clone(ticks);
        ProjectMemory::open_with_clock(&self.runtime, Arc::new(move || *ticks.lock().unwrap()))
            .unwrap()
    }
}

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

// -------------------------------------------------------------------------
// Explicit validity and invalidation conditions.
// -------------------------------------------------------------------------

/// Map lines 883-884: a durable memory may define explicit validity and
/// invalidation conditions when known, and absence stays `None` rather than
/// becoming an empty string — the same distinction every other free-text
/// provenance field in this table preserves.
#[test]
fn validity_and_invalidation_conditions_round_trip_and_absence_stays_none() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let with_conditions = store
        .record(
            NewMemory::new(MemoryKind::Decision, "The cache uses a five minute TTL.")
                .with_validity_conditions(Some(
                    "holds as long as the backing store's write latency stays under 50ms",
                ))
                .with_invalidation_conditions(Some(
                    "no longer holds once the backing store moves to a different region",
                )),
        )
        .unwrap();
    assert_eq!(
        with_conditions.validity_conditions.as_deref(),
        Some("holds as long as the backing store's write latency stays under 50ms")
    );
    assert_eq!(
        with_conditions.invalidation_conditions.as_deref(),
        Some("no longer holds once the backing store moves to a different region")
    );

    let without_conditions = store
        .record(NewMemory::new(
            MemoryKind::Finding,
            "The cache is namespaced per tenant.",
        ))
        .unwrap();
    assert_eq!(without_conditions.validity_conditions, None);
    assert_eq!(without_conditions.invalidation_conditions, None);

    // And both survive a read back through `get`, not only the value handed
    // back from `record`.
    let read_back = store.get(&with_conditions.id).unwrap().unwrap();
    assert_eq!(
        read_back.validity_conditions,
        with_conditions.validity_conditions
    );
    assert_eq!(
        read_back.invalidation_conditions,
        with_conditions.invalidation_conditions
    );
}

// -------------------------------------------------------------------------
// Marking a memory for review.
// -------------------------------------------------------------------------

/// Map lines 885-890, one value per line: every review reason can mark a
/// memory for review, records itself and a timestamp, moves the memory to
/// `NeedsReview`, and never moves `created_at`.
#[test]
fn every_review_reason_can_mark_a_memory_for_review_with_a_stated_cause() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let ticks = Arc::new(Mutex::new(1_000i64));
    let memory = fixture.memory_at(&ticks);
    let store = memory.store();

    assert_eq!(
        ReviewReason::ALL.len(),
        6,
        "the map names exactly six review reasons"
    );

    for reason in ReviewReason::ALL {
        let recorded = store
            .record(NewMemory::new(
                MemoryKind::Decision,
                format!(
                    "a decision that will be marked for review as {}",
                    reason.as_str()
                ),
            ))
            .unwrap();
        assert_eq!(recorded.review_reason, None);
        assert_eq!(
            recorded.review_marked_at, None,
            "unmarked means unknown, not zero"
        );

        *ticks.lock().unwrap() += 1;
        let now = *ticks.lock().unwrap();
        let marked = store.mark_for_review(&recorded.id, *reason).unwrap();

        assert_eq!(marked.status, MemoryStatus::NeedsReview);
        assert_eq!(marked.review_reason, Some(*reason));
        assert_eq!(marked.review_marked_at, Some(now));
        assert_eq!(
            marked.created_at, recorded.created_at,
            "marking for review must never move created_at"
        );
    }
}

// -------------------------------------------------------------------------
// Invalidation: never silently preserved as binding, never deleted,
// represented as history.
// -------------------------------------------------------------------------

/// Lines 891-893. Once a memory is `Invalidated`: it must never come back
/// from `binding()` or a default (`Current`) search as though it were still
/// in force; it must never be deleted — still `get`-able, still counted; and
/// it must still be findable once history is asked for explicitly, carrying
/// its real status rather than a laundered one.
#[test]
fn an_invalidated_memory_is_excluded_from_binding_and_current_search_but_never_deleted() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let decision = store
        .record(
            NewMemory::new(
                MemoryKind::Decision,
                "Zircon uses optimistic locking everywhere.",
            )
            .with_subject(Some("zircon locking strategy"))
            .with_authority(Some(MemoryAuthority::Constraint)),
        )
        .unwrap();

    // While active, it is both a current search result and a binding rule.
    assert!(
        store
            .search("zircon", SearchScope::Current, 10)
            .unwrap()
            .iter()
            .any(|m| m.id == decision.id)
    );
    assert!(
        store
            .binding(50)
            .unwrap()
            .iter()
            .any(|m| m.id == decision.id)
    );

    store
        .set_status(&decision.id, MemoryStatus::Invalidated)
        .unwrap();

    // Line 892: never silently preserved as binding after invalidation.
    assert!(
        !store
            .binding(50)
            .unwrap()
            .iter()
            .any(|m| m.id == decision.id),
        "an invalidated memory must never be returned as a current binding rule"
    );
    let current = store.search("zircon", SearchScope::Current, 10).unwrap();
    assert!(
        current.iter().all(|m| m.id != decision.id),
        "an invalidated memory must not appear in a default search"
    );

    // Line 893: represented as historical evidence, not erased.
    let historical = store.search("zircon", SearchScope::Historical, 10).unwrap();
    let found = historical
        .iter()
        .find(|m| m.id == decision.id)
        .expect("an invalidated memory must remain findable once history is asked for");
    assert_eq!(found.status, MemoryStatus::Invalidated);

    // Line 892's other half: never deleted. There is no method on
    // `MemoryStore` that removes a row at all — `get` and `count` are the
    // proof a caller can drive.
    assert!(store.get(&decision.id).unwrap().is_some());
    assert_eq!(store.count(MemoryStatus::Invalidated).unwrap(), 1);
}

/// Line 905: needs-review and conflicted memories are also excluded from
/// current retrieval while remaining reachable through explicit history
/// search — the same boundary the superseded case already proves in
/// `memory_search.rs`, extended to the two statuses this package adds
/// detection and marking for.
#[test]
fn needs_review_and_conflicted_memories_stay_out_of_current_search_but_are_findable_as_history() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let under_review = store
        .record(NewMemory::new(
            MemoryKind::Decision,
            "Onyx batches writes every 200ms.",
        ))
        .unwrap();
    store
        .mark_for_review(&under_review.id, ReviewReason::BenchmarkOrScale)
        .unwrap();

    let adopted = store
        .record(
            NewMemory::new(MemoryKind::Decision, "Onyx retries a failed write once.")
                .with_subject(Some("onyx write retries")),
        )
        .unwrap();
    let abandoned = store
        .record(
            NewMemory::new(
                MemoryKind::FailedAttempt,
                "Onyx retrying a failed write once caused duplicate writes downstream.",
            )
            .with_subject(Some("Onyx Write Retries")),
        )
        .unwrap();

    // Trigger detection through the one production retrieval path; the
    // subject-normalized, kind-opposed pair must be flagged conflicted.
    store.search("onyx", SearchScope::Current, 10).unwrap();
    assert_eq!(
        store.get(&adopted.id).unwrap().unwrap().status,
        MemoryStatus::Conflicted
    );
    assert_eq!(
        store.get(&abandoned.id).unwrap().unwrap().status,
        MemoryStatus::Conflicted
    );

    let current = store.search("onyx", SearchScope::Current, 10).unwrap();
    let historical = store.search("onyx", SearchScope::Historical, 10).unwrap();
    for id in [&under_review.id, &adopted.id, &abandoned.id] {
        assert!(
            current.iter().all(|m| &m.id != id),
            "{id} must be absent from a current search"
        );
        assert!(
            historical.iter().any(|m| &m.id == id),
            "{id} must still be findable once history is asked for"
        );
    }
}

/// The conflict detector is conservative: two memories about different
/// subjects, or two of the same disposition, are never flagged.
#[test]
fn unrelated_or_agreeing_memories_are_never_flagged_as_conflicted() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let a = store
        .record(
            NewMemory::new(MemoryKind::Decision, "Jasper uses a five minute cache TTL.")
                .with_subject(Some("jasper cache")),
        )
        .unwrap();
    let b = store
        .record(
            NewMemory::new(
                MemoryKind::FailedAttempt,
                "Jasper's retry backoff caused a thundering herd.",
            )
            .with_subject(Some("jasper retries")),
        )
        .unwrap();
    let c = store
        .record(
            NewMemory::new(MemoryKind::Decision, "Jasper reuses one HTTP client.")
                .with_subject(Some("jasper cache")),
        )
        .unwrap();

    store.search("jasper", SearchScope::Current, 10).unwrap();

    for id in [&a.id, &b.id, &c.id] {
        assert_eq!(
            store.get(id).unwrap().unwrap().status,
            MemoryStatus::Active,
            "{id} must not be flagged: different subjects, or the same disposition"
        );
    }
}
