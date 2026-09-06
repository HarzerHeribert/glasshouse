//! Acceptance tests for the ruler's runner (`ruler.rs::attempt`, `::meter`,
//! `::cli`). No test here launches a real harness: every "harness" is a
//! small shell script this file writes into its own temp directory and
//! removes at the end of the process. That is the whole point -- 61D's
//! sandbox is not built, so nothing model-authored may execute here.

#![cfg(unix)]
//! **Unix only, at file scope.** Every fake in this file -- the harness, the
//! test command, the `glasshouse` stand-in -- is a shell script with a mode
//! bit, so the module does not compile on Windows rather than failing there.
//! The Windows pane cell added on 2026-09-05 runs the rest of the crate; a
//! `.cmd` twin for these fakes is the successor if Windows coverage of the
//! runner is wanted.

use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use pane::ruler::attempt::{self, HarnessCommand, RunOpts};
use pane::ruler::cli;
use pane::ruler::meter::{Meter, Readout};
use pane::ruler::model::{Harness, Outcome, Task, Tier};

/// Writes an executable shell script to `dir` that appends its own working
/// directory to `record`, then exits with `exit_code`.
fn write_script(dir: &Path, name: &str, record: &Path, exit_code: i32) -> PathBuf {
    let path = dir.join(name);
    let contents = format!(
        "#!/bin/sh\npwd >> \"{}\"\nexit {}\n",
        record.display(),
        exit_code
    );
    fs::write(&path, contents).unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
    path
}

/// Writes an executable shell script to `dir` that appends its own argv --
/// NUL-separated, so an argument containing spaces, quotes or a `{` survives
/// as the one argument it was launched with -- to `argv_record`, writes its
/// `ANTHROPIC_BASE_URL` to `env_record` and its working directory to
/// `argv_record` with a `.cwd` suffix, then exits with `exit_code`.
fn write_argv_script(
    dir: &Path,
    name: &str,
    argv_record: &Path,
    env_record: &Path,
    exit_code: i32,
) -> PathBuf {
    let path = dir.join(name);
    let cwd_record = argv_record.with_extension("cwd");
    let contents = format!(
        "#!/bin/sh\nprintf '%s\\0' \"$@\" >> \"{}\"\nprintf '%s' \"$ANTHROPIC_BASE_URL\" > \"{}\"\npwd > \"{}\"\nexit {}\n",
        argv_record.display(),
        env_record.display(),
        cwd_record.display(),
        exit_code
    );
    fs::write(&path, contents).unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
    path
}

/// Writes an executable shell script that stands in for `glasshouse` on the
/// `--via-glasshouse` path: a `launch ...` invocation (the harness half)
/// records its NUL-separated argv to `launch_argv_record`; any other
/// invocation (the `routing-cost` half, the meter) is a silent exit 0. One
/// fake binary plays both roles because the real `glasshouse` does too --
/// `--via-glasshouse` needs `--meter <glasshouse>` for exactly that reason.
fn write_via_glasshouse_script(dir: &Path, name: &str, launch_argv_record: &Path) -> PathBuf {
    let path = dir.join(name);
    let contents = format!(
        "#!/bin/sh\nif [ \"$1\" = \"launch\" ]; then\n  printf '%s\\0' \"$@\" >> \"{}\"\nfi\nexit 0\n",
        launch_argv_record.display()
    );
    fs::write(&path, contents).unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
    path
}

/// Writes an executable shell script that stands in for `glasshouse` on the
/// meter half only: a `routing-cost ...` invocation appends its own working
/// directory to `meter_cwd_record`; any other invocation (a `launch ...`
/// call standing in for the harness) is a silent exit 0.
fn write_meter_script(dir: &Path, name: &str, meter_cwd_record: &Path) -> PathBuf {
    let path = dir.join(name);
    let contents = format!(
        "#!/bin/sh\nif [ \"$1\" = \"routing-cost\" ]; then\n  pwd >> \"{}\"\nfi\nexit 0\n",
        meter_cwd_record.display()
    );
    fs::write(&path, contents).unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
    path
}

/// Reads a NUL-separated argv record written by [`write_argv_script`].
fn read_argv(record: &Path) -> Vec<String> {
    fs::read(record)
        .unwrap()
        .split(|&b| b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect()
}

/// Reads the `.cwd` sibling [`write_argv_script`] writes next to `record`.
fn read_argv_cwd(record: &Path) -> PathBuf {
    PathBuf::from(
        fs::read_to_string(record.with_extension("cwd"))
            .unwrap()
            .trim(),
    )
}

/// Groups the NUL-separated tokens [`write_via_glasshouse_script`] recorded
/// across possibly several `launch ...` invocations into one argv per row --
/// each invocation starts with `"launch"`, and its second token is the row.
fn group_launch_calls_by_row(record: &Path) -> HashMap<String, Vec<String>> {
    let mut groups: HashMap<String, Vec<String>> = HashMap::new();
    let mut current: Option<(String, Vec<String>)> = None;
    for token in read_argv(record) {
        if token == "launch" {
            if let Some((row, argv)) = current.take() {
                groups.insert(row, argv);
            }
            current = Some((String::new(), vec![token]));
            continue;
        }
        let Some((row, argv)) = current.as_mut() else {
            continue;
        };
        if row.is_empty() {
            *row = token.clone();
        }
        argv.push(token);
    }
    if let Some((row, argv)) = current {
        groups.insert(row, argv);
    }
    groups
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/pane sits two levels under the repo root")
        .to_path_buf()
}

fn git_output(args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root())
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn head_commit() -> String {
    git_output(&["rev-parse", "HEAD"])
}

fn parent_of(commit: &str) -> String {
    git_output(&["rev-parse", &format!("{commit}^")])
}

fn scratch_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "pane-ruler-test-{}-{}-{}",
        label,
        std::process::id(),
        unique()
    ));
    fs::create_dir_all(&dir).unwrap();
    // Canonicalize immediately: on macOS `TMPDIR` sits under a `/var` that is
    // itself a symlink to `/private/var`, and a shell's `pwd` inside the
    // worktree reports the resolved form. Comparing against that later needs
    // this path already resolved, since by then the worktree (and so the
    // only other thing we could canonicalize against) is gone.
    fs::canonicalize(&dir).unwrap_or(dir)
}

