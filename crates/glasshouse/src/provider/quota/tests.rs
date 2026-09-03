use super::*;
use crate::integrations::IntegrationId;

const OBSERVED: i64 = 1_756_000_000;

fn header(name: &str) -> ReadingSource {
    ReadingSource::ResponseHeader(name.to_owned())
}

fn measured(value: i64, unit: &str, source: ReadingSource) -> Capacity<NativeAmount> {
    Capacity::Measured(Reading::new(
        NativeAmount::whole(value, unit),
        OBSERVED,
        source,
    ))
}

fn measured_usd(micro: i64, source: ReadingSource) -> Capacity<NativeAmount> {
    Capacity::Measured(Reading::new(
        NativeAmount::millionths(micro, "USD"),
        OBSERVED,
        source,
    ))
}

// --- line 1198: the model exists, is provider-independent, and is what
// the quota shape is projected from ------------------------------------

/// The production launch path calls `ResourceKind::quota`. If that were
/// computed beside `CapacityState` instead of out of it, the two could
/// disagree and this module would be a type nothing consults — which is
/// exactly what practice §5 refuses. Deleting the delegation in
/// `ResourceKind::quota` fails this.
#[test]
fn every_resource_kinds_quota_shape_is_projected_out_of_its_capacity_state() {
    for kind in crate::provider::registry::registry() {
        assert_eq!(
            kind.quota(),
            kind.capacity().model(),
            "{} disagrees with its own capacity state",
            kind.label()
        );
    }
}

/// Provider-independence, asserted against the type rather than claimed:
/// two entirely different providers of the same shape produce byte-equal
/// capacity states, so nothing provider-specific leaked into one.
#[test]
fn two_different_remote_providers_produce_the_same_capacity_model() {
    let openrouter = ResourceKind::from_direct_provider("openrouter").capacity();
    let nvidia = ResourceKind::from_direct_provider("nvidia").capacity();
    assert_eq!(openrouter, nvidia);
}

// --- lines 1199-1204: the six shapes are representable, and each is
// distinguishable from the others rather than collapsing --------------

#[test]
fn a_token_limited_resource_is_representable() {
    let state = CapacityState::metered_balance()
        .limited_by(LimitingUnits::These(BTreeSet::from([LimitingUnit::Tokens])))
        .with_tokens(
            TokenBudget::uniform(Pool::unmeasured()).with_combined(
                Pool::unmeasured()
                    .with_limit(measured(
                        1_000_000,
                        "tokens",
                        header("x-ratelimit-limit-tokens"),
                    ))
                    .with_remaining(measured(
                        250_000,
                        "tokens",
                        header("x-ratelimit-remaining-tokens"),
                    )),
            ),
        );
    assert!(state.limiting_units().includes(LimitingUnit::Tokens));
    assert_eq!(
        state
            .tokens()
            .combined()
            .remaining()
            .value()
            .unwrap()
            .value(),
        250_000
    );
}

#[test]
fn a_request_limited_resource_is_representable() {
    let state = CapacityState::metered_balance()
        .limited_by(LimitingUnits::These(BTreeSet::from([
            LimitingUnit::Requests,
        ])))
        .with_requests(
            Pool::unmeasured()
                .with_limit(measured(50, "requests", header("x-ratelimit-limit")))
                .with_remaining(measured(3, "requests", header("x-ratelimit-remaining"))),
        );
    assert!(state.limiting_units().includes(LimitingUnit::Requests));
    assert_eq!(state.requests().remaining().value().unwrap().value(), 3);
    assert_eq!(
        state.requests().remaining().value().unwrap().unit(),
        "requests"
    );
}

#[test]
fn a_credit_limited_resource_is_representable_and_is_what_a_metered_provider_is() {
    let state = ResourceKind::from_direct_provider("openrouter").capacity();
    assert!(state.limiting_units().includes(LimitingUnit::Credits));
    assert_eq!(state.model(), QuotaModel::MeteredBalance);
    // The balance itself is a number nobody has read — not a zero, and
    // not an invented figure.
    assert!(!state.credits().remaining().is_measured());
    assert!(state.credits().remaining().is_readable());
}

