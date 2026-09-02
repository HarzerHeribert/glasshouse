//! `GH-RESPONSIVENESS-TERMS` — the two readers `tests/routing_score.rs`,
//! `tests/pairing_prior.rs` and `tests/routing_cost.rs` do not fit: map line
//! 1850 (`EvidenceLedger::responsiveness_separation`, `routing-cost`'s
//! `responsiveness vs usable turns` block) and map line 1845's other five
//! quantities (`EvaluationObservations::pairing_class_responsiveness`,
//! `route`'s `by pairing class` block).
//!
//! Rows are planted through the real ledger APIs in-process, the same shape
//! `tests/effort_shadow.rs` uses for the identical verdict-subquery
//! mechanism (`EvidenceLedger::effort_shadow`, which
//! `responsiveness_separation` shares its verdict join with) — this file's
//! fixture is a copy of that one's rather than a shared helper, the
//! established convention every integration test binary in this crate
//! follows.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use clap::Parser;
use glasshouse::Runtime;
use glasshouse::evaluation::{
    EvaluationKind, EvaluationObservations, NewObservation as EvaluationNewObservation,
};
use glasshouse::routing::evidence::{
    EvidenceLedger, HARNESS_TURN_PURPOSE, MIN_SAMPLE_FOR_SUMMARY, NewObservation, Outcome,
};
use glasshouse::session::{NewSession, ProjectSessions, SessionPairingClass};

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
        std::fs::create_dir_all(base.join("config")).unwrap();

        let cli = glasshouse::Cli::try_parse_from([
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

    fn run(&self, args: &[&str]) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn glasshouse");
        child.stdin.take().unwrap().write_all(b"").unwrap();
        child.wait_with_output().expect("wait for glasshouse")
    }

    fn routing_cost(&self) -> String {
        let output = self.run(&["routing-cost"]);
        assert!(
            output.status.success(),
            "routing-cost must succeed: stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn route(&self) -> String {
        let output = self.run(&["route"]);
        assert!(
            output.status.success(),
            "route must succeed: stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// A real session record, so `sessions.pairing_class` is a fact the
    /// session-id join can actually read — the same door
    /// `main.rs::launch_session` creates one through
    /// ([`ProjectSessions::open`]'s own doc comment).
    fn session(&self, pairing_class: Option<SessionPairingClass>) -> String {
        let sessions = ProjectSessions::open(&self.runtime).unwrap();
        let record = sessions
            .store()
            .create(NewSession::embedded("claude-code").with_pairing_class(pairing_class))
            .unwrap();
        record.id.as_str().to_owned()
    }

    /// Plant one translated-exchange row through the real ledger API.
    #[allow(clippy::too_many_arguments)]
    fn plant_routing_row(
        &self,
        session_id: Option<&str>,
        provider: &str,
        model: &str,
        first_tool_call_ms: Option<i64>,
        first_token_ms: Option<i64>,
        completed_ms: Option<i64>,
        output_tokens: Option<i64>,
        tool_rounds: Option<u32>,
        repairs: Option<u32>,
        outcome: Option<Outcome>,
        observed_at_unix: i64,
    ) {
        let mut observation = NewObservation::new(provider, model)
            .with_purpose(Some(HARNESS_TURN_PURPOSE))
            .with_harness(Some("claude-code"))
            .with_first_tool_call_ms(first_tool_call_ms)
            .with_first_token_ms(first_token_ms)
            .with_completed_ms(completed_ms)
            .with_tokens(None, output_tokens, None)
            .with_tool_rounds(tool_rounds)
            .with_repairs(repairs);
        if let Some(session_id) = session_id {
            observation = observation.with_session_id(Some(session_id));
        }
        if let Some(outcome) = outcome {
            observation = observation.with_outcome(outcome);
        }
        let ledger = EvidenceLedger::open(&self.runtime).unwrap();
        ledger.record(observation, observed_at_unix).unwrap();
    }

    /// Plant one `TurnOutcomeObserved` row — the usable-turn verdict map
    /// line 1850 reads, the same shape `evaluation::record_turn_outcome`
    /// writes.
    fn plant_turn_verdict(&self, session_id: &str, subject: &str, observed_at_unix: i64) {
        let ledger = EvaluationObservations::open(&self.runtime).unwrap();
        ledger
            .record(
                EvaluationNewObservation::new(EvaluationKind::TurnOutcomeObserved)
                    .with_subject(subject)
                    .with_session_id(session_id),
                observed_at_unix,
            )
            .unwrap();
    }

    /// Plant one `RoutingCostClassObserved` decision row — the same shape
    /// `record_routed_session` writes, and what
    /// `route_outcomes_by_pairing_class`'s own `decision` count and map line
    /// 1845's `user overrides` denominator are read from.
    fn plant_decision(&self, session_id: &str, subject: &str, observed_at_unix: i64) {
        let ledger = EvaluationObservations::open(&self.runtime).unwrap();
        ledger
            .record(
                EvaluationNewObservation::new(EvaluationKind::RoutingCostClassObserved)
                    .with_subject(subject)
                    .with_session_id(session_id),
                observed_at_unix,
            )
            .unwrap();
    }

    /// Plant one `RoutingOutcomeObserved` row — the harness's own verdict on
    /// the routed session's *turns*, what map line 1845's `task success`
    /// half is read from (`route_outcomes_by_pairing_class`'s own `verdict`
    /// CTE) — distinct from [`Self::plant_turn_verdict`]'s
    /// `TurnOutcomeObserved`, which map line 1850 reads instead.
    fn plant_routing_outcome(&self, session_id: &str, subject: &str, observed_at_unix: i64) {
        let ledger = EvaluationObservations::open(&self.runtime).unwrap();
        ledger
            .record(
                EvaluationNewObservation::new(EvaluationKind::RoutingOutcomeObserved)
                    .with_subject(subject)
                    .with_session_id(session_id),
                observed_at_unix,
            )
            .unwrap();
    }

    /// Plant one overridden `RoutingOverrideDecided` row — map line 1845's
    /// `user overrides` numerator.
    fn plant_override(&self, session_id: &str, observed_at_unix: i64) {
        let ledger = EvaluationObservations::open(&self.runtime).unwrap();
        ledger
            .record(
                EvaluationNewObservation::new(EvaluationKind::RoutingOverrideDecided)
                    .with_subject("overridden")
                    .with_session_id(session_id),
                observed_at_unix,
            )
            .unwrap();
    }
}

fn now() -> i64 {
    glasshouse::provider::cache::now_unix_seconds()
}

/// The `responsiveness vs usable turns (1850):` block — everything from its
/// own header to the end of the report, matching
/// `tests/effort_shadow.rs::effort_shadow_section`'s own technique.
fn separation_section(report: &str) -> String {
    let marker = "\nresponsiveness vs usable turns (1850):\n";
    let start = report
        .find(marker)
        .unwrap_or_else(|| panic!("no separation block in:\n{report}"));
    report[start..].to_owned()
}

// ===========================================================================
// Map line 1850 — `EvidenceLedger::responsiveness_separation` /
// `render_responsiveness_separation`.
// ===========================================================================

/// Five usable-turn exchanges on one route and five unusable-turn exchanges
/// on another, each side internally uniform, print all four measures with
/// real separation figures and both sample counts.
#[test]
fn five_usable_and_five_unusable_turns_print_all_four_measures_with_samples() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let at = now() - 60;

    for i in 0..5 {
        let session = fixture.session(Some(SessionPairingClass::VendorNative));
        fixture.plant_routing_row(
            Some(&session),
            "usable-provider",
            "usable-model",
            Some(1_000),
            Some(500),
            Some(2_000),
            Some(300),
            None,
            None,
            Some(Outcome::Succeeded),
            at - i,
        );
        fixture.plant_turn_verdict(&session, "completed", at - i + 1);
    }
    for i in 0..5 {
        let session = fixture.session(Some(SessionPairingClass::VendorNative));
        fixture.plant_routing_row(
            Some(&session),
            "unusable-provider",
            "unusable-model",
            Some(3_000),
            Some(2_500),
            Some(5_000),
            Some(100),
            None,
            None,
            Some(Outcome::Succeeded),
            at - i,
        );
        fixture.plant_turn_verdict(&session, "failed", at - i + 1);
    }

    let report = fixture.routing_cost();
    let section = separation_section(&report);
    let normalised = section.split_whitespace().collect::<Vec<_>>().join(" ");

    for measure in ["raw TTFC", "effective TTFC", "TTFT", "decode tokens/s"] {
        assert!(
            normalised.contains(&format!("{measure} : separates")),
            "{measure} must separate with real evidence:\n{section}"
        );
        assert!(
            normalised.contains("5 usable, 5 unusable turns"),
            "{measure}'s samples must be 5 and 5:\n{section}"
        );
    }
    assert!(
        !normalised.contains("predicts"),
        "the wording must say separates, never predicts: {section}"
    );
}

/// Fewer than `MIN_SAMPLE_FOR_SUMMARY` on the unusable side prints *not
/// enough evidence* for every measure, with the real (small) sample named.
///
/// Mutation target `usable-verdict-ignored`: counting every exchange as
/// usable must fail this test (the unusable side would never reach a
/// non-zero sample, so it stays "not enough" for the wrong reason — see the
/// report's own attribution note).
#[test]
fn fewer_than_the_floor_on_one_side_prints_not_enough_evidence() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let at = now() - 60;

    for i in 0..5 {
        let session = fixture.session(Some(SessionPairingClass::VendorNative));
        fixture.plant_routing_row(
            Some(&session),
            "usable-provider",
            "usable-model",
            Some(1_000),
            Some(500),
            Some(2_000),
            Some(300),
            None,
            None,
            Some(Outcome::Succeeded),
            at - i,
        );
        fixture.plant_turn_verdict(&session, "completed", at - i + 1);
    }
    // Only two unusable turns — below MIN_SAMPLE_FOR_SUMMARY (5).
    for i in 0..2 {
        let session = fixture.session(Some(SessionPairingClass::VendorNative));
        fixture.plant_routing_row(
            Some(&session),
            "unusable-provider",
            "unusable-model",
            Some(3_000),
            Some(2_500),
            Some(5_000),
            Some(100),
            None,
            None,
            Some(Outcome::Succeeded),
            at - i,
        );
        fixture.plant_turn_verdict(&session, "failed", at - i + 1);
    }

    let report = fixture.routing_cost();
    let section = separation_section(&report);
    let normalised = section.split_whitespace().collect::<Vec<_>>().join(" ");

    for measure in ["raw TTFC", "TTFT", "decode tokens/s"] {
        assert!(
            normalised.contains(&format!(
                "{measure} : not enough evidence (5 usable, 2 unusable turns; \
                 {MIN_SAMPLE_FOR_SUMMARY} needed on each side)"
            )),
            "{measure} must say not enough evidence with the real samples:\n{section}"
        );
    }
    // Effective TTFC's own unusable sample is `0`, not `2`: a row's
    // effective-TTFC value is its *route's* aggregate
    // (`responsiveness_separation`'s own doc comment — "attached per row
    // from its own route"), and the unusable route here has only two rows
    // of its own, below `MIN_SAMPLE_FOR_SUMMARY`, so its effective TTFC is
    // itself unmeasured and no row can carry one.
    assert!(
        normalised.contains(&format!(
            "effective TTFC : not enough evidence (5 usable, 0 unusable turns; \
             {MIN_SAMPLE_FOR_SUMMARY} needed on each side)"
        )),
        "{section}"
    );
}

// ===========================================================================
// Map line 1845 — `EvaluationObservations::pairing_class_responsiveness` /
// `main.rs::render_pairing_class_rows`.
// ===========================================================================

/// One pairing class, seeded so all six quantities clear
/// `MIN_SAMPLE_FOR_SUMMARY`: five sessions, four of five turns completed,
/// three of five rows carrying a positive tool round, all five carrying one
/// repair, all five with a real TTFC and a succeeded outcome, two of five
/// overridden.
#[test]
fn a_pairing_class_with_five_sessions_prints_all_six_quantities() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let at = now() - 60;

    for i in 0..5 {
        let session = fixture.session(Some(SessionPairingClass::VendorNative));
        fixture.plant_decision(&session, "free", at - i);
        fixture.plant_routing_outcome(
            &session,
            if i < 4 { "completed" } else { "failed" },
            at - i + 1,
        );
        fixture.plant_routing_row(
            Some(&session),
            "class-provider",
            "class-model",
            Some(1_000),
            None,
            None,
            None,
            Some(if i < 3 { 2 } else { 0 }),
            Some(1),
            Some(Outcome::Succeeded),
            at - i,
        );
        if i < 2 {
            fixture.plant_override(&session, at - i + 2);
        }
    }

    let report = fixture.route();
    assert!(
        report.contains("by pairing class"),
        "the section must be present:\n{report}"
    );
    let start = report.find("by pairing class").unwrap();
    let block = &report[start..];
    let end = block.find("\n  by evidence").unwrap_or(block.len());
    let block = &block[..end];
    let normalised = block.split_whitespace().collect::<Vec<_>>().join(" ");

    assert!(
        normalised.contains("vendor-native"),
        "the bucket must be the session's own pairing class:\n{block}"
    );
    assert!(
        normalised.contains("task success : 4 of 5 reported turns completed"),
        "{block}"
    );
    assert!(
        normalised.contains("usable tool calls : 60.0% (over 5 rows)"),
        "3 of 5 rows carried a positive tool round:\n{block}"
    );
    assert!(
        normalised.contains("repair loops : 1.00 (over 5 rows)"),
        "{block}"
    );
    assert!(
        normalised.contains("effective TTFC : 1000ms (mean, 5 rows)"),
        "a perfectly reliable route's effective TTFC equals its raw TTFC:\n{block}"
    );
    assert!(
        normalised.contains("reliability : 100.0% (over 5 rows)"),
        "{block}"
    );
    assert!(
        normalised.contains("user overrides : 40.0% (over 5 rows)"),
        "2 of 5 decisions were overridden:\n{block}"
    );
}

