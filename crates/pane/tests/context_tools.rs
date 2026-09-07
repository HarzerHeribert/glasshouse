use pane::contract::SessionId;
use pane::glasshouse::Glasshouse;
use pane::runtime::isolate::Runtime;
use pane::runtime::outcome::{CellOutcome, Ended};
use pane::sandbox::profile::Profile;

fn fixture(label: &str) -> std::path::PathBuf {
    let root =
        std::env::temp_dir().join(format!("pane-context-tool-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    root
}

#[test]
fn context_and_edit_form_a_versioned_visible_edit_loop() {
    let root = fixture("success");
    let path = root.join("src/limits.py");
    std::fs::write(&path, "def clamp(value):\n    return value\n").unwrap();
    let profile = Profile::compile(&root, None);
    let mut runtime = Runtime::new(&profile, &Glasshouse::None, &SessionId::new("context-edit"));

    let first = runtime.run_cell(&format!(
        "const ctx = await context({{path:{path:?}, symbol:\"clamp\"}});\nconsole.log(`context-symbol=${{ctx.symbol}}`);"
    ));
    assert!(first.turn().stdout_tail.contains("def clamp(value)"));
    assert!(first.turn().stdout_tail.contains("context-symbol=clamp"));
    assert!(first.turn().stdout_tail.contains("version:"));
    let call = &first.turn().record.calls[0];
    assert_eq!(call.tool, "context");
    assert_eq!(call.ended, Ended::Ok);
    let evidence = call.evidence.as_ref().expect("context records visibility");
    assert!(!first.turn().stdout_tail.contains(&evidence.sha256));
    assert_eq!(evidence.path, "src/limits.py");
    assert!(evidence.complete);
    assert_eq!(evidence.ranges[0].start, 1);
    assert_eq!(evidence.ranges[0].end, 2);

    let second = runtime.run_cell(&format!(
        "const changed = await edit({{path:{path:?}, old:\"    return value\", replacement:\"    return Math.max(0, value)\"}});\nreturn changed.after_sha256;"
    ));
    assert!(matches!(second, CellOutcome::Returned { .. }), "{second:?}");
    assert_eq!(second.turn().record.calls[0].tool, "edit");
    assert_eq!(second.turn().record.calls[0].ended, Ended::Ok);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "def clamp(value):\n    return Math.max(0, value)\n"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn context_cannot_be_used_for_a_semantic_edit_before_it_reaches_the_model() {
    let root = fixture("same-cell");
    let path = root.join("src/value.py");
    std::fs::write(&path, "value = 1\n").unwrap();
    let profile = Profile::compile(&root, None);
    let mut runtime = Runtime::new(&profile, &Glasshouse::None, &SessionId::new("same-cell"));
    let result = runtime.run_cell(&format!(
        "const ctx = await context({{path:{path:?}}});\nawait edit({{path:{path:?}, old:\"value = 1\", replacement:\"value = 2\"}});"
    ));
    assert!(matches!(result, CellOutcome::Yielded { .. }), "{result:?}");
    assert_eq!(result.turn().record.calls.len(), 1);
    assert_eq!(result.turn().record.calls[0].tool, "context");
    assert!(
        result
            .turn()
            .yield_reason
            .as_deref()
            .unwrap()
            .contains("next turn")
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "value = 1\n");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn an_edit_from_stale_visible_context_throws_and_writes_nothing() {
    let root = fixture("stale");
    let path = root.join("src/value.py");
    std::fs::write(&path, "value = 1\n").unwrap();
    let profile = Profile::compile(&root, None);
    let mut runtime = Runtime::new(&profile, &Glasshouse::None, &SessionId::new("stale-edit"));
    runtime.run_cell(&format!("const ctx = await context({{path:{path:?}}});"));
    std::fs::write(&path, "value = 2\n").unwrap();

    let result = runtime.run_cell(&format!(
        "await edit({{path:{path:?}, expected_sha256:ctx.sha256, old:\"value = 1\", replacement:\"value = 3\"}});"
    ));
    assert!(matches!(result, CellOutcome::Threw { .. }), "{result:?}");
    assert_eq!(
        result.turn().record.calls[0].ended,
        Ended::Threw {
            class: "ToolError".into()
        }
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "value = 2\n");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn broad_read_of_one_large_stub_is_promoted_to_visible_context() {
    let root = fixture("promoted-read");
    let path = root.join("src/value.py");
    let source = format!(
        "def value():\n    raise NotImplementedError('stub')\n{}",
        "# far padding\n".repeat(2_000)
    );
    std::fs::write(&path, source).unwrap();
    let profile = Profile::compile(&root, None);
    let mut runtime = Runtime::new(
        &profile,
        &Glasshouse::None,
        &SessionId::new("promoted-read"),
    );

    let first = runtime.run_cell(&format!(
        "const source = await read({{path:{path:?}}});\nconsole.log(source.text);"
    ));
    assert_eq!(first.turn().record.calls[0].tool, "context");
    assert!(
        first.turn().record.calls[0]
            .evidence
            .as_ref()
            .is_some_and(|evidence| evidence.complete),
        "{first:?}"
    );
    assert!(first.turn().stdout_tail.contains("symbol: value"));
    assert!(!first.turn().stdout_tail.contains("far padding"));

    let second = runtime.run_cell(&format!(
        "await edit({{path:{path:?}, old:\"    raise NotImplementedError('stub')\", replacement:\"    return 1\"}});"
    ));
    assert_eq!(second.turn().record.calls[0].tool, "edit");
    assert!(std::fs::read_to_string(&path).unwrap().contains("return 1"));
    let _ = std::fs::remove_dir_all(root);
}