/// Capability map line 1202, and the map's own rule that Glasshouse must
/// never invent exact token balances for opaque subscriptions. The
/// subscription's pools are not merely unread — they are unreadable, so
/// a telemetry pass has no state to transition them out of.
#[test]
fn an_opaque_subscription_is_representable_and_its_token_pools_can_never_be_read() {
    let state = ResourceKind::NativeSubscription {
        harness: IntegrationId::ClaudeCode,
    }
    .capacity();
    assert!(
        state
            .limiting_units()
            .includes(LimitingUnit::OpaqueProviderAllowance)
    );
    for (label, pool) in [
        ("combined", state.tokens().combined()),
        ("input", state.tokens().input()),
        ("output", state.tokens().output()),
        ("cached", state.tokens().cached_input()),
    ] {
        assert_eq!(*pool.remaining(), Capacity::ProviderOpaque, "{label}");
        assert!(!pool.remaining().is_readable(), "{label}");
    }
    // The reset time is a different question and is legitimately
    // readable: a harness prints when the window turns.
    assert!(state.windows().rolling().resets_at_unix().is_readable());
}

#[test]
fn a_user_defined_monetary_budget_for_a_metered_api_is_representable() {
    let state = ResourceKind::from_direct_provider("openrouter")
        .capacity()
        .with_user_budget(
            Pool::unmeasured()
                .with_limit(measured_usd(20_000_000, ReadingSource::UserConfiguration))
                .with_remaining(measured_usd(4_000_000, ReadingSource::UserConfiguration)),
        );
    let remaining = state.user_budget().remaining().value().unwrap();
    assert_eq!(remaining.value(), 4_000_000);
    assert_eq!(remaining.scale(), UnitScale::Millionths);
    assert_eq!(remaining.unit(), "USD");
    assert_eq!(
        state.user_budget().remaining().reading().unwrap().source(),
        &ReadingSource::UserConfiguration
    );
}

/// Capability map line 1204. Local inference is not "unmeasured remote
/// quota" and not "delegated": it is a third answer, and the two other
/// unlimited-looking resources answer differently.
#[test]
fn local_inference_is_unlimited_in_a_way_no_remote_resource_can_be() {
    let ollama = ResourceKind::from_direct_provider("ollama").capacity();
    assert_eq!(*ollama.limiting_units(), LimitingUnits::None);
    assert_eq!(ollama.locality(), Locality::Local);
    for (label, pool) in ollama.pools() {
        assert_eq!(*pool.remaining(), Capacity::Inapplicable, "{label}");
        assert!(!pool.remaining().is_readable(), "{label}");
    }

    let remote = ResourceKind::from_direct_provider("openrouter").capacity();
    assert_ne!(*remote.limiting_units(), LimitingUnits::None);
    let gateway = ResourceKind::GlasshouseGateway.capacity();
    assert_eq!(*gateway.limiting_units(), LimitingUnits::Delegated);
    assert_ne!(*gateway.limiting_units(), LimitingUnits::None);
}

/// `LimitingUnits::None` and `LimitingUnits::Delegated` must not be
/// readable as an empty list — a caller that iterated `named()` would
/// treat the gateway as unlimited.
#[test]
fn neither_none_nor_delegated_can_be_iterated_as_an_empty_set_of_units() {
    assert!(LimitingUnits::None.named().is_none());
    assert!(LimitingUnits::Delegated.named().is_none());
    assert!(!LimitingUnits::Delegated.includes(LimitingUnit::Credits));
}

// --- lines 1205-1209: independence, one pool at a time ----------------

#[test]
fn input_and_output_token_budgets_are_tracked_independently() {
    let tokens = TokenBudget::uniform(Pool::unmeasured())
        .with_combined(Pool::inapplicable())
        .with_input(Pool::unmeasured().with_remaining(measured(
            800,
            "input tokens",
            header("anthropic-ratelimit-input-tokens-remaining"),
        )))
        .with_output(Pool::unmeasured().with_remaining(measured(
            120,
            "output tokens",
            header("anthropic-ratelimit-output-tokens-remaining"),
        )));
    assert_eq!(tokens.input().remaining().value().unwrap().value(), 800);
    assert_eq!(tokens.output().remaining().value().unwrap().value(), 120);
    assert_ne!(tokens.input().remaining(), tokens.output().remaining());
    // A provider that exposes separate limits exposes no combined one,
    // and the model says so rather than duplicating a number.
    assert_eq!(*tokens.combined().remaining(), Capacity::Inapplicable);
}