fn unique() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// A `'static` copy of a runtime string, leaked deliberately: [`Task`]'s
/// fields are `&'static str` and these tests need a real commit and real
/// script paths picked at run time, not baked in at compile time.
fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// One `task.test` entry that runs a single script with no arguments.
fn single_command(script: &Path) -> &'static [&'static [&'static str]] {
    let program = leak(script.to_string_lossy().into_owned());
    let command: &'static [&'static str] = Box::leak(vec![program].into_boxed_slice());
    Box::leak(vec![command].into_boxed_slice())
}

/// Two `task.test` entries, run in order.
fn two_commands(first: &Path, second: &Path) -> &'static [&'static [&'static str]] {
    let a: &'static [&'static str] =
        Box::leak(vec![leak(first.to_string_lossy().into_owned())].into_boxed_slice());
    let b: &'static [&'static str] =
        Box::leak(vec![leak(second.to_string_lossy().into_owned())].into_boxed_slice());
    Box::leak(vec![a, b].into_boxed_slice())
}

fn base_task(commit: &'static str, test: &'static [&'static [&'static str]]) -> Task {
    Task {
        id: "T1",
        tier: Tier::Leaf,
        commit,
        statement: "do the thing",
        test,
        shortstat_lines: 100,
    }
}

fn base_opts(scratch: PathBuf, harness_program: PathBuf) -> RunOpts {
    let mut harnesses = HashMap::new();
    harnesses.insert(
        "fake".to_string(),
        HarnessCommand {
            program: harness_program,
            args: vec!["{statement}".to_string()],
        },
    );
    RunOpts {
        scratch,
        gateway: None,
        via_glasshouse: None,
        meter: Meter::None,
        harnesses,
    }
}

#[test]
fn an_attempt_runs_its_test_on_the_worktree_and_never_in_the_checkout() {
    let scratch = scratch_dir("cwd");
    let harness_cwd = scratch.join("harness_cwd.txt");
    let test_cwd = scratch.join("test_cwd.txt");
    let harness_script = write_script(&scratch, "fake_harness.sh", &harness_cwd, 0);
    let test_script = write_script(&scratch, "fake_test.sh", &test_cwd, 0);

    let before = git_output(&["status", "--porcelain"]);

    let commit = leak(head_commit());
    let task = base_task(commit, single_command(&test_script));
    let opts = base_opts(scratch.clone(), harness_script);
    let harness = Harness::new("fake");

    let result = attempt::run_one(&task, &harness, 1, &opts);

    assert!(
        result.outcome.completed(),
        "attempt should complete: {:?}",
        result.outcome
    );

    let attempt_dir = scratch.join(format!("{}-{}-{}", task.id, harness.as_str(), 1));

    let harness_saw = fs::read_to_string(&harness_cwd).unwrap();
    let test_saw = fs::read_to_string(&test_cwd).unwrap();
    let expected = fs::canonicalize(&attempt_dir).unwrap_or(attempt_dir.clone());
    assert_eq!(PathBuf::from(harness_saw.trim()), expected);
    assert_eq!(PathBuf::from(test_saw.trim()), expected);

    assert!(
        !attempt_dir.exists(),
        "worktree must be removed after the attempt"
    );

    let after = git_output(&["status", "--porcelain"]);
    assert_eq!(
        before, after,
        "checkout must be byte-unchanged after an attempt"
    );
}

#[test]
fn the_attempt_starts_at_the_parent_commit() {
    let scratch = scratch_dir("parent");
    let noop_cwd = scratch.join("noop_cwd.txt");
    let noop = write_script(&scratch, "noop.sh", &noop_cwd, 0);

    let head = head_commit();
    let parent = parent_of(&head);

    let commit = leak(head);
    let task = base_task(commit, single_command(&noop));
    let opts = base_opts(scratch, noop);
    let harness = Harness::new("fake");

    let result = attempt::run_one(&task, &harness, 1, &opts);

    assert_eq!(result.base_commit, parent);
}

#[test]
fn an_unreadable_meter_leaves_the_tokens_absent() {
    let scratch = scratch_dir("unreadable-meter");
    let cwd_record = scratch.join("cwd.txt");
    let noop = write_script(&scratch, "noop.sh", &cwd_record, 0);

    let commit = leak(head_commit());
    let task = base_task(commit, single_command(&noop));
    let mut opts = base_opts(scratch, noop);
    opts.meter = Meter::Command {
        glasshouse: PathBuf::from("/definitely/does/not/exist/glasshouse"),
    };
    let harness = Harness::new("fake");

    let result = attempt::run_one(&task, &harness, 1, &opts);

    assert!(
        result.outcome.completed(),
        "an unreadable meter must not fail the attempt"
    );
    assert_eq!(result.tokens.total(), None);
    assert_eq!(result.turns, None);
}

