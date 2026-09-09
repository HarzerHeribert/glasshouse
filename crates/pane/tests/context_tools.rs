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

#[test]
fn related_sources_can_be_inspected_together_then_edited_and_verified_together() {
    let root = fixture("batch");
    std::fs::write(root.join("src/a.py"), "value = 1\n").unwrap();
    std::fs::write(root.join("src/b.py"), "value = 2\n").unwrap();
    let profile = Profile::compile(
        &root,
        Some(r#"{"permissions":{"allow":["Read(**)","Write(**)","Bash"]}}"#),
    );
    let mut runtime = Runtime::new(&profile, &Glasshouse::None, &SessionId::new("batch"));
    let inspected = runtime
        .run_cell("const a = context({path:'src/a.py'}); const b = context({path:'src/b.py'});");
    assert_eq!(inspected.turn().record.calls.len(), 2, "{inspected:?}");
    assert!(inspected.turn().stdout_tail.contains("value = 1"));
    assert!(inspected.turn().stdout_tail.contains("value = 2"));
    let changed = runtime.run_cell(r#"
        edit({path:'src/a.py', old:'value = 1', replacement:'value = 10'});
        edit({path:'src/b.py', old:'value = 2', replacement:'value = 20'});
        write({path:'check.sh', lines:['test "$(< src/a.py)" = "value = 10" && test "$(< src/b.py)" = "value = 20"']});
        const verified = bash({command:'source check.sh'});
        if (verified.exit_code !== 0) throw new Error("verification failed");
        return verified.exit_code;
    "#);
    assert!(
        matches!(changed, CellOutcome::Returned { .. }),
        "{changed:?}"
    );
    assert_eq!(changed.turn().record.calls.len(), 4, "{changed:?}");
    assert!(
        changed
            .turn()
            .record
            .calls
            .iter()
            .all(|c| c.ended == Ended::Ok),
        "{changed:?}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("src/a.py")).unwrap(),
        "value = 10\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("src/b.py")).unwrap(),
        "value = 20\n"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn refreshing_changed_source_replaces_the_implicit_edit_version() {
    let root = fixture("refresh");
    let path = root.join("src/value.py");
    std::fs::write(&path, "value = 1\n").unwrap();
    let profile = Profile::compile(&root, None);
    let mut runtime = Runtime::new(&profile, &Glasshouse::None, &SessionId::new("refresh"));
    runtime.run_cell("context({path:'src/value.py'});");
    runtime.run_cell("edit({path:'src/value.py', old:'value = 1', replacement:'value = 2'});");
    runtime.run_cell("context({path:'src/value.py'});");
    let changed =
        runtime.run_cell("edit({path:'src/value.py', old:'value = 2', replacement:'value = 3'});");
    assert_eq!(changed.turn().record.calls.len(), 1, "{changed:?}");
    assert_eq!(
        changed.turn().record.calls[0].ended,
        Ended::Ok,
        "{changed:?}"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "value = 3\n");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_full_context_batch_preserves_whole_evidence_and_does_not_certify_overflow() {
    let root = fixture("batch-cap");
    for name in ["a", "b", "c"] {
        let body = format!(
            "# {name}-BEGIN\n{}# {name}-END\nvalue = 1\n",
            format!("# {}\n", "x".repeat(120)).repeat(100)
        );
        std::fs::write(root.join(format!("src/{name}.py")), body).unwrap();
    }
    let profile = Profile::compile(&root, None);
    let mut runtime = Runtime::new(&profile, &Glasshouse::None, &SessionId::new("batch-cap"));
    let inspected = runtime.run_cell("console.log('z'.repeat(100000)); const a = await context({path:'src/a.py'}); const b = await context({path:'src/b.py'}); const c = await context({path:'src/c.py'});");
    assert_eq!(inspected.turn().record.calls.len(), 3, "{inspected:?}");
    for marker in ["a-BEGIN", "a-END", "b-BEGIN", "b-END"] {
        assert!(
            inspected.turn().stdout_tail.contains(marker),
            "lost {marker}"
        );
    }
    assert!(!inspected.turn().stdout_tail.contains("c-BEGIN"));
    assert!(inspected.turn().record.calls[2].evidence.is_none());
    let feedback = pane::prompt::CellResult {
        cell: 1,
        elapsed_ms: inspected.turn().elapsed_ms,
        error: None,
        yield_reason: inspected.turn().yield_reason.clone(),
        output: None,
        handle_table: inspected.turn().table.clone(),
        stdout_tail: Some(inspected.turn().stdout_tail.clone()),
        plan: Vec::new(),
        budget: pane::prompt::Budget {
            turn_cap: 8192,
            task_used: 0,
            task_cap: 0,
            cells_used: 1,
            cells_cap: 40,
        },
    };
    use pane::contract::{Block, Conversation, Message, Role};
    let mut call = Message::text(Role::Assistant, "");
    call.content = vec![Block::ToolUse {
        id: "batch".into(),
        name: "execute_cell".into(),
        input: serde_json::json!({"code":"context batch"}),
    }];
    let mut conversation = Conversation {
        system: String::new(),
        messages: vec![
            Message::text(Role::User, "edit these files"),
            call,
            Message::runtime_tool_result(
                "batch",
                pane::prompt::render_result(&feedback),
                false,
                pane::prompt::render_result_history(&feedback),
            ),
        ],
    };
    for older in [false, true] {
        if older {
            conversation
                .messages
                .push(Message::text(Role::Assistant, "a later inspection"));
            conversation
                .messages
                .push(Message::runtime("later result", "later history"));
        }
        let projected =
            pane::prompt::with_task_context(&conversation, "test-model", "edit these files");
        let wire: serde_json::Value =
            serde_json::from_slice(&pane::wire::request_body_on_model(&projected, "test-model"))
                .unwrap();
        let delivered = wire["messages"][2]["content"][0]["content"]
            .as_str()
            .unwrap();
        for marker in ["a-BEGIN", "a-END", "b-BEGIN", "b-END"] {
            assert!(
                delivered.contains(marker),
                "provider lost {marker}, older={older}"
            );
        }
        assert!(!delivered.contains("c-BEGIN"));
        assert!(!delivered.contains("c-END"));
    }

    assert!(inspected.turn().stdout_dropped_tokens > 0);
    let refused = runtime
        .run_cell("await edit({path:'src/c.py', old:'value = 1', replacement:'value = 2'});");
    assert!(refused.turn().record.calls.is_empty(), "{refused:?}");
    assert!(
        std::fs::read_to_string(root.join("src/c.py"))
            .unwrap()
            .ends_with("value = 1\n")
    );
    runtime.run_cell("await context({path:'src/c.py'});");
    let changed = runtime
        .run_cell("await edit({path:'src/c.py', old:'value = 1', replacement:'value = 2'});");
    assert_eq!(changed.turn().record.calls[0].ended, Ended::Ok);
    let _ = std::fs::remove_dir_all(root);
}
