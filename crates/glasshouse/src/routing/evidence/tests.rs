//! `routing::evidence`'s inline test modules, moved out of `evidence.rs`
//! verbatim (Phase 59 decomposition rule 2). This file's own top-level items
//! are what was inside `mod tests { ... }` (the ledger/readers/signals/joins
//! tests together, since they shared one `use super::*;`); the five sibling
//! `#[cfg(test)] mod ..._tests { ... }` blocks that followed it in the
//! original file are copied verbatim below, unwrapped.

use super::*;
use crate::provider::pricing::PriceTable;
// Named here rather than reached through `use super::*`: `AggregateReading`
// moved to `vocabulary.rs` with its own private import of this enum, so
// `mod.rs` no longer holds the binding these tests used to glob.
use crate::provider::quota::Confidence;
use crate::{Cli, Runtime};
use clap::Parser;
use std::path::Path;

struct Fixture {
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
        let runtime = crate::bootstrap(&cli, &root).unwrap();
        Self { runtime }
    }

    fn ledger(&self) -> EvidenceLedger {
        EvidenceLedger::open(&self.runtime).unwrap()
    }
}

fn observation(provider: &str, model: &str) -> NewObservation {
    NewObservation::new(provider, model)
        .with_route(Some("anthropic-messages"))
        .with_harness(Some("claude-code"))
}

#[test]
fn a_recorded_observation_reads_back_with_every_field_it_was_given() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let ledger = fixture.ledger();

    let new = observation("anyrouter", "claude-opus-4-1")
        .with_timing(Some(1_000), Some(1_002))
        .with_outcome(Outcome::Succeeded)
        .with_context_state(ContextState::Warm);
    let seq = ledger.record(new, 1_002).unwrap();
    assert!(seq > 0);

    let rows = ledger.observations_in_window(1_002, 10).unwrap();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.provider, "anyrouter");
    assert_eq!(row.model, "claude-opus-4-1");
    assert_eq!(row.route.as_deref(), Some("anthropic-messages"));
    assert_eq!(row.harness.as_deref(), Some("claude-code"));
    assert_eq!(row.dispatched_at_unix, Some(1_000));
    assert_eq!(row.completed_at_unix, Some(1_002));
    assert_eq!(row.duration_ms(), Some(2_000));
    assert_eq!(row.outcome, Some(Outcome::Succeeded));
    assert_eq!(row.context_state, ContextState::Warm);
    assert_eq!(
        row.first_byte_at_unix, None,
        "this producer never supplies it"
    );
    assert_eq!(
        row.failure_class, None,
        "a served row has no kind of failure"
    );
    assert_eq!(row.failovers, None, "this test's producer did not count");
    assert_eq!(row.retries, None);
    assert_eq!(
        (
            row.first_byte_ms,
            row.first_token_ms,
            row.first_tool_call_ms,
            row.completed_ms
        ),
        (None, None, None, None),
        "migration 25's four are this producer's absence too"
    );
}

/// Migration 25's four offsets, through the real schema and back — the
/// round trip [`a_recorded_observation_reads_back_with_every_field_it_was_given`]
/// makes for every other column, and the one property that separates
/// them from every other optional column on this row:
/// [`RoutingObservation::duration_ms`] prefers the measured completion.
///
/// Mutation target `fallback-dropped`: making `duration_ms` answer
/// `None` when `completed_ms` is `None` must fail the second half here.
#[test]
fn the_millisecond_offsets_round_trip_and_duration_prefers_the_measured_one() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let ledger = fixture.ledger();

    // A measured row. The seconds say nine; the offsets say 8,910, and
    // the offsets are what was actually timed.
    ledger
        .record(
            observation("measured", "claude-opus-4-1")
                .with_timing(Some(1_000), Some(1_009))
                .with_outcome(Outcome::Succeeded)
                .with_first_byte_ms(Some(120))
                .with_first_token_ms(Some(1_450))
                .with_first_tool_call_ms(Some(2_600))
                .with_completed_ms(Some(8_910)),
            1_009,
        )
        .unwrap();

    // An unmeasured row — every producer that holds no dispatch
    // `Instant`, and every row written before migration 25.
    ledger
        .record(
            observation("unmeasured", "claude-opus-4-1")
                .with_timing(Some(1_000), Some(1_009))
                .with_outcome(Outcome::Succeeded),
            1_009,
        )
        .unwrap();

    // A relayed exchange's own shape: the two offsets its path can
    // measure and `None` for the two only a decoded stream supplies.
    ledger
        .record(
            observation("relayed", "claude-opus-4-1")
                .with_timing(Some(1_000), Some(1_002))
                .with_outcome(Outcome::Succeeded)
                .with_first_byte_ms(Some(88))
                .with_completed_ms(Some(1_940)),
            1_002,
        )
        .unwrap();

    let rows = ledger.observations_in_window(1_009, 10).unwrap();
    let by_provider = |provider: &str| rows.iter().find(|row| row.provider == provider).unwrap();

    let measured = by_provider("measured");
    assert_eq!(measured.first_byte_ms, Some(120));
    assert_eq!(measured.first_token_ms, Some(1_450));
    assert_eq!(measured.first_tool_call_ms, Some(2_600));
    assert_eq!(measured.completed_ms, Some(8_910));
    assert_eq!(
        measured.duration_ms(),
        Some(8_910),
        "a measured completion is preferred over the seconds difference"
    );

    let unmeasured = by_provider("unmeasured");
    assert_eq!(unmeasured.completed_ms, None);
    assert_eq!(
        unmeasured.duration_ms(),
        Some(9_000),
        "with nothing measured the seconds difference is still the answer"
    );

    let relayed = by_provider("relayed");
    assert_eq!(relayed.first_byte_ms, Some(88));
    assert_eq!(relayed.first_token_ms, None);
    assert_eq!(relayed.first_tool_call_ms, None);
    assert_eq!(relayed.duration_ms(), Some(1_940));
}

