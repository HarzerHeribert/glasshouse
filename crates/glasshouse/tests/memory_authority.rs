//! Phase 21A — memory authority classes: distinct, retrieval-preserving, and
//! only ever raised by explicit review.
//!
//! An integration test on purpose, following `memory_store.rs`'s `Fixture`
//! shape: every path here goes through `glasshouse::bootstrap`, so the
//! migration, the project binding and the triggers are all in play.

use std::path::Path;
use std::sync::{Arc, Mutex};

use glasshouse::memory::extract::authority::{self, EXTRACTOR_CEILING, Lowering};
use glasshouse::memory::extract::schema::{Confidence, Disposition};
use glasshouse::memory::search::SearchScope;
use glasshouse::memory::snapshot::{self, SnapshotBudget};
use glasshouse::memory::{
    AuthorityChange, Classifier, MemoryAuthority, MemoryId, MemoryKind, MemoryStatus,
    MemoryStoreError, NewMemory, ProjectMemory,
};
use glasshouse::{Cli, Runtime};

use clap::Parser;

/// A bootstrapped project inside `base`, sharing `base`'s data and config
/// roots. Two fixtures over one `base` are two real projects on one machine.
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

    /// A memory store whose clock is under the test's control, so timestamps
    /// can be asserted exactly instead of slept for.
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
// A. All seven classes are real, and stay distinct.
// -------------------------------------------------------------------------

/// Map line: "Support the authority class invariant / constraint / decision /
/// preference / hypothesis / idea / historical (seven separate lines)."
///
/// Driven from `MemoryAuthority::ALL` so an eighth class added without a
/// migration fails here rather than passing unnoticed.
#[test]
fn every_authority_class_round_trips_through_sqlite_unchanged() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    assert_eq!(
        MemoryAuthority::ALL.len(),
        7,
        "Phase 21A names exactly seven authority classes"
    );

    for authority in MemoryAuthority::ALL {
        let recorded = store
            .record(
                NewMemory::new(
                    MemoryKind::Finding,
                    format!("held at {authority} authority"),
                )
                .with_authority(Some(*authority)),
            )
            .unwrap();
        assert_eq!(recorded.authority, Some(*authority));

        let read_back = store.get(&recorded.id).unwrap().unwrap();
        assert_eq!(
            read_back.authority,
            Some(*authority),
            "{authority} did not survive the round trip"
        );
    }
}

/// Map line: classify by authority rather than flattening. `None` — nobody
/// has judged this — is a distinct fact from every one of the seven classes,
/// and must round-trip as `None`, not as some default class.
#[test]
fn unclassified_round_trips_as_none_and_is_not_any_class() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let unclassified = store
        .record(NewMemory::new(
            MemoryKind::Finding,
            "nobody has classified how binding this is yet",
        ))
        .unwrap();
    assert_eq!(unclassified.authority, None);

    let read_back = store.get(&unclassified.id).unwrap().unwrap();
    assert_eq!(read_back.authority, None);
    for authority in MemoryAuthority::ALL {
        assert_ne!(read_back.authority, Some(*authority));
    }
}

/// `is_binding()` is true for exactly invariant, constraint, decision.
/// Driven from `ALL` so the test is exhaustive.
#[test]
fn is_binding_is_true_for_exactly_invariant_constraint_and_decision() {
    for authority in MemoryAuthority::ALL {
        let expected = matches!(
            authority,
            MemoryAuthority::Invariant | MemoryAuthority::Constraint | MemoryAuthority::Decision
        );
        assert_eq!(
            authority.is_binding(),
            expected,
            "{authority} disagrees about is_binding()"
        );
    }
}

/// Map line: the seven-class list and the six-kind list are independent
/// axes. A `finding` can be an `invariant`, and a `decision` can be
/// `historical` — the schema's own comment says both must be representable.
#[test]
fn authority_and_kind_are_independent_axes() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let finding_invariant = store
        .record(
            NewMemory::new(MemoryKind::Finding, "a finding that is also an invariant")
                .with_authority(Some(MemoryAuthority::Invariant)),
        )
        .unwrap();
    let read_back = store.get(&finding_invariant.id).unwrap().unwrap();
    assert_eq!(read_back.kind, MemoryKind::Finding);
    assert_eq!(read_back.authority, Some(MemoryAuthority::Invariant));

    let decision_historical = store
        .record(
            NewMemory::new(
                MemoryKind::Decision,
                "a decision that has decayed to historical",
            )
            .with_authority(Some(MemoryAuthority::Historical)),
        )
        .unwrap();
    let read_back = store.get(&decision_historical.id).unwrap().unwrap();
    assert_eq!(read_back.kind, MemoryKind::Decision);
    assert_eq!(read_back.authority, Some(MemoryAuthority::Historical));
}

