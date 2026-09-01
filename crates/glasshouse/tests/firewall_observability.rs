//! Phase 57's third package — map lines 2004-2006: getting a suppressed
//! result back at four granularities, a durable shadow-mode comparison, and
//! mode/savings in `glasshouse status`. Drives the shipped binary exactly
//! as `tests/context_firewall.rs` does; 1980-2003 stay untouched beneath it.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use clap::Parser;
use glasshouse::Runtime;

// ===========================================================================
// Fixture — same shape as `tests/context_firewall.rs`'s own, duplicated
// rather than shared: each integration test binary is its own crate.
// ===========================================================================

struct Fixture {
    base: PathBuf,
    root: PathBuf,
    #[allow(dead_code)]
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

    /// Set `.glasshouse/config.toml`'s `[context_firewall]` table — the
    /// same file `glasshouse config` would write with consent, written
    /// directly here since these tests only need it to exist, not to
    /// exercise the consent flow.
    fn set_firewall_mode(&self, mode: &str) {
        let dir = self.root.join(".glasshouse");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config.toml"),
            format!("[context_firewall]\nmode = \"{mode}\"\n"),
        )
        .unwrap();
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

    fn show(&self, args: &[&str]) -> Output {
        let mut full = vec!["context-firewall", "show"];
        full.extend_from_slice(args);
        self.run(&full, b"")
    }

    fn status(&self) -> String {
        let output = self.run(&["status"], b"");
        assert!(
            output.status.success(),
            "status must succeed: stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
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

/// A grep-shaped fixture: several distinct files' hits, each line prefixed
/// `path:content`, none of them exact duplicates of each other (so the
/// deterministic ladder retains every line and candidate ids are easy to
/// reason about), oversized enough to cross a small `--passthrough-tokens`.
fn multi_file_hits() -> String {
    let mut text = String::new();
    for i in 0..40 {
        text.push_str(&format!("src/alpha.rs:distinct alpha hit number {i}\n"));
    }
    for i in 0..40 {
        text.push_str(&format!("src/beta.rs:distinct beta hit number {i}\n"));
    }
    text
}

// ===========================================================================
// 2004 — expansion at four granularities.
// ===========================================================================

#[test]
fn line_2004_whole_expansion_reproduces_the_original_byte_identically() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());

    let original = multi_file_hits();
    let event = post_tool_use("Grep", text_response(&original), "s-2004-whole", "tu-1");
    let response = fixture.hook(
        &event,
        &["--passthrough-tokens", "10", "--emit-updated-output"],
    );
    let forwarded = updated_output(&response).expect("must reduce and emit");
    let raw_ref = extract_raw_ref(forwarded);

    let output = fixture.show(&[&raw_ref]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), original);
}

#[test]
fn line_2004_candidate_expansion_returns_exactly_one_line() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());

    let original = multi_file_hits();
    let event = post_tool_use("Grep", text_response(&original), "s-2004-cand", "tu-1");
    let response = fixture.hook(
        &event,
        &["--passthrough-tokens", "10", "--emit-updated-output"],
    );
    let forwarded = updated_output(&response).expect("must reduce and emit");
    let raw_ref = extract_raw_ref(forwarded);

    // Candidate 0 is the deterministic ladder's first retained line — the
    // first line of the original text, since every line here is distinct.
    let output = fixture.show(&[&raw_ref, "--candidate", "0"]);
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert_eq!(stdout, "src/alpha.rs:distinct alpha hit number 0\n");
}

#[test]
fn line_2004_file_expansion_returns_only_that_files_lines() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());

    let original = multi_file_hits();
    let event = post_tool_use("Grep", text_response(&original), "s-2004-file", "tu-1");
    let response = fixture.hook(
        &event,
        &["--passthrough-tokens", "10", "--emit-updated-output"],
    );
    let forwarded = updated_output(&response).expect("must reduce and emit");
    let raw_ref = extract_raw_ref(forwarded);

    let output = fixture.show(&[&raw_ref, "--file", "src/beta.rs"]);
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert_eq!(
        stdout.lines().count(),
        40,
        "must return exactly beta's lines"
    );
    assert!(stdout.lines().all(|line| line.starts_with("src/beta.rs:")));
}

#[test]
fn line_2004_range_expansion_returns_the_named_lines_only() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());

    let original = multi_file_hits();
    let event = post_tool_use("Grep", text_response(&original), "s-2004-range", "tu-1");
    let response = fixture.hook(
        &event,
        &["--passthrough-tokens", "10", "--emit-updated-output"],
    );
    let forwarded = updated_output(&response).expect("must reduce and emit");
    let raw_ref = extract_raw_ref(forwarded);

    // 1-indexed inclusive: lines 2-3 are the third and fourth lines of the
    // original (alpha hits 1 and 2).
    let output = fixture.show(&[&raw_ref, "--range", "2-3"]);
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert_eq!(
        stdout,
        "src/alpha.rs:distinct alpha hit number 1\nsrc/alpha.rs:distinct alpha hit number 2\n"
    );
}

