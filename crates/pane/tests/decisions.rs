//! Binary-level canaries for the decision model's hold
//! (`docs/product/pane/decision-model.md`): the built `pane` binary against a
//! loopback fake that dispatches on the request path -- `/v1/messages`
//! answers scripted cells in order (the `providers` fake from
//! `tests/evidence_gate.rs`, path-aware here since one task now makes two
//! kinds of request), `/v1/systemone` answers one scripted decision, a
//! non-2xx status, or sleeps past the 2 s bound.
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn root(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "pane-decisions-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn write_config(root: &Path, text: &str) {
    let dir = root.join(".pane");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.toml"), text).unwrap();
}

/// What the decision endpoint does with the one request each test sends it.
enum Decision {
    Answer(Value),
    Status(u16),
    Sleep(Duration),
}

/// Recorded request bodies (or, for the decision endpoint's headers, its raw
/// header block), shared with the fake server's own thread.
type Recorded = Arc<Mutex<Vec<String>>>;

/// A fake provider dispatching on the request's own path: `/v1/messages`
/// answers `cells` in order, `/v1/systemone` answers `decision` once (never,
/// when `decision` is `None` -- the no-model tests prove no connection ever
/// arrives there). Every request's body is kept, and the decision request's
/// header block is kept alongside its body.
fn providers(
    cells: Vec<Value>,
    decision: Option<Decision>,
) -> (String, Recorded, Recorded, Recorded) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let message_bodies = Arc::new(Mutex::new(Vec::new()));
    let decision_bodies = Arc::new(Mutex::new(Vec::new()));
    let decision_headers = Arc::new(Mutex::new(Vec::new()));
    let seen_messages = Arc::clone(&message_bodies);
    let seen_decision_bodies = Arc::clone(&decision_bodies);
    let seen_decision_headers = Arc::clone(&decision_headers);
    std::thread::spawn(move || {
        let mut cells = cells.into_iter();
        let mut decision = decision;
        loop {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(20)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            if reader.read_line(&mut request_line).unwrap() == 0 {
                return;
            }
            let path = request_line
                .split_whitespace()
                .nth(1)
                .unwrap_or_default()
                .to_string();
            let mut length = 0;
            let mut header_block = String::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 {
                    return;
                }
                if line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap();
                }
                header_block.push_str(&line);
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let body_text = String::from_utf8_lossy(&body).into_owned();

            if path == "/v1/systemone" {
                seen_decision_bodies.lock().unwrap().push(body_text);
                seen_decision_headers.lock().unwrap().push(header_block);
                match decision
                    .take()
                    .expect("only one decision request is scripted")
                {
                    Decision::Answer(value) => {
                        let response = value.to_string();
                        let _ = write!(
                            stream,
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
                            response.len()
                        );
                    }
                    Decision::Status(status) => {
                        let response = "{}";
                        let _ = write!(
                            stream,
                            "HTTP/1.1 {status} Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
                            response.len()
                        );
                    }
                    Decision::Sleep(duration) => {
                        std::thread::sleep(duration);
                        let response = "{}";
                        let _ = write!(
                            stream,
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
                            response.len()
                        );
                    }
                }
            } else {
                seen_messages.lock().unwrap().push(body_text);
                let Some(response) = cells.next() else {
                    return;
                };
                let response = response.to_string();
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
                    response.len()
                );
            }
        }
    });
    (url, message_bodies, decision_bodies, decision_headers)
}

fn cell(id: &str, code: &str) -> Value {
    json!({"role": "assistant", "content": [{"type": "tool_use", "id": id, "name": "execute_cell", "input": {"code": code}}],
        "usage": {"input_tokens": 20, "output_tokens": 7}})
}

/// A native `Write` tool call -- the shape a `--interface tools` turn sends
/// instead of `execute_cell` (`abi::dialect`'s `Write` shape).
fn direct_write(id: &str, path: &str, content: &str) -> Value {
    json!({"role": "assistant", "content": [{"type": "tool_use", "id": id, "name": "Write", "input": {"file_path": path, "content": content}}],
        "usage": {"input_tokens": 20, "output_tokens": 7}})
}

/// A native `Read` tool call -- pure, never held.
fn direct_read(id: &str, path: &str) -> Value {
    json!({"role": "assistant", "content": [{"type": "tool_use", "id": id, "name": "Read", "input": {"file_path": path}}],
        "usage": {"input_tokens": 20, "output_tokens": 7}})
}

