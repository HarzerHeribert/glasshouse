use super::*;
use crate::provider::quota::{Percentage, TelemetryClass, UnitScale};
use crate::provider::registry::ResourceKind;

const OBSERVED: i64 = 1_787_800_000;

/// The exact header set `https://anyrouter.dev/api/v1/models` answered
/// with, unauthenticated, on 2026-08-27 — copied field for field from the
/// response, not composed.
///
/// It is a fixture of a **measurement**, which is a different thing from
/// an invented fixture: every name and every value here was observed, and
/// the two facts this package leans on hardest are visible in it — the
/// ceiling arrived and the remaining count did not.
fn anyrouter_models_headers() -> Vec<(&'static str, &'static str)> {
    vec![
        ("date", "Thu, 27 Aug 2026 14:22:24 GMT"),
        ("content-type", "application/json"),
        ("content-length", "375402"),
        ("cf-ray", "a31bba6cb87a290f-DUS"),
        ("cf-cache-status", "HIT"),
        ("access-control-allow-origin", "*"),
        (
            "access-control-expose-headers",
            "X-Request-Id,X-AnyRouter-Trace-Id,X-AnyRouter-Handler,X-RateLimit-Limit,\
             X-RateLimit-Remaining,X-RateLimit-Reset,X-RateLimit-Tier,X-RateLimit-Window,\
             RateLimit-Limit,RateLimit-Policy,RateLimit-Remaining,RateLimit-Reset,Retry-After",
        ),
        ("ratelimit-limit", "300"),
        ("ratelimit-policy", "300;w=60"),
        ("x-ratelimit-limit", "300"),
        ("x-ratelimit-tier", "ip"),
        ("x-ratelimit-window", "60"),
        ("x-anyrouter-handler", "api"),
        ("server", "cloudflare"),
    ]
}

// --- line 1229: read rate-limit headers ------------------------------

#[test]
fn the_headers_a_real_provider_sent_are_read_into_a_ceiling_and_a_window() {
    let read = RateLimitHeaders::read(anyrouter_models_headers());
    assert_eq!(read.limit(), Some(300));
    assert_eq!(read.window_seconds(), Some(60));
    // The host advertises `RateLimit-Remaining` in its CORS list and did
    // not send it. Glasshouse does not fill one in.
    assert_eq!(read.remaining(), None);
    assert_eq!(read.reset(), None);
}

#[test]
fn a_response_with_no_rate_limit_header_reads_as_nothing_rather_than_as_zero() {
    // OpenRouter's own `GET /api/v1/models` response, same day: no
    // rate-limit header of any name.
    let read = RateLimitHeaders::read(vec![
        ("date", "Thu, 27 Aug 2026 14:22:24 GMT"),
        ("content-type", "application/json"),
        ("cf-cache-status", "HIT"),
    ]);
    assert!(read.is_empty());
    assert_eq!(read.limit(), None);
    assert_eq!(read.remaining(), None);
}

#[test]
fn a_ceiling_over_a_minute_becomes_a_requests_per_minute_limit() {
    let state = RateLimitHeaders::read(anyrouter_models_headers()).apply_to(
        ResourceKind::from_direct_provider("anyrouter").capacity(),
        OBSERVED,
    );
    let per_minute = state.rate_ceilings().requests_per_minute();
    let amount = per_minute.value().expect("a ceiling was read");
    assert_eq!(amount.value(), 300);
    assert_eq!(amount.unit(), "requests");
    // And it did not land in the long-window pool as well.
    assert!(!state.rate_ceilings().long_window_requests().is_measured());
}

/// Capability map line 1216 — "requests-per-day **or equivalent**".
///
/// The same `300` over an hour is not a per-minute ceiling, and a parser
/// that filed it as one would report a resource as fifty times more
/// throttled than it is.
#[test]
fn a_ceiling_over_a_longer_window_becomes_a_long_window_pool_carrying_its_period() {
    let state = RateLimitHeaders::read(vec![
        ("ratelimit-limit", "300"),
        ("ratelimit-policy", "300;w=3600"),
    ])
    .apply_to(
        ResourceKind::from_direct_provider("anyrouter").capacity(),
        OBSERVED,
    );
    assert!(!state.rate_ceilings().requests_per_minute().is_measured());
    let long = state
        .rate_ceilings()
        .long_window_requests()
        .value()
        .expect("a long-window ceiling was read");
    assert_eq!(long.limit().value(), 300);
    assert_eq!(long.window_seconds(), 3600);
}

/// A limit with no stated period is not a rate, and inventing the period
/// is the one thing this parser must not do.
#[test]
fn a_ceiling_with_no_stated_window_becomes_no_rate_at_all() {
    let read = RateLimitHeaders::read(vec![("ratelimit-limit", "300")]);
    assert_eq!(read.limit(), Some(300));
    assert_eq!(read.window_seconds(), None);
    let state = read.apply_to(
        ResourceKind::from_direct_provider("anyrouter").capacity(),
        OBSERVED,
    );
    assert!(!state.rate_ceilings().requests_per_minute().is_measured());
    assert!(!state.rate_ceilings().long_window_requests().is_measured());
    // The pool's ceiling is still recorded — a limit without a period is
    // a real fact about the pool, just not a rate.
    assert!(state.requests().limit().is_measured());
}