#[test]
fn a_null_token_column_is_not_a_zero() {
    let from = std::time::UNIX_EPOCH + std::time::Duration::from_secs(0);
    let to = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1000);
    let lines = [
        r#"{"observed_at":100,"input_tokens":40,"output_tokens":10}"#,
        r#"{"observed_at":200,"input_tokens":null,"output_tokens":null}"#,
    ];

    let (tokens, turns) = Readout::for_window(lines, from, to);

    assert_eq!(
        tokens.total(),
        Some(50),
        "the null row must not zero a sum built from real rows"
    );
    assert_eq!(turns, Some(2));
}

#[test]
fn a_configured_meter_that_finds_nothing_completes_with_zero_turns_not_none() {
    // Distinguishes "meter answered with no rows" (turns: Some(0)) from "no
    // meter configured" (turns: None) end to end through `run_one`, not just
    // inside `meter.rs` -- collapsing the two used to make a broken meter
    // (session filtering that could never match) look identical to an
    // intentionally unconfigured one.
    let scratch = scratch_dir("meter-zero-rows");
    let cwd_record = scratch.join("cwd.txt");
    let noop = write_script(&scratch, "noop.sh", &cwd_record, 0);
    let fake_glasshouse = write_script(
        &scratch,
        "fake_glasshouse.sh",
        &scratch.join("glasshouse_calls.txt"),
        0,
    );

    let commit = leak(head_commit());
    let task = base_task(commit, single_command(&noop));
    let mut opts = base_opts(scratch, noop);
    opts.meter = Meter::Command {
        glasshouse: fake_glasshouse,
    };
    let harness = Harness::new("fake");

    let result = attempt::run_one(&task, &harness, 1, &opts);

    assert!(result.outcome.completed());
    assert_eq!(result.tokens.total(), None);
    assert_eq!(
        result.turns,
        Some(0),
        "a meter that ran and printed nothing is zero turns, not an absent meter"
    );
}

#[test]
fn concurrent_attempts_never_overlap_their_harness_launch() {
    // Enforces the fix for the session-id defect: since token attribution is
    // by time window alone (`meter.rs`'s module doc comment), two attempts'
    // harness-launch-through-meter-read spans must never overlap process-wide.
    // Each "harness" here tries to claim an exclusive lock directory; if two
    // ever run concurrently, the second one's claim fails and it records a
    // collision.
    let scratch = scratch_dir("serial-enforcement");
    let lockdir = scratch.join("lock");
    let collisions = scratch.join("collisions.txt");
    fs::write(&collisions, "").unwrap();

    let probe_script = scratch.join("mutex_probe.sh");
    let contents = format!(
        "#!/bin/sh\nif mkdir \"{lock}\" 2>/dev/null; then\n  sleep 0.15\n  rmdir \"{lock}\"\nelse\n  echo collision >> \"{log}\"\nfi\n",
        lock = lockdir.display(),
        log = collisions.display(),
    );
    fs::write(&probe_script, contents).unwrap();
    let mut perms = fs::metadata(&probe_script).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&probe_script, perms).unwrap();

    let noop_a = write_script(&scratch, "noop_a.sh", &scratch.join("noop_a_cwd.txt"), 0);
    let noop_b = write_script(&scratch, "noop_b.sh", &scratch.join("noop_b_cwd.txt"), 0);

    let commit = leak(head_commit());
    let mut task_a = base_task(commit, single_command(&noop_a));
    task_a.id = "T1";
    let mut task_b = base_task(commit, single_command(&noop_b));
    task_b.id = "T2";

    let opts_a = base_opts(scratch.clone(), probe_script.clone());
    let opts_b = base_opts(scratch.clone(), probe_script);
    let harness_a = Harness::new("fake");
    let harness_b = Harness::new("fake");

    let t1 = std::thread::spawn(move || attempt::run_one(&task_a, &harness_a, 1, &opts_a));
    let t2 = std::thread::spawn(move || attempt::run_one(&task_b, &harness_b, 1, &opts_b));

    let r1 = t1.join().unwrap();
    let r2 = t2.join().unwrap();

    assert!(r1.outcome.completed(), "{:?}", r1.outcome);
    assert!(r2.outcome.completed(), "{:?}", r2.outcome);

    let collision_log = fs::read_to_string(&collisions).unwrap();
    assert!(
        collision_log.is_empty(),
        "harness launches overlapped, which would corrupt time-window token attribution: {collision_log}"
    );
}

#[test]
fn the_worktree_is_removed_even_when_the_test_command_fails() {
    let scratch = scratch_dir("removed-on-fail");
    let harness_cwd = scratch.join("harness_cwd.txt");
    let test_cwd = scratch.join("test_cwd.txt");
    let harness_script = write_script(&scratch, "fake_harness.sh", &harness_cwd, 0);
    let failing_test = write_script(&scratch, "failing_test.sh", &test_cwd, 1);

    let commit = leak(head_commit());
    let task = base_task(commit, single_command(&failing_test));
    let opts = base_opts(scratch.clone(), harness_script);
    let harness = Harness::new("fake");

    let result = attempt::run_one(&task, &harness, 1, &opts);

    assert_eq!(result.outcome, Outcome::Fail);

    let attempt_dir = scratch.join(format!("{}-{}-{}", task.id, harness.as_str(), 1));
    assert!(
        !attempt_dir.exists(),
        "worktree must be removed even when the test fails"
    );
}

