use super::*;
use crate::routing::evidence::{ContextState, Outcome};

const NOW: i64 = 1_800_000_000;

/// A row carrying only what this module reads. Built by hand rather than
/// through the ledger because these are unit tests of arithmetic — the
/// ledger's own round trip is `crate::database`'s migration-23 test.
fn row(
    provider: &str,
    quota_context: Option<&str>,
    observed_at_unix: i64,
    class: Option<TaskClass>,
) -> RoutingObservation {
    RoutingObservation {
        seq: observed_at_unix,
        project_id: "p".to_owned(),
        observed_at_unix,
        provider: provider.to_owned(),
        model: "m".to_owned(),
        route: None,
        quota_context: quota_context.map(str::to_owned),
        harness: None,
        purpose: None,
        dispatched_at_unix: None,
        first_byte_at_unix: None,
        first_token_at_unix: None,
        first_tool_call_at_unix: None,
        completed_at_unix: None,
        first_byte_ms: None,
        first_token_ms: None,
        first_tool_call_ms: None,
        completed_ms: None,
        input_tokens: None,
        output_tokens: None,
        cached_input_tokens: None,
        cost: None,
        tool_rounds: None,
        retries: None,
        repairs: None,
        failovers: None,
        outcome: Some(Outcome::Succeeded),
        failure_class: None,
        task_class: class,
        // Migration 24's three columns. This module reads none of them;
        // they are here because the struct literal must be complete.
        session_id: None,
        effort_level: None,
        turn_shape: None,
        context_state: ContextState::Unknown,
    }
}

/// `count` rows spread one per `spacing` seconds, ending `ends_ago`
/// seconds before [`NOW`].
fn steady(
    provider: &str,
    count: usize,
    spacing: i64,
    ends_ago: i64,
    class: Option<TaskClass>,
) -> Vec<RoutingObservation> {
    (0..count)
        .map(|i| {
            let at = NOW - ends_ago - (count as i64 - 1 - i as i64) * spacing;
            row(provider, Some("acct-a"), at, class)
        })
        .collect()
}

fn measured_requests_amount(value: i64) -> Capacity<NativeAmount> {
    Capacity::Measured(crate::provider::quota::Reading::new(
        NativeAmount::whole(value, "requests"),
        NOW,
        crate::provider::quota::ReadingSource::ProviderEndpoint("https://example/usage".to_owned()),
    ))
}

// --- line 1281: the robust statistic --------------------------------

/// **Line 1281's killer.** One bucket carrying a burst forty times the
/// steady rate must not move the estimate the way a mean would.
///
/// The two lists share every steady row and differ only by the burst, so
/// nothing but the statistic can explain a difference. The mean is
/// computed here from the same buckets, so the assertion is not against
/// a hard-coded number that a future constant change would falsify — it
/// is against the naive statistic itself, which is exactly the thing the
/// mutation swaps in.
#[test]
fn one_outlier_bucket_moves_the_median_far_less_than_it_moves_a_mean() {
    // Twelve five-minute buckets, two requests each.
    let mut steady_rows: Vec<RoutingObservation> = Vec::new();
    for bucket in 0..12i64 {
        for slot in 0..2i64 {
            steady_rows.push(row(
                "p",
                Some("acct-a"),
                NOW - 3600 + bucket * BUCKET_SECONDS + slot * 60,
                None,
            ));
        }
    }
    let mut bursty = steady_rows.clone();
    // One bucket with eighty extra requests — an outlier by any reading.
    for extra in 0..80i64 {
        bursty.push(row(
            "p",
            Some("acct-a"),
            NOW - 3600 + 5 * BUCKET_SECONDS + extra % 300,
            None,
        ));
    }
    bursty.sort_by_key(|r| r.observed_at_unix);

    let steady_refs: Vec<&RoutingObservation> = steady_rows.iter().collect();
    let bursty_refs: Vec<&RoutingObservation> = bursty.iter().collect();

    let steady_median = median_rate_per_hour(&steady_refs, NOW).unwrap();
    let bursty_median = median_rate_per_hour(&bursty_refs, NOW).unwrap();

    let mean = |rows: &[&RoutingObservation]| {
        let counts = bucket_counts(rows, NOW);
        let sum: f64 = counts.iter().sum();
        sum / counts.len() as f64 * SECONDS_PER_HOUR / BUCKET_SECONDS as f64
    };
    let steady_mean = mean(&steady_refs);
    let bursty_mean = mean(&bursty_refs);

    let median_shift = (bursty_median - steady_median).abs();
    let mean_shift = (bursty_mean - steady_mean).abs();

    assert!(
        median_shift < mean_shift / 4.0,
        "the robust statistic must absorb the outlier: median moved {median_shift:.1} \
         req/h, a mean moved {mean_shift:.1} req/h"
    );
    assert!(
        mean_shift > 50.0,
        "the fixture must contain an outlier a mean would actually notice, moved \
         {mean_shift:.1} req/h"
    );
}

