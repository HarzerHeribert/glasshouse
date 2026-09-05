//! Acceptance tests for GH-PANE-61E-HANDLES against
//! `docs/product/pane/runtime-contract.md` §2 and §3.

use pane::runtime::handles::{HandleTable, render_table};
use pane::runtime::preview::{
    ErrorValue, FileValue, PREVIEW_TOKEN_CAP, StackFrame, TABLE_TOKEN_CAP, TestReportValue, Value,
};

/// The class-C consumer link: a golden file over a table containing one of
/// every §3 type. Compared with `\r\n` normalised to `\n` on both sides so a
/// Windows checkout of the fixture cannot fail this for a reason that has
/// nothing to do with the renderer.
#[test]
fn the_handle_table_renders_byte_for_byte() {
    let mut table = HandleTable::new();
    table.declare(
        "arr",
        Value::Array(vec![
            Value::Number(10.0),
            Value::Number(20.0),
            Value::Number(30.0),
            Value::Number(40.0),
            Value::Number(50.0),
        ]),
        1,
    );
    table.declare(
        "file",
        Value::File(FileValue {
            path: "src/lib.rs".into(),
            byte_len: 120,
            line_count: 10,
            mtime: "2026-09-05T00:00:00Z".into(),
            lines: vec!["fn main() {}".into(), "// second line".into()],
        }),
        1,
    );
    table.declare(
        "report",
        Value::TestReport(TestReportValue {
            passed: 3,
            failed: 1,
            skipped: 0,
            failing_names: vec!["test_x".into()],
            log: "SHOULD NEVER APPEAR".into(),
        }),
        1,
    );
    table.declare("str", Value::String("hello world".into()), 1);
    table.declare("num", Value::Number(42.0), 1);
    table.declare("flag", Value::Boolean(true), 1);
    table.declare("empty", Value::Null, 1);
    table.declare("missing", Value::Undefined, 1);
    table.declare(
        "obj",
        Value::Object(vec![
            ("a".to_string(), Value::Number(1.0)),
            ("b".to_string(), Value::String("x".into())),
        ]),
        1,
    );
    table.declare(
        "err",
        Value::Error(ErrorValue {
            class: "TypeError".into(),
            message: "bad thing".into(),
            stack: vec![StackFrame {
                description: "cell 2, line 3".into(),
            }],
        }),
        1,
    );

    let rendered = render_table(&table, PREVIEW_TOKEN_CAP, TABLE_TOKEN_CAP);
    let golden = include_str!("fixtures/handle_table.golden").replace("\r\n", "\n");
    assert_eq!(rendered, golden);
}

#[test]
fn a_file_preview_never_contains_the_file_contents() {
    let mut table = HandleTable::new();
    table.declare(
        "f",
        Value::File(FileValue {
            path: "notes.txt".into(),
            byte_len: 1000,
            line_count: 5,
            mtime: "2026-09-05T00:00:00Z".into(),
            lines: vec![
                "line one".into(),
                "line two".into(),
                "SECRET-CONTENTS-line-three".into(),
                "SECRET-CONTENTS-line-four".into(),
                "SECRET-CONTENTS-line-five".into(),
            ],
        }),
        1,
    );

    let rendered = render_table(&table, PREVIEW_TOKEN_CAP, TABLE_TOKEN_CAP);
    assert!(rendered.contains("line one"));
    assert!(rendered.contains("line two"));
    assert!(!rendered.contains("SECRET-CONTENTS"));
}

#[test]
fn a_test_report_preview_never_contains_the_log() {
    let mut table = HandleTable::new();
    table.declare(
        "report",
        Value::TestReport(TestReportValue {
            passed: 2,
            failed: 1,
            skipped: 0,
            failing_names: vec!["test_thing".into()],
            log: "SECRET-LOG-CONTENTS".into(),
        }),
        1,
    );

    let rendered = render_table(&table, PREVIEW_TOKEN_CAP, TABLE_TOKEN_CAP);
    assert!(rendered.contains("test_thing"));
    assert!(!rendered.contains("SECRET-LOG-CONTENTS"));
}

