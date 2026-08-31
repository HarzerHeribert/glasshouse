//! Phase 21K — the assumption guardrail, driven over the shipped binary.
//!
//! Every test here starts a real `glasshouse` process — `mcp serve` over
//! stdio for the protocol tests, which run on every platform, and `api
//! serve` over a Unix socket for the two that need a spawned session — and
//! asserts on what comes back. Nothing reaches into the server's process;
//! the only thing a test shares with it is the project's database, which is
//! how a test seeds a session for the server to scope by and how it reads
//! the row the server wrote.
//!
//! The behavioural contract, in the order the tests take it: a trivial edit
//! passes with no gate and a migration triggers a short preflight naming the
//! factor; an assumption is six fields and never its reasoning; transitions
//! append and the current state is the latest; the mode and the per-task
//! override decide the verdict and say who decided it; a refuted premise can
//! become a failed-approach memory and promotion is explicit; a refutation
//! reaches the watcher and the person; and another project's server sees
//! none of it.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use clap::Parser;
use glasshouse::Cli;
use glasshouse::memory::{MemoryAuthority, MemoryId, MemoryKind, ProjectMemory};
use glasshouse::session::{NewSession, ProjectSessions, SessionRole};
use rusqlite::Connection;
use serde_json::{Value, json};

const TIMEOUT: Duration = Duration::from_secs(30);

// -------------------------------------------------------------------------
// Fixture — one data/config root, any number of real projects under it.
// -------------------------------------------------------------------------

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
}

