use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use pane::contract::SessionId;
use pane::glasshouse::Glasshouse;
use pane::runtime::isolate::Runtime;
use pane::runtime::repair::{SOURCE_BYTE_CAP, SyntaxFailure};
use pane::sandbox::profile::Profile;

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "pane-cell-repair-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(path.join(".claude")).unwrap();
        Self(path)
    }

    fn runtime(&self) -> Runtime {
        Runtime::new(
            &Profile::compile(&self.0, None),
            &Glasshouse::None,
            &SessionId::new("cell-repair"),
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn exact_edit_requires_current_cell_one_nonempty_match_and_a_change() {
    let failure = SyntaxFailure::new(7, "const x = nope;\nconst y = nope;\n").unwrap();
    assert!(
        failure
            .apply(r#"{"cell":6,"replace":"x","with":"z"}"#)
            .is_err()
    );
    assert!(
        failure
            .apply(r#"{"cell":7,"replace":"","with":"z"}"#)
            .is_err()
    );
    assert!(
        failure
            .apply(r#"{"cell":7,"replace":"missing","with":"z"}"#)
            .is_err()
    );
    assert!(
        failure
            .apply(r#"{"cell":7,"replace":"nope","with":"yes"}"#)
            .is_err()
    );
    assert!(
        failure
            .apply(r#"{"cell":7,"replace":"x","with":"x"}"#)
            .is_err()
    );
    assert!(
        failure
            .apply(r#"{"cell":7,"replace":"x","with":"z","extra":1}"#)
            .is_err()
    );
    assert_eq!(
        failure
            .apply(r#"{"cell":7,"replace":"const x = nope;","with":"const x = yes;"}"#)
            .unwrap(),
        "const x = yes;\nconst y = nope;\n"
    );
}

#[test]
fn repair_input_and_result_are_bounded() {
    assert!(SyntaxFailure::new(1, &"x".repeat(SOURCE_BYTE_CAP + 1)).is_none());
    let failure = SyntaxFailure::new(1, "x").unwrap();
    let json = format!(
        "{{\"cell\":1,\"replace\":\"x\",\"with\":\"{}\"}}",
        "y".repeat(SOURCE_BYTE_CAP)
    );
    assert!(failure.apply(&json).is_err());
    let failure = SyntaxFailure::new(1, &format!("x{}", "a".repeat(SOURCE_BYTE_CAP - 1))).unwrap();
    assert!(
        failure
            .apply(r#"{"cell":1,"replace":"x","with":"bb"}"#)
            .is_err()
    );
}

#[test]
fn only_parser_failure_is_eligible_and_any_next_run_consumes_it() {
    let fixture = Fixture::new();
    let mut runtime = fixture.runtime();

    runtime.run_cell("const = ;");
    let failure = runtime.syntax_failure().expect("parse failure is eligible");
    assert_eq!(failure.cell, 1);
    assert!(failure.hint().contains("Nothing in cell 1 ran"));

    let thrown = runtime.run_cell("globalThis.touched = 1; throw new SyntaxError('runtime');");
    assert!(
        matches!(thrown, pane::runtime::outcome::CellOutcome::Threw { ref error, .. } if error.class == "SyntaxError")
    );
    assert!(runtime.syntax_failure().is_none());

    runtime.run_cell("const = ;");
    assert_eq!(runtime.syntax_failure().unwrap().cell, 3);
    runtime.run_cell("const ok = 1;");
    assert!(runtime.syntax_failure().is_none());
}

#[test]
fn a_corrected_parse_failure_becomes_the_new_target_and_task_end_clears_it() {
    let fixture = Fixture::new();
    let mut runtime = fixture.runtime();

    runtime.run_cell("const = ;");
    let amended = runtime
        .syntax_failure()
        .unwrap()
        .apply(r#"{"cell":1,"replace":"const = ;","with":"let = ;"}"#)
        .unwrap();
    runtime.run_cell(&amended);
    assert_eq!(runtime.syntax_failure().unwrap().cell, 2);

    runtime.end_task();
    assert!(runtime.syntax_failure().is_none());
}

#[test]
fn overlapping_matches_are_ambiguous_and_replacements_are_literal_unicode() {
    let failure = SyntaxFailure::new(1, "界界界").unwrap();
    assert!(
        failure
            .apply(r#"{"cell":1,"replace":"界界","with":"x"}"#)
            .is_err()
    );
    let failure = SyntaxFailure::new(1, "const 世界 = 'bad;").unwrap();
    assert_eq!(
        failure
            .apply(r#"{"cell":1,"replace":"'bad;","with":"'$&\\n';"}"#)
            .unwrap(),
        "const 世界 = '$&\\n';"
    );
}
