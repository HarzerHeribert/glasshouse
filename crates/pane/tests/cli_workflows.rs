//! CLI routing and offline diagnostics do not require a provider or gateway.
#[path = "../src/cli_workflows.rs"]
#[allow(dead_code)]
mod cli_workflows;

use std::process::{Command, Stdio};

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).into()).collect()
}

#[test]
fn print_and_exec_preserve_task_boundaries_and_session_options() {
    for input in [
        strings(&["-p", "fix this\nthen test", "--model", "provider/model"]),
        strings(&["exec", "fix this\nthen test", "--model", "provider/model"]),
    ] {
        let args = cli_workflows::prepare(&input).unwrap().unwrap();
        let value = |key: &str| {
            args.iter()
                .position(|arg| arg == key)
                .map(|i| args[i + 1].as_str())
        };
        assert_eq!(value("--task"), Some("fix this\nthen test"));
        assert_eq!(value("--model"), Some("provider/model"));
        assert_eq!(value("--root"), Some("."));
    }
}

#[test]
fn history_and_continue_route_to_existing_session_implementation() {
    assert_eq!(
        cli_workflows::prepare(&strings(&["--continue"])).unwrap(),
        Some(strings(&["--resume=", "--root", "."]))
    );
    assert_eq!(
        cli_workflows::prepare(&strings(&["--resume", "session-id", "--root=/repo"])).unwrap(),
        Some(strings(&["--resume", "session-id", "--root=/repo"]))
    );
    assert_eq!(
        cli_workflows::prepare(&strings(&["--sessions"])).unwrap(),
        Some(strings(&["--sessions", "--root", "."]))
    );
    assert_eq!(cli_workflows::prepare(&strings(&["typo"])).unwrap(), None);
    assert!(cli_workflows::prepare(&strings(&["-p"])).is_err());
}

#[test]
fn exec_rejects_empty_stdin_instead_of_starting_an_interactive_session() {
    let root = std::env::temp_dir().join(format!(
        "pane-cli-workflows-empty-stdin-{}",
        std::process::id()
    ));
    let output = Command::new(env!("CARGO_BIN_EXE_pane"))
        .arg("exec")
        .stdin(Stdio::null())
        .env("XDG_CONFIG_HOME", root.join("global-config"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("empty task"));
}

#[test]
fn doctor_json_reports_invalid_config_without_exposing_its_contents() {
    let root = std::env::temp_dir().join(format!(
        "pane-doctor-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join(".glasshouse")).unwrap();
    std::fs::write(
        root.join(".glasshouse/pane.toml"),
        "SECRET_SHOULD_NOT_APPEAR [ invalid",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_pane"))
        .args(["doctor", "--json", "--root"])
        .arg(&root)
        .env("ANTHROPIC_BASE_URL", "https://SECRET_ENDPOINT.invalid")
        .env("XDG_CONFIG_HOME", root.join("global-config"))
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["ok"], false);
    assert!(
        report["checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check["name"] == "config" && check["status"] == "error")
    );
    assert!(!stdout.contains("SECRET_"));
    assert!(root.join(".glasshouse/pane.toml").exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn doctor_missing_root_is_a_structured_failure() {
    let root = std::env::temp_dir().join(format!("pane-nonexistent-doctor-{}", std::process::id()));
    let output = Command::new(env!("CARGO_BIN_EXE_pane"))
        .args(["doctor", "--json", "--root"])
        .env("XDG_CONFIG_HOME", root.join("global-config"))
        .arg(root)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["checks"][0]["name"], "project");
    assert_eq!(report["checks"][0]["status"], "error");
}
