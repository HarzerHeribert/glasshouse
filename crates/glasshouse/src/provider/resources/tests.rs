use super::*;
use crate::config::{
    BudgetPeriod, MonetaryBudget, PremiumReservePercent, ProjectConfig, ProviderConfig,
    QuotaOverride, UserConfig,
};
use crate::provider::quota::{Capacity, NativeAmount, RateCeilings, Reading, ReadingSource};
use crate::provider::telemetry::{GatewayQuotaCache, RateLimitHeaders};

const OBSERVED: i64 = 1_787_800_000;
const NOW: i64 = OBSERVED + 30;

/// The header set `https://anyrouter.dev/api/v1/models` really answered
/// with on 2026-08-27 — see `provider::telemetry`'s own fixture note.
fn anyrouter_headers() -> RateLimitHeaders {
    RateLimitHeaders::read(vec![
        ("ratelimit-limit", "300"),
        ("ratelimit-policy", "300;w=60"),
        ("x-ratelimit-limit", "300"),
        ("x-ratelimit-tier", "ip"),
        ("x-ratelimit-window", "60"),
    ])
}

fn user_with_anyrouter(quota: QuotaOverride) -> UserConfig {
    let mut user = UserConfig::default();
    let mut provider = ProviderConfig::new("anyrouter");
    provider.set_quota(Some(quota));
    user.providers_mut().set("anyrouter", provider);
    user
}

fn options() -> ReportOptions {
    ReportOptions {
        verbose: false,
        now_unix: NOW,
    }
}

fn anyrouter_row(rendered: &str) -> String {
    rendered
        .split("\n\n")
        .find(|block| block.starts_with("anyrouter"))
        .unwrap_or_else(|| panic!("no anyrouter block in:\n{rendered}"))
        .to_owned()
}

// --- line 1229, through the function `main.rs` calls -------------------

#[test]
fn a_providers_own_rate_limit_header_reaches_the_report_as_authoritative() {
    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);
    let telemetry =
        GatheredTelemetry::new().with_provider_headers("anyrouter", anyrouter_headers(), OBSERVED);

    let rendered = report(&effective, &telemetry, options());
    let row = anyrouter_row(&rendered);

    assert!(row.contains("300 requests"), "{row}");
    assert!(row.contains("authoritative"), "{row}");
    assert!(row.contains("`ratelimit-limit` response header"), "{row}");
    // Line 1236: the observation is dated.
    assert!(row.contains(&format!("unix {OBSERVED}")), "{row}");
}

/// Capability map line 1200, at the report level: AnyRouter's own real
/// header — measured, not invented — evidences that this resource really
/// is request-limited, and `describe_limits`' "limited by" line now says
/// so instead of only naming the shape's default (credits).
#[test]
fn a_request_ceiling_a_reader_measured_reaches_the_limited_by_line() {
    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);
    let before = anyrouter_row(&report(&effective, &GatheredTelemetry::new(), options()));
    assert_eq!(before.lines().nth(3), Some("  limited by      credits"));

    let telemetry =
        GatheredTelemetry::new().with_provider_headers("anyrouter", anyrouter_headers(), OBSERVED);
    let after = anyrouter_row(&report(&effective, &telemetry, options()));
    assert_eq!(
        after.lines().nth(3),
        Some("  limited by      requests, credits")
    );
}

/// The half of line 1229 that is about restraint. AnyRouter advertises
/// `RateLimit-Remaining` and does not send it; a report that showed a
/// remaining count anyway would be inventing one.
#[test]
fn a_count_the_provider_did_not_send_stays_unknown_in_the_report() {
    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);
    let telemetry =
        GatheredTelemetry::new().with_provider_headers("anyrouter", anyrouter_headers(), OBSERVED);
    let row = anyrouter_row(&report(&effective, &telemetry, options()));

    assert!(row.contains("remaining unmeasured (unknown)"), "{row}");
    // And with no both-halves reading there is no percentage at all.
    assert!(row.contains("capacity        unknown"), "{row}");
}

/// Capability map lines 1210 and 1211: the caller `render_windows` gives
/// them. A reset field reads into `windows().rolling()` — proven at the
/// model level by `provider::telemetry`'s own tests — and this is the
/// production rendering path finally reading that field back out, which
/// nothing did before this package.
#[test]
fn a_window_reset_a_reader_supplied_reaches_the_report() {
    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);
    let headers = RateLimitHeaders::read(vec![("ratelimit-reset", "30")]);
    let telemetry = GatheredTelemetry::new().with_provider_headers("anyrouter", headers, OBSERVED);
    let row = anyrouter_row(&report(&effective, &telemetry, options()));

    assert!(row.contains("rolling window"), "{row}");
    assert!(
        row.contains(&format!("resets unix {}", OBSERVED + 30)),
        "{row}"
    );
    // No window *start* has ever been observed anywhere — line 1210
    // stays open on its own honest terms, and the report says so rather
    // than inventing one.
    assert!(row.contains("starts unmeasured (unknown)"), "{row}");
}

// --- BRIDGE-QUOTA: a gateway-captured reading, folded in ---------------

