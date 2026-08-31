//! Map line 1480 — *"Record successful and failed task outcomes by workload
//! tier when enough evidence exists."*
//!
//! The two producers this reader joins are already production and already
//! proven with their own callers: `record_routed_session`'s third row
//! (`EvaluationKind::RoutingTierObserved`, map line 1834,
//! `tests/evaluation_producers.rs`) and the harness-verdict outcome row
//! (`EvaluationKind::RoutingOutcomeObserved`, map line 1835,
//! `tests/routing_outcome.rs`). What is new here is the reader that joins
//! them with a sample gate, and the report section that renders it — so
//! rows are planted through the two producer functions directly rather than
//! through a real launch and a real harness process, the same allowance
//! `tests/evaluation_producers.rs`'s own header gives its rendering test:
//! the producers' own callers are proven elsewhere, and what is left to show
//! is the gate and the arithmetic over rows a launch cannot conveniently
//! place five-plus of.
//!
//! `outcomes_by_tier_gates_by_sample_and_never_counts_undecided_as_failed`
//! calls the reader directly. `the_route_command_prints_the_tier_outcomes_section`
//! goes through the shipped binary's `glasshouse route`, which is
//! `main.rs::route_report`'s real caller and therefore this package's own
//! production entry point (practice §35).

use std::path::{Path, PathBuf};
use std::process::Command;

use clap::Parser;

use glasshouse::evaluation::{
    EvaluationObservations, RoutingEvidence, RoutingTier, TierOutcome, TierOutcomeVerdict,
    now_unix, record_routed_session, record_routing_outcome,
};
use glasshouse::events::TurnOutcome;
use glasshouse::routing::classify::WorkloadTier;
use glasshouse::routing::evidence::MIN_SAMPLE_FOR_SUMMARY;
use glasshouse::{Cli, Runtime};

/// A bootstrapped project inside `base` — the same idiom
/// `tests/route_correlation.rs` and `tests/evaluation_observations.rs` use.
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

/// Record a routing decision for `session_id` under `tier`, exactly the way
/// `main.rs::launch_session`'s two routed exits do — the real producer, not
/// a hand-built row.
fn route_session(runtime: &Runtime, session_id: &str, tier: RoutingTier, at: i64) {
    record_routed_session(
        runtime,
        session_id,
        "probe/a-model",
        None,
        RoutingEvidence::Absent,
        tier,
        at,
    );
}

/// A bucket's [`TierOutcome`], or panics with every bucket the reader
/// actually returned — easier to debug than an `Option` at every call site.
fn bucket<'a>(outcomes: &'a [TierOutcome], name: &str) -> &'a TierOutcome {
    outcomes
        .iter()
        .find(|outcome| outcome.bucket == name)
        .unwrap_or_else(|| {
            panic!(
                "no `{name}` bucket among {:?}",
                outcomes.iter().map(|o| &o.bucket).collect::<Vec<_>>()
            )
        })
}

/// Plants four tiers' worth of sessions and returns the window their rows
/// fall inside:
///
/// - `heavy`: five sessions with a `completed` outcome, plus two more with a
///   tier row and **no** outcome row — at the gate exactly, and proves an
///   undecided session is never folded into `failed`.
/// - `leaf`: three sessions, two `completed` and one `failed` — below the
///   gate, with a non-zero count.
/// - `standard-escalated`: six sessions, four `completed` and two `failed` —
///   above the gate, and proves an escalated tier is its own bucket rather
///   than folded into `standard`.
/// - `unclassified`: two sessions with a tier row and no outcome at all —
///   below the gate at a zero count.
fn plant(runtime: &Runtime, now: i64) -> (i64, i64) {
    let heavy = RoutingTier::Classified {
        tier: WorkloadTier::Heavy,
        escalated: false,
    };
    for i in 0..5 {
        let session = format!("heavy-done-{i}");
        route_session(runtime, &session, heavy, now);
        record_routing_outcome(runtime, &session, TurnOutcome::Completed, now);
    }
    for i in 0..2 {
        let session = format!("heavy-undecided-{i}");
        route_session(runtime, &session, heavy, now);
    }

    let leaf = RoutingTier::Classified {
        tier: WorkloadTier::Leaf,
        escalated: false,
    };
    for i in 0..2 {
        let session = format!("leaf-done-{i}");
        route_session(runtime, &session, leaf, now);
        record_routing_outcome(runtime, &session, TurnOutcome::Completed, now);
    }
    {
        let session = "leaf-failed-0".to_owned();
        route_session(runtime, &session, leaf, now);
        record_routing_outcome(runtime, &session, TurnOutcome::Failed, now);
    }

    let standard_escalated = RoutingTier::Classified {
        tier: WorkloadTier::Standard,
        escalated: true,
    };
    for i in 0..4 {
        let session = format!("standard-escalated-done-{i}");
        route_session(runtime, &session, standard_escalated, now);
        record_routing_outcome(runtime, &session, TurnOutcome::Completed, now);
    }
    for i in 0..2 {
        let session = format!("standard-escalated-failed-{i}");
        route_session(runtime, &session, standard_escalated, now);
        record_routing_outcome(runtime, &session, TurnOutcome::Failed, now);
    }

    for i in 0..2 {
        let session = format!("unclassified-{i}");
        route_session(runtime, &session, RoutingTier::Unclassified, now);
    }

    (now - 1_000, now + 1_000)
}

