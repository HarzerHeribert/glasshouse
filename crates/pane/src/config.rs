//! Pane runtime settings, layered by the native settings store at startup --
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
    /// Whether a terminal return is held once for the deterministic
    /// final-state contract check and the no-progress guard's findings
    /// (`smarter-cheaper-roadmap.md`, *Evidence-gated completion*). On by
    /// default; off is an ablation switch, not a product mode.
    pub evidence_gate: bool,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            cell_wall_clock_s: 30,
            response_bytes: 16 * 1024,
            // A backstop against a runaway, not a working budget: the task's
            // own wall clock and the stall notice are the controls
            // (`progress::Stall`, 2026-09-14).
            cells: 120,
            evidence_gate: true,
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
    /// Per-helper reasoning levels. These are hard defaults, individually
    /// overridable under `[helpers.effort]`.
    pub effort: HelperEfforts,
    /// Run the pushed Scout before the task model's first turn. Off by
    /// default: configured helpers remain callable without paying for a
    /// redundant repository scan on every request.
    pub preflight: bool,
    /// When `preflight` is on, whether every task pays for the Scout or only
    /// a task whose request carries an uncertainty signal
    /// (`smarter-cheaper-roadmap.md`, *Adaptive orchestration*).
    pub preflight_scope: PreflightScope,
    /// What the completion gate does once a task is accepted.
    pub completion: CompletionStyle,
    /// Run the fresh independent checker on the terminal candidate: the
    /// original request, the task's diff and its exact evidence, never the
    /// parent's rationale. Costs one cheap request per completed task.
    pub completion_check: bool,
    /// Derive an acceptance list from the request before the first turn and
    /// check it when the model claims completion (`acceptance.rs`). One
    /// cheap toolless request per task; the list is shown to the model.
    pub acceptance_list: bool,
    pub enabled: bool,
    /// The most helper calls one cell may make, so a loop cannot issue three
    /// hundred requests inside a single program.
    pub calls_per_cell: u32,
    /// Estimated tokens of a command result above which the pushed reducer
    /// is worth a cheap request. Below it the parent reads the output itself.
    pub reduce_above_tokens: usize,
}

/// `[helpers] preflight_scope` -- which tasks the Scout runs for when
/// preflight is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PreflightScope {
    /// Only when the request names a path that does not exist, a command the
    /// session cannot run, verification the session cannot see, or is long
    /// enough that a scan is expected to save parent attention.
    #[default]
    Auto,
    /// Every task, as before this key existed.
    Always,
}

impl PreflightScope {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "auto" => Ok(Self::Auto),
            "always" => Ok(Self::Always),
            other => Err(format!(
                "pane.toml: `preflight_scope` must be \"auto\" or \"always\", not `{other}`"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Always => "always",
        }
    }
}

/// `[helpers.effort]` -- effort follows the work a helper does rather than
/// the model tier it happens to run on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HelperEfforts {
    pub find: crate::wire::Effort,
    pub reduce: crate::wire::Effort,
    pub check: crate::wire::Effort,
    /// The acceptance lister: fixed line forms from a request.
    pub accept: crate::wire::Effort,
}

impl HelperEfforts {
    pub fn for_helper(self, name: &str) -> Option<crate::wire::Effort> {
        match name {
            "find" => Some(self.find),
            "reduce" => Some(self.reduce),
            "check" => Some(self.check),
            "accept" => Some(self.accept),
            // No silent provider-default fallback: a new helper must choose a
            // hard policy and become a config key before it can run.
            _ => None,
        }
    }
}

impl Default for HelperEfforts {
    fn default() -> Self {
        Self {
            // Lookup is mechanically verifiable; filtering needs more
            // discrimination; accepting or rejecting a claim is the most
            // consequential helper decision.
            find: crate::wire::Effort::Low,
            reduce: crate::wire::Effort::Medium,
            check: crate::wire::Effort::High,
            accept: crate::wire::Effort::Low,
        }
    }
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
            effort: HelperEfforts::default(),
            preflight: false,
            preflight_scope: PreflightScope::Auto,
            completion: CompletionStyle::Silent,
            completion_check: false,
            acceptance_list: true,
            enabled: true,
            calls_per_cell: 8,
            reduce_above_tokens: 2048,
        }
    }
}

/// nothing -- holds here too: nothing in this module opens a path for writing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PaneConfig {
    pub limits: Limits,
    pub supervisor: SupervisorConfig,
    pub helpers: HelpersConfig,
    pub agents: AgentsConfig,
    pub model: ModelConfig,
    pub web: crate::web::WebConfig,
}

/// `[model]` -- the parent tier, the one the person talks to.
///
/// It is here for the same reason `[helpers] model` and `[agents] model` are:
/// a session is three models, and the one you chose last should not be the
/// only one that forgets. There is deliberately no compiled-in fallback:
/// startup requires either this value or `--model`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelConfig {
    pub parent: Option<String>,
}

