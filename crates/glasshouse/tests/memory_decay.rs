//! Phase 21D: memory age and relevance decay, exercised through
//! `glasshouse::memory::MemoryStore::search` — the one production retrieval
//! path this decay policy is wired into.
//!
//! An integration test on purpose, for the same reason `memory_store.rs` is.

use std::path::Path;
use std::sync::{Arc, Mutex};

use clap::Parser;

use glasshouse::memory::search::SearchScope;
use glasshouse::memory::{MemoryAuthority, MemoryKind, NewMemory, ProjectMemory};
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

    /// A memory store whose clock is under the test's control, so an
    /// arbitrary age can be asserted exactly instead of slept for.
    fn memory_at(&self, ticks: &Arc<Mutex<i64>>) -> ProjectMemory {
        let ticks = Arc::clone(ticks);
        ProjectMemory::open_with_clock(&self.runtime, Arc::new(move || *ticks.lock().unwrap()))
            .unwrap()
    }
}

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

const DAY: i64 = 86_400;

/// Line 904: *"avoid resurfacing low-authority stale memories merely because
/// their wording has high lexical similarity to the current task."*
///
/// An old `idea` repeats the search term heavily — the strongest possible
/// BM25 match — while a same-instant-fresh `invariant` mentions it once, in
/// a longer sentence, and matches far more weakly on relevance alone. The
/// idea's raw relevance must lose the ranking to the invariant once decay is
/// applied, which is the one property a unit test of the multiplier cannot
/// demonstrate — see `retrieval_weight`'s own tests for that half.
#[test]
fn an_old_idea_with_a_strong_match_ranks_below_a_fresh_invariant_with_a_weak_one() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let ticks = Arc::new(Mutex::new(1_000_000i64));
    let memory = fixture.memory_at(&ticks);
    let store = memory.store();

    let old_idea = store
        .record(
            NewMemory::new(
                MemoryKind::Finding,
                "flamingo flamingo flamingo flamingo flamingo flamingo flamingo",
            )
            .with_subject(Some("flamingo flamingo flamingo"))
            .with_authority(Some(MemoryAuthority::Idea)),
        )
        .unwrap();

    // A year old: far past the idea's 30-day half-life, so its weight sits
    // at the policy's floor.
    *ticks.lock().unwrap() += 365 * DAY;

    let fresh_invariant = store
        .record(
            NewMemory::new(
                MemoryKind::Constraint,
                "This service must never hold a customer's payment card number; \
                 it is out of scope even for debugging, and a flamingo appears \
                 here only as an unrelated code name for the batching queue.",
            )
            .with_authority(Some(MemoryAuthority::Invariant)),
        )
        .unwrap();

    let results = store.search("flamingo", SearchScope::Current, 10).unwrap();
    assert_eq!(results.len(), 2, "both memories must match the query");
    assert_eq!(
        results[0].id, fresh_invariant.id,
        "the fresh invariant must outrank the heavily-decayed idea despite \
         matching the query text far more weakly:\n{results:#?}"
    );
    assert_eq!(results[1].id, old_idea.id);
}

/// Line 898: age alone must never demote a genuine invariant, however old it
/// is. Both memories mention the term once, in a near-identical sentence, so
/// their raw relevance is close; ten years in, only the one that decays
/// should have lost ground — the invariant must not have.
#[test]
fn an_ancient_invariant_is_not_demoted_by_age_the_way_an_equally_old_decision_is() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let ticks = Arc::new(Mutex::new(1_000_000i64));
    let memory = fixture.memory_at(&ticks);
    let store = memory.store();

    let ancient_invariant = store
        .record(
            NewMemory::new(
                MemoryKind::Constraint,
                "The tarragon service key is never logged, not even redacted.",
            )
            .with_authority(Some(MemoryAuthority::Invariant)),
        )
        .unwrap();
    let ancient_decision = store
        .record(
            NewMemory::new(
                MemoryKind::Decision,
                "The tarragon retry queue is drained every ten seconds, not redacted.",
            )
            .with_authority(Some(MemoryAuthority::Decision)),
        )
        .unwrap();

    *ticks.lock().unwrap() += 10 * 365 * DAY;

    let results = store.search("tarragon", SearchScope::Current, 10).unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(
        results[0].id, ancient_invariant.id,
        "a ten-year-old invariant must not decay the way an equally old decision \
         does, even though both matched about as well when they were written:\n{results:#?}"
    );
    assert_eq!(results[1].id, ancient_decision.id);
}

/// Line 901: reaffirming regains retrieval weight without moving
/// `created_at`. Two decisions of equal age and near-identical wording, one
/// reaffirmed yesterday and one never reaffirmed, must not tie: the
/// reaffirmed one ranks first, and neither one's `created_at` moves.
#[test]
fn a_reaffirmed_decision_outranks_an_equally_old_unreaffirmed_twin() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let ticks = Arc::new(Mutex::new(1_000_000i64));
    let memory = fixture.memory_at(&ticks);
    let store = memory.store();

    let never_reaffirmed = store
        .record(
            NewMemory::new(
                MemoryKind::Decision,
                "malachite retries three times before giving up",
            )
            .with_authority(Some(MemoryAuthority::Decision)),
        )
        .unwrap();
    let will_be_reaffirmed = store
        .record(
            NewMemory::new(
                MemoryKind::Decision,
                "malachite retries three times, then gives up",
            )
            .with_authority(Some(MemoryAuthority::Decision)),
        )
        .unwrap();

    // Both age past most of a decision's half-life...
    *ticks.lock().unwrap() += 100 * DAY;
    // ...then one is reaffirmed, yesterday relative to the search below.
    *ticks.lock().unwrap() += DAY;
    let reaffirmed = store.reaffirm(&will_be_reaffirmed.id).unwrap();
    assert_eq!(
        reaffirmed.created_at, will_be_reaffirmed.created_at,
        "reaffirming must never move created_at"
    );
    *ticks.lock().unwrap() += DAY;

    let results = store.search("malachite", SearchScope::Current, 10).unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(
        results[0].id, will_be_reaffirmed.id,
        "the recently reaffirmed decision must outrank its equally old, \
         never-reaffirmed twin:\n{results:#?}"
    );
    assert_eq!(results[1].id, never_reaffirmed.id);
}

