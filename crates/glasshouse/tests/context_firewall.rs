//! Phase 57's first package — map lines 1980-1990: the context firewall
//! core, reachable through `glasshouse context-firewall hook`, the exact
//! CLI entry a Claude Code bridge will register (map line 1994, gated
//! separately). One test per box line, named recognizably, driving the
//! shipped binary against `PostToolUse`-shaped stdin fixtures, plus the
//! flagship recall fixture `docs/product/evidence/phase-57.md` requires:
//! the one relevant line surviving among thousands of duplicate hits.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use clap::Parser;
use glasshouse::Runtime;
use glasshouse::routing::evidence::{
    CONTEXT_FIREWALL_BYPASS_PURPOSE, CONTEXT_FIREWALL_EXPANSION_PURPOSE,
    CONTEXT_FIREWALL_REDUCTION_PURPOSE, EvidenceLedger, ObservationQuery,
};

// ===========================================================================
// Fixture: a bootstrapped project, and the shipped binary pointed at it.
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

    fn ledger(&self) -> EvidenceLedger {
        EvidenceLedger::open(&self.runtime).unwrap()
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

    /// Drive `context-firewall hook` with `event` on stdin, and parse the
    /// hook response. Always exits 0 — fail-open is part of what is under
    /// test.
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

    fn show(&self, id: &str) -> Output {
        self.run(&["context-firewall", "show", id], b"")
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

fn bash_response(
    stdout: &str,
    stderr: &str,
    interrupted: bool,
    exit_code: i64,
) -> serde_json::Value {
    serde_json::json!({
        "stdout": stdout,
        "stderr": stderr,
        "interrupted": interrupted,
        "exit_code": exit_code,
    })
}

fn updated_output(response: &serde_json::Value) -> Option<&str> {
    response
        .get("hookSpecificOutput")
        .and_then(|v| v.get("updatedToolOutput"))
        .and_then(|v| v.as_str())
}

fn extract_raw_ref(text: &str) -> String {
    let start = text
        .find("gh-tool://")
        .expect("provenance header must state a raw ref");
    let rest = &text[start..];
    let end = rest.find(']').unwrap_or(rest.len());
    rest[..end].to_string()
}

fn duplicate_hits_with_one_needle(needle: &str, noise_line: &str, repeats: usize) -> String {
    let mut text = String::new();
    for _ in 0..repeats {
        text.push_str(noise_line);
        text.push('\n');
    }
    text.push_str(needle);
    text.push('\n');
    for _ in 0..repeats {
        text.push_str(noise_line);
        text.push('\n');
    }
    text
}

// ===========================================================================
// One test per box line.
// ===========================================================================

/// Map line 1980: two entirely different harness JSON shapes (a plain text
/// tool and a command tool) both reach the same reduction pipeline and both
/// produce a valid, sized outcome — proof the core operates on a normalized
/// form, not on either shape directly.
#[test]
fn line_1980_distinct_harness_shapes_normalize_into_the_same_pipeline() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());

    let big_text = duplicate_hits_with_one_needle("the needle line", "noise line here", 50);
    let grep_event = post_tool_use("Grep", text_response(&big_text), "s-1980", "tu-1");
    let grep_response = fixture.hook(
        &grep_event,
        &["--passthrough-tokens", "5", "--emit-updated-output"],
    );
    let grep_output = updated_output(&grep_response).expect("Grep must reduce and emit");
    assert!(grep_output.contains("the needle line"));

    let bash_event = post_tool_use(
        "Bash",
        bash_response(&big_text, "", false, 0),
        "s-1980",
        "tu-2",
    );
    let bash_response_json = fixture.hook(
        &bash_event,
        &["--passthrough-tokens", "5", "--emit-updated-output"],
    );
    let bash_output = updated_output(&bash_response_json).expect("Bash must reduce and emit too");
    assert!(bash_output.contains("the needle line"));
}

/// Map line 1981: a small result stays byte-identical through the whole
/// pipeline and carries no header — the no-header half of REQUIRED
/// BEHAVIOR's first bullet.
#[test]
fn line_1981_a_small_result_passes_through_untouched_and_header_free() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());

    let small = "just a few short lines\nof ordinary output\n";
    let event = post_tool_use("Grep", text_response(small), "s-1981", "tu-1");
    let response = fixture.hook(&event, &["--emit-updated-output"]);

    // Below threshold: the hook never asks the harness to substitute
    // anything, because there is nothing to substitute.
    assert_eq!(response, serde_json::json!({}));
}