#[test]
fn the_ietf_spelling_wins_over_the_x_prefixed_one_when_a_host_sends_both() {
    let read = RateLimitHeaders::read(vec![
        ("ratelimit-limit", "300"),
        ("x-ratelimit-limit", "999"),
    ]);
    assert_eq!(read.limit(), Some(300));
    assert!(read.read_from().contains(&"ratelimit-limit"));
    assert!(!read.read_from().contains(&"x-ratelimit-limit"));
}

#[test]
fn the_x_prefixed_spelling_fills_in_when_the_standard_one_is_absent() {
    let read = RateLimitHeaders::read(vec![
        ("X-RateLimit-Remaining", "17"),
        ("X-RateLimit-Window", "60"),
    ]);
    assert_eq!(read.remaining(), Some(17));
    assert_eq!(read.window_seconds(), Some(60));
}

#[test]
fn a_policy_value_yields_its_window_and_not_its_quota_figure() {
    assert_eq!(parse_policy_window("300;w=60"), Some(60));
    assert_eq!(parse_policy_window("100;w=3600;burst=10"), Some(3600));
    // No `w=` parameter: there is no window here to read.
    assert_eq!(parse_policy_window("300"), None);
}

// --- line 1238: nothing here can fail a session ----------------------

#[test]
fn an_unparseable_header_value_leaves_the_quantity_unmeasured_rather_than_erroring() {
    for value in ["", "unlimited", "-5", "12.5", "  ", "300; w=60"] {
        let read = RateLimitHeaders::read(vec![("ratelimit-limit", value)]);
        assert_eq!(read.limit(), None, "`{value}` must not parse as a ceiling");
        let state = read.apply_to(
            ResourceKind::from_direct_provider("anyrouter").capacity(),
            OBSERVED,
        );
        // And the state is still complete and printable.
        assert!(!state.requests().limit().is_measured());
    }
}

#[test]
fn a_status_body_that_is_not_what_the_parser_expects_leaves_the_plan_unmeasured() {
    for body in [
        "",
        "not json",
        "[]",
        "{}",
        r#"{"subscriptionType": ""}"#,
        r#"{"subscriptionType": 7}"#,
        r#"{"subscription_type": "max"}"#,
    ] {
        let report = read_harness_plan(body, OBSERVED, "claude auth status --json");
        assert!(
            !report.known_plan().is_measured(),
            "`{body}` must not yield a plan"
        );
    }
}

// --- line 1232: the two seams are independent ------------------------

/// The line's load-bearing word is *independently*. Applied in either
/// order, in isolation or together, neither reader disturbs the other's
/// fields.
#[test]
fn the_two_telemetry_seams_do_not_overwrite_each_other() {
    let headers = RateLimitHeaders::read(anyrouter_models_headers());
    let harness = HarnessTelemetry::plan("max", OBSERVED, "claude auth status --json");

    let provider_only =
        apply_provider_headers(CapacityState::opaque_subscription(), &headers, OBSERVED);
    let harness_only = apply_harness_report(CapacityState::opaque_subscription(), &harness);
    let both = apply_harness_report(provider_only.clone(), &harness);
    let both_reversed = apply_provider_headers(harness_only.clone(), &headers, OBSERVED);

    // A harness report leaves the rate ceilings exactly as they were...
    assert_eq!(
        harness_only.rate_ceilings(),
        CapacityState::opaque_subscription().rate_ceilings()
    );
    // ...and provider headers leave the plan exactly as it was.
    assert_eq!(
        provider_only.plan(),
        CapacityState::opaque_subscription().plan()
    );
    // Order does not matter, which is what independence means.
    assert_eq!(both, both_reversed);
}

/// A first-party subscription's pools are `ProviderOpaque`, and Phase 32A
/// called `is_readable()` its best property. This is the first reader
/// with the opportunity to break it.
#[test]
fn a_reader_cannot_fill_in_a_pool_the_provider_publishes_nothing_for() {
    let subscription = CapacityState::opaque_subscription();
    assert!(!subscription.requests().limit().is_readable());
    let after = RateLimitHeaders::read(vec![
        ("ratelimit-limit", "300"),
        ("ratelimit-remaining", "42"),
    ])
    .apply_to(subscription, OBSERVED);
    assert!(!after.requests().limit().is_measured());
    assert!(!after.requests().remaining().is_measured());
}

#[test]
fn a_local_server_has_no_plan_for_a_harness_or_a_user_to_set() {
    let local = CapacityState::unmetered_local();
    let after = apply_harness_report(
        local,
        &HarnessTelemetry::plan("max", OBSERVED, "claude auth status --json"),
    );
    assert!(!after.plan().is_measured());
    let configured = apply_user_configuration(
        CapacityState::unmetered_local(),
        Some("pro"),
        None,
        None,
        OBSERVED,
    );
    assert!(!configured.plan().is_measured());
}

// --- line 1228: authoritative wins -----------------------------------

#[test]
fn a_harness_report_outranks_a_plan_the_user_typed_whichever_arrives_first() {
    let configured = apply_user_configuration(
        CapacityState::opaque_subscription(),
        Some("pro"),
        None,
        None,
        OBSERVED,
    );
    assert_eq!(
        configured.plan().telemetry_class(),
        Some(TelemetryClass::Manual)
    );

    let reported = HarnessTelemetry::plan("max", OBSERVED, "claude auth status --json");
    let harness_last = apply_harness_report(configured, &reported);
    assert_eq!(harness_last.plan().value().unwrap().name(), "max");
    assert_eq!(
        harness_last.plan().telemetry_class(),
        Some(TelemetryClass::Authoritative)
    );

    // And the other order: a manual entry never displaces the harness.
    let harness_first = apply_harness_report(CapacityState::opaque_subscription(), &reported);
    let then_user = apply_user_configuration(harness_first, Some("pro"), None, None, OBSERVED);
    assert_eq!(then_user.plan().value().unwrap().name(), "max");
    assert_eq!(
        then_user.plan().telemetry_class(),
        Some(TelemetryClass::Authoritative)
    );
}

