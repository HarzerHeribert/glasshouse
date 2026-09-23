//! `pane ruler run`: parse the command, resolve the task and harness sets,
//! run every (task, harness, attempt) combination, print the per-tier table
//! and write one JSON line per attempt.
//!
//! **Both renderings are [`super::report`]'s and neither is spelled here.**
//! This module briefly carried its own `Serialize` record, which meant the
//! column set [`super::report::JSONL_KEYS`] pins -- map line 2432's whole
//! enforcement -- guarded a renderer the command never called, and the two
//! spellings had already drifted on four keys.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use clap::Parser;

use super::attempt::{self, HarnessCommand, RunOpts};
use super::decisions;
use super::interface::CreditRatios;
use super::meter::Meter;
use super::model::{Attempt, Harness, Task, Tier};
use super::report;
use super::score::Score;
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
    "--via-glasshouse",
    "--pane-interface",
    "--credit-ratio",
    "--pane-decisions",
    "--decisions-model",
    "--parent-model",
    "--pane-feedback",
    "--helpers-model",
    "--out",
];

/// The three decision modes `--pane-decisions` accepts, exactly
/// `decision-model.md` §1's `mode` values.
const DECISIONS_MODES: [&str; 3] = ["off", "shadow", "on"];

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
    /// Sets `ANTHROPIC_BASE_URL` on the harness child, for a standing
    /// gateway -- none exists today
    /// (`.agent-runtime/pane/ask-primary-gateway-for-benchmark.md`'s ANSWER
    /// §1). `--via-glasshouse` is how a run gets a gateway instead; the two
    /// refuse each other.
    #[arg(long)]
    pub gateway: Option<String>,
    /// Path to the `glasshouse` executable to read exchange rows from (via
    /// `routing-cost --json`). Omitted means no meter: tokens and turns are
    /// absent for every attempt, never a fabricated zero.
    #[arg(long)]
    pub meter: Option<PathBuf>,
    /// Launches each row through `<glasshouse> launch <row> --profile
    /// <profile> -- <substituted argv>` in the attempt's own worktree
    /// instead of the row's own program, reading the meter from that same
    /// worktree -- `ruler.md` §3's "one meter" is the project ledger, not a
    /// standing process. A bare value applies one profile to every selected
    /// row; `<row>=<profile>`, repeated once per row, gives each row its own
    /// and requires every selected row to have one. Requires `--meter` (the
    /// launch and the ledger are one binary) and refuses `--gateway` (no
    /// standing gateway to point at).
    #[arg(long)]
    pub via_glasshouse: Vec<String>,
    /// Expands the `pane` row into one `pane:<mode>` arm per listed mode
    /// (`hybrid,cells,tools`), each launched with the row's own argv plus
    /// `--interface <mode> --output-format json`, its stdout captured for
    /// the interface-regret table. Refused unless `pane` is a selected row.
    #[arg(long, value_delimiter = ',')]
    pub pane_interface: Vec<String>,
    /// Helper-token credit ratios for the regret table's weighted spend,
    /// `luna=0.2,terra=0.1`. Both default to 1.0 and the table then says
    /// "assumed ratio"; nothing here is a billed figure.
    #[arg(long)]
    pub credit_ratio: Option<String>,
    /// Expands the `pane` row into one `pane:decisions-<mode>` arm per
    /// listed mode (`off,shadow,on`), each attempt's worktree getting a
    /// `.pane/config.toml` written right after `cut_worktree` and before the
    /// harness launches -- the mode never travels as a session flag. Refused
    /// unless `pane` is a selected row, together with `--pane-interface`
    /// (one expansion at a time), or listing a mode twice or outside the
    /// three known ones.
    #[arg(long, value_delimiter = ',')]
    pub pane_decisions: Vec<String>,
    /// The decision model named in a `shadow`/`on` arm's `.pane/config.toml`.
    /// Required by every `--pane-decisions` mode but `off`, which gets no
    /// `[decisions]` table at all.
    #[arg(long)]
    pub decisions_model: Option<String>,
    /// The model every `pane` row's session runs with, written into each
    /// attempt's own `.pane/config.toml` as `[model] parent`.
    ///
    /// **Required whenever a `pane` row is selected.** An attempt's worktree
    /// is cut detached and carries no configuration, so without this `pane
    /// session` refuses to start and the attempt measures nothing --
    /// silently, because the task's own tests then run against an untouched
    /// tree. The other rows configure their own model and ignore this.
    #[arg(long)]
    pub parent_model: Option<String>,
    /// Expands the `pane` row into one `pane:feedback-<arm>` arm per listed
    /// arm of [`attempt::FEEDBACK_ARMS`] (`bare,shadow,scout,dissect,reduce,prefetch,all`),
    /// each attempt's `.pane/config.toml` carrying the decision mode and the
    /// `[helpers]` switches of its arm. The decision model is
    /// `--decisions-model`, or Jev's default. Refused beside the other two
    /// expansions: one expansion at a time.
    #[arg(long, value_delimiter = ',')]
    pub pane_feedback: Vec<String>,
    /// The helper model every `pane` row's session runs its Scout, its
    /// acceptance lister and its checker with, written as `[helpers] model`.
    /// Without it no helper runs in an attempt.
    #[arg(long)]
    pub helpers_model: Option<String>,
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
    if !args.via_glasshouse.is_empty() && args.meter.is_none() {
        return Err(
            "--via-glasshouse needs --meter <glasshouse>: the launch and the ledger are one binary"
                .to_string(),
        );
    }
    if !args.via_glasshouse.is_empty() && args.gateway.is_some() {
        return Err(
            "--via-glasshouse and --gateway cannot be combined: --gateway is for a standing gateway, and none exists today"
                .to_string(),
        );
    }
    let expansions = [
        !args.pane_interface.is_empty(),
        !args.pane_decisions.is_empty(),
        !args.pane_feedback.is_empty(),
    ];
    if expansions.iter().filter(|given| **given).count() > 1 {
        return Err(
            "--pane-interface, --pane-decisions and --pane-feedback cannot be combined: one expansion at a time"
                .to_string(),
        );
    }

    let selected = resolve_harnesses(&args)?;
    if args.parent_model.is_none()
        && let Some(row) = selected.iter().find(|row| attempt::is_pane_row(row))
    {
        return Err(format!(
            "--parent-model <id> is required to run the `{row}` row: an attempt's worktree carries no .pane/config.toml, so `pane session` would refuse to start and the attempt would measure nothing"
        ));
    }
    if let Some(model) = &args.parent_model {
        crate::config::validate_parent_model(model)?;
    }
    let via_glasshouse = resolve_via_glasshouse(&args.via_glasshouse, &selected)?;
    let ratios = match &args.credit_ratio {
        Some(text) => CreditRatios::parse(text)?,
        None => CreditRatios::default(),
    };

    let mut harness_table = attempt::default_harnesses();
    let harnesses = expand_pane_interfaces(&selected, &args.pane_interface, &mut harness_table)?;
    let harnesses = expand_pane_decisions(
        &harnesses,
        &args.pane_decisions,
        args.decisions_model.as_deref(),
        &mut harness_table,
    )?;
    let harnesses = expand_pane_feedback(
        &harnesses,
        &args.pane_feedback,
        args.decisions_model
            .as_deref()
            .unwrap_or(crate::decide::DEFAULT_MODEL),
        &mut harness_table,
    )?;
    let via_glasshouse = via_glasshouse.map(|profiles| {
        harnesses
            .iter()
            .filter_map(|row| {
                let base = row.split_once(':').map_or(row.as_str(), |(base, _)| base);
                profiles
                    .get(base)
                    .map(|profile| (row.clone(), profile.clone()))
            })
            .collect()
    });

    let tasks = resolve_tasks(&args)?;
    if tasks.is_empty() {
        return Err("no tasks selected: pass --task or --tier".to_string());
    }

    let opts = RunOpts {
        scratch: std::env::temp_dir().join("pane-ruler"),
        gateway: args.gateway.clone(),
        via_glasshouse,
        meter: match &args.meter {
            Some(glasshouse) => Meter::Command {
                glasshouse: glasshouse.clone(),
            },
            None => Meter::None,
        },
        harnesses: harness_table,
        parent_model: args.parent_model.clone(),
        helpers_model: args.helpers_model.clone(),
        // Created before the first attempt rather than with the records at
        // the end: an attempt writes its rollout while it runs, and a
        // missing directory would leave every one of them unstated.
        rollouts: Some(args.out.clone()),
    };
    fs::create_dir_all(&args.out)
        .map_err(|e| format!("could not create --out {}: {e}", args.out.display()))?;

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

    print!(
        "{}",
        report::render_table(&Score::with_ratios(&attempts, ratios))
    );
    print!(
        "{}",
        report::render_decisions_table(&decisions::rows(&attempts))
    );
    print!("{}", report::render_rubric_table(&attempts));
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

