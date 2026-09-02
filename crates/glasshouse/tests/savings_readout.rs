//! Map line 2034: `glasshouse routing-cost`'s `SAVINGS` section, driven
//! through the shipped binary — practice §35's *"a caller every test
//! bypasses is not a caller"* applies here exactly as `tests/routing_cost.rs`'s
//! own header already argues for the per-purpose groups above this section.
//!
//! Test (b) is the one exception, disclosed here rather than left implicit:
//! it plants harness-turn rows through `EvidenceLedger::record` in-process,
//! the same shape `tests/routing_cost.rs::Fixture::record` already uses,
//! because the *producer* of a translated exchange's row is proven in its
//! own phase (`tests/gateway_translate_evidence.rs`) — this package tests
//! the reader, `EvidenceLedger::translation_cache_savings`, and the
//! renderer, `main.rs::render_savings_section`, not the gateway again.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use clap::Parser;
use glasshouse::Runtime;
use glasshouse::routing::evidence::{EvidenceLedger, HARNESS_TURN_PURPOSE, NewObservation};

// ===========================================================================
// Fixture — same shape as `tests/firewall_observability.rs`'s own
// (drives the hook and `status`/`routing-cost`), duplicated rather than
// shared: each integration test binary is its own crate.
// ===========================================================================

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

    fn run(&self, args: &[&str], stdin_bytes: &[u8]) -> Output {
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
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin_bytes)
            .expect("write stdin");
        child.wait_with_output().expect("wait for glasshouse")
    }

    fn hook(&self, event: &serde_json::Value, extra_args: &[&str]) -> serde_json::Value {
        let mut args = vec!["context-firewall", "hook"];
        args.extend_from_slice(extra_args);
        let bytes = serde_json::to_vec(event).unwrap();
        let output = self.run(&args, &bytes);
        assert!(
            output.status.success(),
            "the hook must always exit 0 (fail open): stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("hook response must be valid JSON")
    }

    fn routing_cost(&self) -> String {
        let output = self.run(&["routing-cost"], b"");
        assert!(
            output.status.success(),
            "routing-cost must succeed: stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// Plant one observation through the real ledger API, the same shape
    /// `tests/routing_cost.rs::Fixture::record` uses — every column a
    /// producer might set, so a token count left `None` becomes `NULL`
    /// exactly as `NewObservation::with_tokens`'s own doc comment requires.
    fn plant_harness_turn(
        &self,
        provider: &str,
        model: &str,
        route: &str,
        quota_context: &str,
        tokens: Option<(i64, i64, i64)>,
        observed_at_unix: i64,
    ) {
        let mut observation = NewObservation::new(provider, model)
            .with_harness(Some("claude-code"))
            .with_purpose(Some(HARNESS_TURN_PURPOSE))
            .with_route(Some(route))
            .with_quota_context(Some(quota_context));
        if let Some((input, output, cached)) = tokens {
            observation = observation.with_tokens(Some(input), Some(output), Some(cached));
        }
        let ledger = EvidenceLedger::open(&self.runtime).unwrap();
        ledger.record(observation, observed_at_unix).unwrap();
    }
}

fn post_tool_use(
    tool_name: &str,
    tool_response: serde_json::Value,
    session_id: &str,
    tool_use_id: &str,
) -> serde_json::Value {
    serde_json::json!({
        "tool_name": tool_name,
        "tool_input": {},
        "tool_response": tool_response,
        "tool_use_id": tool_use_id,
        "session_id": session_id,
        "cwd": "/tmp",
    })
}

fn text_response(text: &str) -> serde_json::Value {
    serde_json::json!({"type": "text", "text": text})
}

fn now() -> i64 {
    glasshouse::provider::cache::now_unix_seconds()
}

/// The rendered block for one `SAVINGS` facet's label, from the blank line
/// before `  {label}` to the next blank line (or the end of the report) —
/// the same convention `tests/routing_cost.rs::section` already uses for
/// the per-purpose groups above this section.
fn section(report: &str, label: &str) -> String {
    let marker = format!("\n  {label}\n");
    let start = report
        .find(&marker)
        .unwrap_or_else(|| panic!("no section for {label:?} in:\n{report}"));
    let rest = &report[start + 1..];
    let end = rest.find("\n\n").unwrap_or(rest.len());
    rest[..end].to_owned()
}

// ===========================================================================
// (a) A real hook run that reduces a large tool result.
// ===========================================================================

#[test]
fn firewall_facet_counts_a_real_reduction_with_a_positive_estimate() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());

    // Many duplicate lines: the deterministic ladder (map lines 1982/1983)
    // collapses every repeat after the first, guaranteeing a real,
    // measurable gap between the original and forwarded estimates.
    let original = "duplicate needle line\n".repeat(500);
    let event = post_tool_use("Grep", text_response(&original), "s-savings-a", "tu-1");
    let response = fixture.hook(
        &event,
        &["--passthrough-tokens", "10", "--emit-updated-output"],
    );
    assert!(
        response
            .get("hookSpecificOutput")
            .and_then(|v| v.get("updatedToolOutput"))
            .is_some(),
        "the hook must have reduced this oversized, highly-duplicated result: {response}"
    );

    let report = fixture.routing_cost();
    let facet = section(&report, "context firewall");
    assert!(
        facet.contains("across 1 reductions of 1 results above threshold"),
        "expected exactly one reduction and one result above threshold in:\n{facet}"
    );
    let kept_local: u64 = facet
        .lines()
        .find_map(|line| line.trim_start().strip_prefix("kept local (estimated) "))
        .and_then(|rest| rest.split_whitespace().next())
        .expect("a kept-local figure")
        .parse()
        .expect("kept-local must be a digit");
    assert!(
        kept_local > 0,
        "a highly-duplicated result must show a positive estimated savings: {facet}"
    );
}

