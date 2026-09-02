//! Phase 32E lines 1274 and 1276–1283 — burn rate and exhaustion
//! forecasting, entered through the public surface a caller actually has.
//!
//! Three things this file proves that `routing::burn`'s own `#[cfg(test)]`
//! module cannot, because each one is about a *seam* rather than about
//! arithmetic:
//!
//! - a row written through `EvidenceLedger::record` comes back carrying the
//!   task class it was given (line 1276's propagation, and migration 23);
//! - `SessionRouter::choose` ranks a destination forecast to exhaust well
//!   before its reset **below** an otherwise identical one, and leaves a
//!   destination with no forecast byte-for-byte where it was (line 1280);
//! - a completed request produces a row whose token fields are `None` where
//!   nothing measured them (line 1274).
//!
//! The pairs below follow `tests/subscription_pressure.rs`'s own rule
//! (`docs/product/evidence/phase-9j.md`'s constant-signal rule; practice
//! §35): every pair differs **in the forecast alone**, so a build where the
//! new term is dead cannot pass.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use glasshouse::harness::pairing::PairingOverrides;
use glasshouse::integrations::IntegrationId;
use glasshouse::provider::quota::{
    Capacity, CapacityBand, CapacityState, NativeAmount, Pool, Reading, ReadingSource,
    RemainingCapacityScore,
};
use glasshouse::routing::burn::{self, ExhaustionForecast, MIN_ROWS_FOR_BURN_RATE, ResourceKey};
use glasshouse::routing::classify::WorkloadTier;
use glasshouse::routing::evidence::{EvidenceLedger, NewObservation, Outcome};
use glasshouse::routing::free::FreePool;
use glasshouse::routing::pressure::{CapacityFacts, EXHAUSTION_FORECAST_PENALTY};
use glasshouse::routing::request::TaskClass;
use glasshouse::routing::session::{
    Destination, Routed, RouterInputs, RoutingMoment, SessionRouter, TaskRequirements,
};
use glasshouse::routing::{AssignedModel, Backend, Cost, CredentialId, ToolSemantics};
use glasshouse::secret::SecretRef;

const PROTOCOL: &str = "anthropic-messages";
const HARNESS: IntegrationId = IntegrationId::ClaudeCode;
const NOW: i64 = 1_800_000_000;

/// A bootstrapped project rooted at `base`, the way every other integration
/// test in this crate builds one: a real `--data-dir` and `--config-dir`
/// under a temporary root, so the ledger below is a real project database
/// with migration 23 applied and its project-scope triggers live.
fn project(base: &Path) -> glasshouse::Runtime {
    use clap::Parser;
    let root = base.join("project");
    std::fs::create_dir_all(&root).unwrap();
    let cli = glasshouse::Cli::try_parse_from([
        "glasshouse",
        "--data-dir",
        base.join("data").to_str().unwrap(),
        "--config-dir",
        base.join("config").to_str().unwrap(),
    ])
    .unwrap();
    glasshouse::bootstrap(&cli, &root).unwrap()
}

// ===========================================================================
// Line 1276 and migration 23 — the class survives the round trip.
// ===========================================================================