// -------------------------------------------------------------------------
// B. Retrieval preserves the class — the fixed architectural requirement.
// -------------------------------------------------------------------------

/// Map line: "Retrieval and injection must preserve those distinctions
/// instead of flattening all memories into equally authoritative text."
/// `store.search(...)` must carry `authority` intact.
#[test]
fn search_preserves_authority_for_classified_and_unclassified_memories() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let classified = store
        .record(
            NewMemory::new(
                MemoryKind::Constraint,
                "gremlinfrotz search must not flatten this authority",
            )
            .with_authority(Some(MemoryAuthority::Constraint)),
        )
        .unwrap();
    let unclassified = store
        .record(NewMemory::new(
            MemoryKind::Finding,
            "gremlinfrotz search must not invent an authority for this one either",
        ))
        .unwrap();

    let results = store
        .search("gremlinfrotz", SearchScope::Current, 10)
        .unwrap();

    let found_classified = results
        .iter()
        .find(|record| record.id == classified.id)
        .expect("the classified memory must be found");
    assert_eq!(
        found_classified.authority,
        Some(MemoryAuthority::Constraint)
    );

    let found_unclassified = results
        .iter()
        .find(|record| record.id == unclassified.id)
        .expect("the unclassified memory must be found");
    assert_eq!(found_unclassified.authority, None);
}

/// `snapshot::snapshot(...)` carries `authority` on every entry, and `None`
/// stays `None` rather than being rendered as some class.
#[test]
fn snapshot_carries_authority_on_every_entry() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let classified = store
        .record(
            NewMemory::new(
                MemoryKind::Decision,
                "a decision the snapshot must classify",
            )
            .with_authority(Some(MemoryAuthority::Decision)),
        )
        .unwrap();
    let unclassified = store
        .record(NewMemory::new(
            MemoryKind::Decision,
            "a decision the snapshot must leave unclassified",
        ))
        .unwrap();

    let snap = snapshot::snapshot(&store, &SnapshotBudget::default()).unwrap();
    let section = snap.section(MemoryKind::Decision).unwrap();

    let found_classified = section
        .entries
        .iter()
        .find(|entry| entry.id == classified.id)
        .expect("the classified decision must appear in the snapshot");
    assert_eq!(found_classified.authority, Some(MemoryAuthority::Decision));

    let found_unclassified = section
        .entries
        .iter()
        .find(|entry| entry.id == unclassified.id)
        .expect("the unclassified decision must appear in the snapshot");
    assert_eq!(found_unclassified.authority, None);
}

/// A snapshot whose budget truncates a body still reports the authority
/// correctly — the budget must not be able to flatten the distinction.
#[test]
fn a_truncated_snapshot_body_still_reports_its_authority_correctly() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let long_body = "a".repeat(500);
    let recorded = store
        .record(
            NewMemory::new(MemoryKind::Finding, long_body.clone())
                .with_authority(Some(MemoryAuthority::Invariant)),
        )
        .unwrap();

    let budget = SnapshotBudget::new(10, 50);
    let snap = snapshot::snapshot(&store, &budget).unwrap();
    let section = snap.section(MemoryKind::Finding).unwrap();
    let entry = section
        .entries
        .iter()
        .find(|entry| entry.id == recorded.id)
        .unwrap();

    assert!(entry.body_truncated, "the body must have been truncated");
    assert_eq!(entry.body.chars().count(), 50);
    assert_eq!(
        entry.authority,
        Some(MemoryAuthority::Invariant),
        "truncating the body must not touch the authority"
    );
}

/// `store.with_status(...)` preserves authority.
#[test]
fn with_status_preserves_authority() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let recorded = store
        .record(
            NewMemory::new(MemoryKind::Todo, "with_status must not flatten this")
                .with_authority(Some(MemoryAuthority::Preference)),
        )
        .unwrap();

    let listed = store.with_status(MemoryStatus::Active, 50).unwrap();
    let found = listed
        .iter()
        .find(|record| record.id == recorded.id)
        .unwrap();
    assert_eq!(found.authority, Some(MemoryAuthority::Preference));
}