/// Capability map lines 1217/1218's antecedent, reached at the actual
/// rendering function `main.rs::resources_report` calls, for the first
/// time in this phase's history.
///
/// **The numbers are synthetic and say so.** No live host `--probe` can
/// reach has ever sent both halves of a pool in one unit — AnyRouter's
/// own real header carries a limit and no remaining count at all (see
/// `anyrouter_headers`, above); Groq's real inference response does
/// carry both, proven at the model level by `provider::telemetry`'s
/// `groqs_reading_produces_a_real_exact_percentage_from_the_model_alone`,
/// but Groq is not a provider this build ships a registry template for,
/// so it cannot appear in `report`'s own registry loop. This test uses
/// AnyRouter's real provider slug with an invented remaining count, to
/// isolate what this package can actually prove: that a reading reaching
/// [`GatewayQuotaCache`] reaches this unmodified rendering function and
/// produces a real, correctly-labelled percentage — not that a live host
/// has ever supplied one over this seam.
///
/// This is [`GatheredTelemetry::gather_gateway_quota`]'s only production-
/// shaped proof: nothing in the shipped binary calls it yet (see the
/// package report for the one line `main.rs::resources_report` needs),
/// so this cannot claim §35's production-caller mutation the way
/// `a_providers_own_rate_limit_header_reaches_the_report_as_authoritative`
/// can for `--probe`. What it proves is narrower and still real: *if*
/// that line existed, this is exactly what a user would see.
#[test]
fn a_persisted_gateway_reading_reaches_the_rendered_report() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let cache = GatewayQuotaCache::at(dir.path());
    cache.store(
        "anyrouter",
        &RateLimitHeaders::read(vec![
            ("ratelimit-limit", "300"),
            ("ratelimit-remaining", "297"),
        ]),
        OBSERVED,
    );

    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);
    let telemetry = GatheredTelemetry::new().gather_gateway_quota(&cache);
    let row = anyrouter_row(&report(&effective, &telemetry, options()));

    assert!(
        row.contains("capacity        99%"),
        "a reading with both halves must render a real, unlabelled-as-estimate \
         percentage: {row}"
    );
    assert!(
        !row.contains("capacity        unknown"),
        "the gathered reading must have reached the report: {row}"
    );
}

// --- Phase 32D/32F, through the same production rendering function ----

/// Capability map lines 1259-1268, at the actual rendering function
/// `main.rs::resources_report` calls: a real gateway-captured reading
/// produces a real band line, not just a percentage.
#[test]
fn a_persisted_gateway_reading_reaches_the_rendered_band_line() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let cache = GatewayQuotaCache::at(dir.path());
    cache.store(
        "anyrouter",
        &RateLimitHeaders::read(vec![
            ("ratelimit-limit", "300"),
            ("ratelimit-remaining", "297"),
        ]),
        OBSERVED,
    );

    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);
    let telemetry = GatheredTelemetry::new().gather_gateway_quota(&cache);
    let row = anyrouter_row(&report(&effective, &telemetry, options()));

    assert!(
        row.contains("band            plenty"),
        "297/300 = 99% must fall in the default Plenty band: {row}"
    );
    assert!(row.contains("bound by requests"), "{row}");
}

/// §35: mutate the call, not the callee. Deleting `render_capacity_band`'s
/// call inside `render_resource` must make a named test go red — proven
/// here by asserting the line's own presence, so the mutation described
/// in the package report (removing that one call) fails exactly this
/// test rather than only a lower-level unit test that enters below the
/// production rendering path.
#[test]
fn the_band_line_is_present_for_every_registry_resource_even_with_no_score() {
    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);
    let rendered = report(&effective, &GatheredTelemetry::new(), options());
    let resource_blocks = rendered.split("\n\n").filter(|block| {
        block
            .lines()
            .any(|line| line.trim_start().starts_with("quota shape"))
    });
    let mut checked = 0usize;
    for block in resource_blocks {
        assert!(
            block
                .lines()
                .any(|line| line.trim_start().starts_with("band")),
            "every resource block must print a band line, even `unknown`: {block}"
        );
        checked += 1;
    }
    assert!(
        checked > 0,
        "expected at least one resource block in:\n{rendered}"
    );
}

/// Capability map line 1288: a provider's own protected reserve
/// percentage — configured, not hardcoded — narrows where the Reserve
/// band begins for that resource specifically, reached through the same
/// `EffectiveConfig` every other quota field in this file already goes
/// through.
#[test]
fn a_providers_own_reserve_percentage_narrows_its_reserve_band() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let cache = GatewayQuotaCache::at(dir.path());
    // 40% remaining: Plenty by the global defaults (>= 70% is Plenty,
    // but 40% falls in Healthy — see CapacityBandThresholds::DEFAULT),
    // and Reserve once this provider protects 50% of its own capacity.
    cache.store(
        "anyrouter",
        &RateLimitHeaders::read(vec![
            ("ratelimit-limit", "100"),
            ("ratelimit-remaining", "40"),
        ]),
        OBSERVED,
    );
    let telemetry = GatheredTelemetry::new().gather_gateway_quota(&cache);

    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);
    let without_override = anyrouter_row(&report(&effective, &telemetry, options()));
    assert!(
        without_override.contains("band            healthy"),
        "{without_override}"
    );

    let mut quota = QuotaOverride::default();
    quota.set_reserve_percent(Some(PremiumReservePercent::try_from(50u16).unwrap()));
    let user = user_with_anyrouter(quota);
    let effective = EffectiveConfig::new(&user, None);
    let with_override = anyrouter_row(&report(&effective, &telemetry, options()));
    assert!(
        with_override.contains("band            reserve"),
        "a 50% protected reserve must move a 40%-remaining resource into Reserve: \
         {with_override}"
    );
}

/// The negative half: a cache with nothing in it changes nothing, so
/// `gather_gateway_quota` is safe to fold in unconditionally rather than
/// only when a gateway happens to have run.
#[test]
fn an_empty_gateway_quota_cache_leaves_the_report_exactly_as_before() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let cache = GatewayQuotaCache::at(dir.path());

    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);
    let without = report(&effective, &GatheredTelemetry::new(), options());
    let with = report(
        &effective,
        &GatheredTelemetry::new().gather_gateway_quota(&cache),
        options(),
    );
    assert_eq!(without, with);
}

// --- capability map lines 1311/1321/1322/1324: resource health --------

fn health_reading(
    model: &str,
    consecutive_failures: u32,
    cooling_down_until_unix: Option<i64>,
    credential_rejected: bool,
) -> crate::provider::telemetry::GatewayHealthReading {
    crate::provider::telemetry::GatewayHealthReading {
        credential_label: "anyrouter/ANYROUTER_API_KEY".to_owned(),
        model: model.to_owned(),
        consecutive_failures,
        cooling_down_until_unix,
        cooldown_cause: None,
        credential_rejected,
    }
}