// ===========================================================================
// (b) Translated harness-turn rows planted through `EvidenceLedger::record`.
// ===========================================================================

#[test]
fn translation_facet_sums_cached_and_input_tokens_and_excludes_relayed_rows() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let at = now() - 5;

    // Two translated rows on the same route/credential: cached 300 of input
    // 700 combined.
    fixture.plant_harness_turn(
        "fixture",
        "fixture-model",
        "anthropic-messages->openai-chat",
        "cred-a",
        Some((400, 50, 200)),
        at,
    );
    fixture.plant_harness_turn(
        "fixture",
        "fixture-model",
        "anthropic-messages->openai-chat",
        "cred-a",
        Some((300, 20, 100)),
        at,
    );
    // A relayed row on the same route: no tokens at all, so it must never
    // enter the denominator below.
    fixture.plant_harness_turn(
        "fixture",
        "fixture-model",
        "anthropic-messages->openai-chat",
        "cred-a",
        None,
        at,
    );
    // A route with ONLY a relayed row, no companion translated row at all —
    // this must never surface as a translation group of its own. A reader
    // that dropped the `input_tokens IS NOT NULL` filter would either sum a
    // NULL into this group (a type error reading the row back) or print a
    // group for a route this build never translated a byte for.
    fixture.plant_harness_turn(
        "fixture",
        "fixture-model",
        "anthropic-messages",
        "cred-b",
        None,
        at,
    );

    let report = fixture.routing_cost();
    let facet = section(
        &report,
        "translation anthropic-messages->openai-chat / cred-a",
    );
    assert!(
        facet.contains("300 of 1000"),
        "expected the two counted rows' 300 cached of 1000 (700 input + 300 cached), \
         with the NULL-token row excluded from the denominator, in:\n{facet}"
    );
    assert!(
        !report.contains("\n  translation anthropic-messages / cred-b\n"),
        "a route with only relayed rows must never appear as its own translation group: \
         {report}"
    );
}

// ===========================================================================
// (c) An empty ledger and store.
// ===========================================================================

#[test]
fn every_facet_reports_not_counted_words_and_no_digit_when_nothing_is_recorded() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());

    let report = fixture.routing_cost();

    let firewall = section(&report, "context firewall");
    assert!(
        firewall.contains("not counted: no context-firewall activity recorded in this window"),
        "{firewall}"
    );
    assert!(
        !firewall.lines().skip(1).any(|line| line
            .trim()
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit())),
        "the firewall facet must never print a digit when nothing was recorded: {firewall}"
    );

    let translation = section(&report, "translation");
    assert!(
        translation.contains("not counted: no translated exchange recorded"),
        "{translation}"
    );

    let response_profile = section(&report, "response profile");
    assert!(
        response_profile.contains("not counted: no exchange row carries a response profile"),
        "{response_profile}"
    );
    assert!(
        !response_profile.split_whitespace().any(|word| word == "0"),
        "the response-profile facet must never print a bare 0: {response_profile}"
    );
}
