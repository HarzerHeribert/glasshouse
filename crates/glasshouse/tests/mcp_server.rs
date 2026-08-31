//! Phase 43 — the MCP door, driven over the shipped binary's stdin and stdout.
//!
//! Every test here starts `glasshouse mcp serve` as an orchestrator harness
//! would — a child process, JSON-RPC frames one per line — and asserts on
//! what comes back. Nothing reaches into the server's process; the only
//! thing a test shares with it is the project's database, which is how a
//! test seeds a session for the server to list and how it reads the event
//! log the server wrote.
//!
//! The protocol tests are cross-platform. The two that need a live harness
//! process (`send_message_reaches_the_session_as_a_machine_origin_message`
//! and its interrupt half) are `#[cfg(unix)]`, because the fake harness is
//! a shell script — the same fixture `tests/api_event_log.rs` uses.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use clap::Parser;
use glasshouse::Cli;
use glasshouse::session::{NewSession, ProjectSessions, SessionRole};
use serde_json::{Value, json};

const TIMEOUT: Duration = Duration::from_secs(30);

/// The eight tools Phase 43's packet names plus Phase 21K's five guardrail tools and Phase 21H–J's policy
/// tools, and the whole surface: a fifteenth would be a new decision, and a
/// missing one a regression.
const EXPECTED_TOOLS: [&str; 14] = [
    "glasshouse_get_checkpoint",
    "glasshouse_implementation_policy",
    "glasshouse_interrupt_session",
    "glasshouse_list_assumptions",
    "glasshouse_list_sessions",
    "glasshouse_preflight",
    "glasshouse_promote_assumption",
    "glasshouse_recent_output",
    "glasshouse_record_assumption",
    "glasshouse_search_memory",
    "glasshouse_send_message",
    "glasshouse_session_status",
    "glasshouse_spawn_session",
    "glasshouse_update_assumption",
];

const STATE_CHANGING_TOOLS: [&str; 3] = [
    "glasshouse_spawn_session",
    "glasshouse_send_message",
    "glasshouse_interrupt_session",
];

/// Phase 21K's four writers: they append to the project's assumption ledger
/// (and, on request, write one memory or take one checkpoint) and touch no
/// session's state — not read-only, and not destructive.
const LEDGER_WRITING_TOOLS: [&str; 4] = [
    "glasshouse_preflight",
    "glasshouse_record_assumption",
    "glasshouse_update_assumption",
    "glasshouse_promote_assumption",
];

// -------------------------------------------------------------------------
// Fixture — one data/config root, any number of real projects under it.
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
        // The minimum a user config needs to be one. The Unix-only harness
        // installer below rewrites it with an installed harness.
        std::fs::write(config_dir.join("config.toml"), "version = 1\n").expect("write user config");
        Self { _tmp: tmp, base }
    }

    /// A real, canonicalised project root with its own `.git`.
    fn project_root(&self, name: &str) -> PathBuf {
        let root = self.base.join("workspace").join(name);
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        std::fs::canonicalize(&root).expect("canonicalize project root")
    }

    /// This fixture's own [`glasshouse::Runtime`] for one project,
    /// bootstrapped exactly as the binary bootstraps its own — the one way a
    /// test reaches the database the server is answering from.
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

    /// A session record the server did not create, so a listing has
    /// something to list without a harness process on any platform.
    fn seed_session(&self, root: &Path, harness: &str) -> String {
        let runtime = self.runtime(root);
        let sessions = ProjectSessions::open(&runtime).expect("open the session store");
        let record = sessions
            .store()
            .create(NewSession::embedded(harness).with_role(SessionRole::Worker))
            .expect("seed a session record");
        record.id.as_str().to_owned()
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
}

// -------------------------------------------------------------------------
// The MCP client side: a running `glasshouse mcp serve`, killed on drop.
// -------------------------------------------------------------------------

