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
    NewObservation::new("anyrouter", "claude-opus-4-1")
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
