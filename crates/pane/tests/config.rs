//! `docs/product/pane/supervisor.md` §1: `.glasshouse/pane.toml`, loaded once
//! at session start. Absent means every default the runtime and the task
//! budget already used; anything present is validated with one sentence per
//! refusal.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use pane::config::PaneConfig;

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
    assert_eq!(config.limits.task_tokens, 400_000);
    assert_eq!(config.limits.cells, 40);
    assert_eq!(config.supervisor.every, 4);
    assert_eq!(config.supervisor.model, None);
    assert!(config.supervisor.enabled);
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