/// Below `MIN_SAMPLE_FOR_SUMMARY` on every quantity, each of the five
/// prints *not enough evidence* — never a fabricated number from one or two
/// planted rows.
#[test]
fn a_pairing_class_below_the_floor_prints_not_enough_evidence_on_all_five() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let at = now() - 60;

    let session = fixture.session(Some(SessionPairingClass::VendorNative));
    fixture.plant_decision(&session, "free", at);
    fixture.plant_routing_outcome(&session, "completed", at + 1);
    fixture.plant_routing_row(
        Some(&session),
        "class-provider",
        "class-model",
        Some(1_000),
        None,
        None,
        None,
        Some(2),
        Some(1),
        Some(Outcome::Succeeded),
        at,
    );

    let report = fixture.route();
    let start = report.find("by pairing class").unwrap();
    let block = &report[start..];
    let end = block.find("\n  by evidence").unwrap_or(block.len());
    let block = &block[..end];
    let normalised = block.split_whitespace().collect::<Vec<_>>().join(" ");

    for label in [
        "usable tool calls",
        "repair loops",
        "reliability",
        "user overrides",
    ] {
        assert!(
            normalised.contains(&format!("{label} : not enough evidence")),
            "{label} must say not enough evidence on one planted row:\n{block}"
        );
    }
    assert!(
        normalised.contains("effective TTFC : not enough evidence"),
        "{block}"
    );
}