/// A plain-text completion. A direct tool call never ends the task itself --
/// unlike `return` inside a cell -- so a direct-frame scenario needs one of
/// these to finish.
fn prose(text: &str) -> Value {
    json!({"role": "assistant", "content": [{"type": "text", "text": text}],
        "usage": {"input_tokens": 20, "output_tokens": 7}})
}

fn decision_answer(choice: &str, confidence: f64) -> Value {
    json!({
        "model": "jev-latest",
        "answers": {
            "intent": {
                "type": "choice",
                "choice": choice,
                "probabilities": {"read_only": confidence, "modify": 0.0, "run": 0.0, "other": 0.0},
                "confidence": confidence,
            }
        },
        "usage": {"input_tokens": 40, "output_tokens": 12},
    })
}

const DECISIONS_ON: &str =
    "[decisions]\nmodel = \"jev-latest\"\nmode = \"on\"\nhold_above = 0.85\n";
const DECISIONS_SHADOW: &str =
    "[decisions]\nmodel = \"jev-latest\"\nmode = \"shadow\"\nhold_above = 0.85\n";

/// Runs `pane exec` against `endpoint`, killing it and answering `None` if it
/// has not exited within `timeout` -- the once rule's own mutation (dropping
/// `effect_holds == 0`) makes a held cell hold forever, since a held cell is
/// charged no cell of the budget; this is what turns that hang into a fast,
/// clean test failure instead of blocking the suite.
fn exec_bounded(root: &Path, endpoint: &str, task: &str, interface: Option<&str>) -> Option<Value> {
    let out_path = root.join("stdout.json");
    let stdout_file = std::fs::File::create(&out_path).unwrap();
    let mut args = vec![
        "exec".to_string(),
        task.to_string(),
        "--output-format".to_string(),
        "json".to_string(),
        "--root".to_string(),
        root.display().to_string(),
        "--model".to_string(),
        "test/model".to_string(),
        "--glasshouse".to_string(),
        root.join("absent-glasshouse").display().to_string(),
    ];
    if let Some(interface) = interface {
        args.push("--interface".to_string());
        args.push(interface.to_string());
    }
    let mut child = Command::new(env!("CARGO_BIN_EXE_pane"))
        .args(&args)
        .env("ANTHROPIC_BASE_URL", endpoint)
        .env("ANTHROPIC_API_KEY", "test-only")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .stdout(stdout_file)
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let started = Instant::now();
    loop {
        if child.try_wait().unwrap().is_some() {
            let bytes = std::fs::read(&out_path).unwrap();
            return serde_json::from_slice(&bytes).ok();
        }
        if started.elapsed() > Duration::from_secs(10) {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_read_only_request_holds_the_first_effectful_cell_once_then_lets_it_run() {
    let root = root("hold-once");
    write_config(&root, DECISIONS_ON);
    let held = "await write({path: \"a.txt\", content: \"1\"});\nreturn \"done\";";
    let (endpoint, messages, decisions, _headers) = providers(
        vec![cell("c1", held), cell("c2", held)],
        Some(Decision::Answer(decision_answer("read_only", 0.94))),
    );
    let result = exec_bounded(&root, &endpoint, "read the file for me", None)
        .expect("the once rule keeps the task moving");
    let messages = messages.lock().unwrap();
    assert_eq!(messages.len(), 2, "held, then the re-issue that runs");
    assert!(
        messages[1].contains("## Held (decision)"),
        "the held block reaches the model's next turn: {}",
        messages[1]
    );
    assert_eq!(decisions.lock().unwrap().len(), 1, "asked once per task");
    assert!(root.join("a.txt").exists(), "the re-issued cell ran");
    assert_eq!(result["answer"], "done");
    let telemetry = &result["telemetry"]["decisions"];
    assert_eq!(telemetry["holds"], 1, "{telemetry}");
    assert_eq!(telemetry["overrides"], 1, "{telemetry}");
    assert_eq!(telemetry["intent"]["choice"], "read_only", "{telemetry}");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_confidence_below_hold_above_never_holds() {
    let root = root("below-threshold");
    write_config(&root, DECISIONS_ON);
    let (endpoint, messages, _decisions, _headers) = providers(
        vec![cell(
            "c1",
            "await write({path: \"a.txt\", content: \"1\"});\nreturn \"done\";",
        )],
        Some(Decision::Answer(decision_answer("read_only", 0.80))),
    );
    let result =
        exec_bounded(&root, &endpoint, "read the file for me", None).expect("no hold, no hang");
    assert_eq!(messages.lock().unwrap().len(), 1, "nothing is ever held");
    assert!(root.join("a.txt").exists());
    let telemetry = &result["telemetry"]["decisions"];
    assert_eq!(telemetry["would_hold"], 0, "{telemetry}");
    assert_eq!(telemetry["holds"], 0, "{telemetry}");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn shadow_records_the_would_be_hold_and_writes_the_file() {
    let root = root("shadow");
    write_config(&root, DECISIONS_SHADOW);
    let (endpoint, messages, _decisions, _headers) = providers(
        vec![cell(
            "c1",
            "await write({path: \"a.txt\", content: \"1\"});\nreturn \"done\";",
        )],
        Some(Decision::Answer(decision_answer("read_only", 0.94))),
    );
    let result =
        exec_bounded(&root, &endpoint, "read the file for me", None).expect("shadow never holds");
    let messages = messages.lock().unwrap();
    assert_eq!(messages.len(), 1, "shadow runs the cell as today");
    assert!(
        !messages[0].contains("## Held (decision)"),
        "shadow never reaches the model: {}",
        messages[0]
    );
    assert!(root.join("a.txt").exists(), "shadow still writes the file");
    let telemetry = &result["telemetry"]["decisions"];
    assert_eq!(telemetry["would_hold"], 1, "{telemetry}");
    assert_eq!(telemetry["holds"], 0, "{telemetry}");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_modify_intent_or_a_pure_cell_is_never_held() {
    let modify = root("modify-intent");
    write_config(&modify, DECISIONS_ON);
    let (endpoint, messages, _decisions, _headers) = providers(
        vec![cell(
            "c1",
            "await write({path: \"a.txt\", content: \"1\"});\nreturn \"done\";",
        )],
        Some(Decision::Answer(decision_answer("modify", 0.99))),
    );
    exec_bounded(&modify, &endpoint, "edit the file for me", None).expect("modify never holds");
    assert_eq!(messages.lock().unwrap().len(), 1);
    assert!(modify.join("a.txt").exists());
    let _ = std::fs::remove_dir_all(modify);

    let pure = root("pure-cell");
    write_config(&pure, DECISIONS_ON);
    std::fs::write(pure.join("notes.txt"), "hi\n").unwrap();
    let (endpoint, messages, _decisions, _headers) = providers(
        vec![cell(
            "c1",
            "const seen = await read({path: \"notes.txt\"});\nreturn seen.text;",
        )],
        Some(Decision::Answer(decision_answer("read_only", 0.94))),
    );
    let result = exec_bounded(&pure, &endpoint, "read notes.txt for me", None)
        .expect("a pure cell never holds");
    assert_eq!(messages.lock().unwrap().len(), 1);
    assert_eq!(result["answer"], "hi\n");
    let _ = std::fs::remove_dir_all(pure);
}

#[test]
fn a_direct_tool_frame_is_held_by_the_same_rule() {
    let root = root("direct-frame");
    write_config(&root, DECISIONS_ON);
    let target = root.join("a.txt");
    let (endpoint, messages, _decisions, _headers) = providers(
        vec![
            direct_write("t1", target.to_str().unwrap(), "1"),
            direct_write("t2", target.to_str().unwrap(), "1"),
            prose("Done: a.txt is written."),
        ],
        Some(Decision::Answer(decision_answer("read_only", 0.94))),
    );
    let result = exec_bounded(&root, &endpoint, "write a.txt for me", Some("tools"))
        .expect("the once rule keeps a direct frame moving too");
    let messages = messages.lock().unwrap();
    assert_eq!(
        messages.len(),
        3,
        "held, the re-issued call that runs, then the finishing prose"
    );
    assert!(
        messages[1].contains("## Held (decision)"),
        "the tool_result carries the held block: {}",
        messages[1]
    );
    assert!(
        messages[2].contains("wrote 1 bytes"),
        "the re-issued call actually ran, not held again: {}",
        messages[2]
    );
    assert!(target.exists(), "the re-issued Write call ran");
    assert_eq!(result["answer"], "Done: a.txt is written.");
    let telemetry = &result["telemetry"]["decisions"];
    assert_eq!(telemetry["holds"], 1, "{telemetry}");
    let _ = std::fs::remove_dir_all(&root);

    let read_root = root.with_file_name(format!(
        "{}-read",
        root.file_name().unwrap().to_string_lossy()
    ));
    std::fs::create_dir_all(&read_root).unwrap();
    write_config(&read_root, DECISIONS_ON);
    std::fs::write(read_root.join("notes.txt"), "hi\n").unwrap();
    let (endpoint, messages, _decisions, _headers) = providers(
        vec![
            direct_read("t1", read_root.join("notes.txt").to_str().unwrap()),
            prose("Done: notes.txt says hi."),
        ],
        Some(Decision::Answer(decision_answer("read_only", 0.94))),
    );
    let result = exec_bounded(
        &read_root,
        &endpoint,
        "read notes.txt for me",
        Some("tools"),
    )
    .expect("a Read frame is never held");
    let messages = messages.lock().unwrap();
    assert_eq!(messages.len(), 2, "a Read frame is never held");
    assert!(
        !messages[1].contains("## Held (decision)"),
        "{}",
        messages[1]
    );
    assert_eq!(result["answer"], "Done: notes.txt says hi.");
    let _ = std::fs::remove_dir_all(read_root);
}

#[test]
fn a_failed_or_slow_decision_leaves_the_task_as_it_is() {
    let failed = root("decision-500");
    write_config(&failed, DECISIONS_ON);
    let (endpoint, messages, _decisions, _headers) = providers(
        vec![cell(
            "c1",
            "await write({path: \"a.txt\", content: \"1\"});\nreturn \"done\";",
        )],
        Some(Decision::Status(500)),
    );
    let result = exec_bounded(&failed, &endpoint, "read the file for me", None)
        .expect("a failed decision leaves the task alone");
    assert_eq!(
        messages.lock().unwrap().len(),
        1,
        "no hold on a failed decision"
    );
    assert!(failed.join("a.txt").exists());
    let telemetry = &result["telemetry"]["decisions"];
    assert_eq!(telemetry["failed"], 1, "{telemetry}");
    assert!(telemetry["intent"].is_null(), "{telemetry}");
    let _ = std::fs::remove_dir_all(failed);

    let slow = root("decision-slow");
    write_config(&slow, DECISIONS_ON);
    let (endpoint, messages, _decisions, _headers) = providers(
        vec![cell(
            "c1",
            "await write({path: \"a.txt\", content: \"1\"});\nreturn \"done\";",
        )],
        Some(Decision::Sleep(Duration::from_secs(3))),
    );
    let result = exec_bounded(&slow, &endpoint, "read the file for me", None)
        .expect("a slow decision times out rather than hanging the task");
    assert_eq!(messages.lock().unwrap().len(), 1);
    assert!(slow.join("a.txt").exists());
    assert_eq!(result["telemetry"]["decisions"]["failed"], 1);
    let _ = std::fs::remove_dir_all(slow);
}

#[test]
fn no_model_means_no_request_and_no_thread() {
    let root = root("no-model");
    let (endpoint, messages, decisions, _headers) = providers(
        vec![cell(
            "c1",
            "await write({path: \"a.txt\", content: \"1\"});\nreturn \"done\";",
        )],
        None,
    );
    let result =
        exec_bounded(&root, &endpoint, "read the file for me", None).expect("nothing to hang on");
    assert_eq!(messages.lock().unwrap().len(), 1);
    assert_eq!(
        decisions.lock().unwrap().len(),
        0,
        "no request ever reaches /v1/systemone"
    );
    assert!(result["telemetry"]["decisions"].is_null(), "{result}");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn the_decision_request_carries_purpose_model_and_the_intent_question() {
    let root = root("wire-shape");
    write_config(&root, DECISIONS_ON);
    let (endpoint, _messages, decisions, headers) = providers(
        vec![cell(
            "c1",
            "await write({path: \"a.txt\", content: \"1\"});\nreturn \"done\";",
        )],
        Some(Decision::Answer(decision_answer("read_only", 0.94))),
    );
    exec_bounded(&root, &endpoint, "read the file for me", None).expect("nothing to hang on");
    let headers = headers.lock().unwrap();
    assert_eq!(headers.len(), 1);
    let header_text = headers[0].to_lowercase();
    assert!(
        header_text.contains("x-glasshouse-purpose: decision"),
        "{header_text}"
    );
    assert!(
        header_text.contains("x-glasshouse-model: jev-latest"),
        "{header_text}"
    );
    let bodies = decisions.lock().unwrap();
    let body: Value = serde_json::from_str(&bodies[0]).unwrap();
    assert_eq!(body["model"], "jev-latest");
    assert_eq!(body["questions"]["intent"]["type"], "choice");
    assert!(body["questions"]["intent"]["criteria"]["read_only"].is_string());
    let _ = std::fs::remove_dir_all(root);
}
