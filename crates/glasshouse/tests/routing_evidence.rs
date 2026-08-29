//! Phase 33A's routing evidence ledger, exercised the way a caller outside
//! this crate reaches it: through a real `Runtime`, a real project database
//! on disk, and nothing this crate keeps `pub(crate)`.
//!
//! Behavioral contract: an observation recorded through
//! [`glasshouse::routing::evidence::EvidenceLedger`] survives the process
//! that recorded it, is physically confined to the project it was recorded
//! against, contributes to a rolling summary only once enough of it exists,
//! and is never blended across the context-state boundary the capability map
//! forbids averaging away. This suite does not re-prove the gateway's own
//! production wiring — `crate::gateway::conformance`'s
//! `a_real_forwarded_exchange_reaches_the_routing_evidence_ledger` already
//! does that, mutation-proofed, inside the crate that owns the accept loop.
//! What only an external suite can catch is a mistake in *what this crate
//! actually exports* — a type or method quietly `pub(crate)` when a caller
//! outside the crate needs it public.

use std::path::Path;

use clap::Parser;

use glasshouse::config::pairing::ObservationSource;
use glasshouse::harness::WireProtocol;
use glasshouse::harness::pairing::{EvidenceKey, ServingRoute};
use glasshouse::integrations::IntegrationId;
use glasshouse::routing::AssignedModel;
use glasshouse::routing::evidence::{
    ContextState, EvidenceLedger, MIN_SAMPLE_FOR_SUMMARY, NewObservation, ObservationQuery,
    ObservedEvidenceSource, Outcome,
};
use glasshouse::{Cli, Runtime};

/// A bootstrapped project inside `base`, sharing `base`'s data and config
/// roots — the same idiom `tests/events_log.rs` and `src/checkpoint/store.rs`
/// use.
struct Fixture {
    base: std::path::PathBuf,
    root: std::path::PathBuf,
    runtime: Runtime,
}

impl Fixture {
    fn new(base: &Path, name: &str) -> Self {
        let root = base.join("workspace").join(name);
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        let runtime = bootstrap_at(base, &root);
        Fixture {
            base: base.to_path_buf(),
            root,
            runtime,
        }
    }

    /// Reopen the project through `bootstrap` again, exactly as a fresh
    /// process launch would.
    fn reopen(&self) -> Runtime {
        bootstrap_at(&self.base, &self.root)
    }
}

fn bootstrap_at(base: &Path, root: &Path) -> Runtime {
    let cli = Cli::try_parse_from([
        "glasshouse",
        "--data-dir",
        base.join("data").to_str().unwrap(),
        "--config-dir",
        base.join("config").to_str().unwrap(),
    ])
    .unwrap();
    glasshouse::bootstrap(&cli, root).unwrap()
}

fn synthetic_observation(at: i64, outcome: Outcome) -> NewObservation {
    synthetic_observation_for("claude-opus-4-1", at, outcome)
}

/// [`synthetic_observation`], for a model other than the default —
/// `"claude-opus-4-1"` and `"claude-sonnet-4-5"` sharing every other field so
/// a test can prove they are two distinct identities, not two names for the
/// same one.
fn synthetic_observation_for(model: &str, at: i64, outcome: Outcome) -> NewObservation {
    NewObservation::new("anyrouter", model)
        .with_route(Some("anthropic-messages"))
        .with_harness(Some("claude-code"))
        .with_timing(Some(at), Some(at + 2))
        .with_outcome(outcome)
}

/// A recorded observation is not process-local: a second `Runtime` bootstrapped
/// against the same project root reads it back, exactly as a resumed
/// Glasshouse session would.
#[test]
fn a_recorded_observation_survives_the_process_that_recorded_it() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");

    {
        let ledger = EvidenceLedger::open(&fixture.runtime).unwrap();
        ledger
            .record(synthetic_observation(1_000, Outcome::Succeeded), 1_000)
            .unwrap();
    }

    let reopened = fixture.reopen();
    let ledger = EvidenceLedger::open(&reopened).unwrap();
    let rows = ledger
        .recent(
            ObservationQuery {
                provider: "anyrouter",
                model: "claude-opus-4-1",
                route: Some("anthropic-messages"),
                harness: Some("claude-code"),
            },
            10,
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].outcome, Some(Outcome::Succeeded));
}