// --- line 1230: a provider's own usage endpoint -----------------------

/// The exact shape `GET https://openrouter.ai/api/v1/key` answered with,
/// authenticated, 2026-08-27, field names and *types* recorded in
/// `.agent-runtime/probe-quota-headers-2026-08-27.md` — never a value.
/// `data.limit`, `data.limit_remaining` and `data.limit_reset` really
/// were `null` on the probed account; `9` below stands in for `usage`'s
/// real figure, which was never recorded, and is not asserted on for
/// exactly that reason — this reader does not apply `usage` to anything.
const OPENROUTER_KEY_BODY: &str = r#"{
    "data": {
        "limit": null,
        "limit_remaining": null,
        "limit_reset": null,
        "usage": 9,
        "usage_daily": 9,
        "usage_weekly": 9,
        "usage_monthly": 9,
        "is_free_tier": false,
        "include_byok_in_limit": false,
        "rate_limit": { "requests": 9, "interval": "10s" }
    }
}"#;

#[test]
fn a_null_limit_is_read_as_present_and_inapplicable_not_as_absent() {
    let usage = ProviderUsage::read(OPENROUTER_KEY_BODY);
    assert!(
        !usage.is_empty(),
        "a body carrying three null fields is not nothing"
    );

    let state = usage.apply_to(CapacityState::metered_balance(), OBSERVED);
    assert_eq!(state.credits().limit(), &Capacity::Inapplicable);
    assert_eq!(state.credits().remaining(), &Capacity::Inapplicable);
    assert_eq!(
        state.windows().calendar().resets_at_unix(),
        &Capacity::Inapplicable
    );
    // And the rolling window — what `RateLimitHeaders` fills — is
    // untouched: this endpoint's reset is an account-level one, not a
    // short rolling ceiling's.
    assert!(!state.windows().rolling().resets_at_unix().is_measured());
}

/// D3's other half: an endpoint an account never answered at all reads
/// as nothing, the same as `RateLimitHeaders` on a header-free response.
#[test]
fn a_body_with_no_data_object_reads_as_nothing() {
    for body in ["", "not json", "{}", r#"{"data": []}"#, r#"{"data": {}}"#] {
        let usage = ProviderUsage::read(body);
        assert!(usage.is_empty(), "`{body}` must not yield a reading");
        let state = usage.apply_to(CapacityState::metered_balance(), OBSERVED);
        assert!(!state.credits().limit().is_measured());
        assert_eq!(
            state.credits().limit(),
            CapacityState::metered_balance().credits().limit()
        );
    }
}

