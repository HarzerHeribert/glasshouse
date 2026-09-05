//! Acceptance tests for the three Glasshouse seams (`pane::glasshouse`,
//! map lines 2446, 2447, 2451). No test here launches a real `glasshouse`
//! binary: every "glasshouse" is a small shell script this file writes into
//! its own temp directory -- 61D's sandbox is not built, so nothing
//! model-authored may execute here.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use pane::contract::SessionId;
use pane::glasshouse::{
    Glasshouse, LifecycleEvent, LocalMemory, checkpoint, emit_lifecycle, emit_tool_result,
    search_memory, served_by,
};

fn scratch_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "pane-seams-test-{}-{}-{}",
        label,
        std::process::id(),
        unique()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn unique() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Writes an executable shell script whose body is `body`, returning its path.
fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, body).unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
    path
}

/// A fake `glasshouse` that drains stdin (if any) and prints `stdout_line`.
fn write_glasshouse_script(dir: &Path, stdout_line: &str) -> PathBuf {
    let body = format!("#!/bin/sh\ncat > /dev/null\necho '{stdout_line}'\n");
    write_script(dir, "fake_glasshouse.sh", &body)
}

/// A fake `glasshouse` that records its own argv, one line per invocation,
/// into `record`.
fn write_argv_recorder(dir: &Path, record: &Path) -> PathBuf {
    let body = format!("#!/bin/sh\necho \"$@\" >> \"{}\"\n", record.display());
    write_script(dir, "fake_glasshouse.sh", &body)
}

#[test]
fn an_absent_glasshouse_degrades_to_the_local_store_and_never_errors() {
    let dir = scratch_dir("local-store");
    let local = LocalMemory::new(&dir);
    local.add("a note pane wrote itself").unwrap();

    let glasshouse = Glasshouse::None;

    let hits = search_memory(&glasshouse, &local, "note");
    assert_eq!(hits, vec!["a note pane wrote itself".to_string()]);

    let cp = checkpoint(&glasshouse, &local);
    assert_eq!(cp.as_deref(), Some("a note pane wrote itself"));
}

#[test]
fn a_reachable_mcp_surface_answers_a_memory_search_over_stdio() {
    let dir = scratch_dir("mcp-search");
    let script = write_glasshouse_script(
        &dir,
        r#"{"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"remembered fact"}]}}"#,
    );
    let glasshouse = Glasshouse::Command { glasshouse: script };
    let local = LocalMemory::new(&dir);

    let hits = search_memory(&glasshouse, &local, "fact");

    assert_eq!(hits, vec!["remembered fact".to_string()]);
}

#[test]
fn a_lifecycle_event_is_spelled_the_way_glasshouse_spells_it() {
    let dir = scratch_dir("lifecycle-spelling");
    let record = dir.join("argv.txt");
    let script = write_argv_recorder(&dir, &record);
    let glasshouse = Glasshouse::Command { glasshouse: script };
    let session = SessionId::new("sess-1");

    emit_lifecycle(&glasshouse, &session, LifecycleEvent::SessionStart);

    let seen = fs::read_to_string(&record).unwrap();
    assert_eq!(seen.trim(), "hook --session sess-1 --event SessionStart");
}

#[test]
fn a_tool_result_goes_to_the_context_firewall_not_the_lifecycle_hook() {
    let dir = scratch_dir("tool-result-subcommand");
    let record = dir.join("argv.txt");
    let script = write_argv_recorder(&dir, &record);
    let glasshouse = Glasshouse::Command { glasshouse: script };
    let session = SessionId::new("sess-2");

    emit_tool_result(&glasshouse, &session, "{}");

    let seen = fs::read_to_string(&record).unwrap();
    assert_eq!(seen.trim(), "context-firewall hook --session sess-2");
}

#[test]
fn a_dropped_hook_does_not_stop_the_turn() {
    let glasshouse = Glasshouse::None;
    let session = SessionId::new("sess-3");

    // Reaching the end of this test without a panic is the assertion: a
    // hook that cannot be delivered must never stop the turn that emits it.
    emit_lifecycle(&glasshouse, &session, LifecycleEvent::Stop);
    emit_tool_result(&glasshouse, &session, "{}");
}

#[test]
fn an_unmetered_request_is_unknown_not_free() {
    let glasshouse = Glasshouse::None;

    let served = served_by(&glasshouse, UNIX_EPOCH);

    assert!(!served.is_known());
    assert_eq!(served.provider, None);
    assert_eq!(served.quota_context, None);
    assert_eq!(served.input_tokens, None);
    assert_eq!(served.output_tokens, None);
    assert_eq!(served.cached_input_tokens, None);
}

#[test]
fn the_entitlement_comes_from_quota_context() {
    let dir = scratch_dir("served-by-quota");
    let script = write_glasshouse_script(
        &dir,
        r#"{"observed_at":100,"quota_context":"team-alpha","input_tokens":5,"output_tokens":2}"#,
    );
    let glasshouse = Glasshouse::Command { glasshouse: script };

    let served = served_by(&glasshouse, UNIX_EPOCH);

    assert_eq!(served.quota_context.as_deref(), Some("team-alpha"));
    assert!(served.is_known());
}