/// Capability map line 1324's "never invent a reading" half: a resource
/// nothing has been observed about reports `unknown`, never a number and
/// never a fabricated "available".
#[test]
fn a_resource_with_no_health_observation_reports_unknown() {
    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);
    let rendered = report(&effective, &GatheredTelemetry::new(), options());
    let row = anyrouter_row(&rendered);
    assert!(
        row.contains(&format!("health          {UNKNOWN_TELEMETRY}")),
        "{row}"
    );
    assert!(!row.contains("paced"), "{row}");
    assert!(!row.contains("consecutive failure"), "{row}");
}

/// Capability map line 1324's own point: a resource cooling down after
/// real failures is **paced**, not broken — the property a test that
/// cannot tell "unknown" from "cooling" from "available" would miss
/// entirely.
#[test]
fn a_cooling_down_resource_is_shown_as_paced_not_broken() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let cache = GatewayHealthCache::at(dir.path());
    cache.store(
        "anyrouter",
        &[health_reading(
            "anyrouter/free-model",
            3,
            Some(NOW + 120),
            false,
        )],
        OBSERVED,
    );
    let telemetry = GatheredTelemetry::new().gather_gateway_health(&cache);

    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);
    let row = anyrouter_row(&report(&effective, &telemetry, options()));

    assert!(
        row.contains("health          anyrouter/free-model")
            && row.contains("paced, cooling down until unix"),
        "a resource still cooling down must render as paced: {row}"
    );
    assert!(
        row.contains("3 consecutive failure(s)"),
        "the observed failure count must reach the report: {row}"
    );
    assert!(
        !row.contains(&format!("health          {UNKNOWN_TELEMETRY}")),
        "a real observation must not render as unknown: {row}"
    );
    assert!(
        !row.contains("credential rejected"),
        "a cooldown is not a credential rejection: {row}"
    );
}

/// The other side of the same property: once a cooldown has elapsed by
/// the report's own `now_unix`, the resource reads as available again —
/// without a fresh observation, exactly
/// [`crate::provider::telemetry::GatewayHealthReading::is_available`]'s
/// own contract.
#[test]
fn an_elapsed_cooldown_reads_as_available_again() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let cache = GatewayHealthCache::at(dir.path());
    cache.store(
        "anyrouter",
        &[health_reading(
            "anyrouter/free-model",
            2,
            Some(NOW - 1),
            false,
        )],
        OBSERVED,
    );
    let telemetry = GatheredTelemetry::new().gather_gateway_health(&cache);

    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);
    let row = anyrouter_row(&report(&effective, &telemetry, options()));

    assert!(
        row.contains(
            "health          anyrouter/free-model (anyrouter/ANYROUTER_API_KEY): \
                      available"
        ),
        "{row}"
    );
    assert!(!row.contains("paced"), "{row}");
}

/// A credential Glasshouse's own gateway had to reject reports that fact
/// distinctly from a cooldown — waiting does not fix a revoked key.
#[test]
fn a_rejected_credential_is_shown_as_rejected_not_paced() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let cache = GatewayHealthCache::at(dir.path());
    cache.store(
        "anyrouter",
        &[health_reading("anyrouter/free-model", 1, None, true)],
        OBSERVED,
    );
    let telemetry = GatheredTelemetry::new().gather_gateway_health(&cache);

    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);
    let row = anyrouter_row(&report(&effective, &telemetry, options()));

    assert!(row.contains("credential rejected"), "{row}");
    assert!(!row.contains("paced"), "{row}");
}

/// A corrupt cache file must leave `glasshouse resources` working and
/// simply carry no health — [`GatewayHealthCache::load`]'s own fail-soft
/// contract, proven at the rendering function it feeds.
#[test]
fn a_corrupt_health_cache_file_leaves_the_report_working_with_no_health() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let cache = GatewayHealthCache::at(dir.path());
    cache.store(
        "anyrouter",
        &[health_reading("anyrouter/free-model", 1, None, false)],
        OBSERVED,
    );
    let path = dir.path().join(format!(
        "{}.json",
        crate::provider::cache::file_stem("anyrouter")
    ));
    std::fs::write(&path, b"not json").expect("overwritten with garbage");

    let telemetry = GatheredTelemetry::new().gather_gateway_health(&cache);
    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);
    let row = anyrouter_row(&report(&effective, &telemetry, options()));

    assert!(
        row.contains(&format!("health          {UNKNOWN_TELEMETRY}")),
        "a corrupt cache file must read as no health, not fail the report: {row}"
    );
}

