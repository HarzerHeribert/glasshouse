//! Phase 58, map lines 2028-2030 — the local out-of-process reducer, through
//! the shipped binary against small fake tools on the local filesystem:
//! `main.rs::disposable_reducer`'s `local:` branch and
//! `firewall::reducer::LocalToolReducer` are the production callers under
//! test, and a fake `Reducer` built by hand (as `firewall::mod`'s own unit
//! tests use) would not exercise either.
//!
//! Mirrors `tests/firewall_reducer.rs`'s fixture shape for the model-backed
//! reducer, one seat over: same `Fixture`, `hook`/`show` helpers, and
//! provenance-header assertions, applied to
//! `[context_firewall.local_reducers.<name>]` instead of a canned HTTP
//! endpoint.
//!
//! Unix only, and honestly so: every fake tool below is a shebang script
//! (`#!/bin/sh`, `#!/usr/bin/env python3`) marked executable and spawned as
//! bare argv, which Windows cannot execute at all. The Windows VM leg found
//! this file failing to compile on 2026-09-02 (`std::os::unix` and
//! `set_mode`); the seat itself compiles on Windows, and a Windows-shaped
//! fake tool (a `.exe` or a `cmd` wrapper) is the recorded gap in
//! `docs/product/evidence/phase-58.md`, not something this gate hides.
#![cfg(unix)]

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use clap::Parser;
use glasshouse::config::firewall::LocalReducerConfig;
use glasshouse::config::{EntitlementConfig, EntitlementCredential, UserConfig};
use glasshouse::{Cli, Runtime};
use rusqlite::Connection;

const CREDENTIAL_VAR: &str = "GLASSHOUSE_TEST_ONLY_LOCAL_REDUCER_ENTITLEMENT_KEY";
const CREDENTIAL: &str = "sk-fabricated-local-reducer-test-credential-not-a-real-value";

// ===========================================================================
// A project, and the binary run against it — `tests/firewall_reducer.rs`'s
// own `Fixture` shape, duplicated rather than shared: each integration test
// binary is its own crate.
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

    fn config(&self) -> UserConfig {
        UserConfig::load(self.runtime.paths()).unwrap()
    }

    fn save(&self, user: UserConfig) {
        user.save(self.runtime.paths()).unwrap();
    }

    /// `[context_firewall] reducer = "local:<name>"` plus
    /// `[context_firewall.local_reducers.<name>]` — the table
    /// `main.rs::disposable_reducer`'s `local:` branch reads.
    fn set_local_reducer(
        &self,
        name: &str,
        command: Vec<String>,
        version: Option<&str>,
        timeout_ms: Option<u64>,
    ) {
        let mut user = self.config();
        user.context_firewall_mut()
            .set_reducer(Some(format!("local:{name}")))
            .set_local_reducer(
                name,
                LocalReducerConfig {
                    command,
                    version: version.map(str::to_owned),
                    timeout_ms,
                },
            );
        self.save(user);
    }

    /// An entitlement naming `CREDENTIAL_VAR` as its credential — the exact
    /// shape `EffectiveConfig::foreign_entitlement_credential_vars` scrubs.
    /// Plants the one thing test (f) proves a local reducer's subprocess
    /// never receives.
    fn add_entitlement_with_credential(&self, name: &str) {
        let mut user = self.config();
        let mut entitlement = EntitlementConfig::default();
        entitlement
            .set_provider(Some("unused-provider".to_owned()))
            .set_credential(Some(EntitlementCredential::environment(CREDENTIAL_VAR)));
        user.entitlements_mut().set(name, entitlement);
        self.save(user);
    }

    fn run(&self, args: &[&str], stdin_bytes: &[u8]) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .env(CREDENTIAL_VAR, CREDENTIAL)
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
        let mut args = vec!["context-firewall", "hook", "--emit-updated-output"];
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

    fn db(&self) -> Connection {
        Connection::open(self.runtime.database_path()).unwrap()
    }

    /// The `model` (Phase 58 map line 2030: the tool's own reported
    /// `tool_version`) of the most recent `routing_observations` row
    /// attributed to `provider` — the real reducer-call row
    /// `record_context_firewall_telemetry` writes whenever a call actually
    /// completed with a parseable reply.
    fn last_reduction_model_for(&self, provider: &str) -> Option<String> {
        self.db()
            .query_row(
                "SELECT model FROM routing_observations WHERE provider = ?1 \
                 ORDER BY seq DESC LIMIT 1",
                [provider],
                |row| row.get(0),
            )
            .ok()
    }
}