#[test]
fn cached_input_usage_is_tracked_independently_of_input_tokens() {
    let tokens = TokenBudget::uniform(Pool::unmeasured())
        .with_input(Pool::unmeasured().with_remaining(measured(
            800,
            "input tokens",
            header("anthropic-ratelimit-input-tokens-remaining"),
        )))
        .with_cached_input(Pool::unmeasured().with_remaining(measured(
            5_000,
            "cached input tokens",
            header("anthropic-ratelimit-cache-read-input-tokens-remaining"),
        )));
    assert_eq!(
        tokens.cached_input().remaining().value().unwrap().value(),
        5_000
    );
    assert_ne!(
        tokens.cached_input().remaining(),
        tokens.input().remaining()
    );
}

#[test]
fn request_count_and_token_consumption_can_constrain_one_resource_at_once() {
    let state = CapacityState::metered_balance()
        .limited_by(LimitingUnits::These(BTreeSet::from([
            LimitingUnit::Requests,
            LimitingUnit::Tokens,
        ])))
        .with_requests(Pool::unmeasured().with_remaining(measured(
            2,
            "requests",
            header("x-rl-req"),
        )))
        .with_tokens(TokenBudget::uniform(Pool::unmeasured()).with_combined(
            Pool::unmeasured().with_remaining(measured(90_000, "tokens", header("x-rl-tok"))),
        ));
    assert!(state.limiting_units().includes(LimitingUnit::Requests));
    assert!(state.limiting_units().includes(LimitingUnit::Tokens));
    assert_eq!(state.requests().remaining().value().unwrap().value(), 2);
    assert_eq!(
        state
            .tokens()
            .combined()
            .remaining()
            .value()
            .unwrap()
            .value(),
        90_000
    );
}

#[test]
fn credits_are_tracked_independently_of_raw_tokens() {
    let state = CapacityState::metered_balance()
        .with_credits(Pool::unmeasured().with_remaining(measured_usd(
            1_250_000,
            ReadingSource::ProviderEndpoint("/api/v1/credits".to_owned()),
        )))
        .with_tokens(TokenBudget::uniform(Pool::unmeasured()).with_combined(
            Pool::unmeasured().with_remaining(measured(90_000, "tokens", header("x-rl-tok"))),
        ));
    let credits = state.credits().remaining().value().unwrap();
    let tokens = state.tokens().combined().remaining().value().unwrap();
    assert_eq!(credits.unit(), "USD");
    assert_eq!(tokens.unit(), "tokens");
    assert!(!credits.commensurable_with(tokens));
}

#[test]
fn a_user_budget_is_tracked_separately_from_the_provider_quota_it_binds_before() {
    let state = CapacityState::metered_balance()
        .with_credits(Pool::unmeasured().with_remaining(measured_usd(
            40_000_000,
            ReadingSource::ProviderEndpoint("/api/v1/credits".to_owned()),
        )))
        .with_user_budget(
            Pool::unmeasured()
                .with_remaining(measured_usd(2_000_000, ReadingSource::UserConfiguration)),
        );
    // Forty dollars of provider credit, two dollars of the user's own
    // ceiling. Neither number overwrote the other, and their sources
    // say which is whose.
    assert_eq!(
        state.credits().remaining().value().unwrap().value(),
        40_000_000
    );
    assert_eq!(
        state.user_budget().remaining().value().unwrap().value(),
        2_000_000
    );
    assert_ne!(
        state.credits().remaining().reading().unwrap().source(),
        state.user_budget().remaining().reading().unwrap().source()
    );
}

// --- lines 1210-1212: windows ----------------------------------------

