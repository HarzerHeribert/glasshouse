//! Phase 46, line 1746 — automated tests proving MCP operations remain bound
//! to the active project — and Phase 43's line 1702, which is the property
//! they prove.
//!
//! The shape is `tests/project_isolation.rs`'s: **two real, canonicalised
//! project roots sharing one `--data-dir`/`--config-dir`**, so the two
//! projects are two projects on one machine and not two directories in one
//! test. The MCP server is started in one of them, and every test asks it
//! about the other.
//!
//! Two cases per boundary, as that file has them. The *honest* case is the
//! one a real orchestrator would produce: a session id that exists in the
//! other project's database and not in this one, refused because this
//! project has never heard of it. The *defence-in-depth* case is a row
//! tagged with the other project's id planted directly into this project's
//! file, bypassing the insert trigger the way a restored backup or an older
//! build might — refused because `SessionApi::resolve` re-checks the row's
//! own `project_id`, which is the check the MCP door inherits by reaching
//! that seam through the same `dispatch` the socket does. Mutating that
//! check to a no-op is what fails the second case and not the first.
//!
//! Nothing here is `#[cfg(unix)]`: the server is driven over stdio, the
//! sessions are seeded records rather than processes, and the planted row is
//! SQL. It runs on Windows.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use clap::Parser;
use glasshouse::Cli;
use glasshouse::memory::{MemoryKind, NewMemory, ProjectMemory};
use glasshouse::session::{NewSession, ProjectSessions, SessionRole};
use rusqlite::Connection;
use serde_json::{Value, json};

const TIMEOUT: Duration = Duration::from_secs(30);

/// The four tools that take a session id, and therefore the four places a
/// caller could name another project's session.
const SESSION_TOOLS: [&str; 4] = [
    "glasshouse_session_status",
    "glasshouse_send_message",
    "glasshouse_interrupt_session",
    "glasshouse_recent_output",
];

// -------------------------------------------------------------------------
// Fixture — the same shape as `tests/mcp_server.rs`, kept in step by hand.
// -------------------------------------------------------------------------

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
}

impl Fixture {
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

    fn seed_session(&self, root: &Path) -> String {
        let runtime = self.runtime(root);
        let sessions = ProjectSessions::open(&runtime).expect("open the session store");
        let record = sessions
            .store()
            .create(NewSession::embedded("claude-code").with_role(SessionRole::Worker))
            .expect("seed a session record");
        record.id.as_str().to_owned()
    }

    /// A memory recorded through the project's own store, the way
    /// `glasshouse memory` records one.
    fn seed_memory(&self, root: &Path, body: &str) {
        let runtime = self.runtime(root);
        ProjectMemory::open(&runtime)
            .expect("open the memory store")
            .store()
            .record(NewMemory::new(MemoryKind::Finding, body))
            .expect("seed a memory");
    }

