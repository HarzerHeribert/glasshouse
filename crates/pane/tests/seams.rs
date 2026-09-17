//! Acceptance tests for the three Glasshouse seams (`pane::glasshouse`,
//! map lines 2446, 2447, 2451). No test here launches a real `glasshouse`
//! binary: every "glasshouse" is a small shell script this file writes into
//! its own temp directory -- 61D's sandbox is not built, so nothing
//! model-authored may execute here.

#![cfg(unix)]
//! **Unix only, at file scope.** Every fake in this file -- the harness, the
//! test command, the `glasshouse` stand-in -- is a shell script with a mode
//! bit, so the module does not compile on Windows rather than failing there.
//! The Windows pane cell added on 2026-09-05 runs the rest of the crate; a
//! `.cmd` twin for these fakes is the successor if Windows coverage of the
//! runner is wanted.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use pane::contract::SessionId;
use pane::gateway::{Gateway, served_by};
use pane::glasshouse::{
    Glasshouse, LifecycleEvent, LocalMemory, checkpoint, emit_lifecycle, emit_tool_result,
    search_memory,
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
    let gateway = Gateway::None;

    let served = served_by(&gateway, UNIX_EPOCH);

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
    let gateway = Gateway::Command { gateway: script };

    let served = served_by(&gateway, UNIX_EPOCH);

    assert_eq!(served.quota_context.as_deref(), Some("team-alpha"));
    assert!(served.is_known());
}

/// The line a real `inference-gateway routing-cost --json` prints, parsed by
/// the real reader — **copied verbatim from that binary's output**, keys in
/// its own order, on 2026-09-17.
///
/// The two halves of this seam live in different crates and neither can
/// reference the other's binary, so the thing that keeps them agreeing is this
/// string. It matters because of what it carries: until the gateway kept these
/// rows, `served_by` was always unknown and Pane fell back to reading the
/// response body, where only the Anthropic spelling of the cached figure is
/// understood — so a whole session on an OpenAI-family route reported
/// `cache_read: 0` while saying nothing at all about caching.
#[test]
fn the_cached_figure_survives_the_trip_from_the_gateways_own_output() {
    let dir = scratch_dir("served-by-cached");
    let script = write_glasshouse_script(
        &dir,
        r#"{"cached_input_tokens":48000,"input_tokens":52000,"model":"gpt-5.6-sol","observed_at":1789000000,"output_tokens":900,"provider":"chatgpt-subscription","purpose":"harness-turn","quota_context":"chatgpt-subscription","route":"openai-responses"}"#,
    );

    let served = served_by(&Gateway::Command { gateway: script }, UNIX_EPOCH);

    assert!(served.is_known());
    assert_eq!(served.model.as_deref(), Some("gpt-5.6-sol"));
    assert_eq!(served.route.as_deref(), Some("openai-responses"));
    assert_eq!(served.input_tokens, Some(52_000));
    assert_eq!(
        served.cached_input_tokens,
        Some(48_000),
        "the figure this whole path exists to carry"
    );
}

#[test]
fn local_firewall_bookkeeping_does_not_become_the_serving_model() {
    let dir = scratch_dir("ignore-firewall");
    let script = write_glasshouse_script(
        &dir,
        "{\"provider\":\"real\",\"model\":\"deepseek-v4-flash\",\"quota_context\":\"account\",\"input_tokens\":5}\n{\"provider\":\"glasshouse\",\"model\":\"context-firewall\",\"quota_context\":\"bash\",\"route\":\"unconfirmed-exit\"}",
    );
    let served = served_by(&Gateway::Command { gateway: script }, UNIX_EPOCH);
    assert_eq!(served.model.as_deref(), Some("deepseek-v4-flash"));
    assert_eq!(served.quota_context.as_deref(), Some("account"));
}