/// **Line 1276's propagation, through the real writer and the real reader.**
///
/// This is the seam `main.rs::record_routing_latency` uses: it builds a
/// `NewObservation`, calls `with_task_class`, and hands it to
/// `EvidenceLedger::record`. Everything but the `RouterAnswer` itself is
/// exercised here, and the class must come back on the row.
#[test]
fn a_recorded_row_carries_the_task_class_it_was_given() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime = project(tmp.path());
    let ledger = EvidenceLedger::open(&runtime).unwrap();

    for (index, class) in TaskClass::ALL.into_iter().enumerate() {
        ledger
            .record(
                NewObservation::new("glasshouse", "session-router")
                    .with_harness(Some(HARNESS.slug()))
                    .with_task_class(Some(class)),
                NOW + index as i64,
            )
            .unwrap();
    }
    // And one row with no class at all — the shape every gateway row has.
    ledger
        .record(
            NewObservation::new("glasshouse", "session-router").with_harness(Some(HARNESS.slug())),
            NOW + 100,
        )
        .unwrap();

    let rows = ledger
        .consumption_in_window(NOW + 1_000, 10_000)
        .expect("the consumption read returns every row, outcome or not");
    assert_eq!(rows.len(), 6);

    let recorded: Vec<Option<TaskClass>> = rows.iter().map(|row| row.task_class).collect();
    let mut expected: Vec<Option<TaskClass>> = TaskClass::ALL.into_iter().map(Some).collect();
    expected.push(None);
    assert_eq!(
        recorded, expected,
        "every class written must be the class read back, and an unclassified row must \
         stay unclassified"
    );

    // The rate reader sees them: five classes, one row each.
    let rates = burn::task_class_request_rates(&rows, NOW + 1_000, None);
    assert_eq!(rates.len(), 5, "one entry per class that has rows");
    assert!(rates.iter().all(|rate| rate.rows == 1));
}

/// The routing-latency row records **no outcome**, so
/// `observations_in_window` cannot see it — which is why
/// `consumption_in_window` exists. Pinned here because a future widening of
/// the older read would silently change what four classifiers count.
#[test]
fn the_outcome_filtered_read_does_not_see_an_unfinished_routing_row() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime = project(tmp.path());
    let ledger = EvidenceLedger::open(&runtime).unwrap();
    ledger
        .record(
            NewObservation::new("glasshouse", "session-router")
                .with_task_class(Some(TaskClass::ShellWork)),
            NOW,
        )
        .unwrap();

    assert!(
        ledger
            .observations_in_window(NOW + 10, 1_000)
            .unwrap()
            .is_empty(),
        "a row with no outcome is not evidence about how exchanges went"
    );
    assert_eq!(
        ledger.consumption_in_window(NOW + 10, 1_000).unwrap().len(),
        1,
        "but it is one request consumed"
    );
}

// ===========================================================================
// Line 1274 — a completed request produces a row, and fabricates nothing.
// ===========================================================================

/// **Line 1274, proof-only.** A completed request produces a row; its token
/// fields are `None` where nothing measured them and `Some` where something
/// did. The line's own hedge is *"when measurable"*, and honest silence is
/// compliance.
///
/// The two shapes here are exactly the two production writers:
/// `record_routing_latency`, which measures no tokens and writes none, and
/// `record_extraction_observation`, which calls `with_tokens` from a parsed
/// response it owns.
#[test]
fn a_completed_request_produces_a_row_and_invents_no_token_count() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime = project(tmp.path());
    let ledger = EvidenceLedger::open(&runtime).unwrap();

    // The routing decision: recorded, with nothing measured about its size.
    ledger
        .record(
            NewObservation::new("glasshouse", "session-router")
                .with_harness(Some(HARNESS.slug()))
                .with_task_class(Some(TaskClass::Investigation))
                .with_timing(Some(NOW - 2), Some(NOW)),
            NOW,
        )
        .unwrap();

    // An extraction: recorded, with the count it genuinely parsed.
    ledger
        .record(
            NewObservation::new("anthropic", "the-model")
                .with_outcome(Outcome::Succeeded)
                .with_tokens(Some(1_200), Some(340), None),
            NOW + 1,
        )
        .unwrap();

    let rows = ledger.consumption_in_window(NOW + 10, 1_000).unwrap();
    assert_eq!(rows.len(), 2, "one row per completed request");

    let router = rows
        .iter()
        .find(|row| row.provider == "glasshouse")
        .expect("the routing row");
    assert_eq!(
        (router.input_tokens, router.output_tokens),
        (None, None),
        "nothing measured this turn's tokens, so the row states none — never a zero"
    );
    assert!(
        router.completed_at_unix.is_some(),
        "the consumption itself is recorded even when its size is not"
    );

    let extraction = rows
        .iter()
        .find(|row| row.provider == "anthropic")
        .expect("the extraction row");
    assert_eq!(
        (extraction.input_tokens, extraction.output_tokens),
        (Some(1_200), Some(340)),
        "a producer that measured a count records it"
    );

    // And the burn reader honours the same distinction: a window of rows
    // that measured nothing offers no token rate.
    let unmeasured: Vec<_> = rows
        .iter()
        .filter(|row| row.provider == "glasshouse")
        .cloned()
        .collect();
    assert_eq!(
        burn::burn_rate(
            &unmeasured,
            ResourceKey {
                provider: "glasshouse",
                quota_context: None,
            },
            NOW + 10,
            None,
        ),
        None,
        "one row is below the floor — the reader states no rate rather than a thin one"
    );
}