#[test]
fn a_two_command_task_fails_when_the_second_command_fails() {
    let scratch = scratch_dir("second-command-fails");
    let harness_cwd = scratch.join("harness_cwd.txt");
    let first_record = scratch.join("first_cwd.txt");
    let second_record = scratch.join("second_cwd.txt");
    let harness_script = write_script(&scratch, "fake_harness.sh", &harness_cwd, 0);
    let first_command = write_script(&scratch, "first.sh", &first_record, 0);
    let second_command = write_script(&scratch, "second.sh", &second_record, 1);

    let commit = leak(head_commit());
    let task = base_task(commit, two_commands(&first_command, &second_command));
    let opts = base_opts(scratch.clone(), harness_script);
    let harness = Harness::new("fake");

    let result = attempt::run_one(&task, &harness, 1, &opts);

    assert_eq!(result.outcome, Outcome::Fail);
    assert!(
        first_record.exists(),
        "the first command should still have run"
    );
}

#[test]
fn the_pane_row_launches_session_with_the_attempts_root_and_the_statement() {
    let scratch = scratch_dir("pane-row");
    let argv_record = scratch.join("argv.txt");
    let env_record = scratch.join("env.txt");
    let fake_pane = write_argv_script(&scratch, "fake_pane.sh", &argv_record, &env_record, 0);
    let noop_cwd = scratch.join("noop_cwd.txt");
    let test_script = write_script(&scratch, "noop_test.sh", &noop_cwd, 0);

    let pane_row = attempt::default_harnesses().remove("pane").unwrap();
    let mut harnesses = HashMap::new();
    harnesses.insert(
        "pane".to_string(),
        HarnessCommand {
            program: fake_pane,
            args: pane_row.args,
        },
    );

    let commit = leak(head_commit());
    let task = base_task(commit, single_command(&test_script));
    let opts = RunOpts {
        scratch: scratch.clone(),
        gateway: Some("http://127.0.0.1:8731".to_string()),
        via_glasshouse: None,
        meter: Meter::None,
        harnesses,
    };
    let harness = Harness::new("pane");

    let result = attempt::run_one(&task, &harness, 1, &opts);
    assert!(result.outcome.completed(), "{:?}", result.outcome);

    let expected_root = scratch.join(format!("{}-{}-{}", task.id, harness.as_str(), 1));
    let argv = read_argv(&argv_record);
    assert_eq!(
        argv,
        vec![
            "session".to_string(),
            "--root".to_string(),
            expected_root.to_string_lossy().into_owned(),
            "--task".to_string(),
            task.statement.to_string(),
        ]
    );

    let env = fs::read_to_string(&env_record).unwrap();
    assert_eq!(env, "http://127.0.0.1:8731");
}

#[test]
fn the_claude_code_row_still_carries_the_statement_as_a_bare_argument() {
    let scratch = scratch_dir("claude-code-row");
    let argv_record = scratch.join("argv.txt");
    let env_record = scratch.join("env.txt");
    let fake_claude = write_argv_script(&scratch, "fake_claude.sh", &argv_record, &env_record, 0);
    let noop_cwd = scratch.join("noop_cwd.txt");
    let test_script = write_script(&scratch, "noop_test.sh", &noop_cwd, 0);

    let claude_row = attempt::default_harnesses().remove("claude-code").unwrap();
    let mut harnesses = HashMap::new();
    harnesses.insert(
        "claude-code".to_string(),
        HarnessCommand {
            program: fake_claude,
            args: claude_row.args,
        },
    );

    let commit = leak(head_commit());
    let task = base_task(commit, single_command(&test_script));
    let opts = RunOpts {
        scratch: scratch.clone(),
        gateway: None,
        via_glasshouse: None,
        meter: Meter::None,
        harnesses,
    };
    let harness = Harness::new("claude-code");

    let result = attempt::run_one(&task, &harness, 1, &opts);
    assert!(result.outcome.completed(), "{:?}", result.outcome);

    let argv = read_argv(&argv_record);
    assert_eq!(
        argv,
        vec![
            "--print".to_string(),
            "--dangerously-skip-permissions".to_string(),
            task.statement.to_string(),
        ]
    );
}

#[test]
fn the_codex_row_runs_exec_with_the_bypass_and_the_statement() {
    let scratch = scratch_dir("codex-row");
    let argv_record = scratch.join("argv.txt");
    let env_record = scratch.join("env.txt");
    let fake_codex = write_argv_script(&scratch, "fake_codex.sh", &argv_record, &env_record, 0);
    let noop_cwd = scratch.join("noop_cwd.txt");
    let test_script = write_script(&scratch, "noop_test.sh", &noop_cwd, 0);

    let codex_row = attempt::default_harnesses().remove("codex").unwrap();
    let mut harnesses = HashMap::new();
    harnesses.insert(
        "codex".to_string(),
        HarnessCommand {
            program: fake_codex,
            args: codex_row.args,
        },
    );

    let commit = leak(head_commit());
    let task = base_task(commit, single_command(&test_script));
    let opts = RunOpts {
        scratch: scratch.clone(),
        gateway: None,
        via_glasshouse: None,
        meter: Meter::None,
        harnesses,
    };
    let harness = Harness::new("codex");

    let result = attempt::run_one(&task, &harness, 1, &opts);
    assert!(result.outcome.completed(), "{:?}", result.outcome);

    let argv = read_argv(&argv_record);
    assert_eq!(
        argv,
        vec![
            "exec".to_string(),
            "--dangerously-bypass-approvals-and-sandbox".to_string(),
            task.statement.to_string(),
        ]
    );

    let expected_root = scratch.join(format!("{}-{}-{}", task.id, harness.as_str(), 1));
    assert_eq!(
        read_argv_cwd(&argv_record),
        expected_root,
        "codex takes no --root-equivalent flag; it must still launch in the attempt's own worktree"
    );
}

