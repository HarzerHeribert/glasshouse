//! Map line 1951 — *"Record per-harness task efficiency — tokens,
//! wall-clock, request count, and outcome by task class — so that harness
//! choice can rest on evidence rather than on which vendor bills for it."*
//!
//! Two producers already exist and are proven elsewhere: the tier/outcome
//! rows (`tests/tier_outcomes.rs`, `tests/evaluation_producers.rs`) and the
//! `routing_observations` rows a real gateway exchange or a support call
//! writes (`tests/evaluation_producers.rs`, `tests/classification_call.rs`).
//! `sessions.harness` is a real production column, written by
//! `ProjectSessions::open(..).store().create` — the door `main.rs::launch_session`
//! uses to start a session under a harness — so this file plants a session
//! through that store rather than inserting a bare row, the same allowance
//! `tests/evaluation_producers.rs`'s own header gives arithmetic-only
//! coverage: what is new here is the reader that joins two ledgers by
//! harness, and the report section that renders it, not either producer.
//!
//! `outcomes_by_tier_and_harness_and_request_stats_by_harness_join_by_the_right_key`
//! calls both readers directly. `the_route_command_prints_the_harness_efficiency_section`
//! goes through the shipped binary's `glasshouse route`
//! (`main.rs::route_report`'s real caller, practice §35).

use std::path::{Path, PathBuf};
use std::process::Command;

use clap::Parser;

use glasshouse::evaluation::{
    EvaluationObservations, HarnessTierOutcome, RoutingEvidence, RoutingTier, TierOutcomeVerdict,
    now_unix, record_routed_session, record_routing_outcome,
};
use glasshouse::events::TurnOutcome;
use glasshouse::routing::classify::WorkloadTier;
use glasshouse::routing::evidence::{EvidenceLedger, MIN_SAMPLE_FOR_SUMMARY, NewObservation};
use glasshouse::session::{NewSession, ProjectSessions};
use glasshouse::{Cli, Runtime};

/// A bootstrapped project inside `base` — the same idiom
/// `tests/tier_outcomes.rs` and `tests/evaluation_observations.rs` use.
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

    fn ledger(&self) -> EvaluationObservations {
        EvaluationObservations::open(&self.runtime).unwrap()
    }

    fn evidence_ledger(&self) -> EvidenceLedger {
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
            .output()
            .expect("the glasshouse binary must be runnable")
    }
}

/// Create a real session under `harness` (`ProjectSessions::open(..).store().create`,
/// the same door a launch uses) and route it under `tier`, exactly the way
/// `tests/tier_outcomes.rs::route_session` does for the harness-less case.
fn route_session_for_harness(runtime: &Runtime, harness: &str, tier: RoutingTier, at: i64) {
    let sessions = ProjectSessions::open(runtime).unwrap();
    let record = sessions
        .store()
        .create(NewSession::embedded(harness))
        .unwrap();
    record_routed_session(
        runtime,
        record.id.as_str(),
        "probe/a-model",
        None,
        RoutingEvidence::Absent,
        tier,
        at,
    );
}

