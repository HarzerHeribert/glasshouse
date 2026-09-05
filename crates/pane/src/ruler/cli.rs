//! `pane ruler run`: parse the command, resolve the task and harness sets,
//! run every (task, harness, attempt) combination, and write one JSON line
//! per attempt. The table print (aggregation and per-tier rows) is
//! `score`/`report`'s job, not this module's -- that wiring lands once those
//! two placeholders are filled.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use clap::Parser;
use serde::Serialize;

use super::attempt::{self, RunOpts};
use super::meter::Meter;
use super::model::{Attempt, Harness, Task, Tier};
use super::tasks;

/// The whole accepted flag set for `pane ruler run` (map line 2432: there is
/// no flag that produces a tokens-per-turn figure -- `Attempt::turns` is
/// carried and printed, never divided into).
pub const ACCEPTED_FLAGS: &[&str] = &[
    "--task",
    "--tier",
    "--harness",
    "--repeat",
    "--gateway",
    "--meter",
    "--out",
];

/// A single attempt of an agent task measures the sample, not the harness --
/// this is the minimum `--repeat` may be, and the default.
pub const MIN_REPEAT: u32 = 3;

#[derive(Parser, Debug)]
#[command(name = "pane ruler run")]
pub struct RunArgs {
    #[arg(long)]
    pub task: Vec<String>,
    #[arg(long)]
    pub tier: Option<String>,
    #[arg(long)]
    pub harness: Vec<String>,
    #[arg(long, default_value_t = MIN_REPEAT)]
    pub repeat: u32,
    #[arg(long)]
    pub gateway: Option<String>,
    /// Path to the `glasshouse` executable to read exchange rows from (via
    /// `routing-cost --json`). Omitted means no meter: tokens and turns are
    /// absent for every attempt, never a fabricated zero.
    #[arg(long)]
    pub meter: Option<PathBuf>,
    #[arg(long)]
    pub out: PathBuf,
}

/// Dispatches `args` (everything after `pane ruler`) to the `run`
/// subcommand. `args[0]` must be `"run"`; every other flag is `run`'s own.
pub fn dispatch(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("run") => run(&args[1..]),
        Some(other) => Err(format!("unknown ruler subcommand: {other}")),
        None => Err("usage: pane ruler run [flags]".to_string()),
    }
}

fn run(flags: &[String]) -> Result<(), String> {
    let args = RunArgs::try_parse_from(
        std::iter::once("pane ruler run".to_string()).chain(flags.iter().cloned()),
    )
    .map_err(|e| e.to_string())?;

    if args.repeat < MIN_REPEAT {
        return Err(format!(
            "--repeat must be at least {MIN_REPEAT}: a single attempt of an agent task measures the sample, not the harness"
        ));
    }

    let tasks = resolve_tasks(&args)?;
    if tasks.is_empty() {
        return Err("no tasks selected: pass --task or --tier".to_string());
    }
    let harnesses = resolve_harnesses(&args)?;

    let opts = RunOpts {
        scratch: std::env::temp_dir().join("pane-ruler"),
        gateway: args.gateway.clone(),
        meter: match &args.meter {
            Some(glasshouse) => Meter::Command {
                glasshouse: glasshouse.clone(),
            },
            None => Meter::None,
        },
        harnesses: attempt::default_harnesses(),
    };

    // This loop must stay a plain sequential loop: `attempt::run_one` reads
    // the meter by time window alone (`meter.rs`'s module doc comment), and
    // `attempt::ATTEMPT_LOCK` only makes concurrent calls *safe*, not
    // *meaningful* -- calling `run_one` from multiple threads would still
    // serialize their windows one at a time, silently discarding the
    // parallelism a naive refactor here would be trying to add.
    let mut attempts = Vec::new();
    for task in &tasks {
        for harness_name in &harnesses {
            let harness = Harness::new(harness_name.clone());
            for attempt_no in 1..=args.repeat {
                attempts.push(attempt::run_one(task, &harness, attempt_no, &opts));
            }
        }
    }

    write_records(&args.out, &attempts)
}

fn resolve_tasks(args: &RunArgs) -> Result<Vec<&'static Task>, String> {
    let mut seen = HashMap::new();
    let mut resolved = Vec::new();

    if let Some(tier_name) = &args.tier {
        let tier = parse_tier(tier_name)?;
        for task in tasks::in_tier(tier) {
            if seen.insert(task.id, ()).is_none() {
                resolved.push(task);
            }
        }
    }

    for id in &args.task {
        if id == "all" {
            for task in tasks::CATALOGUE {
                if seen.insert(task.id, ()).is_none() {
                    resolved.push(task);
                }
            }
            continue;
        }
        let task = tasks::lookup(id).ok_or_else(|| format!("unknown task id: {id}"))?;
        if seen.insert(task.id, ()).is_none() {
            resolved.push(task);
        }
    }

    Ok(resolved)
}

fn resolve_harnesses(args: &RunArgs) -> Result<Vec<String>, String> {
    if args.harness.is_empty() {
        return Err("no harness selected: pass at least one --harness".to_string());
    }
    Ok(args.harness.clone())
}

fn parse_tier(name: &str) -> Result<Tier, String> {
    match name {
        "leaf" => Ok(Tier::Leaf),
        "standard" => Ok(Tier::Standard),
        "heavy" => Ok(Tier::Heavy),
        other => Err(format!(
            "unknown tier: {other} (want leaf, standard or heavy)"
        )),
    }
}

/// The JSON shape one attempt is written as. Separate from [`Attempt`]
/// because that type is frozen and carries no `Serialize` impl.
#[derive(Serialize)]
struct AttemptRecord<'a> {
    task: &'a str,
    tier: &'static str,
    harness: &'a str,
    base_commit: &'a str,
    attempt: u32,
    outcome: &'static str,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cached_input_tokens: Option<u64>,
    wall_clock_ms: u128,
    turns: Option<u32>,
    changed_lines: Option<u32>,
}

impl<'a> From<&'a Attempt> for AttemptRecord<'a> {
    fn from(a: &'a Attempt) -> Self {
        AttemptRecord {
            task: a.task,
            tier: a.tier.as_str(),
            harness: a.harness.as_str(),
            base_commit: &a.base_commit,
            attempt: a.attempt,
            outcome: a.outcome.as_str(),
            input_tokens: a.tokens.input,
            output_tokens: a.tokens.output,
            cached_input_tokens: a.tokens.cached_input,
            wall_clock_ms: a.wall_clock.as_millis(),
            turns: a.turns,
            changed_lines: a.changed_lines,
        }
    }
}

fn write_records(out_dir: &Path, attempts: &[Attempt]) -> Result<(), String> {
    fs::create_dir_all(out_dir)
        .map_err(|e| format!("could not create --out {}: {e}", out_dir.display()))?;
    let path = out_dir.join("attempts.jsonl");
    let mut body = String::new();
    for attempt in attempts {
        let record = AttemptRecord::from(attempt);
        body.push_str(&serde_json::to_string(&record).map_err(|e| e.to_string())?);
        body.push('\n');
    }
    fs::write(&path, body).map_err(|e| format!("could not write {}: {e}", path.display()))
}
