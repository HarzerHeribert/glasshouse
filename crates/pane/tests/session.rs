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
        stdout.contains("ProjectSkill"),
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
        stdout.contains("assistant: undefined"),
        "the first block's binding must not exist in the isolate:\n{stdout}"
    );
    assert!(
        !stdout.contains("assistant: number"),
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
        assistant_reply("```pane\nconst before = 1;\nnosuch.field;\n```"),
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
        bodies[1].matches("nosuch.field").count(),
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

    let expected = pane::prompt::render_system(
        "PROJECT-INSTRUCTION-ONE",
        &pane::tools::registry::ALL.iter().collect::<Vec<_>>(),
    );

    let bodies = bodies.lock().unwrap();
    let request: serde_json::Value = serde_json::from_str(&bodies[0]).unwrap();
    assert_eq!(request["system"].as_str().unwrap(), expected);
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
        stdout.contains("assistant: Three files name it; two are tests."),
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
            "```pane\nconst before = 1;\nnosuch.field;\nreturn \"CONFIDENT SENTENCE\";\n```",
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
        stdout.contains(r#"assistant: {"matches":3,"files":2"#),
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
