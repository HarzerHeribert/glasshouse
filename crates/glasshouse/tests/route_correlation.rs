//! Phase 33C's route-correlation reader, exercised the way a caller outside
//! this crate reaches it: through a real `Runtime`, a real project database
//! on disk, and the shipped binary's own `glasshouse route` report.
//!
//! Behavioral contract: what the ledger has observed about two routes
//! failing at the same moments is read back with its sample size; below the
//! minimum it is *insufficient evidence* with the count; rows outside the
//! window contribute nothing; and `glasshouse route` prints the sample size
//! before any correlation reads as meaningful (capability map line 1376).
//! The production consumer — the gateway's own failover ranking — is proven
//! inside the crate that owns the accept loop
//! (`gateway::session::tests::observe_exchange_steers_a_real_failover_off_a_route_the_ledger_shows_failing_with_it`).

use std::path::{Path, PathBuf};
use std::process::Command;

use clap::Parser;

use glasshouse::routing::evidence::{
    CLASSIFICATION_EVIDENCE_WINDOW_SECONDS, CorrelationVerdict, EvidenceLedger, FailureClass,
    MIN_CORRELATION_SAMPLE, NewObservation, Outcome, RouteIdentity,
};
use glasshouse::{Cli, Runtime};

/// A bootstrapped project inside `base`, sharing `base`'s data and config
/// roots — the same idiom `tests/routing_evidence.rs` uses.
struct Fixture {
    base: PathBuf,
    root: PathBuf,
    runtime: Runtime,
}

impl Fixture {
    fn new(base: &Path) -> Self {
        let root = base.join("workspace");
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
            base: base.to_path_buf(),
            root,
            runtime,
        }
    }

    fn ledger(&self) -> EvidenceLedger {
        EvidenceLedger::open(&self.runtime).unwrap()
    }

    /// The shipped binary, pointed at this project.
    fn glasshouse(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args)
            .env(
                "GLASSHOUSE_ROUTE_CORRELATION_TEST_KEY",
                "planted-opaque-value",
            )
            .env("PATH", self.base.join("empty-path"))
            .output()
            .expect("the glasshouse binary must be runnable")
    }
}

fn exchange(provider: &str, start: i64, class: Option<FailureClass>) -> NewObservation {
    NewObservation::new(provider, "the-model")
        .with_route(Some("anthropic-messages"))
        .with_harness(Some("claude-code"))
        .with_timing(Some(start), Some(start + 5))
        .with_outcome(if class.is_some() {
            Outcome::Failed
        } else {
            Outcome::Succeeded
        })
        .with_failure_class(class)
}

fn route(provider: &str) -> RouteIdentity {
    RouteIdentity::new(provider, "the-model")
}

/// Plant, relative to `now`: five moments where `a` and `b` answered 5xx
/// together, two where `a` failed and `c` served, and one overlapping pair
/// of `a` and `d` from before the window.
fn plant(ledger: &EvidenceLedger, now: i64) {
    for i in 0..5 {
        let at = now - 3_600 + i * 300;
        ledger
            .record(exchange("a", at, Some(FailureClass::Upstream5xx)), at + 5)
            .unwrap();
        ledger
            .record(
                exchange("b", at + 10, Some(FailureClass::Upstream5xx)),
                at + 15,
            )
            .unwrap();
    }
    for i in 0..2 {
        let at = now - 1_800 + i * 300;
        ledger
            .record(exchange("a", at, Some(FailureClass::Upstream5xx)), at + 5)
            .unwrap();
        ledger
            .record(exchange("c", at + 10, None), at + 15)
            .unwrap();
    }
    let stale = now - CLASSIFICATION_EVIDENCE_WINDOW_SECONDS - 1_000;
    ledger
        .record(
            exchange("a", stale, Some(FailureClass::Upstream5xx)),
            stale + 5,
        )
        .unwrap();
    ledger
        .record(
            exchange("d", stale + 10, Some(FailureClass::Upstream5xx)),
            stale + 15,
        )
        .unwrap();
}

#[test]
fn correlations_are_read_from_a_real_ledger_with_their_sample_size_and_window() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let ledger = fixture.ledger();
    let now = 1_800_000_000_i64;
    plant(&ledger, now);

    let correlations = ledger
        .route_correlations(now, CLASSIFICATION_EVIDENCE_WINDOW_SECONDS)
        .unwrap();

    let ab = correlations.between(&route("a"), &route("b"));
    assert_eq!(
        ab.verdict(),
        CorrelationVerdict::Measured {
            confidence: 1.0,
            sample_size: 10,
        },
        "five moments, ten failure events, every one answered in kind: {ab:?}"
    );
    let ac = correlations.between(&route("c"), &route("a"));
    assert_eq!(
        ac.verdict(),
        CorrelationVerdict::InsufficientEvidence {
            sample_size: 2,
            required: MIN_CORRELATION_SAMPLE,
        },
        "two failures `c` served through are not enough to say anything, and the count is \
         carried rather than hidden: {ac:?}"
    );
    let ad = correlations.between(&route("a"), &route("d"));
    assert_eq!(
        ad.sample_size(),
        0,
        "an overlap from before the window is not read: {ad:?}"
    );
}

/// Line 1376 at the shipped binary: `glasshouse route` names the sample
/// size on every pair and calls a correlation measured only past the
/// minimum; line 1852's count prints with the honest zero when no failover
/// was steered.
#[test]
fn the_route_command_prints_every_pairs_sample_size_before_any_correlation() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());

    let config_dir = tmp.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let fake = tmp.path().join("fake-claude");
    std::fs::write(&fake, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let escaped = fake.display().to_string().replace('\\', "\\\\");
    std::fs::write(
        config_dir.join("config.toml"),
        format!(
            "version = 1\n\n\
             [integrations.claude-code]\nenabled = true\nexecutable = \"{escaped}\"\n\n\
             [providers.route-probe]\ntemplate = \"openrouter\"\n\
             credential_env = [\"GLASSHOUSE_ROUTE_CORRELATION_TEST_KEY\"]\n\n\
             [profiles.metered]\nharness = \"claude-code\"\n\
             expected_protocol = \"anthropic-messages\"\n\n\
             [profiles.metered.backend]\nkind = \"direct-provider\"\n\
             provider = \"route-probe\"\n"
        ),
    )
    .unwrap();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    plant(&fixture.ledger(), now);

    let output = fixture.glasshouse(&["route"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "route must succeed: status {:?}\nstdout:\n{stdout}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("Route correlations in this project, last 7 days"),
        "the section must be printed:\n{stdout}"
    );
    assert!(
        stdout.contains(
            "a/the-model and b/the-model: failed the same way at the same moment in 10 of 10 \
             overlapping observations — correlation 1.00"
        ),
        "a measured pair prints its sample size before its confidence:\n{stdout}"
    );
    assert!(
        stdout.contains(
            "a/the-model and c/the-model: insufficient evidence — 2 of the 5 overlapping \
             observations a correlation needs; treated as no correlation"
        ),
        "a pair below the minimum prints the count and says what the router does with it:\n\
         {stdout}"
    );
    assert!(
        !stdout.contains("d/the-model"),
        "a pair whose only overlap predates the window is not listed:\n{stdout}"
    );
    assert!(
        stdout.contains(
            "no gateway failover in this window was steered by a measured route correlation \
             (map line 1852)"
        ),
        "line 1852 prints an honest zero rather than nothing:\n{stdout}"
    );
}