#[test]
fn a_windows_start_and_reset_are_tracked_separately_and_either_may_be_unknown() {
    let window = WindowCapacity::uniform(
        WindowShape::Rolling,
        Pool::opaque(),
        Capacity::<i64>::Unmeasured,
    )
    .with_resets_at(Capacity::Measured(Reading::new(
        OBSERVED + 3_600,
        OBSERVED,
        ReadingSource::HarnessReport("session limit line".to_owned()),
    )));
    assert_eq!(*window.started_at_unix(), Capacity::Unmeasured);
    assert_eq!(*window.resets_at_unix().value().unwrap(), OBSERVED + 3_600);
}

#[test]
fn a_rolling_window_and_a_calendar_window_are_tracked_at_the_same_time() {
    let windows = Windows::uniform(Pool::unmeasured(), Capacity::<i64>::Unmeasured)
        .with_rolling(
            WindowCapacity::uniform(
                WindowShape::Rolling,
                Pool::unmeasured(),
                Capacity::<i64>::Unmeasured,
            )
            .with_resets_at(Capacity::Measured(Reading::new(
                OBSERVED + 300,
                OBSERVED,
                header("x-ratelimit-reset"),
            ))),
        )
        .with_calendar(
            WindowCapacity::uniform(
                WindowShape::Calendar,
                Pool::unmeasured(),
                Capacity::<i64>::Unmeasured,
            )
            .with_resets_at(Capacity::Measured(Reading::new(
                OBSERVED + 2_600_000,
                OBSERVED,
                ReadingSource::ProviderEndpoint("/billing".to_owned()),
            ))),
        );
    assert_eq!(windows.rolling().shape(), WindowShape::Rolling);
    assert_eq!(windows.calendar().shape(), WindowShape::Calendar);
    assert_ne!(
        windows.rolling().resets_at_unix(),
        windows.calendar().resets_at_unix()
    );
}

// --- lines 1213-1216: rate ceilings -----------------------------------

#[test]
fn every_rate_ceiling_is_its_own_field_and_a_long_window_names_its_own_period() {
    let rates = RateCeilings::uniform(Capacity::Unmeasured, Capacity::Unmeasured)
        .with_requests_per_minute(measured(
            60,
            "requests",
            header("x-ratelimit-limit-requests"),
        ))
        .with_tokens_per_minute(measured(
            40_000,
            "tokens",
            header("x-ratelimit-limit-tokens"),
        ))
        .with_max_concurrent_requests(measured(4, "requests", header("x-concurrency-limit")))
        .with_long_window_requests(Capacity::Measured(Reading::new(
            LongWindowRequests::new(NativeAmount::whole(1_000, "requests"), 86_400),
            OBSERVED,
            header("x-ratelimit-limit-requests-day"),
        )));
    assert_eq!(rates.requests_per_minute().value().unwrap().value(), 60);
    assert_eq!(rates.tokens_per_minute().value().unwrap().value(), 40_000);
    assert_eq!(rates.max_concurrent_requests().value().unwrap().value(), 4);
    let long = rates.long_window_requests().value().unwrap();
    assert_eq!(long.limit().value(), 1_000);
    assert_eq!(long.window_seconds(), 86_400);
    // A per-minute request ceiling and a per-day one are different
    // numbers about different periods, and neither is the other.
    assert_ne!(
        rates.requests_per_minute().value().unwrap().value(),
        long.limit().value()
    );
}

// --- lines 1217-1218: normalization never costs the raw numbers -------

#[test]
fn a_normalized_score_carries_the_provider_native_readings_it_was_computed_from() {
    let pool = Pool::unmeasured()
        .with_limit(measured_usd(
            10_000_000,
            ReadingSource::ProviderEndpoint("/api/v1/credits".to_owned()),
        ))
        .with_remaining(measured_usd(
            2_500_000,
            ReadingSource::ProviderEndpoint("/api/v1/credits".to_owned()),
        ));
    let score = pool.normalized().expect("both halves were read");
    // Both readings came from the provider's own usage endpoint, so
    // the score is exact — and `exact()` is the only accessor that
    // yields the digits, which is capability map line 1234.
    assert_eq!(score.percent().exact(), Some(25));
    // The percentage did not replace anything: the provider's own unit,
    // scale, numbers, observation time and source are all still here.
    assert_eq!(score.native_unit(), "USD");
    assert_eq!(score.remaining().value().value(), 2_500_000);
    assert_eq!(score.remaining().value().scale(), UnitScale::Millionths);
    assert_eq!(score.limit().value().value(), 10_000_000);
    assert_eq!(score.remaining().observed_at_unix(), OBSERVED);
    assert!(matches!(
        score.remaining().source(),
        ReadingSource::ProviderEndpoint(_)
    ));
    // And the pool itself is untouched.
    assert_eq!(pool.remaining().value().unwrap().value(), 2_500_000);
}

