//! Acceptance tests for `pane session` (map lines 2444, 2446, 2447, 2448,
//! 2449, 2450 and 2457). Every test here drives the **built `pane` binary** as a subprocess
//! (`env!("CARGO_BIN_EXE_pane")`) -- the packet these prove exists precisely
//! because the six modules session.rs wires together were, until now,
//! correct and reachable from nothing but their own unit tests.
//!
//! No test reaches the real network or a real credential: the Anthropic
//! endpoint is a hand-rolled HTTP/1.1 server bound to `127.0.0.1:0`, and
//! every "glasshouse" is a shell script this file writes into its own temp
//! directory, exactly as `tests/seams.rs` and `tests/ruler_run.rs` already
//! do. 61D's sandbox is not built, so nothing model-authored may execute
//! here either.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;

fn scratch_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "pane-session-test-{}-{}-{}",
        label,
        std::process::id(),
        unique()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn unique() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Unix only: the fakes are shell scripts. The Windows pane cell runs every
/// other test in this file; a `.cmd` twin is the successor if one is wanted.
#[cfg(unix)]
fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    fs::write(&path, body).unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
    path
}

/// A fake `glasshouse` that records its own argv, one line per invocation.
#[cfg(unix)]
fn write_argv_recorder(dir: &Path, record: &Path) -> PathBuf {
    let body = format!("#!/bin/sh\necho \"$@\" >> \"{}\"\n", record.display());
    write_script(dir, "fake_glasshouse.sh", &body)
}

/// Binds an ephemeral local port and drops the listener immediately, so any
/// connection to it is refused fast, locally, and without ever reaching a
/// real host -- the guard every test that must not send a request uses for
/// `ANTHROPIC_BASE_URL`.
fn refused_base_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    format!("http://127.0.0.1:{port}")
}

/// A minimal Anthropic Messages endpoint: for each reply in `replies`, in
/// order, accepts one connection, reads the request body up to its declared
/// `Content-Length`, records it, and answers with that reply's bytes as a
/// `200 application/json` response. Exits once every reply has been sent.
fn start_fake_provider(replies: Vec<String>) -> (String, Arc<Mutex<Vec<String>>>) {
    let turns = replies.len();
    let next = Mutex::new(0usize);
    start_answering_provider(turns, move |_body| {
        let mut index = next.lock().unwrap();
        let reply = replies[*index].clone();
        *index += 1;
        reply
    })
}

/// The same endpoint, answering each request from the **request body**.
///
/// A task's second turn is only meaningful if the model saw what the runtime
/// said in the first: a fixed list answers a request nobody looked at, and
/// would pass just as happily if the result block had been empty.
fn start_answering_provider<F>(turns: usize, answer: F) -> (String, Arc<Mutex<Vec<String>>>)
where
    F: Fn(&str) -> String + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let bodies_thread = Arc::clone(&bodies);

    thread::spawn(move || {
        for _ in 0..turns {
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            handle_one_request(stream, &answer, &bodies_thread);
        }
    });

    (format!("http://127.0.0.1:{port}"), bodies)
}

fn handle_one_request<F: Fn(&str) -> String>(
    mut stream: TcpStream,
    answer: &F,
    bodies: &Mutex<Vec<String>>,
) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        if let Some(rest) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = rest.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; content_length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    let body = String::from_utf8_lossy(&body).into_owned();
    let reply = answer(&body);
    bodies.lock().unwrap().push(body);

    let response_body = reply.as_bytes();
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        response_body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.write_all(response_body);
    let _ = stream.flush();
}

/// The same endpoint, answering with a **status** as well as a body.
///
/// A context overflow is a 400, not a reply, so a recovery test cannot be
/// written against a provider that only ever answers 200.
fn start_status_answering_provider<F>(turns: usize, answer: F) -> (String, Arc<Mutex<Vec<String>>>)
where
    F: Fn(&str) -> (u16, String) + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let bodies_thread = Arc::clone(&bodies);

    thread::spawn(move || {
        for _ in 0..turns {
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            handle_one_request_with_status(stream, &answer, &bodies_thread);
        }
    });

    (format!("http://127.0.0.1:{port}"), bodies)
}

fn handle_one_request_with_status<F: Fn(&str) -> (u16, String)>(
    mut stream: TcpStream,
    answer: &F,
    bodies: &Mutex<Vec<String>>,
) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        if let Some(rest) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = rest.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; content_length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    let body = String::from_utf8_lossy(&body).into_owned();
    let (status, reply) = answer(&body);
    bodies.lock().unwrap().push(body);

    let response_body = reply.as_bytes();
    let response = format!(
        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        response_body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.write_all(response_body);
    let _ = stream.flush();
}

/// The body a provider sends when the conversation no longer fits.
fn too_long_body() -> String {
    serde_json::json!({
        "type": "error",
        "error": {
            "type": "invalid_request_error",
            "message": "prompt is too long: 250000 tokens > 200000 maximum"
        }
    })
    .to_string()
}

/// Builds a Messages-shaped response through `serde_json` rather than a
/// format string, so a reply carrying a quote or a newline (the shape
/// `nothing_the_model_returns_is_executed` needs) still serialises to valid
/// JSON.
fn assistant_reply(text: &str) -> String {
    serde_json::json!({
        "role": "assistant",
        "content": [{"type": "text", "text": text}],
    })
    .to_string()
}

/// The same reply, plus a Messages `usage` object -- the shape a direct
/// provider always sends and the gateway tests never need, so this stays a
/// separate builder rather than a change to [`assistant_reply`] that every
/// other fixture in this file would inherit.
#[cfg(unix)] // its only callers are the two unix-gated usage tests; dead on Windows otherwise
fn assistant_reply_with_usage(text: &str, input_tokens: u64, output_tokens: u64) -> String {
    serde_json::json!({
        "role": "assistant",
        "content": [{"type": "text", "text": text}],
        "usage": {"input_tokens": input_tokens, "output_tokens": output_tokens},
    })
    .to_string()
}

fn run_session(
    root: &Path,
    rollout: &Path,
    session_id: &str,
    task: &str,
    base_url: &str,
    glasshouse: Option<&Path>,
) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_pane"));
    command
        .arg("session")
        .arg("--root")
        .arg(root)
        .arg("--rollout")
        .arg(rollout)
        .arg("--session")
        .arg(session_id)
        .arg("--task")
        .arg(task)
        .env("ANTHROPIC_BASE_URL", base_url)
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .env_remove("ANTHROPIC_API_KEY");
    if let Some(glasshouse) = glasshouse {
        command.arg("--glasshouse").arg(glasshouse);
    }
    command.output().unwrap()
}

/// The reply that ends a task: a cell whose one statement is a top-level
/// `return`.
///
/// **Every test whose own scripted reply is prose needs one.** A prose reply
/// is *answered*, not obeyed (`model-contract.md` §5): pane sends back the
/// unchanged handle table and one line, and the task runs on until something
/// ends it. Before the session loop existed a turn was the whole run, and
/// these fixtures scripted one reply because one reply was all a run could
/// consume.
fn ending_reply() -> String {
    assistant_reply("```pane\nreturn 1;\n```")
}

/// [`ending_reply`], with a `usage` object attached.
#[cfg(unix)] // its only callers are the two unix-gated usage tests; dead on Windows otherwise
fn ending_reply_with_usage(input_tokens: u64, output_tokens: u64) -> String {
    assistant_reply_with_usage("```pane\nreturn 1;\n```", input_tokens, output_tokens)
}

/// The text of the last `user` message in a recorded request body -- what the
/// runtime told the model on the turn that request opened.
fn last_user_text(body: &str) -> String {
    let request: serde_json::Value = serde_json::from_str(body).unwrap();
    let messages = request["messages"].as_array().unwrap();
    messages
        .iter()
        .rev()
        .find(|message| message["role"] == "user")
        .expect("every request carries at least the task")["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Every `cell` line in the rollout, in file order.
fn cell_lines(path: &Path) -> Vec<serde_json::Value> {
    rollout_lines(path)
        .into_iter()
        .filter(|line| line["kind"] == "cell")
        .collect()
}

fn rollout_lines(path: &Path) -> Vec<serde_json::Value> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn the_binary_runs_a_turn_and_writes_a_rollout() {
    let root = scratch_dir("turn-root");
    let rollout = root.join("rollout.jsonl");
    let (base_url, bodies) =
        start_fake_provider(vec![assistant_reply("hi from the model"), ending_reply()]);

    let output = run_session(&root, &rollout, "sess-turn", "hello there", &base_url, None);

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(rollout.exists(), "the rollout file must be written");

    let lines = rollout_lines(&rollout);
    assert!(
        lines
            .iter()
            .any(|l| l["kind"] == "turn" && l["role"] == "user" && l["text"] == "hello there"),
        "no user turn in {lines:?}"
    );
    assert!(
        lines.iter().any(|l| l["kind"] == "turn"
            && l["role"] == "assistant"
            && l["text"] == "hi from the model"),
        "no assistant turn in {lines:?}"
    );

    // Two, not one: the model's prose reply is answered with the handle
    // table and one line (§5), and the task runs until the second reply
    // returns. A run that stopped after one turn would be the old
    // one-turn-per-input session, not a task.
    assert_eq!(bodies.lock().unwrap().len(), 2);
}

#[test]
fn the_binary_resumes_an_existing_rollout_instead_of_starting_over() {
    let root = scratch_dir("resume-root");
    let rollout = root.join("rollout.jsonl");

    let (first_url, _first_bodies) =
        start_fake_provider(vec![assistant_reply("first reply"), ending_reply()]);
    let first = run_session(
        &root,
        &rollout,
        "sess-resume",
        "first message",
        &first_url,
        None,
    );
    assert!(
        first.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&first.stderr)
    );

    let (second_url, second_bodies) =
        start_fake_provider(vec![assistant_reply("second reply"), ending_reply()]);
    let second = run_session(
        &root,
        &rollout,
        "sess-resume",
        "second message",
        &second_url,
        None,
    );
    assert!(
        second.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&second.stderr)
    );

    let bodies = second_bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2);
    let request: serde_json::Value = serde_json::from_str(&bodies[0]).unwrap();
    let messages = request["messages"].as_array().unwrap();
    let texts: Vec<&str> = messages
        .iter()
        .map(|m| m["content"][0]["text"].as_str().unwrap())
        .collect();

    assert!(
        texts.contains(&"first message"),
        "second run's request must carry the first run's user turn: {texts:?}"
    );
    assert!(
        texts.contains(&"first reply"),
        "second run's request must carry the first run's assistant turn: {texts:?}"
    );
    assert!(
        texts.contains(&"second message"),
        "second run's request must also carry its own new turn: {texts:?}"
    );
}

#[test]
fn the_binary_loads_the_projects_own_instructions() {
    let root = scratch_dir("instructions-root");
    fs::write(
        root.join("CLAUDE.md"),
        "PANE-SESSION-TEST-MARKER-loads-its-own-claude-md",
    )
    .unwrap();
    let rollout = root.join("rollout.jsonl");
    let (base_url, bodies) = start_fake_provider(vec![assistant_reply("ack"), ending_reply()]);

    let output = run_session(&root, &rollout, "sess-instructions", "hi", &base_url, None);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let bodies = bodies.lock().unwrap();
    let request: serde_json::Value = serde_json::from_str(&bodies[0]).unwrap();
    let system = request["system"].as_str().unwrap();
    assert!(
        system.contains("PANE-SESSION-TEST-MARKER-loads-its-own-claude-md"),
        "system prompt did not carry CLAUDE.md's content: {system}"
    );
}

