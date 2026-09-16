//! Binary-level canaries for the decision model's hold and its completion
//! question (`docs/product/pane/decision-model.md`): the built `pane` binary
//! against a loopback fake that dispatches on the request path --
//! `/v1/messages` answers scripted cells in order (the `providers` fake from
//! `tests/evidence_gate.rs`, path-aware here since one task now makes up to
//! three kinds of request), `/v1/systemone` answers by the request's own
//! question key -- `"intent"` (asked once per task, before the first turn)
//! or `"satisfied"` (asked once per task, at the completion gate; 2616) --
//! each with a scripted answer, a non-2xx status, a sleep past the 2 s
//! bound, or (unscripted) a harmless default.
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

/// What the decision endpoint does with one scripted question.
enum Decision {
    Answer(Value),
    Status(u16),
    Sleep(Duration),
}

/// Recorded request bodies (or, for the decision endpoint's headers, its raw
/// header block), shared with the fake server's own thread.
type Recorded = Arc<Mutex<Vec<String>>>;

/// The single key of a decision request's `questions` object -- `"intent"`
/// or `"satisfied"`, the only two this package ever asks in one request.
fn question_key(body_text: &str) -> String {
    let value: Value = serde_json::from_str(body_text).unwrap();
    value["questions"]
        .as_object()
        .and_then(|questions| questions.keys().next())
        .cloned()
        .unwrap_or_default()
}

/// A fake provider dispatching on the request's own path: `/v1/messages`
/// answers `cells` in order. `/v1/systemone` answers by the request's own
/// question key: `intent`'s scripted answers in order (or a harmless
/// `read_only 0.94` default once the queue is empty), and `satisfied`'s
/// scripted answers in order (or a harmless `noul 0.50` default once its
/// queue is empty) -- most tests script at most one of each, since this
/// package asks the intent question once and the completion question once
/// per distinct diff claimed; a test that claims two different diffs (2616's
/// re-ask fix) scripts two `satisfied` answers. Every request's body is
/// kept, and the decision endpoint's own header blocks are kept alongside
/// its bodies.
fn providers(
    cells: Vec<Value>,
    intent: Vec<Decision>,
    completion: Vec<Decision>,
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
        let mut intent: std::collections::VecDeque<Decision> = intent.into_iter().collect();
        let mut completion: std::collections::VecDeque<Decision> = completion.into_iter().collect();
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
                let key = question_key(&body_text);
                seen_decision_bodies.lock().unwrap().push(body_text);
                seen_decision_headers.lock().unwrap().push(header_block);
                let decision = match key.as_str() {
                    "intent" => intent
                        .pop_front()
                        .unwrap_or_else(|| Decision::Answer(decision_answer("read_only", 0.94))),
                    "satisfied" => completion
                        .pop_front()
                        .unwrap_or_else(|| Decision::Answer(completion_answer(0.50))),
                    other => panic!("unexpected decision question key `{other}`"),
                };
                match decision {
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

fn completion_answer(noul: f64) -> Value {
    json!({
        "model": "jev-latest",
        "answers": {
            "satisfied": {
                "type": "noul",
                "noul": noul,
            }
        },
        "usage": {"input_tokens": 40, "output_tokens": 12},
    })
}

const DECISIONS_ON: &str =
    "[decisions]\nmodel = \"jev-latest\"\nmode = \"on\"\nhold_above = 0.85\n";
const DECISIONS_SHADOW: &str =
    "[decisions]\nmodel = \"jev-latest\"\nmode = \"shadow\"\nhold_above = 0.85\n";
const DECISIONS_ON_WITH_CHECKER: &str = "[decisions]\nmodel = \"jev-latest\"\nmode = \"on\"\nhold_above = 0.85\n[helpers]\nmodel = \"helper-tier\"\ncompletion_check = true\n";

fn write_checks_toml(root: &Path, text: &str) {
    let dir = root.join(".glasshouse");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("checks.toml"), text).unwrap();
}

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
        vec![Decision::Answer(decision_answer("read_only", 0.94))],
        vec![],
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
    assert_eq!(
        decisions.lock().unwrap().len(),
        2,
        "the intent question once, and the completion question once when the task finishes"
    );
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
        vec![Decision::Answer(decision_answer("read_only", 0.80))],
        vec![],
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
        vec![Decision::Answer(decision_answer("read_only", 0.94))],
        vec![],
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
        vec![Decision::Answer(decision_answer("modify", 0.99))],
        vec![],
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
        vec![Decision::Answer(decision_answer("read_only", 0.94))],
        vec![],
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
        vec![Decision::Answer(decision_answer("read_only", 0.94))],
        vec![],
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
        vec![Decision::Answer(decision_answer("read_only", 0.94))],
        vec![],
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
        vec![Decision::Status(500)],
        vec![],
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
        vec![Decision::Sleep(Duration::from_secs(3))],
        vec![],
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
        vec![],
        vec![],
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
        vec![Decision::Answer(decision_answer("read_only", 0.94))],
        vec![],
    );
    exec_bounded(&root, &endpoint, "read the file for me", None).expect("nothing to hang on");
    let headers = headers.lock().unwrap();
    assert_eq!(
        headers.len(),
        1,
        "the effectful cell is held, so the completion question is never reached"
    );
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