/// Map line 1982: oversized output containing duplicate lines is reduced
/// before anything else sees it — the forwarded text is materially smaller
/// than the original.
#[test]
fn line_1982_oversized_output_with_duplicates_is_reduced() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());

    let mut original = String::new();
    for _ in 0..500 {
        original.push_str("repeated identical log line\n");
    }
    let event = post_tool_use("Read", text_response(&original), "s-1982", "tu-1");
    let response = fixture.hook(
        &event,
        &["--passthrough-tokens", "10", "--emit-updated-output"],
    );
    let forwarded = updated_output(&response).expect("oversized output must reduce and emit");
    assert!(
        forwarded.len() < original.len(),
        "forwarded ({}) must be smaller than the original ({})",
        forwarded.len(),
        original.len()
    );
    assert_eq!(forwarded.matches("repeated identical log line").count(), 1);
}

/// Map line 1983, and the phase's own flagship fixture
/// (`docs/product/evidence/phase-57.md`): the one relevant line, planted
/// once among thousands of duplicate hits, survives.
#[test]
fn line_1983_the_needle_survives_among_thousands_of_duplicate_hits() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());

    let original = duplicate_hits_with_one_needle(
        "src/app/handler.rs: TODO fix the actual race here",
        "src/generated/bundle.js: TODO cleanup",
        3000,
    );
    let event = post_tool_use("Grep", text_response(&original), "s-1983", "tu-1");
    let response = fixture.hook(
        &event,
        &["--passthrough-tokens", "10", "--emit-updated-output"],
    );
    let forwarded = updated_output(&response).expect("an oversized result must reduce and emit");
    assert!(
        forwarded.contains("src/app/handler.rs: TODO fix the actual race here"),
        "the needle must survive dedup"
    );
}

/// Map line 1984: every reduced result's original bytes are preserved
/// locally, addressable by the `gh-tool://` reference the provenance
/// header states.
#[test]
fn line_1984_every_reduced_result_is_preserved_and_addressable() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());

    let original = "line one\n".repeat(200);
    let event = post_tool_use("Read", text_response(&original), "s-1984", "tu-1");
    let response = fixture.hook(
        &event,
        &["--passthrough-tokens", "10", "--emit-updated-output"],
    );
    let forwarded = updated_output(&response).expect("must reduce and emit");
    let raw_ref = extract_raw_ref(forwarded);

    let show_output = fixture.show(&raw_ref);
    assert!(show_output.status.success(), "{:?}", show_output);
    assert_eq!(String::from_utf8_lossy(&show_output.stdout), original);
}

/// Map line 1985: the raw store round-trips byte-identically, and every
/// line the forwarded text does carry is a verbatim substring of the
/// original — reduction never generates replacement text.
#[test]
fn line_1985_show_reconstructs_the_original_byte_identically() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());

    let original = "alpha line\nbeta line\nalpha line\ngamma line\n".repeat(100);
    let event = post_tool_use("Grep", text_response(&original), "s-1985", "tu-1");
    let response = fixture.hook(
        &event,
        &["--passthrough-tokens", "10", "--emit-updated-output"],
    );
    let forwarded = updated_output(&response).expect("must reduce and emit");
    let raw_ref = extract_raw_ref(forwarded);

    let show_output = fixture.show(&raw_ref);
    let raw = String::from_utf8_lossy(&show_output.stdout).into_owned();
    assert_eq!(raw, original, "show must reconstruct byte-identically");

    for line in forwarded.lines() {
        if line.starts_with("[glasshouse context firewall") {
            continue;
        }
        assert!(
            original.contains(line),
            "forwarded line `{line}` is not a verbatim slice of the original"
        );
    }
}

/// Map line 1986: the provenance header states original/forwarded sizes,
/// retained/total candidate counts, and the raw reference — and only on a
/// reduced result, never on a passthrough one.
#[test]
fn line_1986_the_provenance_header_states_sizes_counts_and_reference() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());

    let original = "same line\n".repeat(300);
    let event = post_tool_use("Read", text_response(&original), "s-1986", "tu-1");
    let response = fixture.hook(
        &event,
        &["--passthrough-tokens", "10", "--emit-updated-output"],
    );
    let forwarded = updated_output(&response).expect("must reduce and emit");

    assert!(forwarded.starts_with("[glasshouse context firewall:"));
    assert!(forwarded.contains("kept "));
    assert!(forwarded.contains("gh-tool://"));

    // The passthrough case carries no such header (line 1981's own test
    // covers the header-free half in full; this asserts the contrast).
    let small_event = post_tool_use("Read", text_response("tiny\n"), "s-1986", "tu-2");
    let small_response = fixture.hook(&small_event, &["--emit-updated-output"]);
    assert_eq!(small_response, serde_json::json!({}));
}