#[cfg(unix)]
#[test]
fn the_binary_emits_session_start_to_the_hook_command() {
    let root = scratch_dir("hook-root");
    let rollout = root.join("rollout.jsonl");
    let record = root.join("argv.txt");
    let glasshouse = write_argv_recorder(&root, &record);
    let (base_url, _bodies) = start_fake_provider(vec![assistant_reply("ack"), ending_reply()]);

    let output = run_session(
        &root,
        &rollout,
        "sess-hook",
        "hi",
        &base_url,
        Some(&glasshouse),
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let seen = fs::read_to_string(&record).unwrap();
    assert!(
        seen.lines()
            .any(|line| line == "hook --session sess-hook --event SessionStart"),
        "argv log did not carry a SessionStart hook call: {seen}"
    );
}

#[test]
fn a_slash_command_is_answered_without_a_request() {
    let root = scratch_dir("slash-root");
    let rollout = root.join("rollout.jsonl");
    let base_url = refused_base_url();

    let output = run_session(&root, &rollout, "sess-slash", "/model", &base_url, None);

    assert!(
        output.status.success(),
        "a slash command must not fail even though the configured base URL refuses every connection: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    if rollout.exists() {
        let lines = rollout_lines(&rollout);
        assert!(
            lines.iter().all(|l| l["kind"] != "turn"),
            "a slash command must not be recorded as a turn: {lines:?}"
        );
    }
}

#[test]
fn handles_command_reports_the_recorded_preview() {
    let root = scratch_dir("unbuilt-root");
    let rollout = root.join("rollout.jsonl");
    let base_url = refused_base_url();

    let output = run_session(&root, &rollout, "sess-unbuilt", "/handles", &base_url, None);

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("No handles recorded yet"),
        "the command must report available recorded handles: {stdout}"
    );
}

#[test]
fn the_binary_with_no_arguments_still_echoes_a_line() {
    use std::io::Write as _;

    let mut child = Command::new(env!("CARGO_BIN_EXE_pane"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"hello, pane\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();

    assert!(output.status.success());
    assert_eq!(output.stdout, b"hello, pane\n");
}

#[test]
fn nothing_the_model_returns_is_executed() {
    let root = scratch_dir("no-execute-root");
    let rollout = root.join("rollout.jsonl");
    let sentinel = root.join("sentinel-should-not-exist");
    let malicious = format!("```sh\ntouch {}\n```", sentinel.display());
    let (base_url, _bodies) =
        start_fake_provider(vec![assistant_reply(&malicious), ending_reply()]);

    let output = run_session(
        &root,
        &rollout,
        "sess-no-execute",
        "please help",
        &base_url,
        None,
    );

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !sentinel.exists(),
        "the model's text must never be executed: {} was created",
        sentinel.display()
    );
}

/// Map line 2446 reaching the **binary**, which is the only place it counts.
///
/// `glasshouse::{search_memory, checkpoint}` were built and tested by
/// `GH-PANE-61C-SEAMS` and then called by nothing: `commands` was scoped to
/// decide what a command *is*, and no package was given the acting half. The
/// reachability scan over `crates/pane/src` found them with zero production
/// call site outside their own file, which is the same shape that made the
/// whole of 61C need correcting.
///
/// This drives the built binary with `/memory` against a fake `glasshouse`
/// that answers one MCP `tools/call`, and asserts the answer reaches stdout.
#[cfg(unix)]
#[test]
fn the_binary_reads_memory_through_the_mcp_surface() {
    let root = scratch_dir("memory-root");
    let rollout = root.join("rollout.jsonl");

    // A fake `glasshouse mcp serve`: one JSON-RPC result line per request
    // line it reads, carrying a note only a reachable surface could produce.
    // Answers according to which tool was asked for, so the two readers are
    // distinguishable. A fake that answered both the same way let a mutation
    // deleting the `search_memory` call SURVIVE: `checkpoint` alone satisfied
    // an assertion that the surface had been reached.
    let glasshouse = write_script(
        &root,
        "fake_glasshouse_mcp.sh",
        "#!/bin/sh\nwhile IFS= read -r line; do\n  case \"$line\" in\n    *glasshouse_search_memory*) text=MEMORY-REACHED ;;\n    *glasshouse_get_checkpoint*) text=CHECKPOINT-REACHED ;;\n    *) text=UNKNOWN-TOOL ;;\n  esac\n  printf '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"%s\"}]}}\\n' \"$text\"\ndone\n",
    );

    let output = run_session(
        &root,
        &rollout,
        "memory-session",
        "/memory",
        &refused_base_url(),
        Some(&glasshouse),
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("MEMORY-REACHED"),
        "search_memory never reached the MCP surface:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("CHECKPOINT-REACHED"),
        "checkpoint never reached the MCP surface:\n{stdout}\nstderr:\n{stderr}"
    );
}

/// The other half of 2446, and the replacement for
/// `the_binary_falls_back_to_the_local_store_when_glasshouse_is_absent`,
/// whose assertion (`stdout.contains("/memory")`) was satisfied by
/// `"/memory: no notes"` -- the string reporting that the fallback found
/// nothing. This writes a note through one invocation's `/memory <text>` and
/// asserts a second invocation, with Glasshouse still absent both times,
/// reads that exact note back: a message saying nothing was found cannot
/// satisfy this, only the note's own text can.
#[test]
fn a_note_written_through_the_binary_is_read_back_by_a_later_run() {
    let root = scratch_dir("memory-roundtrip-root");
    let rollout = root.join("rollout.jsonl");
    let absent = root.join("no-such-glasshouse");

    let write = run_session(
        &root,
        &rollout,
        "memory-write",
        "/memory PANE-WROTE-THIS-NOTE",
        &refused_base_url(),
        Some(&absent),
    );
    assert!(
        write.status.success(),
        "an absent glasshouse must not fail the session; stderr:\n{}",
        String::from_utf8_lossy(&write.stderr)
    );

    let read = run_session(
        &root,
        &rollout,
        "memory-read",
        "/memory",
        &refused_base_url(),
        Some(&absent),
    );
    assert!(
        read.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&read.stderr)
    );
    let stdout = String::from_utf8_lossy(&read.stdout);
    assert!(
        stdout.contains("PANE-WROTE-THIS-NOTE"),
        "a note written through the binary was not read back by a later run:\n{stdout}"
    );
}

/// 2449: the shipped binary must show something, not build a `TestBackend`
/// and drop it. `run_session` always captures stdout as a pipe, which is
/// exactly the non-tty path every real pipe takes, so a regression back to
/// the dropped `TestBackend` fails this the same way it would fail a user
/// piping the binary's output anywhere.
#[test]
fn a_piped_session_prints_the_models_reply() {
    let root = scratch_dir("print-reply-root");
    let rollout = root.join("rollout.jsonl");
    let absent = root.join("no-such-glasshouse");
    let (base_url, _bodies) = start_fake_provider(vec![
        assistant_reply("PANE-PRINTED-REPLY-MARKER"),
        ending_reply(),
    ]);

    let output = run_session(
        &root,
        &rollout,
        "sess-print-reply",
        "hello",
        &base_url,
        Some(&absent),
    );

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.is_empty(),
        "a piped session must print something to stdout"
    );
    assert!(
        stdout.contains("PANE-PRINTED-REPLY-MARKER"),
        "the assistant's reply never reached stdout:\n{stdout}"
    );
}

/// 2449's other clause: the sidebar's content reaches stdout too, including
/// its honest collapse when Glasshouse is absent. Before this package,
/// stdout was zero bytes and this assertion could not have passed on any
/// string; it is not satisfied by an empty capture, only by the sidebar's
/// own collapsed text actually being printed.
#[test]
fn a_piped_session_prints_the_sidebar_content() {
    let root = scratch_dir("print-sidebar-root");
    let rollout = root.join("rollout.jsonl");
    let absent = root.join("no-such-glasshouse");
    let (base_url, _bodies) = start_fake_provider(vec![assistant_reply("ack"), ending_reply()]);

    let output = run_session(
        &root,
        &rollout,
        "sess-print-sidebar",
        "hello",
        &base_url,
        Some(&absent),
    );

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Glasshouse not connected"),
        "the sidebar's collapsed content never reached stdout:\n{stdout}"
    );
}

/// 2450: `commands::all` decided the full list from the day it was written;
/// this is the first test asserting the binary actually offers it, with a
/// project command and a project skill both present so neither source is
/// standing in for the other.
#[test]
fn the_command_list_is_offered_by_the_binary() {
    let root = scratch_dir("command-list-root");
    fs::create_dir_all(root.join(".claude").join("commands")).unwrap();
    fs::write(
        root.join(".claude").join("commands").join("deploy.md"),
        "deploy the project",
    )
    .unwrap();
    fs::create_dir_all(root.join(".claude").join("skills").join("reviewer")).unwrap();

    let rollout = root.join("rollout.jsonl");
    let absent = root.join("no-such-glasshouse");

    let output = run_session(
        &root,
        &rollout,
        "sess-command-list",
        "/help",
        &refused_base_url(),
        Some(&absent),
    );

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("deploy"),
        "the project command never appeared in the offered list:\n{stdout}"
    );
    assert!(
        stdout.contains("reviewer"),
        "the project skill never appeared in the offered list:\n{stdout}"
    );
}

/// 2450's uncovered branch: `commands::resolve`'s `ProjectSkill` arm, which
/// no test in the crate exercised even though it is the branch the binary
/// itself uses for a bare `/<skill-name>`.
#[test]
fn a_project_skill_resolves_by_name() {
    let root = scratch_dir("skill-resolve-root");
    fs::create_dir_all(root.join(".claude").join("skills").join("reviewer")).unwrap();

    let rollout = root.join("rollout.jsonl");
    let absent = root.join("no-such-glasshouse");

    let output = run_session(
        &root,
        &rollout,
        "sess-skill-resolve",
        "/reviewer",
        &refused_base_url(),
        Some(&absent),
    );

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("project skill"),
        "resolving a bare project skill by name never reached the binary's ProjectSkill branch:\n{stdout}"
    );
}

