//! The table and the JSONL: two renderers of one [`Score`], sharing the
//! same two column sets so neither can gain a figure the other lacks.
//! Specification: `docs/product/pane/ruler.md` §4.
//!
//! [`HEADERS`] and [`JSONL_KEYS`] are the whole column set, asserted
//! element-by-element in `crates/pane/tests/ruler.rs`. **There is no
//! tokens-per-turn column and none is computed here** -- map line 2432;
//! [`Attempt::turns`](super::model::Attempt::turns) is read and printed, and
//! is never used as a divisor anywhere in this module.

use std::fmt::Write as _;

use super::model::{Attempt, Outcome, Tier};
use super::score::{AggregateRow, Row, Score, TaskRow, TierRow};

/// The rendered table's columns, in order. `render_table` emits exactly
/// these and no others.
pub const HEADERS: [&str; 7] = [
    "task",
    "harness",
    "outcome",
    "tokens/completed",
    "wall",
    "turns",
    "tokens(failed)",
];

/// The JSONL's keys, in the order `render_jsonl` writes them. One line per
/// *attempt* (not per aggregated row), so these are the attempt's raw
/// fields: the three token figures the gateway metered, wall-clock, turn
/// count, and the test command's exit status. `exit_status` is derived from
/// [`Outcome`] (0 for a pass, 1 for a fail, absent for `Errored` since that
/// attempt never reached its test) -- `Attempt` carries no separate exit code.
pub const JSONL_KEYS: [&str; 11] = [
    "task",
    "harness",
    "commit",
    "attempt",
    "outcome",
    "tokens_input",
    "tokens_output",
    "tokens_cached_input",
    "wall_ms",
    "turns",
    "exit_status",
];

/// What an unmeasured figure renders as in the table -- never `0`.
const UNMEASURED: &str = "—";

/// Renders `ruler.md` §4's table: a row per `(task, harness)`, then one
/// block per tier, then the aggregate. The tier blocks are never skipped in
/// favour of the aggregate (map line 2431) and every row shares
/// [`HEADERS`]'s column set -- a tier or aggregate row uses the tier name or
/// `"aggregate"` in the `task` column rather than a differently shaped row.
pub fn render_table(score: &Score) -> String {
    let mut out = String::new();
    writeln!(out, "{}", HEADERS.join("  ")).expect("String write is infallible");

    for TaskRow {
        task, harness, row, ..
    } in &score.task_rows
    {
        writeln!(out, "{}", render_row(task, harness.as_str(), row))
            .expect("String write is infallible");
    }

    for tier in Tier::ALL {
        let rows: Vec<&TierRow> = score
            .tier_rows
            .iter()
            .filter(|row| row.tier == tier)
            .collect();
        if rows.is_empty() {
            continue;
        }
        writeln!(out, "-- tier {} --", tier.as_str()).expect("String write is infallible");
        for TierRow { harness, row, .. } in rows {
            writeln!(out, "{}", render_row(tier.as_str(), harness.as_str(), row))
                .expect("String write is infallible");
        }
    }

    if !score.aggregate_rows.is_empty() {
        writeln!(out, "-- aggregate --").expect("String write is infallible");
        for AggregateRow { harness, row } in &score.aggregate_rows {
            writeln!(out, "{}", render_row("aggregate", harness.as_str(), row))
                .expect("String write is infallible");
        }
    }

    out
}

fn render_row(label: &str, harness: &str, row: &Row) -> String {
    format!(
        "{label}  {harness}  {outcome}  {tokens}  {wall}  {turns}  {failed}",
        label = label,
        harness = harness,
        outcome = fmt_outcome(row.attempts_completed, row.attempts_made),
        tokens = fmt_tokens(row.tokens_per_completed),
        wall = fmt_wall(row.wall_per_completed),
        turns = fmt_turns(row.turns),
        failed = fmt_tokens(row.tokens_failed),
    )
}

fn fmt_outcome(completed: u32, made: u32) -> String {
    format!("{completed}/{made} pass")
}

fn fmt_tokens(tokens: Option<u64>) -> String {
    match tokens {
        Some(n) => group_thousands(n),
        None => UNMEASURED.to_string(),
    }
}

fn fmt_turns(turns: Option<u32>) -> String {
    match turns {
        Some(n) => n.to_string(),
        None => UNMEASURED.to_string(),
    }
}

fn fmt_wall(wall: Option<std::time::Duration>) -> String {
    match wall {
        Some(d) => {
            let total_secs = d.as_secs();
            let mins = total_secs / 60;
            let secs = total_secs % 60;
            if mins > 0 {
                format!("{mins}m{secs:02}s")
            } else {
                format!("{secs}s")
            }
        }
        None => UNMEASURED.to_string(),
    }
}

fn group_thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut grouped: Vec<char> = Vec::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    grouped.iter().rev().collect()
}

/// Renders one JSON line per attempt -- task, harness, commit, attempt,
/// outcome, the three metered token figures, wall-clock, turn count, and the
/// derived exit status -- so a later run can be diffed without re-reading a
/// table. Hand-written with `std::fmt`: `pane`'s `Cargo.toml` names no
/// dependency and this module adds none.
pub fn render_jsonl(attempts: &[Attempt]) -> String {
    let mut out = String::new();
    for attempt in attempts {
        writeln!(out, "{}", render_jsonl_line(attempt)).expect("String write is infallible");
    }
    out
}

fn render_jsonl_line(attempt: &Attempt) -> String {
    format!(
        "{{\"task\":{task},\"harness\":{harness},\"commit\":{commit},\"attempt\":{attempt_num},\"outcome\":{outcome},\"tokens_input\":{tokens_input},\"tokens_output\":{tokens_output},\"tokens_cached_input\":{tokens_cached},\"wall_ms\":{wall_ms},\"turns\":{turns},\"exit_status\":{exit_status}}}",
        task = json_str(attempt.task),
        harness = json_str(attempt.harness.as_str()),
        commit = json_str(&attempt.base_commit),
        attempt_num = attempt.attempt,
        outcome = json_str(attempt.outcome.as_str()),
        tokens_input = json_opt(attempt.tokens.input),
        tokens_output = json_opt(attempt.tokens.output),
        tokens_cached = json_opt(attempt.tokens.cached_input),
        wall_ms = attempt.wall_clock.as_millis(),
        turns = json_opt(attempt.turns),
        exit_status = json_opt(exit_status(attempt.outcome)),
    )
}

/// The test command's exit status, derived from `Outcome` since `Attempt`
/// carries no raw exit code: 0 for a pass (suspect or not), 1 for a fail,
/// and absent for `Errored` -- that attempt never reached its test command.
fn exit_status(outcome: Outcome) -> Option<i32> {
    match outcome {
        Outcome::Pass | Outcome::PassSuspect { .. } => Some(0),
        Outcome::Fail => Some(1),
        Outcome::Errored => None,
    }
}

fn json_opt(value: Option<impl std::fmt::Display>) -> String {
    match value {
        Some(v) => v.to_string(),
        None => "null".to_string(),
    }
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}