/// Migration 18's column and line 1334's two counters the gateway can
/// supply, through the real schema and back — including the value the
/// `outcome` `CHECK` two columns over would never have allowed a
/// vocabulary to grow into.
#[test]
fn a_failure_class_and_the_two_counters_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let ledger = fixture.ledger();

    for (i, class) in FailureClass::ALL.iter().enumerate() {
        ledger
            .record(
                observation("anyrouter", "claude-opus-4-1")
                    .with_outcome(Outcome::Failed)
                    .with_failure_class(Some(*class))
                    .with_failovers(Some(u32::from(i == 0)))
                    .with_retries(Some(0)),
                1_000 + i as i64,
            )
            .unwrap();
    }

    let mut rows = ledger
        .observations_in_window(1_000 + FailureClass::ALL.len() as i64, 100)
        .unwrap();
    rows.sort_by_key(|row| row.seq);
    assert_eq!(rows.len(), FailureClass::ALL.len());
    for (row, class) in rows.iter().zip(FailureClass::ALL) {
        assert_eq!(row.failure_class, Some(class));
        assert_eq!(row.retries, Some(0));
    }
    assert_eq!(rows[0].failovers, Some(1));
    assert!(rows[1..].iter().all(|row| row.failovers == Some(0)));
}

/// Which rows count, per [`FailureClassCounts`]' own doc: a row with no
/// outcome is nobody's exchange; a class is counted under itself; a
/// success with no class is served; anything else with no class is
/// unclassified — and line 1365's third figure excludes the two classes
/// that say nothing about the provider's health.
#[test]
fn failure_class_counts_keep_served_unclassified_and_each_class_apart() {
    let mut counts = FailureClassCounts::default();
    assert!(counts.is_empty());

    counts.record(None, None);
    counts.record(None, Some(FailureClass::Throttle));
    assert!(
        counts.is_empty(),
        "rows without an outcome are not exchanges"
    );

    counts.record(Some(Outcome::Succeeded), None);
    counts.record(Some(Outcome::Failed), None);
    counts.record(Some(Outcome::Unknown), None);
    counts.record(Some(Outcome::Failed), Some(FailureClass::Throttle));
    counts.record(Some(Outcome::Failed), Some(FailureClass::Throttle));
    counts.record(Some(Outcome::Failed), Some(FailureClass::ExhaustedQuota));
    counts.record(Some(Outcome::Failed), Some(FailureClass::Upstream5xx));
    counts.record(Some(Outcome::Failed), Some(FailureClass::StreamAbort));
    counts.record(Some(Outcome::Failed), Some(FailureClass::CredentialFailure));
    counts.record(
        Some(Outcome::Failed),
        Some(FailureClass::RequestIncompatibility),
    );

    assert_eq!(counts.served(), 1);
    assert_eq!(counts.unclassified(), 2);
    assert_eq!(counts.cadence_throttled(), 2);
    assert_eq!(counts.exhausted_quota(), 1);
    assert_eq!(
        counts.provider_health_failures(),
        2,
        "upstream 5xx and stream abort; never the credential or the request"
    );
    assert_eq!(counts.count(FailureClass::CredentialFailure), 1);
    assert_eq!(counts.observed(), 10);
}

/// [`EvidenceLedger::summarize`] carries the counts for its identity, and
/// — being counts, not rates — does not withhold them below
/// [`MIN_SAMPLE_FOR_SUMMARY`] the way it withholds `failure_rate`.
#[test]
fn summarize_counts_failure_classes_even_below_the_sample_floor() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let ledger = fixture.ledger();
    for at in [1_000, 1_001] {
        ledger
            .record(
                observation("anyrouter", "claude-opus-4-1")
                    .with_outcome(Outcome::Failed)
                    .with_failure_class(Some(FailureClass::Throttle)),
                at,
            )
            .unwrap();
    }
    let summary = ledger
        .summarize_latest_for_model("anyrouter", "claude-opus-4-1", 1_100, 1_000)
        .unwrap()
        .expect("at least one observation was recorded");
    assert!(summary.failure_rate.is_none(), "two is below the floor");
    assert_eq!(summary.failure_classes.cadence_throttled(), 2);
    assert_eq!(summary.failure_classes.observed(), 2);
}

/// Line 1359: fall back to coarse process-level latency and outcome
/// observations when a harness exposes no structured token or tool
/// events. Every row here carries only timing and outcome —
/// `first_token_at`, `first_tool_call_at` and `tool_rounds` are all
/// `None`, the shape every observation on this project has always had
/// (the structured path has never actually run) — and `summarize` must
/// still produce a usable aggregate from them rather than treating an
/// all-`None`-structured row as unusable.
#[test]
fn summarize_produces_a_usable_aggregate_from_coarse_only_observations() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let ledger = fixture.ledger();

    for (i, at) in [1_000, 1_010, 1_020, 1_030, 1_040].into_iter().enumerate() {
        let dispatched_at = at;
        let completed_at = at + 5;
        let new = observation("anyrouter", "claude-opus-4-1")
            .with_timing(Some(dispatched_at), Some(completed_at))
            .with_outcome(Outcome::Succeeded);
        assert!(new.first_token_at_unix.is_none());
        assert!(new.first_tool_call_at_unix.is_none());
        assert!(new.tool_rounds.is_none());
        ledger.record(new, at + i as i64).unwrap();
    }

    let summary = ledger
        .summarize_latest_for_model("anyrouter", "claude-opus-4-1", 2_000, 2_000)
        .unwrap()
        .expect("at least one observation was recorded");

    assert!(
        summary.median_duration_ms.is_some(),
        "coarse timing alone must produce a duration aggregate, not a skip"
    );
    assert!(
        summary.ewma_duration_ms.is_some(),
        "coarse timing alone must produce an ewma aggregate, not a skip"
    );
    assert!(
        summary.failure_rate.is_some(),
        "coarse outcomes alone must produce a failure rate, not a skip"
    );
}