/// The numeric branch, exercised with a value shaped like the field's
/// documented type rather than a live observation — **no authenticated
/// account this project has read has ever answered a non-null `limit`**,
/// so this proves the parser's arithmetic, not a provider's behaviour.
#[test]
fn a_numeric_limit_becomes_a_measured_credits_ceiling() {
    let usage =
        ProviderUsage::read(r#"{"data": {"limit": 25, "limit_remaining": 10, "limit_reset": 30}}"#);
    let state = usage.apply_to(CapacityState::metered_balance(), OBSERVED);
    assert_eq!(
        state.credits().limit().value().map(NativeAmount::value),
        Some(25)
    );
    assert_eq!(
        state.credits().remaining().value().map(NativeAmount::value),
        Some(10)
    );
    assert_eq!(
        state.windows().calendar().resets_at_unix().value(),
        Some(&(OBSERVED + 30))
    );
    assert!(
        state
            .credits()
            .limit()
            .describe_source()
            .contains("GET /key")
    );
}

/// A subscription has no credit balance at all — `Pool::inapplicable`,
/// per `CapacityState::opaque_subscription`'s own documentation — and
/// `is_readable` refuses it exactly as it refuses a genuinely opaque
/// pool. This reader must respect that exactly as `RateLimitHeaders`
/// does.
#[test]
fn a_reader_cannot_fill_in_a_subscriptions_inapplicable_credits_pool() {
    let usage = ProviderUsage::read(r#"{"data": {"limit": 25, "limit_remaining": 10}}"#);
    let subscription = CapacityState::opaque_subscription();
    assert!(!subscription.credits().limit().is_readable());
    let after = usage.apply_to(subscription, OBSERVED);
    assert!(!after.credits().limit().is_measured());
    assert_eq!(after.credits().limit(), &Capacity::Inapplicable);
}

// --- line 1233: what the user can enter ------------------------------

#[test]
fn a_configured_budget_becomes_a_ceiling_with_the_spend_against_it_left_unknown() {
    let state = apply_user_configuration(
        CapacityState::metered_balance(),
        None,
        Some(10_000_000),
        None,
        OBSERVED,
    );
    let limit = state.user_budget().limit().value().expect("a ceiling");
    assert_eq!(limit.value(), 10_000_000);
    assert_eq!(limit.unit(), "USD");
    assert_eq!(limit.scale(), UnitScale::Millionths);
    assert_eq!(
        state.user_budget().limit().telemetry_class(),
        Some(TelemetryClass::Manual)
    );
    // This caller counted no spend, so the remaining half stays unknown
    // rather than being set equal to the ceiling.
    assert!(!state.user_budget().remaining().is_measured());
}

/// Map line 1519's own half of line 1233: a caller that *did* count
/// priced spend against the configured budget moves the remaining half
/// — the case the test above deliberately does not exercise.
#[test]
fn a_configured_budget_with_counted_spend_sets_the_remaining_half() {
    let spend = CredentialCost {
        micro_usd: Some(4_000_000),
        priced_rows: 3,
        unread_rows: 0,
        unpriced_rows: 0,
        account_narrowed: false,
    };
    let state = apply_user_configuration(
        CapacityState::metered_balance(),
        None,
        Some(10_000_000),
        Some(&spend),
        OBSERVED,
    );
    let remaining = state.user_budget().remaining().value().expect("a reading");
    assert_eq!(remaining.value(), 6_000_000);
    assert_eq!(remaining.unit(), "USD");
    assert_eq!(
        state.user_budget().remaining().telemetry_class(),
        Some(TelemetryClass::Observed)
    );
}

/// The spend counted may reach or exceed the budget — the remaining half
/// saturates at zero rather than going negative.
#[test]
fn spend_at_or_over_the_budget_saturates_remaining_at_zero() {
    let spend = CredentialCost {
        micro_usd: Some(15_000_000),
        priced_rows: 1,
        unread_rows: 0,
        unpriced_rows: 0,
        account_narrowed: false,
    };
    let state = apply_user_configuration(
        CapacityState::metered_balance(),
        None,
        Some(10_000_000),
        Some(&spend),
        OBSERVED,
    );
    let remaining = state.user_budget().remaining().value().expect("a reading");
    assert_eq!(remaining.value(), 0);
}

/// A budget with no priced row at all — `micro_usd: None` — is *unknown
/// spend*, never zero spend, so the remaining half stays exactly as
/// `None` left it: unmeasured.
#[test]
fn a_budget_nobody_could_price_leaves_remaining_unmeasured() {
    let spend = CredentialCost {
        micro_usd: None,
        priced_rows: 0,
        unread_rows: 2,
        unpriced_rows: 1,
        account_narrowed: false,
    };
    let state = apply_user_configuration(
        CapacityState::metered_balance(),
        None,
        Some(10_000_000),
        Some(&spend),
        OBSERVED,
    );
    assert!(!state.user_budget().remaining().is_measured());
}

// --- `budget_period_start` ---------------------------------------------

#[test]
fn rolling_thirty_days_is_exactly_thirty_days_of_seconds_back() {
    let start = budget_period_start(BudgetPeriod::RollingThirtyDays, OBSERVED);
    assert_eq!(OBSERVED - start, 30 * 24 * 60 * 60);
}

/// The machine's own configured zone decides the exact instant, so this
/// can only assert the invariants a calendar-month start must hold —
/// see [`budget_period_start`]'s own doc for why a fixed absolute
/// timestamp cannot be pinned here.
#[test]
fn calendar_month_start_is_the_first_of_the_month_at_local_midnight_at_or_before_now() {
    let now = OBSERVED;
    let start = budget_period_start(BudgetPeriod::CalendarMonth, now);
    assert!(
        start <= now,
        "the start of this month must not be in the future"
    );

    // Re-derive the local calendar day of `start` and assert it reads
    // the first of the month at midnight — the same primitive the
    // function under test used, applied to its own output, which is
    // the only zone-independent way to check this.
    // SAFETY: `time`/`broken_down` are local values this test alone
    // writes, and `localtime_r`/`localtime_s` take a valid pointer to
    // each, which these are.
    #[cfg(unix)]
    let broken_down = unsafe {
        let time = start as libc::time_t;
        let mut broken_down: libc::tm = std::mem::zeroed();
        assert!(!libc::localtime_r(&time, &mut broken_down).is_null());
        broken_down
    };
    #[cfg(windows)]
    let broken_down = unsafe {
        let time = start as libc::time_t;
        let mut broken_down: libc::tm = std::mem::zeroed();
        assert_eq!(libc::localtime_s(&mut broken_down, &time), 0);
        broken_down
    };
    #[cfg(any(unix, windows))]
    {
        assert_eq!(broken_down.tm_mday, 1);
        assert_eq!(broken_down.tm_hour, 0);
        assert_eq!(broken_down.tm_min, 0);
        assert_eq!(broken_down.tm_sec, 0);
    }

    // A second call one month's worth of seconds later must not answer
    // the same instant — the boundary actually moves.
    let later = budget_period_start(BudgetPeriod::CalendarMonth, now + 32 * 24 * 60 * 60);
    assert!(later > start);
}

/// Capability map line 1234, at the seam where it could actually go
/// wrong: a percentage over a user-configured ceiling is an estimate, and
/// there is no accessor that yields its digits without saying so.
#[test]
fn a_percentage_over_a_manually_configured_ceiling_is_never_exact() {
    let observed_remaining = Capacity::Measured(Reading::new(
        NativeAmount::millionths(2_500_000, "USD"),
        OBSERVED,
        ReadingSource::LocalObservation("this session's own spend".to_owned()),
    ));
    let state = apply_user_configuration(
        CapacityState::metered_balance(),
        None,
        Some(10_000_000),
        None,
        OBSERVED,
    );
    let pool = state
        .user_budget()
        .clone()
        .with_remaining(observed_remaining);
    let score = pool.normalized().expect("both halves were read");

    assert_eq!(score.percent().exact(), None);
    let percentage = score.percent();
    let (percent, confidence, source) = percentage.estimated().expect("this is an estimate");
    assert_eq!(percent, 25);
    assert_eq!(confidence, crate::provider::quota::Confidence::Medium);
    assert!(source.contains("configuration"));
    assert!(matches!(score.percent(), Percentage::Estimated { .. }));
    assert!(score.percent().render().starts_with('~'));
    assert!(score.percent().render().contains("estimated"));
}

// --- the security boundary -------------------------------------------

/// `design-decisions.md`: a provider's response may quote an account
/// identifier or a masked tail of the submitted credential, and must
/// never be copied whole into anything a user might share.
///
/// The values here are shaped like the real ones that rule was written
/// from. None of them may survive into any string this module produces.
#[test]
fn a_source_description_is_built_only_from_names_glasshouse_chose() {
    const ACCOUNT: &str = "account-8f21c0de-4b77-11ee-be56-0242ac120002";
    const MASKED_KEY: &str = "sk-or-v1-****************************9f3c";
    let headers = vec![
        ("ratelimit-limit", "300"),
        ("ratelimit-policy", "300;w=60"),
        ("x-account-id", ACCOUNT),
        (
            "set-cookie",
            "__cf_bm=oGkHQJmsGX6wCH7Quh5JYzAK6KXu1icwUg5MExQ2LqQ",
        ),
        ("x-key-tail", MASKED_KEY),
        (
            "access-control-expose-headers",
            "X-RateLimit-Limit,RateLimit-Remaining",
        ),
    ];

    // Nothing but the allowlisted names survives the funnel at all.
    let kept = retain_rate_limit_headers(headers.clone());
    let kept_names: Vec<&str> = kept.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(kept_names, vec!["ratelimit-limit", "ratelimit-policy"]);

    let state = RateLimitHeaders::read(headers).apply_to(
        ResourceKind::from_direct_provider("anyrouter").capacity(),
        OBSERVED,
    );

    let mut rendered = String::new();
    for (_, pool) in state.pools() {
        rendered.push_str(&pool.limit().describe_source());
        rendered.push_str(&pool.remaining().describe_source());
    }
    rendered.push_str(
        &state
            .rate_ceilings()
            .requests_per_minute()
            .describe_source(),
    );
    rendered.push_str(
        &state
            .rate_ceilings()
            .long_window_requests()
            .describe_source(),
    );
    rendered.push_str(&format!("{state:?}"));

    for forbidden in [
        ACCOUNT,
        MASKED_KEY,
        "__cf_bm",
        "oGkHQJmsGX",
        "X-RateLimit-Limit,",
    ] {
        assert!(
            !rendered.contains(forbidden),
            "`{forbidden}` reached a rendered surface"
        );
    }
    // And the thing it may say, it does say.
    assert!(rendered.contains("`ratelimit-limit` response header"));
}

/// The other half of the same rule, on the harness side: the status body
/// measured on 2026-08-27 carried three fields identifying the account
/// holder, and exactly one field about capacity.
#[test]
fn a_harness_report_carries_nothing_but_the_plan() {
    let body = r#"{
        "loggedIn": true,
        "authMethod": "claude.ai",
        "apiProvider": "firstParty",
        "analyticsDisabled": false,
        "email": "someone@example.com",
        "orgId": "5916b68d-0000-0000-0000-000000000000",
        "orgName": "someone@example.com's Organization",
        "subscriptionType": "max"
    }"#;
    let report = read_harness_plan(body, OBSERVED, "claude auth status --json");
    let state = apply_harness_report(CapacityState::opaque_subscription(), &report);

    assert_eq!(state.plan().value().unwrap().name(), "max");
    let rendered = format!("{report:?}{state:?}");
    for forbidden in [
        "someone@example.com",
        "5916b68d",
        "Organization",
        "firstParty",
        "loggedIn",
    ] {
        assert!(
            !rendered.contains(forbidden),
            "`{forbidden}` reached a rendered surface"
        );
    }
}

// --- the allowlist itself --------------------------------------------

#[test]
fn every_known_header_has_a_field_to_land_in() {
    for name in RATE_LIMIT_HEADERS {
        let read = RateLimitHeaders::read(vec![(
            *name,
            if *name == "ratelimit-policy" {
                "10;w=60"
            } else {
                "10"
            },
        )]);
        assert!(
            !read.is_empty(),
            "`{name}` is on the allowlist but nothing reads it"
        );
    }
}

#[test]
fn a_header_name_that_merely_contains_limit_is_not_a_rate_limit_header() {
    assert!(!is_rate_limit_header("access-control-expose-headers"));
    assert!(!is_rate_limit_header("x-ratelimit-tier"));
    assert!(!is_rate_limit_header("content-length"));
    assert!(is_rate_limit_header("RateLimit-Limit"));
    assert!(is_rate_limit_header("ratelimit-limit"));
}

// --- line 1211: a reset field -----------------------------------------

#[test]
fn a_reset_delta_and_a_reset_timestamp_both_become_the_same_unix_second() {
    let delta = RateLimitHeaders::read(vec![("ratelimit-reset", "30")]);
    assert_eq!(delta.resets_at_unix(OBSERVED), Some(OBSERVED + 30));

    let absolute =
        RateLimitHeaders::read(vec![("ratelimit-reset", &(OBSERVED + 30).to_string()[..])]);
    assert_eq!(absolute.resets_at_unix(OBSERVED), Some(OBSERVED + 30));
}

#[test]
fn a_reset_field_reaches_the_rolling_window_and_not_the_calendar_one() {
    let state = RateLimitHeaders::read(vec![("ratelimit-reset", "30")]).apply_to(
        ResourceKind::from_direct_provider("anyrouter").capacity(),
        OBSERVED,
    );
    assert_eq!(
        state.windows().rolling().resets_at_unix().value(),
        Some(&(OBSERVED + 30))
    );
    assert!(!state.windows().calendar().resets_at_unix().is_measured());
}

// --- Groq's real inference-response headers, capability map lines
// 1199, 1200, 1207, 1215, 1217 and 1218 -------------------------------

/// The exact header set `POST /chat/completions` answered with, against a
/// free model with `max_tokens: 1`, read 2026-08-26 and recorded in
/// `.agent-runtime/probe-quota-headers-2026-08-27.md`. Field for field,
/// not composed — the same discipline `anyrouter_models_headers` follows.
fn groq_inference_headers() -> Vec<(&'static str, &'static str)> {
    vec![
        ("x-ratelimit-limit-requests", "7000"),
        ("x-ratelimit-limit-tokens", "6000"),
        ("x-ratelimit-remaining-requests", "6999"),
        ("x-ratelimit-remaining-tokens", "5991"),
        ("x-ratelimit-reset-requests", "12.342s"),
        ("x-ratelimit-reset-tokens", "90ms"),
    ]
}

#[test]
fn groqs_named_headers_split_into_a_request_pool_and_a_token_pool() {
    let read = RateLimitHeaders::read(groq_inference_headers());
    // The `-requests` pair lands in the same fields AnyRouter's unsuffixed
    // spelling fills.
    assert_eq!(read.limit(), Some(7000));
    assert_eq!(read.remaining(), Some(6999));
    // The `-tokens` pair is a pool of its own.
    assert_eq!(read.token_limit(), Some(6000));
    assert_eq!(read.token_remaining(), Some(5991));
}

/// `"12.342s"` and `"90ms"` are not the bare integers every other host
/// measured here sends — this is the duration-suffixed shape only Groq
/// has been observed to use.
#[test]
fn a_duration_suffixed_reset_is_read_in_whole_seconds() {
    assert_eq!(parse_reset_seconds("12.342s"), Some(12));
    assert_eq!(parse_reset_seconds("90ms"), Some(0));
    assert_eq!(parse_reset_seconds("1500ms"), Some(2));
    // The plain-integer shape still works: this function replaces
    // `parse_count` for reset fields, not adds to it.
    assert_eq!(parse_reset_seconds("30"), Some(30));
    // Nonsense is nothing, not a panic and not a guess.
    for junk in ["", "s", "ms", "-3s", "abcs", "3.4.5s"] {
        assert_eq!(parse_reset_seconds(junk), None, "`{junk}`");
    }
}

#[test]
fn groqs_headers_reach_the_token_pool_as_a_reading_never_read_from_anywhere_else() {
    let state = RateLimitHeaders::read(groq_inference_headers()).apply_to(
        ResourceKind::from_direct_provider("groq").capacity(),
        OBSERVED,
    );
    let tokens = state.tokens().combined();
    assert_eq!(tokens.limit().value().map(NativeAmount::value), Some(6000));
    assert_eq!(
        tokens.limit().value().map(NativeAmount::unit),
        Some("tokens")
    );
    assert_eq!(
        tokens.remaining().value().map(NativeAmount::value),
        Some(5991)
    );
    assert!(
        tokens
            .limit()
            .describe_source()
            .contains("x-ratelimit-limit-tokens")
    );

    // And the request pool independently, from the `-requests` spelling.
    assert_eq!(
        state.requests().limit().value().map(NativeAmount::value),
        Some(7000)
    );
}

/// Capability map lines 1199 and 1200: a resource that has just
/// published both a request and a token ceiling is now evidenced to be
/// limited by both units at once, not only by the shape's default
/// (credits, for a metered account — Phase 32A's `metered_balance`).
#[test]
fn a_reading_of_both_pools_evidences_both_limiting_units_at_once() {
    let state = RateLimitHeaders::read(groq_inference_headers()).apply_to(
        ResourceKind::from_direct_provider("groq").capacity(),
        OBSERVED,
    );
    assert!(state.limiting_units().includes(LimitingUnit::Requests));
    assert!(state.limiting_units().includes(LimitingUnit::Tokens));
    // The shape's own default is not lost — a metered account is still
    // credit-limited even once its request and token pools are read.
    assert!(state.limiting_units().includes(LimitingUnit::Credits));
}

/// A resource this reader is not allowed to fill in stays that way: local
/// inference cannot receive headers in the first place, but the guard is
/// asserted directly against `LimitingUnits::None` and `::Delegated`
/// rather than relied on implicitly.
#[test]
fn evidencing_a_unit_is_a_no_op_for_none_and_delegated() {
    use crate::provider::quota::LimitingUnits;
    assert_eq!(
        LimitingUnits::None.with_evidenced(LimitingUnit::Tokens),
        LimitingUnits::None
    );
    assert_eq!(
        LimitingUnits::Delegated.with_evidenced(LimitingUnit::Requests),
        LimitingUnits::Delegated
    );
}

/// Capability map lines 1217 and 1218, at the point this package can
/// actually reach: Groq's headers are the first and only seam observed
/// anywhere that gives both halves of a pool in one unit, so
/// `Pool::normalized` produces a real `Percentage::Exact` from them — the
/// structural guarantee Phase 32A built, exercised for the first time by
/// a live reading rather than by hand-built test data.
///
/// **This still does not close either line, and the reason has moved
/// again.** BRIDGE-QUOTA built the persisted cache
/// (`GatewayQuotaCache`, below) and the gateway-side write into it
/// (`crate::gateway::Gateway::start_with_quota_cache`), so a reading
/// this shaped can now survive the process boundary between the gateway
/// and a `glasshouse resources` invocation — proven end to end at
/// `resources::tests::a_persisted_gateway_reading_reaches_the_rendered_report`.
/// What still does not exist is the one line in `main.rs` that would
/// call either new entry point from the shipped binary; see this
/// package's report for exactly which line, at which of two call sites.
/// Recorded here as proof the model is ready the day that caller exists,
/// per practice §36: a reading arriving is not the same question as
/// something asking for a percentage.
#[test]
fn groqs_reading_produces_a_real_exact_percentage_from_the_model_alone() {
    let state = RateLimitHeaders::read(groq_inference_headers()).apply_to(
        ResourceKind::from_direct_provider("groq").capacity(),
        OBSERVED,
    );

    let requests_score = state
        .requests()
        .normalized()
        .expect("both halves of the request pool were read");
    assert_eq!(requests_score.percent().exact(), Some(99));

    let tokens_score = state
        .tokens()
        .combined()
        .normalized()
        .expect("both halves of the token pool were read");
    assert_eq!(tokens_score.percent().exact(), Some(99));
    assert!(!tokens_score.percent().render().contains("estimated"));
}

// --- GatewayQuotaCache: a reading surviving its own process ------------

#[test]
fn a_stored_reading_comes_back_with_every_field_and_its_timestamp() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let cache = GatewayQuotaCache::at(dir.path());
    let written = RateLimitHeaders::read(groq_inference_headers());
    cache.store("groq", &written, OBSERVED);

    let (read, observed_at_unix) = cache.load("groq").expect("the reading is cached");
    assert_eq!(read.limit(), Some(7000));
    assert_eq!(read.remaining(), Some(6999));
    assert_eq!(read.token_limit(), Some(6000));
    assert_eq!(read.token_remaining(), Some(5991));
    assert_eq!(observed_at_unix, OBSERVED);
    // The round trip is exact enough to reproduce the same real
    // percentage the model-level test above computes directly — proof
    // that persisting and reading back changes nothing about what the
    // reading means.
    let state = read.apply_to(
        ResourceKind::from_direct_provider("groq").capacity(),
        OBSERVED,
    );
    assert_eq!(
        state
            .requests()
            .normalized()
            .and_then(|s| s.percent().exact()),
        Some(99)
    );
}