// ===========================================================================
// Line 1280 — the ranking.
// ===========================================================================

fn backend(provider: &str) -> Backend {
    Backend::new(
        provider,
        PROTOCOL,
        AssignedModel::named("the-same-model"),
        CredentialId::new(
            provider,
            SecretRef::Environment {
                var: format!("{}_KEY", provider.to_uppercase().replace('-', "_")),
            },
        ),
        Cost::Metered,
        ToolSemantics::Verified,
    )
}

/// A fully-measured score at `percent`, so `known quota pressure` reads an
/// exact figure and is equal across a pair built at one percent.
fn capacity(percent: i64) -> RemainingCapacityScore {
    let measured = |value: i64| {
        Capacity::Measured(Reading::new(
            NativeAmount::whole(value, "tokens"),
            NOW,
            ReadingSource::ResponseHeader("x-ratelimit".to_owned()),
        ))
    };
    CapacityState::metered_balance()
        .with_credits(
            Pool::inapplicable()
                .with_remaining(measured(percent))
                .with_limit(measured(100)),
        )
        .remaining_capacity_score()
        .expect("both halves of the credits pool are measured")
}

/// A fresh metered destination. Band, reset and forecast are attached by the
/// caller so a pair can hold everything else equal.
fn destination(id: &str, forecast: Option<ExhaustionForecast>) -> Destination {
    Destination::fresh(
        id,
        HARNESS,
        "profile",
        backend(&format!("{id}-provider")),
        None,
    )
    .with_capacity(Some(capacity(40)))
    .with_capacity_facts(CapacityFacts::new(
        Some(CapacityBand::Healthy),
        Some(28_800),
    ))
    .with_burn_forecast(forecast)
}

/// Forecast to exhaust in 90 minutes with an 8-hour reset: well before it.
fn exhausts_early() -> ExhaustionForecast {
    ExhaustionForecast {
        requests_per_hour: 40.0,
        seconds_to_exhaustion: 5_400,
        survives_until_reset: Some(false),
        seconds_until_reset: Some(28_800),
        rows: 60,
    }
}

/// Forecast to last twelve hours against the same 8-hour reset: comfortable.
fn comfortable() -> ExhaustionForecast {
    ExhaustionForecast {
        requests_per_hour: 5.0,
        seconds_to_exhaustion: 43_200,
        survives_until_reset: Some(true),
        seconds_until_reset: Some(28_800),
        rows: 60,
    }
}

struct Fixture {
    overrides: PairingOverrides,
    health: FreePool,
    now: Instant,
}

impl Fixture {
    fn new() -> Self {
        Self {
            overrides: PairingOverrides::from_parts(
                "no configuration",
                BTreeMap::new(),
                BTreeMap::new(),
            ),
            health: FreePool::new(),
            now: Instant::now(),
        }
    }

    fn choose(&self, destinations: &[Destination]) -> Routed {
        SessionRouter::new()
            .choose(
                RoutingMoment::SessionStart,
                None,
                destinations,
                &RouterInputs {
                    overrides: &self.overrides,
                    health: &self.health,
                    now: self.now,
                    requirements: TaskRequirements {
                        minimum_tier: Some(WorkloadTier::Standard),
                        ..TaskRequirements::default()
                    },
                },
            )
            .expect("a non-empty candidate set is always routed")
    }
}

