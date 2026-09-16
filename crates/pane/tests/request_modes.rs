//! Request modes through the built `pane` binary (map lines 2637, 2638).
//!
//! Every test ignores the system prompt's mode line and has the scripted model
//! invoke the refused tool anyway: the prompt informs, the narrowed profile is
//! what refuses. The decisive assertion is always the filesystem — a refused
//! write left no file — with the refusal's rule text as the second half.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn scratch_dir(label: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "pane-request-modes-{label}-{}-{n}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A loopback provider answering each request from its body.
fn start_provider<F>(turns: usize, answer: F) -> (String, Arc<Mutex<Vec<String>>>)
where
    F: Fn(&str) -> String + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&bodies);
    thread::spawn(move || {
        for _ in 0..turns {
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            answer_one(stream, &answer, &seen);
        }
    });
    (format!("http://127.0.0.1:{port}"), bodies)
}

fn answer_one<F: Fn(&str) -> String>(
    mut stream: TcpStream,
    answer: &F,
    bodies: &Mutex<Vec<String>>,
) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        if let Some(rest) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            length = rest.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    let body = String::from_utf8_lossy(&body).into_owned();
    let reply = answer(&body);
    bodies.lock().unwrap().push(body);
    let _ = stream.write_all(
        format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            reply.len()
        )
        .as_bytes(),
    );
    let _ = stream.write_all(reply.as_bytes());
    let _ = stream.flush();
}

fn cell_reply(code: &str) -> String {
    serde_json::json!({
        "role": "assistant",
        "content": [{"type": "text", "text": format!("```pane\n{code}\n```")}],
    })
    .to_string()
}

/// A project whose profile admits every command line, so a refusal the tests
/// observe is the mode's.
fn project(label: &str) -> PathBuf {
    let root = scratch_dir(label);
    fs::create_dir_all(root.join(".pane")).unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join(".pane/config.toml"),
        "[permissions]\nallow = [\"Bash\"]\n",
    )
    .unwrap();
    fs::write(root.join("src/lib.rs"), "pub fn existing() {}\n").unwrap();
    root
}