#[test]
fn a_statement_with_spaces_and_braces_reaches_the_child_as_one_argument() {
    let scratch = scratch_dir("statement-braces");
    let argv_record = scratch.join("argv.txt");
    let env_record = scratch.join("env.txt");
    let fake_pane = write_argv_script(&scratch, "fake_pane.sh", &argv_record, &env_record, 0);
    let noop_cwd = scratch.join("noop_cwd.txt");
    let test_script = write_script(&scratch, "noop_test.sh", &noop_cwd, 0);

    let pane_row = attempt::default_harnesses().remove("pane").unwrap();
    let mut harnesses = HashMap::new();
    harnesses.insert(
        "pane".to_string(),
        HarnessCommand {
            program: fake_pane,
            args: pane_row.args,
        },
    );

    let statement: &'static str =
        "split \"main.rs\" into {root} and {statement}, quoted 'like this'";
    let commit = leak(head_commit());
    let mut task = base_task(commit, single_command(&test_script));
    task.statement = statement;
    let opts = RunOpts {
        scratch: scratch.clone(),
        gateway: None,
        via_glasshouse: None,
        meter: Meter::None,
        harnesses,
    };
    let harness = Harness::new("pane");

    let result = attempt::run_one(&task, &harness, 1, &opts);
    assert!(result.outcome.completed(), "{:?}", result.outcome);

    let argv = read_argv(&argv_record);
    assert_eq!(
        argv.last().map(String::as_str),
        Some(statement),
        "the statement must arrive as exactly one argv element, unsplit and unsubstituted"
    );
    assert_eq!(argv.len(), 5, "the template's own five elements, no more");
}

#[test]
fn repeat_below_three_is_refused() {
    let out = scratch_dir("repeat-refused");
    let args = vec![
        "run".to_string(),
        "--task".to_string(),
        "L1".to_string(),
        "--harness".to_string(),
        "claude-code".to_string(),
        "--repeat".to_string(),
        "1".to_string(),
        "--out".to_string(),
        out.to_string_lossy().into_owned(),
    ];

    let result = cli::dispatch(&args);

    let message = result.expect_err("--repeat 1 must be refused");
    assert!(
        message.contains("--repeat"),
        "refusal message should say why: {message}"
    );
}

#[test]
fn the_accepted_flags_are_exactly_these() {
    assert_eq!(
        cli::ACCEPTED_FLAGS.to_vec(),
        vec![
            "--task",
            "--tier",
            "--harness",
            "--repeat",
            "--gateway",
            "--meter",
            "--via-glasshouse",
            "--out"
        ]
    );
}

/// The seam with Glasshouse, pinned on the producer's own wire shape.
///
/// `meter.rs` reads four keys out of a row that carries twenty-two, and the
/// join between the two crates is nothing but string equality of those key
/// names -- there is no shared type, because map line 2440 forbids `pane`
/// depending on `glasshouse`. So the fixture below is the full row
/// `commands/routing_cost.rs`'s `ObservationJson` serializes, every key in
/// its declared order, rather than the four-key subset the other tests use.
/// If the producer renames a token column, this fails; without it the meter
/// would silently sum nothing and report an honest-looking absent figure.
///
/// The `null`s are the other half of the contract: an absent column is
/// `null` and never `0`, so the second row here contributes a turn and no
/// tokens.
#[test]
fn the_meter_parses_the_readouts_full_twenty_two_key_row() {
    let counted = r#"{"seq":1,"observed_at":1757100000,"session_id":null,"harness":"claude-code","provider":"anthropic","model":"claude-opus-5","route":"relay","purpose":null,"quota_context":null,"dispatched_at":1757099880,"completed_at":1757100000,"first_byte_ms":420,"completed_ms":120000,"input_tokens":18204,"output_tokens":3311,"cached_input_tokens":140200,"outcome":"succeeded","failure_class":null,"tool_rounds":9,"retries":0,"repairs":0,"failovers":0}"#;
    let uncounted = r#"{"seq":2,"observed_at":1757100010,"session_id":null,"harness":"claude-code","provider":"anthropic","model":"claude-opus-5","route":"relay","purpose":null,"quota_context":null,"dispatched_at":null,"completed_at":null,"first_byte_ms":null,"completed_ms":null,"input_tokens":null,"output_tokens":null,"cached_input_tokens":null,"outcome":null,"failure_class":null,"tool_rounds":null,"retries":null,"repairs":null,"failovers":null}"#;

    let from = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1757099000);
    let to = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1757101000);
    let (tokens, turns) = Readout::for_window([counted, uncounted], from, to);

    assert_eq!(tokens.input, Some(18204));
    assert_eq!(tokens.output, Some(3311));
    assert_eq!(tokens.cached_input, Some(140200));
    assert_eq!(tokens.total(), Some(18204 + 3311 + 140200));
    assert_eq!(
        turns,
        Some(2),
        "both rows are exchanges; only one of them was metered"
    );
}