struct McpServer {
    child: Child,
    stdin: Option<ChildStdin>,
    /// Every line the server writes to stdout, read by a thread so a reply
    /// that never comes is a timeout with a message rather than a hang.
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
        Self {
            child,
            stdin: Some(stdin),
            lines,
            next_id: 0,
        }
    }

    /// Write one frame, exactly as given, plus the newline that ends it.
    fn send_raw(&mut self, frame: &str) {
        let stdin = self.stdin.as_mut().expect("stdin is still open");
        stdin
            .write_all(format!("{frame}\n").as_bytes())
            .expect("write a frame");
        stdin.flush().expect("flush stdin");
    }

    /// The next line the server writes, parsed.
    fn next_reply(&self) -> Value {
        let line = self
            .lines
            .recv_timeout(TIMEOUT)
            .expect("the server must answer within the timeout");
        serde_json::from_str(&line).unwrap_or_else(|err| panic!("not JSON: {err}: {line}"))
    }

    fn notify(&mut self, method: &str, params: Value) {
        let frame = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        self.send_raw(&frame.to_string());
    }

    /// A request, and the reply that carries its id.
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

    /// The handshake, as a client performs it.
    fn initialize(&mut self) -> Value {
        let reply = self.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "mcp_server test", "version": "0" },
            }),
        );
        assert!(reply["error"].is_null(), "initialize was refused: {reply}");
        self.notify("notifications/initialized", json!({}));
        reply["result"].clone()
    }

    fn tools(&mut self) -> Vec<Value> {
        let reply = self.request("tools/list", json!({}));
        reply["result"]["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("tools/list did not answer with tools: {reply}"))
            .clone()
    }

    /// One `tools/call`, whole reply — result or protocol error.
    fn call(&mut self, name: &str, arguments: Value) -> Value {
        self.request(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        )
    }

    /// One `tools/call` that must be answered with a tool result, as
    /// `(isError, text)`.
    fn call_text(&mut self, name: &str, arguments: Value) -> (bool, String) {
        let reply = self.call(name, arguments);
        let result = &reply["result"];
        assert!(
            result.is_object(),
            "`{name}` must answer with a tool result, not a protocol error: {reply}"
        );
        let is_error = result["isError"].as_bool().unwrap_or(false);
        let text = result["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("`{name}` must answer with text content: {reply}"))
            .to_owned();
        assert_eq!(result["content"][0]["type"], "text", "{reply}");
        (is_error, text)
    }

    /// Close the client's end of stdin — how a harness shuts a server down.
    fn close_stdin(&mut self) {
        self.stdin.take();
    }

    fn wait_for_exit(&mut self) -> ExitStatus {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Some(status) = self.child.try_wait().expect("poll the server") {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "the server did not exit after stdin was closed"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn names(tools: &[Value]) -> Vec<&str> {
    let mut names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().expect("a tool name"))
        .collect();
    names.sort_unstable();
    names
}

fn tool<'a>(tools: &'a [Value], name: &str) -> &'a Value {
    tools
        .iter()
        .find(|tool| tool["name"] == name)
        .unwrap_or_else(|| panic!("no tool named `{name}` in {tools:?}"))
}

// -------------------------------------------------------------------------
// The protocol, on every platform.
// -------------------------------------------------------------------------

/// Lines 1694 and 1695: the handshake, the tool catalogue, and a listing
/// that agrees with `glasshouse sessions` about what is in this project.
#[test]
fn an_orchestrator_can_initialize_list_tools_and_list_sessions_over_stdio() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let seeded = fixture.seed_session(&root, "claude-code");

    let mut server = McpServer::start(&fixture, &root);

    let init = server.initialize();
    assert_eq!(init["protocolVersion"], "2025-06-18", "{init}");
    assert!(
        init["capabilities"]["tools"].is_object(),
        "the server must declare the tools capability: {init}"
    );
    assert_eq!(init["serverInfo"]["name"], "glasshouse", "{init}");
    assert!(
        init["instructions"]
            .as_str()
            .is_some_and(|text| text.contains("glasshouse_spawn_session")),
        "the instructions name the tools that change state: {init}"
    );

    let ping = server.request("ping", json!({}));
    assert_eq!(ping["result"], json!({}), "{ping}");

    let tools = server.tools();
    assert_eq!(
        names(&tools),
        EXPECTED_TOOLS,
        "the catalogue is the fourteen tools, exactly"
    );
    for tool in &tools {
        assert_eq!(tool["inputSchema"]["type"], "object", "{tool}");
        assert!(
            tool["description"].as_str().is_some_and(|d| !d.is_empty()),
            "{tool}"
        );
    }

    let (is_error, text) = server.call_text("glasshouse_list_sessions", json!({}));
    assert!(!is_error, "{text}");
    let listed: Vec<Value> = serde_json::from_str(&text).expect("a JSON array of sessions");
    let mine = listed
        .iter()
        .find(|entry| entry["session"] == seeded.as_str())
        .unwrap_or_else(|| panic!("the seeded session is not in the listing: {text}"));
    assert_eq!(mine["harness"], "claude-code", "{mine}");
    assert_eq!(mine["role"], "worker", "{mine}");

    // The same project, through the one-shot command: the two must agree
    // about membership, because they read the same store through the same
    // seam. `glasshouse sessions` prints the first twelve characters of an
    // id.
    let output = fixture
        .command(&root)
        .arg("sessions")
        .output()
        .expect("run `glasshouse sessions`");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(&seeded[..12]),
        "`glasshouse sessions` must list the session the MCP door listed:\n{stdout}"
    );

    // The two read-only tools that need no session, answered as tool results
    // — lines 1700 and 1701's plumbing, on a project with nothing in it yet.
    let (is_error, text) =
        server.call_text("glasshouse_search_memory", json!({ "query": "anything" }));
    assert!(
        !is_error,
        "an empty memory is an empty answer, not a refusal: {text}"
    );
    let _: Value = serde_json::from_str(&text).expect("search results as JSON");
    let reply = server.call("glasshouse_get_checkpoint", json!({}));
    assert!(
        reply["result"]["content"][0]["text"].is_string(),
        "a project with no checkpoint is answered with a tool result either way: {reply}"
    );
}