/// A percentage over two different units is not a percentage. Preserving
/// the native unit is what makes this detectable at all — a model that
/// kept only numbers would have divided requests by tokens and reported
/// a confident figure.
#[test]
fn two_incommensurable_readings_do_not_normalize_into_a_confident_number() {
    let mismatched_unit = Pool::unmeasured()
        .with_limit(measured(100, "tokens", header("x-limit")))
        .with_remaining(measured(25, "requests", header("x-remaining")));
    assert!(mismatched_unit.normalized().is_none());

    let mismatched_scale = Pool::unmeasured()
        .with_limit(measured(100, "USD", header("x-limit")))
        .with_remaining(measured_usd(25_000_000, header("x-remaining")));
    assert!(mismatched_scale.normalized().is_none());
}

#[test]
fn the_binding_pool_is_what_a_resources_normalized_capacity_reports() {
    let state = CapacityState::metered_balance()
        .with_credits(
            Pool::unmeasured()
                .with_limit(measured_usd(10_000_000, header("x-credit-limit")))
                .with_remaining(measured_usd(200_000, header("x-credit-remaining"))),
        )
        .with_tokens(
            TokenBudget::uniform(Pool::unmeasured()).with_combined(
                Pool::unmeasured()
                    .with_limit(measured(1_000, "tokens", header("x-token-limit")))
                    .with_remaining(measured(900, "tokens", header("x-token-remaining"))),
            ),
        );
    let (label, score) = state.normalized().expect("two pools were measured");
    // Two percent of credits, ninety percent of tokens: the resource has
    // two percent of usable capacity, and the answer says which pool.
    assert_eq!(label, "credits");
    assert_eq!(score.percent().exact(), Some(2));
    assert_eq!(score.native_unit(), "USD");
}

#[test]
fn a_resource_nothing_has_measured_reports_no_normalized_score_rather_than_zero() {
    for kind in crate::provider::registry::registry() {
        assert!(
            kind.capacity().normalized().is_none(),
            "{} invented a capacity score with no telemetry behind it",
            kind.label()
        );
    }
}

#[test]
fn every_pool_a_capacity_state_carries_is_listed_by_pools() {
    let state = ResourceKind::from_direct_provider("openrouter").capacity();
    let labels: Vec<&str> = state.pools().into_iter().map(|(label, _)| label).collect();
    assert_eq!(
        labels,
        vec![
            "tokens",
            "input tokens",
            "output tokens",
            "cached input tokens",
            "requests",
            "credits",
            "user budget",
            "rolling window",
            "calendar window",
        ]
    );
}
// ==== Phase 32B ======================================================

// --- line 1227: five terms, four of them classes ---------------------

#[test]
fn every_reading_origin_has_exactly_one_class_and_the_mapping_is_total() {
    let cases = [
        (
            ReadingSource::ResponseHeader("ratelimit-limit".to_owned()),
            TelemetryClass::Authoritative,
        ),
        (
            ReadingSource::ProviderEndpoint("/api/v1/key".to_owned()),
            TelemetryClass::Authoritative,
        ),
        (
            ReadingSource::HarnessReport("claude auth status --json".to_owned()),
            TelemetryClass::Authoritative,
        ),
        (
            ReadingSource::LocalObservation("this session's requests".to_owned()),
            TelemetryClass::Observed,
        ),
        (
            ReadingSource::InferredEstimate("the last window's rate".to_owned()),
            TelemetryClass::Estimated,
        ),
        (ReadingSource::UserConfiguration, TelemetryClass::Manual),
    ];
    for (source, expected) in cases {
        assert_eq!(source.class(), expected, "{source:?}");
    }
}

