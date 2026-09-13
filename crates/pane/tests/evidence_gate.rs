//! Binary-level canaries for the evidence gate and the no-progress guard
//! (`smarter-cheaper-roadmap.md`, *Evidence-gated completion*,
//! *Final-state contract checker*, *No-progress guard*): the three known
//! Terminal-Bench final-state mistakes and a planted repeat, replayed through
//! the built `pane` binary against a loopback-only fake Messages provider.
//!
//! What is proved here is the session's behaviour, not the checker's rules —
//! those are `tests/final_state_contract.rs`. Every assertion below reads the
//! request bodies the binary actually sent and the machine result it wrote.
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::sync::{Arc, Mutex};

fn root(label: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "pane-gate-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    root
}

/// A fake provider answering each request in order and keeping every
/// request body, so a test can read what the model was actually shown.
fn providers(responses: Vec<Value>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&bodies);
    std::thread::spawn(move || {
        for response in responses {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(20)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut length = 0;
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
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            seen.lock()
                .unwrap()
                .push(String::from_utf8_lossy(&body).into_owned());
            let response = response.to_string();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).unwrap();
        }
    });
    (url, bodies)
}

fn cell(id: &str, code: &str) -> Value {
    json!({"role": "assistant", "content": [{"type": "tool_use", "id": id, "name": "execute_cell", "input": {"code": code}}],
        "usage": {"input_tokens": 20, "output_tokens": 7}})
}

fn prose(text: &str) -> Value {
    json!({"role": "assistant", "content": [{"type": "text", "text": text}],
        "usage": {"input_tokens": 20, "output_tokens": 7}})
}

fn exec_json(root: &std::path::Path, endpoint: &str) -> Value {
    exec_json_task(root, endpoint, "finish the deliverable")
}

fn exec_json_task(root: &std::path::Path, endpoint: &str, task: &str) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_pane"))
        .args(["exec", task, "--output-format", "json", "--root"])
        .arg(root)
        .args(["--model", "test/model", "--glasshouse"])
        .arg(root.join("absent-glasshouse"))
        .env("ANTHROPIC_BASE_URL", endpoint)
        .env("ANTHROPIC_API_KEY", "test-only")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

/// The polyglot mistake: the deliverable is written and a compiled binary is
/// left beside it. The first `return` is held with the finding; the same
/// return again finishes with the completion recorded unverified.
#[test]
fn a_stray_binary_beside_the_deliverable_holds_the_return_once_and_is_recorded_unverified() {
    let root = root("stray-binary");
    let (endpoint, bodies) = providers(vec![
        cell(
            "c1",
            "await write({path: \"polyglot/main.py.c\", content: \"int main(){return 0;}\\n\"});\n\
             await write({path: \"polyglot/cmain\", content: \"\\u007fELF compiled test binary\"});\n\
             return \"done\";",
        ),
        cell("c2", "return \"done\";"),
    ]);
    let result = exec_json(&root, &endpoint);
    let completion = &result["telemetry"]["completion"];
    assert_eq!(completion["claimed"], true, "{completion}");
    assert_eq!(completion["verified"], false, "{completion}");
    assert_eq!(completion["deferred"], 1, "{completion}");
    let findings = completion["findings"].as_array().unwrap();
    assert!(
        findings
            .iter()
            .any(|f| f.as_str().unwrap().contains("cmain")),
        "{findings:?}"
    );
    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2, "one held return, then the finish");
    assert!(
        bodies[1].contains("Candidate completion (deferred)") && bodies[1].contains("cmain"),
        "the second request carries the candidate and the finding: {}",
        bodies[1]
    );
    assert_eq!(result["answer"], "done");
    let _ = std::fs::remove_dir_all(root);
}

/// The gcov mistake: coverage data written away from the instrumented source.
#[test]
fn coverage_data_outside_the_source_tree_is_a_finding_before_completion() {
    let root = root("coverage-tree");
    std::fs::create_dir_all(root.join("sqlite/src")).unwrap();
    std::fs::write(
        root.join("sqlite/src/btree.c"),
        "int btree(void){return 1;}\n",
    )
    .unwrap();
    let (endpoint, bodies) = providers(vec![
        cell(
            "c1",
            "await write({path: \"sqlite-gcov-build/btree.gcno\", content: \"gcno\"});\n\
             await write({path: \"sqlite/src/btree.c\", content: \"int btree(void){return 2;}\\n\"});\n\
             return \"built with coverage\";",
        ),
        cell("c2", "return \"built with coverage\";"),
    ]);
    let result = exec_json(&root, &endpoint);
    let completion = &result["telemetry"]["completion"];
    assert_eq!(completion["verified"], false, "{completion}");
    let findings = completion["findings"].as_array().unwrap();
    assert!(
        findings
            .iter()
            .any(|f| f.as_str().unwrap().contains("btree.gcno")),
        "{findings:?}"
    );
    assert_eq!(bodies.lock().unwrap().len(), 2);
    let _ = std::fs::remove_dir_all(root);
}