/// Replaces the `pane` row in `selected` with one `pane:<mode>` row per
/// mode, adding each arm's [`HarnessCommand`] to `table`. No modes: the
/// selection is returned unchanged. Refuses a mode `Interface::parse` does
/// not know, a mode listed twice, and any mode list when `pane` was not
/// selected -- there is no row to expand.
pub fn expand_pane_interfaces(
    selected: &[String],
    modes: &[String],
    table: &mut HashMap<String, HarnessCommand>,
) -> Result<Vec<String>, String> {
    if modes.is_empty() {
        return Ok(selected.to_vec());
    }
    if !selected.iter().any(|row| row == "pane") {
        return Err(
            "--pane-interface expands the pane row, and --harness did not select pane".to_string(),
        );
    }
    let mut parsed = Vec::new();
    for mode in modes {
        let mode = crate::abi::Interface::parse(mode)?.as_str();
        if parsed.contains(&mode) {
            return Err(format!("--pane-interface names {mode} twice"));
        }
        parsed.push(mode);
    }
    let pane = table
        .get("pane")
        .cloned()
        .ok_or_else(|| "the harness table has no pane row to expand".to_string())?;
    let mut rows = Vec::new();
    for row in selected {
        if row != "pane" {
            rows.push(row.clone());
            continue;
        }
        for mode in &parsed {
            let name = attempt::pane_arm_name(mode);
            table.insert(
                name.clone(),
                HarnessCommand::pane_interface_arm(&pane, mode),
            );
            rows.push(name);
        }
    }
    Ok(rows)
}