/// [`capacity_json`]'s own twin: health rides beside capacity as a
/// separate field, never merged into it, and an unobserved resource's
/// list is empty rather than absent or fabricated.
#[test]
fn capacity_json_carries_health_separately_from_capacity() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let cache = GatewayHealthCache::at(dir.path());
    cache.store(
        "anyrouter",
        &[health_reading(
            "anyrouter/free-model",
            2,
            Some(NOW + 60),
            false,
        )],
        OBSERVED,
    );
    let telemetry = GatheredTelemetry::new().gather_gateway_health(&cache);

    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);
    let json = capacity_json(&effective, &telemetry, NOW);
    let anyrouter = json["resources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["resource"].as_str().unwrap().starts_with("anyrouter"))
        .expect("an anyrouter entry");

    let health = anyrouter["health"].as_array().expect("a health array");
    assert_eq!(health.len(), 1);
    assert_eq!(health[0]["model"], "anyrouter/free-model");
    assert_eq!(health[0]["consecutive_failures"], 2);
    assert_eq!(health[0]["available"], false);

    let gateway_kind = json["resources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["resource"] == "glasshouse gateway")
        .expect("the glasshouse gateway entry");
    assert_eq!(
        gateway_kind["health"].as_array().expect("a health array"),
        &Vec::<serde_json::Value>::new(),
        "a resource kind health cannot apply to must carry an empty list, never null"
    );
}

/// An explicit `--probe`'s own reading, folded in after
/// `gather_gateway_quota`, still wins — a live probe the user just ran is
/// never staled out by whatever a gateway happened to persist earlier,
/// mirroring `Capacity::prefer`'s own freshness rule at the seam these
/// two sources actually meet in `main.rs::resources_report`.
#[test]
fn an_explicit_probe_reading_overrides_a_persisted_gateway_one_for_the_same_provider() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let cache = GatewayQuotaCache::at(dir.path());
    cache.store(
        "anyrouter",
        &RateLimitHeaders::read(vec![("ratelimit-limit", "300")]),
        OBSERVED,
    );

    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);
    let telemetry = GatheredTelemetry::new()
        .gather_gateway_quota(&cache)
        .with_provider_headers(
            "anyrouter",
            RateLimitHeaders::read(vec![("ratelimit-limit", "150")]),
            OBSERVED + 60,
        );
    let row = anyrouter_row(&report(&effective, &telemetry, options()));
    assert!(row.contains("150 requests"), "{row}");
    assert!(!row.contains("300 requests"), "{row}");
}

/// The quiet view's own rule, extended to windows: nothing measured means
/// nothing shown, and `--verbose` is what surfaces the unmeasured rows.
#[test]
fn a_resource_with_no_window_reading_shows_no_window_row_unless_verbose() {
    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);
    let quiet = anyrouter_row(&report(&effective, &GatheredTelemetry::new(), options()));
    assert!(!quiet.contains("rolling window"), "{quiet}");
    assert!(!quiet.contains("calendar window"), "{quiet}");

    let verbose = anyrouter_row(&report(
        &effective,
        &GatheredTelemetry::new(),
        ReportOptions {
            verbose: true,
            now_unix: NOW,
        },
    ));
    assert!(verbose.contains("rolling window"), "{verbose}");
    assert!(verbose.contains("calendar window"), "{verbose}");
}

// --- lines 1233 and 1203: what the user entered ------------------------

#[test]
fn a_configured_plan_and_budget_reach_the_report_marked_manual() {
    let mut quota = QuotaOverride::default();
    quota.set_plan(Some("free-tier".to_owned()));
    quota.set_budget(Some(
        MonetaryBudget::new(10_000_000, BudgetPeriod::CalendarMonth).unwrap(),
    ));
    let user = user_with_anyrouter(quota);
    let effective = EffectiveConfig::new(&user, None);
    let rendered = report(&effective, &GatheredTelemetry::new(), options());

    let row = anyrouter_row(&rendered);
    assert!(row.contains("free-tier [manual]"), "{row}");
    assert!(row.contains("the user's own configuration"), "{row}");

    // And the note that tells a user this is a thing they can do.
    assert!(
        rendered.contains("anyrouter: plan `free-tier` (user)"),
        "{rendered}"
    );
    assert!(
        rendered.contains("10.000000 USD per calendar month"),
        "{rendered}"
    );
    // Stated rather than implied: the ceiling is known and nothing was
    // gathered to count spend against it, so the report says so with the
    // breakdown rather than implying a balance.
    assert!(
        rendered.contains("spend not counted (0 exchanges: 0 unread, 0 unpriced)"),
        "{rendered}"
    );
}

#[test]
fn a_project_layer_quota_table_replaces_the_users_and_says_which_layer_won() {
    let mut user_quota = QuotaOverride::default();
    user_quota.set_plan(Some("user-plan".to_owned()));
    let user = user_with_anyrouter(user_quota);

    let mut project = ProjectConfig::default();
    let mut project_provider = ProviderConfig::new("anyrouter");
    let mut project_quota = QuotaOverride::default();
    project_quota.set_plan(Some("project-plan".to_owned()));
    project_provider.set_quota(Some(project_quota));
    project.providers_mut().set("anyrouter", project_provider);

    let effective = EffectiveConfig::new(&user, Some(&project));
    let rendered = report(&effective, &GatheredTelemetry::new(), options());
    assert!(
        rendered.contains("anyrouter: plan `project-plan` (project)"),
        "{rendered}"
    );
    assert!(!rendered.contains("user-plan"), "{rendered}");
}

// --- line 1237: provider-specific staleness ---------------------------

/// Two providers, two configured ages, one reading each of the same age:
/// one is stale and one is not. A single global age could not produce
/// this, which is what makes it a test of "provider-specific" rather
/// than of staleness in general.
#[test]
fn two_providers_with_different_configured_ages_disagree_about_the_same_reading() {
    let mut user = UserConfig::default();
    for (name, seconds) in [("anyrouter", 10_u32), ("openrouter", 3_600)] {
        let mut provider = ProviderConfig::new(name);
        let mut quota = QuotaOverride::default();
        quota.set_stale_after(Some(seconds.try_into().unwrap()));
        provider.set_quota(Some(quota));
        user.providers_mut().set(name, provider);
    }
    let effective = EffectiveConfig::new(&user, None);
    let telemetry = GatheredTelemetry::new()
        .with_provider_headers("anyrouter", anyrouter_headers(), OBSERVED)
        .with_provider_headers("openrouter", anyrouter_headers(), OBSERVED);

    let rendered = report(&effective, &telemetry, options());
    let anyrouter = anyrouter_row(&rendered);
    let openrouter = rendered
        .split("\n\n")
        .find(|block| block.starts_with("openrouter"))
        .expect("an openrouter block")
        .to_owned();

    // 30 seconds old, against a ten-second limit and an hour's.
    assert!(anyrouter.contains("stale:"), "{anyrouter}");
    assert!(!openrouter.contains("stale:"), "{openrouter}");
}