/// A project that declares verification: a mutation with no check run is
/// held; without a declared check the same task completes verified, because
/// nothing declared an acceptance contract for a check.
#[test]
fn a_declared_check_that_never_ran_holds_the_return_and_an_undeclared_one_does_not() {
    let held = root("declared-check");
    std::fs::create_dir_all(held.join(".glasshouse")).unwrap();
    std::fs::write(
        held.join(".glasshouse/checks.toml"),
        "[checks.tests]\ncommand = \"true\"\n",
    )
    .unwrap();
    let (endpoint, bodies) = providers(vec![
        cell(
            "c1",
            "await write({path: \"src/lib.rs\", content: \"pub fn x() {}\\n\"});\nreturn \"done\";",
        ),
        cell("c2", "return \"done\";"),
    ]);
    let result = exec_json(&held, &endpoint);
    assert_eq!(result["telemetry"]["completion"]["verified"], false);
    assert!(
        bodies.lock().unwrap()[1].contains("Run a verification before finishing"),
        "{}",
        bodies.lock().unwrap()[1]
    );
    let _ = std::fs::remove_dir_all(held);

    let plain = root("undeclared-check");
    let (endpoint, bodies) = providers(vec![cell(
        "c1",
        "await write({path: \"src/lib.rs\", content: \"pub fn x() {}\\n\"});\nreturn \"done\";",
    )]);
    let result = exec_json(&plain, &endpoint);
    assert_eq!(result["telemetry"]["completion"]["verified"], true);
    assert_eq!(result["telemetry"]["completion"]["deferred"], 0);
    assert_eq!(bodies.lock().unwrap().len(), 1);
    let _ = std::fs::remove_dir_all(plain);
}

/// A planted repeat: the same failing call twice with an unchanged tree gets
/// one notice at the head of the next feedback, and it is counted.
#[test]
fn an_identical_failing_cell_repeated_is_noticed_once_and_counted() {
    let root = root("no-progress");
    let repeat = "const missing = await read({path: \"missing.txt\"});";
    let (endpoint, bodies) = providers(vec![
        cell("c1", repeat),
        cell("c2", repeat),
        cell("c3", "return \"gave up\";"),
    ]);
    let result = exec_json(&root, &endpoint);
    assert_eq!(result["telemetry"]["progress"]["no_progress_notices"], 1);
    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 3);
    assert!(
        !bodies[1].contains("repeated without progress"),
        "the first failure is not a repeat: {}",
        bodies[1]
    );
    assert!(
        bodies[2].contains("repeated without progress"),
        "the second identical failure is noticed: {}",
        bodies[2]
    );
    assert_eq!(result["answer"], "gave up");
    let _ = std::fs::remove_dir_all(root);
}

/// The capsule reaches the parent as a `## Task` block once it has something
/// to say, and rides the result as telemetry.
#[test]
fn the_task_capsule_reaches_the_feedback_and_the_result() {
    let root = root("capsule");
    let (endpoint, bodies) = providers(vec![
        cell(
            "c1",
            "await write({path: \"notes.txt\", content: \"hello\\n\"});",
        ),
        cell("c2", "return \"done\";"),
    ]);
    let result = exec_json(&root, &endpoint);
    let capsule = &result["telemetry"]["capsule"];
    assert!(capsule.is_object(), "{result}");
    assert_eq!(capsule["goal"], "finish the deliverable", "{capsule}");
    let bodies = bodies.lock().unwrap();
    assert!(
        bodies[1].contains("## Task") && bodies[1].contains("notes.txt"),
        "{}",
        bodies[1]
    );
    let _ = std::fs::remove_dir_all(root);
}

