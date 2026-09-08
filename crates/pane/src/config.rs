//! `<project root>/.glasshouse/pane.toml`, read once at session start --
//! `docs/product/pane/supervisor.md` §1. A missing file means every default
//! the runtime limits already used before this package existed
//! (`runtime-contract.md` §7), so an absent file changes no
//! existing test.

use std::path::Path;

use crate::tools::registry;

/// `[limits]` -- the runtime constants `runtime-contract.md` §7 and the cell
/// limit, now loadable. Token spend is telemetry rather than a limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub cell_wall_clock_s: u64,
    pub response_bytes: usize,
    pub cells: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            cell_wall_clock_s: 30,
            response_bytes: 16 * 1024,
            cells: 40,
        }
    }
}

/// `[supervisor]` -- the look's cadence, model and switch (§1, §3). `model`
/// has no default: unset means the supervisor is off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupervisorConfig {
    pub every: u32,
    pub model: Option<String>,
    pub enabled: bool,
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        Self {
            every: 4,
            model: None,
            enabled: true,
        }
    }
}

/// The whole of `pane.toml`. `project.rs`'s own invariant -- loading edits
/// `[helpers]` -- the little-helper tier (`docs/product/pane/little-helpers.md`).
///
/// `model` has no default, exactly as `[supervisor] model` has none: unset
/// means helpers are off, said once at start. A helper spends money on the
/// user's behalf, so the fail-closed direction is *not configured, not run*.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpersConfig {
    pub model: Option<String>,
    /// What the completion gate does once a task is accepted.
    pub completion: CompletionStyle,
    pub enabled: bool,
    /// The most helper calls one cell may make, so a loop cannot issue three
    /// hundred requests inside a single program.
    pub calls_per_cell: u32,
}

/// `[helpers] completion` -- what the gate says when a task is ACCEPTED.
///
/// A refusal is not affected: an unverified completion is reported either way.
/// This is only about the accepted case, where the honest default is silence —
/// a line printed after every task is a line nobody reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompletionStyle {
    /// Say nothing on acceptance.
    #[default]
    Silent,
    /// One or two sentences recapping the session, and one suggested next
    /// prompt. Costs one cheap request per completed task.
    Recap,
}

impl CompletionStyle {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "silent" => Ok(Self::Silent),
            "recap" => Ok(Self::Recap),
            other => Err(format!(
                "pane.toml: `completion` must be \"silent\" or \"recap\", not `{other}`"
            )),
        }
    }
}

impl Default for HelpersConfig {
    fn default() -> Self {
        Self {
            model: None,
            completion: CompletionStyle::Silent,
            enabled: true,
            calls_per_cell: 8,
        }
    }
}

/// nothing -- holds here too: nothing in this module opens a path for writing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PaneConfig {
    pub limits: Limits,
    pub supervisor: SupervisorConfig,
    pub helpers: HelpersConfig,
}

/// One integer key's valid range, spelled once so the refusal sentence and
/// the check it comes from cannot drift apart.
struct Range {
    key: &'static str,
    min: i64,
    max: i64,
}

const CELL_WALL_CLOCK_S: Range = Range {
    key: "cell_wall_clock_s",
    min: 1,
    max: 600,
};
const RESPONSE_BYTES: Range = Range {
    key: "response_bytes",
    min: 1024,
    max: 1_048_576,
};
const CELLS: Range = Range {
    key: "cells",
    min: 1,
    max: 1000,
};
const CALLS_PER_CELL: Range = Range {
    key: "calls_per_cell",
    min: 1,
    max: 64,
};

const EVERY: Range = Range {
    key: "every",
    min: 1,
    max: 100,
};

impl Range {
    fn check(&self, value: i64) -> Result<i64, String> {
        if value < self.min || value > self.max {
            Err(format!(
                "pane.toml: `{}` must be between {} and {}",
                self.key, self.min, self.max
            ))
        } else {
            Ok(value)
        }
    }
}

impl PaneConfig {
    /// Loads `<root>/.glasshouse/pane.toml`. A missing file is the default,
    /// never an error -- most projects have none.
    pub fn load(root: &Path) -> Result<Self, String> {
        match std::fs::read_to_string(root.join(".glasshouse").join("pane.toml")) {
            Ok(text) => Self::parse(&text),
            Err(_) => Ok(Self::default()),
        }
    }

    fn parse(text: &str) -> Result<Self, String> {
        let value: toml::Value = toml::from_str(text).map_err(|e| format!("pane.toml: {e}"))?;
        let table = value.as_table().ok_or_else(|| {
            "pane.toml: must be a table of [limits], [supervisor] and [helpers]".to_string()
        })?;

        for key in table.keys() {
            if key != "limits" && key != "supervisor" && key != "helpers" {
                return Err(format!(
                    "pane.toml: unknown table `[{key}]`; only [limits], [supervisor] and [helpers] are \
                     recognised"
                ));
            }
        }

        let limits = match table.get("limits") {
            Some(value) => parse_limits(value)?,
            None => Limits::default(),
        };
        let supervisor = match table.get("supervisor") {
            Some(value) => parse_supervisor(value)?,
            None => SupervisorConfig::default(),
        };

        let helpers = match table.get("helpers") {
            Some(value) => parse_helpers(value)?,
            None => HelpersConfig::default(),
        };

        Ok(Self {
            limits,
            supervisor,
            helpers,
        })
    }
}

