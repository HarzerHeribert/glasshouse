//! Acceptance tests for GH-PANE-61E-PROMPT against
//! `docs/product/pane/model-contract.md`.

use pane::prompt::{self, Budget, CellResult, ErrorSection, Extracted};
use pane::runtime::handles::{HandleMeta, HandleTable, render_table};
use pane::runtime::preview::{
    ArrayValue, FileValue, PREVIEW_TOKEN_CAP, StringValue, TABLE_TOKEN_CAP, Value,
};
use pane::tools::registry::{self, Tool};

/// §7's four messages, as a golden file. The handle table region is
/// `render_table`'s real bytes for a small fixture, not `model-contract.md`
/// §7's own repository-measured figures (`n=1195`, `inline cost ~30,565
/// tok`) -- those were measured on a real grep and this fixture is five
/// elements, so its `n=`, `inline cost` and `preview` figures are honest for
/// the fixture and illustrative of the shape, not §7's literal numbers.
///
/// Compared with `\r\n` normalised to `\n` on both sides, per the packet's
/// cross-platform requirement.
#[test]
fn the_worked_turn_renders_byte_for_byte() {
    let golden = include_str!("prompt/worked_turn.golden").replace("\r\n", "\n");
    let golden = golden.strip_suffix('\n').unwrap_or(&golden);
    let messages: Vec<&str> = golden.split("\n===MSG===\n").collect();
    assert_eq!(
        messages.len(),
        4,
        "golden file must hold exactly 4 messages"
    );
    let (task, cell_1_program, cell_1_result, cell_2_program) =
        (messages[0], messages[1], messages[2], messages[3]);

    assert_eq!(
        task,
        "Every file that names `IntegrationId` — how many are tests, and which\nproduction files would a new variant force me to touch?"
    );

    // Turn 1 assistant and turn 2 assistant are the model's own literal
    // output; this package's only claim on them is that `extract_program`
    // reads the program back out unchanged.
    assert_eq!(
        prompt::extract_program(cell_1_program),
        Extracted::Program(
            "const hits = await grep({ pattern: \"IntegrationId\", glob: \"crates/glasshouse/**/*.rs\" });\nconst adapter = await read({ path: \"crates/glasshouse/src/harness/mod.rs\" });"
                .to_string()
        )
    );
    assert_eq!(
        prompt::extract_program(cell_2_program),
        Extracted::Program(
            "const isTest = (m) => m.path.startsWith(\"crates/glasshouse/tests/\");\nconst inTests = hits.filter(isTest);\nconst prodFiles = new Set(hits.filter(m => !isTest(m)).map(m => m.path));\nreturn { total: hits.length, in_tests: inTests.length, prod_files: prodFiles.size };"
                .to_string()
        )
    );

    // The cell-1 result is this package's own rendering: `render_table`'s
    // real bytes for a fixture holding `hits` and `adapter`, declared with
    // the same `HandleMeta` the isolate would record for a real `grep`/
    // `read` call so the header carries §7's declared type names.
    let mut fixture = HandleTable::new();
    fixture.declare_with(
        "hits",
        Value::Array(ArrayValue::sampled(
            5,
            vec![
                Value::String(StringValue::sampled(
                    54,
                    "crates/glasshouse/tests/gateway_translate_effort.rs:29".to_string(),
                )),
                Value::String(StringValue::sampled(
                    55,
                    "crates/glasshouse/tests/gateway_translate_effort.rs:512".to_string(),
                )),
                Value::String(StringValue::sampled(
                    57,
                    "crates/glasshouse/tests/gateway_translate_responses.rs:35".to_string(),
                )),
            ],
            Some(Value::String(StringValue::sampled(
                36,
                "crates/pane/src/tools/registry.rs:12".to_string(),
            ))),
        )),
        1,
        HandleMeta {
            type_label: Some("Grep.Match[]".into()),
            // Illustrative of a real grep's stdout size, not a measurement:
            // this fixture's own five elements are a few hundred bytes.
            size_estimate: 4_296,
            ..HandleMeta::default()
        },
    );
    fixture.declare(
        "adapter",
        Value::File(FileValue {
            path: "crates/glasshouse/src/harness/mod.rs".into(),
            byte_len: 63_979,
            line_count: 1_508,
            mtime: "2026-09-05T14:18:26Z".into(),
            lines: vec![
                "//! The contract every supported harness is reached through.".into(),
                "//!".into(),
            ],
        }),
        1,
    );
    let handle_table = render_table(&fixture, PREVIEW_TOKEN_CAP, TABLE_TOKEN_CAP);

    let result = CellResult {
        cell: 1,
        elapsed_ms: 412,
        error: None,
        handle_table: handle_table.clone(),
        stdout_tail: None,
        budget: Budget {
            turn_cap: 8_000,
            task_used: 3_412,
            task_cap: 400_000,
            cells_used: 1,
            cells_cap: 40,
        },
    };

    assert_eq!(prompt::render_result(&result), cell_1_result);
}