/// Line 1703: the three tools that change a session's state are their own
/// tools, marked so a harness that gates on the annotations can, and
/// described so a harness that shows the description to a person can too.
#[test]
fn state_changing_tools_are_separate_and_marked_so_a_harness_can_gate_them() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let mut server = McpServer::start(&fixture, &root);
    server.initialize();
    let tools = server.tools();

    for entry in &tools {
        let name = entry["name"].as_str().unwrap();
        assert!(
            !name.contains("control"),
            "no umbrella tool with an action argument: {name}"
        );
        for property in ["action", "op", "operation"] {
            assert!(
                entry["inputSchema"]["properties"][property].is_null(),
                "`{name}` must not multiplex operations through `{property}`: {entry}"
            );
        }
        for hint in [
            "readOnlyHint",
            "destructiveHint",
            "idempotentHint",
            "openWorldHint",
        ] {
            assert!(
                entry["annotations"][hint].is_boolean(),
                "`{name}` must state `{hint}` rather than leave it to the default: {entry}"
            );
        }
        assert_eq!(
            entry["annotations"]["openWorldHint"], false,
            "nothing here reaches beyond this machine: {entry}"
        );
    }

    for name in STATE_CHANGING_TOOLS {
        let entry = tool(&tools, name);
        assert_eq!(
            entry["annotations"]["readOnlyHint"], false,
            "`{name}` changes a session's state and must say so: {entry}"
        );
    }
    let says = |name: &str, phrase: &str| {
        let description = tool(&tools, name)["description"].as_str().unwrap();
        assert!(
            description.contains(phrase),
            "`{name}`'s description must say `{phrase}`: {description}"
        );
    };
    says("glasshouse_spawn_session", "STARTS A PROCESS");
    says(
        "glasshouse_send_message",
        "INJECTS INPUT INTO A RUNNING HARNESS",
    );
    says(
        "glasshouse_interrupt_session",
        "INTERRUPTS A RUNNING HARNESS",
    );

    for name in EXPECTED_TOOLS {
        if STATE_CHANGING_TOOLS.contains(&name) {
            continue;
        }
        let entry = tool(&tools, name);
        if LEDGER_WRITING_TOOLS.contains(&name) {
            assert_eq!(
                entry["annotations"]["readOnlyHint"], false,
                "`{name}` writes to the ledger and must say so: {entry}"
            );
            assert_eq!(
                entry["annotations"]["destructiveHint"], false,
                "`{name}` only appends and must say so: {entry}"
            );
            continue;
        }
        assert_eq!(
            entry["annotations"]["readOnlyHint"], true,
            "`{name}` only reads and must say so: {entry}"
        );
        assert_eq!(
            entry["annotations"]["destructiveHint"], false,
            "`{name}` only reads and must say so: {entry}"
        );
    }
}

