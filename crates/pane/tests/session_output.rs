//! End-to-end machine output against a loopback-only fake Messages provider.
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::Command;

fn root(label: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "pane-output-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn provider(status: u16, response: Value) -> String {
    providers(vec![(status, response)])
}

fn providers(responses: Vec<(u16, Value)>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for (status, response) in responses {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(10)))
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
            let request: Value = serde_json::from_slice(&body).unwrap();
            assert!(request["messages"].is_array());
            let response = response.to_string();
            write!(stream, "HTTP/1.1 {status} Reply\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).unwrap();
        }
    });
    url
}

fn run(format: &str, status: u16, reply: Value) -> std::process::Output {
    let root = root(format);
    let endpoint = provider(status, reply);
    let output = Command::new(env!("CARGO_BIN_EXE_pane"))
        .args(["exec", "compute", "--output-format", format, "--root"])
        .arg(&root)
        .args(["--model", "test/model", "--glasshouse"])
        .arg(root.join("absent-glasshouse"))
        .env("ANTHROPIC_BASE_URL", endpoint)
        .env("ANTHROPIC_API_KEY", "test-only")
        .output()
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
    output
}

fn native_return() -> Value {
    json!({"role": "assistant", "content": [{"type": "tool_use", "id": "call-answer", "name": "execute_cell", "input": {"code": "return 'answer 42';"}}],
        "usage": {"input_tokens": 20, "output_tokens": 7, "cache_read_input_tokens": 4,
            "cache_creation_input_tokens": 1}})
}

#[test]
fn json_returns_one_document_with_typed_records_and_terminal_answer() {
    let output = run("json", 200, native_return());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["schema_version"], 1);
    assert_eq!(result["type"], "result");
    assert_eq!(result["success"], true);
    assert_eq!(result["answer"], "answer 42");
    assert_eq!(result["telemetry"]["tokens"]["known_total"], 32);
    assert_eq!(result["telemetry"]["tokens"]["parent"]["requests"], 1);
    assert_eq!(result["telemetry"]["tokens"]["parent"]["input_tokens"], 20);
    assert_eq!(result["telemetry"]["tokens"]["parent"]["output_tokens"], 7);
    assert_eq!(
        result["telemetry"]["tokens"]["parent"]["cache_read_input_tokens"],
        4
    );
    assert_eq!(
        result["telemetry"]["tokens"]["parent"]["cache_creation_input_tokens"],
        1
    );
    assert_eq!(
        result["telemetry"]["tokens"]["parent"]["models"][0]["model"],
        "test/model"
    );
    assert_eq!(
        result["telemetry"]["provider_requests"]["coverage_complete"],
        true
    );
    assert_eq!(result["telemetry"]["cells"]["executed"], 1);
    assert!(result["telemetry"]["wall_time_ms"].is_u64());
    let events = result["events"].as_array().unwrap();
    assert_eq!(events[0]["type"], "session_started");
    assert!(events.iter().any(|event| event["type"] == "cell"));
    assert!(
        events
            .iter()
            .any(|event| event["data"]["content"][0]["id"] == "call-answer")
    );
}

#[test]
fn stream_json_is_ordered_jsonl_ending_in_a_result() {
    let output = run("stream-json", 200, native_return());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    let events: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    for (sequence, event) in events.iter().enumerate() {
        assert_eq!(event["sequence"], sequence);
        assert_eq!(event["schema_version"], 1);
    }
    assert_eq!(events.last().unwrap()["type"], "result");
    assert_eq!(events.last().unwrap()["answer"], "answer 42");
    assert_eq!(events.last().unwrap()["success"], true);
    assert_eq!(
        events.last().unwrap()["telemetry"]["tokens"]["known_total"],
        32
    );
    assert_eq!(
        events.last().unwrap()["telemetry"]["provider_requests"]["total"],
        1
    );
}