/// **2462, and the package's whole point: the model acts by returning a
/// TypeScript program that calls tools by name on live objects.**
///
/// Two cells, `model-contract.md` §7's own worked turn with its paths adapted
/// to a fixture tree: cell 1 greps and reads, cell 2 computes over the array
/// the grep produced and returns. Nothing between them is a person -- the
/// binary sends the second turn itself.
///
/// The provider answers from the **request body** rather than from a list, so
/// cell 2 is only sent because the runtime's own result block reached the
/// model naming `hits`. A fixed list would pass with an empty result block.
///
/// **2465 is the marker assertion.** `harness.rs` line 5 is never in a
/// message: `adapter` is a live `File` handle whose preview is a path, a size
/// and its first two lines, and there is no code path that writes a payload
/// into the conversation.
///
/// Unix only, and the reason is the runtime's, not this file's: on Windows a
/// tool call refuses before spawning, so cell 1 would throw `PermissionDenied`
/// and `hits` would never bind. That refusal is correct and is the runtime's
/// own to test; what this test needs is a host where a program's tool call
/// actually runs.
#[cfg(unix)]
#[test]
fn a_scripted_two_cell_task_runs_through_the_binary_and_returns() {
    let root = scratch_dir("two-cell-root");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("tests")).unwrap();
    fs::write(
        root.join("src").join("lib.rs"),
        "use crate::IntegrationId;\npub struct IntegrationId;\n",
    )
    .unwrap();
    fs::write(
        root.join("src").join("harness.rs"),
        "// IntegrationId lives here\n// two\n// three\n// four\n// PAYLOAD-MARKER-NEVER-IN-A-MESSAGE\n",
    )
    .unwrap();
    fs::write(
        root.join("tests").join("it.rs"),
        "use pane::IntegrationId;\n",
    )
    .unwrap();

    // Outside the fixture tree on purpose: a rollout inside it would be one
    // more file the model's own `grep` reads, and the counts it returns would
    // then depend on the session's own record of asking for them.
    let rollout = scratch_dir("two-cell-rollout").join("rollout.jsonl");
    let absent = root.join("no-such-glasshouse");

    let cell_one = format!(
        "```pane\nconst hits = await grep({{ pattern: \"IntegrationId\", path: \"{root}\" }});\nconst adapter = await read({{ path: \"{root}/src/harness.rs\" }});\n```",
        root = root.display()
    );
    let cell_two = "```pane\nconst isTest = (m) => m.path.includes(\"/tests/\");\nconst inTests = hits.filter(isTest);\nconst prodFiles = new Set(hits.filter(m => !isTest(m)).map(m => m.path));\nreturn { total: hits.length, in_tests: inTests.length, prod_files: prodFiles.size };\n```";

    let (base_url, bodies) = start_answering_provider(2, move |body| {
        if body.contains("## Handles") && body.contains("hits") {
            assistant_reply(cell_two)
        } else {
            assistant_reply(&cell_one)
        }
    });

    let output = run_session(
        &root,
        &rollout,
        "sess-two-cell",
        "How many files name that type, and how many are tests?",
        &base_url,
        Some(&absent),
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let cells = cell_lines(&rollout);
    assert_eq!(cells.len(), 2, "two cells ran: {cells:?}");
    assert_eq!(cells[0]["cell"], 1);
    assert_eq!(cells[0]["outcome"], "yielded");
    assert_eq!(cells[1]["cell"], 2);
    assert_eq!(cells[1]["outcome"], "returned");

    let hits = cells[0]["handles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|handle| handle["name"] == "hits")
        .unwrap_or_else(|| panic!("the grep result never became a handle: {cells:?}"));
    assert_eq!(hits["provenance"]["tool"], "grep");
    let preview = hits["preview"].as_str().unwrap();
    assert!(
        preview.contains("n=4"),
        "the four matches must be countable through the handle: {preview}"
    );

    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2, "the binary sent the second turn itself");
    let result_block = last_user_text(&bodies[1]);
    assert!(
        result_block.starts_with("[cell 1 yielded in"),
        "the second turn opened with the first cell's result: {result_block}"
    );
    assert!(
        !bodies[1].contains("PAYLOAD-MARKER-NEVER-IN-A-MESSAGE"),
        "a handle's payload reached the conversation: {}",
        bodies[1]
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    for key in ["\"total\"", "\"in_tests\"", "\"prod_files\""] {
        assert!(
            stdout.contains(key),
            "the returned value's preview must name {key}:\n{stdout}"
        );
    }
}

/// §5: a message with no `pane` block is prose. The task does not advance,
/// **the cell counter does not move**, and the answer is the unchanged handle
/// table and one line.
///
/// The prose here contains a ```` ```ts ```` block, which is the case §5 names
/// outright: a model writing about TypeScript emits those constantly, and a
/// parser that ran them would run the model's explanations.
#[test]
fn a_prose_reply_advances_no_cell_and_is_answered_with_the_table() {
    let root = scratch_dir("prose-root");
    let rollout = root.join("rollout.jsonl");
    let absent = root.join("no-such-glasshouse");

    let (base_url, bodies) = start_fake_provider(vec![
        assistant_reply("Here is how I would do it:\n\n```ts\nconst x = 1;\n```\n\nShall I?"),
        ending_reply(),
    ]);

    let output = run_session(
        &root,
        &rollout,
        "sess-prose",
        "count them",
        &base_url,
        Some(&absent),
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let cells = cell_lines(&rollout);
    assert_eq!(
        cells.len(),
        1,
        "only the second reply ran a cell: {cells:?}"
    );
    assert_eq!(
        cells[0]["cell"], 1,
        "the prose turn must not have consumed cell 1: {cells:?}"
    );

    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2);
    assert_eq!(
        last_user_text(&bodies[1]),
        "## Handles\n(none)\n\nno program ran; send one pane block",
        "prose is answered with the unchanged table and one line"
    );
}

/// §5: two `pane` blocks in one message are a protocol error and **neither
/// runs** -- running the first is the silently-wrong reading, because the
/// second is usually the one the model meant.
///
/// The third cell asks the isolate itself: `typeof a` is `"undefined"` only if
/// the first block never ran. A loop that ran the first block would bind `a`
/// on the persistent scope and this would return `"number"`.
#[test]
fn two_pane_blocks_run_neither() {
    let root = scratch_dir("two-blocks-root");
    let rollout = root.join("rollout.jsonl");
    let absent = root.join("no-such-glasshouse");

    let (base_url, bodies) = start_fake_provider(vec![
        assistant_reply("```pane\nconst a = 1;\n```\n\n```pane\nconst b = 2;\n```"),
        assistant_reply("```pane\nreturn typeof a;\n```"),
    ]);

    let output = run_session(
        &root,
        &rollout,
        "sess-two-blocks",
        "do the thing",
        &base_url,
        Some(&absent),
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let cells = cell_lines(&rollout);
    assert_eq!(
        cells.len(),
        1,
        "neither block ran: only the third message's cell is recorded: {cells:?}"
    );

    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2);
    assert_eq!(
        last_user_text(&bodies[1]),
        "two pane blocks in one turn; send one",
        "the answer is the contract's own sentence and carries no handle table"
    );

    // The returned string is the terminal response, verbatim (§9.2): the
    // screen carries `undefined` as the assistant's reply, unquoted.
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(" undefined"),
        "the first block's binding must not exist in the isolate:\n{stdout}"
    );
    assert!(
        !stdout.contains(" number"),
        "the first block ran and bound `a`:\n{stdout}"
    );
}

/// §5: a throw is a result. It fills the turn slot a yield would have used,
/// carries the class, the message and the position inside the model's own
/// program, and **the turn is not retried** -- the session sends the next one
/// and the task keeps going.
#[test]
fn a_cell_that_throws_is_answered_and_the_session_continues() {
    let root = scratch_dir("throw-root");
    let rollout = root.join("rollout.jsonl");
    let absent = root.join("no-such-glasshouse");

    let (base_url, bodies) = start_fake_provider(vec![
        assistant_reply("```pane\nconst before = 1;\nthrow new ReferenceError(\"fixture\");\n```"),
        ending_reply(),
    ]);

    let output = run_session(
        &root,
        &rollout,
        "sess-throw",
        "do the thing",
        &base_url,
        Some(&absent),
    );
    assert!(
        output.status.success(),
        "a throw must not fail the session; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let cells = cell_lines(&rollout);
    assert_eq!(cells.len(), 2, "{cells:?}");
    assert_eq!(cells[0]["outcome"], "threw");
    assert_eq!(cells[1]["outcome"], "returned");

    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2, "the session continued after the throw");
    let answer = last_user_text(&bodies[1]);
    assert!(
        answer.starts_with("[cell 1 threw in"),
        "the throw fills the turn slot a yield would have used: {answer}"
    );
    assert!(
        answer.contains("## Error\nReferenceError:"),
        "the error section carries the class and the message: {answer}"
    );
    assert!(
        answer.contains("before"),
        "the binding made before the throw must still be in the table: {answer}"
    );
    assert_eq!(
        bodies[1].matches("throw new ReferenceError").count(),
        1,
        "the turn is never retried: the throwing program appears once, as the \
         assistant message that sent it"
    );

    // §9.1: a cell that threw did not return, so no assistant `turn` line
    // follows its cell line -- the runtime's own answer does.
    let lines = rollout_lines(&rollout);
    let threw_at = lines
        .iter()
        .position(|line| line["kind"] == "cell" && line["outcome"] == "threw")
        .unwrap();
    assert_eq!(lines[threw_at + 1]["kind"], "turn", "{lines:?}");
    assert_eq!(lines[threw_at + 1]["role"], "user", "{lines:?}");
}

