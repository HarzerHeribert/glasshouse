use super::*;

/// The Rust vocabulary and the constant beside the schema must agree.
///
/// `LIFECYCLE_EVENT_KINDS`' own doc comment says why this pin is the real
/// guarantee rather than the `CHECK`: a renamed variant otherwise compiles
/// perfectly and fails as a constraint violation somewhere nobody is
/// looking. Migration 15 has no `CHECK` at all, so this pin is the *only*
/// guarantee, which makes it load-bearing rather than belt-and-braces.
#[test]
fn every_kind_the_type_can_produce_is_one_the_schema_constant_declares() {
    let declared = [
        EvaluationKind::MemoryRetrieved,
        EvaluationKind::MemoryRetrievalMiss,
        EvaluationKind::DisposableRouteDecided,
        EvaluationKind::RoutingOverrideDecided,
        EvaluationKind::RoutingContinuationDecided,
        EvaluationKind::RoutingCostClassObserved,
        EvaluationKind::RoutingEvidenceObserved,
        EvaluationKind::RoutingOutcomeObserved,
        EvaluationKind::RoutingTierObserved,
        EvaluationKind::FailoverPrevented,
        EvaluationKind::MemoryRated,
        EvaluationKind::MemoryRevalidated,
        EvaluationKind::TurnOutcomeObserved,
        EvaluationKind::SessionRouteDecided,
        EvaluationKind::RoutingConsumptionEstimated,
        EvaluationKind::ReserveAvailabilityObserved,
    ];
    let names: Vec<&str> = declared.iter().map(|kind| kind.as_str()).collect();
    assert_eq!(
        names.as_slice(),
        EVALUATION_KINDS.as_slice(),
        "an evaluation kind was added or renamed without the constant \
         beside migration 15"
    );
    for name in EVALUATION_KINDS {
        assert!(
            EvaluationKind::from_stored(name).is_some(),
            "`{name}` is declared beside the schema and cannot be decoded"
        );
    }
}

#[test]
fn an_unrecognized_stored_value_decodes_to_nothing_rather_than_a_neighbour() {
    assert!(EvaluationKind::from_stored("route_preferred").is_none());
    assert!(EvaluationOutcome::from_stored("helped").is_none());
}

/// `glasshouse memory rate`'s vocabulary, spelled once — [`EvaluationKind::MemoryRated`]
/// and [`MEMORY_RATING_VERDICTS`]' eight words round-trip through
/// `as_str`/`from_stored`, and `Unknown` is not one of them: it is the
/// sentinel every other kind writes for "not yet known", never a verdict
/// a person types.
#[test]
fn memory_rated_and_its_verdict_vocabulary_round_trip() {
    assert_eq!(
        EvaluationKind::from_stored(EvaluationKind::MemoryRated.as_str()),
        Some(EvaluationKind::MemoryRated)
    );
    for verdict in MEMORY_RATING_VERDICTS {
        assert_eq!(
            EvaluationOutcome::from_stored(verdict.as_str()),
            Some(verdict),
            "`{}` must round-trip",
            verdict.as_str()
        );
        assert_ne!(verdict, EvaluationOutcome::Unknown);
    }
    assert_eq!(MEMORY_RATING_VERDICTS.len(), 8);
}

#[test]
fn the_shipped_retention_is_ninety_days_and_a_hundred_thousand_rows() {
    assert_eq!(Retention::DEFAULT.max_age_secs, 7_776_000);
    assert_eq!(Retention::DEFAULT.max_rows, 100_000);
    assert_eq!(Retention::DEFAULT.trim_every, 256);
    assert_eq!(Retention::default(), Retention::DEFAULT);
}

#[test]
fn the_history_flag_and_the_subject_vocabulary_are_the_same_distinction() {
    assert_eq!(
        RetrievalScope::from_history_flag(true),
        RetrievalScope::Historical
    );
    assert_eq!(
        RetrievalScope::from_history_flag(false),
        RetrievalScope::Current
    );
    assert_eq!(RetrievalScope::Historical.as_str(), "historical");
    assert_eq!(RetrievalScope::Current.as_str(), "current");
}

/// The briefing door's own scope, distinct from both search scopes so a
/// miss row names which door produced it.
#[test]
fn the_injection_scope_is_its_own_word_not_current() {
    assert_eq!(RetrievalScope::Injection.as_str(), "injection");
    assert_ne!(
        RetrievalScope::Injection.as_str(),
        RetrievalScope::Current.as_str()
    );
}