/// The constant against §2's text, embedded here so the two can never drift
/// without this test noticing.
#[test]
fn the_preamble_is_the_contracts_verbatim() {
    let expected = "You act by writing TypeScript. Each turn you emit exactly one code block\ntagged `pane`; pane runs it in a persistent V8 isolate and answers with\nwhat your program produced.\n\nTool results are live objects, not text. `await grep(...)` returns an\narray you can filter, index and count in the next line of the same\nprogram. You are shown each object's name and a short preview; you are\nnever shown its payload, and you never need it.\n\nBindings persist. A top-level `const` in one cell is in scope in the\nnext. Redeclaring a name replaces the object and frees the old one.\n\nA cell that runs off the end yields: you get the handle table and another\nturn. A top-level `return` ends the task with that value. Return when the\ntask is answered, not before.\n\nA cell that throws is answered, not retried. You get the error, the line,\nand every binding that completed before the throw. Write the next cell.\n\nA call outside this session's sandbox grant throws PermissionDenied. It\nis catchable and it is final: nothing you write widens a grant.";
    assert_eq!(prompt::PREAMBLE, expected);
}

#[test]
fn every_registered_tool_has_exactly_one_declaration_and_no_other_does() {
    // The declarations table's keys equal `registry::names()` exactly, so
    // the two cannot silently drift.
    let mut declared: Vec<&str> = prompt::declarations::ENTRIES
        .iter()
        .map(|entry| entry.name)
        .collect();
    let mut registered = registry::names();
    declared.sort_unstable();
    registered.sort_unstable();
    assert_eq!(declared, registered);

    let tools: Vec<&Tool> = registry::ALL.iter().collect();
    let system = prompt::render_system("", &tools);

    assert_eq!(
        system.matches("declare function ").count(),
        registry::ALL.len(),
        "one declaration per registered tool"
    );
    for name in registry::names() {
        assert_eq!(
            system.matches(&format!("declare function {name}(")).count(),
            1,
            "`{name}` must render exactly one declaration"
        );
    }
    assert_eq!(
        system.matches("// @callers program").count(),
        registry::ALL.len(),
        "every declaration ends with its own @callers line"
    );
}