#[test]
fn a_provider_with_no_persisted_reading_is_a_miss_rather_than_an_error() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    assert!(
        GatewayQuotaCache::at(dir.path())
            .load("never-forwarded")
            .is_none()
    );
}

#[test]
fn storing_again_replaces_the_previous_reading_for_the_same_provider() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let cache = GatewayQuotaCache::at(dir.path());
    cache.store(
        "anyrouter",
        &RateLimitHeaders::read(vec![("ratelimit-limit", "300")]),
        OBSERVED,
    );
    cache.store(
        "anyrouter",
        &RateLimitHeaders::read(vec![("ratelimit-limit", "150")]),
        OBSERVED + 60,
    );
    let (read, observed_at_unix) = cache.load("anyrouter").expect("cached");
    assert_eq!(read.limit(), Some(150), "the newer reading must win");
    assert_eq!(observed_at_unix, OBSERVED + 60);
}

#[test]
fn an_empty_reading_is_never_written_at_all() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let cache = GatewayQuotaCache::at(dir.path());
    cache.store("groq", &RateLimitHeaders::read(Vec::new()), OBSERVED);
    assert!(
        !dir.path().exists() || std::fs::read_dir(dir.path()).unwrap().next().is_none(),
        "an exchange that carried no rate-limit header must not create a cache file"
    );
}