/// [`EvidenceLedger::failure_classes_by_provider`] counts every model,
/// route and harness of a provider together, within the window only, and
/// leaves an outcome-less row (the extraction producer's shape) out.
#[test]
fn failure_classes_by_provider_counts_across_identities_within_the_window() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let ledger = fixture.ledger();
    let now = 10_000;

    ledger
        .record(
            observation("anyrouter", "claude-opus-4-1")
                .with_outcome(Outcome::Failed)
                .with_failure_class(Some(FailureClass::Throttle)),
            now - 10,
        )
        .unwrap();
    ledger
        .record(
            NewObservation::new("anyrouter", "claude-sonnet-4-5")
                .with_outcome(Outcome::Failed)
                .with_failure_class(Some(FailureClass::Upstream5xx)),
            now - 20,
        )
        .unwrap();
    ledger
        .record(
            observation("anyrouter", "claude-opus-4-1").with_outcome(Outcome::Succeeded),
            now - 30,
        )
        .unwrap();
    // No outcome: not an exchange, not counted.
    ledger
        .record(
            NewObservation::new("anyrouter", "claude-opus-4-1"),
            now - 40,
        )
        .unwrap();
    // Outside the window.
    ledger
        .record(
            observation("anyrouter", "claude-opus-4-1")
                .with_outcome(Outcome::Failed)
                .with_failure_class(Some(FailureClass::ExhaustedQuota)),
            now - 1_001,
        )
        .unwrap();
    // Another provider entirely.
    ledger
        .record(
            observation("groq", "llama")
                .with_outcome(Outcome::Failed)
                .with_failure_class(Some(FailureClass::CredentialFailure)),
            now - 5,
        )
        .unwrap();

    let by_provider = ledger.failure_classes_by_provider(now, 1_000).unwrap();
    assert_eq!(by_provider.len(), 2, "{by_provider:?}");
    let anyrouter = &by_provider["anyrouter"];
    assert_eq!(anyrouter.observed(), 3);
    assert_eq!(anyrouter.cadence_throttled(), 1);
    assert_eq!(anyrouter.provider_health_failures(), 1);
    assert_eq!(anyrouter.served(), 1);
    assert_eq!(
        anyrouter.exhausted_quota(),
        0,
        "yesterday's row is outside the window"
    );
    let groq = &by_provider["groq"];
    assert_eq!(groq.count(FailureClass::CredentialFailure), 1);
    assert_eq!(groq.observed(), 1);
}

/// The ledger's own append-oriented promise, proven rather than assumed:
/// there is no way to reach a second, differently-timestamped copy of one
/// observation through this store's own API.
#[test]
fn there_is_no_way_to_edit_a_recorded_observation() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let ledger = fixture.ledger();
    ledger
        .record(
            observation("anyrouter", "m").with_outcome(Outcome::Succeeded),
            1_000,
        )
        .unwrap();
    ledger
        .record(
            observation("anyrouter", "m").with_outcome(Outcome::Succeeded),
            1_001,
        )
        .unwrap();

    let rows = ledger.observations_in_window(1_001, 10).unwrap();
    assert_eq!(
        rows.len(),
        2,
        "two records must produce two rows, never one edited in place"
    );
}

/// Capability map line 1343's structural half: nothing built on this
/// ledger can read another project's observations, because each project
/// has a physically separate database file.
#[test]
fn a_ledger_never_sees_another_projects_observations() {
    let tmp = tempfile::tempdir().unwrap();
    let alpha = Fixture::new(tmp.path(), "alpha");
    let beta = Fixture::new(tmp.path(), "beta");

    alpha
        .ledger()
        .record(
            observation("anyrouter", "m").with_outcome(Outcome::Succeeded),
            1_000,
        )
        .unwrap();

    let beta_rows = beta.ledger().observations_in_window(1_000, 10).unwrap();
    assert!(
        beta_rows.is_empty(),
        "a sibling project's database must never contain another project's observation"
    );
}

/// Capability map line 1340: below the minimum sample, every aggregate is
/// `None` rather than a number computed from too little evidence.
#[test]
fn a_summary_below_the_minimum_sample_is_unknown() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let ledger = fixture.ledger();

    for i in 0..(MIN_SAMPLE_FOR_SUMMARY - 1) {
        let at = 1_000 + i as i64;
        let new = observation("anyrouter", "m")
            .with_timing(Some(at), Some(at + 1))
            .with_outcome(Outcome::Succeeded);
        ledger.record(new, at).unwrap();
    }

    let summary = ledger
        .summarize_latest_for_model("anyrouter", "m", 10_000, 100_000)
        .unwrap()
        .expect("at least one observation was recorded");
    assert!(summary.median_duration_ms.is_none());
    assert!(summary.failure_rate.is_none());
}

#[test]
fn a_summary_at_the_minimum_sample_is_a_real_number() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let ledger = fixture.ledger();

    for i in 0..MIN_SAMPLE_FOR_SUMMARY {
        let at = 1_000 + i as i64 * 10;
        let new = observation("anyrouter", "m")
            .with_timing(Some(at), Some(at + 2))
            .with_outcome(Outcome::Succeeded);
        ledger.record(new, at).unwrap();
    }

    let summary = ledger
        .summarize_latest_for_model("anyrouter", "m", 10_000, 100_000)
        .unwrap()
        .expect("at least one observation was recorded");
    let median = summary
        .median_duration_ms
        .expect("five samples must produce a reading");
    assert_eq!(*median.value(), 2_000);
    assert_eq!(median.sample_count(), MIN_SAMPLE_FOR_SUMMARY);
    assert_eq!(median.confidence(), Confidence::Medium);
    let failure_rate = summary
        .failure_rate
        .expect("five outcomes must produce a reading");
    assert_eq!(*failure_rate.value(), 0.0);
}

/// Capability map line 1341: an observation older than the summary's
/// window is excluded from the aggregate, but stays readable raw — decay
/// without deletion.
#[test]
fn an_observation_outside_the_window_is_excluded_from_the_summary_but_not_deleted() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let ledger = fixture.ledger();

    // One very old observation, then enough recent ones to clear the
    // minimum sample on their own.
    let old = observation("anyrouter", "m")
        .with_timing(Some(0), Some(1))
        .with_outcome(Outcome::Failed);
    ledger.record(old, 0).unwrap();
    for i in 0..MIN_SAMPLE_FOR_SUMMARY {
        let at = 100_000 + i as i64;
        let new = observation("anyrouter", "m")
            .with_timing(Some(at), Some(at + 1))
            .with_outcome(Outcome::Succeeded);
        ledger.record(new, at).unwrap();
    }

    let raw = ledger
        .observations_in_window(100_000 + MIN_SAMPLE_FOR_SUMMARY as i64, 200_000)
        .unwrap();
    assert_eq!(
        raw.len(),
        MIN_SAMPLE_FOR_SUMMARY + 1,
        "the old row must still be readable raw"
    );

    let summary = ledger
        .summarize_latest_for_model(
            "anyrouter",
            "m",
            100_000 + MIN_SAMPLE_FOR_SUMMARY as i64,
            1_000,
        )
        .unwrap()
        .expect("the recent, in-window observations must be found");
    let failure_rate = summary
        .failure_rate
        .expect("the recent, in-window observations alone must clear the minimum sample");
    assert_eq!(
        *failure_rate.value(),
        0.0,
        "the old failed observation is outside the window and must not pull the rate down"
    );
}