#[test]
fn a_stale_reading_is_still_reported_rather_than_discarded() {
    let mut user = UserConfig::default();
    let mut provider = ProviderConfig::new("anyrouter");
    let mut quota = QuotaOverride::default();
    quota.set_stale_after(Some(10_u32.try_into().unwrap()));
    provider.set_quota(Some(quota));
    user.providers_mut().set("anyrouter", provider);

    let effective = EffectiveConfig::new(&user, None);
    let telemetry =
        GatheredTelemetry::new().with_provider_headers("anyrouter", anyrouter_headers(), OBSERVED);
    let state = observed_capacity(
        &ResourceKind::from_direct_provider("anyrouter"),
        &effective,
        &telemetry,
        NOW,
    );

    assert!(is_entirely_stale(
        &state,
        NOW,
        effective.quota_stale_after("anyrouter").value
    ));
    // Line 1238: stale is not gone. The number is still there.
    assert_eq!(
        state.requests().limit().value().map(NativeAmount::value),
        Some(300)
    );
    let row = anyrouter_row(&report(&effective, &telemetry, options()));
    assert!(row.contains("300 requests"), "{row}");
}

// --- line 1228, at the report level -----------------------------------

#[test]
fn a_providers_own_header_outranks_the_plan_a_user_typed_for_the_same_provider() {
    let mut quota = QuotaOverride::default();
    quota.set_plan(Some("free-tier".to_owned()));
    let user = user_with_anyrouter(quota);
    let effective = EffectiveConfig::new(&user, None);

    let manual_only = strongest_class(&effective, &GatheredTelemetry::new(), NOW);
    assert_eq!(manual_only, Some(TelemetryClass::Manual));

    let with_header = strongest_class(
        &effective,
        &GatheredTelemetry::new().with_provider_headers("anyrouter", anyrouter_headers(), OBSERVED),
        NOW,
    );
    assert_eq!(with_header, Some(TelemetryClass::Authoritative));
}

// --- lines 1231 and 1232, at the report level --------------------------

#[test]
fn a_harness_report_reaches_the_subscription_row_and_no_provider_row() {
    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);
    let telemetry = GatheredTelemetry::new().with_harness(
        IntegrationId::ClaudeCode,
        crate::provider::telemetry::HarnessTelemetry::plan(
            "max",
            OBSERVED,
            "claude auth status --json",
        ),
    );
    let rendered = report(&effective, &telemetry, options());

    let claude = rendered
        .split("\n\n")
        .find(|block| block.starts_with("Claude Code subscription"))
        .expect("a Claude Code block")
        .to_owned();
    assert!(claude.contains("max [authoritative]"), "{claude}");
    assert!(claude.contains("claude auth status --json"), "{claude}");

    // Independence, line 1232: no other harness and no provider learned
    // anything from it.
    let codex = rendered
        .split("\n\n")
        .find(|block| block.starts_with("Codex subscription"))
        .expect("a Codex block");
    assert!(
        codex.contains("plan            unmeasured (unknown)"),
        "{codex}"
    );
}

// --- line 1240 / map line 1761: the debug view -------------------------

/// Every resource names the class of claim its knowledge rests on,
/// including the ones where the answer is `unknown`. Driven over the
/// whole registry rather than a sample, so a resource kind added later
/// fails here instead of silently rendering without a source.
#[test]
fn every_resource_in_the_report_names_its_telemetry_class() {
    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);
    let rendered = report(&effective, &GatheredTelemetry::new(), options());

    for kind in registry() {
        let block = rendered
            .split("\n\n")
            .find(|block| block.starts_with(&kind.label()))
            .unwrap_or_else(|| panic!("no block for `{}`:\n{rendered}", kind.label()));
        assert!(
            block.contains("telemetry       "),
            "`{}` names no telemetry class",
            kind.label()
        );
    }
    // With nothing read at all, every one of them says so.
    assert_eq!(
        rendered.matches("telemetry       unknown").count(),
        registry().len()
    );
}

#[test]
fn the_verbose_view_shows_every_pool_including_the_ones_nothing_is_known_about() {
    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);
    let quiet = report(&effective, &GatheredTelemetry::new(), options());
    let verbose = report(
        &effective,
        &GatheredTelemetry::new(),
        ReportOptions {
            verbose: true,
            now_unix: NOW,
        },
    );

    assert!(quiet.len() < verbose.len());
    assert!(!quiet.contains("cached input tokens"), "{quiet}");
    assert!(verbose.contains("cached input tokens"), "{verbose}");
    assert!(verbose.contains("calendar window"), "{verbose}");
}

/// The characteristic failure of this whole phase, guarded at the surface
/// a person reads: with nothing measured, no number appears anywhere.
#[test]
fn a_report_with_no_telemetry_shows_no_capacity_figure_at_all() {
    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);
    let rendered = report(
        &effective,
        &GatheredTelemetry::new(),
        ReportOptions {
            verbose: true,
            now_unix: NOW,
        },
    );
    assert!(
        !rendered.contains('%'),
        "a percentage was printed: {rendered}"
    );
    assert!(
        !rendered.contains("last observed   unix"),
        "an observation was dated: {rendered}"
    );
    assert_eq!(
        rendered.matches("last observed   never").count(),
        registry().len()
    );
}

// --- line 1238, at the report level ------------------------------------

/// Every reader is total, so the report is producible from any
/// combination of nothing. This drives the exact function `main.rs`
/// calls, with the readings a completely failed telemetry pass leaves
/// behind.
#[test]
fn a_report_is_still_produced_when_every_telemetry_read_yielded_nothing() {
    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);
    let nothing = GatheredTelemetry::new()
        .with_harness(
            IntegrationId::ClaudeCode,
            crate::provider::telemetry::HarnessTelemetry::nothing(),
        )
        .with_provider_headers("anyrouter", RateLimitHeaders::read(Vec::new()), OBSERVED);

    let rendered = report(&effective, &nothing, options());
    assert!(rendered.starts_with("RESOURCES"));
    for kind in registry() {
        assert!(rendered.contains(&kind.label()), "{}", kind.label());
    }
}