/// The median itself, on the two shapes a bucket list takes.
#[test]
fn the_median_is_the_middle_and_averages_the_two_middles_when_even() {
    assert_eq!(median(&[]), None);
    assert_eq!(median(&[3.0]), Some(3.0));
    assert_eq!(median(&[9.0, 1.0, 2.0]), Some(2.0));
    assert_eq!(median(&[1.0, 2.0, 3.0, 100.0]), Some(2.5));
}

// --- lines 1278 and 1279 --------------------------------------------

/// **Line 1278's killer.** A percentage without a native count is
/// *insufficiently known*: no forecast, never a fabricated one.
///
/// Every non-`Measured` state is asserted, because they mean different
/// things and a reader must not be able to fix one and miss three.
#[test]
fn time_to_exhaustion_is_absent_without_a_measured_request_unit_amount() {
    let rows = steady("p", 20, 120, 0, None);
    let key = ResourceKey {
        provider: "p",
        quota_context: None,
    };
    // The rate itself is established — so a `None` below is about the
    // remaining amount and nothing else.
    assert!(burn_rate(&rows, key, NOW, Some(7200)).is_some());

    for absent in [
        Capacity::Inapplicable,
        Capacity::ProviderOpaque,
        Capacity::Unmeasured,
        Capacity::DelegatedUpstream,
    ] {
        assert_eq!(
            forecast(&rows, key, &absent, NOW, Some(7200)),
            None,
            "{} is not a count, so it cannot be divided by a rate",
            absent.as_str()
        );
    }

    // A measured amount in the wrong unit is equally absent: tokens over
    // requests-per-hour is not a time.
    let tokens = Capacity::Measured(crate::provider::quota::Reading::new(
        NativeAmount::whole(500, "tokens"),
        NOW,
        crate::provider::quota::ReadingSource::ProviderEndpoint("https://example/usage".to_owned()),
    ));
    assert_eq!(forecast(&rows, key, &tokens, NOW, Some(7200)), None);

    // And with a real request count it answers.
    let forecast = forecast(&rows, key, &measured_requests_amount(60), NOW, Some(7200))
        .expect("a measured request count and an established rate is a forecast");
    assert!(forecast.seconds_to_exhaustion > 0);
}

/// The arithmetic itself: sixty requests left at thirty an hour is two
/// hours, and the rate is the one `burn_rate` reported.
#[test]
fn time_to_exhaustion_is_the_remaining_count_over_the_rate() {
    // One request every two minutes: 30/hour by the median of buckets
    // holding 2.5 each — spacing is chosen so every bucket holds the
    // same count and the median is unambiguous.
    let rows = steady("p", 24, 120, 0, None);
    let key = ResourceKey {
        provider: "p",
        quota_context: None,
    };
    let rate = burn_rate(&rows, key, NOW, None).unwrap();
    let forecast = forecast(&rows, key, &measured_requests_amount(60), NOW, None).unwrap();
    let expected = (60.0 / rate.requests_per_hour * SECONDS_PER_HOUR) as i64;
    assert_eq!(forecast.seconds_to_exhaustion, expected);
    assert_eq!(forecast.requests_per_hour, rate.requests_per_hour);
}