/// Capability map line 1337: rows in different context-state buckets are
/// never blended into one summary.
#[test]
fn warm_and_cold_observations_never_share_one_summary() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let ledger = fixture.ledger();

    for i in 0..MIN_SAMPLE_FOR_SUMMARY {
        let at = 1_000 + i as i64;
        let new = observation("anyrouter", "m")
            .with_timing(Some(at), Some(at + 1))
            .with_outcome(Outcome::Failed)
            .with_context_state(ContextState::Cold);
        ledger.record(new, at).unwrap();
    }
    for i in 0..MIN_SAMPLE_FOR_SUMMARY {
        let at = 2_000 + i as i64;
        let new = observation("anyrouter", "m")
            .with_timing(Some(at), Some(at + 1))
            .with_outcome(Outcome::Succeeded)
            .with_context_state(ContextState::Warm);
        ledger.record(new, at).unwrap();
    }

    // Windowed to each batch alone: the latest row inside each window names
    // that batch's own context state, so a summary blending the two buckets
    // would show up as a failure rate neither batch alone produced.
    let cold = ledger
        .summarize_latest_for_model("anyrouter", "m", 1_010, 100)
        .unwrap()
        .expect("the cold batch was recorded");
    let warm = ledger
        .summarize_latest_for_model("anyrouter", "m", 2_010, 100)
        .unwrap()
        .expect("the warm batch was recorded");
    assert_eq!(*cold.failure_rate.unwrap().value(), 1.0);
    assert_eq!(*warm.failure_rate.unwrap().value(), 0.0);
}

/// A raw insert that pairs `cost_micro_usd` with no `cost_confidence`
/// cannot happen through this store's own `record` — [`NewObservation`]
/// has no way to construct that combination, since [`ObservedCost`]
/// always carries both.
#[test]
fn a_cost_recorded_through_this_store_always_carries_a_confidence() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let ledger = fixture.ledger();
    let mut new = observation("anyrouter", "m").with_outcome(Outcome::Succeeded);
    new.cost = Some(ObservedCost {
        micro_usd: 500,
        confidence: CostConfidence::Estimated,
    });
    ledger.record(new, 1_000).unwrap();

    let rows = ledger.observations_in_window(1_000, 10).unwrap();
    let cost = rows[0].cost.expect("the cost must round-trip");
    assert_eq!(cost.micro_usd, 500);
    assert_eq!(cost.confidence, CostConfidence::Estimated);
}

/// Capability map line 1342: token volume, request count and spend are
/// resource telemetry, never evidence of quality. A summary computed from
/// two batches that differ only in `input_tokens`/`output_tokens`/`cost`
/// must be byte-for-byte identical — if a later change folded token
/// volume into a quality aggregate, this test would be the one to notice.
#[test]
fn no_aggregate_changes_when_only_token_volume_or_cost_changes() {
    let tmp = tempfile::tempdir().unwrap();
    let cheap = Fixture::new(tmp.path(), "cheap");
    let expensive = Fixture::new(tmp.path(), "expensive");
    let cheap_ledger = cheap.ledger();
    let expensive_ledger = expensive.ledger();

    for i in 0..MIN_SAMPLE_FOR_SUMMARY {
        let at = 1_000 + i as i64;
        let mut small = observation("anyrouter", "m")
            .with_timing(Some(at), Some(at + 1))
            .with_outcome(Outcome::Succeeded);
        small.input_tokens = Some(10);
        small.output_tokens = Some(10);
        cheap_ledger.record(small, at).unwrap();

        let mut large = observation("anyrouter", "m")
            .with_timing(Some(at), Some(at + 1))
            .with_outcome(Outcome::Succeeded);
        large.input_tokens = Some(200_000);
        large.output_tokens = Some(50_000);
        large.cost = Some(ObservedCost {
            micro_usd: 9_000_000,
            confidence: CostConfidence::Exact,
        });
        expensive_ledger.record(large, at).unwrap();
    }

    let cheap_summary = cheap_ledger
        .summarize_latest_for_model("anyrouter", "m", 10_000, 100_000)
        .unwrap()
        .expect("the cheap batch was recorded");
    let expensive_summary = expensive_ledger
        .summarize_latest_for_model("anyrouter", "m", 10_000, 100_000)
        .unwrap()
        .expect("the expensive batch was recorded");

    assert_eq!(
        cheap_summary.failure_rate.map(|r| *r.value()),
        expensive_summary.failure_rate.map(|r| *r.value())
    );
    assert_eq!(
        cheap_summary.median_duration_ms.map(|r| *r.value()),
        expensive_summary.median_duration_ms.map(|r| *r.value())
    );
}

/// Acceptance test 1: two recorded identities come back as exactly two
/// distinct identities, with their real sample counts — the enumeration
/// [`EvidenceLedger::recent`] and [`EvidenceLedger::summarize`] cannot
/// answer (practice §71).
#[test]
fn observed_identities_returns_the_distinct_identities_actually_recorded() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let ledger = fixture.ledger();

    ledger.record(observation("anyrouter", "m"), 1_000).unwrap();
    ledger.record(observation("anyrouter", "m"), 1_001).unwrap();
    ledger
        .record(NewObservation::new("openai-router", "gpt-5"), 1_002)
        .unwrap();

    let identities = ledger.observed_identities(10_000, 100_000, 50).unwrap();
    let mut pairs: Vec<(String, String)> = identities
        .iter()
        .map(|i| (i.provider.clone(), i.model.clone()))
        .collect();
    pairs.sort();
    assert_eq!(
        pairs,
        vec![
            ("anyrouter".to_owned(), "m".to_owned()),
            ("openai-router".to_owned(), "gpt-5".to_owned()),
        ]
    );

    let anyrouter = identities
        .iter()
        .find(|i| i.provider == "anyrouter")
        .expect("anyrouter identity");
    let openai = identities
        .iter()
        .find(|i| i.provider == "openai-router")
        .expect("openai identity");
    assert_eq!(anyrouter.sample_count(), 2);
    assert_eq!(openai.sample_count(), 1);
    assert_ne!(
        anyrouter.sample_count(),
        openai.sample_count(),
        "two identities with different counts must be distinguishable"
    );
}

