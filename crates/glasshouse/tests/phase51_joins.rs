//! Phase 51 — the read-side join replaying what the router estimated
//! against what actually happened.
//!
//! - **1836** *"Measure the accuracy of estimated subscription headroom
//!   against observed throttling and resets."*
//!
//! Map lines 1854 and 1855 were also this file's, but the routing deletion
//! (design-decisions, 2026-09-16) removed both of their production entry
//! points: 1855's `EvaluationKind::RoutingConsumptionEstimated` and
//! `EvidenceLedger::output_estimate_accuracy` have no reader or writer left,
//! and 1854's `glasshouse route` rendering command is gone. Their tests went
//! with them.
//!
//! Practice §35 decides which half of 1836 is proved through the shipped
//! binary and which is proved directly: the replay is a pure reader over
//! rows this test can hand it directly (like `estimate_subscription_headroom`
//! itself is tested in `tests/subscription_estimator.rs`), so most of 1836
//! plants rows straight into the ledger. The rendering test (the pool view)
//! runs the shipped binary because that is the only thing that proves the
//! reader and the render are actually wired together.

use std::path::{Path, PathBuf};
use std::process::Command;

use clap::Parser;

use glasshouse::routing::evidence::{
    CLASSIFICATION_EVIDENCE_WINDOW_SECONDS, EvidenceLedger, FailureClass, MIN_SAMPLE_FOR_SUMMARY,
    NewObservation as RoutingNewObservation, Outcome,
};
use glasshouse::{Cli, Runtime};

// Splitting a pre-2026-09-11 fixture into Glasshouse's `config.toml` and the
// gateway's `gateway.toml` — see the included file for what moves and why.
include!("fixtures/gateway_split.rs");

const CREDENTIAL_VAR: &str = "GLASSHOUSE_PHASE51_JOINS_KEY";
const CREDENTIAL: &str = "sk-fabricated-test-value-not-a-real-credential";
const PROVIDER: &str = "phase51-joins-probe";
const MODEL: &str = "phase51-joins-probe/a-model";

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn now_unix() -> i64 {
    glasshouse::provider::cache::now_unix_seconds()
}

fn both_streams(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[cfg(unix)]
fn install_fake_harness(bin_dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = bin_dir.join("fake-claude-code");
    std::fs::write(&path, "#!/bin/sh\nexit 0\n").expect("write fake harness");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

#[cfg(windows)]
fn install_fake_harness(bin_dir: &Path) -> PathBuf {
    let path = bin_dir.join("fake-claude-code.cmd");
    std::fs::write(&path, "@echo off\r\nexit /b 0\r\n").expect("write fake harness");
    path
}

/// A project wired with a fake harness, one direct-provider profile and one
/// entitlement over it — enough to launch, to route, and to render
/// `glasshouse entitlements`. Modelled on `tests/subscription_estimator.rs`'s
/// and `tests/evaluation_producers.rs`'s own fixtures.
struct Fixture {
    base: PathBuf,
    runtime: Runtime,
}

impl Fixture {
    fn new(base: &Path) -> Self {
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        let root = std::fs::canonicalize(&root).expect("canonicalize project root");

        let bin_dir = base.join("bin");
        std::fs::create_dir_all(&bin_dir).expect("create bin dir");
        let harness = install_fake_harness(&bin_dir);
        let escaped = harness.display().to_string().replace('\\', "\\\\");

        let config_dir = base.join("config");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        // The accounts are the gateway's since the 2026-09-11 ruling; this
        // fixture writes both halves so the binary reads what it used to.
        let (config_text, gateway_text) = split_gateway_state(&format!(
            "version = 1\n\n\
                 [integrations.claude-code]\nenabled = true\nexecutable = \"{escaped}\"\n\n\
                 [providers.{PROVIDER}]\ntemplate = \"anthropic-compatible\"\n\
                 base_url = \"http://127.0.0.1:9/\"\n\
                 credential_env = [\"{CREDENTIAL_VAR}\"]\n\n\
                 [profiles.metered]\nharness = \"claude-code\"\nmodel = \"{MODEL}\"\n\n\
                 [profiles.metered.backend]\nkind = \"direct-provider\"\nprovider = \"{PROVIDER}\"\n\n\
                 [entitlements.acct]\nkind = \"claude\"\nvendor = \"claude\"\n\
                 provider = \"{PROVIDER}\"\ncredential = {{ env = \"{CREDENTIAL_VAR}\" }}\n"
        ));
        std::fs::write(config_dir.join("config.toml"), config_text).expect("write user config");
        std::fs::write(config_dir.join("gateway.toml"), gateway_text).expect("write user config");

        let cli = Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            base.join("data").to_str().unwrap(),
            "--config-dir",
            config_dir.to_str().unwrap(),
        ])
        .unwrap();
        let runtime = glasshouse::bootstrap(&cli, &root).unwrap();

        Fixture {
            base: base.to_path_buf(),
            runtime,
        }
    }

    fn data_dir(&self) -> PathBuf {
        self.base.join("data")
    }

    fn glasshouse(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .current_dir(self.runtime.project().root())
            .env(CREDENTIAL_VAR, CREDENTIAL)
            .arg("--data-dir")
            .arg(self.data_dir())
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args)
            .output()
            .expect("the glasshouse binary must run")
    }

    fn evidence_ledger(&self) -> EvidenceLedger {
        EvidenceLedger::open(&self.runtime).unwrap()
    }
}