/// Two projects sharing one data root never see each other's observations —
/// the public half of migration 11's own isolation trigger.
#[test]
fn two_projects_never_share_a_routing_observation() {
    let tmp = tempfile::tempdir().unwrap();
    let alpha = Fixture::new(tmp.path(), "alpha");
    let beta = Fixture::new(tmp.path(), "beta");

    EvidenceLedger::open(&alpha.runtime)
        .unwrap()
        .record(synthetic_observation(1_000, Outcome::Succeeded), 1_000)
        .unwrap();

    let beta_rows = EvidenceLedger::open(&beta.runtime)
        .unwrap()
        .recent(
            ObservationQuery {
                provider: "anyrouter",
                model: "claude-opus-4-1",
                route: Some("anthropic-messages"),
                harness: Some("claude-code"),
            },
            10,
        )
        .unwrap();
    assert!(beta_rows.is_empty());
}

/// Capability map lines 1339 and 1340, driven from outside the crate: a
/// summary is a real, computed number once the minimum sample is met, and
/// its sample count and window agree with what was actually recorded.
#[test]
fn a_summary_reflects_exactly_the_observations_recorded_in_its_window() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let ledger = EvidenceLedger::open(&fixture.runtime).unwrap();

    for i in 0..MIN_SAMPLE_FOR_SUMMARY {
        let at = 1_000 + i as i64 * 100;
        let outcome = if i == 0 {
            Outcome::Failed
        } else {
            Outcome::Succeeded
        };
        ledger
            .record(synthetic_observation(at, outcome), at)
            .unwrap();
    }

    let summary = ledger
        .summarize(
            ObservationQuery {
                provider: "anyrouter",
                model: "claude-opus-4-1",
                route: Some("anthropic-messages"),
                harness: Some("claude-code"),
            },
            ContextState::Unknown,
            10_000,
            100_000,
        )
        .unwrap();
    let failure_rate = summary.failure_rate.expect("the minimum sample was met");
    assert_eq!(failure_rate.sample_count(), MIN_SAMPLE_FOR_SUMMARY);
    assert_eq!(
        *failure_rate.value(),
        1.0 / MIN_SAMPLE_FOR_SUMMARY as f64,
        "exactly one of the recorded outcomes was a failure"
    );
    let (window_start, window_end) = failure_rate.window();
    assert_eq!(window_start, 1_000);
    assert_eq!(
        window_end,
        1_000 + (MIN_SAMPLE_FOR_SUMMARY as i64 - 1) * 100
    );
}

/// Capability map line 1312's "recent": a failure recorded long before the
/// summarised window does not lower a failure rate that only covers what is
/// actually recent, even though the row is never deleted from the table.
#[test]
fn an_old_failure_does_not_contribute_to_a_recent_failure_rate() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let ledger = EvidenceLedger::open(&fixture.runtime).unwrap();

    for i in 0..MIN_SAMPLE_FOR_SUMMARY {
        let at = 1_000 + i as i64;
        ledger
            .record(synthetic_observation(at, Outcome::Failed), at)
            .unwrap();
    }
    let recent_start = 90_000;
    for i in 0..MIN_SAMPLE_FOR_SUMMARY {
        let at = recent_start + i as i64 * 100;
        ledger
            .record(synthetic_observation(at, Outcome::Succeeded), at)
            .unwrap();
    }

    let query = ObservationQuery {
        provider: "anyrouter",
        model: "claude-opus-4-1",
        route: Some("anthropic-messages"),
        harness: Some("claude-code"),
    };

    // Assert the premise (§17) before the absence: both blocks are really
    // in the table, and the old block is really recorded as failures.
    let all_rows = ledger.recent(query, 100).unwrap();
    assert_eq!(
        all_rows.len(),
        MIN_SAMPLE_FOR_SUMMARY * 2,
        "both the old failures and the recent successes were genuinely recorded"
    );
    assert_eq!(
        all_rows
            .iter()
            .filter(|o| o.outcome == Some(Outcome::Failed))
            .count(),
        MIN_SAMPLE_FOR_SUMMARY,
        "the old block is genuinely readable back as failures"
    );

    // A window that starts exactly at the first recent observation and ends
    // at `now_unix`, containing none of the old failures.
    let now_unix = recent_start + (MIN_SAMPLE_FOR_SUMMARY as i64 - 1) * 100;
    let window_seconds = now_unix - recent_start;
    let summary = ledger
        .summarize(query, ContextState::Unknown, now_unix, window_seconds)
        .unwrap();

    let failure_rate = summary.failure_rate.expect("the minimum sample was met");
    assert_eq!(
        *failure_rate.value(),
        0.0,
        "only the recent successes fall inside the window"
    );
    assert_eq!(failure_rate.sample_count(), MIN_SAMPLE_FOR_SUMMARY);
    let (window_start, _window_end) = failure_rate.window();
    assert_eq!(window_start, recent_start);
}