// --- the provider-native unit survives rendering -----------------------

/// Capability map line 1217, at the surface: a millionths-scaled amount
/// prints as the provider's own money in the provider's own unit, never
/// as a bare integer of microdollars and never converted.
#[test]
fn a_rendered_amount_keeps_the_providers_own_unit_and_scale() {
    assert_eq!(
        render_amount(&NativeAmount::whole(300, "requests")),
        "300 requests"
    );
    assert_eq!(
        render_amount(&NativeAmount::millionths(2_500_000, "USD")),
        "2.500000 USD"
    );
    assert_eq!(
        render_amount(&NativeAmount::millionths(1_200_000, "credits")),
        "1.200000 credits"
    );
}

/// The estimate path, at the surface. A percentage this report can print
/// at all must go through `Percentage::render`, so an estimate is marked
/// wherever it appears.
#[test]
fn an_estimated_percentage_is_marked_as_one_in_the_report() {
    let mut quota = QuotaOverride::default();
    quota.set_budget(Some(
        MonetaryBudget::new(10_000_000, BudgetPeriod::RollingThirtyDays).unwrap(),
    ));
    let user = user_with_anyrouter(quota);
    let effective = EffectiveConfig::new(&user, None);

    let state = observed_capacity(
        &ResourceKind::from_direct_provider("anyrouter"),
        &effective,
        &GatheredTelemetry::new(),
        NOW,
    );
    // Glasshouse counts no spend, so supply the observed half the way a
    // future local counter would, and check how it renders.
    let pool = state
        .user_budget()
        .clone()
        .with_remaining(Capacity::Measured(Reading::new(
            NativeAmount::millionths(2_500_000, "USD"),
            OBSERVED,
            ReadingSource::LocalObservation("this session's own spend".to_owned()),
        )));
    let state = state.with_user_budget(pool);
    let (label, score) = state.normalized().expect("the budget pool has both halves");
    assert_eq!(label, "user budget");

    let rendered = score.percent().render();
    assert!(rendered.starts_with("~25%"), "{rendered}");
    assert!(rendered.contains("estimated"), "{rendered}");
    assert_eq!(score.percent().exact(), None);
}

// --- line 1230: a provider's own usage endpoint -------------------------

/// No credential in memory and none resolved — every `resolve` and
/// `is_present` answers empty. Deliberately not
/// `crate::secret::EnvironmentSecretStore`: a test process's own
/// environment must never leak into what a probe request is built with.
struct NoSecrets;
impl crate::secret::SecretStore for NoSecrets {
    fn resolve(&self, _reference: &crate::secret::SecretRef) -> Option<crate::secret::Secret> {
        None
    }
    fn is_present(&self, _reference: &crate::secret::SecretRef) -> bool {
        false
    }
    fn describe(&self) -> &'static str {
        "no-secrets (test)"
    }
}

#[test]
fn usage_probe_builds_a_request_against_the_declared_path_for_openrouter() {
    let openrouter = crate::provider::template("openrouter").expect("a built-in template");
    let request = usage_probe(&openrouter, &NoSecrets).expect("openrouter declares one");
    assert_eq!(request.url(), "https://openrouter.ai/api/v1/key");
}

/// Every other built-in template declares no usage endpoint, so
/// `--probe`ing one makes exactly the one request it always did.
#[test]
fn a_provider_with_no_declared_usage_endpoint_is_not_probed_a_second_time() {
    let anyrouter = crate::provider::template("anyrouter").expect("a built-in template");
    assert!(usage_probe(&anyrouter, &NoSecrets).is_none());
}

/// `render_probe`, fed a reading exactly as `probe_provider` would build
/// one from OpenRouter's real recorded response — the production path a
/// `--probe openrouter` run actually renders through, driven here without
/// a socket the way `a_providers_own_rate_limit_header_reaches_the_report_as_authoritative`
/// drives `report` without one.
#[test]
fn a_null_usage_endpoint_limit_renders_as_not_applicable_and_no_percentage() {
    let reading = ProbeReading::Answered {
        outcome: crate::provider::discovery::ProbeOutcome::Reached { status: 200 },
        headers: RateLimitHeaders::read(Vec::new()),
        observed_at_unix: OBSERVED,
        usage: Some(Box::new(crate::provider::telemetry::ProviderUsage::read(
            r#"{"data": {"limit": null, "limit_remaining": null, "limit_reset": null}}"#,
        ))),
    };
    let mut rendered = String::new();
    render_probe(&mut rendered, "openrouter", &reading);
    assert!(
        rendered.contains("usage endpoint: limit not applicable"),
        "{rendered}"
    );
    assert!(
        !rendered.contains("usage endpoint capacity:"),
        "no percentage is computable from a null limit: {rendered}"
    );
}

/// The positive half: a hypothetical numeric reading — no live account
/// has ever answered one — renders a real percentage line, proving the
/// caller `main.rs` unconditionally invokes actually asks the question
/// capability map lines 1217/1218 need answered.
#[test]
fn a_numeric_usage_endpoint_reading_renders_a_real_percentage() {
    let reading = ProbeReading::Answered {
        outcome: crate::provider::discovery::ProbeOutcome::Reached { status: 200 },
        headers: RateLimitHeaders::read(Vec::new()),
        observed_at_unix: OBSERVED,
        usage: Some(Box::new(crate::provider::telemetry::ProviderUsage::read(
            r#"{"data": {"limit": 20, "limit_remaining": 5}}"#,
        ))),
    };
    let mut rendered = String::new();
    render_probe(&mut rendered, "openrouter", &reading);
    assert!(
        rendered.contains("usage endpoint capacity: 25%"),
        "{rendered}"
    );
    assert!(!rendered.contains("estimated"), "{rendered}");
}

