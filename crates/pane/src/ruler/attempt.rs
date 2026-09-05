//! One attempt: cut a worktree at the task's parent commit, launch the
//! harness there, run the task's own test command, meter the exchange, and
//! remove the worktree on every path -- including every failure path.
//!
//! **The invariant that is not negotiable:** neither the harness nor the
//! task's test command ever runs anywhere but the worktree cut for that
//! attempt, and that worktree is always the one that gets removed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use super::meter::Meter;
use super::model::{Attempt, Harness, Outcome, Task, Tokens};

/// Serializes every attempt's harness-launch-through-meter-read span,
/// process-wide.
///
/// **Why this exists (a defect, not a hypothetical):** the meter has no
/// session to filter by -- `routing_observations.session_id` is set by
/// Glasshouse's own session machinery for sessions *Glasshouse* launched,
/// and `run_one` launches the harness directly, so no such session exists.
/// [`super::meter`] therefore attributes tokens by time window alone. Two
/// attempts whose windows overlap would each see the other's rows (or worse,
/// silently split them), and there would be no signal that anything was
/// wrong -- the sums would just be quietly incorrect. A doc comment saying
/// "call this serially" cannot stop a future refactor from parallelizing the
/// loop in `cli::run`; this lock can.
static ATTEMPT_LOCK: Mutex<()> = Mutex::new(());

/// One harness's executable, the argv it is always launched with, and
/// whether the task's statement is appended as the final argument. A second
/// harness is a row here, never a branch inside [`run_one`].
#[derive(Debug, Clone)]
pub struct HarnessCommand {
    pub program: PathBuf,
    pub fixed_args: Vec<String>,
    /// `false` for `pane`: its invocation is bare (no argument vector at
    /// all) until 61C gives it a way to carry a statement. Passing one today
    /// would hand it an argument it does not accept.
    pub carries_statement: bool,
}

/// The table's two production rows. Claude Code is run non-interactively and
/// without permission prompts, exactly as `ruler.md` §4 specifies it; `pane`
/// is invoked bare, per the adapter that landed on `main`.
pub fn default_harnesses() -> HashMap<String, HarnessCommand> {
    let mut table = HashMap::new();
    table.insert(
        "claude-code".to_string(),
        HarnessCommand {
            program: PathBuf::from("claude"),
            fixed_args: vec![
                "--print".to_string(),
                "--dangerously-skip-permissions".to_string(),
            ],
            carries_statement: true,
        },
    );
    table.insert(
        "pane".to_string(),
        HarnessCommand {
            program: PathBuf::from("pane"),
            fixed_args: vec![],
            carries_statement: false,
        },
    );
    table
}

/// What one call to [`run_one`] needs beyond the task and the harness.
pub struct RunOpts {
    /// Parent directory for attempt worktrees. Refused if it resolves inside
    /// this process's own checkout.
    pub scratch: PathBuf,
    /// Passed to the harness child as `ANTHROPIC_BASE_URL` when set.
    pub gateway: Option<String>,
    pub meter: Meter,
    pub harnesses: HashMap<String, HarnessCommand>,
}

/// Runs one attempt of `task` by `harness` and returns its record.
///
/// Wall-clock is measured from the harness launch to the test command
/// exiting, not from "the first request leaving the gateway"
/// (`ruler.md` §3's stricter definition) -- that instant is not observable
/// from here without the exchange-row producer named in the packet's
/// limits, so this measures the widest interval it can actually see.
pub fn run_one(task: &Task, harness: &Harness, attempt_no: u32, opts: &RunOpts) -> Attempt {
    let errored = || Attempt {
        task: task.id,
        tier: task.tier,
        harness: harness.clone(),
        base_commit: String::new(),
        attempt: attempt_no,
        outcome: Outcome::Errored,
        tokens: Tokens::default(),
        wall_clock: Duration::default(),
        turns: None,
        changed_lines: None,
    };

    if task.test.is_empty() || scratch_inside_checkout(&opts.scratch) {
        return errored();
    }

    let dir = opts
        .scratch
        .join(format!("{}-{}-{}", task.id, harness.as_str(), attempt_no));
    if dir.exists() {
        return errored();
    }

    if !cut_worktree(&dir, task.commit) {
        return errored();
    }

    let attempt = {
        let _serial = ATTEMPT_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        run_attempt_in(&dir, task, harness, attempt_no, opts)
    };
    remove_worktree(&dir);
    attempt
}

