//! Map line 1629 — *"Record which resource performed important memory
//! extraction or classification for debugging."*
//!
//! The producer is already production and already proven with its own
//! caller: `EvidenceLedger::record`, written by `main.rs`'s classification
//! and extraction call sites (`tests/evaluation_producers.rs`,
//! `tests/classification_call.rs`). What is new here is the purpose-filtered
//! reader — [`glasshouse::routing::evidence::EvidenceLedger::recent_support_work`]
//! — and the report section that renders it, so rows are planted through
//! [`glasshouse::routing::evidence::NewObservation`] directly rather than
//! through a real model call, the same allowance
//! `tests/evaluation_producers.rs`'s own header gives arithmetic-only
//! coverage of an already-proven producer.
//!
//! `recent_support_work_reads_only_the_two_support_purposes_newest_first`
//! calls the reader directly. `the_route_command_prints_the_support_work_section`
//! goes through the shipped binary's `glasshouse route`
//! (`main.rs::route_report`'s real caller, practice §35).

use std::path::{Path, PathBuf};
use std::process::Command;

use clap::Parser;

use glasshouse::evaluation::now_unix;
use glasshouse::routing::evidence::{
    CLASSIFICATION_PURPOSE, EXTRACTION_PURPOSE, EvidenceLedger, NewObservation, Outcome,
    ROUTING_LATENCY_PURPOSE,
};
use glasshouse::{Cli, Runtime};

/// A bootstrapped project inside `base` — the same idiom
/// `tests/tier_outcomes.rs` and `tests/evaluation_producers.rs` use.
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

/// Plants, oldest first: two classification calls, three extraction calls,
/// one interactive coding-agent exchange (a harness named, no purpose — the
/// shape `crate::gateway::session::record_routing_observation` writes), and
/// one routing-latency row (a purpose, but not a support-work one). The last
/// two exist so a mutation that drops the purpose filter is caught by an
/// unrelated row leaking into the support-work reader.
fn plant(evidence: &EvidenceLedger, now: i64) {
    evidence
        .record(
            NewObservation::new("router", "a-router-model")
                .with_purpose(Some(CLASSIFICATION_PURPOSE))
                .with_route(Some("anthropic-messages"))
                .with_timing(Some(now - 100), Some(now - 99))
                .with_outcome(Outcome::Succeeded),
            now - 100,
        )
        .unwrap();
    evidence
        .record(
            NewObservation::new("router", "a-router-model")
                .with_purpose(Some(CLASSIFICATION_PURPOSE))
                .with_route(Some("anthropic-messages"))
                .with_timing(Some(now - 90), Some(now - 89))
                .with_outcome(Outcome::Failed),
            now - 90,
        )
        .unwrap();
    for (i, at) in [now - 80, now - 70, now - 60].into_iter().enumerate() {
        evidence
            .record(
                NewObservation::new("extractor", "a-cheap-local-model")
                    .with_purpose(Some(EXTRACTION_PURPOSE))
                    .with_route(Some("openai-chat"))
                    .with_timing(Some(at), Some(at + 1))
                    .with_outcome(Outcome::Succeeded),
                at + i as i64,
            )
            .unwrap();
    }
    evidence
        .record(
            NewObservation::new("anyrouter", "a-coding-model")
                .with_harness(Some("claude-code"))
                .with_timing(Some(now - 10), Some(now - 9))
                .with_outcome(Outcome::Succeeded),
            now - 10,
        )
        .unwrap();
    evidence
        .record(
            NewObservation::new("glasshouse", "session-router")
                .with_purpose(Some(ROUTING_LATENCY_PURPOSE))
                .with_harness(Some("claude-code")),
            now - 5,
        )
        .unwrap();
}

/// Behavioral contract: [`EvidenceLedger::recent_support_work`] returns only
/// rows whose `purpose` is [`CLASSIFICATION_PURPOSE`] or
/// [`EXTRACTION_PURPOSE`] — never the interactive coding-agent row (no
/// purpose) and never the routing-latency row (a purpose, but not a
/// support-work one) — newest first, and honors its `limit`.
#[test]
fn recent_support_work_reads_only_the_two_support_purposes_newest_first() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let now = now_unix();
    let evidence = fixture.evidence_ledger();
    plant(&evidence, now);

    let recent = evidence.recent_support_work(10).unwrap();
    assert_eq!(
        recent.len(),
        5,
        "two classification and three extraction rows, and nothing else: {recent:?}"
    );
    assert!(
        recent
            .iter()
            .all(|o| o.purpose.as_deref() == Some(CLASSIFICATION_PURPOSE)
                || o.purpose.as_deref() == Some(EXTRACTION_PURPOSE)),
        "the interactive row (no purpose) and the routing-latency row must both be excluded: \
         {recent:?}"
    );
    assert!(
        recent
            .windows(2)
            .all(|pair| pair[0].observed_at_unix >= pair[1].observed_at_unix),
        "newest first: {recent:?}"
    );

    let limited = evidence.recent_support_work(2).unwrap();
    assert_eq!(limited.len(), 2, "limit is honored: {limited:?}");
    assert_eq!(
        limited[0].provider, "extractor",
        "the two most recent rows are both extraction calls: {limited:?}"
    );
}

/// Line 1629 at the shipped binary: `glasshouse route` prints a
/// support-work section beside the tier and harness-efficiency sections,
/// naming purpose, provider, model, route and outcome, and never lists the
/// interactive coding-agent row or the routing-latency row.
#[test]
fn the_route_command_prints_the_support_work_section() {
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
             credential_env = [\"GLASSHOUSE_SUPPORT_WORK_TEST_KEY\"]\n\n\
             [profiles.metered]\nharness = \"claude-code\"\n\
             expected_protocol = \"anthropic-messages\"\n\n\
             [profiles.metered.backend]\nkind = \"direct-provider\"\n\
             provider = \"route-probe\"\n"
        ),
    )
    .unwrap();

    let now = now_unix();
    plant(&fixture.evidence_ledger(), now);

    let output = fixture.glasshouse(&["route"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "route must succeed: status {:?}\nstdout:\n{stdout}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let block = stdout
        .split_once("Recent support work in this project (map line 1629)")
        .unwrap_or_else(|| panic!("no map-line-1629 section in:\n{stdout}"))
        .1;

    assert!(
        block.contains("classification by router / a-router-model via anthropic-messages"),
        "{block}"
    );
    assert!(
        block.contains("memory-extraction by extractor / a-cheap-local-model via openai-chat"),
        "{block}"
    );
    assert!(
        !block.contains("anyrouter"),
        "the interactive coding-agent row (no purpose) must not appear here: {block}"
    );
    assert!(
        !block.contains("session-router"),
        "the routing-latency row (not a support purpose) must not appear here: {block}"
    );
}