fn table_of<'a>(value: &'a toml::Value, name: &str) -> Result<&'a toml::value::Table, String> {
    value
        .as_table()
        .ok_or_else(|| format!("pane.toml: [{name}] must be a table"))
}

fn int_field(table: &toml::value::Table, key: &str) -> Result<Option<i64>, String> {
    match table.get(key) {
        None => Ok(None),
        Some(value) => value
            .as_integer()
            .map(Some)
            .ok_or_else(|| format!("pane.toml: `{key}` must be an integer")),
    }
}

fn parse_limits(value: &toml::Value) -> Result<Limits, String> {
    let table = table_of(value, "limits")?;
    let defaults = Limits::default();

    for key in table.keys() {
        if ![
            "cell_wall_clock_s",
            "response_bytes",
            // Accepted as a no-op so an existing project does not stop
            // starting when token caps are removed. New sessions account for
            // spend but never use this value to control execution.
            "task_tokens",
            "cells",
        ]
        .contains(&key.as_str())
        {
            return Err(format!("pane.toml: unknown key `{key}` in [limits]"));
        }
    }

    let cell_wall_clock_s = match int_field(table, "cell_wall_clock_s")? {
        Some(v) => u64::try_from(CELL_WALL_CLOCK_S.check(v)?).expect("range is non-negative"),
        None => defaults.cell_wall_clock_s,
    };
    let response_bytes = match int_field(table, "response_bytes")? {
        Some(v) => usize::try_from(RESPONSE_BYTES.check(v)?).expect("range is non-negative"),
        None => defaults.response_bytes,
    };
    if let Some(value) = table.get("task_tokens")
        && !value.is_integer()
    {
        return Err("pane.toml: `task_tokens` must be an integer".into());
    }
    let cells = match int_field(table, "cells")? {
        Some(v) => u64::try_from(CELLS.check(v)?).expect("range is non-negative"),
        None => defaults.cells,
    };

    Ok(Limits {
        cell_wall_clock_s,
        response_bytes,
        cells,
    })
}

fn parse_supervisor(value: &toml::Value) -> Result<SupervisorConfig, String> {
    let table = table_of(value, "supervisor")?;
    let defaults = SupervisorConfig::default();

    for key in table.keys() {
        if !["every", "model", "enabled"].contains(&key.as_str()) {
            return Err(format!("pane.toml: unknown key `{key}` in [supervisor]"));
        }
    }

    let every = match int_field(table, "every")? {
        Some(v) => u32::try_from(EVERY.check(v)?).expect("range is non-negative"),
        None => defaults.every,
    };
    let model = match table.get("model") {
        None => None,
        Some(value) => {
            let text = value
                .as_str()
                .ok_or_else(|| "pane.toml: `model` must be a string".to_string())?;
            check_names_no_tool_path_or_grant("model", text)?;
            Some(text.to_string())
        }
    };
    let enabled = match table.get("enabled") {
        None => defaults.enabled,
        Some(value) => value
            .as_bool()
            .ok_or_else(|| "pane.toml: `enabled` must be true or false".to_string())?,
    };

    Ok(SupervisorConfig {
        every,
        model,
        enabled,
    })
}

fn parse_helpers(value: &toml::Value) -> Result<HelpersConfig, String> {
    let table = table_of(value, "helpers")?;
    let defaults = HelpersConfig::default();

    for key in table.keys() {
        if !["model", "enabled", "calls_per_cell", "completion"].contains(&key.as_str()) {
            return Err(format!("pane.toml: unknown key `{key}` in [helpers]"));
        }
    }

    let model = match table.get("model") {
        None => None,
        Some(value) => {
            let text = value
                .as_str()
                .ok_or_else(|| "pane.toml: `model` must be a string".to_string())?;
            check_names_no_tool_path_or_grant("model", text)?;
            Some(text.to_string())
        }
    };
    let enabled = match table.get("enabled") {
        None => defaults.enabled,
        Some(value) => value
            .as_bool()
            .ok_or_else(|| "pane.toml: `enabled` must be true or false".to_string())?,
    };
    let calls_per_cell = match int_field(table, "calls_per_cell")? {
        Some(v) => u32::try_from(CALLS_PER_CELL.check(v)?).expect("range is non-negative"),
        None => defaults.calls_per_cell,
    };

    let completion = match table.get("completion") {
        None => defaults.completion,
        Some(value) => CompletionStyle::parse(
            value
                .as_str()
                .ok_or_else(|| "pane.toml: `completion` must be a string".to_string())?,
        )?,
    };

    Ok(HelpersConfig {
        model,
        completion,
        enabled,
        calls_per_cell,
    })
}

/// SECURITY / ISOLATION: `pane.toml` can name no tool, a path or a grant --
/// those are the sandbox's (`sandbox-grants.md`) and stay there. A path
/// separator, a glob character, or a registered tool's own name refuses the
/// value with one sentence naming the key.
fn check_names_no_tool_path_or_grant(key: &str, value: &str) -> Result<(), String> {
    let looks_like_a_path_or_glob =
        value.contains('/') || value.contains('\\') || value.contains('*') || value.contains('?');
    let names_a_tool = registry::names().contains(&value);
    if looks_like_a_path_or_glob || names_a_tool {
        return Err(format!("pane.toml: `{key}` names no tool, path or grant"));
    }
    Ok(())
}