    /// A checkpoint taken through the shipped command, against a session
    /// the project really holds.
    fn seed_checkpoint(&self, root: &Path, objective: &str) {
        self.seed_session(root);
        let output = self
            .command(root)
            .args([
                "checkpoint",
                "save",
                "--objective",
                objective,
                "--state",
                "seeded",
            ])
            .output()
            .expect("run `glasshouse checkpoint save`");
        assert!(
            output.status.success(),
            "seeding a checkpoint failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn project_id(&self, root: &Path) -> String {
        self.runtime(root).project().id().as_str().to_owned()
    }

    /// A second, independent connection to one project's own database file,
    /// reached through the path `Runtime` already makes public.
    fn raw_connection(&self, root: &Path) -> Connection {
        Connection::open(self.runtime(root).database_path()).expect("open the database file")
    }

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
}

/// Insert a session row directly, bypassing the project-id trigger — copied
/// from `tests/project_isolation.rs::plant_foreign_session`, which copies
/// it from the trigger `database.rs` installs. Models a row that reached
/// the file by some route the trigger never saw.
fn plant_foreign_session(conn: &Connection, id: &str, project_id: &str) {
    conn.execute_batch("DROP TRIGGER sessions_reject_foreign_project_insert;")
        .unwrap();
    conn.execute(
        "INSERT INTO sessions (id, project_id, harness, native_session_id, role, \
         lifecycle, presentation, created_at, last_activity_at) \
         VALUES (?1, ?2, 'codex', 'native-1', 'normal', 'stopped', 'embedded', 10, 20)",
        rusqlite::params![id, project_id],
    )
    .unwrap();
    conn.execute_batch(
        "CREATE TRIGGER sessions_reject_foreign_project_insert
         BEFORE INSERT ON sessions
         FOR EACH ROW
         WHEN NEW.project_id IS NOT (
             SELECT value FROM project_metadata WHERE key = 'project_id'
         )
         BEGIN
             SELECT RAISE(ABORT, 'session belongs to a different project');
         END;",
    )
    .unwrap();
}

struct McpServer {
    child: Child,
    stdin: ChildStdin,
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
            stdin,
            lines,
            next_id: 0,
        };
        let reply = server.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "mcp_project_scope test", "version": "0" },
            }),
        );
        assert!(reply["error"].is_null(), "initialize was refused: {reply}");
        server.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        server
    }

    fn send(&mut self, frame: &Value) {
        self.stdin
            .write_all(format!("{frame}\n").as_bytes())
            .expect("write a frame");
        self.stdin.flush().expect("flush stdin");
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        let line = self
            .lines
            .recv_timeout(TIMEOUT)
            .expect("the server must answer within the timeout");
        let reply: Value =
            serde_json::from_str(&line).unwrap_or_else(|err| panic!("not JSON: {err}: {line}"));
        assert_eq!(
            reply["id"],
            json!(id),
            "a reply to a request this test never made: {reply}"
        );
        reply
    }

    fn tools(&mut self) -> Vec<Value> {
        let reply = self.request("tools/list", json!({}));
        reply["result"]["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("tools/list did not answer with tools: {reply}"))
            .clone()
    }

    /// One `tools/call` that must be answered with a tool result, as
    /// `(isError, text)`.
    fn call(&mut self, name: &str, arguments: Value) -> (bool, String) {
        let reply = self.request(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        );
        let result = &reply["result"];
        assert!(
            result.is_object(),
            "`{name}` must answer with a tool result, not a protocol error: {reply}"
        );
        let text = result["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("`{name}` must answer with text content: {reply}"))
            .to_owned();
        (result["isError"].as_bool().unwrap_or(false), text)
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The arguments each session tool needs, with `session` filled in.
fn arguments_for(tool: &str, session: &str) -> Value {
    match tool {
        "glasshouse_send_message" => json!({ "session": session, "text": "hello" }),
        _ => json!({ "session": session }),
    }
}

// -------------------------------------------------------------------------
// Line 1746 / line 1702.
// -------------------------------------------------------------------------

/// A server started in project alpha, asked about project beta's session —
/// by the id beta really assigned, and by a row planted into alpha's own
/// file under beta's project id. Refused both ways, on every tool that
/// takes a session, and the refusal names an id and never a path.
#[test]
fn a_tool_call_naming_another_projects_session_is_refused_without_leaking_its_path() {
    let fixture = Fixture::new();
    let alpha_root = fixture.project_root("alpha");
    let beta_root = fixture.project_root("beta");
    let alpha_session = fixture.seed_session(&alpha_root);
    let beta_session = fixture.seed_session(&beta_root);
    let beta_id = fixture.project_id(&beta_root);
    assert_ne!(
        fixture.project_id(&alpha_root),
        beta_id,
        "two roots must be two projects before the boundary means anything"
    );

    let mut alpha = McpServer::start(&fixture, &alpha_root);

    // The premise, first: alpha answers for its own session. A door that
    // refused everything would pass every negative assertion below.
    let (is_error, text) = alpha.call(
        "glasshouse_session_status",
        json!({ "session": alpha_session }),
    );
    assert!(!is_error, "alpha must answer for its own session: {text}");
    let status: Value = serde_json::from_str(&text).unwrap();
    assert!(status["lifecycle"].is_string(), "{status}");

    // Every string a refusal could leak. The roots and the shared base are
    // the paths; the database file name is the one thing under them a
    // caller could not already guess.
    let must_not_leak = [
        alpha_root.to_str().unwrap().to_owned(),
        beta_root.to_str().unwrap().to_owned(),
        fixture.base.to_str().unwrap().to_owned(),
        fixture
            .runtime(&beta_root)
            .database_path()
            .to_str()
            .unwrap()
            .to_owned(),
        "glasshouse.db".to_owned(),
    ];
    let leaks_nothing = |label: &str, text: &str| {
        for needle in &must_not_leak {
            assert!(
                !text.contains(needle.as_str()),
                "{label}: the refusal names a path it must not: `{needle}` in `{text}`"
            );
        }
    };

    // The honest case: beta's real session id, which alpha's database has
    // never held. Every session tool refuses it, and says which id.
    for tool in SESSION_TOOLS {
        let (is_error, text) = alpha.call(tool, arguments_for(tool, &beta_session));
        assert!(
            is_error,
            "{tool}: another project's session must be refused, got `{text}`"
        );
        assert!(
            text.contains(&beta_session),
            "{tool}: the refusal should name the id it refused: `{text}`"
        );
        leaks_nothing(tool, &text);
    }

    // The defence-in-depth case: a row in alpha's own file, tagged with
    // beta's project id. `SessionApi::resolve` compares the row's project
    // to the store's before anything else happens, and that comparison is
    // the check this door inherits from the socket.
    const PLANTED: &str = "planted-from-beta";
    plant_foreign_session(&fixture.raw_connection(&alpha_root), PLANTED, &beta_id);
    for tool in SESSION_TOOLS {
        let (is_error, text) = alpha.call(tool, arguments_for(tool, PLANTED));
        assert!(
            is_error,
            "{tool}: a row tagged with another project must be refused even from this \
             project's own file, got `{text}`"
        );
        assert!(
            text.contains("refusing to act on another project's session"),
            "{tool}: the refusal is the project check's, not a not-found: `{text}`"
        );
        leaks_nothing(tool, &text);
    }

    // Refusing is not deleting: the planted row is still there, still
    // beta's.
    let still_there: String = fixture
        .raw_connection(&alpha_root)
        .query_row(
            "SELECT project_id FROM sessions WHERE id = ?1",
            [PLANTED],
            |row| row.get(0),
        )
        .expect("the planted row survives a refusal");
    assert_eq!(still_there, beta_id);

    // And alpha's listing, asked afterwards, does not hand the foreign row
    // back either: the same seam filters it out of a list as refuses it by
    // name.
    let (is_error, text) = alpha.call("glasshouse_list_sessions", json!({}));
    assert!(!is_error, "{text}");
    assert!(
        !text.contains(PLANTED) && !text.contains(&beta_session),
        "alpha's listing must not include another project's session: {text}"
    );
    assert!(
        text.contains(&alpha_session),
        "alpha's listing still has alpha's own session: {text}"
    );
}

/// The other two stores the tools reach — memory and checkpoints — take no
/// session and no id at all, so the only project they can answer for is
/// the one the server was started in. Seeded in beta, invisible from alpha;
/// and the premise first, so an empty answer cannot pass for a scoped one.
#[test]
fn memory_and_checkpoints_are_answered_only_for_the_project_the_server_was_started_in() {
    let fixture = Fixture::new();
    let alpha_root = fixture.project_root("alpha");
    let beta_root = fixture.project_root("beta");
    fixture.seed_memory(&alpha_root, "alpha-only fact about the build");
    fixture.seed_memory(&beta_root, "beta-only fact about the build");
    fixture.seed_checkpoint(&beta_root, "beta objective nobody else may read");

    let mut alpha = McpServer::start(&fixture, &alpha_root);
    let mut beta = McpServer::start(&fixture, &beta_root);

    // The premise: each server finds its own project's memory.
    let query = json!({ "query": "fact about the build" });
    let (is_error, alpha_text) = alpha.call("glasshouse_search_memory", query.clone());
    assert!(!is_error, "{alpha_text}");
    assert!(
        alpha_text.contains("alpha-only fact"),
        "alpha must find its own memory before the boundary means anything: {alpha_text}"
    );
    let (is_error, beta_text) = beta.call("glasshouse_search_memory", query);
    assert!(!is_error, "{beta_text}");
    assert!(beta_text.contains("beta-only fact"), "{beta_text}");

    // The boundary, both ways.
    assert!(
        !alpha_text.contains("beta-only fact"),
        "alpha's server returned beta's memory: {alpha_text}"
    );
    assert!(
        !beta_text.contains("alpha-only fact"),
        "beta's server returned alpha's memory: {beta_text}"
    );

    // Checkpoints: beta has one, alpha has none, and alpha's server says so
    // rather than reaching for the nearest checkpoint on the machine.
    let (is_error, beta_text) = beta.call("glasshouse_get_checkpoint", json!({}));
    assert!(
        !is_error,
        "beta must retrieve its own checkpoint: {beta_text}"
    );
    assert!(beta_text.contains("beta objective"), "{beta_text}");
    let (is_error, alpha_text) = alpha.call("glasshouse_get_checkpoint", json!({}));
    assert!(
        is_error,
        "alpha has no checkpoint and must not be handed beta's: {alpha_text}"
    );
    assert!(
        !alpha_text.contains("beta objective"),
        "alpha's refusal leaks beta's checkpoint: {alpha_text}"
    );
    assert!(
        !alpha_text.contains(beta_root.to_str().unwrap()),
        "alpha's refusal names beta's path: {alpha_text}"
    );
}

/// The structural half of line 1702: the MCP layer adds no store access of
/// its own. Every operation reaches a store through `ServerContext::handle`
/// — the same `dispatch` the socket door uses — or not at all. Read as a
/// source scan because that is the level the ruling is made at: a
/// `rusqlite` import or a store `open` in this file would be the seam being
/// bypassed, whatever the code around it did with the handle.
#[test]
fn the_mcp_layer_opens_no_store_of_its_own() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/api/mcp.rs");
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("read {}: {err}", path.display()));

    // Code only: a doc comment may name the things this test forbids, since
    // explaining a rule is not breaking it.
    let code: String = source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");

    // The premise: this is the file, and it goes through the one seam.
    assert!(
        code.contains("ServerContext::open(") && code.contains(".handle("),
        "mcp.rs must reach the handlers through ServerContext, and nothing else"
    );

    for forbidden in [
        "rusqlite",
        "Connection::open",
        "database::open",
        "SessionStore::open",
        "ProjectSessions::open",
        "ProjectMemory::open",
        "MemoryStore::open",
        "ProjectCheckpoints::open",
        "EventLog::open",
        "SessionApi::new",
        "SessionRuntime::",
    ] {
        assert!(
            !code.contains(forbidden),
            "mcp.rs reaches a store on its own: `{forbidden}` appears outside a comment"
        );
    }
}

/// Ruling 5, checked against what a client actually sees: no tool has an
/// argument through which a project, a path, a database, or a socket could
/// be named. The scope is the process, and the schema offers no way around
/// it.
#[test]
fn no_tool_argument_can_name_a_project_a_path_or_a_socket() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let mut server = McpServer::start(&fixture, &root);
    let tools = server.tools();
    assert_eq!(tools.len(), 14, "{tools:?}");

    fn property_names(schema: &Value, into: &mut Vec<String>) {
        if let Some(properties) = schema["properties"].as_object() {
            for (name, property) in properties {
                into.push(name.clone());
                property_names(property, into);
                if let Some(items) = property.get("items") {
                    property_names(items, into);
                }
            }
        }
    }

    let mut names = Vec::new();
    for tool in &tools {
        property_names(&tool["inputSchema"], &mut names);
        assert_eq!(
            tool["inputSchema"]["additionalProperties"],
            json!(false),
            "an argument the schema does not name is refused, so the names are the whole \
             surface: {tool}"
        );
    }
    assert!(
        names.len() >= 10,
        "the premise — there are arguments to check: {names:?}"
    );

    for name in &names {
        let lower = name.to_ascii_lowercase();
        for forbidden in [
            "project", "path", "dir", "database", "db", "socket", "scope", "root", "file", "cwd",
        ] {
            assert!(
                !lower.contains(forbidden),
                "argument `{name}` could name another project's state (`{forbidden}`)"
            );
        }
    }
}
