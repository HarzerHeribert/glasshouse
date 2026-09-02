//! Phase 51 — the read-side joins linking what the router estimated to what
//! actually happened.
//!
//! - **1836** *"Measure the accuracy of estimated subscription headroom
//!   against observed throttling and resets."*
//! - **1855** *"Measure estimated versus actual marginal token or request
//!   consumption when telemetry permits."* — the token half.
//! - **1854** *"Measure how often sparse, stale, or incorrectly segmented
//!   evidence causes a poor routing decision."* — proving the *by evidence
//!   held* rendering carries `observed-stale` and `absent` with their own
//!   success counts, beside `tests/evaluation_producers.rs`'s own
//!   stale/absent producer proof.
//!
//! Practice §35 decides which half of each line is proved through the
//! shipped binary and which is proved directly: 1836's replay and 1855's
//! join are pure readers over rows this test can hand them directly (like
//! `estimate_subscription_headroom` itself is tested in
//! `tests/subscription_estimator.rs`), so most of 1836 and one 1855 test
//! plant rows straight into the ledger. What only a launch can write — the
//! 1855 producer call, and 1854's real staleness computation off a real
//! gateway-health reading — goes through `glasshouse launch` and
//! `glasshouse hook`. The two rendering tests (1836's pool view, 1855's
//! route-outcomes block) run the shipped binary because that is the only
//! thing that proves the reader and the render are actually wired together.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use clap::Parser;