/// Required behaviour 3: the bare form (`--via-glasshouse <profile>`, no
/// `=`) applies one profile to every selected row, and its argv is exactly
/// `launch <row> --profile <profile> -- <the row's own substituted argv>` --
/// pinned unchanged for both `pane` and `claude-code`.
#[test]
fn a_bare_via_glasshouse_still_applies_one_profile_to_every_row() {
    let scratch = scratch_dir("via-glasshouse-pane");
    let launch_argv_record = scratch.join("launch_argv.txt");
    let fake_glasshouse =
        write_via_glasshouse_script(&scratch, "fake_glasshouse.sh", &launch_argv_record);
    let test_cwd = scratch.join("test_cwd.txt");
    let test_script = write_script(&scratch, "noop_test.sh", &test_cwd, 0);

    let commit = leak(head_commit());
    let task = base_task(commit, single_command(&test_script));
    let opts = RunOpts {
        scratch: scratch.clone(),
        gateway: None,
        via_glasshouse: Some(HashMap::from([("pane".to_string(), "bench".to_string())])),
        meter: Meter::Command {
            glasshouse: fake_glasshouse,
        },
        harnesses: attempt::default_harnesses(),
    };
    let harness = Harness::new("pane");

    let result = attempt::run_one(&task, &harness, 1, &opts);
    assert!(result.outcome.completed(), "{:?}", result.outcome);

    let expected_root = scratch.join(format!("{}-{}-{}", task.id, harness.as_str(), 1));
    let argv = read_argv(&launch_argv_record);
    assert_eq!(
        argv,
        vec![
            "launch".to_string(),
            "pane".to_string(),
            "--profile".to_string(),
            "bench".to_string(),
            "--headless".to_string(),
            "--fresh".to_string(),
            "--no-routing".to_string(),
            "--no-memory".to_string(),
            "--".to_string(),
            "session".to_string(),
            "--root".to_string(),
            expected_root.to_string_lossy().into_owned(),
            "--task".to_string(),
            task.statement.to_string(),
        ]
    );

    // The same shape for claude-code -- the second baseline the packet
    // names by name.
    let scratch2 = scratch_dir("via-glasshouse-claude-code");
    let launch_argv_record2 = scratch2.join("launch_argv.txt");
    let fake_glasshouse2 =
        write_via_glasshouse_script(&scratch2, "fake_glasshouse.sh", &launch_argv_record2);
    let test_cwd2 = scratch2.join("test_cwd.txt");
    let test_script2 = write_script(&scratch2, "noop_test.sh", &test_cwd2, 0);

    let commit2 = leak(head_commit());
    let task2 = base_task(commit2, single_command(&test_script2));
    let opts2 = RunOpts {
        scratch: scratch2.clone(),
        gateway: None,
        via_glasshouse: Some(HashMap::from([(
            "claude-code".to_string(),
            "bench".to_string(),
        )])),
        meter: Meter::Command {
            glasshouse: fake_glasshouse2,
        },
        harnesses: attempt::default_harnesses(),
    };
    let harness2 = Harness::new("claude-code");

    let result2 = attempt::run_one(&task2, &harness2, 1, &opts2);
    assert!(result2.outcome.completed(), "{:?}", result2.outcome);

    let argv2 = read_argv(&launch_argv_record2);
    assert_eq!(
        argv2,
        vec![
            "launch".to_string(),
            "claude-code".to_string(),
            "--profile".to_string(),
            "bench".to_string(),
            "--headless".to_string(),
            "--fresh".to_string(),
            "--no-routing".to_string(),
            "--no-memory".to_string(),
            "--".to_string(),
            "--print".to_string(),
            "--dangerously-skip-permissions".to_string(),
            task2.statement.to_string(),
        ]
    );
}

/// Required behaviour 1: two rows, two profiles, each launch's `--profile`
/// is its own row's -- one shared argv recorder tells the two launches apart
/// by the row key ([`group_launch_calls_by_row`]).
#[test]
fn via_glasshouse_takes_one_profile_per_row() {
    let scratch = scratch_dir("via-glasshouse-per-row");
    let launch_argv_record = scratch.join("launch_argv.txt");
    let fake_glasshouse =
        write_via_glasshouse_script(&scratch, "fake_glasshouse.sh", &launch_argv_record);

    let mut profiles = HashMap::new();
    profiles.insert("pane".to_string(), "bench-pane".to_string());
    profiles.insert("claude-code".to_string(), "bench-claude".to_string());

    let commit = leak(head_commit());
    let pane_test = write_script(&scratch, "pane_test.sh", &scratch.join("pane_cwd.txt"), 0);
    let pane_task = base_task(commit, single_command(&pane_test));
    let pane_opts = RunOpts {
        scratch: scratch.clone(),
        gateway: None,
        via_glasshouse: Some(profiles.clone()),
        meter: Meter::Command {
            glasshouse: fake_glasshouse.clone(),
        },
        harnesses: attempt::default_harnesses(),
    };
    let pane_harness = Harness::new("pane");
    let pane_result = attempt::run_one(&pane_task, &pane_harness, 1, &pane_opts);
    assert!(pane_result.outcome.completed(), "{:?}", pane_result.outcome);

    let claude_test = write_script(
        &scratch,
        "claude_test.sh",
        &scratch.join("claude_cwd.txt"),
        0,
    );
    let claude_task = base_task(commit, single_command(&claude_test));
    let claude_opts = RunOpts {
        scratch: scratch.clone(),
        gateway: None,
        via_glasshouse: Some(profiles),
        meter: Meter::Command {
            glasshouse: fake_glasshouse,
        },
        harnesses: attempt::default_harnesses(),
    };
    let claude_harness = Harness::new("claude-code");
    let claude_result = attempt::run_one(&claude_task, &claude_harness, 1, &claude_opts);
    assert!(
        claude_result.outcome.completed(),
        "{:?}",
        claude_result.outcome
    );

    let by_row = group_launch_calls_by_row(&launch_argv_record);

    let pane_root = scratch.join(format!("{}-{}-{}", pane_task.id, pane_harness.as_str(), 1));
    assert_eq!(
        by_row.get("pane"),
        Some(&vec![
            "launch".to_string(),
            "pane".to_string(),
            "--profile".to_string(),
            "bench-pane".to_string(),
            "--headless".to_string(),
            "--fresh".to_string(),
            "--no-routing".to_string(),
            "--no-memory".to_string(),
            "--".to_string(),
            "session".to_string(),
            "--root".to_string(),
            pane_root.to_string_lossy().into_owned(),
            "--task".to_string(),
            pane_task.statement.to_string(),
        ])
    );

    assert_eq!(
        by_row.get("claude-code"),
        Some(&vec![
            "launch".to_string(),
            "claude-code".to_string(),
            "--profile".to_string(),
            "bench-claude".to_string(),
            "--headless".to_string(),
            "--fresh".to_string(),
            "--no-routing".to_string(),
            "--no-memory".to_string(),
            "--".to_string(),
            "--print".to_string(),
            "--dangerously-skip-permissions".to_string(),
            claude_task.statement.to_string(),
        ])
    );
}