/// Acceptance test 2: bounded, the same shape [`EvidenceLedger::recent`]
/// takes.
#[test]
fn observed_identities_is_bounded_by_the_limit() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let ledger = fixture.ledger();
    for i in 0..5 {
        ledger
            .record(NewObservation::new(format!("provider-{i}"), "m"), 1_000 + i)
            .unwrap();
    }

    let identities = ledger.observed_identities(10_000, 100_000, 3).unwrap();
    assert_eq!(identities.len(), 3, "at most the limit must come back");
}

/// Acceptance test 3, structural half: physical per-project database
/// separation, the same guarantee
/// [`a_ledger_never_sees_another_projects_observations`] proves for
/// [`EvidenceLedger::recent`].
#[test]
fn observed_identities_never_sees_another_projects_observations() {
    let tmp = tempfile::tempdir().unwrap();
    let alpha = Fixture::new(tmp.path(), "alpha");
    let beta = Fixture::new(tmp.path(), "beta");
    alpha
        .ledger()
        .record(observation("anyrouter", "m"), 1_000)
        .unwrap();

    let beta_identities = beta
        .ledger()
        .observed_identities(10_000, 100_000, 50)
        .unwrap();
    assert!(
        beta_identities.is_empty(),
        "a sibling project's database must never contain another project's identity"
    );
}

/// Acceptance test 3, defensive half — and why this ledger's own
/// `WHERE project_id = ?1` cannot be demonstrated to fail by a mutation
/// that removes it: a row tagged with a foreign `project_id` can never
/// even be inserted into this database. Migration 11's own
/// `routing_observations_reject_foreign_project_insert` trigger refuses
/// it at the SQL layer, before [`EvidenceLedger::observed_identities`] or
/// [`EvidenceLedger::record`] ever runs — a stronger guarantee than this
/// method's own filter, and the reason
/// [`observed_identities_never_sees_another_projects_observations`]
/// above is this project's only *reachable* isolation test for this
/// method, exactly as it already is for [`EvidenceLedger::recent`] and
/// [`EvidenceLedger::summarize`], neither of which filters by
/// `project_id` in SQL at all.
#[test]
fn a_foreign_project_id_row_cannot_even_be_inserted_into_this_database() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let ledger = fixture.ledger();

    let conn = ledger.lock();
    let err = conn
        .execute(
            "INSERT INTO routing_observations
                    (project_id, observed_at, provider, model, context_state)
                 VALUES ('someone-elses-project', 1_001, 'anyrouter', 'm', 'unknown')",
            [],
        )
        .expect_err("the schema's own trigger must refuse a foreign project_id");
    assert!(err.to_string().contains("different project"), "got: {err}");
}

/// The window and sample count both reflect real recorded timestamps —
/// not a placeholder — and rows outside the queried window are excluded
/// from both, the same decay-without-deletion contract
/// [`an_observation_outside_the_window_is_excluded_from_the_summary_but_not_deleted`]
/// proves for [`EvidenceLedger::summarize`].
#[test]
fn observed_identities_reports_the_real_window_and_excludes_rows_outside_it() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let ledger = fixture.ledger();
    ledger.record(observation("anyrouter", "old"), 0).unwrap();
    ledger
        .record(observation("anyrouter", "m"), 100_000)
        .unwrap();
    ledger
        .record(observation("anyrouter", "m"), 100_050)
        .unwrap();

    let identities = ledger.observed_identities(100_050, 1_000, 50).unwrap();
    let models: Vec<&str> = identities.iter().map(|i| i.model.as_str()).collect();
    assert_eq!(
        models,
        vec!["m"],
        "the row outside the window must not appear at all"
    );
    let m = identities.iter().find(|i| i.model == "m").unwrap();
    assert_eq!(m.sample_count(), 2);
    assert_eq!(m.window(), (100_000, 100_050));
}

/// Capability map line 1764, at the enumeration layer: rows in different
/// [`ContextState`] buckets are never blended into one identity — the
/// same separation [`warm_and_cold_observations_never_share_one_summary`]
/// proves for [`EvidenceLedger::summarize`].
#[test]
fn observed_identities_keeps_different_context_states_as_separate_rows() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let ledger = fixture.ledger();
    ledger
        .record(
            observation("anyrouter", "m").with_context_state(ContextState::Warm),
            1_000,
        )
        .unwrap();
    ledger
        .record(
            observation("anyrouter", "m").with_context_state(ContextState::Unknown),
            1_001,
        )
        .unwrap();

    let identities = ledger.observed_identities(10_000, 100_000, 50).unwrap();
    assert_eq!(
        identities.len(),
        2,
        "warm and unknown must not be blended into one row"
    );
    assert!(
        identities
            .iter()
            .any(|i| i.context_state == ContextState::Warm)
    );
    assert!(
        identities
            .iter()
            .any(|i| i.context_state == ContextState::Unknown)
    );
}

/// Capability map line 1661's own gap: a caller that only knows a
/// provider and model from configuration must still get a real
/// aggregate, without naming the route/harness/context-state
/// [`EvidenceLedger::summarize`] requires.
#[test]
fn summarize_latest_for_model_finds_the_real_identity_and_summarizes_it() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let ledger = fixture.ledger();

    for i in 0..MIN_SAMPLE_FOR_SUMMARY {
        let at = 1_000 + i as i64 * 10;
        let new = observation("anyrouter", "claude-opus-4-1")
            .with_timing(Some(at), Some(at + 2))
            .with_outcome(Outcome::Succeeded);
        ledger.record(new, at).unwrap();
    }

    let summary = ledger
        .summarize_latest_for_model("anyrouter", "claude-opus-4-1", 10_000, 100_000)
        .unwrap()
        .expect("an observed model must produce a summary");
    let median = summary
        .median_duration_ms
        .expect("five samples must produce a reading");
    assert_eq!(*median.value(), 2_000);
    assert_eq!(summary.provider, "anyrouter");
    assert_eq!(summary.model, "claude-opus-4-1");
}

