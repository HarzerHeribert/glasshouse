//! A check that passed on byte-identical files is not run again
//! (`verification::ShellChecks`), driven through a real runtime: the model's
//! own `bash` call, in a real git repository, with a real check.
#![cfg(any(target_os = "macos", target_os = "linux"))]

use pane::contract::SessionId;
use pane::glasshouse::Glasshouse;
use pane::runtime::isolate::Runtime;
use pane::runtime::outcome::CellOutcome;
use pane::runtime::preview::Value;
use pane::sandbox::profile::Profile;
use std::path::{Path, PathBuf};
use std::process::Command;

const ADMITS: &str = r#"{"permissions":{"allow":["Bash","Read(**)"]}}"#;
const FORMATTED: &str = "fn main() {\n    println!(\"hi\");\n}\n";

fn git(root: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(["-c", "user.email=t@example.invalid", "-c", "user.name=t"])
        .args(args)
        .current_dir(root)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

fn crate_fixture() -> PathBuf {
    let root = std::env::temp_dir().join(format!("pane-check-reuse-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(root.join("src/main.rs"), FORMATTED).unwrap();
    std::fs::write(root.join(".gitignore"), "target\n").unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "fixture"]);
    root
}

/// The cell's `{exit, stderr}` as one string.
fn check(runtime: &mut Runtime, command: &str) -> String {
    let outcome = runtime.run_cell(&format!(
        "const r = await bash({{command: {command:?}}}); return `${{r.exit_code}}|${{r.stderr}}`;"
    ));
    match &outcome {
        CellOutcome::Returned {
            value: Value::String(text),
            ..
        } => text.head().to_string(),
        other => panic!("expected a returned string, got {other:?}"),
    }
}

#[test]
fn a_passing_check_is_repeated_only_while_the_files_are_byte_identical() {
    let root = crate_fixture();
    let profile = Profile::compile(&root, Some(ADMITS));
    let mut runtime = Runtime::new(&profile, &Glasshouse::None, &SessionId::new("reuse"));
    let reused = pane::verification::REUSED_NOTE;

    let first = check(&mut runtime, "cargo fmt --check");
    assert!(
        first.starts_with("0|") && !first.contains(reused),
        "{first}"
    );
    let second = check(&mut runtime, "cargo fmt --check");
    assert!(
        second.starts_with("0|") && second.contains(reused),
        "{second}"
    );

    // One byte changes the tree: the check runs again.
    std::fs::write(root.join("src/main.rs"), format!("{FORMATTED}// changed\n")).unwrap();
    let after_edit = check(&mut runtime, "cargo fmt --check");
    assert!(
        after_edit.starts_with("0|") && !after_edit.contains(reused),
        "{after_edit}"
    );

    // A failing check is never repeated: each run is a run.
    std::fs::write(root.join("src/main.rs"), "fn main(){println!(\"hi\");}\n").unwrap();
    let failing = check(&mut runtime, "cargo fmt --check");
    let again = check(&mut runtime, "cargo fmt --check");
    assert!(
        !failing.starts_with("0|") && !again.contains(reused),
        "{failing} / {again}"
    );
    let _ = std::fs::remove_dir_all(root);
}