/// `pane session` with `args`, `inputs` piped one per line.
fn run(root: &Path, args: &[&str], inputs: &[&str], base_url: &str) -> std::process::Output {
    let rollout = scratch_dir("rollout").join("rollout.jsonl");
    let mut command = Command::new(env!("CARGO_BIN_EXE_pane"));
    command
        .arg("session")
        .arg("--root")
        .arg(root)
        .arg("--rollout")
        .arg(&rollout)
        .arg("--session")
        .arg(format!(
            "sess-mode-{}",
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
        .arg("--model")
        .arg(pane::wire::MODEL)
        .args(args)
        .env("ANTHROPIC_BASE_URL", base_url)
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .env_remove("ANTHROPIC_API_KEY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    {
        let stdin = child.stdin.as_mut().unwrap();
        for line in inputs {
            writeln!(stdin, "{line}").unwrap();
        }
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

/// One cell that attempts a write, and returns what happened as a string.
fn attempt_write(file: &str) -> String {
    format!(
        "let out;\ntry {{ await write({{ path: \"{file}\", content: \"changed\" }}); out = \"wrote\"; }} catch (e) {{ out = \"refused: \" + e.message; }}\nreturn out;"
    )
}

#[test]
fn explore_refuses_a_write_the_model_attempts_despite_the_prompt() {
    let root = project("explore-write");
    let (base_url, bodies) = start_provider(1, |_| cell_reply(&attempt_write("src/new.rs")));
    let output = run(&root, &["--mode", "explore"], &["add a module"], &base_url);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        !root.join("src/new.rs").exists(),
        "explore wrote outside its globs: {stdout}"
    );
    assert!(
        stdout.contains("mode explore: writes only under"),
        "{stdout}"
    );
    let bodies = bodies.lock().unwrap();
    assert!(
        bodies[0].contains("Request mode: explore"),
        "the prompt did not name the mode"
    );
}

/// The control half: the same scripted write lands in `execute`, so the
/// refusal above is the mode's and not a broken fixture.
#[test]
fn execute_runs_the_same_write() {
    let root = project("execute-write");
    let (base_url, bodies) = start_provider(1, |_| cell_reply(&attempt_write("src/new.rs")));
    run(&root, &[], &["add a module"], &base_url);
    assert_eq!(
        fs::read_to_string(root.join("src/new.rs")).unwrap(),
        "changed"
    );
    assert!(!bodies.lock().unwrap()[0].contains("Request mode:"));
}

#[test]
fn plan_reads_and_refuses_a_write() {
    let root = project("plan");
    let code = "const file = await read({ path: \"src/lib.rs\" });\nlet out = \"read:\" + file.preview;\ntry { await write({ path: \"PLAN.md\", content: \"changed\" }); out += \"|wrote\"; } catch (e) { out += \"|refused: \" + e.message; }\nreturn out;";
    let (base_url, _) = start_provider(1, move |_| cell_reply(code));
    let output = run(&root, &["--plan"], &["plan the change"], &base_url);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !root.join("PLAN.md").exists(),
        "plan executed a change: {stdout}"
    );
    assert!(stdout.contains("existing"), "plan could not read: {stdout}");
    assert!(stdout.contains("mode plan: no change executes"), "{stdout}");
}

/// `/mode execute` between requests lifts the narrowing from the next one.
#[test]
fn slash_mode_execute_restores_writes_from_the_next_request() {
    let root = project("mode-switch");
    let (base_url, _) = start_provider(2, |body| {
        if body.contains("second request") {
            cell_reply(&attempt_write("src/second.rs"))
        } else {
            cell_reply(&attempt_write("src/first.rs"))
        }
    });
    let output = run(
        &root,
        &["--mode", "explore"],
        &["first request", "/mode execute", "second request"],
        &base_url,
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!root.join("src/first.rs").exists(), "{stdout}");
    assert!(
        root.join("src/second.rs").exists(),
        "/mode execute did not lift the narrowing: {stdout}"
    );
}

/// The shell stays read-only in explore: a redirect and `rm` are refused, a
/// read-only command runs.
#[cfg(unix)]
#[test]
fn explore_bash_runs_read_only_commands_and_refuses_writers() {
    let root = project("explore-bash");
    fs::write(root.join("victim.txt"), "keep").unwrap();
    let code = "const listed = await bash({ command: \"ls src\" });\nlet out = \"ls:\" + listed.stdout;\nfor (const command of [\"echo x > made.txt\", \"rm victim.txt\"]) {\n  try { await bash({ command }); out += \"|ran \" + command; } catch (e) { out += \"|refused: \" + e.message; }\n}\nreturn out;";
    let (base_url, _) = start_provider(1, move |_| cell_reply(code));
    let output = run(&root, &["--mode", "explore"], &["look around"], &base_url);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!root.join("made.txt").exists(), "{stdout}");
    assert!(root.join("victim.txt").exists(), "{stdout}");
    assert!(
        stdout.contains("lib.rs"),
        "a read-only command did not run: {stdout}"
    );
    assert!(stdout.contains("writes through a redirect"), "{stdout}");
    assert!(
        stdout.contains("`rm` is not a read-only command"),
        "{stdout}"
    );
}

/// `/tool` is the direct frame a person types; it takes the same narrowing.
#[test]
fn a_direct_tool_frame_is_refused_by_the_same_rule() {
    let root = project("explore-tool");
    let (base_url, _) = start_provider(0, |_| String::new());
    let output = run(
        &root,
        &["--mode", "explore"],
        &["/tool write path=src/direct.rs content=changed"],
        &base_url,
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!root.join("src/direct.rs").exists(), "{stdout}");
    assert!(
        stdout.contains("mode explore: writes only under"),
        "{stdout}"
    );
}