/// The two tests above drive `render_probe` with a hand-built
/// [`ProbeReading`], which proves the rendering path but not that
/// `probe_provider` — the function `main.rs` actually calls — makes the
/// second request at all. Practice §35: a caller every test bypasses is
/// not a caller. This one goes through a real socket and a real
/// `EffectiveConfig`, over both requests `--probe openrouter` makes.
#[test]
fn probe_provider_makes_both_the_connectivity_request_and_the_usage_request() {
    let fixture = crate::provider::fixture::FixtureProvider::answering(
        "HTTP/1.1 200 OK",
        "content-type: application/json\r\nratelimit-limit: 300\r\n",
        r#"{"data":{"limit":null,"limit_remaining":null,"limit_reset":null}}"#,
    );
    let mut user = UserConfig::default();
    let mut provider = ProviderConfig::new("openrouter");
    provider.set_base_url(Some(fixture.base_url()));
    user.providers_mut().set("openrouter", provider);
    let effective = EffectiveConfig::new(&user, None);
    let secrets = NoSecrets;

    let reading = probe_provider(&effective, &secrets, "openrouter", OBSERVED);
    match reading {
        ProbeReading::Answered { headers, usage, .. } => {
            assert_eq!(
                headers.limit(),
                Some(300),
                "the connectivity request's own headers did not reach the reading"
            );
            let usage = usage.expect(
                "openrouter declares a usage endpoint, so probe_provider must have queried it",
            );
            assert!(
                !usage.is_empty(),
                "the usage endpoint's real body was not read"
            );
        }
        other => panic!("expected an answered probe, got {other:?}"),
    }
    // Two requests: the model-list connectivity probe and the usage
    // endpoint, both against the same fixture.
    assert_eq!(fixture.connections(), 2, "expected exactly two requests");
}

// --- the harness status command ----------------------------------------

/// Practice §5: check a declaration against the *use*. Only harnesses
/// with an interface checked on a real binary are listed, and the
/// executable name comes from the adapter rather than from a second copy
/// here.
#[test]
fn only_harnesses_with_a_checked_status_interface_are_listed() {
    for (id, args) in HARNESS_STATUS_ARGS {
        assert_eq!(id.kind(), IntegrationKind::Harness);
        assert!(!args.is_empty());
        // The name is the adapter's, never this module's.
        assert!(!id.executable_candidates().is_empty(), "{id:?}");
        assert_eq!(harness_status_args(*id), Some(*args));
    }
    // A harness with no checked interface is asked for nothing.
    assert_eq!(harness_status_args(IntegrationId::Cursor), None);
    // Codex is deliberately absent: `codex doctor --json` is stable and
    // machine-readable and carries no usage field at all.
    assert!(
        !HARNESS_STATUS_ARGS
            .iter()
            .any(|(id, _)| *id == IntegrationId::Codex)
    );
}

/// The interface string is printed in a report a user may share, so it
/// must be the command and never the resolved path — a path under a home
/// directory names the user.
#[test]
fn a_harness_interface_name_is_a_command_and_never_a_resolved_path() {
    let name = harness_interface_name(IntegrationId::ClaudeCode, &["auth", "status", "--json"]);
    assert_eq!(name, "claude auth status --json");
    assert!(!name.contains('/'));
}

// --- all four rate ceilings, not only the two something already reads --

#[test]
fn every_rate_ceiling_appears_in_the_verbose_view_including_the_unread_ones() {
    let state = CapacityState::metered_balance().with_rate_ceilings(RateCeilings::uniform(
        Capacity::Unmeasured,
        Capacity::Unmeasured,
    ));
    let mut out = String::new();
    render_rate_ceilings(
        &mut out,
        &state,
        ReportOptions {
            verbose: true,
            now_unix: NOW,
        },
    );

    // Premise first (practice §17): the two ceilings that already
    // rendered must still be here, so this could not pass because the
    // whole function was deleted.
    assert!(out.contains("requests/minute"), "{out}");
    assert!(out.contains("long window"), "{out}");
    assert!(out.contains("tokens/minute"), "{out}");
    assert!(out.contains("max concurrent"), "{out}");
}

#[test]
fn a_measured_tokens_per_minute_ceiling_reaches_the_report() {
    let rates = RateCeilings::uniform(Capacity::Unmeasured, Capacity::Unmeasured)
        .with_tokens_per_minute(Capacity::Measured(Reading::new(
            NativeAmount::whole(40_000, "tokens"),
            OBSERVED,
            ReadingSource::LocalObservation("this session's own reading".to_owned()),
        )));
    let state = CapacityState::metered_balance().with_rate_ceilings(rates);
    let mut out = String::new();
    render_rate_ceilings(&mut out, &state, options());

    assert!(out.contains("tokens/minute"), "{out}");
    assert!(out.contains("40000 tokens"), "{out}");
}

#[test]
fn an_unread_rate_ceiling_is_absent_from_the_non_verbose_view() {
    let state = CapacityState::metered_balance().with_rate_ceilings(RateCeilings::uniform(
        Capacity::Unmeasured,
        Capacity::Unmeasured,
    ));
    let mut out = String::new();
    render_rate_ceilings(&mut out, &state, options());

    assert!(!out.contains("tokens/minute"), "{out}");
}

// --- lines 1316 and 1365, through the same production rendering function ----

use crate::routing::evidence::Outcome as RoutingOutcome;

fn counts(throttle: usize, exhausted: usize, upstream: usize, served: usize) -> FailureClassCounts {
    let mut counts = FailureClassCounts::default();
    for _ in 0..throttle {
        counts.record(Some(RoutingOutcome::Failed), Some(FailureClass::Throttle));
    }
    for _ in 0..exhausted {
        counts.record(
            Some(RoutingOutcome::Failed),
            Some(FailureClass::ExhaustedQuota),
        );
    }
    for _ in 0..upstream {
        counts.record(
            Some(RoutingOutcome::Failed),
            Some(FailureClass::Upstream5xx),
        );
    }
    for _ in 0..served {
        counts.record(Some(RoutingOutcome::Succeeded), None);
    }
    counts
}

