//! `docs/product/pane/supervisor.md` §1: `.glasshouse/pane.toml`, loaded once
//! at session start. Absent means every default the runtime already used;
//! anything present is validated with one sentence per
//! refusal.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use pane::config::{CompletionStyle, PaneConfig};

fn unique() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn scratch_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "pane-config-test-{}-{}-{}",
        label,
        std::process::id(),
        unique()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_pane_toml(root: &Path, text: &str) {
    let dir = root.join(".glasshouse");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("pane.toml"), text).unwrap();
}

#[test]
fn absent_pane_toml_means_the_defaults() {
    let root = scratch_dir("absent");
    let config = PaneConfig::load(&root).unwrap();

    assert_eq!(config, PaneConfig::default());
    assert_eq!(config.limits.cell_wall_clock_s, 30);
    assert_eq!(config.limits.response_bytes, 16384);
    assert_eq!(config.limits.cells, 40);
    assert_eq!(config.supervisor.every, 4);
    assert_eq!(config.supervisor.model, None);
    assert!(config.supervisor.enabled);
    assert!(!config.helpers.preflight);
}

#[test]
fn helper_preflight_is_an_explicit_boolean_opt_in() {
    let root = scratch_dir("preflight-on");
    write_pane_toml(
        &root,
        "[helpers]\nmodel = \"helper-tier\"\npreflight = true\n",
    );
    assert!(PaneConfig::load(&root).unwrap().helpers.preflight);

    let root = scratch_dir("preflight-not-boolean");
    write_pane_toml(&root, "[helpers]\npreflight = \"sometimes\"\n");
    let error = PaneConfig::load(&root).unwrap_err();
    assert!(error.contains("preflight"), "{error}");
    assert_eq!(error.lines().count(), 1);
}

#[test]
fn legacy_task_tokens_is_accepted_but_does_not_configure_a_cap() {
    let root = scratch_dir("legacy-task-tokens");
    write_pane_toml(&root, "[limits]\ntask_tokens = 1000\n");

    let config = PaneConfig::load(&root).unwrap();
    assert_eq!(config.limits, PaneConfig::default().limits);
}

#[test]
fn a_limit_outside_its_range_is_refused_with_one_sentence() {
    let root = scratch_dir("range");
    write_pane_toml(&root, "[limits]\ncell_wall_clock_s = 0\n");

    let err = PaneConfig::load(&root).unwrap_err();
    assert!(err.contains("cell_wall_clock_s"), "{err}");
    assert!(err.contains('1') && err.contains("600"), "{err}");
    assert_eq!(err.lines().count(), 1, "refused with one sentence: {err}");

    let root = scratch_dir("range-every");
    write_pane_toml(&root, "[supervisor]\nevery = 0\n");
    let err = PaneConfig::load(&root).unwrap_err();
    assert!(err.contains("every"), "{err}");
    assert_eq!(err.lines().count(), 1, "refused with one sentence: {err}");
}

#[test]
fn an_unknown_key_is_refused() {
    let root = scratch_dir("unknown-key");
    write_pane_toml(&root, "[limits]\nbogus = 1\n");
    let err = PaneConfig::load(&root).unwrap_err();
    assert!(err.contains("bogus"), "{err}");

    let root = scratch_dir("unknown-table");
    write_pane_toml(&root, "[nope]\nx = 1\n");
    let err = PaneConfig::load(&root).unwrap_err();
    assert!(err.contains("nope"), "{err}");
}

#[test]
fn pane_toml_names_no_tool_path_or_grant() {
    let root = scratch_dir("tool-name");
    write_pane_toml(&root, "[supervisor]\nmodel = \"grep\"\n");
    let err = PaneConfig::load(&root).unwrap_err();
    assert!(err.contains("names no tool, path or grant"), "{err}");

    let root = scratch_dir("path-like");
    write_pane_toml(&root, "[supervisor]\nmodel = \"../etc/passwd\"\n");
    let err = PaneConfig::load(&root).unwrap_err();
    assert!(err.contains("names no tool, path or grant"), "{err}");
}

/// `[helpers] completion` -- the only thing that decides whether an accepted
/// task says anything at all. The default is silence: a line printed after
/// every task is a line nobody reads, so the recap is opt-in and its key
/// takes exactly two values.
#[test]
fn completion_parses_both_styles_and_defaults_to_silent() {
    let root = scratch_dir("completion-absent");
    let config = PaneConfig::load(&root).unwrap();
    assert_eq!(
        config.helpers.completion,
        CompletionStyle::Silent,
        "an absent key is silence, not a recap nobody asked for"
    );

    let root = scratch_dir("completion-silent");
    write_pane_toml(&root, "[helpers]\ncompletion = \"silent\"\n");
    assert_eq!(
        PaneConfig::load(&root).unwrap().helpers.completion,
        CompletionStyle::Silent
    );

    let root = scratch_dir("completion-recap");
    write_pane_toml(&root, "[helpers]\ncompletion = \"recap\"\n");
    assert_eq!(
        PaneConfig::load(&root).unwrap().helpers.completion,
        CompletionStyle::Recap
    );
}

#[test]
fn a_third_completion_style_is_refused_with_one_sentence() {
    let root = scratch_dir("completion-bogus");
    write_pane_toml(&root, "[helpers]\ncompletion = \"chatty\"\n");
    let err = PaneConfig::load(&root).unwrap_err();
    assert!(err.contains("completion"), "{err}");
    assert!(
        err.contains("chatty"),
        "the refusal names what was written: {err}"
    );
    assert!(
        err.contains("silent") && err.contains("recap"),
        "and what would have been accepted: {err}"
    );
    assert_eq!(err.lines().count(), 1, "refused with one sentence: {err}");

    let root = scratch_dir("completion-not-a-string");
    write_pane_toml(&root, "[helpers]\ncompletion = true\n");
    let err = PaneConfig::load(&root).unwrap_err();
    assert!(err.contains("completion"), "{err}");
    assert_eq!(err.lines().count(), 1, "refused with one sentence: {err}");
}