fn run_attempt_in(
    dir: &Path,
    task: &Task,
    harness: &Harness,
    attempt_no: u32,
    opts: &RunOpts,
) -> Attempt {
    let base_commit = git_rev_parse(dir, "HEAD").unwrap_or_default();

    let finish = |outcome, tokens, wall_clock, turns, changed_lines| Attempt {
        task: task.id,
        tier: task.tier,
        harness: harness.clone(),
        base_commit: base_commit.clone(),
        attempt: attempt_no,
        outcome,
        tokens,
        wall_clock,
        turns,
        changed_lines,
    };

    let Some(command) = opts.harnesses.get(harness.as_str()) else {
        return finish(
            Outcome::Errored,
            Tokens::default(),
            Duration::default(),
            None,
            None,
        );
    };

    let mut launch = Command::new(&command.program);
    launch.args(&command.fixed_args);
    if command.carries_statement {
        launch.arg(task.statement);
    }
    launch.current_dir(dir);
    if let Some(gateway) = &opts.gateway {
        launch.env("ANTHROPIC_BASE_URL", gateway);
    }

    let start_wall = Instant::now();
    let start_time = SystemTime::now();

    if launch.status().is_err() {
        return finish(
            Outcome::Errored,
            Tokens::default(),
            start_wall.elapsed(),
            None,
            None,
        );
    }

    let test_result = run_test_commands(dir, task.test);

    let end_time = SystemTime::now();
    let wall_clock = start_wall.elapsed();
    let (tokens, turns) = opts.meter.read(start_time, end_time);
    let changed_lines = diff_shortstat(dir);

    let outcome = match test_result {
        TestResult::Passed => {
            let bound = task.suspect_bound();
            let changed = changed_lines.unwrap_or(0);
            if changed < bound {
                Outcome::PassSuspect {
                    changed_lines: changed,
                    bound,
                }
            } else {
                Outcome::Pass
            }
        }
        TestResult::Failed => Outcome::Fail,
        TestResult::Errored => Outcome::Errored,
    };

    finish(outcome, tokens, wall_clock, turns, changed_lines)
}

enum TestResult {
    Passed,
    Failed,
    Errored,
}

/// Runs every command in `commands` in order in `dir`, stopping at the first
/// one that does not exit 0. The task completes only if every command in it
/// exits 0 -- one non-zero exit is [`TestResult::Failed`] regardless of how
/// many commands came before it.
fn run_test_commands(dir: &Path, commands: &[&[&str]]) -> TestResult {
    for command in commands {
        let Some((program, args)) = command.split_first() else {
            continue;
        };
        let mut cmd = Command::new(program);
        cmd.args(args);
        cmd.current_dir(dir);
        match cmd.status() {
            Ok(status) if status.success() => continue,
            Ok(_) => return TestResult::Failed,
            Err(_) => return TestResult::Errored,
        }
    }
    TestResult::Passed
}

/// Refuses a scratch directory that resolves inside this process's own
/// working directory -- best-effort: a scratch path that does not exist yet
/// is compared lexically rather than canonically, since there is nothing on
/// disk yet to canonicalize.
fn scratch_inside_checkout(scratch: &Path) -> bool {
    let Ok(cwd) = std::env::current_dir() else {
        return false;
    };
    let cwd = cwd.canonicalize().unwrap_or(cwd);
    let scratch_abs = scratch
        .canonicalize()
        .unwrap_or_else(|_| scratch.to_path_buf());
    scratch_abs.starts_with(&cwd)
}

fn cut_worktree(dir: &Path, commit: &str) -> bool {
    Command::new("git")
        .arg("worktree")
        .arg("add")
        .arg("--detach")
        .arg(dir)
        .arg(format!("{commit}^"))
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn remove_worktree(dir: &Path) {
    let _ = Command::new("git")
        .arg("worktree")
        .arg("remove")
        .arg("--force")
        .arg(dir)
        .status();
}

fn git_rev_parse(dir: &Path, rev: &str) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("rev-parse")
        .arg(rev)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// `git diff --shortstat`'s insertions plus deletions, or `None` if the
/// command itself could not be read.
fn diff_shortstat(dir: &Path) -> Option<u32> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("diff")
        .arg("--shortstat")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(parse_shortstat(&String::from_utf8_lossy(&output.stdout)))
}

fn parse_shortstat(text: &str) -> u32 {
    text.trim()
        .split(',')
        .filter(|part| part.contains("insertion") || part.contains("deletion"))
        .filter_map(|part| {
            part.trim()
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse::<u32>()
                .ok()
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_row_is_bare_and_claude_code_row_carries_the_statement() {
        let table = default_harnesses();

        let claude_code = &table["claude-code"];
        assert!(claude_code.carries_statement);

        let pane = &table["pane"];
        assert!(!pane.carries_statement);
        assert!(pane.fixed_args.is_empty());
    }
}