/// Line 1365 at the rendering function `main.rs::resources_report`
/// calls: three figures, each with the denominator, and no line that
/// adds them up.
#[test]
fn three_failure_figures_render_separately_with_their_denominator() {
    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);
    let telemetry =
        GatheredTelemetry::new().with_provider_failure_classes("anyrouter", counts(3, 1, 2, 12));
    let row = anyrouter_row(&report(&effective, &telemetry, options()));

    assert!(
        row.contains(
            "failures 24h    cadence throttled 3, quota exhausted 1, provider unhealthy 2 — \
             of 18 exchange(s), 12 served"
        ),
        "{row}"
    );
    assert!(
        row.contains("by class        throttle 3, exhausted quota 1, upstream 5xx 2"),
        "{row}"
    );
    // Nothing sums the three: neither the total failures (6) nor the
    // total with served (18) appears as a failures figure.
    assert!(!row.contains("failures 24h    6"), "{row}");
    assert!(!row.contains("6 failure"), "{row}");
}

/// Line 1316 at the same function: rate-limit responses counted apart
/// from transport and model failures — a provider that was only ever
/// throttled shows zero under provider health, and the other way round.
#[test]
fn rate_limit_responses_are_counted_apart_from_provider_failures() {
    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);

    let throttled_only =
        GatheredTelemetry::new().with_provider_failure_classes("anyrouter", counts(4, 0, 0, 1));
    let row = anyrouter_row(&report(&effective, &throttled_only, options()));
    assert!(
        row.contains("cadence throttled 4, quota exhausted 0, provider unhealthy 0"),
        "{row}"
    );

    let failing_only =
        GatheredTelemetry::new().with_provider_failure_classes("anyrouter", counts(0, 0, 4, 1));
    let row = anyrouter_row(&report(&effective, &failing_only, options()));
    assert!(
        row.contains("cadence throttled 0, quota exhausted 0, provider unhealthy 4"),
        "{row}"
    );
}

/// Line 1324's rule applied to this line: a resource nothing has been
/// recorded for says `unknown`, never zero.
#[test]
fn a_resource_with_no_recorded_exchange_prints_unknown_failures_not_zero() {
    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);
    let row = anyrouter_row(&report(&effective, &GatheredTelemetry::new(), options()));
    assert!(
        row.contains("failures 24h    unknown — no routing observation has been recorded"),
        "{row}"
    );
    assert!(!row.contains("cadence throttled 0"), "{row}");
}

/// The verbose view lists every class, zero or not — line 1761's debug
/// view applied here; the quiet view lists only what happened.
#[test]
fn the_verbose_view_lists_every_class_and_the_quiet_view_only_the_nonzero_ones() {
    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);
    let telemetry =
        GatheredTelemetry::new().with_provider_failure_classes("anyrouter", counts(1, 0, 0, 0));

    let quiet = anyrouter_row(&report(&effective, &telemetry, options()));
    assert!(quiet.contains("by class        throttle 1\n"), "{quiet}");

    let verbose = anyrouter_row(&report(
        &effective,
        &telemetry,
        ReportOptions {
            verbose: true,
            ..options()
        },
    ));
    for class in FailureClass::ALL {
        assert!(
            verbose.contains(&format!(
                "{} {}",
                describe_failure_class(class),
                telemetry_count(&telemetry, class)
            )),
            "verbose must list `{class}`: {verbose}"
        );
    }
}

fn telemetry_count(telemetry: &GatheredTelemetry, class: FailureClass) -> usize {
    telemetry
        .for_provider_failure_classes("anyrouter")
        .map_or(0, |counts| counts.count(class))
}

/// The gather path against a real ledger, so the bridge from the rows
/// the gateway writes to the line `glasshouse resources` prints is
/// proven end to end minus the one `main.rs` call the report names.
#[test]
fn gather_failure_classes_reads_a_real_ledger_into_the_rendered_report() {
    use crate::routing::evidence::NewObservation;
    use crate::{Cli, Runtime};
    use clap::Parser;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("workspace").join("proj");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    let root = std::fs::canonicalize(&root).unwrap();
    let cli = Cli::try_parse_from([
        "glasshouse",
        "--data-dir",
        tmp.path().join("data").to_str().unwrap(),
        "--config-dir",
        tmp.path().join("config").to_str().unwrap(),
    ])
    .unwrap();
    let runtime: Runtime = crate::bootstrap(&cli, &root).unwrap();
    let ledger = EvidenceLedger::open(&runtime).unwrap();

    for (class, outcome) in [
        (Some(FailureClass::Throttle), RoutingOutcome::Failed),
        (Some(FailureClass::Throttle), RoutingOutcome::Failed),
        (Some(FailureClass::Upstream5xx), RoutingOutcome::Failed),
        (None, RoutingOutcome::Succeeded),
    ] {
        ledger
            .record(
                NewObservation::new("anyrouter", "some-model")
                    .with_outcome(outcome)
                    .with_failure_class(class),
                NOW - 60,
            )
            .unwrap();
    }
    // Outside the window: yesterday's outage must not colour today's
    // reading.
    ledger
        .record(
            NewObservation::new("anyrouter", "some-model")
                .with_outcome(RoutingOutcome::Failed)
                .with_failure_class(Some(FailureClass::ExhaustedQuota)),
            NOW - FAILURE_CLASS_WINDOW_SECONDS - 1,
        )
        .unwrap();

    let user = UserConfig::default();
    let effective = EffectiveConfig::new(&user, None);
    let telemetry = GatheredTelemetry::new().gather_failure_classes(&ledger, NOW);
    let row = anyrouter_row(&report(&effective, &telemetry, options()));
    assert!(
        row.contains(
            "cadence throttled 2, quota exhausted 0, provider unhealthy 1 — of 4 \
             exchange(s), 1 served"
        ),
        "{row}"
    );
}