/// **Line 1279's killer.** The verdict is a comparison against the
/// reset, and it is absent when either side is.
#[test]
fn survives_until_reset_compares_against_the_reset_and_is_none_without_one() {
    let rows = steady("p", 24, 120, 0, None);
    let key = ResourceKey {
        provider: "p",
        quota_context: None,
    };
    let remaining = measured_requests_amount(60);

    let unknown_reset = forecast(&rows, key, &remaining, NOW, None).unwrap();
    assert_eq!(
        unknown_reset.survives_until_reset, None,
        "an unknown reset produces no verdict — never a `true` built on an absence"
    );

    let to_exhaustion = unknown_reset.seconds_to_exhaustion;

    // A reset that arrives before exhaustion: it survives.
    let survives = forecast(&rows, key, &remaining, NOW, Some(to_exhaustion - 600)).unwrap();
    assert_eq!(survives.survives_until_reset, Some(true));

    // A reset that arrives after exhaustion: it does not.
    let does_not = forecast(&rows, key, &remaining, NOW, Some(to_exhaustion + 600)).unwrap();
    assert_eq!(does_not.survives_until_reset, Some(false));
}

/// Line 1280's own threshold, on the type that carries it.
#[test]
fn well_before_is_half_the_window_and_is_false_without_a_reset() {
    let base = ExhaustionForecast {
        requests_per_hour: 10.0,
        seconds_to_exhaustion: 1000,
        survives_until_reset: Some(false),
        seconds_until_reset: Some(3000),
        rows: 20,
    };
    assert!(base.exhausts_well_before_reset(), "1000 < 0.5 * 3000");

    let marginal = ExhaustionForecast {
        seconds_until_reset: Some(1900),
        ..base
    };
    assert!(
        !marginal.exhausts_well_before_reset(),
        "1000 is not well before 1900 — inside the estimator's own tolerance"
    );

    let no_reset = ExhaustionForecast {
        seconds_until_reset: None,
        survives_until_reset: None,
        ..base
    };
    assert!(!no_reset.exhausts_well_before_reset());
}

// --- line 1277 ------------------------------------------------------

/// The floor: too little history is no rate, never a zero and never a
/// figure over three rows.
#[test]
fn a_burn_rate_below_the_minimum_row_count_is_absent() {
    let key = ResourceKey {
        provider: "p",
        quota_context: None,
    };
    let thin = steady("p", MIN_ROWS_FOR_BURN_RATE - 1, 120, 0, None);
    assert_eq!(burn_rate(&thin, key, NOW, None), None);

    let enough = steady("p", MIN_ROWS_FOR_BURN_RATE, 120, 0, None);
    assert!(burn_rate(&enough, key, NOW, None).is_some());
}

/// The key is the provider, narrowed by `quota_context` exactly as
/// `recent_credential_throttles` narrows a credential — including its
/// refusal to narrow when the history is only partly attributed.
#[test]
fn the_burn_rate_is_keyed_by_provider_and_narrowed_by_the_account() {
    // Twenty rows for `p`, eight of them relabelled to a second
    // account, so BOTH the narrowed and the wide set clear
    // `MIN_ROWS_FOR_BURN_RATE` — otherwise a `None` here would be about
    // the floor rather than about the key.
    let mut rows = steady("p", 20, 120, 0, None);
    rows.extend(steady("other", 12, 120, 0, None));
    for row in rows.iter_mut().filter(|r| r.provider == "p").take(8) {
        row.quota_context = Some("acct-b".to_owned());
    }

    let wide = burn_rate(
        &rows,
        ResourceKey {
            provider: "p",
            quota_context: None,
        },
        NOW,
        None,
    )
    .unwrap();
    assert_eq!(wide.rows, 20, "another provider's rows are not this one's");
    assert!(!wide.account_narrowed);

    let narrowed = burn_rate(
        &rows,
        ResourceKey {
            provider: "p",
            quota_context: Some("acct-a"),
        },
        NOW,
        None,
    )
    .unwrap();
    assert_eq!(narrowed.rows, 12);
    assert!(narrowed.account_narrowed);

    // One unattributed row and the narrowing is refused wholesale — a
    // fraction of a partly-labelled history is a wrong number wearing a
    // right label.
    rows[0].quota_context = None;
    let refused = burn_rate(
        &rows,
        ResourceKey {
            provider: "p",
            quota_context: Some("acct-a"),
        },
        NOW,
        None,
    )
    .unwrap();
    assert!(!refused.account_narrowed);
    assert_eq!(refused.rows, 20);
}