/// A model nothing has ever recorded gets `Ok(None)`, distinct from a
/// [`RoutingSummary`] whose fields are all `None` below the minimum
/// sample — [`a_summary_below_the_minimum_sample_is_unknown`] proves the
/// latter.
#[test]
fn summarize_latest_for_model_is_none_when_nothing_was_ever_observed() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let ledger = fixture.ledger();

    let summary = ledger
        .summarize_latest_for_model("anyrouter", "claude-opus-4-1", 10_000, 100_000)
        .unwrap();
    assert!(summary.is_none());
}

/// Ruling 3: attributed to the named model, never a blend with a
/// differently-performing sibling.
#[test]
fn summarize_latest_for_model_never_blends_a_second_models_observations_in() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let ledger = fixture.ledger();

    for i in 0..MIN_SAMPLE_FOR_SUMMARY {
        let at = 1_000 + i as i64 * 10;
        ledger
            .record(
                observation("anyrouter", "cheap-model").with_timing(Some(at), Some(at + 2)),
                at,
            )
            .unwrap();
    }
    for i in 0..MIN_SAMPLE_FOR_SUMMARY {
        let at = 2_000 + i as i64 * 10;
        ledger
            .record(
                observation("anyrouter", "slow-model").with_timing(Some(at), Some(at + 500)),
                at,
            )
            .unwrap();
    }

    let cheap = ledger
        .summarize_latest_for_model("anyrouter", "cheap-model", 10_000, 100_000)
        .unwrap()
        .expect("cheap-model was observed");
    let slow = ledger
        .summarize_latest_for_model("anyrouter", "slow-model", 10_000, 100_000)
        .unwrap()
        .expect("slow-model was observed");
    assert_eq!(*cheap.median_duration_ms.unwrap().value(), 2_000);
    assert_eq!(*slow.median_duration_ms.unwrap().value(), 500_000);
}

/// Picks the most recently active `(route, harness, context_state)`
/// bucket rather than the first one it finds — observations recorded
/// under a different route earlier must not win over a more recent one
/// under the route this project actually uses now.
#[test]
fn summarize_latest_for_model_uses_the_most_recent_identitys_own_route_and_harness() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let ledger = fixture.ledger();

    for i in 0..MIN_SAMPLE_FOR_SUMMARY {
        let at = 1_000 + i as i64 * 10;
        ledger
            .record(
                NewObservation::new("anyrouter", "m")
                    .with_route(Some("old-route"))
                    .with_harness(Some("old-harness"))
                    .with_timing(Some(at), Some(at + 2)),
                at,
            )
            .unwrap();
    }
    for i in 0..MIN_SAMPLE_FOR_SUMMARY {
        let at = 5_000 + i as i64 * 10;
        ledger
            .record(
                NewObservation::new("anyrouter", "m")
                    .with_route(Some("new-route"))
                    .with_harness(Some("new-harness"))
                    .with_timing(Some(at), Some(at + 900)),
                at,
            )
            .unwrap();
    }

    let summary = ledger
        .summarize_latest_for_model("anyrouter", "m", 10_000, 100_000)
        .unwrap()
        .expect("m was observed");
    assert_eq!(summary.route.as_deref(), Some("new-route"));
    assert_eq!(
        *summary.median_duration_ms.unwrap().value(),
        900_000,
        "the most recently active identity's own observations must be summarized, \
             not the older route's"
    );
}

/// The identity-discovery step must itself filter by `model`: two models
/// sharing a provider, observed at the exact same timestamps so they tie
/// on `observed_at`, must never let one model's route leak into the
/// other's summary — the mutation this proof exists to kill drops
/// `AND model = ?3` from that lookup's own `WHERE` clause. Batch
/// overview-latency's own mutation run found this SURVIVED against every
/// test that gave both models the same route and harness (§80: a
/// SURVIVED that means "the fixture never varied the thing the mutation
/// touches" reads exactly like one that means "nothing watches this").
#[test]
fn summarize_latest_for_model_never_lets_a_tied_second_models_route_leak_in() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let ledger = fixture.ledger();

    for i in 0..MIN_SAMPLE_FOR_SUMMARY {
        let at = 1_000 + i as i64 * 10;
        ledger
            .record(
                NewObservation::new("anyrouter", "target-model")
                    .with_route(Some("route-a"))
                    .with_harness(Some("harness-a"))
                    .with_timing(Some(at), Some(at + 2)),
                at,
            )
            .unwrap();
        ledger
            .record(
                NewObservation::new("anyrouter", "other-model")
                    .with_route(Some("route-b"))
                    .with_harness(Some("harness-b"))
                    .with_timing(Some(at), Some(at + 900)),
                at,
            )
            .unwrap();
    }

    let summary = ledger
        .summarize_latest_for_model("anyrouter", "target-model", 10_000, 100_000)
        .unwrap()
        .expect("target-model was observed");
    assert_eq!(summary.route.as_deref(), Some("route-a"));
    assert_eq!(*summary.median_duration_ms.unwrap().value(), 2_000);
}

#[cfg(test)]
mod credential_throttle_tests {
    use super::*;

    fn row(
        provider: &str,
        account: Option<&str>,
        class: Option<FailureClass>,
    ) -> RoutingObservation {
        RoutingObservation {
            seq: 0,
            project_id: "project".to_owned(),
            observed_at_unix: 1_000,
            provider: provider.to_owned(),
            model: "m".to_owned(),
            route: Some("anthropic-messages".to_owned()),
            quota_context: account.map(str::to_owned),
            harness: Some("claude-code".to_owned()),
            purpose: None,
            dispatched_at_unix: Some(995),
            first_byte_at_unix: None,
            first_token_at_unix: None,
            first_tool_call_at_unix: None,
            completed_at_unix: Some(1_000),
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
            outcome: Some(if class.is_some() {
                Outcome::Failed
            } else {
                Outcome::Succeeded
            }),
            failure_class: class,
            task_class: None,
            session_id: None,
            effort_level: None,
            turn_shape: None,
            context_state: ContextState::Unknown,
        }
    }

    const THROTTLE: Option<FailureClass> = Some(FailureClass::Throttle);