/// Required behaviour 2: a selected row with no profile under the per-row
/// form is refused, naming the row, before `resolve_tasks` runs or any
/// attempt starts.
#[test]
fn a_per_row_via_glasshouse_refuses_a_row_with_no_profile_before_any_attempt() {
    let out = scratch_dir("via-glasshouse-missing-row");

    let result = cli::dispatch(&[
        "run".to_string(),
        "--task".to_string(),
        "L1".to_string(),
        "--harness".to_string(),
        "claude-code".to_string(),
        "--harness".to_string(),
        "codex".to_string(),
        "--meter".to_string(),
        "/definitely/does/not/exist/glasshouse".to_string(),
        "--via-glasshouse".to_string(),
        "claude-code=bench".to_string(),
        "--out".to_string(),
        out.to_string_lossy().into_owned(),
    ]);

    let message = result.expect_err("a row left without a profile must be refused");
    assert!(
        message.contains("codex"),
        "refusal message should name the row: {message}"
    );
    assert!(
        !out.join("attempts.jsonl").exists(),
        "no attempt may have run: attempts.jsonl must not have been written"
    );
}

/// Required behaviour 2: mixing a bare profile with `row=profile` entries in
/// one `--via-glasshouse` is refused before `resolve_tasks` runs or any
/// attempt starts.
#[test]
fn a_mixed_bare_and_per_row_via_glasshouse_is_refused_before_any_attempt() {
    let out = scratch_dir("via-glasshouse-mixed-forms");

    let result = cli::dispatch(&[
        "run".to_string(),
        "--task".to_string(),
        "L1".to_string(),
        "--harness".to_string(),
        "claude-code".to_string(),
        "--harness".to_string(),
        "codex".to_string(),
        "--meter".to_string(),
        "/definitely/does/not/exist/glasshouse".to_string(),
        "--via-glasshouse".to_string(),
        "bench".to_string(),
        "--via-glasshouse".to_string(),
        "codex=bench2".to_string(),
        "--out".to_string(),
        out.to_string_lossy().into_owned(),
    ]);

    let message = result.expect_err("mixing a bare profile with row=profile must be refused");
    // The mix refusal's own sentence, not merely the flag's name: with the
    // mix check gone, the per-row pass refuses `claude-code` for having no
    // profile, and that message names the flag too (a SURVIVED mutation at
    // integration, 2026-09-06).
    assert!(
        message.contains("cannot mix a bare profile with row=profile"),
        "the mix refusal must be the one that fires: {message}"
    );
    assert!(
        !out.join("attempts.jsonl").exists(),
        "no attempt may have run: attempts.jsonl must not have been written"
    );
}

/// Required behaviour 2: a row named twice in the per-row form is refused
/// before `resolve_tasks` runs or any attempt starts.
#[test]
fn a_duplicate_row_in_via_glasshouse_is_refused_before_any_attempt() {
    let out = scratch_dir("via-glasshouse-duplicate-row");

    let result = cli::dispatch(&[
        "run".to_string(),
        "--task".to_string(),
        "L1".to_string(),
        "--harness".to_string(),
        "claude-code".to_string(),
        "--meter".to_string(),
        "/definitely/does/not/exist/glasshouse".to_string(),
        "--via-glasshouse".to_string(),
        "claude-code=bench".to_string(),
        "--via-glasshouse".to_string(),
        "claude-code=other".to_string(),
        "--out".to_string(),
        out.to_string_lossy().into_owned(),
    ]);

    let message = result.expect_err("a row named twice must be refused");
    assert!(
        message.contains("claude-code"),
        "refusal message should name the row: {message}"
    );
    assert!(
        !out.join("attempts.jsonl").exists(),
        "no attempt may have run: attempts.jsonl must not have been written"
    );
}