/// A token rate exists only where rows already carry token counts —
/// nothing here parses a body, and a window of relayed exchanges has
/// none.
#[test]
fn a_token_rate_exists_only_where_rows_already_carry_tokens() {
    let key = ResourceKey {
        provider: "p",
        quota_context: None,
    };
    let untokened = steady("p", 12, 120, 0, None);
    assert_eq!(
        burn_rate(&untokened, key, NOW, None)
            .unwrap()
            .tokens_per_hour,
        None
    );

    let mut tokened = untokened.clone();
    for row in &mut tokened {
        row.input_tokens = Some(100);
        row.output_tokens = Some(50);
    }
    let rate = burn_rate(&tokened, key, NOW, None).unwrap().tokens_per_hour;
    assert!(rate.is_some_and(|value| value > 0.0));
}

// --- line 1282 ------------------------------------------------------

/// **Line 1282's killer, first half.** Rows before an idle gap longer
/// than the constant contribute nothing.
#[test]
fn rows_before_a_long_idle_gap_are_excluded() {
    // A burst yesterday, silence, then a quiet hour now.
    let mut rows = steady("p", 40, 30, IDLE_GAP_SECONDS + 7200, None);
    rows.extend(steady("p", 12, 300, 0, None));
    rows.sort_by_key(|r| r.observed_at_unix);

    let live = live_rows(&rows, NOW, None);
    assert_eq!(
        live.len(),
        12,
        "only the rows after the gap are still evidence about now"
    );

    // And the rate the caller sees is the recent one, not yesterday's.
    let key = ResourceKey {
        provider: "p",
        quota_context: None,
    };
    let rate = burn_rate(&rows, key, NOW, None).unwrap();
    assert_eq!(rate.rows, 12);
}

/// **Line 1282's killer, second half.** Rows from before the resource's
/// own window turned are spent against a quota that no longer exists.
#[test]
fn rows_before_the_last_reset_boundary_are_excluded() {
    // The window turned an hour ago: `seconds_until_reset` is negative,
    // which `CapacityState::seconds_until_reset` returns as-is.
    let turned_ago = -3600;
    let mut rows = steady("p", 10, 120, 7200, None); // before the turn
    rows.extend(steady("p", 12, 120, 0, None)); // after it
    rows.sort_by_key(|r| r.observed_at_unix);

    let live = live_rows(&rows, NOW, Some(turned_ago));
    assert_eq!(
        live.len(),
        12,
        "capacity given back at the reset is not capacity this rate should count"
    );
    // With no reset known, nothing is excluded on that ground.
    assert_eq!(live_rows(&rows, NOW, None).len(), 22);
}

// --- line 1276 ------------------------------------------------------

/// Per-class rates: one entry per class that has rows, in declaration
/// order, and no entry at all for a class nothing was routed as.
#[test]
fn task_class_rates_name_only_the_classes_that_have_rows() {
    let mut rows = steady("p", 12, 120, 0, Some(TaskClass::CodeModification));
    rows.extend(steady("p", 6, 240, 0, Some(TaskClass::Question)));
    // Rows carrying no class at all join no bucket.
    rows.extend(steady("p", 20, 60, 0, None));
    rows.sort_by_key(|r| r.observed_at_unix);

    let rates = task_class_request_rates(&rows, NOW, None);
    let classes: Vec<TaskClass> = rates.iter().map(|rate| rate.class).collect();
    assert_eq!(
        classes,
        vec![TaskClass::Question, TaskClass::CodeModification],
        "declaration order, and only the classes with rows"
    );
    assert_eq!(rates[0].rows, 6);
    assert_eq!(rates[1].rows, 12);
    assert!(rates.iter().all(|rate| rate.requests_per_hour > 0.0));

    assert!(
        task_class_request_rates(&[], NOW, None).is_empty(),
        "no rows is no rates, never five zeroes"
    );
}