/// `store.binding(limit)` returns only memories whose class `is_binding()`,
/// only `Active` ones, and never an unclassified one. `None` means unjudged,
/// and the conservative reading of unjudged is "not a rule".
#[test]
fn binding_returns_only_active_binding_classified_memories() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let mut expected_ids = Vec::new();
    for authority in MemoryAuthority::ALL {
        let recorded = store
            .record(
                NewMemory::new(MemoryKind::Finding, format!("binding test at {authority}"))
                    .with_authority(Some(*authority)),
            )
            .unwrap();
        if authority.is_binding() {
            expected_ids.push(recorded.id);
        }
    }

    // An unclassified memory: must never appear, even though `None` is
    // treated as high-impact elsewhere in the module — that is a different
    // concern (conflict review) from "may this be presented as a rule".
    let unclassified = store
        .record(NewMemory::new(
            MemoryKind::Finding,
            "nobody has classified how binding this is",
        ))
        .unwrap();

    // A binding memory that is not active must not appear either.
    let superseded = store
        .record(
            NewMemory::new(MemoryKind::Finding, "a constraint that got superseded")
                .with_authority(Some(MemoryAuthority::Constraint)),
        )
        .unwrap();
    store
        .set_status(&superseded.id, MemoryStatus::Superseded)
        .unwrap();

    let bound = store.binding(50).unwrap();
    let bound_ids: Vec<MemoryId> = bound.iter().map(|record| record.id.clone()).collect();

    for id in &expected_ids {
        assert!(
            bound_ids.contains(id),
            "a binding, active, classified memory must appear in binding()"
        );
    }
    assert!(
        !bound_ids.contains(&unclassified.id),
        "None means unjudged, and unjudged must not be presented as a rule"
    );
    assert!(
        !bound_ids.contains(&superseded.id),
        "a non-active memory must not be presented as a current rule"
    );
    for record in &bound {
        assert!(
            record.authority.is_some_and(MemoryAuthority::is_binding),
            "every entry returned by binding() must itself be binding"
        );
        assert_eq!(record.status, MemoryStatus::Active);
    }
}

/// `binding`'s `limit` is honoured.
#[test]
fn binding_honours_its_limit() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    for i in 0..5 {
        store
            .record(
                NewMemory::new(MemoryKind::Constraint, format!("binding limit test {i}"))
                    .with_authority(Some(MemoryAuthority::Constraint)),
            )
            .unwrap();
    }

    let limited = store.binding(2).unwrap();
    assert_eq!(limited.len(), 2);

    let unlimited = store.binding(50).unwrap();
    assert_eq!(unlimited.len(), 5);
}

// -------------------------------------------------------------------------
// C. Automatic classification can only lower — `authority::conservative`.
// -------------------------------------------------------------------------

/// A declared `invariant` with `certain` confidence and `accepted`
/// disposition is stored as `constraint`, and `Classification::reasons`
/// names `Lowering::AutomaticExtraction`. `EXTRACTOR_CEILING` is asserted to
/// be `constraint` too, so the constant and the behaviour cannot drift.
#[test]
fn a_declared_invariant_is_lowered_to_the_extractor_ceiling() {
    assert_eq!(EXTRACTOR_CEILING, MemoryAuthority::Constraint);

    let result = authority::conservative(
        MemoryAuthority::Invariant,
        Confidence::Certain,
        Disposition::Accepted,
    );
    assert_eq!(result.stored, MemoryAuthority::Constraint);
    assert_eq!(result.reasons, vec![Lowering::AutomaticExtraction]);
}

/// There is no `(declared, confidence, disposition)` triple that yields
/// `invariant`. Loop over the full input space — 7 x 3 x 3 = 63 cases.
#[test]
fn no_input_triple_yields_an_invariant() {
    let mut cases = 0;
    for declared in MemoryAuthority::ALL {
        for confidence in Confidence::ALL {
            for disposition in Disposition::ALL {
                cases += 1;
                let result = authority::conservative(*declared, *confidence, *disposition);
                assert_ne!(
                    result.stored,
                    MemoryAuthority::Invariant,
                    "{declared} + {confidence} + {disposition} minted an invariant"
                );
            }
        }
    }
    assert_eq!(cases, 63, "the input space is 7 x 3 x 3 = 63 cases");
}