/// Capability map line 1312's "for gateway-backed resources": a summary is
/// per `(provider, model, route, harness)` identity, not per provider — the
/// same doctrine `routing::free`'s own header states for health.
#[test]
fn another_resources_failures_are_not_this_resources_failures() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let ledger = EvidenceLedger::open(&fixture.runtime).unwrap();

    for i in 0..MIN_SAMPLE_FOR_SUMMARY {
        let at = 1_000 + i as i64 * 100;
        ledger
            .record(synthetic_observation(at, Outcome::Succeeded), at)
            .unwrap();
    }
    for i in 0..MIN_SAMPLE_FOR_SUMMARY {
        let at = 1_000 + i as i64 * 100;
        ledger
            .record(
                synthetic_observation_for("claude-sonnet-4-5", at, Outcome::Failed),
                at,
            )
            .unwrap();
    }

    let now_unix = 10_000;
    let window_seconds = 100_000;

    let sonnet_query = ObservationQuery {
        provider: "anyrouter",
        model: "claude-sonnet-4-5",
        route: Some("anthropic-messages"),
        harness: Some("claude-code"),
    };
    // Assert the premise: the other identity's failures are genuinely
    // recorded and genuinely visible before asserting this identity can't
    // see them.
    let sonnet_summary = ledger
        .summarize(
            sonnet_query,
            ContextState::Unknown,
            now_unix,
            window_seconds,
        )
        .unwrap();
    let sonnet_failure_rate = sonnet_summary
        .failure_rate
        .expect("the minimum sample was met");
    assert_eq!(
        *sonnet_failure_rate.value(),
        1.0,
        "every claude-sonnet-4-5 observation recorded was a failure"
    );

    let opus_query = ObservationQuery {
        provider: "anyrouter",
        model: "claude-opus-4-1",
        route: Some("anthropic-messages"),
        harness: Some("claude-code"),
    };
    let opus_summary = ledger
        .summarize(opus_query, ContextState::Unknown, now_unix, window_seconds)
        .unwrap();
    let opus_failure_rate = opus_summary
        .failure_rate
        .expect("the minimum sample was met");
    assert_eq!(
        *opus_failure_rate.value(),
        0.0,
        "the sonnet identity's failures never contribute to the opus identity's rate"
    );
}

/// [`ObservedEvidenceSource`] — design decision 6's replacement for
/// `NoObservations` — reachable and correct from outside the crate,
/// against a real [`EvidenceKey`] built the way `crate::config::pairing`
/// would build one.
#[test]
fn observed_evidence_source_is_reachable_from_outside_the_crate() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let ledger = EvidenceLedger::open(&fixture.runtime).unwrap();

    for i in 0..MIN_SAMPLE_FOR_SUMMARY {
        let at = 1_000 + i as i64;
        ledger
            .record(synthetic_observation(at, Outcome::Succeeded), at)
            .unwrap();
    }

    let key = EvidenceKey::new(
        IntegrationId::ClaudeCode,
        "default",
        AssignedModel::named("claude-opus-4-1"),
        ServingRoute {
            provider: Some("anyrouter".to_owned()),
            gateway: None,
            protocol: Some(WireProtocol::AnthropicMessages),
        },
    );
    let source = ObservedEvidenceSource::new(&ledger, 10_000, 100_000);
    let observed = source
        .observed(&key)
        .expect("five successes must produce evidence");
    assert_eq!(observed.task_success_rate, Some(1.0));
}

