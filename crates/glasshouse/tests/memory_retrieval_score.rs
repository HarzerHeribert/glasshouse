//! Acceptance tests for the retrieval relevance leaving `MemoryStore::search`.
//!
//! Producer P7: `memory/search.rs` computed a BM25 relevance for every hit,
//! sorted on it, and then dropped it on the floor
//! (`.map(|(record, _)| record)`), so nothing downstream could ever see how
//! well anything had matched. These tests hold the number to four claims: it
//! survives the call, it is genuinely the query's answer rather than a
//! placeholder, it changed no ordering on the way out, and it is absent —
//! not zero — when there was nothing to score.
//!
//! **These tests deliberately assert nothing about a threshold.** A relevance
//! is a within-query match score, not a confidence; see
//! `RetrievalResult::relevance` and `memory::inject::briefing` for why map
//! line 1129 is still refused.
//!
//! Exercises the crate exactly as a caller does: through
//! `glasshouse::memory::ProjectMemory::open`, never through anything private
//! to the crate.

use clap::Parser;

use glasshouse::memory::search::{RetrievalResult, SearchScope};
use glasshouse::memory::{
    DecisionProvenance, MemoryAuthority, MemoryId, MemoryKind, MemoryStore, NewMemory,
    ProjectMemory,
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

/// Six memories that all match `quokka`, spread across three ladder rungs and
/// two authority classes so that every stage of the ranking has something to
/// do: the rung comparison separates the invariant from the idea, the blended
/// relevance/decay weight orders the four in the middle rung, and
/// `demote_thin_decisions` puts the well-proven decision ahead of the thin
/// one.
///
/// # Two of the middle rung are constraints on purpose
///
/// An earlier version of this fixture put a single constraint between the two
/// decisions, and a deliberate mutation reversing the within-rung comparison
/// **survived it**. Reversing a three-element sequence leaves its centre
/// fixed, and the two elements it does move are exactly the two decisions
/// `demote_thin_decisions` then re-sorts back to well-proven-first — so the
/// perturbed sort reproduced the original order and the ordering test could
/// not see it. A second constraint gives the reversal something to move that
/// the demotion never touches.
///
/// Returned in declaration order, which is deliberately **not** the order a
/// search returns them in — see
/// `attaching_the_relevance_changed_neither_which_memories_come_back_nor_their_order`.
fn seed_ranking_corpus(store: &MemoryStore<'_>) -> Vec<MemoryId> {
    seed_ranking_corpus_with(store, MemoryAuthority::Decision)
}

/// [`seed_ranking_corpus`] with the thin decision's **authority class** as a
/// parameter and its text untouched.
///
/// `demote_thin_decisions` only permutes decisions that share an authority
/// class, so recording the thin decision under a different class turns the
/// demotion into a no-op without changing a single indexed byte — `subject`,
/// `body` and `rationale` are what `memories_fts` indexes, and authority is
/// not among them. Two stores seeded this way therefore give any one memory
/// the identical BM25 relevance for the identical query, while only one of
/// them runs the permutation. That is the fixed point
/// `a_relevance_stays_with_its_own_record_when_the_demotion_permutes` measures
/// against.
fn seed_ranking_corpus_with(
    store: &MemoryStore<'_>,
    thin_decision_authority: MemoryAuthority,
) -> Vec<MemoryId> {
    let record = |memory: NewMemory| store.record(memory).unwrap().id;

    vec![
        record(
            NewMemory::new(
                MemoryKind::Constraint,
                "a quokka credential is never written to a log",
            )
            .with_subject(Some("quokka credential handling"))
            .with_authority(Some(MemoryAuthority::Invariant)),
        ),
        record(
            NewMemory::new(MemoryKind::Constraint, "quokka requests time out at 30s")
                .with_subject(Some("quokka timeout"))
                .with_authority(Some(MemoryAuthority::Constraint)),
        ),
        record(
            NewMemory::new(
                MemoryKind::Constraint,
                "a quokka payload is never larger than one megabyte",
            )
            .with_subject(Some("quokka payload size"))
            .with_authority(Some(MemoryAuthority::Constraint)),
        ),
        record(
            NewMemory::new(MemoryKind::Decision, "quokka retries three times")
                .with_subject(Some("quokka retry policy"))
                .with_authority(Some(MemoryAuthority::Decision))
                .with_provenance(DecisionProvenance {
                    rationale: Some("three covers a transient upstream outage".into()),
                    assumptions: Some("the upstream recovers inside a minute".into()),
                    ..DecisionProvenance::default()
                }),
        ),
        record(
            NewMemory::new(MemoryKind::Decision, "quokka batches on write")
                .with_subject(Some("quokka batching"))
                .with_authority(Some(thin_decision_authority)),
        ),
        record(
            NewMemory::new(MemoryKind::Finding, "quokka could maybe use a cache")
                .with_subject(Some("quokka caching idea"))
                .with_authority(Some(MemoryAuthority::Idea)),
        ),
    ]
}

/// The relevance survives the call.
///
/// Before this change `MemoryStore::search` mapped its scored pairs back to
/// bare records and `RetrievalResult` had no relevance at all, so this test
/// does not compile against that tree — the strongest possible form of
/// "fails on `main`".
#[test]
fn a_retrieval_carries_a_relevance_for_every_memory_it_returned() {
    let fixture = Fixture::new();
    let project = fixture.open();
    let store = project.store();
    seed_ranking_corpus(&store);

    let grouped = store
        .search_grouped("quokka", SearchScope::Current, 10)
        .unwrap();

    let returned: Vec<&glasshouse::memory::MemoryRecord> = grouped
        .invariants_and_constraints
        .iter()
        .chain(grouped.other.iter())
        .collect();
    assert_eq!(
        returned.len(),
        6,
        "the corpus is six matching memories; the ranking tests below depend on all five \
         coming back"
    );

    for record in returned {
        let relevance = grouped.relevance(&record.id);
        assert!(
            relevance.is_some(),
            "every memory a retrieval returned must carry the relevance it earned, but \
             {:?} carried none",
            record.subject
        );
        assert!(
            relevance.unwrap().is_finite(),
            "a relevance must be a real number, not a NaN or an infinity: {:?} scored {:?}",
            record.subject,
            relevance
        );
    }
}

/// The relevance is the *query's* answer, not a placeholder.
///
/// A number that is the same whatever was asked is not a retrieval signal,
/// and would satisfy every other test here. So: one memory, two queries, and
/// the two scores must differ.
///
/// The corpus is built so the two terms cannot have the same statistics —
/// `quokka` appears in five memories and `wombat` in one — which is what BM25
/// scores on: term frequency, document frequency and document length.
#[test]
fn the_same_memory_scores_differently_for_two_different_queries() {
    let fixture = Fixture::new();
    let project = fixture.open();
    let store = project.store();

    let both = store
        .record(
            NewMemory::new(
                MemoryKind::Finding,
                "the quokka and the wombat share one burrow",
            )
            .with_subject(Some("quokka and wombat cohabitation")),
        )
        .unwrap();
    for body in [
        "quokka feeding happens at dawn",
        "quokka tagging uses a numbered collar",
        "quokka relocation needs a permit",
        "quokka photography is discouraged",
    ] {
        store
            .record(NewMemory::new(MemoryKind::Finding, body))
            .unwrap();
    }

    let common = store
        .search_grouped("quokka", SearchScope::Current, 10)
        .unwrap()
        .relevance(&both.id)
        .expect("the memory matches `quokka` and must carry a relevance for that query");
    let rare = store
        .search_grouped("wombat", SearchScope::Current, 10)
        .unwrap()
        .relevance(&both.id)
        .expect("the memory matches `wombat` and must carry a relevance for that query");

    assert_ne!(
        common, rare,
        "one memory scored {common} against a term in five memories and {rare} against a term \
         in one; a relevance that does not move with the query is not a retrieval signal, it \
         is a constant with a good name"
    );
}

/// The regression that matters most: nothing about which memories come back,
/// or in what order, changed when the relevance stopped being discarded.
///
/// The two expected orders below are **literals captured from the tree before
/// this change** by running the same fixture through `search` and
/// `search_grouped`. They are deliberately not computed from `ladder_rung`,
/// `retrieval_weight` or the comparator: a test that derives its expectation
/// from the thing under test cannot detect that thing changing (practice
/// §80, case 6).
#[test]
fn attaching_the_relevance_changed_neither_which_memories_come_back_nor_their_order() {
    let fixture = Fixture::new();
    let project = fixture.open();
    let store = project.store();
    seed_ranking_corpus(&store);

    let flat: Vec<String> = store
        .search("quokka", SearchScope::Current, 10)
        .unwrap()
        .into_iter()
        .map(|record| record.subject.unwrap())
        .collect();
    assert_eq!(
        flat,
        vec![
            "quokka credential handling".to_string(),
            "quokka retry policy".to_string(),
            "quokka timeout".to_string(),
            "quokka payload size".to_string(),
            "quokka batching".to_string(),
            "quokka caching idea".to_string(),
        ],
        "`search` must return exactly the order it returned before the relevance was carried \
         out of it: the invariant first by rung, the four middle-rung memories in their \
         blended relevance/decay order with the thin decision demoted behind the well-proven \
         one, and the idea last. Both constraints are in there so that reversing the \
         within-rung comparison has something to move that the demotion does not put back"
    );

    let grouped = store
        .search_grouped("quokka", SearchScope::Current, 10)
        .unwrap();
    let rules: Vec<String> = grouped
        .invariants_and_constraints
        .iter()
        .map(|record| record.subject.clone().unwrap())
        .collect();
    let other: Vec<String> = grouped
        .other
        .iter()
        .map(|record| record.subject.clone().unwrap())
        .collect();
    assert_eq!(
        rules,
        vec![
            "quokka credential handling".to_string(),
            "quokka timeout".to_string(),
            "quokka payload size".to_string(),
        ],
        "the rules group must keep its pre-change membership and order"
    );
    assert_eq!(
        other,
        vec![
            "quokka retry policy".to_string(),
            "quokka batching".to_string(),
            "quokka caching idea".to_string(),
        ],
        "the other group must keep its pre-change membership and order"
    );
}

/// A search that matched nothing reports no relevance, rather than a
/// fabricated zero.
///
/// Both halves are asserted, because they fail differently: an empty result
/// that still answered `Some(0.0)` would hand a future caller a number that
/// reads as "matched as badly as possible" for a memory the query never
/// looked at, and zero is not even a bad BM25 score — the scale is negative
/// and unbounded, so `0.0` would rank as the *best* match on this corpus.
#[test]
fn a_retrieval_that_matched_nothing_reports_no_relevance_rather_than_zero() {
    let fixture = Fixture::new();
    let project = fixture.open();
    let store = project.store();
    let ids = seed_ranking_corpus(&store);

    let nothing = store
        .search_grouped("bandicoot", SearchScope::Current, 10)
        .unwrap();
    assert!(
        nothing.invariants_and_constraints.is_empty() && nothing.other.is_empty(),
        "no memory in the corpus contains `bandicoot`"
    );
    for id in &ids {
        assert_eq!(
            nothing.relevance(id),
            None,
            "a retrieval that returned nothing must report no relevance for any memory, not a \
             zero for one it never scored"
        );
    }

    // A query that sanitizes away entirely never reaches SQLite at all, and
    // must answer the same way rather than through a different path.
    let unsearchable = store
        .search_grouped("***", SearchScope::Current, 10)
        .unwrap();
    assert!(unsearchable.invariants_and_constraints.is_empty() && unsearchable.other.is_empty());
    for id in &ids {
        assert_eq!(unsearchable.relevance(id), None);
    }

    // And a retrieval that *did* match something reports nothing for the
    // memories it did not return: absence is per-memory, not per-query.
    let unrelated = store
        .record(NewMemory::new(
            MemoryKind::Finding,
            "the bilby is nocturnal and unrelated",
        ))
        .unwrap();
    let matched = store
        .search_grouped("quokka", SearchScope::Current, 10)
        .unwrap();
    assert!(
        matched.relevance(&ids[0]).is_some(),
        "the invariant matched `quokka` and must carry a relevance"
    );
    assert_eq!(
        matched.relevance(&unrelated.id),
        None,
        "a memory the query never matched has no relevance to report, and inventing one would \
         be exactly the fabricated number this signal must not become"
    );
}

/// Every returned memory's relevance, keyed by its subject so that two stores
/// holding the same text under different identifiers can be compared.
fn relevance_by_subject(grouped: &RetrievalResult) -> std::collections::BTreeMap<String, f64> {
    grouped
        .invariants_and_constraints
        .iter()
        .chain(grouped.other.iter())
        .map(|record| {
            let subject = record
                .subject
                .clone()
                .expect("every fixture memory has a subject");
            let relevance = grouped
                .relevance(&record.id)
                .expect("every returned memory carries a relevance");
            (subject, relevance)
        })
        .collect()
}

/// A relevance belongs to *its own* record, and survives being permuted.
///
/// # Why this test exists
///
/// `demote_thin_decisions` reorders results after the sort, and it does so by
/// writing clones into slots. A permutation that moved the records but left
/// each slot's relevance where it was would leave every demoted decision
/// holding the score of whichever memory used to sit where it landed — a
/// number that is real, in range, and about a different memory. A deliberate
/// mutation doing exactly that **survived the other four tests in this file**:
/// each of them checks that a relevance is present, that it moves with the
/// query, that the order is unchanged, or that it is absent when nothing
/// matched, and swapping two scores between two records breaks none of those.
///
/// # How it is measured without recomputing BM25
///
/// The test does not predict a score. It seeds the same text twice and varies
/// only the thin decision's authority class, which `demote_thin_decisions`
/// keys on and `memories_fts` does not index — so the permutation runs in one
/// store and not the other while every BM25 input stays identical. Any
/// difference between the two is the permutation moving a number it should
/// not have touched.
#[test]
fn a_relevance_stays_with_its_own_record_when_the_demotion_permutes() {
    let permuted_fixture = Fixture::new();
    let permuted_project = permuted_fixture.open();
    let permuted_store = permuted_project.store();
    seed_ranking_corpus_with(&permuted_store, MemoryAuthority::Decision);

    let undisturbed_fixture = Fixture::new();
    let undisturbed_project = undisturbed_fixture.open();
    let undisturbed_store = undisturbed_project.store();
    seed_ranking_corpus_with(&undisturbed_store, MemoryAuthority::Preference);

    let permuted = relevance_by_subject(
        &permuted_store
            .search_grouped("quokka", SearchScope::Current, 10)
            .unwrap(),
    );
    let undisturbed = relevance_by_subject(
        &undisturbed_store
            .search_grouped("quokka", SearchScope::Current, 10)
            .unwrap(),
    );

    // Non-vacuity: if the two decisions scored the same, exchanging their
    // relevances would be undetectable and everything below would pass on a
    // tree that had exchanged them.
    assert_ne!(
        permuted["quokka retry policy"], permuted["quokka batching"],
        "the two decisions must score differently or this test cannot see them swapped"
    );

    for subject in ["quokka retry policy", "quokka batching"] {
        assert_eq!(
            permuted[subject], undisturbed[subject],
            "{subject} must carry its own BM25 relevance whether or not the thin-decision \
             demotion permuted it; the same text scored {} where the demotion ran and {} where \
             it did not, which means the permutation moved a score off its record",
            permuted[subject], undisturbed[subject]
        );
    }
}

/// The number runs in the direction the module documents.
///
/// `memory/search.rs`'s own header states that SQLite's `bm25()` returns *a
/// more negative number for a better match*, and the whole ranking rests on
/// it. Once the number is exposed, that claim stops being an implementation
/// detail: the first consumer to compare two relevances gets the comparison
/// backwards if it is wrong. A mutation negating the relevance on its way out
/// of `search_matching` **survived every other test in this file** — presence,
/// variation, ordering, absence and pairing are all sign-blind.
///
/// The fixture makes the ladder and the decay weight constant — same kind, no
/// authority, all recorded together — so the order `search` returns is decided
/// by relevance alone. The first result is therefore the better match by
/// construction, and its relevance must be the more negative of the two.
#[test]
fn the_relevance_runs_in_the_direction_the_ordering_does() {
    let fixture = Fixture::new();
    let project = fixture.open();
    let store = project.store();

    for body in [
        "wallaby wallaby wallaby: the wallaby migration note",
        "a single mention of wallaby inside a much longer note about seasonal rainfall, \
         grazing pressure, fence lines and the several other things that decide where a \
         herd goes in a dry year",
    ] {
        store
            .record(NewMemory::new(MemoryKind::Finding, body))
            .unwrap();
    }

    let grouped = store
        .search_grouped("wallaby", SearchScope::Current, 10)
        .unwrap();
    assert_eq!(
        grouped.other.len(),
        2,
        "both notes mention wallaby and neither is a rule, so both land in `other`"
    );
    assert!(
        grouped.invariants_and_constraints.is_empty(),
        "no memory here is an invariant or a constraint"
    );

    let best = grouped.relevance(&grouped.other[0].id).unwrap();
    let worst = grouped.relevance(&grouped.other[1].id).unwrap();

    assert!(
        best < worst,
        "`search` returns the better match first, and a better BM25 match is a *more \
         negative* number, so the first result must carry the lower relevance — got {best} \
         first and {worst} second. If these are the right way round in value but the wrong \
         way round in sign, every consumer that compares two relevances will prefer the \
         worse match."
    );
}