/// The fifth term. A quantity nobody read has no class, and every one of
/// the four unknown states renders the same word — so a view cannot
/// accidentally distinguish "opaque" from "unmeasured" as though one of
/// them were a number.
#[test]
fn a_quantity_nothing_read_has_no_class_and_renders_as_unknown() {
    let unknowns: [Capacity<NativeAmount>; 4] = [
        Capacity::Inapplicable,
        Capacity::ProviderOpaque,
        Capacity::Unmeasured,
        Capacity::DelegatedUpstream,
    ];
    for state in unknowns {
        assert_eq!(state.telemetry_class(), None);
        assert_eq!(state.telemetry_class_str(), UNKNOWN_TELEMETRY);
        assert_eq!(state.describe_source(), UNKNOWN_TELEMETRY);
    }
    let measured = measured(10, "requests", header("ratelimit-limit"));
    assert_eq!(
        measured.telemetry_class(),
        Some(TelemetryClass::Authoritative)
    );
    assert_ne!(measured.telemetry_class_str(), UNKNOWN_TELEMETRY);
}

/// The two classes named in line 1227 that `ReadingSource` had no origin
/// for before this phase. A test rather than a note, because their whole
/// purpose is to be distinguishable from the authoritative three.
#[test]
fn an_observed_and_an_inferred_reading_are_not_authoritative() {
    let observed = ReadingSource::LocalObservation("requests this session made".to_owned());
    let inferred = ReadingSource::InferredEstimate("the previous window".to_owned());
    assert!(!observed.class().is_authoritative());
    assert!(!inferred.class().is_authoritative());
    assert!(!observed.class().may_be_exact());
    assert!(!inferred.class().may_be_exact());
    assert_ne!(observed.class(), inferred.class());
}

// --- line 1228: authoritative wins ------------------------------------

#[test]
fn an_authoritative_reading_outranks_every_other_class_in_both_directions() {
    let authoritative = measured(10, "requests", header("ratelimit-limit"));
    let manual = Capacity::Measured(Reading::new(
        NativeAmount::whole(999, "requests"),
        OBSERVED + 1_000,
        ReadingSource::UserConfiguration,
    ));
    // The fresher, weaker reading still loses.
    assert_eq!(
        authoritative
            .clone()
            .prefer(manual.clone())
            .value()
            .unwrap()
            .value(),
        10
    );
    assert_eq!(manual.prefer(authoritative).value().unwrap().value(), 10);
}

#[test]
fn between_two_readings_of_one_class_the_fresher_one_wins() {
    let older = measured(10, "requests", header("ratelimit-limit"));
    let newer = Capacity::Measured(Reading::new(
        NativeAmount::whole(4, "requests"),
        OBSERVED + 60,
        ReadingSource::ResponseHeader("ratelimit-limit".to_owned()),
    ));
    assert_eq!(
        older.clone().prefer(newer.clone()).value().unwrap().value(),
        4
    );
    assert_eq!(newer.prefer(older).value().unwrap().value(), 4);
}

/// A measurement always beats an unknown, and — the part that matters —
/// an unknown never overwrites a measurement, so a failed telemetry pass
/// cannot blank a good earlier reading. Capability map line 1238.
#[test]
fn an_unknown_never_displaces_a_measurement_and_never_loses_its_own_kind() {
    let measured_value = measured(10, "requests", header("ratelimit-limit"));
    assert!(
        measured_value
            .clone()
            .prefer(Capacity::Unmeasured)
            .is_measured()
    );
    assert!(
        Capacity::<NativeAmount>::Unmeasured
            .prefer(measured_value)
            .is_measured()
    );
    // Two unknowns: the starting state's own distinction survives, which
    // is what keeps `opaque` from silently becoming `unmeasured`.
    let opaque: Capacity<NativeAmount> = Capacity::ProviderOpaque;
    assert!(!opaque.prefer(Capacity::Unmeasured).is_readable());
}

// --- line 1234: exactness is structural -------------------------------

#[test]
fn a_percentage_from_two_provider_readings_is_exact_and_renders_bare() {
    let pool = Pool::unmeasured()
        .with_limit(measured(1_000, "requests", header("ratelimit-limit")))
        .with_remaining(measured(250, "requests", header("ratelimit-remaining")));
    let score = pool.normalized().expect("both halves were read");
    assert_eq!(score.percent().exact(), Some(25));
    assert_eq!(score.percent().estimated(), None);
    assert_eq!(score.percent().render(), "25%");
    assert_eq!(score.percent().class(), TelemetryClass::Authoritative);
}