#[test]
fn a_reading_already_on_disk_is_not_erased_by_a_later_empty_one() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let cache = GatewayQuotaCache::at(dir.path());
    cache.store(
        "groq",
        &RateLimitHeaders::read(vec![("x-ratelimit-limit-requests", "7000")]),
        OBSERVED,
    );
    cache.store("groq", &RateLimitHeaders::read(Vec::new()), OBSERVED + 60);
    assert_eq!(
        cache.load("groq").and_then(|(h, _)| h.limit()),
        Some(7000),
        "an empty reading must not overwrite a real one on disk, mirroring \
         SessionRouting::observe_quota_headers's own in-memory guard"
    );
}

#[test]
fn load_all_finds_every_provider_a_gateway_has_ever_written_for() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let cache = GatewayQuotaCache::at(dir.path());
    cache.store(
        "groq",
        &RateLimitHeaders::read(groq_inference_headers()),
        OBSERVED,
    );
    cache.store(
        "anyrouter",
        &RateLimitHeaders::read(vec![("ratelimit-limit", "300")]),
        OBSERVED + 10,
    );
    let mut found: Vec<String> = cache
        .load_all()
        .into_iter()
        .map(|(provider, _, _)| provider)
        .collect();
    found.sort();
    assert_eq!(found, vec!["anyrouter".to_owned(), "groq".to_owned()]);
}