use glasshouse::evaluation::{
    EvaluationKind, EvaluationObservations, HEALTH_EVIDENCE_HORIZON_SECONDS,
    NewObservation as EvalNewObservation,
};
use glasshouse::provider::telemetry::{GatewayHealthCache, GatewayHealthReading};
use glasshouse::routing::evidence::{
    CLASSIFICATION_EVIDENCE_WINDOW_SECONDS, EvidenceLedger, FailureClass, HARNESS_TURN_PURPOSE,
    MIN_SAMPLE_FOR_SUMMARY, NewObservation as RoutingNewObservation, Outcome,
};
use glasshouse::routing::request::TaskClass;
use glasshouse::{Cli, Runtime};

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
        std::fs::write(
            config_dir.join("config.toml"),
            format!(
                "version = 1\n\n\
                 [integrations.claude-code]\nenabled = true\nexecutable = \"{escaped}\"\n\n\
                 [providers.{PROVIDER}]\ntemplate = \"anthropic-compatible\"\n\
                 base_url = \"http://127.0.0.1:9/\"\n\
                 credential_env = [\"{CREDENTIAL_VAR}\"]\n\n\
                 [profiles.metered]\nharness = \"claude-code\"\nmodel = \"{MODEL}\"\n\n\
                 [profiles.metered.backend]\nkind = \"direct-provider\"\nprovider = \"{PROVIDER}\"\n\n\
                 [entitlements.acct]\nkind = \"claude\"\nvendor = \"claude\"\n\
                 provider = \"{PROVIDER}\"\ncredential = {{ env = \"{CREDENTIAL_VAR}\" }}\n"
            ),
        )
        .expect("write user config");

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

    /// Run `glasshouse hook`, exactly as a harness runs it: a separate
    /// process, the event on argv, a payload on standard input.
    fn hook(&self, session: &str, event: &str) {
        use std::io::Write as _;

        let mut child = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .current_dir(self.runtime.project().root())
            .env(CREDENTIAL_VAR, CREDENTIAL)
            .arg("--data-dir")
            .arg(self.data_dir())
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .arg("hook")
            .arg("--session")
            .arg(session)
            .arg("--event")
            .arg(event)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the glasshouse binary must be runnable");
        child
            .stdin
            .as_mut()
            .expect("stdin was piped")
            .write_all(b"{\"prompt\":\"PHASE51-JOINS-HOOK-MARKER\"}")
            .expect("write the hook payload");
        let output = child.wait_with_output().expect("the hook must exit");
        assert!(
            output.status.success(),
            "a hook always exits zero:\n{}",
            both_streams(&output)
        );
    }

    /// Launch, and return the id of the one new session it created.
    fn launch(&self, args: &[&str]) -> String {
        let before = self.session_ids();
        let mut argv = vec![
            "launch",
            "claude-code",
            "--headless",
            "--profile",
            "metered",
        ];
        argv.extend_from_slice(args);
        let launched = self.glasshouse(&argv);
        assert!(
            launched.status.success(),
            "the launch must succeed:\n{}",
            both_streams(&launched)
        );
        let mut created: Vec<String> = self
            .session_ids()
            .into_iter()
            .filter(|id| !before.contains(id))
            .collect();
        assert_eq!(
            created.len(),
            1,
            "one launch, one session; before: {before:?}"
        );
        created.remove(0)
    }

    fn db(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.runtime.database_path()).unwrap()
    }

    fn session_ids(&self) -> Vec<String> {
        let conn = self.db();
        let mut statement = conn.prepare("SELECT id FROM sessions").unwrap();
        statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    fn evidence_ledger(&self) -> EvidenceLedger {
        EvidenceLedger::open(&self.runtime).unwrap()
    }

    fn eval_ledger(&self) -> EvaluationObservations {
        EvaluationObservations::open(&self.runtime).unwrap()
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

// ===========================================================================
// 1855 — expected versus actual output tokens.
// ===========================================================================

/// A task text `classify_heuristically` reads as code modification, with no
/// routing model configured — the same heuristic path
/// `tests/route_rationale.rs`'s launches already prove records a real
/// rationale.
const CODE_MODIFICATION_TASK: &str = "fix the bug in this file";

/// **(b).** A launch whose task class has comparable rows in the window
/// records an estimate naming that class and the real median — and until a
/// routing row lands on that session, the join counts it as pending, never
/// as a zero.
#[test]
fn test_1855_a_launch_with_comparable_rows_records_an_estimate_and_tracks_it_as_pending() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path());
    let evidence_ledger = fixture.evidence_ledger();
    let now = now_unix();

    // Five comparable rows for `code modification`, medians to 1020.
    for (i, output_tokens) in [1000, 1010, 1020, 1030, 1040].into_iter().enumerate() {
        let at = now - 500 + (i as i64) * 20;
        evidence_ledger
            .record(
                RoutingNewObservation::new(PROVIDER, MODEL)
                    .with_purpose(Some(HARNESS_TURN_PURPOSE))
                    .with_task_class(Some(TaskClass::CodeModification))
                    .with_tokens(None, Some(output_tokens), None)
                    .with_outcome(Outcome::Succeeded),
                at,
            )
            .unwrap();
    }

    let session = fixture.launch(&["--task", CODE_MODIFICATION_TASK]);

    let eval_ledger = fixture.eval_ledger();
    let rows = eval_ledger
        .recent_of_kind(EvaluationKind::RoutingConsumptionEstimated, 10)
        .unwrap();
    let row = rows
        .iter()
        .find(|row| row.session_id.as_deref() == Some(session.as_str()))
        .unwrap_or_else(|| panic!("the launch must have recorded an estimate row: {rows:?}"));
    assert_eq!(row.subject.as_deref(), Some("code modification"), "{row:?}");
    assert_eq!(
        row.detail.as_deref(),
        Some("1020"),
        "the median of 1000/1010/1020/1030/1040 is the middle value: {row:?}"
    );

    // Pending: no routing row has landed on this session yet.
    let joined = evidence_ledger
        .output_estimate_accuracy(now_unix(), CLASSIFICATION_EVIDENCE_WINDOW_SECONDS)
        .unwrap();
    let class_row = joined
        .iter()
        .find(|row| row.task_class == "code modification")
        .unwrap_or_else(|| panic!("the class must appear: {joined:?}"));
    assert_eq!(class_row.pending, 1, "{class_row:?}");
    assert_eq!(class_row.sample_count, 0, "{class_row:?}");
    assert_eq!(
        class_row.median_ratio, None,
        "a session with no actual yet must never read as a zero ratio: {class_row:?}"
    );

    // A routing row lands on the same session: pending clears.
    let after = now_unix() + 5;
    evidence_ledger
        .record(
            RoutingNewObservation::new(PROVIDER, MODEL)
                .with_purpose(Some(HARNESS_TURN_PURPOSE))
                .with_session_id(Some(session.clone()))
                .with_tokens(None, Some(1200), None)
                .with_outcome(Outcome::Succeeded),
            after,
        )
        .unwrap();
    let joined = evidence_ledger
        .output_estimate_accuracy(now_unix() + 10, CLASSIFICATION_EVIDENCE_WINDOW_SECONDS)
        .unwrap();
    let class_row = joined
        .iter()
        .find(|row| row.task_class == "code modification")
        .unwrap_or_else(|| panic!("the class must appear: {joined:?}"));
    assert_eq!(class_row.pending, 0, "{class_row:?}");
    assert_eq!(class_row.sample_count, 1, "{class_row:?}");
}