fn accepted() -> RoutingNewObservation {
    RoutingNewObservation::new(PROVIDER, MODEL).with_outcome(Outcome::Succeeded)
}

fn throttle() -> RoutingNewObservation {
    RoutingNewObservation::new(PROVIDER, MODEL)
        .with_outcome(Outcome::Failed)
        .with_failure_class(Some(FailureClass::Throttle))
}

fn exhausted() -> RoutingNewObservation {
    RoutingNewObservation::new(PROVIDER, MODEL)
        .with_outcome(Outcome::Failed)
        .with_failure_class(Some(FailureClass::ExhaustedQuota))
}

// ===========================================================================
// 1836 — the estimator replayed against its own provider's history.
// ===========================================================================

/// **(a), first case.** Three accepted rows are real evidence of headroom;
/// the throttle that follows them replays as `Ample`, which is a miss.
#[test]
fn test_1836_a_throttle_after_only_accepted_activity_is_missed() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path());
    let ledger = fixture.evidence_ledger();
    let now = now_unix();
    for i in 0..3 {
        let at = now - 300 + i * 60;
        ledger.record(accepted(), at).unwrap();
    }
    let throttle_at = now - 60;
    ledger.record(throttle(), throttle_at).unwrap();

    let replay = ledger
        .headroom_replay(PROVIDER, now, CLASSIFICATION_EVIDENCE_WINDOW_SECONDS)
        .unwrap();
    assert_eq!(replay.missed, 1, "{replay:?}");
    assert_eq!(replay.warned, 0, "{replay:?}");
    assert_eq!(replay.unestimable, 0, "{replay:?}");
}

/// **(a), second and third cases together.** A provider's very first
/// recorded throttle has no prior row for the estimator to read at all —
/// `unestimable`. The exhaustion 60s later replays against a window that
/// already holds that first throttle, recent enough to read as live
/// pressure — `warned`.
#[test]
fn test_1836_a_first_ever_throttle_is_unestimable_and_the_exhaustion_after_it_is_warned() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path());
    let ledger = fixture.evidence_ledger();
    let now = now_unix();
    let throttle_at = now - 120;
    ledger.record(throttle(), throttle_at).unwrap();
    let exhausted_at = now - 60;
    ledger.record(exhausted(), exhausted_at).unwrap();

    let replay = ledger
        .headroom_replay(PROVIDER, now, CLASSIFICATION_EVIDENCE_WINDOW_SECONDS)
        .unwrap();
    assert_eq!(
        replay.unestimable, 1,
        "the first throttle this provider ever recorded has no prior row at all: {replay:?}"
    );
    assert_eq!(
        replay.warned, 1,
        "the exhaustion replays against a window already holding that first throttle: {replay:?}"
    );
    assert_eq!(replay.missed, 0, "{replay:?}");
}