#[test]
fn a_result_block_omits_empty_sections_and_writes_none_for_an_empty_table() {
    let empty = CellResult {
        cell: 1,
        elapsed_ms: 5,
        error: None,
        handle_table: String::new(),
        stdout_tail: None,
        budget: Budget {
            turn_cap: 8_000,
            task_used: 0,
            task_cap: 400_000,
            cells_used: 0,
            cells_cap: 40,
        },
    };
    let rendered = prompt::render_result(&empty);
    assert!(rendered.contains("## Handles\n(none)"));
    assert!(!rendered.contains("## Error"));
    assert!(!rendered.contains("## stdout"));
    assert!(rendered.contains("## Budget"));

    let with_stdout = CellResult {
        cell: 2,
        elapsed_ms: 5,
        error: None,
        handle_table: "x  number\n1".to_string(),
        stdout_tail: Some("hello".to_string()),
        budget: Budget {
            turn_cap: 8_000,
            task_used: 1,
            task_cap: 400_000,
            cells_used: 1,
            cells_cap: 40,
        },
    };
    let rendered = prompt::render_result(&with_stdout);
    assert!(rendered.contains("## stdout\nhello"));
    assert!(!rendered.contains("## Error"));

    let with_error = CellResult {
        cell: 3,
        elapsed_ms: 5,
        error: Some(ErrorSection {
            class: "TypeError".to_string(),
            message: "bad thing".to_string(),
            line: 2,
            column: 4,
            frames: vec!["cell 3, line 2".to_string()],
        }),
        handle_table: String::new(),
        stdout_tail: None,
        budget: Budget {
            turn_cap: 8_000,
            task_used: 1,
            task_cap: 400_000,
            cells_used: 1,
            cells_cap: 40,
        },
    };
    let rendered = prompt::render_result(&with_error);
    assert!(rendered.starts_with("[cell 3 threw in 5 ms]"));
    assert!(rendered.contains("## Error\nTypeError: bad thing"));
    assert!(rendered.contains("line 2, column 4"));
    assert!(rendered.contains("at cell 3, line 2"));

    // Section order: Handles, Error, stdout, Budget.
    let handles_at = rendered.find("## Handles").unwrap();
    let error_at = rendered.find("## Error").unwrap();
    let budget_at = rendered.find("## Budget").unwrap();
    assert!(handles_at < error_at);
    assert!(error_at < budget_at);
}

#[test]
fn the_budget_line_warns_at_ninety_percent_and_the_exhausted_preamble_is_one_sentence() {
    let below = CellResult {
        cell: 1,
        elapsed_ms: 1,
        error: None,
        handle_table: String::new(),
        stdout_tail: None,
        budget: Budget {
            turn_cap: 8_000,
            task_used: 359_999,
            task_cap: 400_000,
            cells_used: 1,
            cells_cap: 40,
        },
    };
    assert!(!prompt::render_result(&below).contains("finish or return"));

    let at_ninety = CellResult {
        cell: 1,
        elapsed_ms: 1,
        error: None,
        handle_table: String::new(),
        stdout_tail: None,
        budget: Budget {
            turn_cap: 8_000,
            task_used: 360_000,
            task_cap: 400_000,
            cells_used: 1,
            cells_cap: 40,
        },
    };
    assert!(
        prompt::render_result(&at_ninety)
            .contains("turn cap 8,000 · task 360,000/400,000 · cells 1/40 — finish or return")
    );

    let sentence = prompt::exhausted_preamble();
    assert!(!sentence.contains('\n'));
    assert_eq!(sentence.matches('.').count(), 1);
    assert!(sentence.contains("return"));
}

#[test]
fn exactly_one_pane_block_is_a_program_two_are_an_error_and_a_ts_block_is_prose() {
    assert_eq!(
        prompt::extract_program("```pane\nconst x = 1;\n```"),
        Extracted::Program("const x = 1;".to_string())
    );
    assert_eq!(
        prompt::extract_program(
            "here you go\n```pane\nconst x = 1;\n```\nand also\n```pane\nconst y = 2;\n```"
        ),
        Extracted::TwoBlocks
    );
    assert_eq!(
        prompt::extract_program("just some prose, no code"),
        Extracted::Prose
    );
    assert_eq!(
        prompt::extract_program("```ts\nconst x: number = 1;\n```"),
        Extracted::Prose
    );
}

/// The source scan: this module has no type for a provider-native call block
/// and never renders a live value's payload itself.
#[test]
fn the_prompt_module_never_names_a_tool_use_block() {
    let mod_rs = include_str!("../src/prompt/mod.rs");
    let declarations_rs = include_str!("../src/prompt/declarations.rs");
    for (label, source) in [("mod.rs", mod_rs), ("declarations.rs", declarations_rs)] {
        assert!(!source.contains("tool_use"), "{label} names tool_use");
        assert!(!source.contains("tool_result"), "{label} names tool_result");
        assert!(
            !source.contains("runtime::preview"),
            "{label} names runtime::preview"
        );
    }
}