    /// Map line 1965's per-account narrowing: every throttle row of the
    /// provider names its account, so each credential is counted its own
    /// rows and no other's — and another provider's throttles are not this
    /// provider's however many there are.
    #[test]
    fn every_row_naming_its_account_narrows_the_count_to_the_credential() {
        let rows = vec![
            row("alpha", Some("alpha/KEY_A"), THROTTLE),
            row("alpha", Some("alpha/KEY_A"), THROTTLE),
            row("alpha", Some("alpha/KEY_B"), THROTTLE),
            row("beta", None, THROTTLE),
            row("beta", None, THROTTLE),
        ];
        let counted = recent_credential_throttles(&rows, "alpha", Some("alpha/KEY_A"));
        assert_eq!(
            counted,
            CredentialThrottles {
                throttled: 2,
                account_narrowed: true,
            },
            "KEY_A's own rows, not KEY_B's and not beta's"
        );
        let sibling = recent_credential_throttles(&rows, "alpha", Some("alpha/KEY_B"));
        assert_eq!(sibling.throttled, 1);
        assert!(sibling.account_narrowed);
    }

    /// One context-less throttle row makes the whole reading provider-wide:
    /// a throttle no row attributes to an account cannot be subtracted from
    /// one, so the honest count is the provider's total.
    #[test]
    fn a_contextless_throttle_row_widens_the_reading_to_provider_scope() {
        let rows = vec![
            row("alpha", Some("alpha/KEY_A"), THROTTLE),
            row("alpha", None, THROTTLE),
        ];
        let counted = recent_credential_throttles(&rows, "alpha", Some("alpha/KEY_A"));
        assert_eq!(
            counted,
            CredentialThrottles {
                throttled: 2,
                account_narrowed: false,
            }
        );
    }

    /// Zero rows are a provider-wide zero — "none observed" is a statement
    /// about the provider's rows, never a per-account claim — and rows that
    /// are not informative throttles (a served exchange, a correlation
    /// probe's own row, a row with no outcome) contribute nothing.
    #[test]
    fn only_informative_throttles_count_and_zero_is_provider_wide() {
        let mut probe = row("alpha", Some("alpha/KEY_A"), THROTTLE);
        probe.purpose = Some(CORRELATION_PURPOSE.to_owned());
        let mut outcomeless = row("alpha", Some("alpha/KEY_A"), THROTTLE);
        outcomeless.outcome = None;
        let rows = vec![row("alpha", Some("alpha/KEY_A"), None), probe, outcomeless];
        let counted = recent_credential_throttles(&rows, "alpha", Some("alpha/KEY_A"));
        assert_eq!(
            counted,
            CredentialThrottles {
                throttled: 0,
                account_narrowed: false,
            }
        );
    }
}

/// Map line 1971's spend reader — [`recent_credential_spend`].
#[cfg(test)]
mod credential_spend_tests {
    use super::*;

    fn row(
        provider: &str,
        account: Option<&str>,
        tokens: Option<(i64, i64)>,
    ) -> RoutingObservation {
        RoutingObservation {
            seq: 0,
            project_id: "project".to_owned(),
            observed_at_unix: 1_000,
            provider: provider.to_owned(),
            model: "m".to_owned(),
            route: Some("anthropic-messages".to_owned()),
            quota_context: account.map(str::to_owned),
            harness: Some("claude-code".to_owned()),
            purpose: None,
            dispatched_at_unix: Some(995),
            first_byte_at_unix: None,
            first_token_at_unix: None,
            first_tool_call_at_unix: None,
            completed_at_unix: Some(1_000),
            first_byte_ms: None,
            first_token_ms: None,
            first_tool_call_ms: None,
            completed_ms: None,
            input_tokens: tokens.map(|(input, _)| input),
            output_tokens: tokens.map(|(_, output)| output),
            cached_input_tokens: Some(9_999),
            cost: None,
            tool_rounds: None,
            retries: None,
            repairs: None,
            failovers: None,
            outcome: Some(Outcome::Succeeded),
            failure_class: None,
            task_class: None,
            session_id: None,
            effort_level: None,
            turn_shape: None,
            context_state: ContextState::Unknown,
        }
    }

    /// Every counted row names its account, so the sum is this account's own
    /// — and it is input plus output and **not** the cached-input column,
    /// which providers disagree about.
    #[test]
    fn every_row_naming_its_account_narrows_the_sum_to_the_credential() {
        let rows = vec![
            row("alpha", Some("alpha/KEY_A"), Some((100, 20))),
            row("alpha", Some("alpha/KEY_A"), Some((5, 5))),
            row("alpha", Some("alpha/KEY_B"), Some((900, 900))),
            row("beta", Some("beta/KEY_A"), Some((1_000, 1_000))),
        ];
        assert_eq!(
            recent_credential_spend(&rows, "alpha", Some("alpha/KEY_A")),
            CredentialSpend {
                tokens: Some(130),
                account_narrowed: true,
                sample_count: 2,
            },
            "KEY_A's own rows on this provider, input plus output, and nothing else"
        );
    }

    /// One contextless counted row means the ledger holds spend nobody can
    /// attribute. The reading widens to provider scope rather than quietly
    /// dropping it: under-reporting is the direction that would let a
    /// ceiling be exceeded.
    #[test]
    fn a_contextless_counted_row_widens_the_reading_to_provider_scope() {
        let rows = vec![
            row("alpha", Some("alpha/KEY_A"), Some((100, 20))),
            row("alpha", None, Some((7, 3))),
        ];
        assert_eq!(
            recent_credential_spend(&rows, "alpha", Some("alpha/KEY_A")),
            CredentialSpend {
                tokens: Some(130),
                account_narrowed: false,
                sample_count: 2,
            }
        );
    }

    /// A row that carried no token count at all is not a zero. With no
    /// counted row anywhere the reading is `None` — unknown — which is what
    /// keeps a stated ceiling from being judged reached by a build that
    /// measured nothing.
    #[test]
    fn no_counted_row_reads_unknown_and_never_zero() {
        let rows = vec![
            row("alpha", Some("alpha/KEY_A"), None),
            row("alpha", Some("alpha/KEY_A"), None),
        ];
        assert_eq!(
            recent_credential_spend(&rows, "alpha", Some("alpha/KEY_A")),
            CredentialSpend {
                tokens: None,
                account_narrowed: false,
                sample_count: 0,
            }
        );

        // And an account with no rows of its own, beside a sibling that has
        // them, reads unknown rather than zero for the same reason.
        let rows = vec![row("alpha", Some("alpha/KEY_B"), Some((10, 10)))];
        assert_eq!(
            recent_credential_spend(&rows, "alpha", Some("alpha/KEY_A")).tokens,
            None
        );
    }