/// Map line: "distinguish an accepted decision from an idea that was merely
/// discussed enthusiastically." `disposition: proposed` caps at `idea`,
/// whatever the declared class and however confident the model claims to be.
#[test]
fn a_proposed_disposition_caps_at_idea_regardless_of_declared_class_or_confidence() {
    // Never binding, and never anything stronger than `idea` — a declared
    // class weaker than `idea` (only `historical`) is left alone, but nothing
    // is ever raised past the cap.
    for declared in MemoryAuthority::ALL {
        for confidence in Confidence::ALL {
            let result = authority::conservative(*declared, *confidence, Disposition::Proposed);
            assert!(
                matches!(
                    result.stored,
                    MemoryAuthority::Idea | MemoryAuthority::Historical
                ),
                "{declared} + {confidence} + proposed must cap at idea, got {}",
                result.stored
            );
            assert!(!result.stored.is_binding());
        }
    }

    // The named case: however confident and whatever was declared (as long
    // as it declared something stronger than idea), a proposal lands exactly
    // on `idea` — enthusiasm is not acceptance.
    for declared in [
        MemoryAuthority::Invariant,
        MemoryAuthority::Constraint,
        MemoryAuthority::Decision,
        MemoryAuthority::Preference,
        MemoryAuthority::Hypothesis,
    ] {
        for confidence in Confidence::ALL {
            let result = authority::conservative(declared, *confidence, Disposition::Proposed);
            assert_eq!(
                result.stored,
                MemoryAuthority::Idea,
                "{declared} + {confidence} + proposed must land exactly on idea, got {}",
                result.stored
            );
        }
    }
}

/// `disposition: abandoned` caps at `historical`, and the result is not
/// binding.
#[test]
fn an_abandoned_disposition_caps_at_historical_and_is_never_binding() {
    for declared in MemoryAuthority::ALL {
        for confidence in Confidence::ALL {
            let result = authority::conservative(*declared, *confidence, Disposition::Abandoned);
            assert_eq!(result.stored, MemoryAuthority::Historical);
            assert!(!result.stored.is_binding());
        }
    }
}

/// `confidence: unsure` caps at `hypothesis`; `probable` caps at `decision`.
#[test]
fn unsure_caps_at_hypothesis_and_probable_caps_at_decision() {
    for declared in MemoryAuthority::ALL {
        let unsure = authority::conservative(*declared, Confidence::Unsure, Disposition::Accepted);
        assert!(
            matches!(
                unsure.stored,
                MemoryAuthority::Hypothesis | MemoryAuthority::Idea | MemoryAuthority::Historical
            ),
            "{declared} + unsure + accepted must be no stronger than hypothesis, got {}",
            unsure.stored
        );

        let probable =
            authority::conservative(*declared, Confidence::Probable, Disposition::Accepted);
        assert!(
            matches!(
                probable.stored,
                MemoryAuthority::Decision
                    | MemoryAuthority::Preference
                    | MemoryAuthority::Hypothesis
                    | MemoryAuthority::Idea
                    | MemoryAuthority::Historical
            ),
            "{declared} + probable + accepted must be no stronger than decision, got {}",
            probable.stored
        );
    }

    // The named cases the map cares about, isolated from any other ceiling.
    let unsure = authority::conservative(
        MemoryAuthority::Constraint,
        Confidence::Unsure,
        Disposition::Accepted,
    );
    assert_eq!(unsure.stored, MemoryAuthority::Hypothesis);

    let probable = authority::conservative(
        MemoryAuthority::Constraint,
        Confidence::Probable,
        Disposition::Accepted,
    );
    assert_eq!(probable.stored, MemoryAuthority::Decision);
}

/// A weak declaration is never raised: a declared `idea` with `certain`
/// confidence stays an `idea`, and `Classification::was_lowered()` is false.
#[test]
fn a_weak_declaration_is_never_raised() {
    let result = authority::conservative(
        MemoryAuthority::Idea,
        Confidence::Certain,
        Disposition::Accepted,
    );
    assert_eq!(result.stored, MemoryAuthority::Idea);
    assert!(!result.was_lowered());
    assert!(result.reasons.is_empty());
}

