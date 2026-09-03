//! Capability map lines 2402–2405 — Phase 60's edit-intent slice: a session
//! records what it is about to change, is told when another live session
//! already claimed the same file, and is **never** stopped.
//!
//! # Everything here drives the shipped binary
//!
//! Practice §35: a caller every test bypasses is not a caller. Every test
//! below runs `glasshouse edit-intent hook` in its own process with a real
//! `PreToolUse` document on standard input, exactly as Claude Code runs it —
//! including the captured payload from the installed harness. The store is
//! opened directly only to assert on rows the hook's own stdout is not meant
//! to print.
//!
//! # The one thing this file exists to prove
//!
//! `permissionDecision` is `allow` on every path. `PreToolUse` is a gate, and
//! steering decision 4 rules soft coordination with no blocking; a mutation
//! that makes the conflict path answer `deny` has to fail here.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use clap::Parser;

use glasshouse::session::{FileClaim, NewSession, ProjectSessions, SessionId, SessionLifecycle};
use glasshouse::{Cli, Runtime};

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
    runtime: Runtime,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        let runtime = bootstrap(&base, &root);
        Self {
            _tmp: tmp,
            base,
            root,
            runtime,
        }
    }

    /// `glasshouse edit-intent hook`, exactly as Claude Code runs it: its own
    /// process, the session on argv, one `PreToolUse` document on standard
    /// input. Returns the parsed response.
    fn hook(&self, session: Option<&SessionId>, payload: &str) -> serde_json::Value {
        let (status, stdout, stderr) = self.hook_raw(session, payload);
        assert!(
            status.success(),
            "a PreToolUse hook that exits non-zero vetoes the tool call; stderr: {stderr}"
        );
        serde_json::from_str(&stdout)
            .unwrap_or_else(|err| panic!("the hook must answer with JSON ({err}): {stdout:?}"))
    }

    fn hook_raw(
        &self,
        session: Option<&SessionId>,
        payload: &str,
    ) -> (std::process::ExitStatus, String, String) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_glasshouse"));
        command
            .current_dir(&self.root)
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(["edit-intent", "hook"]);
        if let Some(session) = session {
            command.args(["--session", session.as_str()]);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the glasshouse binary must be runnable");
        child
            .stdin
            .as_mut()
            .expect("stdin was piped")
            .write_all(payload.as_bytes())
            .expect("the hook must read its payload rather than closing the pipe");
        let output = child.wait_with_output().expect("the hook must exit");
        (
            output.status,
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }

    fn claims(&self) -> Vec<FileClaim> {
        ProjectSessions::open(&self.runtime)
            .unwrap()
            .store()
            .active_claims()
            .unwrap()
    }
}

fn bootstrap(base: &Path, root: &Path) -> Runtime {
    let cli = Cli::try_parse_from([
        "glasshouse",
        "--data-dir",
        base.join("data").to_str().unwrap(),
        "--config-dir",
        base.join("config").to_str().unwrap(),
    ])
    .unwrap();
    glasshouse::bootstrap(&cli, root).unwrap()
}

fn running_session(runtime: &Runtime) -> SessionId {
    let sessions = ProjectSessions::open(runtime).unwrap();
    let store = sessions.store();
    let record = store.create(NewSession::embedded("claude-code")).unwrap();
    store
        .set_lifecycle(&record.id, SessionLifecycle::Running)
        .unwrap();
    record.id
}

/// A `PreToolUse` document in the shape the installed Claude Code actually
/// sends — the capture in `firewall::adapter`'s own tests, with the path
/// substituted.
fn write_event(root: &Path, relative: &str) -> String {
    serde_json::json!({
        "session_id": "claude-code-own-session",
        "transcript_path": "/capture/transcript.jsonl",
        "cwd": root.display().to_string(),
        "prompt_id": "capture-prompt",
        "permission_mode": "acceptEdits",
        "effort": {"level": "xhigh"},
        "hook_event_name": "PreToolUse",
        "tool_name": "Write",
        "tool_input": {
            "file_path": root.join(relative).display().to_string(),
            "content": "done",
        },
        "tool_use_id": "capture-tool-use-id",
    })
    .to_string()
}

fn decision(response: &serde_json::Value) -> &serde_json::Value {
    &response["hookSpecificOutput"]["permissionDecision"]
}

// ---------------------------------------------------------------------------
// 2402 — the intent is recorded before the operation runs.
// ---------------------------------------------------------------------------