/// The reachability test above only ever records successes, so it cannot
/// show that a recorded failure moves [`ObservedEvidence::task_success_rate`]
/// at all — a bug that always reported `1.0` would still pass it. This proves
/// the fraction actually reflects recorded failures, not just presence.
#[test]
fn observed_evidence_source_reflects_recorded_failures() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let ledger = EvidenceLedger::open(&fixture.runtime).unwrap();

    for i in 0..MIN_SAMPLE_FOR_SUMMARY {
        let at = 1_000 + i as i64 * 100;
        ledger
            .record(synthetic_observation(at, Outcome::Succeeded), at)
            .unwrap();
    }
    for i in 0..MIN_SAMPLE_FOR_SUMMARY {
        let at = 1_000 + (MIN_SAMPLE_FOR_SUMMARY as i64 + i as i64) * 100;
        ledger
            .record(synthetic_observation(at, Outcome::Failed), at)
            .unwrap();
    }

    let key = EvidenceKey::new(
        IntegrationId::ClaudeCode,
        "default",
        AssignedModel::named("claude-opus-4-1"),
        ServingRoute {
            provider: Some("anyrouter".to_owned()),
            gateway: None,
            protocol: Some(WireProtocol::AnthropicMessages),
        },
    );
    let source = ObservedEvidenceSource::new(&ledger, 10_000, 100_000);
    let observed = source
        .observed(&key)
        .expect("ten observations must produce evidence");
    assert_eq!(
        observed.task_success_rate,
        Some(0.5),
        "half of the recorded observations were failures"
    );
    assert!(
        observed.reliable_observation_count > 0,
        "the recorded observations must actually count toward reliability"
    );
}

/// Batch 43's `observed_identities` — the enumeration link batch 42 found
/// missing (practice §71) — reachable and correct from outside the crate:
/// [`EvidenceLedger::recent`] and [`EvidenceLedger::summarize`] both require
/// the caller to already name an identity, and this is the one public method
/// that answers which identities this project has actually recorded.
#[test]
fn observed_identities_is_reachable_from_outside_the_crate_and_returns_real_rows() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let ledger = EvidenceLedger::open(&fixture.runtime).unwrap();

    ledger
        .record(synthetic_observation(1_000, Outcome::Succeeded), 1_000)
        .unwrap();
    ledger
        .record(synthetic_observation(1_001, Outcome::Succeeded), 1_001)
        .unwrap();
    ledger
        .record(NewObservation::new("openai-router", "gpt-5"), 1_002)
        .unwrap();

    let identities = ledger.observed_identities(10_000, 100_000, 50).unwrap();
    assert_eq!(identities.len(), 2, "two distinct identities were recorded");

    let anyrouter = identities
        .iter()
        .find(|i| i.provider == "anyrouter")
        .expect("anyrouter identity");
    assert_eq!(anyrouter.model, "claude-opus-4-1");
    assert_eq!(anyrouter.route.as_deref(), Some("anthropic-messages"));
    assert_eq!(anyrouter.context_state, ContextState::Unknown);
    assert_eq!(anyrouter.sample_count(), 2);

    let bounded = ledger.observed_identities(10_000, 100_000, 1).unwrap();
    assert_eq!(
        bounded.len(),
        1,
        "the limit must be honored from outside the crate too"
    );
}

/// The public half of migration 11's own isolation trigger, for
/// `observed_identities` specifically: two projects sharing one data root
/// never see each other's identities.
#[test]
fn observed_identities_is_project_scoped_from_outside_the_crate() {
    let tmp = tempfile::tempdir().unwrap();
    let alpha = Fixture::new(tmp.path(), "alpha");
    let beta = Fixture::new(tmp.path(), "beta");

    EvidenceLedger::open(&alpha.runtime)
        .unwrap()
        .record(synthetic_observation(1_000, Outcome::Succeeded), 1_000)
        .unwrap();

    let beta_identities = EvidenceLedger::open(&beta.runtime)
        .unwrap()
        .observed_identities(10_000, 100_000, 50)
        .unwrap();
    assert!(beta_identities.is_empty());
}
