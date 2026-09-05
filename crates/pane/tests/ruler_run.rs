//! Acceptance tests for the ruler's runner (`ruler.rs::attempt`, `::meter`,
//! `::cli`). No test here launches a real harness: every "harness" is a
//! small shell script this file writes into its own temp directory and
//! removes at the end of the process. That is the whole point -- 61D's
//! sandbox is not built, so nothing model-authored may execute here.

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
            fixed_args: vec![],
            carries_statement: true,
        },
    );
    RunOpts {
        scratch,
        gateway: None,
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