/// The transport's error handling: a frame the server cannot parse is
/// answered with `-32700`, and the frames after it are answered normally.
/// Alongside it, the other protocol-level refusals — a batch, an unknown
/// method, an unknown tool, an argument a tool does not take — and the one
/// thing that must *not* be answered, a notification.
#[test]
fn a_malformed_frame_is_answered_with_a_parse_error_and_the_server_keeps_serving() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let mut server = McpServer::start(&fixture, &root);
    server.initialize();

    server.send_raw(r#"{"jsonrpc":"2.0","id":7,"method":"tools/list""#);
    let reply = server.next_reply();
    assert_eq!(reply["error"]["code"], -32700, "{reply}");
    assert!(
        reply["id"].is_null(),
        "a frame with no readable id is answered to null: {reply}"
    );

    server.send_raw(r#"[{"jsonrpc":"2.0","id":8,"method":"ping"}]"#);
    let reply = server.next_reply();
    assert_eq!(
        reply["error"]["code"], -32600,
        "a batch is refused: {reply}"
    );

    let reply = server.request("no/such/method", json!({}));
    assert_eq!(reply["error"]["code"], -32601, "{reply}");

    let reply = server.call("glasshouse_no_such_tool", json!({}));
    assert_eq!(
        reply["error"]["code"], -32602,
        "an unknown tool is a protocol error: {reply}"
    );

    let reply = server.call(
        "glasshouse_session_status",
        json!({ "session": "whatever", "project": "elsewhere" }),
    );
    assert_eq!(
        reply["error"]["code"], -32602,
        "an argument a tool does not take is refused, not ignored: {reply}"
    );
    let reply = server.call("glasshouse_session_status", json!({}));
    assert_eq!(
        reply["error"]["code"], -32602,
        "a missing required argument: {reply}"
    );

    // A notification gets no reply: the very next line must be the ping's.
    server.notify("notifications/cancelled", json!({ "requestId": 1 }));
    server.next_id += 1;
    let id = server.next_id;
    server.send_raw(&json!({ "jsonrpc": "2.0", "id": id, "method": "ping" }).to_string());
    let reply = server.next_reply();
    assert_eq!(
        reply["id"],
        json!(id),
        "a notification must not be answered: {reply}"
    );
    assert_eq!(reply["result"], json!({}), "{reply}");

    // And after all of that, the server is still the same server.
    assert_eq!(names(&server.tools()), EXPECTED_TOOLS);
}

/// The shutdown a harness performs: close stdin, expect the server to exit.
#[test]
fn the_server_exits_cleanly_when_the_client_closes_stdin() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let mut server = McpServer::start(&fixture, &root);
    server.initialize();
    assert_eq!(server.request("ping", json!({}))["result"], json!({}));

    server.close_stdin();
    let status = server.wait_for_exit();
    assert!(status.success(), "EOF on stdin is a clean end: {status}");
}

// -------------------------------------------------------------------------
// A live harness — Unix only, because the fake harness is a shell script.
// -------------------------------------------------------------------------

#[cfg(unix)]
mod live {
    use super::*;
    use glasshouse::events::{EventLog, LifecycleEvent, MessageOrigin};
    use glasshouse::session::SessionId;
    use std::os::unix::fs::PermissionsExt;

    /// The session-tagging harness `tests/api_event_log.rs` uses: it names
    /// its log files after the session it was started for, taken from the
    /// `--settings <state>/sessions/<id>/settings.json` argument the
    /// lifecycle-hook installation adds, echoes every line it reads, and
    /// records it.
    fn install_harness(fixture: &Fixture) {
        let bin_dir = fixture.base.join("bin");
        std::fs::create_dir_all(&bin_dir).expect("create bin dir");
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
        .expect("write the harness");
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();

        let escaped = path.display().to_string().replace('\\', "\\\\");
        // `implementation_policy = false`: this file is about the MCP door's
        // own tools, and Glasshouse's implementation policy (`src/policy`) is
        // several machine-origin deliveries into every session it briefs,
        // which would shift the delivery this test reads without saying
        // anything about MCP. The policy's own delivery is proven in
        // `tests/implementation_policy.rs`, and its own MCP tool is asserted
        // in the catalogue above.
        std::fs::write(
            fixture.base.join("config").join("config.toml"),
            format!(
                "version = 1\nimplementation_policy = false\n\n[integrations.claude-code]\nenabled = \
                 true\nexecutable = \"{escaped}\"\n"
            ),
        )
        .expect("write user config");
    }

    fn argv(root: &Path, session: &str) -> Option<String> {
        std::fs::read_to_string(root.join(format!("argv-{session}.log"))).ok()
    }

    fn received(root: &Path, session: &str) -> Option<String> {
        std::fs::read_to_string(root.join(format!("received-{session}.log"))).ok()
    }

    /// Poll until `done`, or fail with `what` — only the live tests wait on
    /// anything, which is why this lives here and not at the top of the
    /// file, where Windows would find it unused.
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