#[test]
fn a_write_records_an_edit_intent_for_the_path_it_names() {
    let fixture = Fixture::new();
    let id = running_session(&fixture.runtime);

    let response = fixture.hook(Some(&id), &write_event(&fixture.root, "src/main.rs"));
    assert_eq!(decision(&response), &serde_json::json!("allow"));

    let claims = fixture.claims();
    assert_eq!(claims.len(), 1, "{claims:?}");
    assert_eq!(claims[0].path, "src/main.rs");
    assert_eq!(claims[0].session_id, id);
}

/// The distinction `LifecycleEvent::FileTouched` already draws: an intent to
/// edit is not earned by a glance.
#[test]
fn a_read_records_nothing_and_still_allows() {
    let fixture = Fixture::new();
    let id = running_session(&fixture.runtime);

    let payload = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Read",
        "tool_input": {"file_path": fixture.root.join("src/main.rs").display().to_string()},
    })
    .to_string();
    let response = fixture.hook(Some(&id), &payload);

    assert_eq!(decision(&response), &serde_json::json!("allow"));
    assert!(fixture.claims().is_empty(), "{:?}", fixture.claims());
}

/// Project isolation, at the producer: a file outside the root is never
/// recorded, not even to be filtered out later.
#[test]
fn a_path_outside_the_project_is_not_recorded_and_still_allows() {
    let fixture = Fixture::new();
    let id = running_session(&fixture.runtime);

    let payload = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Write",
        "tool_input": {"file_path": "/etc/hosts", "content": "x"},
    })
    .to_string();
    let response = fixture.hook(Some(&id), &payload);

    assert_eq!(decision(&response), &serde_json::json!("allow"));
    assert!(fixture.claims().is_empty(), "{:?}", fixture.claims());
}

/// Line 2395 through this producer: a session writing the same file twice in
/// a turn renews rather than accumulating rows.
#[test]
fn writing_the_same_file_twice_renews_one_intent() {
    let fixture = Fixture::new();
    let id = running_session(&fixture.runtime);
    let event = write_event(&fixture.root, "src/main.rs");

    fixture.hook(Some(&id), &event);
    fixture.hook(Some(&id), &event);

    let claims = fixture.claims();
    assert_eq!(claims.len(), 1, "a renew is not a second row: {claims:?}");
}

// ---------------------------------------------------------------------------
// 2403 — the comparison with other sessions' live claims.
// ---------------------------------------------------------------------------