/// `[agents]` -- what a subagent runs on when the cell does not say.
///
/// Without this a delegated goal inherits the **parent's** model, so a session
/// driven by a frontier model pays frontier rates for every investigation it
/// hands off, unless the model remembers to name a cheaper one each time.
/// A model the cell names still wins: this is a default, not a ceiling.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AgentsMode {
    /// Use a model named by the cell, otherwise inherit the parent.
    #[default]
    Auto,
    /// Refuse every subagent spawn, including one that names a model.
    Off,
    /// Use `model` unless the cell explicitly names another model.
    Pinned,
}

impl AgentsMode {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "auto" => Ok(Self::Auto),
            "off" => Ok(Self::Off),
            "pinned" => Ok(Self::Pinned),
            other => Err(format!(
                "pane.toml: `[agents] mode` must be \"auto\", \"off\" or \"pinned\", not `{other}`"
            )),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentsConfig {
    pub mode: AgentsMode,
    pub model: Option<String>,
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
const REDUCE_ABOVE_TOKENS: Range = Range {
    key: "reduce_above_tokens",
    min: 256,
    max: 32_768,
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
    /// Loads global and `<root>/.pane/config.toml` settings. Missing files use defaults,
    /// never an error -- most projects have none.
    pub fn load(root: &Path) -> Result<Self, String> {
        Self::load_profile(root, None)
    }

    /// Select a named configuration overlay without modifying project defaults.
    pub fn load_profile(root: &Path, profile: Option<&str>) -> Result<Self, String> {
        Ok(crate::settings::Store::new(root)?.load(profile)?.config)
    }

    pub fn parse_profile(text: &str, selected: Option<&str>) -> Result<Self, String> {
        let mut value: toml::Value =
            toml::from_str(text).map_err(|error| format!("pane.toml: {error}"))?;
        let table = value.as_table_mut().ok_or("pane.toml must be a table")?;
        let profiles = table.remove("profiles");
        if let Some(profiles) = &profiles {
            let profiles = profiles
                .as_table()
                .ok_or("pane.toml: [profiles] must be a table")?;
            for (name, overlay) in profiles {
                if name.is_empty()
                    || !name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
                    || !overlay.is_table()
                {
                    return Err(
                        "pane.toml: profiles must be named tables (letters, digits, _ or -)".into(),
                    );
                }
            }
        }
        if let Some(name) = selected {
            let overlay = profiles
                .as_ref()
                .and_then(|profiles| profiles.get(name))
                .ok_or_else(|| format!("pane.toml: no profile named `{name}`"))?;
            if let Some(agents) = overlay.get("agents").and_then(toml::Value::as_table)
                && matches!(
                    agents.get("mode").and_then(toml::Value::as_str),
                    Some("off" | "auto")
                )
                && !agents.contains_key("model")
                && let Some(base_agents) =
                    value.get_mut("agents").and_then(toml::Value::as_table_mut)
            {
                base_agents.remove("model");
            }
            merge_tables(&mut value, overlay);
        }
        Self::parse_base(&toml::to_string(&value).map_err(|error| format!("pane.toml: {error}"))?)
    }

    /// Parses without touching the filesystem.
    ///
    /// Public so that a caller about to **write** `pane.toml` can prove the
    /// text it is about to save loads — the same check `/permissions` makes
    /// by compiling a profile before saving `settings.json`. Reading stays
    /// this module's only filesystem verb; the write belongs to the command
    /// that made the edit.
    pub fn parse(text: &str) -> Result<Self, String> {
        Self::parse_profile(text, None)
    }

    fn parse_base(text: &str) -> Result<Self, String> {
        let value: toml::Value = toml::from_str(text).map_err(|e| format!("pane.toml: {e}"))?;
        let table = value.as_table().ok_or_else(|| {
            "pane.toml: must be a table of [limits], [supervisor], [helpers] and [agents]"
                .to_string()
        })?;

        for key in table.keys() {
            if !["limits", "supervisor", "helpers", "agents", "model", "web"]
                .contains(&key.as_str())
            {
                return Err(format!(
                    "pane.toml: unknown table `[{key}]`; only [limits], [supervisor], [helpers], \
                     [agents], [model] and [web] are recognised"
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

        let agents = match table.get("agents") {
            Some(value) => parse_agents(value)?,
            None => AgentsConfig::default(),
        };

        let model = match table.get("model") {
            Some(value) => parse_model(value)?,
            None => ModelConfig::default(),
        };

        let web = match table.get("web") {
            Some(value) => value
                .clone()
                .try_into::<crate::web::WebConfig>()
                .map_err(|error| format!("pane.toml: [web]: {error}"))?,
            None => crate::web::WebConfig::default(),
        };
        crate::web::WebBroker::new(web.clone())?;

        Ok(Self {
            limits,
            supervisor,
            helpers,
            agents,
            model,
            web,
        })
    }
}

fn merge_tables(base: &mut toml::Value, overlay: &toml::Value) {
    if let (Some(base), Some(overlay)) = (base.as_table_mut(), overlay.as_table()) {
        for (key, value) in overlay {
            match base.get_mut(key) {
                Some(existing) if existing.is_table() && value.is_table() => {
                    merge_tables(existing, value)
                }
                _ => {
                    base.insert(key.clone(), value.clone());
                }
            }
        }
    }
}

/// `[agents]` -- an explicit mode plus a model for `pinned`.
///
/// A legacy table containing only `model` is interpreted as `pinned`, so an
/// existing project keeps the behaviour it selected before modes existed.
fn parse_agents(value: &toml::Value) -> Result<AgentsConfig, String> {
    let table = table_of(value, "agents")?;
    for key in table.keys() {
        if !["mode", "model"].contains(&key.as_str()) {
            return Err(format!(
                "pane.toml: unknown key `{key}` in [agents]; only `mode` and `model` are recognised"
            ));
        }
    }
    let model = match table.get("model") {
        None => None,
        Some(value) => {
            let text = value
                .as_str()
                .ok_or_else(|| "pane.toml: `[agents] model` must be a string".to_string())?;
            validate_concrete_model("[agents] model", text)?;
            Some(text.to_string())
        }
    };
    let mode = match table.get("mode") {
        None if model.is_some() => AgentsMode::Pinned,
        None => AgentsMode::Auto,
        Some(value) => AgentsMode::parse(
            value
                .as_str()
                .ok_or_else(|| "pane.toml: `[agents] mode` must be a string".to_string())?,
        )?,
    };
    match (mode, model.as_ref()) {
        (AgentsMode::Pinned, None) => {
            return Err("pane.toml: `[agents] mode = \"pinned\"` requires `model`".to_string());
        }
        (AgentsMode::Auto | AgentsMode::Off, Some(_)) => {
            return Err(format!(
                "pane.toml: `[agents] mode = \"{}\"` cannot also set `model`",
                match mode {
                    AgentsMode::Auto => "auto",
                    AgentsMode::Off => "off",
                    AgentsMode::Pinned => unreachable!(),
                }
            ));
        }
        _ => {}
    }
    Ok(AgentsConfig { mode, model })
}

/// `[model] parent` -- one optional key, refused the same way the other two
/// tiers' model names are, so one validator covers all three.
fn parse_model(value: &toml::Value) -> Result<ModelConfig, String> {
    let table = table_of(value, "model")?;
    for key in table.keys() {
        if key != "parent" {
            return Err(format!(
                "pane.toml: unknown key `{key}` in [model]; only `parent` is recognised"
            ));
        }
    }
    let parent = match table.get("parent") {
        None => None,
        Some(value) => {
            let text = value
                .as_str()
                .ok_or_else(|| "pane.toml: `parent` must be a string".to_string())?;
            validate_parent_model(text)?;
            Some(text.to_string())
        }
    };
    Ok(ModelConfig { parent })
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
            "evidence_gate",
        ]
        .contains(&key.as_str())
        {
            return Err(format!("pane.toml: unknown key `{key}` in [limits]"));
        }
    }
    let evidence_gate = match table.get("evidence_gate") {
        None => defaults.evidence_gate,
        Some(value) => value
            .as_bool()
            .ok_or_else(|| "pane.toml: `evidence_gate` must be true or false".to_string())?,
    };

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
        evidence_gate,
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
        if ![
            "model",
            "effort",
            "enabled",
            "preflight",
            "preflight_scope",
            "calls_per_cell",
            "completion",
            "completion_check",
            "acceptance_list",
            "reduce_above_tokens",
        ]
        .contains(&key.as_str())
        {
            return Err(format!("pane.toml: unknown key `{key}` in [helpers]"));
        }
    }

    let model = match table.get("model") {
        None => None,
        Some(value) => {
            let text = value
                .as_str()
                .ok_or_else(|| "pane.toml: `model` must be a string".to_string())?;
            validate_concrete_model("[helpers] model", text)?;
            Some(text.to_string())
        }
    };
    let effort = match table.get("effort") {
        None => defaults.effort,
        Some(value) => parse_helper_efforts(value)?,
    };
    let enabled = match table.get("enabled") {
        None => defaults.enabled,
        Some(value) => value
            .as_bool()
            .ok_or_else(|| "pane.toml: `enabled` must be true or false".to_string())?,
    };
    let preflight = match table.get("preflight") {
        None => defaults.preflight,
        Some(value) => value
            .as_bool()
            .ok_or_else(|| "pane.toml: `preflight` must be true or false".to_string())?,
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
    let preflight_scope = match table.get("preflight_scope") {
        None => defaults.preflight_scope,
        Some(value) => PreflightScope::parse(
            value
                .as_str()
                .ok_or_else(|| "pane.toml: `preflight_scope` must be a string".to_string())?,
        )?,
    };
    let acceptance_list = match table.get("acceptance_list") {
        None => defaults.acceptance_list,
        Some(value) => value
            .as_bool()
            .ok_or_else(|| "pane.toml: `acceptance_list` must be true or false".to_string())?,
    };
    let completion_check = match table.get("completion_check") {
        None => defaults.completion_check,
        Some(value) => value
            .as_bool()
            .ok_or_else(|| "pane.toml: `completion_check` must be true or false".to_string())?,
    };
    let reduce_above_tokens = match int_field(table, "reduce_above_tokens")? {
        Some(v) => usize::try_from(REDUCE_ABOVE_TOKENS.check(v)?).expect("range is non-negative"),
        None => defaults.reduce_above_tokens,
    };

    Ok(HelpersConfig {
        model,
        effort,
        preflight,
        preflight_scope,
        completion,
        completion_check,
        acceptance_list,
        enabled,
        calls_per_cell,
        reduce_above_tokens,
    })
}

fn parse_helper_efforts(value: &toml::Value) -> Result<HelperEfforts, String> {
    let table = table_of(value, "helpers.effort")?;
    for key in table.keys() {
        if !["find", "reduce", "check", "accept"].contains(&key.as_str()) {
            return Err(format!(
                "pane.toml: unknown key `{key}` in [helpers.effort]; only `find`, `reduce`, `check` and `accept` are recognised"
            ));
        }
    }
    let defaults = HelperEfforts::default();
    let read = |name: &str, default| match table.get(name) {
        None => Ok(default),
        Some(value) => {
            let word = value
                .as_str()
                .ok_or_else(|| format!("pane.toml: `[helpers.effort] {name}` must be a string"))?;
            let effort = crate::wire::Effort::parse(word).ok_or_else(|| {
                format!(
                    "pane.toml: `[helpers.effort] {name}` must be low, medium, high, xhigh or max, not `{word}`"
                )
            })?;
            if effort == crate::wire::Effort::Default {
                return Err(format!(
                    "pane.toml: `[helpers.effort] {name}` must be a hard value: low, medium, high, xhigh or max"
                ));
            }
            Ok(effort)
        }
    };
    Ok(HelperEfforts {
        find: read("find", defaults.find)?,
        reduce: read("reduce", defaults.reduce)?,
        accept: read("accept", defaults.accept)?,
        check: read("check", defaults.check)?,
    })
}

/// SECURITY / ISOLATION: `pane.toml` can name no tool, a path or a grant --
/// those are the sandbox's (`sandbox-grants.md`) and stay there. A path
/// separator, a glob character, or a registered tool's own name refuses the
/// value with one sentence naming the key.
fn check_names_no_tool_path_or_grant(key: &str, value: &str) -> Result<(), String> {
    // Gateway catalogues legitimately use provider-qualified ids such as
    // `vendor/model-302`. A slash alone therefore cannot mean "path". Path
    // roots and traversal segments can, and no concrete model id needs a
    // backslash, glob, or permission-expression parenthesis.
    let slash_segments: Vec<_> = value.split('/').collect();
    let bytes = value.as_bytes();
    let windows_drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    let looks_like_a_path_or_glob = value.starts_with('/')
        || value.starts_with("~/")
        || windows_drive
        || value.contains('\\')
        || value.contains('*')
        || value.contains('?')
        || value.contains('(')
        || value.contains(')')
        || slash_segments
            .iter()
            .any(|segment| segment.is_empty() || matches!(*segment, "." | ".."));
    let names_a_tool = registry::names().contains(&value);
    if looks_like_a_path_or_glob || names_a_tool {
        return Err(format!("pane.toml: `{key}` names no tool, path or grant"));
    }
    Ok(())
}

/// Validates a model used by the parent. Mode words are controls, never
/// concrete request model identifiers.
pub fn validate_parent_model(value: &str) -> Result<(), String> {
    validate_concrete_model("[model] parent", value)
}

fn validate_concrete_model(key: &str, value: &str) -> Result<(), String> {
    if value.is_empty() || value.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!("pane.toml: `{key}` must be one concrete model id"));
    }
    if matches!(value, "auto" | "off" | "inherit") {
        return Err(format!(
            "pane.toml: `{key}` must be a concrete model id, not `{value}`"
        ));
    }
    check_names_no_tool_path_or_grant(key, value)
}