/// **(a), the reset-lag figure.** A throttle followed 90s later by an
/// accepted row reports that lag, over one sample.
#[test]
fn test_1836_a_throttle_followed_by_an_accepted_row_reports_the_observed_reset_lag() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path());
    let ledger = fixture.evidence_ledger();
    let now = now_unix();
    let throttle_at = now - 200;
    ledger.record(throttle(), throttle_at).unwrap();
    let recovery_at = throttle_at + 90;
    ledger.record(accepted(), recovery_at).unwrap();

    let replay = ledger
        .headroom_replay(PROVIDER, now, CLASSIFICATION_EVIDENCE_WINDOW_SECONDS)
        .unwrap();
    assert_eq!(replay.observed_reset_lag_sample_count, 1, "{replay:?}");
    assert_eq!(
        replay.observed_reset_lag_median_seconds,
        Some(90),
        "{replay:?}"
    );
}

/// **(a), the floor.** Below `MIN_SAMPLE_FOR_SUMMARY` throttles, the pool
/// view says so rather than printing a count nobody would trust.
#[test]
fn test_1836_below_the_throttle_floor_the_pool_view_says_not_enough_to_score() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path());
    let ledger = fixture.evidence_ledger();
    let now = now_unix();
    // Two throttles, well below `MIN_SAMPLE_FOR_SUMMARY` (5).
    for i in 0..2 {
        let at = now - 300 + i * 60;
        ledger.record(throttle(), at).unwrap();
    }

    let out = fixture.glasshouse(&["entitlements"]);
    assert!(out.status.success(), "{}", both_streams(&out));
    let printed = both_streams(&out);
    assert!(
        printed.contains("headroom estimate vs throttles (1836): not enough throttles to score"),
        "got:\n{printed}"
    );
}

/// **Readout wiring.** At or above the floor, `glasshouse entitlements`
/// prints exactly the counts `headroom_replay` itself computes over the
/// same provider and window — proving the render reads the replay rather
/// than a second, possibly different, computation.
#[test]
fn test_1836_the_replayed_counts_reach_the_pool_view_verbatim() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path());
    let ledger = fixture.evidence_ledger();
    let now = now_unix();
    for i in 0..3 {
        let at = now - 2000 + i * 30;
        ledger.record(accepted(), at).unwrap();
    }
    let t1 = now - 1800;
    ledger.record(throttle(), t1).unwrap();
    let t2 = now - 1700;
    ledger.record(exhausted(), t2).unwrap();
    let t3 = now - 1600;
    ledger.record(throttle(), t3).unwrap();
    let recovery = t3 + 100;
    ledger.record(accepted(), recovery).unwrap();
    let t4 = now - 900;
    ledger.record(exhausted(), t4).unwrap();
    let t5 = now - 800;
    ledger.record(throttle(), t5).unwrap();

    let expected = ledger
        .headroom_replay(PROVIDER, now, CLASSIFICATION_EVIDENCE_WINDOW_SECONDS)
        .unwrap();
    assert!(
        expected.throttles() >= MIN_SAMPLE_FOR_SUMMARY,
        "premise: at least {MIN_SAMPLE_FOR_SUMMARY} throttles must be seeded to exercise the \
         scored branch rather than the floor: {expected:?}"
    );
    let reset_clause = match expected.observed_reset_lag_median_seconds {
        Some(seconds) => format!(
            "observed reset lag median {seconds}s over {}",
            expected.observed_reset_lag_sample_count
        ),
        None => "no observed resets".to_owned(),
    };
    let expected_line = format!(
        "headroom estimate vs throttles (1836): warned {} / missed {} / unestimable {} of {} \
         throttles; {reset_clause}",
        expected.warned,
        expected.missed,
        expected.unestimable,
        expected.throttles()
    );

    let out = fixture.glasshouse(&["entitlements"]);
    assert!(out.status.success(), "{}", both_streams(&out));
    let printed = both_streams(&out);
    assert!(
        printed.contains(&expected_line),
        "got:\n{printed}\nexpected line:\n{expected_line}"
    );
}