/// Behavioral contract: below `MIN_SAMPLE_FOR_SUMMARY` reported turns a tier
/// reports insufficient evidence with its count, exactly as
/// `route_correlations_section` does for a route pair; at or above it, a
/// tier reports its successful and failed counts; a session with a tier row
/// and no outcome row is `undecided` and never counted as failed; and
/// escalated and non-escalated tiers are distinct buckets.
#[test]
fn outcomes_by_tier_gates_by_sample_and_never_counts_undecided_as_failed() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let now = now_unix();
    let (from, to) = plant(&fixture.runtime, now);

    let outcomes = fixture.ledger().outcomes_by_tier(from, to).unwrap();

    let heavy = bucket(&outcomes, "heavy");
    assert_eq!(
        heavy.verdict,
        TierOutcomeVerdict::Measured {
            successful: 5,
            failed: 0,
            sample_size: 5,
        },
        "five completed turns clear the gate with zero failures: {heavy:?}"
    );
    assert_eq!(
        heavy.undecided, 2,
        "two sessions routed to `heavy` with no turn end reported yet must be counted as \
         undecided, not folded into `failed`: {heavy:?}"
    );

    let leaf = bucket(&outcomes, "leaf");
    assert_eq!(
        leaf.verdict,
        TierOutcomeVerdict::InsufficientEvidence {
            sample_size: 3,
            required: MIN_SAMPLE_FOR_SUMMARY,
        },
        "three reported turns is below the gate, and the count is carried rather than hidden: \
         {leaf:?}"
    );
    assert_eq!(leaf.undecided, 0);

    let standard_escalated = bucket(&outcomes, "standard-escalated");
    assert_eq!(
        standard_escalated.verdict,
        TierOutcomeVerdict::Measured {
            successful: 4,
            failed: 2,
            sample_size: 6,
        },
        "an escalated tier is its own bucket, gated the same way: {standard_escalated:?}"
    );

    // `standard` itself was never routed to, so it must not appear as a
    // zero-filled row — an escalated tier is never folded into its
    // non-escalated sibling.
    assert!(
        outcomes.iter().all(|outcome| outcome.bucket != "standard"),
        "no session was routed to plain `standard`, and none should appear: {outcomes:?}"
    );

    let unclassified = bucket(&outcomes, "unclassified");
    assert_eq!(
        unclassified.verdict,
        TierOutcomeVerdict::InsufficientEvidence {
            sample_size: 0,
            required: MIN_SAMPLE_FOR_SUMMARY,
        },
        "zero reported turns is still below the gate, not a crash or an omission: \
         {unclassified:?}"
    );
    assert_eq!(
        unclassified.undecided, 2,
        "both unclassified sessions have no outcome yet: {unclassified:?}"
    );
}

/// Line 1480 at the shipped binary: `glasshouse route` prints a workload-tier
/// section beside the correlation and throttle-scope sections, gated the
/// same way the reader is.
#[test]
fn the_route_command_prints_the_tier_outcomes_section() {
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
             credential_env = [\"GLASSHOUSE_TIER_OUTCOMES_TEST_KEY\"]\n\n\
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
        .split_once("Workload-tier outcomes in this project, last 30 days (map line 1480)")
        .unwrap_or_else(|| panic!("no map-line-1480 section in:\n{stdout}"))
        .1;

    assert!(
        block.contains("heavy: 5 of 5 reported turns succeeded, 0 failed"),
        "{block}"
    );
    assert!(
        block.contains("2 session(s) with no turn end reported yet — undecided, never a failure"),
        "{block}"
    );
    assert!(
        block.contains(&format!(
            "leaf: insufficient evidence — 3 of the {MIN_SAMPLE_FOR_SUMMARY} reported turns a \
             tier summary needs; treated as no summary"
        )),
        "{block}"
    );
    assert!(
        block.contains("standard-escalated: 4 of 6 reported turns succeeded, 2 failed"),
        "{block}"
    );
    assert!(
        block.contains(&format!(
            "unclassified: insufficient evidence — 0 of the {MIN_SAMPLE_FOR_SUMMARY} reported \
             turns a tier summary needs; treated as no summary"
        )),
        "{block}"
    );
}