impl Fixture {
    /// A root with no harness installed: enough for every tool but spawn.
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().to_path_buf();
        let config_dir = base.join("config");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        std::fs::write(config_dir.join("config.toml"), "version = 1\n").expect("write user config");
        Self { _tmp: tmp, base }
    }

    fn project_root(&self, name: &str) -> PathBuf {
        let root = self.base.join("workspace").join(name);
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        std::fs::canonicalize(&root).expect("canonicalize project root")
    }

    /// Bootstrapped exactly as the binary bootstraps its own.
    fn runtime(&self, root: &Path) -> glasshouse::Runtime {
        let cli = Cli::try_parse_from([
            "glasshouse",
            "--scope",
            root.to_str().unwrap(),
            "--data-dir",
            self.base.join("data").to_str().unwrap(),
            "--config-dir",
            self.base.join("config").to_str().unwrap(),
        ])
        .expect("parse the fixture command line");
        glasshouse::bootstrap(&cli, root).expect("bootstrap the fixture runtime")
    }

    /// A session record the server did not create, so a session-keyed
    /// request has something to key on without a harness process.
    fn seed_session(&self, root: &Path) -> String {
        let runtime = self.runtime(root);
        let sessions = ProjectSessions::open(&runtime).expect("open the session store");
        let record = sessions
            .store()
            .create(NewSession::embedded("claude-code").with_role(SessionRole::Worker))
            .expect("seed a session record");
        record.id.as_str().to_owned()
    }

    /// A raw connection to one project's database, for reading what the
    /// server wrote.
    fn db(&self, root: &Path) -> Connection {
        Connection::open(self.runtime(root).database_path()).expect("open the database file")
    }

    /// Write the project's `.glasshouse/config.toml`.
    fn write_project_config(&self, root: &Path, contents: &str) {
        let dir = root.join(".glasshouse");
        std::fs::create_dir_all(&dir).expect("create .glasshouse");
        std::fs::write(dir.join("config.toml"), contents).expect("write project config");
    }

    /// The shipped binary, pointed at one project and this fixture's roots.
    fn command(&self, root: &Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_glasshouse"));
        command
            .arg("--scope")
            .arg(root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"));
        command
    }

    /// `glasshouse <args...>`, stdout as text; the command must succeed.
    fn run(&self, root: &Path, args: &[&str]) -> String {
        let output = self
            .command(root)
            .args(args)
            .output()
            .expect("run the binary");
        assert!(
            output.status.success(),
            "`glasshouse {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("utf-8 output")
    }
}

// -------------------------------------------------------------------------
// The MCP door: a running `glasshouse mcp serve`, killed on drop.
// -------------------------------------------------------------------------

struct McpServer {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    next_id: u64,
}

impl McpServer {
    fn start(fixture: &Fixture, root: &Path) -> Self {
        let mut child = fixture
            .command(root)
            .arg("mcp")
            .arg("serve")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn `glasshouse mcp serve`");
        let stdout = child.stdout.take().expect("captured stdout");
        let stdin = child.stdin.take().expect("captured stdin");
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let mut server = Self {
            child,
            stdin: Some(stdin),
            lines,
            next_id: 0,
        };
        server.initialize();
        server
    }

    fn send_raw(&mut self, frame: &str) {
        let stdin = self.stdin.as_mut().expect("stdin is still open");
        stdin
            .write_all(format!("{frame}\n").as_bytes())
            .expect("write a frame");
        stdin.flush().expect("flush stdin");
    }

    fn next_reply(&self) -> Value {
        let line = self
            .lines
            .recv_timeout(TIMEOUT)
            .expect("the server must answer within the timeout");
        serde_json::from_str(&line).unwrap_or_else(|err| panic!("not JSON: {err}: {line}"))
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        let frame = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.send_raw(&frame.to_string());
        let reply = self.next_reply();
        assert_eq!(
            reply["id"],
            json!(id),
            "a reply to a request this test never made: {reply}"
        );
        reply
    }

    fn initialize(&mut self) {
        let reply = self.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "assumption_guardrails test", "version": "0" },
            }),
        );
        assert!(reply["error"].is_null(), "initialize was refused: {reply}");
        let frame =
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized", "params": {} });
        self.send_raw(&frame.to_string());
    }

    /// One `tools/call`, whole reply.
    fn call(&mut self, name: &str, arguments: Value) -> Value {
        self.request(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        )
    }

    /// A tool call that must succeed, decoded from its text content.
    fn ok(&mut self, name: &str, arguments: Value) -> Value {
        let reply = self.call(name, arguments);
        let result = &reply["result"];
        assert!(
            result.is_object(),
            "`{name}` answered a protocol error: {reply}"
        );
        assert_eq!(result["isError"], false, "`{name}` refused: {reply}");
        let text = result["content"][0]["text"].as_str().expect("text content");
        serde_json::from_str(text)
            .unwrap_or_else(|err| panic!("`{name}` answered non-JSON: {err}: {text}"))
    }

    /// A tool call the handler must refuse, as its message.
    fn refused(&mut self, name: &str, arguments: Value) -> String {
        let reply = self.call(name, arguments);
        let result = &reply["result"];
        assert!(
            result.is_object(),
            "`{name}` answered a protocol error: {reply}"
        );
        assert_eq!(result["isError"], true, "`{name}` did not refuse: {reply}");
        result["content"][0]["text"]
            .as_str()
            .expect("text content")
            .to_owned()
    }

    /// A tool call the *protocol* must refuse — nothing ran — as its code.
    fn protocol_error(&mut self, name: &str, arguments: Value) -> (i64, String) {
        let reply = self.call(name, arguments);
        let error = &reply["error"];
        assert!(
            error.is_object(),
            "`{name}` was not refused at the protocol: {reply}"
        );
        (
            error["code"].as_i64().expect("an error code"),
            error["message"].as_str().unwrap_or_default().to_owned(),
        )
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        self.stdin.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn six_fields(session: Option<&str>, claim: &str) -> Value {
    let mut fields = json!({
        "claim": claim,
        "evidence": "grep found exactly one production caller",
        "evidence_source": "repository",
        "uncertainty": "medium",
        "affected": "api/unix.rs and anything that dispatches through it",
        "verification": "run the door's tests against the merged tree",
    });
    if let Some(session) = session {
        fields["session"] = json!(session);
    }
    fields
}

fn migration_change(description: &str) -> Value {
    json!({
        "description": description,
        "footprint": 3,
        "subsystems": ["database"],
        "reversible": true,
        "blast_radius": "module",
        "migration": true,
    })
}

// -------------------------------------------------------------------------
// The gate
// -------------------------------------------------------------------------

/// Lines 1004–1007, 1013, 1036, 1049: a one-file reversible edit is trivial
/// and asks nothing; a migration is substantial, names `migration`, asks at
/// most three questions, carries the map's guidance and the seven
/// responses, records the gate on the session, and takes a checkpoint.
#[test]
fn a_trivial_edit_passes_with_no_gate_and_a_migration_triggers_a_short_preflight_naming_the_factor()
{
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let mut server = McpServer::start(&fixture, &root);

    // Trivial: nothing asked, nothing written, and no gate.
    let trivial = server.ok(
        "glasshouse_preflight",
        json!({ "change": { "footprint": 1, "reversible": true, "blast_radius": "local" } }),
    );
    assert_eq!(trivial["risk"], "trivial", "{trivial}");
    assert_eq!(trivial["factor"], Value::Null, "{trivial}");
    assert_eq!(trivial["verdict"], "proceed", "{trivial}");
    assert_eq!(trivial["gate"]["triggered"], false, "{trivial}");
    assert_eq!(
        trivial["gate"]["decided_by"], "trivial changes never gate",
        "{trivial}"
    );
    assert_eq!(
        trivial["prompts"],
        json!([]),
        "a trivial edit is asked nothing: {trivial}"
    );
    assert_eq!(trivial["guidance"], json!([]), "{trivial}");
    assert_eq!(trivial["responses"], json!([]), "{trivial}");
    assert_eq!(trivial["session"], Value::Null, "{trivial}");

    // The same change, stated as a migration, for a session this project
    // holds.
    let session = fixture.seed_session(&root);
    let substantial = server.ok(
        "glasshouse_preflight",
        json!({ "session": session, "change": migration_change("add migration 19") }),
    );
    assert_eq!(substantial["risk"], "substantial", "{substantial}");
    assert_eq!(
        substantial["factor"], "migration",
        "line 1049: {substantial}"
    );
    assert_eq!(substantial["category"], "migration", "{substantial}");
    assert_eq!(
        substantial["verdict"], "advisory",
        "the default mode: {substantial}"
    );
    assert_eq!(
        substantial["gate"]["triggered"], false,
        "advisory never blocks: {substantial}"
    );
    assert_eq!(substantial["gate"]["mode"], "advisory", "{substantial}");
    assert_eq!(
        substantial["gate"]["mode_source"], "by default",
        "{substantial}"
    );
    assert_eq!(
        substantial["gate"]["decided_by"], "guardrails.mode = advisory",
        "{substantial}"
    );
    assert_eq!(
        substantial["description"], "add migration 19",
        "{substantial}"
    );

    let prompts = substantial["prompts"].as_array().expect("prompts");
    assert!(
        !prompts.is_empty() && prompts.len() <= 3,
        "line 1013: at most three prompts: {substantial}"
    );
    assert_eq!(
        prompts[0]["key"], "migration-undo",
        "the factor that fired is asked first"
    );
    assert!(
        prompts[0]["ask"].as_str().unwrap().contains("undo"),
        "{substantial}"
    );

    // The guidance reaches the agent through the door, in the map's words.
    let guidance = substantial["guidance"].as_array().expect("guidance");
    let text_for = |line: u64| {
        guidance
            .iter()
            .find(|g| g["line"] == json!(line))
            .and_then(|g| g["text"].as_str())
            .unwrap_or_else(|| panic!("guidance for line {line} missing: {substantial}"))
    };
    assert!(text_for(997).contains("presentation, not evidence"));
    assert!(text_for(1009).contains("long plan"));
    assert!(text_for(1024).contains("direct evidence"));
    assert!(text_for(1027).contains("read-only inspection"));
    assert!(text_for(1028).contains("baseline"));
    assert!(text_for(1029).contains("time-box"));
    assert!(text_for(1030).contains("independent"));
    assert!(text_for(1031).contains("fresh session"));
    assert!(text_for(1032).contains("weak confirmation"));
    assert!(text_for(1038).contains("smallest"));
    assert!(text_for(1040).contains("adapters"));
    assert!(text_for(1041).contains("stop compounding"));
    assert!(text_for(1042).contains("failed-approach"));
    assert!(text_for(1043).contains("rewrite the task history"));

    // Line 1051: the seven responses, in the map's order.
    let responses: Vec<&str> = substantial["responses"]
        .as_array()
        .expect("responses")
        .iter()
        .map(|r| r["response"].as_str().unwrap())
        .collect();
    assert_eq!(
        responses,
        [
            "inspect",
            "continue",
            "verify",
            "checkpoint",
            "handoff",
            "re-plan",
            "stop"
        ]
    );

    // Line 1036: a checkpoint was taken through the existing path, and the
    // answer says so.
    assert_eq!(
        substantial["checkpoint"]["session"], session,
        "{substantial}"
    );
    let checkpoint_id = substantial["checkpoint"]["checkpoint"]
        .as_str()
        .expect("a checkpoint id");
    let fetched = server.ok(
        "glasshouse_get_checkpoint",
        json!({ "checkpoint": checkpoint_id }),
    );
    assert_eq!(fetched["session"], session, "{fetched}");

    // Line 1049, durably: the gate is a row on the session's ledger.
    let listed = server.ok("glasshouse_list_assumptions", json!({ "session": session }));
    let gates: Vec<&Value> = listed["events"]
        .as_array()
        .expect("events")
        .iter()
        .filter(|e| e["kind"] == "gate")
        .collect();
    assert_eq!(gates.len(), 1, "{listed}");
    assert_eq!(
        gates[0]["subject"], "substantial/migration/advisory",
        "{listed}"
    );
    assert_eq!(gates[0]["origin"], "glasshouse", "{listed}");
    assert_eq!(gates[0]["note"], "add migration 19", "{listed}");
    assert_eq!(gates[0]["seq"], substantial["gate"]["seq"], "{listed}");

    // A trivial preflight for the session records its gate too, and does
    // not checkpoint.
    let trivial_again = server.ok(
        "glasshouse_preflight",
        json!({ "session": session, "change": { "footprint": 1 } }),
    );
    assert_eq!(trivial_again["risk"], "trivial");
    assert!(trivial_again["checkpoint"].is_null(), "{trivial_again}");

    // Line 1013's number, through the door: a change that fires every rung
    // at once is still asked exactly three questions, the most severe first.
    let everything = server.ok(
        "glasshouse_preflight",
        json!({ "change": {
            "footprint": 20, "reversible": false, "blast_radius": "system",
            "migration": true, "destructive": true, "security": true, "data_integrity": true,
            "unfamiliar_integration": true, "architecture": true, "broad_refactor": true,
            "premise_evidence": "inference",
        } }),
    );
    let keys: Vec<&str> = everything["prompts"]
        .as_array()
        .expect("prompts")
        .iter()
        .map(|p| p["key"].as_str().unwrap())
        .collect();
    assert_eq!(
        keys,
        ["migration-undo", "destructive-loss", "security-boundary"],
        "exactly three, most severe first: {everything}"
    );

    // Line 998, at the gate: a field that could carry reasoning is refused
    // before anything runs.
    let (code, message) = server.protocol_error(
        "glasshouse_preflight",
        json!({ "change": { "footprint": 1, "reasoning": "I think so because..." } }),
    );
    assert_eq!(code, -32602, "{message}");
    assert!(message.contains("reasoning"), "{message}");

    // Line 1037/1039/1050: a budget stated and then exceeded is recorded,
    // and the session's open premises come back to be re-evaluated.
    server.ok(
        "glasshouse_record_assumption",
        six_fields(Some(&session), "the index is unused"),
    );
    let over = server.ok(
        "glasshouse_preflight",
        json!({
            "session": session,
            "change": {
                "footprint": 3,
                "budget": { "footprint": 4, "tool_rounds": 20 },
                "spent": { "footprint": 9, "tool_rounds": 12 },
            }
        }),
    );
    assert_eq!(over["budget"]["exceeded"], true, "{over}");
    assert!(over["budget"]["seq"].as_i64().is_some(), "{over}");
    let axes = over["budget"]["axes"].as_array().unwrap();
    assert_eq!(axes.len(), 2);
    assert_eq!(axes[0]["axis"], "footprint");
    assert_eq!(axes[0]["exceeded"], true);
    assert_eq!(axes[1]["exceeded"], false);
    let re_evaluate = over["re_evaluate"].as_array().expect("open premises");
    assert_eq!(re_evaluate.len(), 1);
    assert_eq!(re_evaluate[0]["claim"], "the index is unused");
}

// -------------------------------------------------------------------------
// The record
// -------------------------------------------------------------------------

/// Lines 998, 1014, 1015, 1016: six fields, all required; nothing with room
/// for reasoning in the request or the table; untrusted text bounded and
/// stripped.
#[test]
fn an_assumption_is_recorded_with_its_six_fields_and_never_its_reasoning() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let mut server = McpServer::start(&fixture, &root);

    let recorded = server.ok(
        "glasshouse_record_assumption",
        six_fields(None, "the door has one dispatch"),
    );
    assert_eq!(recorded["state"], "proposed", "{recorded}");
    assert_eq!(recorded["claim"], "the door has one dispatch");
    assert_eq!(
        recorded["evidence"],
        "grep found exactly one production caller"
    );
    assert_eq!(recorded["evidence_source"], "repository");
    assert_eq!(recorded["uncertainty"], "medium");
    assert_eq!(
        recorded["affected"],
        "api/unix.rs and anything that dispatches through it"
    );
    assert_eq!(
        recorded["verification"],
        "run the door's tests against the merged tree"
    );
    assert_eq!(recorded["origin"], "agent");
    assert_eq!(recorded["transitions"], 1);
    assert_eq!(recorded["session"], Value::Null);
    let id = recorded["id"].as_str().expect("an id");
    assert_eq!(id.len(), 32);
    assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    let keys: Vec<&str> = recorded
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert!(!keys.iter().any(|k| k.contains("reason")), "{keys:?}");

    // Every one of the six is required.
    for missing in [
        "claim",
        "evidence",
        "evidence_source",
        "uncertainty",
        "affected",
        "verification",
    ] {
        let mut without = six_fields(None, "c");
        without.as_object_mut().unwrap().remove(missing);
        let (code, message) = server.protocol_error("glasshouse_record_assumption", without);
        assert_eq!(code, -32602, "`{missing}` must be required: {message}");
    }

    // And a seventh, however it is named, is refused rather than dropped.
    for extra in ["reasoning", "transcript", "output", "chain_of_thought"] {
        let mut with = six_fields(None, "c");
        with[extra] = json!("...");
        let (code, message) = server.protocol_error("glasshouse_record_assumption", with);
        assert_eq!(code, -32602, "{message}");
        assert!(message.contains(extra), "{message}");
    }

    // A source class or uncertainty outside the vocabulary is refused by
    // name.
    let mut wrong = six_fields(None, "c");
    wrong["evidence_source"] = json!("vibes");
    let (_, message) = server.protocol_error("glasshouse_record_assumption", wrong);
    assert!(
        message.contains("`inference`"),
        "the refusal names the vocabulary: {message}"
    );

    // Untrusted text: control characters cannot be stored, and a claim over
    // the ceiling is refused, not cut.
    let hostile = server.ok(
        "glasshouse_record_assumption",
        six_fields(None, "one\r\nline\u{1b}[2J\tonly"),
    );
    assert_eq!(
        hostile["claim"], "one line (2J only",
        "quoted on the way out: brackets rewritten"
    );
    let stored: String = fixture
        .db(&root)
        .query_row(
            "SELECT claim FROM task_assumptions WHERE id = ?1",
            [hostile["id"].as_str().unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        stored, "one line [2J only",
        "stored sanitized, brackets kept"
    );
    assert!(!stored.chars().any(char::is_control));

    let long = "x".repeat(281);
    let refusal = server.refused("glasshouse_record_assumption", six_fields(None, &long));
    assert!(refusal.contains("280"), "{refusal}");
    let rows: i64 = fixture
        .db(&root)
        .query_row("SELECT COUNT(*) FROM task_assumptions", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(rows, 2, "a refusal leaves no row");

    // The table is the six fields plus bookkeeping — nothing else.
    let columns: Vec<String> = {
        let conn = fixture.db(&root);
        let mut statement = conn.prepare("PRAGMA table_info(task_assumptions)").unwrap();
        statement
            .query_map([], |row| row.get::<_, String>("name"))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(
        columns,
        [
            "id",
            "project_id",
            "session_id",
            "created_at",
            "origin",
            "claim",
            "evidence",
            "evidence_source",
            "uncertainty",
            "affected",
            "verification"
        ]
    );

    // And the person's view says the same: the CLI prints what was stated.
    let report = fixture.run(&root, &["assumptions"]);
    assert!(report.contains("the door has one dispatch"), "{report}");
    assert!(report.contains("proposed 2"), "{report}");
    assert!(report.contains("medium/repository"), "{report}");
}

// -------------------------------------------------------------------------
// The history
// -------------------------------------------------------------------------

/// Line 1018: the six states; every move appends; the current state is the
/// latest row; nothing is ever updated — by the store, and by the schema.
#[test]
fn transitions_append_and_the_current_state_is_the_latest() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let mut server = McpServer::start(&fixture, &root);

    let recorded = server.ok("glasshouse_record_assumption", six_fields(None, "claim"));
    let id = recorded["id"].as_str().unwrap().to_owned();
    let short = &id[..12];

    let probing = server.ok(
        "glasshouse_update_assumption",
        json!({ "assumption": short, "state": "probing", "response": "verify" }),
    );
    assert_eq!(probing["assumption"]["state"], "probing", "{probing}");
    assert_eq!(probing["transition"]["response"], "verify", "{probing}");
    assert_eq!(probing["transition"]["kind"], "transition");

    // A note without a move re-states the current state.
    let noted = server.ok(
        "glasshouse_update_assumption",
        json!({ "assumption": id, "note": "test written, running it" }),
    );
    assert_eq!(noted["assumption"]["state"], "probing", "{noted}");
    assert_eq!(noted["transition"]["note"], "test written, running it");

    let supported = server.ok(
        "glasshouse_update_assumption",
        json!({ "assumption": id, "state": "supported" }),
    );
    assert_eq!(supported["assumption"]["state"], "supported");
    assert_eq!(supported["assumption"]["transitions"], 4);

    let listed = server.ok("glasshouse_list_assumptions", json!({}));
    assert_eq!(listed["counts"]["supported"], 1, "{listed}");
    assert_eq!(listed["counts"]["proposed"], 0, "{listed}");
    assert_eq!(listed["assumptions"][0]["state"], "supported");
    assert_eq!(listed["assumptions"][0]["latest"]["state"], "supported");

    // The rows, in order, and the schema's own refusal to edit one.
    let conn = fixture.db(&root);
    let states: Vec<String> = {
        let mut statement = conn
            .prepare(
                "SELECT state FROM assumption_transitions WHERE assumption_id = ?1 ORDER BY seq",
            )
            .unwrap();
        statement
            .query_map([&id], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(states, ["proposed", "probing", "probing", "supported"]);
    let err = conn
        .execute(
            "UPDATE assumption_transitions SET state = 'refuted' WHERE assumption_id = ?1",
            [&id],
        )
        .unwrap_err();
    assert!(err.to_string().contains("append-only"), "{err}");
    let err = conn
        .execute(
            "UPDATE task_assumptions SET claim = 'edited' WHERE id = ?1",
            [&id],
        )
        .unwrap_err();
    assert!(err.to_string().contains("never edited"), "{err}");
    drop(conn);

    // Every one of the six states is reachable, and `waived_by_user` only
    // by a person: an MCP caller is a program and is refused.
    for state in ["refuted", "unresolved", "proposed"] {
        let moved = server.ok(
            "glasshouse_update_assumption",
            json!({ "assumption": id, "state": state }),
        );
        assert_eq!(moved["assumption"]["state"], state);
    }
    let refusal = server.refused(
        "glasshouse_update_assumption",
        json!({ "assumption": id, "state": "waived_by_user" }),
    );
    assert!(refusal.contains("origin: user"), "{refusal}");
    let (code, _) = server.protocol_error(
        "glasshouse_update_assumption",
        json!({ "assumption": id, "state": "maybe" }),
    );
    assert_eq!(code, -32602, "a seventh state does not exist");
    let unknown = server.refused(
        "glasshouse_update_assumption",
        json!({ "assumption": "00000000", "state": "probing" }),
    );
    assert!(unknown.contains("no assumption"), "{unknown}");

    // The store's production code contains no UPDATE at all.
    let source = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/guardrails/store.rs"),
    )
    .expect("read the store's source");
    let production = source.split("#[cfg(test)]").next().unwrap();
    let code: String = production
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !code.to_ascii_uppercase().contains("UPDATE"),
        "guardrails/store.rs updates a row in production code"
    );
}

// -------------------------------------------------------------------------
// Memory
// -------------------------------------------------------------------------

/// Lines 1017, 1019, 1020: a refutation writes a failed-approach memory only
/// when asked, with provenance naming the assumption; promotion needs a
/// supported assumption and one of three kinds, and is never automatic.
#[test]
fn a_refuted_premise_can_become_a_failed_approach_memory_and_promotion_is_explicit() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let session = fixture.seed_session(&root);
    let mut server = McpServer::start(&fixture, &root);
    let runtime = fixture.runtime(&root);
    let memories = || -> i64 {
        fixture
            .db(&root)
            .query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))
            .unwrap()
    };
    assert_eq!(memories(), 0);

    // Refuted, with the flag: one memory, kind failed_attempt, naming the
    // assumption and the session.
    let a = server.ok(
        "glasshouse_record_assumption",
        six_fields(Some(&session), "the cache is keyed by path"),
    );
    let a_id = a["id"].as_str().unwrap().to_owned();
    let refuted = server.ok(
        "glasshouse_update_assumption",
        json!({
            "assumption": a_id,
            "state": "refuted",
            "record_failed_approach": true,
            "note": "the key is the inode, so a rename hits",
            "response": "re-plan",
        }),
    );
    let memory_id = refuted["memory"].as_str().expect("a memory id").to_owned();
    assert_eq!(refuted["transition"]["subject"], memory_id, "{refuted}");
    assert_eq!(refuted["transition"]["response"], "re-plan");
    assert_eq!(memories(), 1);
    {
        let memory = ProjectMemory::open(&runtime).unwrap();
        let record = memory
            .store()
            .get(&MemoryId::new(memory_id.clone()))
            .unwrap()
            .expect("the memory exists");
        assert_eq!(record.kind, MemoryKind::FailedAttempt);
        assert_eq!(record.authority, None, "never classified by the door");
        assert!(
            record.body.contains("the cache is keyed by path"),
            "{}",
            record.body
        );
        assert!(record.body.contains("rename hits"), "{}", record.body);
        assert_eq!(record.source_session_id.as_deref(), Some(session.as_str()));
        let rationale = record.provenance.rationale.as_deref().unwrap_or_default();
        assert!(
            rationale.contains(&a_id),
            "provenance names the assumption: {rationale}"
        );
        assert!(rationale.contains("1019"), "{rationale}");
        assert_eq!(
            record.provenance.assumptions.as_deref(),
            Some("the cache is keyed by path")
        );
    }

    // Refuted without the flag: no memory.
    let b = server.ok("glasshouse_record_assumption", six_fields(None, "b"));
    let refuted_quietly = server.ok(
        "glasshouse_update_assumption",
        json!({ "assumption": b["id"], "state": "refuted" }),
    );
    assert_eq!(refuted_quietly["memory"], Value::Null);
    assert_eq!(memories(), 1);

    // The flag on anything but a refutation is refused, and writes nothing.
    let c = server.ok(
        "glasshouse_record_assumption",
        six_fields(Some(&session), "the index is covering"),
    );
    let c_id = c["id"].as_str().unwrap().to_owned();
    let refusal = server.refused(
        "glasshouse_update_assumption",
        json!({ "assumption": c_id, "state": "supported", "record_failed_approach": true }),
    );
    assert!(refusal.contains("refuted"), "{refusal}");
    assert_eq!(memories(), 1);

    // Promotion: refused while proposed, refused for a kind outside the
    // three, and explicit when supported.
    let not_yet = server.refused(
        "glasshouse_promote_assumption",
        json!({ "assumption": c_id, "kind": "decision" }),
    );
    assert!(not_yet.contains("`proposed`, not `supported`"), "{not_yet}");
    assert_eq!(memories(), 1);
    let (code, message) = server.protocol_error(
        "glasshouse_promote_assumption",
        json!({ "assumption": c_id, "kind": "todo" }),
    );
    assert_eq!(code, -32602, "{message}");
    assert!(message.contains("`finding`"), "{message}");

    server.ok(
        "glasshouse_update_assumption",
        json!({ "assumption": c_id, "state": "supported", "note": "EXPLAIN shows the index" }),
    );
    assert_eq!(
        memories(),
        1,
        "supporting an assumption promotes nothing by itself"
    );
    let promoted = server.ok(
        "glasshouse_promote_assumption",
        json!({ "assumption": c_id, "kind": "decision", "note": "we rely on it" }),
    );
    let decision_id = promoted["memory"].as_str().unwrap().to_owned();
    assert_eq!(promoted["authority"], "decision", "{promoted}");
    assert_eq!(promoted["transition"]["subject"], decision_id);
    assert_eq!(
        promoted["transition"]["state"], "supported",
        "promotion moves nothing"
    );
    assert_eq!(memories(), 2);
    {
        let memory = ProjectMemory::open(&runtime).unwrap();
        let record = memory
            .store()
            .get(&MemoryId::new(decision_id))
            .unwrap()
            .expect("the decision exists");
        assert_eq!(record.kind, MemoryKind::Decision);
        assert_eq!(record.authority, Some(MemoryAuthority::Decision));
        assert_eq!(record.body, "the index is covering");
        let rationale = record.provenance.rationale.as_deref().unwrap_or_default();
        assert!(
            rationale.contains(&c_id) && rationale.contains("we rely on it"),
            "{rationale}"
        );
    }
    let view = server.ok("glasshouse_list_assumptions", json!({ "session": session }));
    let c_view = view["assumptions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["id"] == c_id)
        .expect("c is listed");
    assert_eq!(c_view["transitions"], 3);
    assert!(
        c_view["latest"]["note"]
            .as_str()
            .unwrap()
            .starts_with("promoted as decision")
    );

    // A finding is knowledge, not a rule: unclassified.
    let d = server.ok("glasshouse_record_assumption", six_fields(None, "d"));
    server.ok(
        "glasshouse_update_assumption",
        json!({ "assumption": d["id"], "state": "supported" }),
    );
    let finding = server.ok(
        "glasshouse_promote_assumption",
        json!({ "assumption": d["id"], "kind": "finding" }),
    );
    assert_eq!(finding["authority"], Value::Null, "{finding}");
}