/// The strongest thing extraction can produce is a `constraint`, and it does
/// produce it — the policy is conservative, not inert. A test suite that
/// only proves things get weaker would pass against a function that always
/// returned `historical`.
#[test]
fn a_certain_accepted_constraint_is_stored_as_a_constraint() {
    let result = authority::conservative(
        MemoryAuthority::Constraint,
        Confidence::Certain,
        Disposition::Accepted,
    );
    assert_eq!(result.stored, MemoryAuthority::Constraint);
    assert!(result.stored.is_binding());
    assert!(!result.was_lowered());
    assert!(result.reasons.is_empty());
}

// -------------------------------------------------------------------------
// D. Explicit promotion and demotion — `set_authority`.
// -------------------------------------------------------------------------

/// `Classifier::Reviewed` may set any class, `invariant` included, and the
/// change is visible on the returned record and on a fresh `get`.
#[test]
fn a_reviewer_may_set_any_class_including_invariant() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let recorded = store
        .record(NewMemory::new(
            MemoryKind::Constraint,
            "a memory a reviewer will promote",
        ))
        .unwrap();

    let (updated, change) = store
        .set_authority(
            &recorded.id,
            Some(MemoryAuthority::Invariant),
            Classifier::Reviewed,
        )
        .unwrap();
    assert_eq!(updated.authority, Some(MemoryAuthority::Invariant));
    assert_eq!(change, AuthorityChange::Changed);

    let read_back = store.get(&recorded.id).unwrap().unwrap();
    assert_eq!(read_back.authority, Some(MemoryAuthority::Invariant));
}

/// `Classifier::Extractor` setting `invariant` is refused with
/// `MemoryStoreError::ReviewRequired`, and the stored class is unchanged
/// afterwards.
#[test]
fn an_extractor_may_not_mint_an_invariant_and_nothing_is_written() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let recorded = store
        .record(
            NewMemory::new(MemoryKind::Constraint, "an extractor must not mint this")
                .with_authority(Some(MemoryAuthority::Constraint)),
        )
        .unwrap();

    let error = store
        .set_authority(
            &recorded.id,
            Some(MemoryAuthority::Invariant),
            Classifier::Extractor,
        )
        .expect_err("an extractor may not set invariant");
    assert!(
        matches!(error, MemoryStoreError::ReviewRequired { .. }),
        "unexpected error: {error}"
    );

    let untouched = store.get(&recorded.id).unwrap().unwrap();
    assert_eq!(
        untouched.authority,
        Some(MemoryAuthority::Constraint),
        "a refused promotion must leave the stored class unchanged"
    );
}

/// `Classifier::Extractor` may set any of the other six, and may set `None`.
#[test]
fn an_extractor_may_set_any_non_invariant_class_or_clear_it() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    for authority in MemoryAuthority::ALL {
        if *authority == MemoryAuthority::Invariant {
            continue;
        }
        let recorded = store
            .record(NewMemory::new(
                MemoryKind::Finding,
                format!("extractor sets {authority}"),
            ))
            .unwrap();
        let (updated, _) = store
            .set_authority(&recorded.id, Some(*authority), Classifier::Extractor)
            .unwrap_or_else(|error| panic!("extractor could not set {authority}: {error}"));
        assert_eq!(updated.authority, Some(*authority));
    }

    let classified = store
        .record(
            NewMemory::new(MemoryKind::Finding, "extractor clears this")
                .with_authority(Some(MemoryAuthority::Decision)),
        )
        .unwrap();
    let (cleared, _) = store
        .set_authority(&classified.id, None, Classifier::Extractor)
        .unwrap();
    assert_eq!(cleared.authority, None);
}

/// Demotion is never refused, from any class, by either classifier.
#[test]
fn demotion_is_never_refused_from_any_class_by_either_classifier() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    for classifier in [Classifier::Extractor, Classifier::Reviewed] {
        for authority in MemoryAuthority::ALL {
            let recorded = store
                .record(
                    NewMemory::new(
                        MemoryKind::Finding,
                        format!("{authority} demoted by {classifier}"),
                    )
                    .with_authority(Some(*authority)),
                )
                .unwrap();
            store
                .set_authority(&recorded.id, Some(MemoryAuthority::Historical), classifier)
                .unwrap_or_else(|error| {
                    panic!("{classifier} could not demote {authority}: {error}")
                });
        }
    }
}