/// The line itself. One weak reading is enough to make the whole figure
/// an estimate, and there is no accessor that yields its digits without
/// the confidence and the source travelling with them.
#[test]
fn one_non_authoritative_reading_makes_the_whole_percentage_an_estimate() {
    let pool = Pool::unmeasured()
        .with_limit(Capacity::Measured(Reading::new(
            NativeAmount::whole(1_000, "requests"),
            OBSERVED,
            ReadingSource::UserConfiguration,
        )))
        .with_remaining(measured(250, "requests", header("ratelimit-remaining")));
    let score = pool.normalized().expect("both halves were read");

    assert_eq!(score.percent().exact(), None);
    let percentage = score.percent();
    let (percent, confidence, source) = percentage.estimated().expect("an estimate");
    assert_eq!(percent, 25);
    // Line 1235: a confidence value and a source description, both
    // required by the variant rather than added by a caller.
    assert_eq!(confidence, Confidence::Medium);
    assert!(source.contains("configuration"), "{source}");
    assert!(source.contains("ratelimit-remaining"), "{source}");

    let rendered = score.percent().render();
    assert!(rendered.starts_with('~'), "{rendered}");
    assert!(rendered.contains("estimated"), "{rendered}");
    assert!(rendered.contains("medium confidence"), "{rendered}");
}

/// An exact and an estimated figure at the same number must not render
/// the same way. This is the property a view could break and the one the
/// mutation ledger attacks.
#[test]
fn an_estimate_and_an_exact_reading_at_the_same_figure_never_render_alike() {
    let exact = Percentage::Exact(25);
    let estimated = Percentage::Estimated {
        percent: 25,
        confidence: Confidence::Low,
        source: "an estimate derived from the previous window".to_owned(),
    };
    assert_ne!(exact.render(), estimated.render());
    assert_ne!(exact.class(), estimated.class());
    assert_eq!(estimated.exact(), None);
}

#[test]
fn the_weakest_reading_decides_an_estimates_confidence() {
    let inferred = ReadingSource::InferredEstimate("the previous window".to_owned());
    let pool = Pool::unmeasured()
        .with_limit(Capacity::Measured(Reading::new(
            NativeAmount::whole(1_000, "requests"),
            OBSERVED,
            inferred,
        )))
        .with_remaining(measured(250, "requests", header("ratelimit-remaining")));
    let score = pool.normalized().expect("both halves were read");
    let percentage = score.percent();
    let (_, confidence, _) = percentage.estimated().expect("an estimate");
    // High (the header) and Low (the inference) give Low, not High.
    assert_eq!(confidence, Confidence::Low);
    assert_eq!(Confidence::High.weaker(Confidence::Low), Confidence::Low);
    assert_eq!(
        Confidence::Medium.weaker(Confidence::High),
        Confidence::Medium
    );
}

/// Capability map line 1234 names *subscription* percentages
/// specifically, and for a subscription the guard fires one layer
/// earlier: there is no percentage at all to mislabel.
#[test]
fn a_subscription_produces_no_percentage_for_line_1234_to_have_to_label() {
    let subscription = CapacityState::opaque_subscription();
    assert!(subscription.normalized().is_none());
    for (label, pool) in subscription.pools() {
        assert!(pool.normalized().is_none(), "{label} produced a percentage");
    }
}

// --- line 1236: when the last observation succeeded --------------------

#[test]
fn the_last_observation_is_the_latest_reading_anywhere_in_the_state() {
    let state = CapacityState::metered_balance();
    assert_eq!(state.last_observed_at_unix(), None);

    let state = state
        .with_credits(Pool::unmeasured().with_limit(measured_usd(
            10_000_000,
            ReadingSource::ProviderEndpoint("/api/v1/credits".to_owned()),
        )))
        .with_plan(Capacity::Measured(Reading::new(
            KnownPlan::new("pro"),
            OBSERVED + 500,
            ReadingSource::UserConfiguration,
        )));
    // The latest of the two, not the first and not the strongest.
    assert_eq!(state.last_observed_at_unix(), Some(OBSERVED + 500));
}