/// The per-class rate obeys line 1282 too: it is computed over live rows
/// and not over the whole window.
#[test]
fn task_class_rates_see_only_live_rows() {
    let mut rows = steady(
        "p",
        30,
        30,
        IDLE_GAP_SECONDS + 7200,
        Some(TaskClass::ShellWork),
    );
    rows.extend(steady("p", 9, 300, 0, Some(TaskClass::ShellWork)));
    rows.sort_by_key(|r| r.observed_at_unix);

    let rates = task_class_request_rates(&rows, NOW, None);
    assert_eq!(rates.len(), 1);
    assert_eq!(rates[0].rows, 9);
}

/// **Line 1275's killer.** A class whose rows all carry token counts
/// gets a token rate; a class whose rows carry none gets `None` — never
/// a `0` a caller could mistake for a genuinely idle class.
#[test]
fn task_class_rates_carry_a_token_rate_only_where_rows_carry_tokens() {
    let mut tokened = steady("p", 12, 120, 0, Some(TaskClass::CodeModification));
    for row in &mut tokened {
        row.input_tokens = Some(100);
        row.output_tokens = Some(50);
    }
    let mut rows = tokened;
    rows.extend(steady("p", 8, 120, 0, Some(TaskClass::Question)));
    rows.sort_by_key(|r| r.observed_at_unix);

    let rates = task_class_request_rates(&rows, NOW, None);
    let by_class = |class: TaskClass| {
        rates
            .iter()
            .find(|rate| rate.class == class)
            .unwrap_or_else(|| panic!("{class:?} missing from {rates:?}"))
    };

    let tokened_rate = by_class(TaskClass::CodeModification);
    assert!(
        tokened_rate
            .tokens_per_hour
            .is_some_and(|value| value > 0.0),
        "every row of this class carries tokens: {tokened_rate:?}"
    );
    assert_eq!(tokened_rate.token_rows, 12);

    let untokened_rate = by_class(TaskClass::Question);
    assert_eq!(
        untokened_rate.tokens_per_hour, None,
        "no row of this class carries a token count, so the rate is absent, not zero"
    );
    assert_eq!(untokened_rate.token_rows, 0);
}

/// Mixed rows within one class: only the token-carrying rows contribute
/// to that class's token rate, and the class's request rate is
/// unaffected either way.
#[test]
fn task_class_rates_token_axis_counts_only_the_counted_rows_of_its_own_class() {
    let mut rows = steady("p", 12, 120, 0, Some(TaskClass::ShellWork));
    for row in rows.iter_mut().take(5) {
        row.input_tokens = Some(40);
        row.output_tokens = Some(10);
    }

    let rates = task_class_request_rates(&rows, NOW, None);
    assert_eq!(rates.len(), 1);
    let rate = &rates[0];
    assert_eq!(rate.rows, 12, "the request count is unaffected by tokens");
    assert_eq!(rate.token_rows, 5, "only the five rows that carry tokens");
    assert!(rate.tokens_per_hour.is_some_and(|value| value > 0.0));
}

/// A zero or negative burn rate is "no forecast", not an infinity and
/// not a panic — the guard the packet's cross-platform section names.
#[test]
fn a_rate_of_zero_produces_no_forecast_rather_than_an_infinity() {
    // Every row far enough in the past that `now` is many empty buckets
    // later: the median bucket is empty, so the rate is zero.
    let rows = steady("p", 12, 60, 4 * 3600, None);
    let key = ResourceKey {
        provider: "p",
        quota_context: None,
    };
    let rate = burn_rate(&rows, key, NOW, None).unwrap();
    assert_eq!(rate.requests_per_hour, 0.0);
    assert_eq!(
        forecast(&rows, key, &measured_requests_amount(60), NOW, None),
        None,
        "nothing is being spent, so nothing exhausts"
    );
}