/// Plants three harnesses' worth of sessions and `routing_observations`
/// rows and returns the window they fall inside:
///
/// - `claude-code` / `heavy`: five sessions, all completed (clears
///   [`MIN_SAMPLE_FOR_SUMMARY`]), plus two more with no outcome —
///   undecided, never folded into failed. Four `routing_observations` rows:
///   two carry tokens (100 in / 50 out each) and a 5000ms span, two carry
///   neither — proving the *"N of M exchanges"* split.
/// - `codex` / `heavy`: three sessions, two completed and one failed —
///   below the gate. Three `routing_observations` rows, none carrying
///   tokens at all (the relay-path shape refusal-register P1b describes),
///   each a 2000ms span.
/// - `opencode` / `leaf`: one session with a tier row and no outcome —
///   undecided at a zero sample count, and **no** `routing_observations`
///   row at all, proving a harness with zero requests prints `0` rather
///   than being dropped.
fn plant(runtime: &Runtime, now: i64) -> (i64, i64) {
    let heavy = RoutingTier::Classified {
        tier: WorkloadTier::Heavy,
        escalated: false,
    };
    for _ in 0..5 {
        let sessions = ProjectSessions::open(runtime).unwrap();
        let record = sessions
            .store()
            .create(NewSession::embedded("claude-code"))
            .unwrap();
        record_routed_session(
            runtime,
            record.id.as_str(),
            "probe/a-model",
            None,
            RoutingEvidence::Absent,
            heavy,
            now,
        );
        record_routing_outcome(runtime, record.id.as_str(), TurnOutcome::Completed, now);
    }
    for _ in 0..2 {
        route_session_for_harness(runtime, "claude-code", heavy, now);
    }

    // Two codex tier rows in the strict past: the shipped `glasshouse route`
    // under test computes its own clock, so a row stamped in this test's
    // future is excluded until a real second elapses — deterministically past
    // avoids that flake (was `now + i`).
    for i in 0..2 {
        let sessions = ProjectSessions::open(runtime).unwrap();
        let record = sessions
            .store()
            .create(NewSession::embedded("codex"))
            .unwrap();
        record_routed_session(
            runtime,
            record.id.as_str(),
            "probe/a-model",
            None,
            RoutingEvidence::Absent,
            heavy,
            now - 1 - i,
        );
        record_routing_outcome(
            runtime,
            record.id.as_str(),
            TurnOutcome::Completed,
            now - 1 - i,
        );
    }
    {
        let sessions = ProjectSessions::open(runtime).unwrap();
        let record = sessions
            .store()
            .create(NewSession::embedded("codex"))
            .unwrap();
        record_routed_session(
            runtime,
            record.id.as_str(),
            "probe/a-model",
            None,
            RoutingEvidence::Absent,
            heavy,
            now,
        );
        record_routing_outcome(runtime, record.id.as_str(), TurnOutcome::Failed, now);
    }

    let leaf = RoutingTier::Classified {
        tier: WorkloadTier::Leaf,
        escalated: false,
    };
    route_session_for_harness(runtime, "opencode", leaf, now);

    let evidence = EvidenceLedger::open(runtime).unwrap();
    for i in 0..2 {
        evidence
            .record(
                NewObservation::new("provider-a", "model-a")
                    .with_harness(Some("claude-code"))
                    .with_timing(Some(now - 5), Some(now))
                    .with_tokens(Some(100), Some(50), None),
                now - 100 + i,
            )
            .unwrap();
    }
    for i in 0..2 {
        evidence
            .record(
                NewObservation::new("provider-a", "model-a")
                    .with_harness(Some("claude-code"))
                    .with_timing(Some(now - 5), Some(now)),
                now - 90 + i,
            )
            .unwrap();
    }
    for i in 0..3 {
        evidence
            .record(
                NewObservation::new("provider-b", "model-b")
                    .with_harness(Some("codex"))
                    .with_timing(Some(now - 2), Some(now)),
                now - 80 + i,
            )
            .unwrap();
    }

    (now - 1_000, now + 1_000)
}

/// A `(harness, bucket)` row's [`HarnessTierOutcome`], or panics with every
/// row the reader actually returned — easier to debug than an `Option` at
/// every call site (the same idiom `tests/tier_outcomes.rs::bucket` uses).
fn row<'a>(rows: &'a [HarnessTierOutcome], harness: &str, bucket: &str) -> &'a HarnessTierOutcome {
    rows.iter()
        .find(|row| row.harness == harness && row.outcome.bucket == bucket)
        .unwrap_or_else(|| {
            panic!(
                "no ({harness}, {bucket}) row among {:?}",
                rows.iter()
                    .map(|r| (&r.harness, &r.outcome.bucket))
                    .collect::<Vec<_>>()
            )
        })
}