/// REQUIRED BEHAVIOR 6, and `model-contract.md` §1: the system block the
/// binary sends is `prompt::render_system`'s bytes for the same inputs -- the
/// preamble, one declaration per registered tool, then the project's own
/// instructions.
///
/// **Byte equality, not `contains`.** The system block is what the provider's
/// prompt cache holds for the whole task; a second spelling of it here that
/// merely carried the same words would break the cache and would make §8's
/// gateway comparison a comparison of two prompts.
#[test]
fn the_system_block_is_render_systems_own_bytes() {
    let root = scratch_dir("system-bytes-root");
    fs::write(root.join("CLAUDE.md"), "PROJECT-INSTRUCTION-ONE").unwrap();
    let rollout = root.join("rollout.jsonl");
    let absent = root.join("no-such-glasshouse");
    let (base_url, bodies) = start_fake_provider(vec![ending_reply()]);

    let output = run_session(
        &root,
        &rollout,
        "sess-system-bytes",
        "hi",
        &base_url,
        Some(&absent),
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // The facts are built from a profile compiled exactly as the binary
    // compiles it, through `session_facts` itself: a second spelling here
    // would be the very drift this test exists to catch.
    let profile = pane::sandbox::profile::Profile::compile(&root, None);
    let expected = pane::prompt::render_system(
        "PROJECT-INSTRUCTION-ONE",
        &pane::tools::registry::ALL.iter().collect::<Vec<_>>(),
        &pane::session::session_facts(&profile),
    );

    let bodies = bodies.lock().unwrap();
    let request: serde_json::Value = serde_json::from_str(&bodies[0]).unwrap();
    let expected = pane::prompt::with_task_context(
        &pane::contract::Conversation {
            system: expected,
            messages: Vec::new(),
        },
        pane::wire::MODEL,
        "hi",
    );
    assert_eq!(request["system"].as_str().unwrap(), expected.system);
}

/// §6's cell cap, and the one sentence that replaces the preamble when a
/// budget is spent.
///
/// **The loop ends after that turn whatever the model does**, so the cap is
/// asserted by the provider running out of scripted turns: a loop that kept
/// going would open a forty-second connection to a listener that has already
/// exited, and the run would fail rather than succeed.
///
/// The final turn's program still runs. It has to: `exhausted_preamble` says
/// the only permitted action is a top-level `return`, and a return is a
/// program -- refusing to run it would make the sentence unfollowable.
#[test]
fn the_cell_cap_replaces_the_preamble_and_ends_the_task_after_one_more_turn() {
    let root = scratch_dir("cell-cap-root");
    let rollout = root.join("rollout.jsonl");
    let absent = root.join("no-such-glasshouse");

    // 40 is `CELL_CAP`; the forty-first turn is the one the spent budget buys.
    let turns = 41;
    let replies = (0..turns)
        .map(|_| assistant_reply("```pane\nconst x = 1;\n```"))
        .collect();
    let (base_url, bodies) = start_fake_provider(replies);

    let output = run_session(
        &root,
        &rollout,
        "sess-cell-cap",
        "keep going",
        &base_url,
        Some(&absent),
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let bodies = bodies.lock().unwrap();
    assert_eq!(
        bodies.len(),
        turns,
        "the task must stop one turn after the cap, not run on"
    );
    assert!(
        !last_user_text(&bodies[turns - 2]).starts_with("The task budget is exhausted"),
        "the turn before the cap carries the ordinary result block"
    );
    assert!(
        last_user_text(&bodies[turns - 1]).starts_with("The task budget is exhausted"),
        "the turn a spent budget buys opens with the one sentence that replaces \
         the preamble: {}",
        last_user_text(&bodies[turns - 1])
    );
}

/// A fake `glasshouse` whose `routing-cost --json` answers with one
/// observation row, and which is silent for every other subcommand.
/// `once_only` makes it answer the **first** call and nothing after it.
#[cfg(unix)]
fn write_routing_cost(dir: &Path, name: &str, once_only: bool) -> PathBuf {
    let state = dir.join(format!("{name}.seen"));
    let row = r#"{"provider":"anthropic","model":"claude-sonnet-5","quota_context":"pro-plan","input_tokens":100,"output_tokens":20}"#;
    let guard = if once_only {
        format!(
            "[ -f {state} ] && exit 0\ntouch {state}\n",
            state = state.display()
        )
    } else {
        String::new()
    };
    let body = format!(
        "#!/bin/sh\ncase \"$1\" in\n  routing-cost)\n{guard}    echo '{row}'\n    ;;\nesac\nexit 0\n"
    );
    write_script(dir, name, &body)
}

/// §6: the task's token figure is the gateway's own usage row when there is
/// one -- "read from the gateway's own usage row rather than estimated".
///
/// 120 is `100 + 20`, the row's own two figures. The estimate for this
/// conversation is several hundred tokens, so a budget line reading `task
/// 120/400,000` cannot have been produced by the fallback.
#[cfg(unix)]
#[test]
fn a_gateway_reported_turn_is_counted_from_the_usage_row_not_estimated() {
    let root = scratch_dir("budget-gateway-root");
    let rollout = root.join("rollout.jsonl");
    let glasshouse = write_routing_cost(&root, "fake_routing_cost.sh", false);

    let (base_url, bodies) = start_fake_provider(vec![
        assistant_reply("```pane\nconst x = 1;\n```"),
        ending_reply(),
    ]);

    let output = run_session(
        &root,
        &rollout,
        "sess-budget-gateway",
        "count them",
        &base_url,
        Some(&glasshouse),
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let bodies = bodies.lock().unwrap();
    let result_block = last_user_text(&bodies[1]);
    assert!(
        result_block.contains("turn cap 8,192 · task 120/400,000 · cells 1/40"),
        "the budget line must carry the gateway's own figures: {result_block}"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("budget: 240/400000 tok"),
        "two reported turns total 240 in the sidebar:\n{stdout}"
    );
    assert!(
        stdout.contains("counted: reported"),
        "the sidebar must say the figure was reported, not estimated:\n{stdout}"
    );
}

/// The other half of §6's rule, and the reason the sidebar has a provenance
/// line at all: **a total that mixes a measurement with a heuristic says so.**
///
/// This gateway meters the first turn and not the second, so the total is one
/// reported figure plus one estimate. Labelling that `gateway-reported` would
/// be the honesty failure 2449 forbids -- a number that looks measured and is
/// not -- and labelling it `estimated` would understate a figure that is
/// partly real.
#[cfg(unix)]
#[test]
fn a_turn_the_gateway_never_metered_is_labelled_rather_than_averaged() {
    let root = scratch_dir("budget-mixed-root");
    let rollout = root.join("rollout.jsonl");
    let glasshouse = write_routing_cost(&root, "fake_routing_cost_once.sh", true);

    let (base_url, _bodies) = start_fake_provider(vec![
        assistant_reply("```pane\nconst x = 1;\n```"),
        ending_reply(),
    ]);

    let output = run_session(
        &root,
        &rollout,
        "sess-budget-mixed",
        "count them",
        &base_url,
        Some(&glasshouse),
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("counted: part estimated"),
        "a total built from both sources must say so:\n{stdout}"
    );
}

// --- runtime-contract.md §9: ending a task from inside the program -------

/// §9.2 through the binary: a program ending `return "…"` -- the rollout's
/// last two lines are the cell line and an assistant `turn` line carrying
/// the string verbatim, the reply is on the screen as the assistant's turn,
/// and the provider saw exactly as many requests as there were programs. A
/// third reply is scripted so that a request sent after the return would be
/// served and counted rather than fail on the connection.
#[test]
fn a_returned_string_is_the_assistants_turn_and_no_request_follows() {
    let root = scratch_dir("terminal-string-root");
    let rollout = root.join("rollout.jsonl");
    let absent = root.join("no-such-glasshouse");
    let answer = "Three files name it; two are tests.\nThe third is src/lib.rs.";

    let (base_url, bodies) = start_fake_provider(vec![
        assistant_reply("```pane\nconst n = 3;\n```"),
        assistant_reply(&format!(
            "```pane\nreturn {};\n```",
            serde_json::to_string(answer).unwrap()
        )),
        assistant_reply("```pane\nreturn \"NEVER REQUESTED\";\n```"),
    ]);

    let output = run_session(
        &root,
        &rollout,
        "sess-terminal-string",
        "count them",
        &base_url,
        Some(&absent),
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let lines = rollout_lines(&rollout);
    let n = lines.len();
    assert_eq!(lines[n - 2]["kind"], "cell", "{lines:?}");
    assert_eq!(lines[n - 2]["outcome"], "returned", "{lines:?}");
    assert_eq!(lines[n - 1]["kind"], "turn", "{lines:?}");
    assert_eq!(lines[n - 1]["role"], "assistant", "{lines:?}");
    assert_eq!(
        lines[n - 1]["text"],
        answer,
        "the response is kept verbatim"
    );

    let bodies = bodies.lock().unwrap();
    assert_eq!(
        bodies.len(),
        2,
        "as many requests as programs, and none after the return"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(" Three files name it; two are tests."),
        "the reply must be on the screen as the assistant's turn:\n{stdout}"
    );
}

/// §9.1: a program whose only call throws, and which then returns a
/// sentence anyway, is answered with the throw. `Threw` never ends the task:
/// no assistant `turn` line follows the throw's cell line, the runtime's
/// answer does, and the session sends the next turn.
#[test]
fn a_throw_never_becomes_a_terminal_response() {
    let root = scratch_dir("throw-terminal-root");
    let rollout = root.join("rollout.jsonl");
    let absent = root.join("no-such-glasshouse");

    let (base_url, bodies) = start_fake_provider(vec![
        assistant_reply(
            "```pane\nconst before = 1;\nthrow new ReferenceError(\"fixture\");\nreturn \"CONFIDENT SENTENCE\";\n```",
        ),
        ending_reply(),
    ]);

    let output = run_session(
        &root,
        &rollout,
        "sess-throw-terminal",
        "do the thing",
        &base_url,
        Some(&absent),
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let lines = rollout_lines(&rollout);
    let threw_at = lines
        .iter()
        .position(|line| line["kind"] == "cell" && line["outcome"] == "threw")
        .unwrap_or_else(|| panic!("the throw's cell line is missing: {lines:?}"));
    assert_eq!(lines[threw_at + 1]["kind"], "turn", "{lines:?}");
    assert_eq!(
        lines[threw_at + 1]["role"],
        "user",
        "the line after a throw is the runtime's answer, never an assistant turn: {lines:?}"
    );
    assert!(
        !lines.iter().any(|line| {
            line["kind"] == "turn"
                && line["role"] == "assistant"
                && line["text"] == "CONFIDENT SENTENCE"
        }),
        "the sentence after a throw became a terminal response: {lines:?}"
    );

    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2, "the session continued after the throw");
    let answer = last_user_text(&bodies[1]);
    assert!(answer.starts_with("[cell 1 threw in"), "{answer}");
    assert!(!answer.contains("CONFIDENT SENTENCE"), "{answer}");
}

/// Addendum 3: a non-string result is rendered as its JSON **with values**
/// -- a person reading `{matches: 3, files: 2}` needs the 3 and the 2 -- and
/// one over 2 KiB is cut on a character boundary, says so, and is followed
/// by the type-only preview of the whole value.
#[test]
fn a_returned_object_is_rendered_as_its_json_with_values() {
    let root = scratch_dir("terminal-json-root");
    let rollout = root.join("rollout.jsonl");
    let absent = root.join("no-such-glasshouse");
    let (base_url, _bodies) = start_fake_provider(vec![assistant_reply(
        "```pane\nreturn { matches: 3, files: 2, names: [\"a.rs\", \"b.rs\"] };\n```",
    )]);

    let output = run_session(
        &root,
        &rollout,
        "sess-terminal-json",
        "count them",
        &base_url,
        Some(&absent),
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let lines = rollout_lines(&rollout);
    let last = lines.last().unwrap();
    assert_eq!(last["role"], "assistant", "{lines:?}");
    assert_eq!(
        last["text"],
        r#"{"matches":3,"files":2,"names":["a.rs","b.rs"]}"#
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(r#" {"matches":3,"files":2"#),
        "the values, not their types, reach the screen:\n{stdout}"
    );

    // Over the render cap: `{"b":"` is six bytes and every `€` three, so a
    // cut at 2,048 falls inside a character and must back off to 2,046.
    let root = scratch_dir("terminal-json-cut-root");
    let rollout = root.join("rollout.jsonl");
    let (base_url, _bodies) = start_fake_provider(vec![assistant_reply(
        "```pane\nreturn { b: \"€\".repeat(3000), n: 1 };\n```",
    )]);
    let output = run_session(
        &root,
        &rollout,
        "sess-terminal-json-cut",
        "count them",
        &base_url,
        Some(&absent),
    );
    assert!(output.status.success());
    let lines = rollout_lines(&rollout);
    let text = lines.last().unwrap()["text"].as_str().unwrap().to_string();
    let (json, rest) = text
        .split_once('\n')
        .expect("a cut result says so on the next line");
    assert_eq!(
        json.len(),
        2046,
        "cut at 2 KiB on a character boundary: {json:?}"
    );
    assert!(json.starts_with(r#"{"b":"€"#), "{json}");
    assert!(json.ends_with('€'), "{json}");
    assert!(rest.starts_with("…(cut at 2,048 bytes"), "{rest}");
    assert!(
        rest.contains("\"b\": string"),
        "the type-only preview follows: {rest}"
    );
    assert!(rest.contains("\"n\": number"), "{rest}");
}

/// Addendum 2: the third consecutive prose turn carries the exhausted
/// preamble naming the reason, the task ends after one more turn whatever
/// the model does, and the second prose turn ends nothing. A fifth reply is
/// scripted so a loop that ran on would be served and counted. A program in
/// between resets the count.
#[test]
fn three_prose_turns_end_the_task_and_two_do_not() {
    let prose = || assistant_reply("I would grep for it, then count the files.");

    let root = scratch_dir("prose-cap-root");
    let rollout = root.join("rollout.jsonl");
    let absent = root.join("no-such-glasshouse");
    let (base_url, bodies) = start_fake_provider(vec![prose(), prose(), prose(), prose(), prose()]);
    let output = run_session(
        &root,
        &rollout,
        "sess-prose-cap",
        "count them",
        &base_url,
        Some(&absent),
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bodies = bodies.lock().unwrap();
    assert_eq!(
        bodies.len(),
        4,
        "the third prose turn buys exactly one more turn"
    );
    assert!(
        !last_user_text(&bodies[2]).contains("Three turns without a program"),
        "the second prose turn ends nothing: {}",
        last_user_text(&bodies[2])
    );
    assert!(
        last_user_text(&bodies[3]).starts_with("Three turns without a program;"),
        "the third carries the exhausted preamble naming the reason: {}",
        last_user_text(&bodies[3])
    );
    drop(bodies);

    let root = scratch_dir("prose-reset-root");
    let rollout = root.join("rollout.jsonl");
    let (base_url, bodies) = start_fake_provider(vec![
        prose(),
        prose(),
        assistant_reply("```pane\nconst x = 1;\n```"),
        prose(),
        prose(),
        ending_reply(),
    ]);
    let output = run_session(
        &root,
        &rollout,
        "sess-prose-reset",
        "count them",
        &base_url,
        Some(&absent),
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 6, "a program resets the count");
    for body in bodies.iter() {
        assert!(
            !last_user_text(body).contains("Three turns without a program"),
            "{}",
            last_user_text(body)
        );
    }
}

/// Addendum 1: the budget line's turn cap is the `max_tokens` the request
/// actually carries -- one constant, read from the wire -- so the model is
/// told the figure that binds it.
#[test]
fn the_budget_line_names_the_max_tokens_actually_sent() {
    let root = scratch_dir("turn-cap-root");
    let rollout = root.join("rollout.jsonl");
    let absent = root.join("no-such-glasshouse");
    let (base_url, bodies) = start_fake_provider(vec![
        assistant_reply("```pane\nconst x = 1;\n```"),
        ending_reply(),
    ]);
    let output = run_session(
        &root,
        &rollout,
        "sess-turn-cap",
        "count them",
        &base_url,
        Some(&absent),
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bodies = bodies.lock().unwrap();
    let request: serde_json::Value = serde_json::from_str(&bodies[1]).unwrap();
    let sent = request["max_tokens"].as_u64().unwrap();
    assert_eq!(sent, u64::from(pane::wire::MAX_TOKENS));
    assert_eq!(sent, 8_192);
    let result_block = last_user_text(&bodies[1]);
    assert!(
        result_block.contains("turn cap 8,192 ·"),
        "the budget line names the figure actually sent: {result_block}"
    );
}

// --- runtime-contract.md §6 addendum: a direct provider's own `usage` -----

/// §6, direct-provider path: with no gateway data at all, a Messages
/// response's own `usage` object is counted as reported, not estimated.
///
/// 30 is `20 + 10`, one reply's own two figures; 60 is both replies'. The
/// estimate for this conversation is a different figure entirely (several
/// hundred tokens, as the gateway test's own comment notes), so a budget
/// line reading `task 30/400,000` cannot have come from the fallback.
#[cfg(unix)]
#[test]
fn a_direct_providers_usage_is_counted_as_reported_not_estimated() {
    let root = scratch_dir("budget-direct-root");
    let rollout = root.join("rollout.jsonl");
    let absent = root.join("no-such-glasshouse");

    let (base_url, bodies) = start_fake_provider(vec![
        assistant_reply_with_usage("```pane\nconst x = 1;\n```", 20, 10),
        ending_reply_with_usage(20, 10),
    ]);

    let output = run_session(
        &root,
        &rollout,
        "sess-budget-direct",
        "count them",
        &base_url,
        Some(&absent),
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let bodies = bodies.lock().unwrap();
    let result_block = last_user_text(&bodies[1]);
    assert!(
        result_block.contains("turn cap 8,192 · task 30/400,000 · cells 1/40"),
        "the budget line must carry the response's own usage: {result_block}"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("budget: 60/400000 tok"),
        "two reported turns total 60 in the sidebar:\n{stdout}"
    );
    assert!(
        stdout.contains("counted: reported"),
        "the sidebar must say the figure was reported, not estimated, when a \
         direct provider's own usage is all there is:\n{stdout}"
    );
}

/// The precedence half of the §6 addendum, which neither gateway test above
/// can see because their replies carry no `usage`: when the gateway's own row
/// and the response's `usage` both report a turn, the gateway's figures are
/// the ones counted. The row says 100 + 20 per turn; the replies say 20 + 10.
/// Written by the lead at integration, because a mutation preferring the
/// response's `usage` would otherwise survive every test in this file.
#[cfg(unix)]
#[test]
fn the_gateways_row_wins_over_the_responses_usage_when_both_report() {
    let root = scratch_dir("budget-gateway-over-usage-root");
    let rollout = root.join("rollout.jsonl");
    let glasshouse = write_routing_cost(&root, "fake_routing_cost.sh", false);

    let (base_url, bodies) = start_fake_provider(vec![
        assistant_reply_with_usage("```pane\nconst x = 1;\n```", 20, 10),
        ending_reply_with_usage(20, 10),
    ]);

    let output = run_session(
        &root,
        &rollout,
        "sess-budget-gateway-over-usage",
        "count them",
        &base_url,
        Some(&glasshouse),
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let bodies = bodies.lock().unwrap();
    let result_block = last_user_text(&bodies[1]);
    assert!(
        result_block.contains("turn cap 8,192 · task 120/400,000 · cells 1/40"),
        "the gateway's row (120) must win over the response's usage (30): {result_block}"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("budget: 240/400000 tok"),
        "two gateway-reported turns total 240, not the responses' 60:\n{stdout}"
    );
}

// --- docs/product/pane/supervisor.md: the supervisor's look --------------

/// The system prompt every look's request carries -- §3, verbatim's own
/// first sentence, distinctive enough that no ordinary task turn ever
/// contains it.
const SUPERVISOR_SYSTEM_MARKER: &str = "You watch a coding agent's trajectory";

fn is_supervisor_request(body: &str) -> bool {
    let request: serde_json::Value = serde_json::from_str(body).unwrap();
    request["system"]
        .as_str()
        .unwrap_or("")
        .contains(SUPERVISOR_SYSTEM_MARKER)
}

fn looping_cell_reply() -> String {
    assistant_reply("```pane\nconst x = 1;\n```")
}

fn write_supervisor_pane_toml(root: &Path, every: u32, extra: &str) {
    fs::create_dir_all(root.join(".glasshouse")).unwrap();
    fs::write(
        root.join(".glasshouse").join("pane.toml"),
        format!("[supervisor]\nevery = {every}\nmodel = \"claude-sonnet-5\"\n{extra}"),
    )
    .unwrap();
}

/// §5, the acceptance test itself: a scripted provider answers the same
/// program three turns running, `every = 3` batches exactly those three
/// cells into one look, and the scripted supervisor model says `intervene`
/// on the trajectory that shows the repeat -- the nudge heads the very next
/// user message, the turn after the third (and second-repeated) cell,
/// within two turns of it.
///
/// The mutation this test kills: the cadence off by one. A look fired after
/// two cells instead of three would see only the first repeat, and one fired
/// after four would miss the window this test asserts on.
#[test]
fn a_planted_three_turn_loop_is_nudged_within_two_turns() {
    let root = scratch_dir("supervisor-loop-root");
    write_supervisor_pane_toml(&root, 3, "");
    let rollout = root.join("rollout.jsonl");
    let absent = root.join("no-such-glasshouse");

    let task_count = Mutex::new(0usize);
    let (base_url, bodies) = start_answering_provider(5, move |body| {
        if is_supervisor_request(body) {
            return assistant_reply(
                r#"{"intervene": true, "reason": "the same program three times"}"#,
            );
        }
        let mut count = task_count.lock().unwrap();
        *count += 1;
        if *count <= 3 {
            looping_cell_reply()
        } else {
            ending_reply()
        }
    });

    let output = run_session(
        &root,
        &rollout,
        "sess-supervisor-loop",
        "keep going",
        &base_url,
        Some(&absent),
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 5, "3 task turns, 1 look, 1 final task turn");
    let task_bodies: Vec<&String> = bodies
        .iter()
        .filter(|body| !is_supervisor_request(body))
        .collect();
    assert_eq!(
        task_bodies.len(),
        4,
        "the look is not a task turn: {bodies:?}"
    );
    assert_eq!(
        bodies
            .iter()
            .filter(|body| is_supervisor_request(body))
            .count(),
        1,
        "exactly one look for the three planted cells: {bodies:?}"
    );

    let fourth_turn_answer = last_user_text(task_bodies[3]);
    assert!(
        fourth_turn_answer.starts_with("supervisor: the same program three times"),
        "the nudge must head the turn after the third cell: {fourth_turn_answer}"
    );

    let lines = rollout_lines(&rollout);
    assert!(
        lines.iter().any(|l| l["kind"] == "turn"
            && l["role"] == "user"
            && l["text"]
                .as_str()
                .unwrap_or("")
                .starts_with("supervisor: the same program three times")),
        "the nudge must be recorded as a user turn: {lines:?}"
    );

    // §5's other half, and the lead's second mutation: `enabled = false`
    // sends no supervisor request at all, however the cadence would
    // otherwise trigger.
    let root = scratch_dir("supervisor-off-root");
    write_supervisor_pane_toml(&root, 3, "enabled = false\n");
    let rollout = root.join("rollout.jsonl");
    let absent = root.join("no-such-glasshouse");

    let task_count = Mutex::new(0usize);
    let (base_url, bodies) = start_answering_provider(4, move |body| {
        assert!(
            !is_supervisor_request(body),
            "enabled = false must never send a supervisor request"
        );
        let mut count = task_count.lock().unwrap();
        *count += 1;
        if *count <= 3 {
            looping_cell_reply()
        } else {
            ending_reply()
        }
    });

    let output = run_session(
        &root,
        &rollout,
        "sess-supervisor-off",
        "keep going",
        &base_url,
        Some(&absent),
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let bodies = bodies.lock().unwrap();
    assert_eq!(
        bodies.len(),
        4,
        "no supervisor request was inserted: {bodies:?}"
    );
    for body in bodies.iter() {
        assert!(
            !last_user_text(body).starts_with("supervisor:"),
            "enabled = false must never nudge: {}",
            last_user_text(body)
        );
    }
}

/// REQUIRED BEHAVIOR 2: an unparseable look answer is not a nudge, and is
/// shown as such -- it folds into the ordinary "looked, no nudge" outcome
/// rather than silently becoming an intervention. The lead's mutation:
/// unparseable treated as `intervene`.
#[test]
fn an_unparseable_supervisor_answer_is_not_a_nudge() {
    let root = scratch_dir("supervisor-unparseable-root");
    write_supervisor_pane_toml(&root, 1, "");
    let rollout = root.join("rollout.jsonl");
    let absent = root.join("no-such-glasshouse");

    let task_count = Mutex::new(0usize);
    let (base_url, bodies) = start_answering_provider(3, move |body| {
        if is_supervisor_request(body) {
            return assistant_reply("not json at all");
        }
        let mut count = task_count.lock().unwrap();
        *count += 1;
        if *count == 1 {
            looping_cell_reply()
        } else {
            ending_reply()
        }
    });

    let output = run_session(
        &root,
        &rollout,
        "sess-supervisor-unparseable",
        "keep going",
        &base_url,
        Some(&absent),
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 3, "task turn, look, task turn: {bodies:?}");
    assert!(is_supervisor_request(&bodies[1]), "{bodies:?}");
    assert!(
        !last_user_text(&bodies[2]).starts_with("supervisor:"),
        "an unparseable answer must never become a nudge: {}",
        last_user_text(&bodies[2])
    );

    let cells = cell_lines(&rollout);
    assert_eq!(cells.len(), 2, "{cells:?}");
}

/// §3: the look's request carries `x-glasshouse-purpose: supervisor`, so the
/// ledger can tell it apart from a task turn before the gateway reads the
/// header itself.
#[test]
fn the_look_carries_the_purpose_header() {
    let root = scratch_dir("supervisor-header-root");
    write_supervisor_pane_toml(&root, 1, "");
    let rollout = root.join("rollout.jsonl");
    let absent = root.join("no-such-glasshouse");

    let task_count = Mutex::new(0usize);
    let (base_url, captured) = start_capturing_provider(3, move |body| {
        if is_supervisor_request(body) {
            return assistant_reply(r#"{"intervene": false, "reason": "fine"}"#);
        }
        let mut count = task_count.lock().unwrap();
        *count += 1;
        if *count == 1 {
            looping_cell_reply()
        } else {
            ending_reply()
        }
    });

    let output = run_session(
        &root,
        &rollout,
        "sess-supervisor-header",
        "count them",
        &base_url,
        Some(&absent),
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let captured = captured.lock().unwrap();
    let look = captured
        .iter()
        .find(|(_, body)| is_supervisor_request(body))
        .expect("the look's own request must have been sent");
    assert_eq!(
        look.0.get("x-glasshouse-purpose").map(String::as_str),
        Some("supervisor"),
        "the look must carry the purpose header: {:?}",
        look.0
    );
}

/// The addendum (lead, 07:12): the look must name `[supervisor] model` --
/// map line 2469's "with a cheaper model" clause, the one part of it this
/// package had left the task's own model standing in for. Every ordinary
/// task turn still carries `wire::MODEL`; only the look's own request names
/// the configured, deliberately distinct id.
#[test]
fn the_look_names_the_supervisors_model_and_the_turns_name_the_tasks() {
    let root = scratch_dir("supervisor-model-root");
    fs::create_dir_all(root.join(".glasshouse")).unwrap();
    fs::write(
        root.join(".glasshouse").join("pane.toml"),
        "[supervisor]\nevery = 1\nmodel = \"cheap-model-for-the-test\"\n",
    )
    .unwrap();
    let rollout = root.join("rollout.jsonl");
    let absent = root.join("no-such-glasshouse");

    let task_count = Mutex::new(0usize);
    let (base_url, bodies) = start_answering_provider(3, move |body| {
        if is_supervisor_request(body) {
            return assistant_reply(r#"{"intervene": false, "reason": "fine"}"#);
        }
        let mut count = task_count.lock().unwrap();
        *count += 1;
        if *count == 1 {
            looping_cell_reply()
        } else {
            ending_reply()
        }
    });

    let output = run_session(
        &root,
        &rollout,
        "sess-supervisor-model",
        "count them",
        &base_url,
        Some(&absent),
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 3, "task turn, look, task turn: {bodies:?}");

    for (index, body) in bodies.iter().enumerate() {
        let request: serde_json::Value = serde_json::from_str(body).unwrap();
        let model = request["model"].as_str().unwrap();
        if is_supervisor_request(body) {
            assert_eq!(
                model, "cheap-model-for-the-test",
                "the look must name the configured model: request {index}: {body}"
            );
        } else {
            assert_eq!(
                model,
                pane::wire::MODEL,
                "every task turn must still name the task's own model: request {index}: {body}"
            );
        }
    }
}

/// REQUIRED BEHAVIOR 4: the four limits actually bind the runtime and the
/// budget -- `cells` here, loaded from `pane.toml` rather than the built-in
/// default of 40.
#[test]
fn a_loaded_cell_limit_ends_the_task() {
    let root = scratch_dir("loaded-cell-limit-root");
    fs::create_dir_all(root.join(".glasshouse")).unwrap();
    fs::write(
        root.join(".glasshouse").join("pane.toml"),
        "[limits]\ncells = 2\n",
    )
    .unwrap();
    let rollout = root.join("rollout.jsonl");
    let absent = root.join("no-such-glasshouse");

    // Three scripted turns: the third is the one turn a spent budget buys.
    let turns = 3;
    let replies = (0..turns)
        .map(|_| assistant_reply("```pane\nconst x = 1;\n```"))
        .collect();
    let (base_url, bodies) = start_fake_provider(replies);

    let output = run_session(
        &root,
        &rollout,
        "sess-loaded-cell-limit",
        "keep going",
        &base_url,
        Some(&absent),
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let bodies = bodies.lock().unwrap();
    assert_eq!(
        bodies.len(),
        turns,
        "the task must stop one turn after the loaded cap, not run on"
    );
    assert!(
        last_user_text(&bodies[turns - 1]).starts_with("The task budget is exhausted"),
        "a `cells = 2` pane.toml must end the task after two cells: {}",
        last_user_text(&bodies[turns - 1])
    );
}

/// One captured request: its headers (lower-cased names) and its body.
type CapturedRequest = (std::collections::HashMap<String, String>, String);

/// The same minimal endpoint as `start_answering_provider`, but also records
/// each request's headers alongside its body -- only
/// `the_look_carries_the_purpose_header` above needs a header, and nothing
/// before this heading reads one, so this is an addition rather than a
/// change to the helper `pane-61e-usage` also builds on.
fn start_capturing_provider<F>(
    turns: usize,
    answer: F,
) -> (String, Arc<Mutex<Vec<CapturedRequest>>>)
where
    F: Fn(&str) -> String + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let captured = Arc::new(Mutex::new(Vec::new()));
    let captured_thread = Arc::clone(&captured);

    thread::spawn(move || {
        for _ in 0..turns {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut headers = std::collections::HashMap::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                if line == "\r\n" || line == "\n" {
                    break;
                }
                if let Some((name, value)) = line.trim_end().split_once(':') {
                    headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
                }
            }
            let content_length = headers
                .get("content-length")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0usize);
            let mut body = vec![0u8; content_length];
            if reader.read_exact(&mut body).is_err() {
                continue;
            }
            let body = String::from_utf8_lossy(&body).into_owned();
            let reply = answer(&body);
            captured_thread.lock().unwrap().push((headers, body));

            let response_body = reply.as_bytes();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                response_body.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.write_all(response_body);
            let _ = stream.flush();
        }
    });

    (format!("http://127.0.0.1:{port}"), captured)
}

// --- the keyboard's end of the cancellation facility (GH-PANE-SIGINT) ----

/// SIGINT during a session, and the three things it must do: cancel the tool
/// call in flight, leave a cell that is only computing alone, and end the
/// session on a second Ctrl-C without cutting a rollout line in half.
///
/// Unix only, because the signal is: the Windows half is
/// `SetConsoleCtrlHandler`, which cannot be raised from a test the way
/// `kill -INT` can. Every helper below is inside the module so nothing here
/// is dead code on the Windows cell.
#[cfg(unix)]
mod interrupts {
    use super::*;
    use std::process::{Child, Stdio};
    use std::time::{Duration, Instant};

    /// A command that never ends and writes a marker the moment it starts.
    ///
    /// **`bash` builtins only, on purpose.** The seatbelt names one resolved
    /// binary in `process-exec*` (the 61D exec-roots ruling), so a confined
    /// `bash` cannot exec `/bin/sleep` at all -- the same reason
    /// `runtime_cells.rs`'s own cancellation test spins rather than sleeps.
    /// The marker is what lets a test signal *after* the child exists rather
    /// than after a guessed delay.
    ///
    /// **`label` makes the command line itself unique**, so
    /// [`no_spinner_survives`] can look for one test's child in `ps` while the
    /// other tests in this binary are running their own beside it.
    fn spins_and_marks(label: &str) -> String {
        format!("while :; do echo go > marker-{label}; done")
    }

    fn marker_of(root: &Path, label: &str) -> PathBuf {
        root.join(format!("marker-{label}"))
    }

    /// A cell whose one statement is that call, so the cell's ending is the
    /// call's ending.
    fn spinning_cell(label: &str) -> String {
        assistant_reply(&format!(
            "```pane\nconst out = await bash({{ command: \"{}\" }});\nreturn out.stdout;\n```",
            spins_and_marks(label)
        ))
    }

    /// No confined child of this test is still running.
    ///
    /// `std::process::exit` does not touch a process's children, so the
    /// second-Ctrl-C path is the one place `pane` could reparent a spinning
    /// `bash` to `init` and leave it there. It did -- one child at 87% of a
    /// core, for ever -- until `Interrupter::end_the_session` learned to
    /// cancel and then hold the rollout's write lock across the reap grace.
    /// This is a regression test, not a tidiness check.
    ///
    /// **`ps -A -ww`, and the width flag is load-bearing**: macOS cuts
    /// `-o command` at the terminal width, and a confined `bash`'s command
    /// line carries an absolute interpreter path before the needle -- so a
    /// truncated listing reads exactly like "nothing survived". The row is
    /// printed with `pid` and `ppid` because an orphan's `ppid 1` is what
    /// names the defect.
    fn no_spinner_survives(label: &str) {
        let needle = spins_and_marks(label);
        let listing = Command::new("ps")
            .args(["-A", "-ww", "-o", "pid,ppid,stat,%cpu,command"])
            .output()
            .expect("ps runs");
        let listing = String::from_utf8_lossy(&listing.stdout);
        let survivors: Vec<&str> = listing
            .lines()
            .filter(|line| line.contains(&needle))
            .collect();
        assert!(
            survivors.is_empty(),
            "the exit left a confined child running: {survivors:?}"
        );
    }

    /// A cell that only computes: no call, no handle from a tool, and long
    /// enough that a signal sent when the turn was answered lands inside it.
    fn computing_cell() -> String {
        assistant_reply(
            "```pane\nlet n = 0;\nfor (let i = 0; i < 50000000; i++) { n = (n + i) % 1000003; \
             }\nconst spun = n;\n```",
        )
    }

    /// The grants [`spins_and_marks`] needs, and nothing else: three command
    /// prefixes, no path rule of any kind (`sandbox-grants.md` §2 -- argv
    /// admission grants no file access).
    fn grant_the_spin(root: &Path) {
        fs::create_dir_all(root.join(".claude")).unwrap();
        fs::write(
            root.join(".claude").join("settings.json"),
            // `trap` is here for the stubborn-job test below, which needs a
            // job that ignores every catchable signal; it grants no file
            // access either (`sandbox-grants.md` §2).
            r#"{"permissions":{"allow":["Bash(while*)","Bash(do*)","Bash(echo*)","Bash(trap*)"]}}"#,
        )
        .unwrap();
    }

    /// [`run_session`], but spawned rather than waited on, because these
    /// tests need the pid while it runs.
    ///
    /// stdout is discarded rather than piped: the session redraws the whole
    /// notebook every turn, and a pipe nobody drains while the test waits for
    /// a marker would fill and stop the very process being signalled.
    fn spawn_session(root: &Path, rollout: &Path, task: &str, base_url: &str) -> Child {
        Command::new(env!("CARGO_BIN_EXE_pane"))
            .arg("session")
            .arg("--root")
            .arg(root)
            .arg("--rollout")
            .arg(rollout)
            .arg("--session")
            .arg("sess-interrupt")
            .arg("--task")
            .arg(task)
            .env("ANTHROPIC_BASE_URL", base_url)
            .env_remove("ANTHROPIC_AUTH_TOKEN")
            .env_remove("ANTHROPIC_API_KEY")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("pane starts")
    }

    fn wait_for_marker(marker: &Path, child: &mut Child) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if marker.exists() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let _ = child.kill();
        let _ = child.wait();
        panic!(
            "the confined child never started: {} absent",
            marker.display()
        );
    }

    fn wait_for_turns(bodies: &Arc<Mutex<Vec<String>>>, count: usize, child: &mut Child) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if bodies.lock().unwrap().len() >= count {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let _ = child.kill();
        let _ = child.wait();
        panic!("the session never asked for turn {count}");
    }

    fn send_interrupt(child: &Child) {
        let status = Command::new("kill")
            .arg("-INT")
            .arg(child.id().to_string())
            .status()
            .expect("kill runs");
        assert!(status.success(), "kill -INT {} failed", child.id());
    }

    /// How one call of a cell's recorded trajectory ended -- `{"threw":
    /// "Cancelled"}` on the line (`runtime-contract.md` §9.4).
    fn call_endings(cell: &serde_json::Value) -> Vec<serde_json::Value> {
        cell["calls"]
            .as_array()
            .expect("every cell line carries a trajectory")
            .iter()
            .map(|call| call["ended"].clone())
            .collect()
    }

    fn outcomes(rollout: &Path) -> Vec<String> {
        cell_lines(rollout)
            .iter()
            .map(|cell| cell["outcome"].as_str().unwrap().to_string())
            .collect()
    }

    /// Ctrl-C with a call in flight: the call ends as §5's `Cancelled` throw,
    /// the cell is answered, the model is asked for another turn, and the
    /// session goes on to end normally -- against a child that would
    /// otherwise never exit.
    #[test]
    fn a_sigint_during_a_tool_call_cancels_it_and_the_session_continues() {
        let root = scratch_dir("sigint-call");
        grant_the_spin(&root);
        let rollout = root.join("rollout.jsonl");
        let marker = marker_of(&root, "call");
        let (base_url, bodies) = start_fake_provider(vec![
            spinning_cell("call"),
            assistant_reply("```pane\nreturn \"done\";\n```"),
        ]);

        let started = Instant::now();
        let mut child = spawn_session(&root, &rollout, "spin for me", &base_url);
        wait_for_marker(&marker, &mut child);
        send_interrupt(&child);
        let output = child.wait_with_output().unwrap();
        let elapsed = started.elapsed();

        assert!(
            output.status.success(),
            "status {:?}, stderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            elapsed < Duration::from_secs(25),
            "the session ran for {elapsed:?} against a child that never exits"
        );

        let cells = cell_lines(&rollout);
        assert_eq!(cells[0]["outcome"], "threw", "{cells:?}");
        assert_eq!(
            call_endings(&cells[0]),
            vec![serde_json::json!({"threw": "Cancelled"})],
            "{cells:?}"
        );

        // The second turn was requested, and what it carried is §5's error
        // section naming the class -- so the model was told the call was
        // cancelled rather than being asked to guess.
        let bodies = bodies.lock().unwrap();
        assert_eq!(bodies.len(), 2, "the session did not ask for another turn");
        let answer = last_user_text(&bodies[1]);
        assert!(answer.contains("## Error"), "{answer}");
        assert!(answer.contains("Cancelled"), "{answer}");
    }

    /// A second Ctrl-C inside two seconds ends the session with the status a
    /// shell reports for an interrupted process, and the rollout it leaves
    /// behind is whole: every line parses, including the last.
    #[test]
    fn a_second_sigint_within_two_seconds_ends_the_session_with_exit_130() {
        let root = scratch_dir("sigint-twice");
        grant_the_spin(&root);
        let rollout = root.join("rollout.jsonl");
        let marker = marker_of(&root, "twice");
        let (base_url, _bodies) = start_fake_provider(vec![
            spinning_cell("twice"),
            spinning_cell("twice"),
            spinning_cell("twice"),
        ]);

        let mut child = spawn_session(&root, &rollout, "spin twice", &base_url);
        wait_for_marker(&marker, &mut child);
        send_interrupt(&child);
        thread::sleep(Duration::from_millis(200));
        send_interrupt(&child);
        let output = child.wait_with_output().unwrap();

        assert_eq!(
            output.status.code(),
            Some(130),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("interrupted twice"),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        // The exit cancelled before it took the writing lock, so the call in
        // flight killed and reaped its own child on the way out.
        no_spinner_survives("twice");

        // `rollout_lines` parses every line and panics on one that does not,
        // so this is the whole-last-line assertion: an exit taken in the
        // middle of a write would leave a fragment here.
        let lines = rollout_lines(&rollout);
        assert!(
            lines.len() >= 2,
            "the session exited before it recorded anything: {lines:?}"
        );
    }

    /// Ctrl-C with no call in flight does not end the cell: JavaScript is
    /// stopped by the wall-clock watchdog and never by the interrupt, so a
    /// cell that is only computing runs to its own end. The interrupt is not
    /// lost either -- the **next** cell's call is what it cancels.
    #[test]
    fn a_sigint_with_no_call_in_flight_does_not_end_the_cell() {
        let root = scratch_dir("sigint-compute");
        grant_the_spin(&root);
        let rollout = root.join("rollout.jsonl");
        let (base_url, bodies) = start_fake_provider(vec![
            computing_cell(),
            spinning_cell("compute"),
            assistant_reply("```pane\nreturn \"done\";\n```"),
        ]);

        let mut child = spawn_session(&root, &rollout, "compute then spin", &base_url);
        // The first turn has been answered, so the computing cell is running
        // or about to.
        wait_for_turns(&bodies, 1, &mut child);
        send_interrupt(&child);
        let output = child.wait_with_output().unwrap();

        assert!(
            output.status.success(),
            "status {:?}, stderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let cells = cell_lines(&rollout);
        assert_eq!(
            outcomes(&rollout),
            vec!["yielded", "threw", "returned"],
            "the computing cell was ended by the signal: {cells:?}"
        );
        assert!(
            call_endings(&cells[0]).is_empty(),
            "the computing cell made a call: {cells:?}"
        );
        assert_eq!(
            call_endings(&cells[1]),
            vec![serde_json::json!({"threw": "Cancelled"})],
            "the interrupt was dropped instead of spent on the next call: {cells:?}"
        );
    }

    /// The other side of the window, and the reason "within two seconds" is a
    /// claim rather than a decoration: two Ctrl-Cs **far enough apart** are
    /// two first interrupts, each cancelling one call, and the session
    /// survives both to end normally.
    ///
    /// Without this, widening [`DOUBLE_INTERRUPT_WINDOW`] to any larger value
    /// changes no observable behaviour the test above watches -- it sends its
    /// pair 200 ms apart, which is inside every window a mutation would
    /// choose. The gap here is 3.5 s against a 2 s window, so the margin
    /// absorbs a loaded machine's scheduling without reaching the boundary.
    #[test]
    fn two_sigints_more_than_two_seconds_apart_do_not_end_the_session() {
        let root = scratch_dir("sigint-apart");
        grant_the_spin(&root);
        let rollout = root.join("rollout.jsonl");
        let marker = marker_of(&root, "apart");
        let (base_url, _bodies) = start_fake_provider(vec![
            spinning_cell("apart"),
            spinning_cell("apart"),
            assistant_reply("```pane\nreturn \"done\";\n```"),
        ]);

        let mut child = spawn_session(&root, &rollout, "spin, wait, spin", &base_url);
        wait_for_marker(&marker, &mut child);
        send_interrupt(&child);
        // The second cell is spinning by now, and stays so until this lands.
        thread::sleep(Duration::from_millis(3500));
        send_interrupt(&child);
        let output = child.wait_with_output().unwrap();

        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(
            output.status.success(),
            "status {:?}, stderr: {stderr}",
            output.status
        );
        assert!(
            !stderr.contains("interrupted twice"),
            "the second interrupt was paired with one 3.5 s older: {stderr}"
        );
        assert_eq!(
            outcomes(&rollout),
            vec!["threw", "threw", "returned"],
            "each interrupt must have cancelled one call of its own"
        );
    }

    // --- GH-PANE-BG-EXIT-AND-COST: what an exit must take with it ---------
    //
    // The three tests above watch the foreground child of a call in flight.
    // The three below watch the **background board**, which was added after
    // `end_the_session` was written and was never wired into its fix: §5's
    // "a background job outlives no session" has to be true of every exit
    // this binary can take, not only of the tidy ones.

    /// A cell that starts a background job and then **yields**.
    ///
    /// It must not `return`: a top-level return ends the task, and
    /// `run_task`'s own `bg::shutdown` would take the job with it before any
    /// signal arrived -- a different exit path, tested separately below.
    fn background_cell(label: &str) -> String {
        assistant_reply(&format!(
            "```pane\nbg.run(\"{}\");\nconst started = 1;\n```",
            spins_and_marks(label)
        ))
    }

    /// Waits, bounded, for a path to appear, and answers whether it did.
    /// Every wait in these tests is bounded: a job that never starts must
    /// fail a test rather than hang one.
    fn waits_for(path: &Path) -> bool {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if path.exists() {
                return true;
            }
            thread::sleep(Duration::from_millis(10));
        }
        path.exists()
    }

    /// **The second Ctrl-C takes the background jobs with it.**
    ///
    /// `std::process::exit` does not touch a process's children, so before
    /// `end_the_session` learned to shut the board down this left the job's
    /// `bash` on `ppid 1` spinning at 99% of a core after `pane` had exited
    /// 130 -- the same defect the foreground child's fix closed, in the same
    /// function, for the half of it that did not exist yet.
    ///
    /// The exit is timed as well as asserted: a shutdown that waited for a
    /// job that would not stop would be a worse defect than the orphan.
    #[test]
    fn a_second_sigint_takes_the_background_jobs_with_it() {
        let root = scratch_dir("sigint-bg");
        grant_the_spin(&root);
        let rollout = root.join("rollout.jsonl");
        let job = marker_of(&root, "bgjob");
        let foreground = marker_of(&root, "bgfg");
        let (base_url, _bodies) = start_fake_provider(vec![
            background_cell("bgjob"),
            spinning_cell("bgfg"),
            spinning_cell("bgfg"),
            spinning_cell("bgfg"),
        ]);

        let mut child = spawn_session(&root, &rollout, "start a job, then spin", &base_url);
        // Both processes exist before the first signal: the job's, started by
        // the first cell, and the call's, started by the second.
        wait_for_marker(&job, &mut child);
        wait_for_marker(&foreground, &mut child);
        send_interrupt(&child);
        thread::sleep(Duration::from_millis(200));
        send_interrupt(&child);
        let asked = Instant::now();
        let output = child.wait_with_output().unwrap();
        let exit_took = asked.elapsed();

        assert_eq!(
            output.status.code(),
            Some(130),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        no_spinner_survives("bgfg");
        no_spinner_survives("bgjob");
        assert!(
            exit_took < Duration::from_secs(5),
            "the second Ctrl-C took {exit_took:?} to end the session; a Ctrl-C that waits is not \
             a Ctrl-C"
        );
    }

    /// The tidy exit: a task's top-level `return` reaches `run_task`'s own
    /// `bg::shutdown`, and nothing it started is left running.
    ///
    /// The second turn is not answered until the job's own process exists,
    /// so the assertion cannot hold vacuously by racing the job's start.
    #[test]
    fn a_task_that_returns_takes_its_background_job_with_it() {
        let root = scratch_dir("bg-return");
        grant_the_spin(&root);
        let rollout = root.join("rollout.jsonl");
        let job = marker_of(&root, "bgret");
        let gate = job.clone();
        let replies = [background_cell("bgret"), ending_reply()];
        let next = Mutex::new(0usize);
        let (base_url, _bodies) = start_answering_provider(2, move |_body| {
            let mut index = next.lock().unwrap();
            if *index > 0 {
                assert!(waits_for(&gate), "the background job never started");
            }
            let reply = replies[*index].clone();
            *index += 1;
            reply
        });

        let output = run_session(
            &root,
            &rollout,
            "sess-bg-return",
            "start a job, then return",
            &base_url,
            None,
        );

        assert!(
            output.status.success(),
            "status {:?}, stderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(job.exists(), "the background job never started");
        no_spinner_survives("bgret");
    }

    /// The untidy one: a task that fails mid-flight leaves `run_task` by `?`
    /// without reaching its own shutdown, so `session::run`'s is what has to
    /// catch the job -- which is the promise that call was added for.
    ///
    /// The failure is a reply that is not a Messages response at all, gated
    /// on the job's marker so the job is running when the task dies.
    #[test]
    fn a_task_that_fails_mid_flight_takes_its_background_job_with_it() {
        let root = scratch_dir("bg-fail");
        grant_the_spin(&root);
        let rollout = root.join("rollout.jsonl");
        let job = marker_of(&root, "bgfail");
        let gate = job.clone();
        let replies = [
            background_cell("bgfail"),
            "this is not a Messages response".to_string(),
        ];
        let next = Mutex::new(0usize);
        let (base_url, _bodies) = start_answering_provider(2, move |_body| {
            let mut index = next.lock().unwrap();
            if *index > 0 {
                assert!(waits_for(&gate), "the background job never started");
            }
            let reply = replies[*index].clone();
            *index += 1;
            reply
        });

        let output = run_session(
            &root,
            &rollout,
            "sess-bg-fail",
            "start a job, then fail",
            &base_url,
            None,
        );

        assert!(
            !output.status.success(),
            "the unparseable reply was accepted: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(job.exists(), "the background job never started");
        no_spinner_survives("bgfail");
    }

    /// A job that ignores every catchable signal, so only the ladder's second
    /// rung stops it -- `invoke`'s `killpg(SIGKILL)` on the group the call
    /// created.
    fn stubborn_spin(label: &str) -> String {
        format!("trap '' TERM INT HUP; {}", spins_and_marks(label))
    }

    /// A cell that starts ten of them and yields.
    fn ten_stubborn_jobs(label: &str) -> String {
        assistant_reply(&format!(
            "```pane\nfor (let i = 0; i < 10; i++) {{ bg.run(\"{}\"); }}\nconst started = 1;\n```",
            stubborn_spin(label)
        ))
    }

    /// **The other half of the Blocker: the exit must stay prompt.**
    ///
    /// Killing the board is only half a fix. `bg::shutdown`'s own grace is
    /// ten seconds, and an exit that spent it — or one settle per job — would
    /// have made a double Ctrl-C worse than the orphan it closes, which is
    /// why `end_the_session` passes its own reap grace and why the whole
    /// shutdown is bounded by one grace rather than by one per job. Ten jobs
    /// that ignore `TERM`, `INT` and `HUP`, and the exit is still measured in
    /// hundreds of milliseconds.
    #[test]
    fn ten_signal_ignoring_jobs_do_not_hold_the_exit() {
        let root = scratch_dir("sigint-stubborn");
        grant_the_spin(&root);
        let rollout = root.join("rollout.jsonl");
        let job = marker_of(&root, "bgstub");
        let foreground = marker_of(&root, "bgstubfg");
        let (base_url, _bodies) = start_fake_provider(vec![
            ten_stubborn_jobs("bgstub"),
            spinning_cell("bgstubfg"),
            spinning_cell("bgstubfg"),
            spinning_cell("bgstubfg"),
        ]);

        let mut child = spawn_session(&root, &rollout, "start ten jobs, then spin", &base_url);
        wait_for_marker(&job, &mut child);
        wait_for_marker(&foreground, &mut child);
        send_interrupt(&child);
        thread::sleep(Duration::from_millis(200));
        send_interrupt(&child);
        let asked = Instant::now();
        let output = child.wait_with_output().unwrap();
        let exit_took = asked.elapsed();

        assert_eq!(
            output.status.code(),
            Some(130),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        // Bounded by `REAP_GRACE` for the board and `REAP_GRACE` again for
        // the foreground child, plus the settles the board's own grace caps:
        // two seconds is a generous ceiling that a per-job grace would still
        // blow through, and ten seconds is what `bg`'s own grace would cost.
        assert!(
            exit_took < Duration::from_secs(2),
            "ten stubborn jobs held the exit for {exit_took:?}"
        );
        no_spinner_survives("bgstub");
        no_spinner_survives("bgstubfg");
    }
}

// ---------------------------------------------------------------------
// The three ways a session used to die, and the flag that opens the grant
// (the primary's fixes of 2026-09-06, from a real run against a strict
// gateway). Each test reproduces the failure through the built binary.
// ---------------------------------------------------------------------

/// Drives the binary as a REPL rather than with `--task`: `inputs` are piped
/// one per line, exactly as a person types them.
fn run_session_stdin(
    root: &Path,
    rollout: &Path,
    session_id: &str,
    inputs: &[&str],
    base_url: &str,
    glasshouse: Option<&Path>,
    yolo: bool,
) -> std::process::Output {
    use std::process::Stdio;
    let mut command = Command::new(env!("CARGO_BIN_EXE_pane"));
    command
        .arg("session")
        .arg("--root")
        .arg(root)
        .arg("--rollout")
        .arg(rollout)
        .arg("--session")
        .arg(session_id)
        .env("ANTHROPIC_BASE_URL", base_url)
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .env_remove("ANTHROPIC_API_KEY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if yolo {
        command.arg("--yolo");
    }
    if let Some(glasshouse) = glasshouse {
        command.arg("--glasshouse").arg(glasshouse);
    }
    let mut child = command.spawn().unwrap();
    {
        let stdin = child.stdin.as_mut().unwrap();
        for line in inputs {
            writeln!(stdin, "{line}").unwrap();
        }
    }
    child.wait_with_output().unwrap()
}

/// A blank line is the commonest keystroke in a REPL, and it used to compose
/// a message with no content — which a gateway enforcing the Messages shape
/// answers `400` to, killing the task. It must not reach the provider at all.
#[test]
fn a_blank_input_is_not_a_turn_and_never_reaches_the_provider() {
    let root = scratch_dir("blank-input-root");
    let rollout = root.join("rollout.jsonl");
    let absent = root.join("no-such-glasshouse");
    // One reply, because exactly one of the four inputs is a turn.
    let (base_url, bodies) = start_fake_provider(vec![ending_reply()]);

    let output = run_session_stdin(
        &root,
        &rollout,
        "sess-blank",
        &["", "   ", "\t", "hi"],
        &base_url,
        Some(&absent),
        false,
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 1, "only `hi` is a turn; three blanks are not");
    let request: serde_json::Value = serde_json::from_str(&bodies[0]).unwrap();
    let messages = request["messages"].as_array().unwrap();
    for message in messages {
        let text: String = message["content"]
            .as_array()
            .unwrap()
            .iter()
            .map(|block| block["text"].as_str().unwrap_or_default())
            .collect();
        assert!(
            !text.trim().is_empty(),
            "no message may be empty; got {message}"
        );
    }
}

/// An empty reply used to be appended to the conversation and replayed on
/// every later request, so one of them turned the whole task into a stream of
/// `400`s. It must end that task instead — and the REPL must survive it, or a
/// person loses the session to one bad turn.
#[test]
fn an_empty_reply_ends_its_task_without_ending_the_session() {
    let root = scratch_dir("empty-reply-root");
    let rollout = root.join("rollout.jsonl");
    let absent = root.join("no-such-glasshouse");
    // Turn one is answered with an empty message; turn two, a fresh task, is
    // answered normally. Two requests prove the REPL lived through the first.
    let (base_url, bodies) = start_fake_provider(vec![assistant_reply(""), ending_reply()]);

    let output = run_session_stdin(
        &root,
        &rollout,
        "sess-empty-reply",
        &["first task", "second task"],
        &base_url,
        Some(&absent),
        false,
    );
    assert!(
        output.status.success(),
        "the session must survive an empty reply; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("empty reply"),
        "the person is told why the task ended; stdout: {stdout}"
    );

    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2, "the second task was still attempted");
    // The decisive assertion: the empty reply is nowhere in the second
    // request. Appending it is what poisoned every later turn.
    let second: serde_json::Value = serde_json::from_str(&bodies[1]).unwrap();
    for message in second["messages"].as_array().unwrap() {
        let text: String = message["content"]
            .as_array()
            .unwrap()
            .iter()
            .map(|block| block["text"].as_str().unwrap_or_default())
            .collect();
        assert!(
            !text.trim().is_empty(),
            "the empty assistant turn must not be replayed; got {message}"
        );
    }
}

/// `--yolo` is the person widening their own grant at session start, and the
/// model is told so in the same breath: a grant it cannot see is a grant it
/// plans around by failing.
#[test]
fn yolo_grants_every_command_line_and_the_system_block_says_so() {
    let root = scratch_dir("yolo-root");
    fs::write(root.join("CLAUDE.md"), "PROJECT").unwrap();
    let rollout = root.join("rollout.jsonl");
    let absent = root.join("no-such-glasshouse");
    let (base_url, bodies) = start_fake_provider(vec![ending_reply()]);

    let output = run_session_stdin(
        &root,
        &rollout,
        "sess-yolo",
        &["go"],
        &base_url,
        Some(&absent),
        true,
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let bodies = bodies.lock().unwrap();
    let request: serde_json::Value = serde_json::from_str(&bodies[0]).unwrap();
    let system = request["system"].as_str().unwrap();
    assert!(
        system.contains("every command line is admitted"),
        "system block must state the yolo grant; got:\n{system}"
    );
    assert!(
        system.contains("To change part of a file, `read` it, edit the"),
        "the model must be told how a file is changed; got:\n{system}"
    );
}

/// Without `--yolo` and without a settings document the sandbox grants
/// nothing, and the system block must say that rather than leave the model to
/// discover it one `PermissionDenied` at a time.
#[test]
fn without_a_grant_the_system_block_says_no_command_may_run() {
    let root = scratch_dir("nogrant-root");
    let rollout = root.join("rollout.jsonl");
    let absent = root.join("no-such-glasshouse");
    let (base_url, bodies) = start_fake_provider(vec![ending_reply()]);

    let output = run_session_stdin(
        &root,
        &rollout,
        "sess-nogrant",
        &["go"],
        &base_url,
        Some(&absent),
        false,
    );
    assert!(output.status.success());

    let bodies = bodies.lock().unwrap();
    let request: serde_json::Value = serde_json::from_str(&bodies[0]).unwrap();
    let system = request["system"].as_str().unwrap();
    assert!(
        system.contains("no command may be run at all"),
        "got:\n{system}"
    );
}

// --- /model actually selects the model ---------------------------------

/// The defect: `/model <slug>` resolved, printed `/model (BuiltIn(Model))`
/// and changed nothing, so the request still named the compiled-in default.
/// Observed 2026-09-06 while driving a real gateway — the run looked like it
/// had switched model and had not.
#[test]
fn slash_model_changes_the_slug_the_next_request_carries() {
    let root = scratch_dir("model-switch-root");
    let rollout = root.join("rollout.jsonl");
    let (base_url, bodies) = start_fake_provider(vec![ending_reply()]);

    let output = run_session_stdin(
        &root,
        &rollout,
        "sess-model-switch",
        &["/model deepseek-v4-flash", "do the thing"],
        &base_url,
        None,
        false,
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let bodies = bodies.lock().unwrap();
    let request: serde_json::Value = serde_json::from_str(&bodies[0]).unwrap();
    assert_eq!(
        request["model"].as_str().unwrap(),
        "deepseek-v4-flash",
        "the request carried the default rather than the slug `/model` was given"
    );
    assert_ne!(request["model"].as_str().unwrap(), pane::wire::MODEL);
}

/// A slash command answers between tasks, so `/model` alone must name what
/// is active without sending anything at all.
#[test]
fn model_picker_names_the_active_slug_without_calling_the_provider() {
    let root = scratch_dir("model-report-root");
    let rollout = root.join("rollout.jsonl");
    let (base_url, bodies) = start_fake_provider(vec![ending_reply()]);

    let output = run_session_stdin(
        &root,
        &rollout,
        "sess-model-report",
        &["/model"],
        &base_url,
        None,
        false,
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(&format!("Current: {}", pane::wire::MODEL)),
        "`/model` did not name the active slug: {stdout}"
    );
    assert!(
        bodies.lock().unwrap().is_empty(),
        "`/model` reached the provider"
    );
}

/// A slug carrying a space is a typo, not a model, and taking it would send
/// a request that can only 404 — so it is refused and the active slug stands.
#[test]
fn a_model_slug_with_a_space_is_refused_and_the_active_slug_stands() {
    let root = scratch_dir("model-refuse-root");
    let rollout = root.join("rollout.jsonl");
    let (base_url, bodies) = start_fake_provider(vec![ending_reply()]);

    let output = run_session_stdin(
        &root,
        &rollout,
        "sess-model-refuse",
        &["/model claude sonnet 5", "do the thing"],
        &base_url,
        None,
        false,
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("/model expects one model name"),
        "no refusal printed: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let bodies = bodies.lock().unwrap();
    let request: serde_json::Value = serde_json::from_str(&bodies[0]).unwrap();
    assert_eq!(
        request["model"].as_str().unwrap(),
        pane::wire::MODEL,
        "a refused slug still changed the model"
    );
}

#[test]
fn untaken_tool_branches_are_not_reported_as_executed_calls() {
    let root = scratch_dir("untaken-branch");
    let rollout = root.join("rollout.jsonl");
    let (base, _) = start_fake_provider(vec![assistant_reply(
        "```pane\nif (false) await bash({command: 'never-run'});\nreturn 'done';\n```",
    )]);
    let output = run_session(&root, &rollout, "untaken", "do it", &base, None);
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("No tool calls ran in this cell."));
    assert!(!text.contains("└─ bash"));
}

// --- a conversation that no longer fits --------------------------------

/// Rung one, and the whole point of it: the retry after an overflow carries a
/// **smaller** request, and the task goes on. Before this, a conversation
/// that outgrew the window ended the task -- the one failure every long task
/// is guaranteed to reach.
#[test]
fn an_overflow_compacts_the_conversation_and_the_task_continues() {
    let root = scratch_dir("overflow-compact-root");
    let rollout = root.join("rollout.jsonl");
    let turn = std::sync::atomic::AtomicUsize::new(0);
    // Two cells first, so there is an older result for compaction to drop;
    // the third request overflows, the fourth is the retry.
    let (base_url, bodies) = start_status_answering_provider(4, move |_body| {
        let n = turn.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        match n {
            0 | 1 => (200, assistant_reply("```pane\nconst a = 1;\n```")),
            2 => (400, too_long_body()),
            _ => (200, ending_reply()),
        }
    });

    let output = run_session(&root, &rollout, "sess-overflow", "do it", &base_url, None);
    assert!(
        output.status.success(),
        "the task did not survive the overflow. stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 4, "expected a retry after the overflow");
    assert!(
        bodies[3].len() < bodies[2].len(),
        "the retry was not smaller than the request that overflowed: {} vs {}",
        bodies[3].len(),
        bodies[2].len()
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("nothing was lost"),
        "the compaction was not reported: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

/// Rung two: with nothing redundant to drop -- the very first request of a
/// task -- the conversation is replaced by a checkpoint, and the retry says
/// so. This is the rung a text harness cannot take, because its results are
/// its transcript.
#[test]
fn an_overflow_with_nothing_to_compact_falls_back_to_a_checkpoint() {
    let root = scratch_dir("overflow-checkpoint-root");
    let rollout = root.join("rollout.jsonl");
    let turn = std::sync::atomic::AtomicUsize::new(0);
    let (base_url, bodies) = start_status_answering_provider(2, move |_body| {
        let n = turn.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if n == 0 {
            (400, too_long_body())
        } else {
            (200, ending_reply())
        }
    });

    let output = run_session(
        &root,
        &rollout,
        "sess-overflow-cp",
        "summarise every caller",
        &base_url,
        None,
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2, "expected one retry");
    let retry: serde_json::Value = serde_json::from_str(&bodies[1]).unwrap();
    let messages = retry["messages"].as_array().unwrap();
    assert_eq!(
        messages.len(),
        1,
        "the checkpoint did not replace the conversation"
    );
    let text = messages[0]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("every handle below is live"),
        "the checkpoint does not tell the model its objects survived: {text}"
    );
    assert!(
        text.contains("summarise every caller"),
        "the checkpoint lost the task: {text}"
    );
}

/// An ordinary 400 is not an overflow and must not be retried as one: a
/// malformed request retried unchanged is a loop, and retried after a
/// checkpoint has thrown away a conversation for nothing.
#[test]
fn a_plain_bad_request_is_reported_rather_than_compacted() {
    let root = scratch_dir("plain-400-root");
    let rollout = root.join("rollout.jsonl");
    let (base_url, bodies) = start_status_answering_provider(1, move |_body| {
        (
            400,
            serde_json::json!({"type":"error","error":{"message":"model: unknown field"}})
                .to_string(),
        )
    });

    let output = run_session(&root, &rollout, "sess-plain-400", "do it", &base_url, None);
    assert_eq!(
        bodies.lock().unwrap().len(),
        1,
        "a plain 400 was retried as though it were an overflow"
    );
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        all.contains("unknown field"),
        "the real error was not reported: {all}"
    );
}