fn term(routed: &Routed, destination: &str, name: &str) -> (f64, String) {
    let (_, explanation) = routed
        .considered()
        .iter()
        .find(|(d, _)| d.id() == destination)
        .unwrap_or_else(|| {
            panic!(
                "`{destination}` was not ranked:\n{}",
                routed.render_overview()
            )
        });
    let contribution = explanation
        .contributions()
        .iter()
        .find(|c| c.name() == name)
        .unwrap_or_else(|| {
            panic!(
                "`{destination}` carried no `{name}` term:\n{}",
                explanation.render()
            )
        });
    (contribution.magnitude(), contribution.evidence().to_owned())
}

/// **Line 1280's killer.** Two destinations identical in every axis the
/// router reads — same percentage, same band, same reset, same cost, same
/// freshness — differing **only** in their forecast. The one forecast to
/// exhaust well before its reset must rank below the other, and the
/// explanation must say so in words.
#[test]
fn a_destination_forecast_to_exhaust_early_ranks_below_an_identical_comfortable_one() {
    let fixture = Fixture::new();
    let set = [
        destination("early", Some(exhausts_early())),
        destination("comfortable", Some(comfortable())),
    ];
    let routed = fixture.choose(&set);

    assert_eq!(
        routed.chosen().id(),
        "comfortable",
        "the forecast is the only axis these two differ in:\n{}",
        routed.render_overview()
    );

    let (early_magnitude, early_why) = term(&routed, "early", "exhaustion forecast");
    assert_eq!(early_magnitude, EXHAUSTION_FORECAST_PENALTY);
    assert!(
        early_why.contains("estimated to last about 1.5h at the current rate"),
        "{early_why}"
    );
    assert!(
        early_why.contains("may not reach that reset"),
        "the explanation must name the forecast in hedged words: {early_why}"
    );
    for promise in ["will exhaust", "will run out", "guaranteed"] {
        assert!(!early_why.contains(promise), "{early_why}");
    }

    let (comfortable_magnitude, comfortable_why) =
        term(&routed, "comfortable", "exhaustion forecast");
    assert_eq!(comfortable_magnitude, 0.0);
    assert!(comfortable_why.starts_with("inert:"), "{comfortable_why}");
}

/// **The inert case, and it is the one that matters most.** A candidate set
/// with no forecast anywhere ranks exactly as it did before Phase 32E, and
/// the term is present, zero, and says why.
///
/// The two destinations differ only in a tiebreak-free way, so this asserts
/// the *whole* explanation total rather than only the term: a term that
/// accidentally contributed anything would move it.
#[test]
fn a_destination_with_no_forecast_ranks_exactly_as_it_did() {
    let fixture = Fixture::new();
    let with_none = [destination("a", None), destination("b", None)];
    let routed = fixture.choose(&with_none);

    let (magnitude, why) = term(&routed, "a", "exhaustion forecast");
    assert_eq!(magnitude, 0.0);
    assert_eq!(
        why, "inert: no exhaustion forecast is sufficiently known for this resource",
        "an inert term must say why it is inert"
    );

    // The same set, and the same totals, whichever destination is read.
    let total_a = routed
        .considered()
        .iter()
        .find(|(d, _)| d.id() == "a")
        .unwrap()
        .1
        .total();
    let total_b = routed
        .considered()
        .iter()
        .find(|(d, _)| d.id() == "b")
        .unwrap()
        .1
        .total();
    assert_eq!(
        total_a,
        total_b,
        "with no forecast on either, nothing separates them:\n{}",
        routed.render_overview()
    );

    // And attaching a *comfortable* forecast to one of them still does not
    // separate them — only "well before the reset" costs anything.
    let one_comfortable = [
        destination("a", Some(comfortable())),
        destination("b", None),
    ];
    let routed = fixture.choose(&one_comfortable);
    assert_eq!(
        term(&routed, "a", "exhaustion forecast").0,
        0.0,
        "a resource forecast to outlive its reset is not penalised"
    );
}