fn post_tool_use(
    tool_name: &str,
    tool_response: serde_json::Value,
    tool_input: serde_json::Value,
    session_id: &str,
    tool_use_id: &str,
) -> serde_json::Value {
    serde_json::json!({
        "tool_name": tool_name,
        "tool_input": tool_input,
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

/// `count` fully distinct lines, each long enough on its own to cross a
/// `--passthrough-tokens 10` / `--min-semantic-tokens 10` gate once joined —
/// deliberately never repeated, so the deterministic ladder's own dedup and
/// blob elision never touch them and every line becomes exactly one
/// candidate, in appearance order (`firewall::reduce::reduce`'s own
/// `id: candidates.len()` at push time).
fn many_unique_lines(count: usize) -> String {
    let mut text = String::new();
    for i in 0..count {
        text.push_str(&format!(
            "distinct unique line number {i} with enough padding to cross the token minimum\n"
        ));
    }
    text
}

/// Write `contents` to `dir/name` and mark it executable — the shape every
/// fake local-reducer tool below needs to be spawnable as bare argv, no
/// shell involved.
fn write_script(dir: &Path, name: &str, contents: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, contents).unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

/// (a)/(e)/(f)'s fake tool: answers the local-reducer contract, marking
/// even-indexed candidates `relevant` and odd-indexed ones `discard`,
/// always reporting `tool_version` `1.2.3`. When invoked with one argument,
/// it dumps its own raw stdin and its full environment to that path as
/// JSON first — test (f)'s own evidence that the request never carries the
/// task string or a credential variable.
const ANSWERING_SCRIPT: &str = "\
#!/usr/bin/env python3
import json
import os
import sys

data = sys.stdin.read()
if len(sys.argv) > 1:
    with open(sys.argv[1], \"w\") as dump_file:
        json.dump({\"stdin\": data, \"env\": dict(os.environ)}, dump_file)

request = json.loads(data)
verdicts = []
for index, candidate in enumerate(request[\"candidates\"]):
    relevance = \"relevant\" if index % 2 == 0 else \"discard\"
    verdicts.append({\"id\": candidate[\"id\"], \"relevance\": relevance, \"reason\": \"fixture\"})

print(json.dumps({\"version\": 1, \"tool_version\": \"1.2.3\", \"verdicts\": verdicts}))
";

/// (b)'s fake tool: reads stdin, then sleeps well past any timeout this
/// file configures — proving the hook kills it rather than waiting.
const SLEEPING_SCRIPT: &str = "\
#!/usr/bin/env python3
import sys
import time

sys.stdin.read()
time.sleep(30)
print('{\"version\": 1, \"tool_version\": \"1.0.0\", \"verdicts\": []}')
";

/// (c)'s fake tool: exits 0 but answers with something that is not JSON at
/// all — `local-reducer-failed`'s \"a reply that is not the contract\" half.
const GARBAGE_SCRIPT: &str = "\
#!/bin/sh
cat >/dev/null
echo 'not json at all'
";

// ===========================================================================
// (a) A local reducer that answers the contract.
// ===========================================================================

#[test]
fn a_local_reducer_that_answers_the_contract_rebuilds_the_forwarded_result_from_originals() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let script = write_script(tmp.path(), "answering.py", ANSWERING_SCRIPT);

    fixture.set_local_reducer(
        "fake",
        vec![script.to_str().unwrap().to_owned()],
        None,
        None,
    );

    let text = many_unique_lines(10);
    let event = post_tool_use(
        "Grep",
        text_response(&text),
        serde_json::json!({}),
        "s-local-answer",
        "tu-1",
    );
    let response = fixture.hook(
        &event,
        &["--passthrough-tokens", "10", "--min-semantic-tokens", "10"],
    );
    let forwarded = updated_output(&response).expect("must reduce and emit");

    assert!(
        forwarded.contains("semantic reduction by local:fake 1.2.3 kept"),
        "the header must name the local reducer and its reported version: {forwarded}"
    );

    let survivors = (0..10)
        .filter(|i| forwarded.contains(&format!("distinct unique line number {i} ")))
        .count();
    assert_eq!(
        survivors, 5,
        "exactly the even-indexed half the fake tool marked relevant must survive, rebuilt \
         from the original candidates: {forwarded}"
    );
    for i in (0..10).step_by(2) {
        assert!(
            forwarded.contains(&format!("distinct unique line number {i} ")),
            "candidate {i} was marked relevant and must survive: {forwarded}"
        );
    }
    for i in (1..10).step_by(2) {
        assert!(
            !forwarded.contains(&format!("distinct unique line number {i} ")),
            "candidate {i} was marked discard and must not survive: {forwarded}"
        );
    }

    let model = fixture
        .last_reduction_model_for("local:fake")
        .expect("a real reducer-call row must be recorded, attributed to `local:fake`");
    assert_eq!(model, "1.2.3");
}

// ===========================================================================
// (b) A local reducer that sleeps past its timeout.
// ===========================================================================

#[test]
fn a_local_reducer_that_sleeps_past_its_timeout_bypasses_and_the_hook_still_answers() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let script = write_script(tmp.path(), "sleeping.py", SLEEPING_SCRIPT);

    fixture.set_local_reducer(
        "slow",
        vec![script.to_str().unwrap().to_owned()],
        None,
        Some(300),
    );

    let text = many_unique_lines(10);
    let event = post_tool_use(
        "Grep",
        text_response(&text),
        serde_json::json!({}),
        "s-local-timeout",
        "tu-1",
    );

    let started = std::time::Instant::now();
    let response = fixture.hook(
        &event,
        &["--passthrough-tokens", "10", "--min-semantic-tokens", "10"],
    );
    let elapsed = started.elapsed();
    let forwarded = updated_output(&response).expect("must still reduce and emit");

    assert!(
        forwarded.contains("distinct unique line number 0 "),
        "a timed-out reducer must never lose the deterministic result: {forwarded}"
    );
    assert!(
        forwarded.contains("semantic reduction bypassed (local-reducer-timeout)"),
        "the header must say the local reducer timed out: {forwarded}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "the hook must kill the tool at its configured timeout rather than waiting out the \
         tool's own 30-second sleep: {elapsed:?}"
    );
}

// ===========================================================================
// (c) A local reducer that prints garbage.
// ===========================================================================

#[test]
fn a_local_reducer_that_prints_garbage_bypasses_as_failed() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let script = write_script(tmp.path(), "garbage.sh", GARBAGE_SCRIPT);

    fixture.set_local_reducer(
        "garbage",
        vec![script.to_str().unwrap().to_owned()],
        None,
        None,
    );

    let text = many_unique_lines(10);
    let event = post_tool_use(
        "Grep",
        text_response(&text),
        serde_json::json!({}),
        "s-local-garbage",
        "tu-1",
    );
    let response = fixture.hook(
        &event,
        &["--passthrough-tokens", "10", "--min-semantic-tokens", "10"],
    );
    let forwarded = updated_output(&response).expect("must still reduce and emit");

    assert!(forwarded.contains("distinct unique line number 0 "));
    assert!(
        forwarded.contains("semantic reduction bypassed (local-reducer-failed)"),
        "a reply that is not the contract must bypass as failed: {forwarded}"
    );
}

// ===========================================================================
// (d) An absent local reducer command.
// ===========================================================================

#[test]
fn an_absent_local_reducer_command_bypasses_as_absent() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let missing = tmp.path().join("does-not-exist-tool");

    fixture.set_local_reducer(
        "missing",
        vec![missing.to_str().unwrap().to_owned()],
        None,
        None,
    );

    let text = many_unique_lines(10);
    let event = post_tool_use(
        "Grep",
        text_response(&text),
        serde_json::json!({}),
        "s-local-absent",
        "tu-1",
    );
    let response = fixture.hook(
        &event,
        &["--passthrough-tokens", "10", "--min-semantic-tokens", "10"],
    );
    let forwarded = updated_output(&response).expect("must still reduce and emit");

    assert!(forwarded.contains("distinct unique line number 0 "));
    assert!(
        forwarded.contains("semantic reduction bypassed (local-reducer-absent)"),
        "a command that cannot be started must bypass as absent: {forwarded}"
    );
}

// ===========================================================================
// (e) A local reducer reporting a version the pin refuses.
// ===========================================================================

#[test]
fn a_local_reducer_reporting_an_unpinned_version_bypasses_as_version_mismatch() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let script = write_script(tmp.path(), "answering.py", ANSWERING_SCRIPT);

    // The fake tool always reports `1.2.3` (see `ANSWERING_SCRIPT`); `"2."`
    // does not prefix-match it.
    fixture.set_local_reducer(
        "pinned",
        vec![script.to_str().unwrap().to_owned()],
        Some("2."),
        None,
    );

    let text = many_unique_lines(10);
    let event = post_tool_use(
        "Grep",
        text_response(&text),
        serde_json::json!({}),
        "s-local-version",
        "tu-1",
    );
    let response = fixture.hook(
        &event,
        &["--passthrough-tokens", "10", "--min-semantic-tokens", "10"],
    );
    let forwarded = updated_output(&response).expect("must still reduce and emit");

    assert!(forwarded.contains("distinct unique line number 0 "));
    assert!(
        forwarded.contains("semantic reduction bypassed (local-reducer-version)"),
        "a tool_version outside the pin must bypass as a version mismatch: {forwarded}"
    );
}

// ===========================================================================
// (f) The request never carries the task string or a credential variable.
// ===========================================================================

#[test]
fn the_local_reducer_request_never_carries_the_task_or_a_credential_variable() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let script = write_script(tmp.path(), "answering.py", ANSWERING_SCRIPT);
    let dump_path = tmp.path().join("dump.json");

    fixture.add_entitlement_with_credential("planted");
    fixture.set_local_reducer(
        "dumping",
        vec![
            script.to_str().unwrap().to_owned(),
            dump_path.to_str().unwrap().to_owned(),
        ],
        None,
        None,
    );

    let text = many_unique_lines(10);
    let event = post_tool_use(
        "Grep",
        text_response(&text),
        serde_json::json!({}),
        "s-local-privacy",
        "tu-1",
    );
    let response = fixture.hook(
        &event,
        &[
            "--passthrough-tokens",
            "10",
            "--min-semantic-tokens",
            "10",
            "--task",
            "SECRET-TASK-STRING-NEVER-SENT-TO-A-LOCAL-TOOL",
        ],
    );
    assert!(
        updated_output(&response).is_some(),
        "the local reducer must still have applied"
    );

    let dump_text = std::fs::read_to_string(&dump_path).expect("the fake tool must write its dump");
    let dump: serde_json::Value = serde_json::from_str(&dump_text).unwrap();
    let stdin_text = dump["stdin"].as_str().expect("dump must carry raw stdin");

    assert!(
        stdin_text.contains("\"tool\":\"Grep\"") || stdin_text.contains("\"tool\": \"Grep\""),
        "the request must name the tool: {stdin_text}"
    );
    assert!(
        stdin_text.contains("distinct unique line number 0"),
        "the request must carry the candidates: {stdin_text}"
    );
    assert!(
        !stdin_text.contains("SECRET-TASK-STRING-NEVER-SENT-TO-A-LOCAL-TOOL"),
        "the local-reducer contract never carries the task string: {stdin_text}"
    );
    assert!(
        !dump_text.contains(CREDENTIAL),
        "the local reducer's subprocess must never see a scrubbed entitlement credential: \
         {dump_text}"
    );
}