/// **The MVP behaviour**, end to end and through the shipped binary: two
/// active sessions express edit intent for the same file, and Glasshouse
/// detects the direct overlap and explains it.
#[test]
fn a_second_session_editing_the_same_file_is_told_who_holds_it() {
    let fixture = Fixture::new();
    let first = running_session(&fixture.runtime);
    let second = running_session(&fixture.runtime);
    let event = write_event(&fixture.root, "src/main.rs");

    let quiet = fixture.hook(Some(&first), &event);
    assert_eq!(
        quiet,
        serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "allow",
            }
        }),
        "the first session conflicts with nobody and must be told nothing"
    );

    let reported = fixture.hook(Some(&second), &event);
    let message = reported["systemMessage"]
        .as_str()
        .unwrap_or_else(|| panic!("the conflict must be surfaced: {reported}"));
    assert!(message.contains("src/main.rs"), "{message}");
    assert!(
        message.contains(&first.as_str()[..12]),
        "the message must name the session that holds it: {message}"
    );
    assert_eq!(
        reported["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .expect("the model is told too"),
        message,
        "all three channels carry the same sentence"
    );
    assert_eq!(
        reported["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .expect("the harness records why"),
        message
    );

    // Both sessions hold their intent afterwards. Two claims on one path is
    // the overlap, not an error.
    let claims = fixture.claims();
    assert_eq!(claims.len(), 2, "{claims:?}");
    assert!(claims.iter().all(|claim| claim.path == "src/main.rs"));
}

/// Map lines 2409-2410: the same-path collision the hook detects is named as
/// a direct file overlap and treated as the high-confidence case, and the
/// message says plainly that broader semantic overlap is not assessed.
#[test]
fn a_conflict_is_named_a_direct_file_overlap_and_says_semantic_overlap_is_not_assessed() {
    let fixture = Fixture::new();
    let first = running_session(&fixture.runtime);
    let second = running_session(&fixture.runtime);
    let event = write_event(&fixture.root, "src/main.rs");

    fixture.hook(Some(&first), &event);
    let reported = fixture.hook(Some(&second), &event);
    let message = reported["systemMessage"]
        .as_str()
        .unwrap_or_else(|| panic!("the conflict must be surfaced: {reported}"));

    assert!(
        message.contains("direct file overlap"),
        "the overlap must be named: {message}"
    );
    assert!(
        message.contains("high-confidence"),
        "line 2409's own word must appear: {message}"
    );
    assert!(
        message.contains("semantic overlap is not assessed"),
        "line 2410's distinction must be stated, not merely absent: {message}"
    );
}

/// A session's own earlier intent is not a conflict with itself.
#[test]
fn a_session_does_not_conflict_with_its_own_claim() {
    let fixture = Fixture::new();
    let id = running_session(&fixture.runtime);
    let event = write_event(&fixture.root, "src/main.rs");

    fixture.hook(Some(&id), &event);
    let again = fixture.hook(Some(&id), &event);
    assert!(
        again.get("systemMessage").is_none(),
        "a session's own claim is not a conflict: {again}"
    );
}

/// A claim whose owning session is no longer live is invisible to the read,
/// so it can never produce a conflict against a ghost.
#[test]
fn a_stopped_sessions_claim_is_not_reported_as_a_conflict() {
    let fixture = Fixture::new();
    let first = running_session(&fixture.runtime);
    let second = running_session(&fixture.runtime);
    let event = write_event(&fixture.root, "src/main.rs");

    fixture.hook(Some(&first), &event);
    ProjectSessions::open(&fixture.runtime)
        .unwrap()
        .store()
        .set_lifecycle(&first, SessionLifecycle::Stopped)
        .unwrap();

    let response = fixture.hook(Some(&second), &event);
    assert!(
        response.get("systemMessage").is_none(),
        "a stopped session holds nothing: {response}"
    );
    assert_eq!(decision(&response), &serde_json::json!("allow"));
}

// ---------------------------------------------------------------------------
// 2405 — always allow, on every path. This is the mutation target.
// ---------------------------------------------------------------------------

/// **The decisive test.** `PreToolUse` is a gate and this build must never
/// use it as one. A conflict is told, never enforced.
#[test]
fn a_conflict_never_denies_the_operation() {
    let fixture = Fixture::new();
    let first = running_session(&fixture.runtime);
    let second = running_session(&fixture.runtime);
    let event = write_event(&fixture.root, "src/main.rs");

    fixture.hook(Some(&first), &event);
    let reported = fixture.hook(Some(&second), &event);

    assert!(
        reported.get("systemMessage").is_some(),
        "this test is only meaningful on the conflict path: {reported}"
    );
    assert_eq!(
        decision(&reported),
        &serde_json::json!("allow"),
        "soft coordination: a conflict is told, never enforced — {reported}"
    );
    assert_ne!(decision(&reported), &serde_json::json!("deny"));
    assert_ne!(decision(&reported), &serde_json::json!("ask"));
}

/// Every way this hook can fail ends in the same quiet allowance. A
/// coordination layer that broke a user's edit because its own lookup failed
/// would be worse than no coordination at all.
#[test]
fn every_failure_path_allows_and_stays_silent() {
    let fixture = Fixture::new();
    let id = running_session(&fixture.runtime);
    let quiet = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
        }
    });

    // Not JSON at all.
    assert_eq!(fixture.hook(Some(&id), "not json"), quiet);
    // JSON, but not an event.
    assert_eq!(fixture.hook(Some(&id), "[]"), quiet);
    // An event naming a tool this build has never heard of.
    assert_eq!(
        fixture.hook(Some(&id), r#"{"tool_name":"SomeMcpTool","tool_input":{}}"#),
        quiet
    );
    // An empty payload.
    assert_eq!(fixture.hook(Some(&id), ""), quiet);
    // No `--session`, so nothing can be attributed.
    assert_eq!(
        fixture.hook(None, &write_event(&fixture.root, "src/main.rs")),
        quiet
    );
    // A session this project does not have.
    assert_eq!(
        fixture.hook(
            Some(&SessionId::new("no-such-session")),
            &write_event(&fixture.root, "src/main.rs")
        ),
        quiet
    );

    assert!(
        fixture.claims().is_empty(),
        "no failure path may leave a claim behind: {:?}",
        fixture.claims()
    );
}