/// The producer end of the same seam: `burn::forecast` over real ledger rows
/// yields the value a destination carries, and a window too thin to support
/// one yields `None` — which is what makes the term inert on a young project.
#[test]
fn a_thin_history_yields_no_forecast_and_a_real_one_does() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime = project(tmp.path());
    let ledger = EvidenceLedger::open(&runtime).unwrap();

    let remaining = Capacity::Measured(Reading::new(
        NativeAmount::whole(120, "requests"),
        NOW,
        ReadingSource::ResponseHeader("x-ratelimit-remaining".to_owned()),
    ));
    let key = ResourceKey {
        provider: "anthropic",
        quota_context: Some("acct-a"),
    };

    // One row short of the floor: no forecast, and that is the whole reason
    // a fresh install's ranking is unchanged.
    for i in 0..(MIN_ROWS_FOR_BURN_RATE as i64 - 1) {
        ledger
            .record(
                NewObservation::new("anthropic", "the-model")
                    .with_quota_context(Some("acct-a"))
                    .with_outcome(Outcome::Succeeded),
                NOW - 3_600 + i * 120,
            )
            .unwrap();
    }
    let thin = ledger.consumption_in_window(NOW, 7_200).unwrap();
    assert_eq!(thin.len(), MIN_ROWS_FOR_BURN_RATE - 1);
    assert_eq!(
        burn::forecast(&thin, key, &remaining, NOW, Some(28_800)),
        None,
        "below the floor the reader states nothing rather than a thin figure"
    );

    // Enough rows: a forecast, keyed to the account the rows name.
    for i in 0..30i64 {
        ledger
            .record(
                NewObservation::new("anthropic", "the-model")
                    .with_quota_context(Some("acct-a"))
                    .with_outcome(Outcome::Succeeded),
                NOW - 3_500 + i * 100,
            )
            .unwrap();
    }
    let rows = ledger.consumption_in_window(NOW, 7_200).unwrap();
    let forecast = burn::forecast(&rows, key, &remaining, NOW, Some(28_800))
        .expect("thirty rows and a measured request count is a forecast");
    assert!(forecast.requests_per_hour > 0.0);
    assert!(forecast.seconds_to_exhaustion > 0);
    assert!(forecast.rows >= 30);

    // A destination carrying it reaches the term.
    let fixture = Fixture::new();
    let routed = fixture.choose(&[
        destination("measured", Some(forecast)),
        destination("unknown", None),
    ]);
    let (_, why) = term(&routed, "measured", "exhaustion forecast");
    assert!(
        why.contains("requests/hour"),
        "the term must name the rate it read: {why}"
    );
}

/// A resource whose remaining capacity is only a **percentage** — no native
/// request count anywhere — produces no forecast, so its ranking and its
/// surfaced text are what they were. This is line 1278's *"sufficiently
/// known"* at the seam rather than in the unit tests.
#[test]
fn a_percentage_without_a_native_count_forecasts_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime = project(tmp.path());
    let ledger = EvidenceLedger::open(&runtime).unwrap();
    for i in 0..30i64 {
        ledger
            .record(
                NewObservation::new("anthropic", "the-model")
                    .with_quota_context(Some("acct-a"))
                    .with_outcome(Outcome::Succeeded),
                NOW - 3_500 + i * 100,
            )
            .unwrap();
    }
    let rows = ledger.consumption_in_window(NOW, 7_200).unwrap();
    let key = ResourceKey {
        provider: "anthropic",
        quota_context: Some("acct-a"),
    };

    // The rate is established — so the `None` is about the amount.
    assert!(burn::burn_rate(&rows, key, NOW, Some(28_800)).is_some());
    assert_eq!(
        burn::forecast(&rows, key, &Capacity::ProviderOpaque, NOW, Some(28_800)),
        None,
        "a percentage of an unknown ceiling divided by a rate is not a duration"
    );
}