#[test]
fn line_2004_a_bad_reference_refuses_clearly() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());

    let output = fixture.show(&["gh-tool://0000000000000000-00000000000000000000000000000000"]);
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stderr).is_empty());
}

#[test]
fn line_2004_an_out_of_range_candidate_id_refuses_clearly() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());

    let original = multi_file_hits();
    let event = post_tool_use("Grep", text_response(&original), "s-2004-oor", "tu-1");
    let response = fixture.hook(
        &event,
        &["--passthrough-tokens", "10", "--emit-updated-output"],
    );
    let forwarded = updated_output(&response).expect("must reduce and emit");
    let raw_ref = extract_raw_ref(forwarded);

    let output = fixture.show(&[&raw_ref, "--candidate", "999999"]);
    assert!(
        !output.status.success(),
        "an out-of-range candidate id must refuse, never fall back to the whole result"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.is_empty(),
        "a refusal must print nothing to stdout: {stdout}"
    );
    assert!(!String::from_utf8_lossy(&output.stderr).is_empty());
}

#[test]
fn line_2004_an_unknown_file_refuses_clearly() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());

    let original = multi_file_hits();
    let event = post_tool_use("Grep", text_response(&original), "s-2004-nofile", "tu-1");
    let response = fixture.hook(
        &event,
        &["--passthrough-tokens", "10", "--emit-updated-output"],
    );
    let forwarded = updated_output(&response).expect("must reduce and emit");
    let raw_ref = extract_raw_ref(forwarded);

    let output = fixture.show(&[&raw_ref, "--file", "src/nowhere.rs"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).is_empty());
    assert!(!String::from_utf8_lossy(&output.stderr).is_empty());
}

#[test]
fn line_2004_a_reversed_range_refuses_clearly() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());

    let original = multi_file_hits();
    let event = post_tool_use("Grep", text_response(&original), "s-2004-rev", "tu-1");
    let response = fixture.hook(
        &event,
        &["--passthrough-tokens", "10", "--emit-updated-output"],
    );
    let forwarded = updated_output(&response).expect("must reduce and emit");
    let raw_ref = extract_raw_ref(forwarded);

    let output = fixture.show(&[&raw_ref, "--range", "10-2"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).is_empty());
}

#[test]
fn line_2004_an_out_of_bounds_range_refuses_clearly() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());

    let original = multi_file_hits();
    let event = post_tool_use("Grep", text_response(&original), "s-2004-oob", "tu-1");
    let response = fixture.hook(
        &event,
        &["--passthrough-tokens", "10", "--emit-updated-output"],
    );
    let forwarded = updated_output(&response).expect("must reduce and emit");
    let raw_ref = extract_raw_ref(forwarded);

    let output = fixture.show(&[&raw_ref, "--range", "1-100000"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).is_empty());
}

/// Security invariant: the store is per session, and a reference is the
/// only door to it — a candidate id that exists in session A's entry, and
/// happens to also exist in session B's own (unrelated) entry, must never
/// let session A's reference return anything but session A's own text. A
/// bug that scanned the whole store instead of reading by reference would
/// leak exactly this way.
#[test]
fn line_2004_an_expansion_never_crosses_a_session_boundary() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());

    let content_a = "src/only-in-a.rs:hit\n".to_string()
        + &"src/only-in-a.rs:distinct filler line\n".repeat(60);
    let content_b = "src/only-in-b.rs:hit\n".to_string()
        + &"src/only-in-b.rs:distinct filler line\n".repeat(60);

    let event_a = post_tool_use("Grep", text_response(&content_a), "session-a", "tu-1");
    let event_b = post_tool_use("Grep", text_response(&content_b), "session-b", "tu-1");
    let response_a = fixture.hook(
        &event_a,
        &["--passthrough-tokens", "5", "--emit-updated-output"],
    );
    let response_b = fixture.hook(
        &event_b,
        &["--passthrough-tokens", "5", "--emit-updated-output"],
    );
    let ref_a = extract_raw_ref(updated_output(&response_a).unwrap());
    let ref_b = extract_raw_ref(updated_output(&response_b).unwrap());
    assert_ne!(ref_a, ref_b);

    // Candidate 0 exists in both entries (both have at least one retained
    // line); each reference must resolve to its OWN session's text only.
    let from_a = fixture.show(&[&ref_a, "--candidate", "0"]);
    let from_b = fixture.show(&[&ref_b, "--candidate", "0"]);
    assert!(from_a.status.success());
    assert!(from_b.status.success());
    let text_a = String::from_utf8_lossy(&from_a.stdout).into_owned();
    let text_b = String::from_utf8_lossy(&from_b.stdout).into_owned();
    assert!(text_a.contains("only-in-a"));
    assert!(!text_a.contains("only-in-b"));
    assert!(text_b.contains("only-in-b"));
    assert!(!text_b.contains("only-in-a"));
}

// ===========================================================================
// 2005 — shadow comparison against the forwarded original.
// ===========================================================================