/// Line 899: a newer validated decision is preferred over an older
/// unvalidated one addressing the same concern — the ordinary case, with no
/// reaffirming involved at all, just two creation times.
#[test]
fn a_newer_decision_outranks_an_older_one_about_the_same_concern() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let ticks = Arc::new(Mutex::new(1_000_000i64));
    let memory = fixture.memory_at(&ticks);
    let store = memory.store();

    let older = store
        .record(
            NewMemory::new(MemoryKind::Decision, "peridot batches events every 500ms")
                .with_authority(Some(MemoryAuthority::Decision)),
        )
        .unwrap();

    *ticks.lock().unwrap() += 200 * DAY;

    let newer = store
        .record(
            NewMemory::new(
                MemoryKind::Decision,
                "peridot batches events every 200ms now",
            )
            .with_authority(Some(MemoryAuthority::Decision)),
        )
        .unwrap();

    let results = store.search("peridot", SearchScope::Current, 10).unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(
        results[0].id, newer.id,
        "the newer decision must outrank the older one about the same concern:\n{results:#?}"
    );
    assert_eq!(results[1].id, older.id);
}

/// Every durable memory's age is tracked from `created_at`, which `record`
/// stamps unconditionally — Phase 21D's first line, and the foundation the
/// rest of this file's decay tests rest on.
#[test]
fn every_recorded_memory_carries_a_creation_timestamp() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let ticks = Arc::new(Mutex::new(42_i64));
    let memory = fixture.memory_at(&ticks);
    let store = memory.store();

    let recorded = store
        .record(NewMemory::new(
            MemoryKind::Finding,
            "sunstone starts cold in under 3s",
        ))
        .unwrap();
    assert_eq!(recorded.created_at, 42);
}

/// The over-fetch is load-bearing, and until this test nothing proved it.
///
/// `search` pulls `overfetch_limit(limit)` candidates from SQLite — five times
/// the caller's limit — ranks them by `relevance * retrieval_weight`, and only
/// then truncates. With a plain `LIMIT limit`, decay could reorder the rows
/// SQLite happened to return and **never promote** a memory that ranked outside
/// the raw BM25 top-`limit`, which is precisely the case decay exists for: a
/// fresh invariant that matches weakly, buried under a pile of stale ideas that
/// match strongly.
///
/// Every other test in this file uses a corpus smaller than its `limit`, so the
/// over-fetch is invisible to all of them — the integrator removed it as a
/// mutation and the whole suite stayed green. This test asks for `limit = 3`
/// against a corpus of ten, with the memory that must win sitting last on raw
/// relevance, so it can only be returned if more than three rows were fetched.
#[test]
fn decay_can_promote_a_memory_that_raw_relevance_would_have_truncated_away() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let ticks = Arc::new(Mutex::new(1_000_000i64));
    let memory = fixture.memory_at(&ticks);
    let store = memory.store();

    // Nine stale ideas, each repeating the term so their raw BM25 relevance is
    // high, recorded first so they are also the oldest.
    for n in 0..9 {
        store
            .record(
                NewMemory::new(
                    MemoryKind::Finding,
                    "pelican pelican pelican pelican pelican pelican pelican",
                )
                .with_subject(Some(&format!("pelican note {n}")))
                .with_authority(Some(MemoryAuthority::Idea)),
            )
            .unwrap();
    }

    // A year on — well past the ideas' 30-day half-life.
    *ticks.lock().unwrap() += 365 * DAY;

    // One fresh invariant that mentions the term exactly once, in a long
    // sentence: the weakest raw match in the corpus by construction.
    let fresh_invariant = store
        .record(
            NewMemory::new(
                MemoryKind::Constraint,
                "Outbound webhook bodies are never written to the debug log, \
                 not even truncated, and the pelican queue is no exception to \
                 that rule however convenient it would be while tracing a \
                 delivery failure end to end.",
            )
            .with_authority(Some(MemoryAuthority::Invariant)),
        )
        .unwrap();

    // Ask for three. On raw relevance the invariant is tenth of ten and could
    // not appear at all; only the over-fetch lets decay lift it.
    let results = store.search("pelican", SearchScope::Current, 3).unwrap();

    assert_eq!(
        results.len(),
        3,
        "the search must fill its limit:\n{results:#?}"
    );
    assert_eq!(
        results[0].id, fresh_invariant.id,
        "the fresh invariant ranks tenth of ten on raw relevance, so it can \
         only lead once more than `limit` rows were fetched and decay applied — \
         this is the over-fetch, and without it the search cannot see it at \
         all:\n{results:#?}"
    );
}