// -------------------------------------------------------------------------
// Scope
// -------------------------------------------------------------------------

/// Two projects under one root: what one's server records, the other's
/// cannot list, read, update or key a preflight on.
#[test]
fn another_projects_server_sees_none_of_it() {
    let fixture = Fixture::new();
    let alpha = fixture.project_root("alpha");
    let beta = fixture.project_root("beta");
    let alpha_session = fixture.seed_session(&alpha);
    let beta_session = fixture.seed_session(&beta);

    let mut alpha_server = McpServer::start(&fixture, &alpha);
    let recorded = alpha_server.ok(
        "glasshouse_record_assumption",
        six_fields(Some(&alpha_session), "alpha's premise"),
    );
    let id = recorded["id"].as_str().unwrap().to_owned();
    alpha_server.ok(
        "glasshouse_preflight",
        json!({ "session": alpha_session, "change": migration_change("alpha's migration") }),
    );
    drop(alpha_server);

    let mut beta_server = McpServer::start(&fixture, &beta);
    let listed = beta_server.ok("glasshouse_list_assumptions", json!({}));
    assert_eq!(listed["assumptions"], json!([]), "{listed}");
    for (state, count) in listed["counts"].as_object().unwrap() {
        assert_eq!(count, &json!(0), "{state}");
    }
    let unknown = beta_server.refused(
        "glasshouse_update_assumption",
        json!({ "assumption": id, "state": "probing" }),
    );
    assert!(unknown.contains("no assumption"), "{unknown}");
    let foreign = beta_server.refused(
        "glasshouse_preflight",
        json!({ "session": alpha_session, "change": { "footprint": 1 } }),
    );
    assert!(foreign.contains("no session"), "{foreign}");
    let foreign = beta_server.refused(
        "glasshouse_record_assumption",
        six_fields(Some(&alpha_session), "smuggled"),
    );
    assert!(foreign.contains("no session"), "{foreign}");
    let foreign = beta_server.refused(
        "glasshouse_list_assumptions",
        json!({ "session": alpha_session }),
    );
    assert!(foreign.contains("no session"), "{foreign}");

    // Beta's own session works, and its gate lands on beta's ledger only.
    let own = beta_server.ok(
        "glasshouse_preflight",
        json!({ "session": beta_session, "change": migration_change("beta's migration") }),
    );
    assert_eq!(own["risk"], "substantial");
    let beta_rows: i64 = fixture
        .db(&beta)
        .query_row("SELECT COUNT(*) FROM assumption_transitions", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(
        beta_rows, 1,
        "beta holds its own gate and nothing of alpha's"
    );
    let alpha_rows: i64 = fixture
        .db(&alpha)
        .query_row("SELECT COUNT(*) FROM assumption_transitions", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(alpha_rows, 2, "alpha's first state and its gate");

    // The trigger is the structural half: a row for alpha cannot be written
    // into beta's file even by hand.
    let err = fixture
        .db(&beta)
        .execute(
            "INSERT INTO assumption_transitions (project_id, session_id, at, kind, origin) \
             SELECT value, 's', 1, 'gate', 'glasshouse' FROM project_metadata WHERE key = 'project_id' \
             AND 0 = 1 UNION ALL SELECT 'not-beta', 's', 1, 'gate', 'glasshouse'",
            [],
        )
        .unwrap_err();
    assert!(err.to_string().contains("different project"), "{err}");
}

// -------------------------------------------------------------------------
// Unix only: the tests that need a spawned session
// -------------------------------------------------------------------------

#[cfg(unix)]
mod with_a_harness {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixStream;
    use std::time::Instant;

    /// A root with an installed fake harness that records every line it
    /// reads under a name derived from its own `--settings` argument — the
    /// fixture `worker_wakeup.rs` and `api_event_log.rs` use.
    struct HarnessFixture {
        inner: Fixture,
    }

    impl HarnessFixture {
        fn new() -> Self {
            let inner = Fixture::new();
            let bin_dir = inner.base.join("bin");
            std::fs::create_dir_all(&bin_dir).expect("create bin dir");
            let harness = install_session_tagging_harness(&bin_dir);
            let escaped = harness.display().to_string().replace('\\', "\\\\");
            std::fs::write(
                inner.base.join("config").join("config.toml"),
                format!(
                    "version = 1\n\n[integrations.claude-code]\nenabled = true\nexecutable = \"{escaped}\"\n"
                ),
            )
            .expect("write user config");
            Self { inner }
        }

        fn received(&self, root: &Path, session: &str) -> Option<String> {
            std::fs::read_to_string(root.join(format!("received-{session}.log"))).ok()
        }

        fn argv(&self, root: &Path, session: &str) -> Option<String> {
            std::fs::read_to_string(root.join(format!("argv-{session}.log"))).ok()
        }

        /// A real `glasshouse hook` process, as a harness's lifecycle hook
        /// runs it.
        fn hook(&self, root: &Path, session: &str, event: &str) {
            let status = self
                .inner
                .command(root)
                .arg("hook")
                .arg("--session")
                .arg(session)
                .arg("--event")
                .arg(event)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .expect("run `glasshouse hook`");
            assert!(status.success(), "`glasshouse hook` must never fail");
        }
    }

    fn install_session_tagging_harness(bin_dir: &Path) -> PathBuf {
        let path = bin_dir.join("session-tagging-harness");
        std::fs::write(
            &path,
            "#!/bin/sh\n\
             tag=unknown\n\
             prev=\"\"\n\
             for a in \"$@\"; do\n\
             if [ \"$prev\" = \"--settings\" ]; then tag=$(basename \"$(dirname \"$a\")\"); fi\n\
             prev=\"$a\"\n\
             done\n\
             echo \"$@\" > \"$PWD/argv-$tag.log\"\n\
             echo READY\n\
             while IFS= read -r line; do\n\
             printf '%s\\n' \"$line\" >> \"$PWD/received-$tag.log\"\n\
             echo \"got:$line\"\n\
             done\n",
        )
        .expect("write the session-tagging harness");
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path
    }

    /// A running `glasshouse api serve`, killed on drop.
    struct Server {
        child: Child,
        socket: PathBuf,
    }

    impl Server {
        fn start(fixture: &HarnessFixture, root: &Path) -> Self {
            let mut child = fixture
                .inner
                .command(root)
                .arg("api")
                .arg("serve")
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn `glasshouse api serve`");
            let stderr = child.stderr.take().expect("captured stderr");
            let mut reader = BufReader::new(stderr);
            let deadline = Instant::now() + TIMEOUT;
            let socket = loop {
                let mut line = String::new();
                let read = reader.read_line(&mut line).expect("read server stderr");
                assert!(read > 0, "the server exited before announcing its socket");
                if let Some(path) = line
                    .trim_end()
                    .strip_prefix("glasshouse: control API listening on ")
                {
                    break PathBuf::from(path);
                }
                assert!(
                    Instant::now() < deadline,
                    "timed out waiting for the socket"
                );
            };
            Self { child, socket }
        }

        fn call(&self, request: Value) -> Value {
            let deadline = Instant::now() + TIMEOUT;
            let mut stream = loop {
                match UnixStream::connect(&self.socket) {
                    Ok(stream) => break stream,
                    Err(err) => {
                        assert!(Instant::now() < deadline, "timed out connecting: {err}");
                        std::thread::sleep(Duration::from_millis(20));
                    }
                }
            };
            let mut payload = serde_json::to_string(&request).expect("encode request");
            payload.push('\n');
            stream.write_all(payload.as_bytes()).expect("write request");
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).expect("read response");
            serde_json::from_str(line.trim_end()).expect("parse response")
        }

        fn ok(&self, request: Value) -> Value {
            let response = self.call(request);
            assert_eq!(response["status"], "ok", "{response}");
            response["result"].clone()
        }

        fn refused(&self, request: Value) -> String {
            let response = self.call(request);
            assert_eq!(response["status"], "error", "{response}");
            response["message"].as_str().unwrap().to_owned()
        }

        fn spawn(&self, role: &str, guardrail: Option<&str>) -> String {
            let mut request =
                json!({ "op": "spawn_session", "harness": "claude-code", "role": role });
            if let Some(guardrail) = guardrail {
                request["guardrail"] = json!(guardrail);
            }
            let result = self.ok(request);
            result["session"].as_str().expect("a session id").to_owned()
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    fn wait_for<F: FnMut() -> bool>(what: &str, mut done: F) {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if done() {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn completions(text: &str) -> Vec<Value> {
        text.lines()
            .filter_map(|line| line.trim().strip_prefix("glasshouse worker-completion "))
            .map(|json| serde_json::from_str(json).expect("one line of JSON"))
            .collect()
    }

    /// Lines 1008, 1052, 1053: `off` proceeds; `risk_gated` gates only a
    /// blocking category; a per-task override outranks the mode; and every
    /// verdict names what decided it and, for a gate, what lifts it.
    #[test]
    fn the_mode_and_the_per_task_override_decide_the_verdict_and_are_attributable() {
        let fixture = HarnessFixture::new();
        let root = fixture.inner.project_root("alpha");
        let server = Server::start(&fixture, &root);
        let session = fixture.inner.seed_session(&root);
        let preflight = |session: &str, change: Value| {
            server.ok(json!({ "op": "preflight", "session": session, "change": change }))
        };

        // `off`: every preflight proceeds, and says the mode decided it.
        fixture
            .inner
            .write_project_config(&root, "[guardrails]\nmode = \"off\"\n");
        let off = preflight(&session, migration_change("m"));
        assert_eq!(off["verdict"], "proceed", "{off}");
        assert_eq!(
            off["risk"], "substantial",
            "the class is stated even when off"
        );
        assert_eq!(off["gate"]["decided_by"], "guardrails.mode = off");
        assert_eq!(
            off["gate"]["mode_source"],
            "in this project's configuration"
        );
        assert!(
            off["checkpoint"].is_null(),
            "off disables the whole mechanism: {off}"
        );

        // `risk_gated` with the default blocking list: a migration gates, an
        // architectural change is advisory, and the gate says what lifts it.
        fixture
            .inner
            .write_project_config(&root, "[guardrails]\nmode = \"risk_gated\"\n");
        let gated = preflight(&session, migration_change("m"));
        assert_eq!(gated["verdict"], "gated", "{gated}");
        assert_eq!(gated["gate"]["triggered"], true);
        assert_eq!(gated["gate"]["decided_by"], "guardrails.mode = risk_gated");
        assert_eq!(
            gated["gate"]["blocking"],
            json!(["security", "destructive", "migration"])
        );
        let lifts = gated["gate"]["lifts"]
            .as_str()
            .expect("a gate says what lifts it");
        assert!(
            lifts.contains("--guardrail skip") && lifts.contains("advisory"),
            "{lifts}"
        );
        assert_eq!(
            gated["checkpoint"]["session"], session,
            "line 1036 still holds"
        );
        let architecture = preflight(&session, json!({ "footprint": 5, "architecture": true }));
        assert_eq!(architecture["verdict"], "advisory", "{architecture}");
        assert_eq!(architecture["factor"], "architecture");
        assert_eq!(architecture["category"], Value::Null);

        // A narrowed blocking list: migration no longer blocks, security does.
        fixture.inner.write_project_config(
            &root,
            "[guardrails]\nmode = \"risk_gated\"\nblocking = [\"security\"]\n",
        );
        let narrowed = preflight(&session, migration_change("m"));
        assert_eq!(narrowed["verdict"], "advisory", "{narrowed}");
        let security = preflight(&session, json!({ "footprint": 2, "security": true }));
        assert_eq!(security["verdict"], "gated", "{security}");
        assert_eq!(security["factor"], "security");

        // A category outside the four is refused at load, by name.
        fixture.inner.write_project_config(
            &root,
            "[guardrails]\nmode = \"risk_gated\"\nblocking = [\"architecture\"]\n",
        );
        let refusal = server.refused(json!({ "op": "preflight", "change": { "footprint": 1 } }));
        assert!(refusal.contains("`data_integrity`"), "{refusal}");
        fixture
            .inner
            .write_project_config(&root, "[guardrails]\nmode = \"risk_gated\"\n");

        // `--guardrail skip` on a spawn: a waived_by_user row with its
        // origin, and every preflight for that session proceeds under it.
        let skipped = server.spawn("worker", Some("skip"));
        let listed = server.ok(json!({ "op": "list_assumptions", "session": skipped }));
        let overrides: Vec<&Value> = listed["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["kind"] == "override")
            .collect();
        assert_eq!(overrides.len(), 1, "{listed}");
        assert_eq!(overrides[0]["subject"], "skip");
        assert_eq!(overrides[0]["state"], "waived_by_user", "{listed}");
        assert_eq!(
            overrides[0]["origin"], "agent",
            "a spawn is a program's request"
        );
        let waived = preflight(&skipped, migration_change("m"));
        assert_eq!(waived["verdict"], "proceed", "{waived}");
        assert_eq!(
            waived["gate"]["decided_by"],
            "per-task override `--guardrail skip`"
        );
        assert_eq!(waived["gate"]["override"]["kind"], "skip");
        assert_eq!(waived["gate"]["override"]["seq"], overrides[0]["seq"]);
        assert_eq!(waived["prompts"], json!([]), "a waived gate asks nothing");

        // `--guardrail force` outranks even `off`.
        fixture
            .inner
            .write_project_config(&root, "[guardrails]\nmode = \"off\"\n");
        let forced = server.spawn("worker", Some("force"));
        let forced_verdict = preflight(&forced, migration_change("m"));
        assert_eq!(forced_verdict["verdict"], "gated", "{forced_verdict}");
        assert_eq!(
            forced_verdict["gate"]["decided_by"],
            "per-task override `--guardrail force`"
        );
        let forced_trivial = preflight(&forced, json!({ "footprint": 1 }));
        assert_eq!(
            forced_trivial["verdict"], "proceed",
            "trivial still never gates"
        );

        // A misspelt override is refused before anything is spawned.
        let bad = server.refused(json!({
            "op": "spawn_session", "harness": "claude-code", "guardrail": "please",
        }));
        assert!(bad.contains("`lower`"), "{bad}");

        // The person sees the override and the gate.
        let report = fixture
            .inner
            .run(&root, &["assumptions", "--session", &skipped[..8]]);
        assert!(report.contains("override"), "{report}");
        assert!(report.contains("skip"), "{report}");
        assert!(report.contains("substantial/migration/proceed"), "{report}");
        let shown = fixture
            .inner
            .run(&root, &["sessions", "show", &skipped[..8]]);
        assert!(shown.contains("guardrail"), "{shown}");
        assert!(shown.contains("skip"), "{shown}");
        assert!(shown.contains("last gate"), "{shown}");
    }

    /// Line 1050: a refuted premise reaches the orchestrator watching the
    /// worker — on the completion line and through `events` — and the
    /// person, through `glasshouse assumptions` and `sessions show`.
    #[test]
    fn a_refutation_reaches_the_watcher_and_the_person() {
        let fixture = HarnessFixture::new();
        let root = fixture.inner.project_root("alpha");
        let server = Server::start(&fixture, &root);

        let worker = server.spawn("worker", None);
        let orchestrator = server.spawn("orchestrator", None);
        wait_for("both harnesses to start", || {
            fixture.argv(&root, &worker).is_some() && fixture.argv(&root, &orchestrator).is_some()
        });

        let registered = server.ok(json!({
            "op": "watch_worker", "session": worker, "notify": orchestrator,
        }));
        let assumptions_from = registered["assumptions_from"].as_i64().expect("a cursor");

        // The worker states a premise and then refutes it.
        let recorded = server.ok(json!({
            "op": "record_assumption",
            "session": worker,
            "claim": "the parser is line-buffered",
            "evidence": "the docs say so",
            "evidence_source": "external",
            "uncertainty": "high",
            "affected": "the relay",
            "verification": "feed it a partial line",
        }));
        let id = recorded["id"].as_str().unwrap().to_owned();
        server.ok(json!({
            "op": "update_assumption", "assumption": id, "state": "refuted",
            "note": "a partial line was delivered",
        }));

        // Through `events`, on its own cursor.
        let events =
            server.ok(json!({ "op": "events", "after": 0, "assumptions_after": assumptions_from }));
        let notified = events["assumptions"]
            .as_array()
            .expect("an assumptions array");
        assert_eq!(notified.len(), 1, "{events}");
        assert_eq!(notified[0]["state"], "refuted");
        assert_eq!(notified[0]["assumption"], id);
        assert_eq!(notified[0]["session"], worker);
        assert_eq!(notified[0]["claim"], "the parser is line-buffered");
        assert!(events["assumptions_head"].as_i64().unwrap() > assumptions_from);
        let again = server.ok(json!({
            "op": "events", "after": 0, "assumptions_after": events["assumptions_head"],
        }));
        assert_eq!(again["assumptions"], json!([]), "consumed is consumed");

        // Through the watcher, on the completion line, when the turn ends.
        fixture.hook(&root, &worker, "UserPromptSubmit");
        fixture.hook(&root, &worker, "Stop");
        wait_for("the orchestrator to be woken", || {
            fixture
                .received(&root, &orchestrator)
                .is_some_and(|text| !completions(&text).is_empty())
        });
        let text = fixture.received(&root, &orchestrator).unwrap();
        let delivered = completions(&text);
        assert_eq!(delivered.len(), 1, "{text}");
        let completion = &delivered[0];
        assert_eq!(completion["worker"], worker);
        assert_eq!(completion["assumptions"]["refuted"], 1, "{completion}");
        assert_eq!(
            completion["assumptions"]["budget_exceeded"], 0,
            "{completion}"
        );
        assert_eq!(
            completion["assumptions"]["ids"],
            json!([id]),
            "{completion}"
        );
        assert!(
            !completion.to_string().contains("line-buffered"),
            "a claim is never typed into another agent's terminal: {completion}"
        );

        // A second turn with nothing new carries zeros, not the same
        // refutation again.
        fixture.hook(&root, &worker, "UserPromptSubmit");
        fixture.hook(&root, &worker, "Stop");
        wait_for("the second completion", || {
            fixture
                .received(&root, &orchestrator)
                .is_some_and(|text| completions(&text).len() == 2)
        });
        let second = &completions(&fixture.received(&root, &orchestrator).unwrap())[1];
        assert_eq!(second["assumptions"]["refuted"], 0, "{second}");
        assert_eq!(second["assumptions"]["ids"], json!([]), "{second}");

        // And the person.
        let report = fixture
            .inner
            .run(&root, &["assumptions", "--session", &worker[..8]]);
        assert!(report.contains("refuted 1"), "{report}");
        assert!(report.contains("the parser is line-buffered"), "{report}");
        assert!(report.contains("a partial line was delivered"), "{report}");
        let shown = fixture
            .inner
            .run(&root, &["sessions", "show", &worker[..8]]);
        assert!(shown.contains("assumptions"), "{shown}");
        assert!(shown.contains("1 refuted"), "{shown}");
    }
}