/// A hook that exits non-zero vetoes the tool call outright, whatever its
/// stdout says. Every payload above must exit zero, and this states it as its
/// own fact rather than as a side condition of `hook`.
#[test]
fn the_hook_always_exits_zero() {
    let fixture = Fixture::new();
    let id = running_session(&fixture.runtime);
    for payload in [
        "",
        "not json",
        "[]",
        r#"{"tool_name":"Write"}"#,
        &write_event(&fixture.root, "src/main.rs"),
    ] {
        let (status, stdout, stderr) = fixture.hook_raw(Some(&id), payload);
        assert!(
            status.success(),
            "payload {payload:?} exited {status:?}; stdout {stdout:?} stderr {stderr:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 2404/2405 — the bypass, and the harnesses that have no hook at all.
// ---------------------------------------------------------------------------

/// Line 2405: the off switch is one word in a configuration file, and it
/// installs nothing rather than installing something inert.
#[test]
fn the_configured_bypass_is_off_and_the_default_is_on() {
    use glasshouse::config::firewall::{EditIntentConfig, EditIntentMode};
    use glasshouse::config::{EffectiveConfig, UserConfig};

    let mut user = UserConfig::default();
    assert!(user.edit_intent().is_unset());
    assert_eq!(
        EffectiveConfig::new(&user, None).edit_intent_mode().value,
        EditIntentMode::On,
        "an undecided stack coordinates"
    );

    user.edit_intent_mut().set_mode(Some(EditIntentMode::Off));
    assert_eq!(
        EffectiveConfig::new(&user, None).edit_intent_mode().value,
        EditIntentMode::Off
    );

    // And a project overrides the user, in both directions.
    let mut project = glasshouse::config::ProjectConfig::default();
    project.edit_intent_mut().set_mode(Some(EditIntentMode::On));
    assert_eq!(
        EffectiveConfig::new(&user, Some(&project))
            .edit_intent_mode()
            .value,
        EditIntentMode::On
    );

    // The table a user actually writes.
    let parsed: EditIntentConfig = toml::from_str("mode = \"off\"").unwrap();
    assert_eq!(parsed.mode(), Some(EditIntentMode::Off));
}

/// Line 2404: a harness with no structured pre-tool hook simply does not
/// have this feature, and `glasshouse doctor` says so instead of leaving a
/// reader to assume Glasshouse is watching their terminal.
#[test]
fn doctor_says_which_harnesses_have_no_pre_tool_hook() {
    let fixture = Fixture::new();
    let report = glasshouse::integrations::doctor_report(&fixture.runtime);

    assert!(
        report.contains("edit intent:"),
        "the capability must be reported per adapter: {report}"
    );
    assert!(
        report.contains("`PreToolUse`, so file coordination is available"),
        "Claude Code has a verified bridge and the report must say so: {report}"
    );
    assert!(
        report.contains("file coordination is unavailable for this harness"),
        "and the harnesses without one must be named as unavailable: {report}"
    );
    // Exactly one adapter claims the capability, and the rest say plainly
    // that nothing stands in for it.
    let available = report.matches("file coordination is available").count();
    let unavailable = report.matches("file coordination is unavailable").count();
    assert_eq!(available, 1, "{report}");
    assert!(unavailable >= 5, "{unavailable} unavailable rows: {report}");
}

// ---------------------------------------------------------------------------
// The two hooks in one settings document.
// ---------------------------------------------------------------------------

/// The regression the packet names: a merge that clobbered the context
/// firewall's `PostToolUse` entry would be a silent security regression.
#[test]
fn the_coordination_hook_and_the_firewall_hook_share_one_document() {
    use glasshouse::harness::claude_code;

    let document = serde_json::json!({
        "outputStyle": "Concise",
        "hooks": {
            "Stop": [{"hooks": [{"type": "command", "command": "lifecycle", "timeout": 5}]}]
        }
    })
    .to_string();

    let firewall = claude_code::context_firewall_hook_entry(
        "glasshouse context-firewall hook --mode safe --session s",
    );
    let intent = claude_code::edit_intent_hook_entry(&claude_code::edit_intent_command_line(
        std::path::Path::new("/bin/glasshouse"),
        "s",
    ));

    let merged = claude_code::merge_context_firewall_hook(&document, &firewall).unwrap();
    let merged = claude_code::merge_edit_intent_hook(&merged, &intent).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&merged).unwrap();

    assert_eq!(parsed["outputStyle"], "Concise");
    assert_eq!(
        parsed["hooks"]["Stop"][0]["hooks"][0]["command"],
        "lifecycle"
    );
    assert_eq!(
        parsed["hooks"]["PostToolUse"][0]["hooks"][0]["command"],
        "glasshouse context-firewall hook --mode safe --session s"
    );
    assert!(
        parsed["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("edit-intent hook --session s")
    );
    // The coordination hook is spawned only for the tools that change files.
    assert_eq!(
        parsed["hooks"]["PreToolUse"][0]["matcher"],
        claude_code::edit_intent_tool_matcher()
    );
    assert_eq!(parsed["hooks"]["PostToolUse"][0]["matcher"], "*");
}