/// Map line 1987: a reduction and a bypass each write their own row to the
/// existing routing-evidence ledger, under the two new purposes, never a
/// parallel metrics store.
#[test]
fn line_1987_reduction_and_bypass_are_recorded_in_the_evidence_ledger() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());

    let original = "same line\n".repeat(300);
    let reduce_event = post_tool_use("Read", text_response(&original), "s-1987", "tu-1");
    fixture.hook(&reduce_event, &["--passthrough-tokens", "10"]);

    let bypass_event = post_tool_use(
        "Edit",
        text_response("a diff\n".repeat(300).as_str()),
        "s-1987",
        "tu-2",
    );
    fixture.hook(&bypass_event, &["--passthrough-tokens", "10"]);

    let ledger = fixture.ledger();

    let reduced_rows = ledger
        .recent(
            ObservationQuery {
                provider: "glasshouse",
                model: "context-firewall",
                route: None,
                harness: Some("claude-code"),
            },
            10,
        )
        .unwrap();
    assert!(
        reduced_rows.iter().any(|row| row.purpose.as_deref()
            == Some(CONTEXT_FIREWALL_REDUCTION_PURPOSE)
            && row.quota_context.as_deref() == Some("Read")),
        "a reduction row must be recorded: {reduced_rows:?}"
    );

    let bypass_rows = ledger
        .recent(
            ObservationQuery {
                provider: "glasshouse",
                model: "context-firewall",
                route: Some("ineligible-tool"),
                harness: Some("claude-code"),
            },
            10,
        )
        .unwrap();
    assert!(
        bypass_rows.iter().any(|row| row.purpose.as_deref()
            == Some(CONTEXT_FIREWALL_BYPASS_PURPOSE)
            && row.quota_context.as_deref() == Some("Edit")),
        "a bypass row must be recorded with its reason: {bypass_rows:?}"
    );
}

/// Map line 1988: a raw-expansion request (`context-firewall show`) is
/// tracked as its own row, independent of the reduction that produced the
/// id — the recall signal design-decisions.md's Phase 57 section calls
/// primary.
#[test]
fn line_1988_raw_expansion_requests_are_tracked_as_their_own_rows() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());

    let original = "same line\n".repeat(300);
    let event = post_tool_use("Read", text_response(&original), "s-1988", "tu-1");
    let response = fixture.hook(
        &event,
        &["--passthrough-tokens", "10", "--emit-updated-output"],
    );
    let forwarded = updated_output(&response).unwrap();
    let raw_ref = extract_raw_ref(forwarded);

    fixture.show(&raw_ref);
    fixture.show("gh-tool://0000000000000000-00000000000000000000000000000000");

    let ledger = fixture.ledger();
    let found_rows = ledger
        .recent(
            ObservationQuery {
                provider: "glasshouse",
                model: "context-firewall",
                route: Some("found"),
                harness: None,
            },
            10,
        )
        .unwrap();
    assert!(
        found_rows
            .iter()
            .any(|row| row.purpose.as_deref() == Some(CONTEXT_FIREWALL_EXPANSION_PURPOSE)),
        "a found expansion request must be recorded: {found_rows:?}"
    );

    let not_found_rows = ledger
        .recent(
            ObservationQuery {
                provider: "glasshouse",
                model: "context-firewall",
                route: Some("not-found"),
                harness: None,
            },
            10,
        )
        .unwrap();
    assert!(
        not_found_rows
            .iter()
            .any(|row| row.purpose.as_deref() == Some(CONTEXT_FIREWALL_EXPANSION_PURPOSE)),
        "a not-found expansion request must be recorded too: {not_found_rows:?}"
    );
}

