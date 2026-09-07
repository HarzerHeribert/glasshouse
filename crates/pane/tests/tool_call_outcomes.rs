use pane::contract::SessionId;
use pane::glasshouse::Glasshouse;
use pane::runtime::isolate::Runtime;
use pane::runtime::outcome::Ended;
use pane::sandbox::profile::Profile;

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn typed_tool_failures_and_bash_exit_codes_have_truthful_call_outcomes() {
    let root = std::env::temp_dir().join(format!("pane-tool-call-outcomes-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("directory")).unwrap();
    let profile = Profile::compile(
        &root,
        Some(r#"{"permissions":{"allow":["Read(**)","Bash"]}}"#),
    );
    let mut runtime = Runtime::new(
        &profile,
        &Glasshouse::None,
        &SessionId::new("tool-call-outcomes"),
    );

    let directory = root.join("directory");
    let read = runtime.run_cell(&format!(
        r#"try {{ await read({{path:{directory:?}}}); }} catch (_) {{}}"#
    ));
    let read_calls = &read.turn().record.calls;
    assert_eq!(read_calls.len(), 1);
    assert_eq!(read_calls[0].tool, "read");
    assert_eq!(
        read_calls[0].ended,
        Ended::Threw {
            class: "ToolError".into()
        }
    );

    let bash = runtime.run_cell(r#"await bash({command:"exit 7"});"#);
    let bash_calls = &bash.turn().record.calls;
    assert_eq!(bash_calls.len(), 1);
    assert_eq!(bash_calls[0].tool, "bash");
    assert_eq!(bash_calls[0].ended, Ended::Ok);

    let _ = std::fs::remove_dir_all(root);
}