#[test]
fn a_rate_ceiling_alone_is_still_a_successful_observation() {
    let state = CapacityState::metered_balance().with_rate_ceilings(
        RateCeilings::uniform(Capacity::Unmeasured, Capacity::Unmeasured)
            .with_requests_per_minute(measured(300, "requests", header("ratelimit-policy"))),
    );
    assert_eq!(state.last_observed_at_unix(), Some(OBSERVED));
}

// --- line 1237: staleness against a configurable age -------------------

#[test]
fn a_reading_is_stale_only_once_it_is_older_than_the_age_it_is_given() {
    let reading = Reading::new(1_i64, OBSERVED, header("ratelimit-limit"));
    assert_eq!(
        reading.freshness(OBSERVED + 100, 120),
        Freshness::Fresh { age_seconds: 100 }
    );
    // Exactly at the limit is not yet stale.
    assert_eq!(
        reading.freshness(OBSERVED + 120, 120),
        Freshness::Fresh { age_seconds: 120 }
    );
    assert_eq!(
        reading.freshness(OBSERVED + 121, 120),
        Freshness::Stale {
            age_seconds: 121,
            stale_after_seconds: 120
        }
    );
}

/// The same reading, two ages: which is the whole content of
/// "provider-specific configurable age". A staleness rule that answered
/// the same for every provider would not be one.
#[test]
fn the_same_reading_is_fresh_under_one_configured_age_and_stale_under_another() {
    let reading = Reading::new(1_i64, OBSERVED, header("ratelimit-limit"));
    let now = OBSERVED + 300;
    assert!(!reading.freshness(now, 900).is_stale());
    assert!(reading.freshness(now, 120).is_stale());
}

#[test]
fn a_reading_stamped_in_the_future_is_fresh_rather_than_an_error() {
    let reading = Reading::new(1_i64, OBSERVED + 60, header("ratelimit-limit"));
    assert!(!reading.freshness(OBSERVED, 30).is_stale());
}

// --- lines 1231 and 1233: the plan ------------------------------------

#[test]
fn a_plan_is_a_reading_with_an_origin_rather_than_a_bare_string() {
    let harness = CapacityState::opaque_subscription().with_plan(Capacity::Measured(Reading::new(
        KnownPlan::new("max"),
        OBSERVED,
        ReadingSource::HarnessReport("claude auth status --json".to_owned()),
    )));
    assert_eq!(harness.plan().value().unwrap().name(), "max");
    assert_eq!(
        harness.plan().telemetry_class(),
        Some(TelemetryClass::Authoritative)
    );

    // A local server has no plan, and that is `Inapplicable` rather than
    // unmeasured — nothing should ever go looking for one.
    assert!(!CapacityState::unmetered_local().plan().is_readable());
    // The gateway's plan belongs to its upstream.
    assert!(matches!(
        CapacityState::delegated_to_upstream().plan(),
        Capacity::DelegatedUpstream
    ));
    // A subscription's is unmeasured — a number 32B may legitimately read.
    assert!(CapacityState::opaque_subscription().plan().is_readable());
}

// --- line 1240: one word per resource ----------------------------------

#[test]
fn a_resources_telemetry_class_is_the_strongest_claim_anything_in_it_rests_on() {
    assert_eq!(CapacityState::metered_balance().telemetry_class(), None);
    assert_eq!(
        CapacityState::metered_balance().telemetry_class_str(),
        UNKNOWN_TELEMETRY
    );

    let manual_only = CapacityState::metered_balance().with_plan(Capacity::Measured(Reading::new(
        KnownPlan::new("pro"),
        OBSERVED,
        ReadingSource::UserConfiguration,
    )));
    assert_eq!(manual_only.telemetry_class(), Some(TelemetryClass::Manual));

    let plus_header = manual_only.with_requests(Pool::unmeasured().with_limit(measured(
        300,
        "requests",
        header("ratelimit-limit"),
    )));
    assert_eq!(
        plus_header.telemetry_class(),
        Some(TelemetryClass::Authoritative)
    );
}