    /// Lines 1696–1699 on a real process, and ruling 4: a message sent
    /// through the MCP door reaches the harness's terminal and is recorded
    /// in the project's event log as a machine-originated delivery — and
    /// there is no argument through which a caller could say otherwise.
    #[test]
    fn send_message_reaches_the_session_as_a_machine_origin_message() {
        const TEXT: &str = "mcp-intervention-one";

        let fixture = Fixture::new();
        install_harness(&fixture);
        let root = fixture.project_root("alpha");
        let mut server = McpServer::start(&fixture, &root);
        server.initialize();

        let (is_error, text) = server.call_text(
            "glasshouse_spawn_session",
            json!({ "harness": "claude-code", "role": "worker" }),
        );
        assert!(!is_error, "spawn refused: {text}");
        let spawned: Value = serde_json::from_str(&text).expect("spawn result as JSON");
        let session = spawned["session"]
            .as_str()
            .expect("a session id")
            .to_owned();
        wait_for("the worker's harness to start", || {
            argv(&root, &session).is_some()
        });

        let (is_error, text) =
            server.call_text("glasshouse_session_status", json!({ "session": session }));
        assert!(!is_error, "{text}");
        let status: Value = serde_json::from_str(&text).unwrap();
        assert!(status["lifecycle"].is_string(), "{status}");

        let (is_error, text) = server.call_text(
            "glasshouse_send_message",
            json!({ "session": session, "text": TEXT }),
        );
        assert!(!is_error, "send refused: {text}");
        wait_for("the worker to read the delivered line", || {
            received(&root, &session).is_some_and(|log| log.contains(TEXT))
        });

        // What came back, through the read-only half — line 1698's
        // "status" in the sense a person means it: what is the worker doing.
        wait_for("the harness's echo to reach the scrollback", || {
            let (is_error, text) =
                server.call_text("glasshouse_recent_output", json!({ "session": session }));
            !is_error
                && serde_json::from_str::<Value>(&text).unwrap()["output"]
                    .as_str()
                    .is_some_and(|output| output.contains(&format!("got:{TEXT}")))
        });

        // The origin is not the caller's to state: the tool has no such
        // argument, and offering one is refused before anything is sent.
        let reply = server.call(
            "glasshouse_send_message",
            json!({ "session": session, "text": "never-delivered", "origin": "user" }),
        );
        assert_eq!(reply["error"]["code"], -32602, "{reply}");

        let (is_error, text) = server.call_text(
            "glasshouse_interrupt_session",
            json!({ "session": session }),
        );
        assert!(!is_error, "interrupt refused: {text}");

        // Asserted through the durable log, the way `tests/api_event_log.rs`
        // asserts the socket door's deliveries: the recorder writes on its
        // own thread, so the rows are waited for rather than expected.
        let runtime = fixture.runtime(&root);
        let log = EventLog::open(&runtime).expect("open the event log");
        let id = SessionId::new(session.clone());
        let mut history = Vec::new();
        wait_for("the delivery and the interrupt to be recorded", || {
            history = log.for_session(&id).expect("read the session's history");
            let delivered = history
                .iter()
                .any(|row| matches!(row.event, LifecycleEvent::TextDelivered { .. }));
            let interrupted = history
                .iter()
                .any(|row| matches!(row.event, LifecycleEvent::InterruptDelivered { .. }));
            delivered && interrupted
        });
        let delivered = history
            .iter()
            .find(|row| matches!(row.event, LifecycleEvent::TextDelivered { .. }))
            .unwrap();
        match delivered.event {
            LifecycleEvent::TextDelivered { origin, bytes } => {
                assert_eq!(
                    origin,
                    MessageOrigin::Machine,
                    "an MCP caller is a program, and is recorded as one"
                );
                assert_eq!(bytes, TEXT.len() + 1, "the line, carriage return included");
            }
            _ => unreachable!(),
        }
        let interrupted = history
            .iter()
            .find(|row| matches!(row.event, LifecycleEvent::InterruptDelivered { .. }))
            .unwrap();
        assert!(
            matches!(
                interrupted.event,
                LifecycleEvent::InterruptDelivered {
                    origin: MessageOrigin::Machine
                }
            ),
            "an interrupt through this door is a machine's too: {:?}",
            interrupted.event
        );
        assert!(
            received(&root, &session).is_some_and(|log| !log.contains("never-delivered")),
            "a refused call must not have delivered anything"
        );
    }
}