/// The shape the first hybrid Terminal-Bench trial showed on 2026-09-13: the
/// model returns a structured value (notebook output, not terminal) and then
/// finishes in prose. The gate covers that prose completion the same way:
/// held once with the finding, then recorded unverified.
#[test]
fn a_prose_completion_after_a_stray_binary_is_held_once_and_recorded_unverified() {
    let root = root("prose-stray-binary");
    let (endpoint, bodies) = providers(vec![
        cell(
            "c1",
            "await write({path: \"polyglot/main.py.c\", content: \"int main(){return 0;}\\n\"});\n\
             await write({path: \"polyglot/cmain\", content: \"\\u007fELF compiled test binary\"});\n\
             return {ok: true};",
        ),
        prose("Done: the deliverable is written."),
        prose("Done: the deliverable is written."),
    ]);
    let result = exec_json(&root, &endpoint);
    let completion = &result["telemetry"]["completion"];
    assert_eq!(completion["claimed"], true, "{completion}");
    assert_eq!(completion["verified"], false, "{completion}");
    assert_eq!(completion["deferred"], 1, "{completion}");
    assert!(
        completion["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f.as_str().unwrap().contains("cmain")),
        "{completion}"
    );
    let bodies = bodies.lock().unwrap();
    assert_eq!(
        bodies.len(),
        3,
        "the structured return, one held prose completion, then the finish"
    );
    assert!(
        bodies[2].contains("Candidate completion (deferred)") && bodies[2].contains("cmain"),
        "the held prose completion reaches the model with the finding: {}",
        bodies[2]
    );
    assert_eq!(result["answer"], "Done: the deliverable is written.");
    let _ = std::fs::remove_dir_all(root);
}

/// A prose completion with nothing to find completes at once, and the
/// completion is now recorded (it was absent for prose before 2026-09-13).
#[test]
fn a_prose_completion_with_nothing_to_find_completes_verified_in_one_turn() {
    let root = root("prose-clean");
    let (endpoint, bodies) = providers(vec![
        cell(
            "c1",
            "await write({path: \"notes.txt\", content: \"hello\\n\"});",
        ),
        prose("Done: notes written."),
    ]);
    let result = exec_json(&root, &endpoint);
    let completion = &result["telemetry"]["completion"];
    assert_eq!(completion["claimed"], true, "{completion}");
    assert_eq!(completion["verified"], true, "{completion}");
    assert_eq!(completion["deferred"], 0, "{completion}");
    assert_eq!(bodies.lock().unwrap().len(), 2);
    assert_eq!(result["answer"], "Done: notes written.");
    let _ = std::fs::remove_dir_all(root);
}

/// The request-derived acceptance list (`acceptance.rs`): with helpers
/// configured, the lister answers first, its items are shown to the model
/// before its first turn, and an item the finished tree does not meet holds
/// the completion once with the item and what was observed.
#[test]
fn an_unmet_acceptance_item_holds_the_completion_with_what_was_observed() {
    let root = root("acceptance");
    std::fs::create_dir_all(root.join(".pane")).unwrap();
    std::fs::write(
        root.join(".pane/config.toml"),
        "[helpers]\nmodel = \"test/helper\"\npreflight = false\n",
    )
    .unwrap();
    let (endpoint, bodies) = providers(vec![
        // The lister's answer: two file items and one judge item.
        prose(
            "file: out/a.txt exists\nfile: out/b.txt exists\njudge: the files are named as requested",
        ),
        cell(
            "c1",
            "await write({path: \"out/a.txt\", content: \"a\\n\"});\nreturn {ok: true};",
        ),
        prose("Done: both files are written."),
        prose("Done: both files are written."),
    ]);
    // Four words or more: a shorter request needs no repository and gets no
    // lister, the same rule as the preflight.
    let result = exec_json_task(&root, &endpoint, "write out/a.txt and out/b.txt for me");
    let bodies = bodies.lock().unwrap();
    assert_eq!(
        bodies.len(),
        4,
        "lister, first turn, held completion, finish"
    );
    assert!(
        bodies[0].contains("acceptance items"),
        "the first request is the lister's: {}",
        &bodies[0][..bodies[0].len().min(600)]
    );
    assert!(
        bodies[1].contains("## Acceptance list") && bodies[1].contains("file `out/b.txt` exists"),
        "the model sees the list before its first turn: {}",
        bodies[1]
    );
    assert!(
        bodies[3].contains("Acceptance item not met: file `out/b.txt` exists — absent"),
        "the held completion names the unmet item: {}",
        bodies[3]
    );
    assert!(
        !bodies[3].contains("out/a.txt` exists — absent"),
        "the met item is not a finding: {}",
        bodies[3]
    );
    let completion = &result["telemetry"]["completion"];
    assert_eq!(completion["verified"], false, "{completion}");
    assert_eq!(completion["deferred"], 1, "{completion}");
    let acceptance = &result["telemetry"]["acceptance"];
    assert_eq!(acceptance["items"], 3, "{acceptance}");
    assert_eq!(acceptance["met"], 1, "{acceptance}");
    assert_eq!(acceptance["unmet"], 1, "{acceptance}");
    assert_eq!(acceptance["judged"], 1, "{acceptance}");
    let _ = std::fs::remove_dir_all(root);
}

/// The stall notice (`progress::Stall`): six cells in a row that change
/// nothing — no tree change, no new fact, no verification — get one notice
/// at the head of the next feedback, counted, and the task goes on.
#[test]
fn six_cells_without_progress_get_one_stall_notice_and_the_task_continues() {
    let root = root("stall");
    let mut responses = vec![cell(
        "c1",
        "await write({path: \"notes.txt\", content: \"hello\\n\"});",
    )];
    for i in 0..6 {
        responses.push(cell(
            &format!("r{i}"),
            &format!("const look{i} = await read({{path: \"notes.txt\"}});"),
        ));
    }
    responses.push(prose("Done: read it six times."));
    let (endpoint, bodies) = providers(responses);
    let result = exec_json(&root, &endpoint);
    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 8);
    assert!(
        !bodies[6].contains("No progress for"),
        "five idle cells are not yet a stall: {}",
        bodies[6]
    );
    assert!(
        bodies[7].contains("No progress for 6 cells"),
        "the sixth idle cell is noticed: {}",
        bodies[7]
    );
    assert_eq!(result["telemetry"]["progress"]["stall_notices"], 1);
    assert_eq!(result["telemetry"]["progress"]["no_progress_notices"], 0);
    assert_eq!(result["answer"], "Done: read it six times.");
    let _ = std::fs::remove_dir_all(root);
}