/// Required behaviour 2: a `row=profile` value naming a row `--harness` did
/// not select is refused before `resolve_tasks` runs or any attempt starts.
#[test]
fn a_row_via_glasshouse_did_not_select_is_refused_before_any_attempt() {
    let out = scratch_dir("via-glasshouse-unselected-row");

    let result = cli::dispatch(&[
        "run".to_string(),
        "--task".to_string(),
        "L1".to_string(),
        "--harness".to_string(),
        "claude-code".to_string(),
        "--meter".to_string(),
        "/definitely/does/not/exist/glasshouse".to_string(),
        "--via-glasshouse".to_string(),
        "codex=bench".to_string(),
        "--out".to_string(),
        out.to_string_lossy().into_owned(),
    ]);

    let message = result.expect_err("a row --harness did not select must be refused");
    assert!(
        message.contains("codex"),
        "refusal message should name the row: {message}"
    );
    assert!(
        !out.join("attempts.jsonl").exists(),
        "no attempt may have run: attempts.jsonl must not have been written"
    );
}

/// Required behaviour 4: `routing-cost` runs with the attempt's own worktree
/// as its current directory, both without and with `--via-glasshouse`.
#[test]
fn the_meter_reads_routing_cost_from_the_attempts_worktree() {
    // Without --via-glasshouse: the harness is one program, the meter another.
    let scratch = scratch_dir("meter-cwd-plain");
    let meter_cwd_record = scratch.join("meter_cwd.txt");
    let fake_glasshouse = write_meter_script(&scratch, "fake_glasshouse.sh", &meter_cwd_record);
    let harness_cwd = scratch.join("harness_cwd.txt");
    let harness_script = write_script(&scratch, "fake_harness.sh", &harness_cwd, 0);
    let test_cwd = scratch.join("test_cwd.txt");
    let test_script = write_script(&scratch, "noop_test.sh", &test_cwd, 0);

    let commit = leak(head_commit());
    let task = base_task(commit, single_command(&test_script));
    let mut opts = base_opts(scratch.clone(), harness_script);
    opts.meter = Meter::Command {
        glasshouse: fake_glasshouse,
    };
    let harness = Harness::new("fake");

    let result = attempt::run_one(&task, &harness, 1, &opts);
    assert!(result.outcome.completed(), "{:?}", result.outcome);

    let expected_dir = scratch.join(format!("{}-{}-{}", task.id, harness.as_str(), 1));
    let expected_dir = fs::canonicalize(&expected_dir).unwrap_or(expected_dir);
    let recorded = fs::read_to_string(&meter_cwd_record).unwrap();
    assert_eq!(
        PathBuf::from(recorded.trim()),
        expected_dir,
        "the meter must read the attempt's worktree, not the ruler's own cwd"
    );

    // With --via-glasshouse: harness and meter share the same glasshouse
    // binary, but the meter call must still land in the attempt's worktree.
    let scratch2 = scratch_dir("meter-cwd-via-glasshouse");
    let meter_cwd_record2 = scratch2.join("meter_cwd.txt");
    let fake_glasshouse2 = write_meter_script(&scratch2, "fake_glasshouse.sh", &meter_cwd_record2);
    let test_cwd2 = scratch2.join("test_cwd.txt");
    let test_script2 = write_script(&scratch2, "noop_test.sh", &test_cwd2, 0);

    let commit2 = leak(head_commit());
    let task2 = base_task(commit2, single_command(&test_script2));
    let opts2 = RunOpts {
        scratch: scratch2.clone(),
        gateway: None,
        via_glasshouse: Some(HashMap::from([("pane".to_string(), "bench".to_string())])),
        meter: Meter::Command {
            glasshouse: fake_glasshouse2,
        },
        harnesses: attempt::default_harnesses(),
    };
    let harness2 = Harness::new("pane");

    let result2 = attempt::run_one(&task2, &harness2, 1, &opts2);
    assert!(result2.outcome.completed(), "{:?}", result2.outcome);

    let expected_dir2 = scratch2.join(format!("{}-{}-{}", task2.id, harness2.as_str(), 1));
    let expected_dir2 = fs::canonicalize(&expected_dir2).unwrap_or(expected_dir2);
    let recorded2 = fs::read_to_string(&meter_cwd_record2).unwrap();
    assert_eq!(
        PathBuf::from(recorded2.trim()),
        expected_dir2,
        "via-glasshouse path: the meter must still read the attempt's own worktree"
    );
}

/// Required behaviour 5: both illegal combinations are refused before any
/// attempt runs.
#[test]
fn via_glasshouse_needs_the_meter_and_excludes_a_standing_gateway() {
    let out = scratch_dir("via-glasshouse-refusals");

    let no_meter = cli::dispatch(&[
        "run".to_string(),
        "--task".to_string(),
        "L1".to_string(),
        "--harness".to_string(),
        "pane".to_string(),
        "--via-glasshouse".to_string(),
        "bench".to_string(),
        "--out".to_string(),
        out.to_string_lossy().into_owned(),
    ]);
    let message = no_meter.expect_err("--via-glasshouse without --meter must be refused");
    assert!(message.contains("--via-glasshouse"), "{message}");
    assert!(message.contains("--meter"), "{message}");

    let with_gateway = cli::dispatch(&[
        "run".to_string(),
        "--task".to_string(),
        "L1".to_string(),
        "--harness".to_string(),
        "pane".to_string(),
        "--via-glasshouse".to_string(),
        "bench".to_string(),
        "--meter".to_string(),
        "/definitely/does/not/exist/glasshouse".to_string(),
        "--gateway".to_string(),
        "http://127.0.0.1:8731".to_string(),
        "--out".to_string(),
        out.to_string_lossy().into_owned(),
    ]);
    let message2 = with_gateway.expect_err("--via-glasshouse with --gateway must be refused");
    assert!(message2.contains("--via-glasshouse"), "{message2}");
    assert!(message2.contains("--gateway"), "{message2}");
}