    /// This ledger's own bookkeeping is not spend, and neither is an
    /// exchange that never completed.
    #[test]
    fn correlation_rows_and_unfinished_exchanges_are_not_spend() {
        let mut correlation = row("alpha", Some("alpha/KEY_A"), Some((100, 100)));
        correlation.purpose = Some(CORRELATION_PURPOSE.to_owned());
        let mut unfinished = row("alpha", Some("alpha/KEY_A"), Some((100, 100)));
        unfinished.outcome = None;
        let rows = vec![
            correlation,
            unfinished,
            row("alpha", Some("alpha/KEY_A"), Some((1, 2))),
        ];
        assert_eq!(
            recent_credential_spend(&rows, "alpha", Some("alpha/KEY_A")).tokens,
            Some(3)
        );
    }
}

/// Map line 1519's spend reader — [`recent_credential_cost`].
#[cfg(test)]
mod credential_cost_tests {
    use super::*;

    fn row(
        provider: &str,
        model: &str,
        observed_at_unix: i64,
        tokens: Option<(i64, i64)>,
    ) -> RoutingObservation {
        RoutingObservation {
            seq: 0,
            project_id: "project".to_owned(),
            observed_at_unix,
            provider: provider.to_owned(),
            model: model.to_owned(),
            route: Some("anthropic-messages".to_owned()),
            quota_context: None,
            harness: Some("claude-code".to_owned()),
            purpose: None,
            dispatched_at_unix: Some(observed_at_unix - 5),
            first_byte_at_unix: None,
            first_token_at_unix: None,
            first_tool_call_at_unix: None,
            completed_at_unix: Some(observed_at_unix),
            first_byte_ms: None,
            first_token_ms: None,
            first_tool_call_ms: None,
            completed_ms: None,
            input_tokens: tokens.map(|(input, _)| input),
            output_tokens: tokens.map(|(_, output)| output),
            cached_input_tokens: None,
            cost: None,
            tool_rounds: None,
            retries: None,
            repairs: None,
            failovers: None,
            outcome: Some(Outcome::Succeeded),
            failure_class: None,
            task_class: None,
            session_id: None,
            effort_level: None,
            turn_shape: None,
            context_state: ContextState::Unknown,
        }
    }

    fn priced_table(dir: &std::path::Path) -> PriceTable {
        std::fs::write(
            dir.join("pricing.toml"),
            "[[prices]]\nprovider = \"alpha\"\nmodel = \"m\"\n\
             input_per_million_usd = 3.0\noutput_per_million_usd = 15.0\n",
        )
        .expect("write pricing.toml");
        PriceTable::load_from_dir(dir)
    }

    fn temp_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("temp pricing dir")
    }

    /// Two priced rows: `input * input_per_million_usd + output *
    /// output_per_million_usd`, summed in micro-USD, and the row counts land
    /// in the right buckets.
    #[test]
    fn priced_rows_sum_input_and_output_at_their_own_rates() {
        let dir = temp_dir();
        let prices = priced_table(dir.path());
        let rows = vec![
            // 1,000,000 input @ $3/M + 1,000,000 output @ $15/M = $18 = 18_000_000 micro-USD.
            row("alpha", "m", 1_000, Some((1_000_000, 1_000_000))),
            // 500,000 input @ $3/M + 0 output = $1.5 = 1_500_000 micro-USD.
            row("alpha", "m", 1_100, Some((500_000, 0))),
        ];
        let cost = recent_credential_cost(&rows, "alpha", None, &prices, 0);
        assert_eq!(cost.micro_usd, Some(19_500_000));
        assert_eq!(cost.priced_rows, 2);
        assert_eq!(cost.unread_rows, 0);
        assert_eq!(cost.unpriced_rows, 0);
    }

    /// A row with tokens and no `pricing.toml` entry is *unpriced*, and a
    /// row with no token count at all is *unread* — two different gaps, and
    /// neither contributes to the sum nor is treated as zero spend.
    #[test]
    fn unpriced_and_unread_rows_are_counted_apart_and_never_as_zero() {
        let dir = temp_dir();
        let prices = priced_table(dir.path());
        let rows = vec![
            row("alpha", "m", 1_000, Some((1_000_000, 0))), // priced: $3.
            row("alpha", "no-such-model", 1_001, Some((1_000_000, 0))), // unpriced.
            row("alpha", "m", 1_002, None),                 // unread (relayed).
        ];
        let cost = recent_credential_cost(&rows, "alpha", None, &prices, 0);
        assert_eq!(cost.micro_usd, Some(3_000_000));
        assert_eq!(cost.priced_rows, 1);
        assert_eq!(cost.unpriced_rows, 1);
        assert_eq!(cost.unread_rows, 1);
    }

    /// `priced_rows == 0` is the one condition that makes `micro_usd`
    /// `None` — a budget nobody could price against is *unknown* spend,
    /// never zero, even though this window is not otherwise empty.
    #[test]
    fn no_priced_row_leaves_micro_usd_none_even_with_other_rows_present() {
        let dir = temp_dir();
        let prices = priced_table(dir.path());
        let rows = vec![
            row("alpha", "no-such-model", 1_000, Some((1_000_000, 0))),
            row("alpha", "m", 1_001, None),
        ];
        let cost = recent_credential_cost(&rows, "alpha", None, &prices, 0);
        assert_eq!(cost.micro_usd, None);
        assert_eq!(cost.priced_rows, 0);
        assert_eq!(cost.unpriced_rows, 1);
        assert_eq!(cost.unread_rows, 1);
    }

    /// `since_unix` bounds the window this reader counts, independently of
    /// whatever the caller already fetched.
    #[test]
    fn rows_before_since_unix_are_excluded() {
        let dir = temp_dir();
        let prices = priced_table(dir.path());
        let rows = vec![
            row("alpha", "m", 500, Some((1_000_000, 0))), // before the window.
            row("alpha", "m", 1_500, Some((1_000_000, 0))), // inside it.
        ];
        let cost = recent_credential_cost(&rows, "alpha", None, &prices, 1_000);
        assert_eq!(cost.priced_rows, 1);
        assert_eq!(cost.micro_usd, Some(3_000_000));
    }
}