/// Replaces the `pane` row in `selected` with one `pane:decisions-<mode>`
/// row per mode, adding each arm's [`HarnessCommand`] to `table`. No modes:
/// the selection is returned unchanged. Refuses a mode outside
/// [`DECISIONS_MODES`], a mode listed twice, any mode list when `pane` was
/// not selected, and `shadow`/`on` without `model` -- all before any
/// worktree is cut. `off` never needs `model`: unset model already means
/// off, and the arm gets no `[decisions]` table.
pub fn expand_pane_decisions(
    selected: &[String],
    modes: &[String],
    model: Option<&str>,
    table: &mut HashMap<String, HarnessCommand>,
) -> Result<Vec<String>, String> {
    if modes.is_empty() {
        return Ok(selected.to_vec());
    }
    if !selected.iter().any(|row| row == "pane") {
        return Err(
            "--pane-decisions expands the pane row, and --harness did not select pane".to_string(),
        );
    }
    let mut parsed = Vec::new();
    for mode in modes {
        if !DECISIONS_MODES.contains(&mode.as_str()) {
            return Err(format!(
                "--pane-decisions knows off, shadow and on, not `{mode}`"
            ));
        }
        if parsed.contains(mode) {
            return Err(format!("--pane-decisions names {mode} twice"));
        }
        parsed.push(mode.clone());
    }
    for mode in &parsed {
        if mode != "off" && model.is_none() {
            return Err(format!(
                "--pane-decisions {mode} needs --decisions-model <name>"
            ));
        }
    }
    let pane = table
        .get("pane")
        .cloned()
        .ok_or_else(|| "the harness table has no pane row to expand".to_string())?;
    let mut rows = Vec::new();
    for row in selected {
        if row != "pane" {
            rows.push(row.clone());
            continue;
        }
        for mode in &parsed {
            let name = attempt::decisions_arm_name(mode);
            let arm_model = if mode == "off" { None } else { model };
            table.insert(
                name.clone(),
                HarnessCommand::pane_decisions_arm(&pane, arm_model, mode),
            );
            rows.push(name);
        }
    }
    Ok(rows)
}