/// Behavioral contract: [`EvaluationObservations::outcomes_by_tier_and_harness`]
/// joins `sessions.harness` onto [`EvaluationObservations::outcomes_by_tier`]'s
/// own tier join, so two harnesses routed to the same tier are two rows, each
/// gated and counted exactly as the tier-only reader gates and counts one; and
/// [`EvidenceLedger::request_stats_by_harness`] carries its token and
/// wall-clock figures with their own denominators, never printing a token
/// sum for a group where every row left it `NULL`.
#[test]
fn outcomes_by_tier_and_harness_and_request_stats_by_harness_join_by_the_right_key() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let now = now_unix();
    let (from, to) = plant(&fixture.runtime, now);

    let outcomes = fixture
        .ledger()
        .outcomes_by_tier_and_harness(from, to)
        .unwrap();

    let claude_heavy = row(&outcomes, "claude-code", "heavy");
    assert_eq!(
        claude_heavy.outcome.verdict,
        TierOutcomeVerdict::Measured {
            successful: 5,
            failed: 0,
            sample_size: 5,
        },
        "five completed turns on claude-code clear the gate with zero failures: {claude_heavy:?}"
    );
    assert_eq!(
        claude_heavy.outcome.undecided, 2,
        "two claude-code sessions with no turn end yet must be undecided, not failed: \
         {claude_heavy:?}"
    );

    let codex_heavy = row(&outcomes, "codex", "heavy");
    assert_eq!(
        codex_heavy.outcome.verdict,
        TierOutcomeVerdict::InsufficientEvidence {
            sample_size: 3,
            required: MIN_SAMPLE_FOR_SUMMARY,
        },
        "codex's three reported turns are its own bucket, below the gate, separate from \
         claude-code's five: {codex_heavy:?}"
    );

    let opencode_leaf = row(&outcomes, "opencode", "leaf");
    assert_eq!(
        opencode_leaf.outcome.undecided, 1,
        "opencode's one session has no outcome yet: {opencode_leaf:?}"
    );

    let stats = fixture
        .evidence_ledger()
        .request_stats_by_harness(from, to)
        .unwrap();
    let claude_stats = stats
        .iter()
        .find(|s| s.harness == "claude-code")
        .unwrap_or_else(|| panic!("no claude-code stats among {stats:?}"));
    assert_eq!(claude_stats.requests, 4);
    assert_eq!(claude_stats.token_rows_present, 2);
    assert_eq!(claude_stats.input_tokens_sum, 200);
    assert_eq!(claude_stats.output_tokens_sum, 100);
    let claude_wall_clock = claude_stats
        .wall_clock
        .unwrap_or_else(|| panic!("claude-code must have a wall-clock summary: {claude_stats:?}"));
    assert_eq!(claude_wall_clock.sample_count, 4);
    assert_eq!(claude_wall_clock.median_ms, 5000);

    let codex_stats = stats
        .iter()
        .find(|s| s.harness == "codex")
        .unwrap_or_else(|| panic!("no codex stats among {stats:?}"));
    assert_eq!(codex_stats.requests, 3);
    assert_eq!(
        codex_stats.token_rows_present, 0,
        "the relay path never carries tokens (refusal register P1b): {codex_stats:?}"
    );
    assert_eq!(codex_stats.input_tokens_sum, 0);
    assert_eq!(codex_stats.output_tokens_sum, 0);

    assert!(
        stats.iter().all(|s| s.harness != "opencode"),
        "opencode has no routing_observations row in this window and must not appear: \
         {stats:?}"
    );
}

/// Line 1951 at the shipped binary: `glasshouse route` prints a per-harness
/// section beside the tier section, naming the harness first, and never
/// prints a `0` token sum for a group whose rows are all `NULL`.
#[test]
fn the_route_command_prints_the_harness_efficiency_section() {
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
             credential_env = [\"GLASSHOUSE_HARNESS_EFFICIENCY_TEST_KEY\"]\n\n\
             [profiles.metered]\nharness = \"claude-code\"\n\
             expected_protocol = \"anthropic-messages\"\n\n\
             [profiles.metered.backend]\nkind = \"direct-provider\"\n\
             provider = \"route-probe\"\n"
        ),
    )
    .unwrap();

    let now = now_unix();
    plant(&fixture.runtime, now);

    let output = fixture.glasshouse(&["route"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "route must succeed: status {:?}\nstdout:\n{stdout}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let block = stdout
        .split_once("Per-harness task efficiency in this project, last 30 days (map line 1951)")
        .unwrap_or_else(|| panic!("no map-line-1951 section in:\n{stdout}"))
        .1;

    assert!(
        block.contains(
            "claude-code — heavy: 5 of 5 reported turns succeeded, 0 failed; 2 undecided; 4 \
             request(s), median wall-clock 5000ms across 4 timed exchange(s); tokens: 200 in / \
             100 out on 2 of 4 exchanges; not exposed on 2"
        ),
        "{block}"
    );
    assert!(
        block.contains(&format!(
            "codex — heavy: insufficient evidence — 3 of the {MIN_SAMPLE_FOR_SUMMARY} reported \
             turns a tier summary needs; treated as no summary; 3 request(s), median wall-clock \
             2000ms across 3 timed exchange(s); tokens: not exposed on 3 of 3 exchanges"
        )),
        "{block}"
    );
    assert!(
        block.contains(&format!(
            "opencode — leaf: insufficient evidence — 0 of the {MIN_SAMPLE_FOR_SUMMARY} \
             reported turns a tier summary needs; treated as no summary; 1 undecided; 0 \
             request(s); tokens: not exposed on 0 of 0 exchanges"
        )),
        "{block}",
    );
}