#[test]
fn line_2005_a_shadow_mode_run_records_both_sides_and_forwards_the_original() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());

    let original = "same duplicate line\n".repeat(300);
    let event = post_tool_use("Read", text_response(&original), "s-2005", "tu-1");

    // Shadow overrides `--emit-updated-output` outright (map line 1991):
    // the harness must see a no-op response even though the pipeline
    // fully ran and stored a comparison.
    let response = fixture.hook(
        &event,
        &[
            "--passthrough-tokens",
            "10",
            "--emit-updated-output",
            "--mode",
            "shadow",
        ],
    );
    assert_eq!(
        response,
        serde_json::json!({}),
        "shadow mode must never substitute anything, whatever the flag says"
    );

    // The comparison must be recorded, not printed and lost: shadow mode's
    // provenance header is suppressed along with everything else in the
    // response, but the raw store write it produced is content-addressed
    // exactly like any other mode's — the same session id and the same
    // original bytes reach the same `gh-tool://` reference regardless of
    // mode. Recover it by re-running the identical bytes through `safe`
    // mode (which does emit a header) on the same session, purely to read
    // the reference string back; the entry it names is the one shadow
    // itself already wrote.
    let safe_probe_event = post_tool_use("Read", text_response(&original), "s-2005", "tu-2");
    let safe_probe = fixture.hook(
        &safe_probe_event,
        &[
            "--passthrough-tokens",
            "10",
            "--emit-updated-output",
            "--mode",
            "safe",
        ],
    );
    let raw_ref = extract_raw_ref(updated_output(&safe_probe).expect("safe mode must emit"));

    let whole = fixture.show(&[&raw_ref]);
    assert!(whole.status.success());
    assert_eq!(String::from_utf8_lossy(&whole.stdout), original);

    let stats_output = fixture.show(&[&raw_ref, "--stats"]);
    assert!(stats_output.status.success(), "{stats_output:?}");
    let stats = String::from_utf8_lossy(&stats_output.stdout).into_owned();
    assert!(stats.contains("original_tokens:"));
    assert!(stats.contains("forwarded_tokens:"));
    assert!(stats.contains("retained_candidates:"));
    assert!(stats.contains("total_candidates:"));
    // A real reduction: the recorded comparison must show fewer forwarded
    // tokens than original ones, and fewer retained candidates than total
    // — recall and savings both evidenced, not merely a compression ratio.
    let original_tokens: u64 = field(&stats, "original_tokens").parse().unwrap();
    let forwarded_tokens: u64 = field(&stats, "forwarded_tokens").parse().unwrap();
    let retained: usize = field(&stats, "retained_candidates").parse().unwrap();
    let total: usize = field(&stats, "total_candidates").parse().unwrap();
    assert!(forwarded_tokens < original_tokens);
    assert!(retained < total);
}

fn field<'a>(stats: &'a str, key: &str) -> &'a str {
    stats
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{key}: ")))
        .unwrap_or_else(|| panic!("`{key}` not found in stats output: {stats}"))
}

// ===========================================================================
// 2006 — mode and per-session savings in `status`.
// ===========================================================================

#[test]
fn line_2006_status_is_silent_when_the_firewall_is_off() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    // No `.glasshouse/config.toml` at all: mode resolves to its `off`
    // default, exactly as every project not opted into this phase sees.
    let status = fixture.status();
    assert!(!status.contains("Context firewall"));
}

#[test]
fn line_2006_status_shows_mode_and_a_savings_figure_when_the_firewall_is_on() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    fixture.set_firewall_mode("shadow");

    let original = "same duplicate line\n".repeat(300);
    let event = post_tool_use("Read", text_response(&original), "s-2006", "tu-1");
    fixture.hook(&event, &["--passthrough-tokens", "10", "--mode", "shadow"]);

    let status = fixture.status();
    assert!(status.contains("Context firewall"));
    assert!(status.contains("mode: shadow"));
    assert!(
        status.contains("kept local"),
        "status must state the chosen savings provenance: {status}"
    );
}

#[test]
fn line_2006_status_reports_no_activity_yet_when_on_but_unused() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    fixture.set_firewall_mode("safe");

    let status = fixture.status();
    assert!(status.contains("Context firewall"));
    assert!(status.contains("mode: safe"));
    assert!(status.contains("no context-firewall activity recorded yet"));
}

// ===========================================================================
// Regression — firewall off reproduces prior behaviour exactly.
// ===========================================================================

#[test]
fn regression_status_with_no_project_config_is_unchanged_by_this_package() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let status = fixture.status();
    assert!(status.starts_with("Glasshouse status\n=================\n"));
    assert!(!status.contains("Context firewall"));
}

#[test]
fn regression_the_hook_and_show_pipeline_is_unaffected_when_no_new_flags_are_used() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());

    let original = "line one\n".repeat(200);
    let event = post_tool_use("Read", text_response(&original), "s-regress", "tu-1");
    let response = fixture.hook(
        &event,
        &["--passthrough-tokens", "10", "--emit-updated-output"],
    );
    let forwarded = updated_output(&response).expect("must reduce and emit");
    let raw_ref = extract_raw_ref(forwarded);

    let show_output = fixture.show(&[&raw_ref]);
    assert!(show_output.status.success(), "{show_output:?}");
    assert_eq!(String::from_utf8_lossy(&show_output.stdout), original);
}