#[test]
fn provider_failure_produces_machine_error_and_nonzero_exit() {
    for format in ["json", "stream-json"] {
        let output = run(
            format,
            401,
            json!({"error": {"type": "authentication_error", "message": "fixture denied"}}),
        );
        assert!(!output.status.success());
        let text = String::from_utf8(output.stdout).unwrap();
        let result: Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
        assert_eq!(result["type"], "result");
        assert_eq!(result["success"], false);
        assert_eq!(result["telemetry"]["provider_requests"]["total"], 1);
        assert_eq!(result["telemetry"]["provider_requests"]["reported"], 0);
        assert_eq!(
            result["telemetry"]["provider_requests"]["coverage_complete"],
            false
        );
        assert!(
            result["error"]
                .as_str()
                .is_some_and(|error| error.contains("401"))
        );
    }
}

#[test]
fn machine_telemetry_splits_preflight_helper_and_parent_usage_by_model() {
    let root = root("preflight-telemetry");
    std::fs::create_dir_all(root.join(".pane")).unwrap();
    std::fs::write(
        root.join(".pane/config.toml"),
        "[helpers]\nmodel = \"helper/model\"\npreflight = true\n",
    )
    .unwrap();
    let helper = json!({
        "role": "assistant",
        "content": [{"type": "text", "text": "```pane\nreturn 'fixture.rs:1 relevant';\n```"}],
        "usage": {"input_tokens": 10, "output_tokens": 5, "cache_read_input_tokens": 3,
            "cache_creation_input_tokens": 2}
    });
    let endpoint = providers(vec![(200, helper), (200, native_return())]);
    let output = Command::new(env!("CARGO_BIN_EXE_pane"))
        .args([
            "exec",
            "find the relevant fixture and report the answer",
            "--output-format",
            "json",
            "--root",
        ])
        .arg(&root)
        .args(["--model", "test/model", "--glasshouse"])
        .arg(root.join("absent-glasshouse"))
        .env("ANTHROPIC_BASE_URL", endpoint)
        .env("ANTHROPIC_API_KEY", "test-only")
        .output()
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    let telemetry = &result["telemetry"];
    assert_eq!(telemetry["provider_requests"]["total"], 2);
    assert_eq!(telemetry["tokens"]["known_total"], 52);
    assert_eq!(telemetry["tokens"]["parent"]["known_tokens"], 32);
    assert_eq!(telemetry["tokens"]["helpers"]["known_tokens"], 20);
    assert_eq!(telemetry["tokens"]["helpers"]["calls"], 1);
    assert_eq!(
        telemetry["tokens"]["helpers"]["models"][0]["model"],
        "helper/model"
    );
    assert_eq!(telemetry["preflight_helpers"].as_array().unwrap().len(), 1);
    assert_eq!(telemetry["preflight_helpers"][0]["call_site"], "preflight");
    assert_eq!(
        telemetry["preflight_helpers"][0]["record"]["helper"],
        "find"
    );
    assert!(
        result["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["type"] == "helper" && event["data"]["call_site"] == "preflight")
    );
}

#[test]
fn machine_telemetry_counts_failed_cells_and_tool_calls() {
    let root = root("failed-tool-telemetry");
    let failed = json!({
        "role": "assistant",
        "content": [{"type": "tool_use", "id": "call-fail", "name": "execute_cell",
            "input": {"code": "await read({path:'definitely-absent'});"}}],
        "usage": {"input_tokens": 3, "output_tokens": 2,
            "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0}
    });
    let endpoint = providers(vec![(200, failed), (200, native_return())]);
    let output = Command::new(env!("CARGO_BIN_EXE_pane"))
        .args(["exec", "compute", "--output-format", "json", "--root"])
        .arg(&root)
        .args(["--model", "test/model", "--glasshouse"])
        .arg(root.join("absent-glasshouse"))
        .env("ANTHROPIC_BASE_URL", endpoint)
        .env("ANTHROPIC_API_KEY", "test-only")
        .output()
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["telemetry"]["cells"]["executed"], 2);
    assert_eq!(result["telemetry"]["cells"]["failed"], 1);
    assert_eq!(result["telemetry"]["tools"]["calls"], 1);
    assert_eq!(result["telemetry"]["tools"]["failures"], 1);
}

#[test]
fn machine_output_requires_a_single_task_without_starting_a_session() {
    let output = Command::new(env!("CARGO_BIN_EXE_pane"))
        .args(["--output-format", "json"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["success"], false);
    assert!(
        result["error"]
            .as_str()
            .unwrap()
            .contains("requires --task")
    );
}