/// Setting the class a memory already has reports `AuthorityChange::Unchanged`;
/// setting a different one reports `Changed`.
#[test]
fn setting_the_same_class_reports_unchanged_and_a_different_one_reports_changed() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let recorded = store
        .record(
            NewMemory::new(MemoryKind::Finding, "unchanged vs changed")
                .with_authority(Some(MemoryAuthority::Preference)),
        )
        .unwrap();

    let (_, unchanged) = store
        .set_authority(
            &recorded.id,
            Some(MemoryAuthority::Preference),
            Classifier::Reviewed,
        )
        .unwrap();
    assert_eq!(unchanged, AuthorityChange::Unchanged);

    let (_, changed) = store
        .set_authority(
            &recorded.id,
            Some(MemoryAuthority::Decision),
            Classifier::Reviewed,
        )
        .unwrap();
    assert_eq!(changed, AuthorityChange::Changed);
}

/// `set_authority` on an id that does not exist is `MemoryStoreError::NotFound`.
#[test]
fn set_authority_on_a_missing_id_is_not_found() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let memory = fixture.memory();
    let store = memory.store();

    let invented = MemoryId::new("ffffffffffffffffffffffffffffffff");
    let error = store
        .set_authority(
            &invented,
            Some(MemoryAuthority::Decision),
            Classifier::Reviewed,
        )
        .expect_err("an id that does not exist must be refused");
    assert!(matches!(error, MemoryStoreError::NotFound { .. }));
}

/// A promotion updates `updated_at` (using the fixture's controlled clock,
/// never sleeping) and leaves `created_at` alone.
#[test]
fn a_promotion_updates_updated_at_and_leaves_created_at_alone() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let ticks = Arc::new(Mutex::new(1_000i64));
    let memory = fixture.memory_at(&ticks);
    let store = memory.store();

    let recorded = store
        .record(NewMemory::new(
            MemoryKind::Constraint,
            "a memory whose promotion time must be exact",
        ))
        .unwrap();
    assert_eq!(recorded.created_at, 1_000);
    assert_eq!(recorded.updated_at, 1_000);

    *ticks.lock().unwrap() = 5_000;
    let (promoted, _) = store
        .set_authority(
            &recorded.id,
            Some(MemoryAuthority::Invariant),
            Classifier::Reviewed,
        )
        .unwrap();
    assert_eq!(
        promoted.created_at, 1_000,
        "creation time is when it was learned and never moves"
    );
    assert_eq!(promoted.updated_at, 5_000);
}

// -------------------------------------------------------------------------
// E. Two projects, one machine.
// -------------------------------------------------------------------------

/// Two fixtures over one base are two real projects. A memory classified in
/// one is invisible to the other's `binding()` and `search()`, and
/// `set_authority` with the other project's id is refused.
#[test]
fn authority_is_isolated_between_two_projects_sharing_one_data_root() {
    let tmp = tempdir();
    let alpha = Fixture::new(tmp.path(), "alpha");
    let beta = Fixture::new(tmp.path(), "beta");

    let alpha_memory = alpha.memory();
    let alpha_store = alpha_memory.store();
    let alpha_record = alpha_store
        .record(
            NewMemory::new(
                MemoryKind::Constraint,
                "alpha's invariant must never leak into beta",
            )
            .with_authority(Some(MemoryAuthority::Invariant)),
        )
        .unwrap();

    let beta_memory = beta.memory();
    let beta_store = beta_memory.store();

    // Invisible to beta's binding().
    let beta_binding = beta_store.binding(50).unwrap();
    assert!(
        beta_binding
            .iter()
            .all(|record| record.id != alpha_record.id),
        "alpha's classified memory must not appear in beta's binding()"
    );

    // Invisible to beta's search().
    let beta_search = beta_store
        .search("invariant must never leak", SearchScope::Current, 10)
        .unwrap();
    assert!(
        beta_search
            .iter()
            .all(|record| record.id != alpha_record.id),
        "alpha's classified memory must not appear in beta's search()"
    );

    // `set_authority` with the other project's id is refused.
    let error = beta_store
        .set_authority(
            &alpha_record.id,
            Some(MemoryAuthority::Historical),
            Classifier::Reviewed,
        )
        .expect_err("beta must not be able to change alpha's memory's authority");
    assert!(
        matches!(
            error,
            MemoryStoreError::NotFound { .. } | MemoryStoreError::ForeignProject { .. }
        ),
        "unexpected error: {error}"
    );

    // Alpha still has it, unchanged.
    assert_eq!(
        alpha_store
            .get(&alpha_record.id)
            .unwrap()
            .unwrap()
            .authority,
        Some(MemoryAuthority::Invariant)
    );
}