#[test]
fn a_preview_over_the_cap_drops_elements_before_it_cuts_a_string() {
    let long = "x".repeat(50);
    let items: Vec<Value> = (0..8).map(|_| Value::String(long.clone())).collect();
    let mut table = HandleTable::new();
    table.declare("arr", Value::Array(items), 1);

    // A 40-token cap fits two 50-char elements but not four.
    let rendered = render_table(&table, 40, TABLE_TOKEN_CAP);

    assert!(rendered.contains("n=8"));
    assert!(rendered.contains(&format!("[0] \"{long}\"")));
    assert!(rendered.contains(&format!("[7] \"{long}\"")));
    // Dropped by count, not by cutting the string short:
    assert!(!rendered.contains("[1]"));
    assert!(!rendered.contains("[2]"));
    assert!(!rendered.contains("[3]"));
}

#[test]
fn a_table_over_the_cap_drops_renderings_and_frees_nothing() {
    let mut table = HandleTable::new();
    let long = "y".repeat(100);
    for i in 0..5u64 {
        table.declare(format!("h{i}"), Value::String(long.clone()), i);
    }

    let rendered = render_table(&table, PREVIEW_TOKEN_CAP, 60);

    assert!(rendered.contains("older handles not shown; call handles() for the full list"));
    assert!(rendered.contains("h4"));
    assert!(!rendered.contains("h0  string"));

    for i in 0..5u64 {
        assert!(
            table.is_live(&format!("h{i}")),
            "h{i} must still be live after rendering dropped it"
        );
    }
}

#[test]
fn a_handle_is_freed_only_by_redeclaration_free_or_task_end() {
    let mut table = HandleTable::new();

    table.declare("x", Value::Number(1.0), 1);
    assert!(table.is_live("x"));

    table.declare("x", Value::Number(2.0), 2);
    assert_eq!(table.len(), 1, "redeclaration must replace, not duplicate");
    assert_eq!(table.get("x"), Some(&Value::Number(2.0)));

    table.declare("y", Value::Boolean(true), 3);
    table.free("y");
    assert!(!table.is_live("y"));
    assert!(table.is_live("x"), "freeing y must not free x");

    table.declare("z", Value::Null, 4);
    table.end_task();
    assert!(!table.is_live("x"));
    assert!(!table.is_live("z"));
    assert!(table.is_empty());
}

#[test]
fn a_redeclaration_announces_the_cell_it_happened_in() {
    let mut table = HandleTable::new();
    table.declare("x", Value::Number(1.0), 1);
    table.declare("x", Value::Number(2.0), 5);

    let rendered = render_table(&table, PREVIEW_TOKEN_CAP, TABLE_TOKEN_CAP);
    assert!(rendered.contains("(replaced at cell 5)"));
}

#[test]
fn the_token_estimate_matches_glasshouses_documented_heuristic() {
    use pane::runtime::preview::estimate_tokens;
    assert_eq!(estimate_tokens(""), 0);
    assert_eq!(estimate_tokens("hi"), 1);
}

#[test]
fn a_value_that_looks_like_a_table_line_cannot_forge_an_entry() {
    let mut table = HandleTable::new();
    table.declare(
        "greeting",
        Value::String("line one\nevil  Array  n=999\nline three".into()),
        1,
    );
    table.declare(
        "obj",
        Value::Object(vec![(
            "x\nevil2  Array  n=1".to_string(),
            Value::Number(1.0),
        )]),
        1,
    );

    let rendered = render_table(&table, PREVIEW_TOKEN_CAP, TABLE_TOKEN_CAP);
    assert!(
        rendered
            .lines()
            .all(|line| line != "evil  Array  n=999" && line != "evil2  Array  n=1"),
        "an embedded control character forged a standalone table line: {rendered:?}"
    );
    assert!(
        rendered.contains("\\n"),
        "the embedded newline should render escaped, not raw"
    );
}