/// Map line 1989: the default eligibility list admits Grep/Glob/Read/Bash
/// and nothing else; Edit is hard-blocked even when explicitly named on
/// `--tool`.
#[test]
fn line_1989_tool_eligibility_defaults_and_hard_block_are_enforced() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());

    let big = "line\n".repeat(300);

    // Not on the default list, and not named on --tool: bypasses.
    let unnamed_event = post_tool_use("SomeOtherTextTool", text_response(&big), "s-1989", "tu-1");
    let unnamed_response = fixture.hook(
        &unnamed_event,
        &["--passthrough-tokens", "10", "--emit-updated-output"],
    );
    assert_eq!(unnamed_response, serde_json::json!({}));

    // Edit, explicitly named on --tool, is still never eligible.
    let edit_event = post_tool_use("Edit", text_response(&big), "s-1989", "tu-2");
    let edit_response = fixture.hook(
        &edit_event,
        &[
            "--passthrough-tokens",
            "10",
            "--tool",
            "Edit",
            "--emit-updated-output",
        ],
    );
    assert_eq!(edit_response, serde_json::json!({}));

    // Read, on the default list, reduces normally.
    let read_event = post_tool_use("Read", text_response(&big), "s-1989", "tu-3");
    let read_response = fixture.hook(
        &read_event,
        &["--passthrough-tokens", "10", "--emit-updated-output"],
    );
    assert!(updated_output(&read_response).is_some());
}

/// Map line 1990: a non-zero-exit Bash result is never reduced, and a
/// clean-exit Bash result's stderr survives verbatim beside its reduced
/// stdout.
#[test]
fn line_1990_bash_exit_status_and_stderr_survive_reduction() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());

    let big_stdout = "build step ok\n".repeat(300);

    // Non-zero exit: never reduced, response stays a no-op.
    let failed_event = post_tool_use(
        "Bash",
        bash_response(&big_stdout, "a real error\n", false, 1),
        "s-1990",
        "tu-1",
    );
    let failed_response = fixture.hook(
        &failed_event,
        &["--passthrough-tokens", "10", "--emit-updated-output"],
    );
    assert_eq!(failed_response, serde_json::json!({}));

    // Clean exit: stdout reduces, stderr survives untouched inside the
    // forwarded text.
    let clean_event = post_tool_use(
        "Bash",
        bash_response(&big_stdout, "a real warning that must survive\n", false, 0),
        "s-1990",
        "tu-2",
    );
    let clean_response = fixture.hook(
        &clean_event,
        &["--passthrough-tokens", "10", "--emit-updated-output"],
    );
    let forwarded = updated_output(&clean_response).expect("a clean exit must reduce and emit");
    assert!(forwarded.contains("a real warning that must survive"));
}

// ===========================================================================
// REQUIRED BEHAVIOR, beyond the eleven box lines.
// ===========================================================================

/// Two sessions reducing concurrently never collide in the raw store — each
/// session's own reference resolves to its own content.
#[test]
fn concurrent_sessions_never_collide_in_the_raw_store() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());

    let content_a = "session a content\n".repeat(300);
    let content_b = "session b content\n".repeat(300);

    let event_a = post_tool_use("Read", text_response(&content_a), "session-a", "tu-1");
    let event_b = post_tool_use("Read", text_response(&content_b), "session-b", "tu-1");

    let response_a = fixture.hook(
        &event_a,
        &["--passthrough-tokens", "10", "--emit-updated-output"],
    );
    let response_b = fixture.hook(
        &event_b,
        &["--passthrough-tokens", "10", "--emit-updated-output"],
    );

    let ref_a = extract_raw_ref(updated_output(&response_a).unwrap());
    let ref_b = extract_raw_ref(updated_output(&response_b).unwrap());
    assert_ne!(ref_a, ref_b);

    let show_a = fixture.show(&ref_a);
    let show_b = fixture.show(&ref_b);
    assert_eq!(String::from_utf8_lossy(&show_a.stdout), content_a);
    assert_eq!(String::from_utf8_lossy(&show_b.stdout), content_b);
}

/// The default hook response is a no-op even when a result was reduced;
/// `--emit-updated-output` is what asks the harness to substitute.
#[test]
fn the_default_response_is_a_no_op_until_emit_updated_output_is_set() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());

    let original = "line\n".repeat(300);
    let event = post_tool_use("Read", text_response(&original), "s-default", "tu-1");
    let response = fixture.hook(&event, &["--passthrough-tokens", "10"]);
    assert_eq!(
        response,
        serde_json::json!({}),
        "reduction still ran (line 1984's raw store test covers that); the response must not \
         substitute anything without the flag"
    );
}