// -- the completion question (2616) --------------------------------------

#[test]
fn a_confident_no_holds_the_completion_once_then_records_it_unverified() {
    let root = root("completion-no");
    write_config(&root, DECISIONS_ON);
    let (endpoint, messages, decisions, _headers) = providers(
        vec![
            cell("c1", "return \"done\";"),
            cell("c2", "return \"done\";"),
        ],
        vec![],
        vec![Decision::Answer(completion_answer(0.06))],
    );
    let result = exec_bounded(&root, &endpoint, "fix the bug", None)
        .expect("a held completion is recorded unverified, not a refusal");
    let messages = messages.lock().unwrap();
    assert_eq!(messages.len(), 2, "held once, then the same claim again");
    assert!(
        messages[1].contains("the decision model reads the diff as not satisfying the request"),
        "{}",
        messages[1]
    );
    assert_eq!(result["telemetry"]["completion"]["verified"], false);
    assert_eq!(result["telemetry"]["completion"]["deferred"], 1);
    let telemetry = &result["telemetry"]["decisions"]["completion"];
    assert_eq!(telemetry["noul"], 0.06, "{telemetry}");
    assert_eq!(telemetry["finding_added"], true, "{telemetry}");
    assert_eq!(
        decisions.lock().unwrap().len(),
        2,
        "the intent question once, and the completion question once for the unchanged diff -- not twice"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_changed_diff_is_asked_again_and_a_fixed_task_verifies() {
    let root = root("completion-changed-diff");
    write_config(&root, DECISIONS_ON);
    let (endpoint, messages, decisions, _headers) = providers(
        vec![
            cell(
                "c1",
                "await write({path: \"a.txt\", content: \"1\"});\nreturn \"done\";",
            ),
            cell(
                "c2",
                "await write({path: \"b.txt\", content: \"1\"});\nreturn \"done\";",
            ),
        ],
        vec![Decision::Answer(decision_answer("modify", 0.99))],
        vec![
            Decision::Answer(completion_answer(0.05)),
            Decision::Answer(completion_answer(0.95)),
        ],
    );
    let result = exec_bounded(&root, &endpoint, "fix the bug", None)
        .expect("a fixed task verifies once the diff changes");
    let messages = messages.lock().unwrap();
    assert_eq!(
        messages.len(),
        2,
        "held on the first diff, then a second cell whose diff has changed"
    );
    assert!(
        messages[1].contains("the decision model reads the diff as not satisfying the request"),
        "{}",
        messages[1]
    );
    assert_eq!(
        result["telemetry"]["completion"]["verified"], true,
        "the fixed diff is judged fresh, not against the stale no: {result}"
    );
    let telemetry = &result["telemetry"]["decisions"]["completion"];
    assert_eq!(telemetry["noul"], 0.95, "{telemetry}");
    assert_eq!(telemetry["finding_added"], false, "{telemetry}");
    let bodies = decisions.lock().unwrap();
    assert_eq!(
        bodies.len(),
        3,
        "the intent question once, and the completion question twice -- once per distinct diff"
    );
    let satisfied: Vec<Value> = bodies
        .iter()
        .filter(|body| body.contains("\"satisfied\""))
        .map(|body| serde_json::from_str(body).unwrap())
        .collect();
    assert_eq!(satisfied.len(), 2, "{bodies:?}");
    assert_ne!(
        satisfied[0]["state"]["diff"], satisfied[1]["state"]["diff"],
        "the second question is asked about the changed diff, not the cached one: {satisfied:?}"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_confident_yes_spares_the_fresh_checker_when_nothing_else_is_found() {
    let root = root("completion-yes");
    write_config(&root, DECISIONS_ON_WITH_CHECKER);
    let (endpoint, messages, decisions, _headers) = providers(
        vec![cell("c1", "return \"done\";")],
        vec![],
        vec![Decision::Answer(completion_answer(0.94))],
    );
    let result = exec_bounded(&root, &endpoint, "fix the bug", None)
        .expect("a spared checker still finishes the task");
    assert_eq!(
        messages.lock().unwrap().len(),
        1,
        "no second request reaches /v1/messages for the checker"
    );
    assert_eq!(result["telemetry"]["completion"]["verified"], true);
    let telemetry = &result["telemetry"]["decisions"]["completion"];
    assert_eq!(telemetry["noul"], 0.94, "{telemetry}");
    assert_eq!(telemetry["checker_skipped"], "decision 0.94", "{telemetry}");
    assert_eq!(
        decisions.lock().unwrap().len(),
        2,
        "the intent question, and the completion question"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn an_undecided_answer_runs_the_checker_as_today() {
    let root = root("completion-undecided");
    write_config(&root, DECISIONS_ON_WITH_CHECKER);
    let (endpoint, messages, _decisions, _headers) = providers(
        vec![
            cell("c1", "return \"done\";"),
            prose("The change holds; nothing more is needed."),
        ],
        vec![],
        vec![Decision::Answer(completion_answer(0.55))],
    );
    let result = exec_bounded(&root, &endpoint, "fix the bug", None)
        .expect("the checker runs and the task still finishes");
    assert_eq!(
        messages.lock().unwrap().len(),
        2,
        "the task turn, then the checker's own request"
    );
    assert_eq!(result["telemetry"]["completion"]["verified"], true);
    let telemetry = &result["telemetry"]["decisions"]["completion"];
    assert_eq!(telemetry["noul"], 0.55, "{telemetry}");
    assert!(telemetry["checker_skipped"].is_null(), "{telemetry}");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_yes_never_removes_a_mechanical_finding() {
    let root = root("completion-yes-with-finding");
    write_config(&root, DECISIONS_ON_WITH_CHECKER);
    write_checks_toml(&root, "[contract]\nrequired = [\"missing.txt\"]\n");
    let (endpoint, messages, _decisions, _headers) = providers(
        vec![
            cell("c1", "return \"done\";"),
            prose("The change holds; nothing more is needed."),
            cell("c2", "return \"done\";"),
        ],
        vec![],
        vec![Decision::Answer(completion_answer(0.94))],
    );
    let result = exec_bounded(&root, &endpoint, "fix the bug", None)
        .expect("a confident yes does not remove the required-path finding");
    assert_eq!(
        messages.lock().unwrap().len(),
        3,
        "the checker still runs, then the same claim finishes unverified"
    );
    let telemetry = &result["telemetry"]["decisions"]["completion"];
    assert_eq!(telemetry["noul"], 0.94, "{telemetry}");
    assert!(telemetry["checker_skipped"].is_null(), "{telemetry}");
    assert_eq!(telemetry["finding_added"], false, "{telemetry}");
    let completion = &result["telemetry"]["completion"];
    assert_eq!(completion["verified"], false, "{completion}");
    assert!(
        completion["findings"][0]
            .as_str()
            .unwrap()
            .contains("missing.txt"),
        "{completion}"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn shadow_records_the_completion_answer_and_changes_nothing() {
    let root = root("completion-shadow");
    write_config(&root, DECISIONS_SHADOW);
    let (endpoint, messages, _decisions, _headers) = providers(
        vec![cell("c1", "return \"done\";")],
        vec![],
        vec![Decision::Answer(completion_answer(0.06))],
    );
    let result = exec_bounded(&root, &endpoint, "fix the bug", None)
        .expect("shadow never holds the completion");
    assert_eq!(
        messages.lock().unwrap().len(),
        1,
        "shadow finishes on the first claim"
    );
    assert_eq!(result["telemetry"]["completion"]["verified"], true);
    let telemetry = &result["telemetry"]["decisions"]["completion"];
    assert_eq!(telemetry["noul"], 0.06, "{telemetry}");
    assert_eq!(telemetry["finding_added"], false, "{telemetry}");
    assert!(telemetry["checker_skipped"].is_null(), "{telemetry}");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_failed_or_slow_completion_decision_leaves_the_gate_as_it_is() {
    let failed = root("completion-500");
    write_config(&failed, DECISIONS_ON);
    let (endpoint, messages, _decisions, _headers) = providers(
        vec![cell("c1", "return \"done\";")],
        vec![],
        vec![Decision::Status(500)],
    );
    let result = exec_bounded(&failed, &endpoint, "fix the bug", None)
        .expect("a failed completion decision leaves the gate alone");
    assert_eq!(messages.lock().unwrap().len(), 1);
    assert_eq!(result["telemetry"]["completion"]["verified"], true);
    let telemetry = &result["telemetry"]["decisions"];
    assert_eq!(telemetry["failed"], 1, "{telemetry}");
    assert!(telemetry["completion"].is_null(), "{telemetry}");
    let _ = std::fs::remove_dir_all(failed);

    let slow = root("completion-slow");
    write_config(&slow, DECISIONS_ON);
    let (endpoint, messages, _decisions, _headers) = providers(
        vec![cell("c1", "return \"done\";")],
        vec![],
        vec![Decision::Sleep(Duration::from_secs(3))],
    );
    let result = exec_bounded(&slow, &endpoint, "fix the bug", None)
        .expect("a slow completion decision times out rather than hanging the task");
    assert_eq!(messages.lock().unwrap().len(), 1);
    assert_eq!(result["telemetry"]["completion"]["verified"], true);
    assert_eq!(result["telemetry"]["decisions"]["failed"], 1);
    let _ = std::fs::remove_dir_all(slow);
}

#[test]
fn a_large_diff_is_cut_at_a_hunk_boundary_and_still_asked() {
    let root = root("completion-large-diff");
    write_config(&root, DECISIONS_SHADOW);
    let mut code = String::new();
    for i in 0..40 {
        code.push_str(&format!(
            "await write({{path: \"f{i}.txt\", content: \"{}\"}});\n",
            "x".repeat(2_000)
        ));
    }
    code.push_str("return \"done\";");
    let (endpoint, messages, decisions, _headers) = providers(
        vec![cell("c1", &code)],
        vec![],
        vec![Decision::Answer(completion_answer(0.50))],
    );
    let result = exec_bounded(&root, &endpoint, "write many files", None)
        .expect("a large diff still gets a completion question");
    assert_eq!(messages.lock().unwrap().len(), 1, "shadow never holds");
    let bodies = decisions.lock().unwrap();
    assert_eq!(
        bodies.len(),
        2,
        "the intent question, then the completion question"
    );
    let satisfied: Value = serde_json::from_str(&bodies[1]).unwrap();
    assert_eq!(satisfied["state"]["diff_truncated"], true, "{satisfied}");
    let diff = satisfied["state"]["diff"].as_str().unwrap();
    assert!(
        diff.len() <= pane::decide::DIFF_STATE_BYTES,
        "bounded to {}: got {}",
        pane::decide::DIFF_STATE_BYTES,
        diff.len()
    );
    let telemetry = &result["telemetry"]["decisions"]["completion"];
    assert_eq!(telemetry["truncated"], true, "{telemetry}");
    let _ = std::fs::remove_dir_all(root);
}