#[test]
fn a_provider_name_that_looks_like_a_path_cannot_escape_the_cache_directory() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let cache = GatewayQuotaCache::at(dir.path());
    cache.store(
        "../../.ssh/authorized_keys",
        &RateLimitHeaders::read(vec![("ratelimit-limit", "1")]),
        OBSERVED,
    );
    assert!(
        !dir.path().parent().unwrap().join(".ssh").exists(),
        "a hostile provider name must never steer a write outside the cache directory"
    );
    assert_eq!(
        cache
            .load("../../.ssh/authorized_keys")
            .and_then(|(h, _)| h.limit()),
        Some(1),
        "the same hostile name must still round-trip through its own digested file"
    );
}

/// A cache file for one provider must never answer another provider's
/// query, even if the file were somehow moved or hand-edited to a
/// different provider name inside — [`ModelCache::load`]'s own guard,
/// mirrored here.
#[test]
fn a_reading_stored_for_one_provider_is_never_returned_for_another() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let cache = GatewayQuotaCache::at(dir.path());
    cache.store(
        "groq",
        &RateLimitHeaders::read(groq_inference_headers()),
        OBSERVED,
    );
    assert!(cache.load("anyrouter").is_none());
}

/// design-decisions.md's own rule, checked against the bytes actually
/// written: header *values* Groq or AnyRouter sent become parsed
/// integers or vanish, and only names Glasshouse chose from
/// [`RATE_LIMIT_HEADERS`] ever reach the file — mirroring
/// `discovery::tests::nothing_but_an_allowlisted_header_survives_the_capture`
/// at the point this reading is written to disk rather than read off the
/// wire.
#[test]
fn nothing_but_an_allowlisted_header_name_survives_into_the_persisted_file() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let cache = GatewayQuotaCache::at(dir.path());
    cache.store(
        "groq",
        &RateLimitHeaders::read(vec![
            ("x-ratelimit-limit-requests", "7000"),
            ("set-cookie", "__cf_bm=a-planted-session-cookie-value"),
            ("authorization", "Bearer sk-planted-provider-credential"),
        ]),
        OBSERVED,
    );
    let bytes = std::fs::read(cache.path_for("groq")).expect("the file was written");
    let text = String::from_utf8(bytes).expect("the cache file is UTF-8 JSON");
    assert!(!text.contains("cf_bm"));
    assert!(!text.contains("planted-session-cookie"));
    assert!(!text.contains("planted-provider-credential"));
    assert!(!text.contains("authorization"));
    assert!(text.contains("x-ratelimit-limit-requests"));
}