/// Replaces the `pane` row in `selected` with one `pane:feedback-<arm>` row
/// per arm, adding each arm's [`HarnessCommand`] to `table`. No arms: the
/// selection is returned unchanged. Refuses an arm outside
/// [`attempt::FEEDBACK_ARMS`], an arm listed twice, and any arm list when
/// `pane` was not selected -- all before any worktree is cut.
pub fn expand_pane_feedback(
    selected: &[String],
    arms: &[String],
    model: &str,
    table: &mut HashMap<String, HarnessCommand>,
) -> Result<Vec<String>, String> {
    if arms.is_empty() {
        return Ok(selected.to_vec());
    }
    if !selected.iter().any(|row| row == "pane") {
        return Err(
            "--pane-feedback expands the pane row, and --harness did not select pane".to_string(),
        );
    }
    let mut parsed = Vec::new();
    for name in arms {
        let arm = attempt::FEEDBACK_ARMS
            .iter()
            .find(|arm| arm.name == name)
            .ok_or_else(|| {
                format!(
                    "--pane-feedback knows bare, shadow, scout, dissect, reduce, prefetch and all, not `{name}`"
                )
            })?;
        if parsed.contains(&arm) {
            return Err(format!("--pane-feedback names {name} twice"));
        }
        parsed.push(arm);
    }
    let pane = table
        .get("pane")
        .cloned()
        .ok_or_else(|| "the harness table has no pane row to expand".to_string())?;
    let mut rows = Vec::new();
    for row in selected {
        if row != "pane" {
            rows.push(row.clone());
            continue;
        }
        for arm in &parsed {
            let name = attempt::feedback_arm_name(arm.name);
            table.insert(
                name.clone(),
                HarnessCommand::pane_feedback_arm(&pane, model, arm),
            );
            rows.push(name);
        }
    }
    Ok(rows)
}

fn resolve_harnesses(args: &RunArgs) -> Result<Vec<String>, String> {
    if args.harness.is_empty() {
        return Err("no harness selected: pass at least one --harness".to_string());
    }
    Ok(args.harness.clone())
}

/// Parses `--via-glasshouse` into a row -> profile map, or `None` if the
/// flag was not given at all. Refuses, before any attempt runs: a value list
/// mixing the bare and `<row>=<profile>` forms, more than one bare value, a
/// per-row value naming a row `harnesses` did not select, a row named twice,
/// and a selected row left without a profile under the per-row form.
fn resolve_via_glasshouse(
    values: &[String],
    harnesses: &[String],
) -> Result<Option<HashMap<String, String>>, String> {
    if values.is_empty() {
        return Ok(None);
    }

    let per_row: Vec<(&str, &str)> = values.iter().filter_map(|v| v.split_once('=')).collect();
    let bare: Vec<&str> = values
        .iter()
        .filter(|v| !v.contains('='))
        .map(String::as_str)
        .collect();

    if !per_row.is_empty() && !bare.is_empty() {
        return Err(
            "--via-glasshouse cannot mix a bare profile with row=profile entries".to_string(),
        );
    }

    if per_row.is_empty() {
        let profile = match bare.as_slice() {
            [profile] => *profile,
            _ => {
                return Err(
                    "--via-glasshouse takes one bare profile, applied to every selected row"
                        .to_string(),
                );
            }
        };
        return Ok(Some(
            harnesses
                .iter()
                .map(|row| (row.clone(), profile.to_string()))
                .collect(),
        ));
    }

    let mut map = HashMap::new();
    for &(row, profile) in &per_row {
        if !harnesses.iter().any(|h| h == row) {
            return Err(format!(
                "--via-glasshouse names row {row}, which --harness did not select"
            ));
        }
        if map.insert(row.to_string(), profile.to_string()).is_some() {
            return Err(format!("--via-glasshouse names row {row} twice"));
        }
    }
    for row in harnesses {
        if !map.contains_key(row) {
            return Err(format!(
                "--via-glasshouse names no profile for row {row}; give {row}=<profile>"
            ));
        }
    }

    Ok(Some(map))
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

fn write_records(out_dir: &Path, attempts: &[Attempt]) -> Result<(), String> {
    fs::create_dir_all(out_dir)
        .map_err(|e| format!("could not create --out {}: {e}", out_dir.display()))?;
    let path = out_dir.join("attempts.jsonl");
    fs::write(&path, report::render_jsonl(attempts))
        .map_err(|e| format!("could not write {}: {e}", path.display()))
}