/// **(b).** A launch whose task class has no comparable rows in the window
/// records no estimate at all — never a fabricated zero.
#[test]
fn test_1855_a_launch_with_no_comparable_rows_records_no_estimate() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path());

    let session = fixture.launch(&["--task", CODE_MODIFICATION_TASK]);

    let eval_ledger = fixture.eval_ledger();
    let rows = eval_ledger
        .recent_of_kind(EvaluationKind::RoutingConsumptionEstimated, 10)
        .unwrap();
    assert!(
        rows.iter()
            .all(|row| row.session_id.as_deref() != Some(session.as_str())),
        "a class with no comparable rows in the window must record nothing: {rows:?}"
    );
}

/// **The join's arithmetic**, over rows planted directly — the same
/// precedent `tests/evaluation_producers.rs`'s own header states for
/// "arithmetic over a window a launch cannot place rows in": five sessions'
/// estimate and actual rows, joined and medianed by `session_id`.
#[test]
fn test_1855_the_median_ratio_is_computed_per_session_and_crosses_the_floor() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path());
    let evidence_ledger = fixture.evidence_ledger();
    let eval_ledger = fixture.eval_ledger();
    let now = now_unix();

    // Ratios .8, .9, 1.0, 1.1, 1.2 against a shared estimate of 1000 —
    // median 1.0.
    for (i, actual) in [800, 900, 1000, 1100, 1200].into_iter().enumerate() {
        let session_id = format!("phase51-joins-ratio-session-{i}");
        let at = now - 600 + (i as i64) * 10;
        eval_ledger
            .record(
                EvalNewObservation::new(EvaluationKind::RoutingConsumptionEstimated)
                    .with_subject(TaskClass::CodeModification.as_str())
                    .with_session_id(session_id.clone())
                    .with_detail("1000"),
                at,
            )
            .unwrap();
        evidence_ledger
            .record(
                RoutingNewObservation::new(PROVIDER, MODEL)
                    .with_purpose(Some(HARNESS_TURN_PURPOSE))
                    .with_session_id(Some(session_id))
                    .with_tokens(None, Some(actual), None)
                    .with_outcome(Outcome::Succeeded),
                at + 5,
            )
            .unwrap();
    }

    let rows = evidence_ledger
        .output_estimate_accuracy(now, CLASSIFICATION_EVIDENCE_WINDOW_SECONDS)
        .unwrap();
    let row = rows
        .iter()
        .find(|row| row.task_class == "code modification")
        .unwrap_or_else(|| panic!("the class must appear: {rows:?}"));
    assert_eq!(row.sample_count, 5, "{row:?}");
    assert_eq!(row.pending, 0, "{row:?}");
    assert_eq!(
        row.median_ratio,
        Some(1.0),
        "the middle ratio of .8/.9/1.0/1.1/1.2 is 1.0: {row:?}"
    );
}

// ===========================================================================
// 1854 — the `by evidence held` block carries `observed-stale` and `absent`
// with their own success counts.
// ===========================================================================

/// **(c).** `observed-stale` and `absent` are not the same bucket, and each
/// carries the reported-turn count belonging to it.
#[test]
fn test_1854_by_evidence_held_carries_stale_and_absent_with_their_success_counts() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path());

    // absent: nothing has ever observed this destination.
    let absent_session = fixture.launch(&["--fresh"]);
    fixture.hook(&absent_session, "StopFailure");

    // observed-stale: a reading older than the horizon.
    let cache = GatewayHealthCache::at(fixture.data_dir().join("gateway-health"));
    let long_ago = now_unix() - HEALTH_EVIDENCE_HORIZON_SECONDS - 60;
    cache.store(
        PROVIDER,
        &[GatewayHealthReading {
            credential_label: format!("{PROVIDER}/{CREDENTIAL_VAR}"),
            model: MODEL.to_owned(),
            consecutive_failures: 1,
            cooling_down_until_unix: None,
            cooldown_cause: None,
            credential_rejected: false,
        }],
        long_ago,
    );
    let stale_session = fixture.launch(&["--fresh"]);
    fixture.hook(&stale_session, "Stop");

    let report = fixture.glasshouse(&["route"]);
    assert!(report.status.success(), "{}", both_streams(&report));
    let printed = both_streams(&report);
    let normalised = printed.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        printed.contains("by evidence held about the destination when it was chosen"),
        "{printed}"
    );
    assert!(
        normalised.contains("absent : 0 of 1 reported turns completed"),
        "the failed absent-evidence turn must show its own success count:\n{printed}"
    );
    assert!(
        normalised.contains("observed-stale : 1 of 1 reported turns completed"),
        "the stale-evidence turn's completion must be counted under its own bucket, not folded \
         into `absent`:\n{printed}"
    );
}