/// A file written by a future format version is a miss, not a misread —
/// [`ModelCache::load`]'s own contract, mirrored here.
#[test]
fn a_future_format_version_is_ignored_rather_than_misread() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let cache = GatewayQuotaCache::at(dir.path());
    cache.store(
        "groq",
        &RateLimitHeaders::read(groq_inference_headers()),
        OBSERVED,
    );
    // Overwrite with a hand-bumped version, the same way a future build
    // that changed the shape would leave one behind for this build.
    // `serde_json::Value` rather than a string replace, so this does not
    // depend on `to_vec_pretty`'s exact spacing.
    let path = cache.path_for("groq");
    let bytes = std::fs::read(&path).expect("written above");
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).expect("valid JSON");
    value["version"] = serde_json::json!(99);
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).expect("overwritten");
    assert!(cache.load("groq").is_none());
}

/// A corrupted or partially written file is a miss, not a panic — the
/// same crash-mid-write case
/// `crate::provider::cache::ModelCache::store`'s own doc names, proven
/// here at the read end.
#[test]
fn a_truncated_file_is_a_miss_rather_than_a_panic() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let cache = GatewayQuotaCache::at(dir.path());
    std::fs::create_dir_all(dir.path()).unwrap();
    std::fs::write(cache.path_for("groq"), b"{\"version\":1,\"provider\":\"gr")
        .expect("a deliberately truncated file");
    assert!(cache.load("groq").is_none());
}

/// [`RateLimitHeaders::from_persisted`]'s own refusal: a name in a
/// hand-edited file that is not on [`RATE_LIMIT_HEADERS`] must not
/// survive into `read_from`, the same way a header off the wire that is
/// not on the allowlist never does.
#[test]
fn a_hand_edited_read_from_name_off_the_allowlist_is_dropped_on_load() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let cache = GatewayQuotaCache::at(dir.path());
    cache.store(
        "groq",
        &RateLimitHeaders::read(vec![("x-ratelimit-limit-requests", "7000")]),
        OBSERVED,
    );
    let path = cache.path_for("groq");
    let text = std::fs::read_to_string(&path).unwrap().replacen(
        "\"x-ratelimit-limit-requests\"",
        "\"x-a-name-nobody-chose\"",
        1,
    );
    std::fs::write(&path, text).unwrap();

    let (read, _) = cache
        .load("groq")
        .expect("the rest of the file is still valid");
    assert_eq!(
        read.read_from(),
        &[] as &[&str],
        "a name off the allowlist must not reach read_from even once the number beside it did"
    );
}
