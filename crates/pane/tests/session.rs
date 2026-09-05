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
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let bodies_thread = Arc::clone(&bodies);

    thread::spawn(move || {
        for reply in replies {
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            handle_one_request(stream, &reply, &bodies_thread);
        }
    });

    (format!("http://127.0.0.1:{port}"), bodies)
}

fn handle_one_request(mut stream: TcpStream, reply: &str, bodies: &Mutex<Vec<String>>) {
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
    bodies
        .lock()
        .unwrap()
        .push(String::from_utf8_lossy(&body).into_owned());

    let response_body = reply.as_bytes();
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        response_body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.write_all(response_body);
    let _ = stream.flush();
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
    let (base_url, bodies) = start_fake_provider(vec![assistant_reply("hi from the model")]);

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

    assert_eq!(bodies.lock().unwrap().len(), 1);
}

#[test]
fn the_binary_resumes_an_existing_rollout_instead_of_starting_over() {
    let root = scratch_dir("resume-root");
    let rollout = root.join("rollout.jsonl");

    let (first_url, _first_bodies) = start_fake_provider(vec![assistant_reply("first reply")]);
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

    let (second_url, second_bodies) = start_fake_provider(vec![assistant_reply("second reply")]);
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
    assert_eq!(bodies.len(), 1);
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
    let (base_url, bodies) = start_fake_provider(vec![assistant_reply("ack")]);

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
    let (base_url, _bodies) = start_fake_provider(vec![assistant_reply("ack")]);

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
fn an_unbuilt_slash_command_names_its_subphase() {
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
        stdout.contains("61E"),
        "an unbuilt built-in must name its own sub-phase: {stdout}"
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
    let (base_url, _bodies) = start_fake_provider(vec![assistant_reply(&malicious)]);

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
    let (base_url, _bodies) =
        start_fake_provider(vec![assistant_reply("PANE-PRINTED-REPLY-MARKER")]);

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
    let (base_url, _bodies) = start_fake_provider(vec![assistant_reply("ack")]);

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
        stdout.contains("ProjectSkill"),
        "resolving a bare project skill by name never reached the binary's ProjectSkill branch:\n{stdout}"
    );
}
