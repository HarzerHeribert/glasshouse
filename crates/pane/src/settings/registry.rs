//! The one typed settings registry -- `docs/product/pane/settings-experience.md`
//! §"Advanced `/config` and CLI counterpart": *one* registry defines every
//! supported key for file loading, the panel, the slash command and the CLI,
//! so a second permissive parser cannot grow beside it.
//!
//! Two rules hold this module together. The first is that an unknown key is
//! refused rather than carried: [`validate`] answers `Err` for anything
//! [`specs`] does not list, which is what keeps a typo out of a saved file
//! and out of the effective configuration. The second is that no range,
//! model rule or web constraint is restated here. A runtime key is checked by
//! building the smallest possible document that sets it and handing that to
//! [`crate::config::PaneConfig::parse_profile`] -- the same parser a session
//! start uses -- so the refusal sentence and the check it comes from cannot
//! drift apart.

use crate::config::PaneConfig;

/// What a value looks like once typed. Plain strings never need TOML quoting;
/// everything else is parsed from the word the user typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `true` or `false`.
    Bool,
    /// A signed integer; the range belongs to the runtime parser.
    Integer,
    /// A floating-point number; the range belongs to the runtime parser.
    Float,
    /// One of `choices`, and nothing else.
    Choice,
    /// One concrete model id, refused the way `pane.toml` refuses one.
    Model,
    /// Free text, validated by whatever owns the key.
    Text,
    /// A list of strings: `a, b, c` or a TOML array literal.
    List,
}

/// One supported key, as every surface reads it.
#[derive(Debug, Clone, Copy)]
pub struct SettingSpec {
    /// The dotted key, exactly as it is spelled in the file.
    pub key: &'static str,
    /// The short name a panel row shows.
    pub label: &'static str,
    /// One sentence: what it does, and what it does not do.
    pub description: &'static str,
    /// How the written word becomes a value.
    pub kind: Kind,
    /// The complete set of valid words for [`Kind::Choice`], empty otherwise.
    pub choices: &'static [&'static str],
    /// Everyday setting (`/settings`' curated panel) rather than advanced.
    pub basic: bool,
    /// Whether a saved change needs a new session before it is in force.
    pub restart: bool,
}

/// Reasoning effort as the parent tier accepts it: `default` means *the
/// provider's own*, which is not the same as removing a saved override.
const EFFORT: &[&str] = &["default", "low", "medium", "high", "xhigh", "max"];
/// Per-helper effort is a hard policy (`config.rs`): a helper never inherits
/// `default`, so the curated word is absent here on purpose.
const HARD_EFFORT: &[&str] = &["low", "medium", "high", "xhigh", "max"];
/// `tui::Theme::ALL`, spelled for the registry. `tests/settings_store.rs`
/// asserts the two lists stay identical, so a new theme cannot appear in the
/// picker and be unsavable.
const THEMES: &[&str] = &[
    "neon", "amber", "ice", "mono", "violet", "cobalt", "mint", "rose",
];
const STATUS_LINES: &[&str] = &["full", "compact", "hidden"];
const SIDEBAR: &[&str] = &["auto", "show", "hide"];
/// The working mode a session starts in. `build` is this file's word for the
/// runtime's `execute`; both are accepted, and `build` is what is written.
const MODES: &[&str] = &["build", "plan"];
const AGENT_MODES: &[&str] = &["auto", "off", "pinned"];
const COMPLETION: &[&str] = &["silent", "recap"];
const PREFLIGHT_SCOPE: &[&str] = &["auto", "always"];
const DECISION_MODES: &[&str] = &["off", "shadow", "on"];

/// The top-level tables `PaneConfig` parses. A key under one of these is
/// validated by the runtime parser; everything else is owned here.
pub(crate) const RUNTIME_TABLES: [&str; 8] = [
    "limits",
    "supervisor",
    "helpers",
    "agents",
    "model",
    "web",
    "decisions",
    "modes",
];

/// Whether a key belongs to a runtime table, and so reaches `PaneConfig`.
pub fn is_runtime(key: &str) -> bool {
    key.split('.')
        .next()
        .is_some_and(|table| RUNTIME_TABLES.contains(&table))
}

static SPECS: &[SettingSpec] = &[
    // -- the three model tiers, the everyday half of `/settings` ----------
    SettingSpec {
        key: "model.parent",
        label: "Main model",
        description: "The model you talk to. One concrete model id; `--model` still wins for one session.",
        kind: Kind::Model,
        choices: &[],
        basic: true,
        restart: true,
    },
    SettingSpec {
        key: "session.effort",
        label: "Reasoning effort",
        description: "Effort for the main model. `default` is the provider's own setting, not a removed override.",
        kind: Kind::Choice,
        choices: EFFORT,
        basic: true,
        restart: true,
    },
    SettingSpec {
        key: "session.mode",
        label: "Working mode",
        description: "The mode a new session starts in. `build` is the runtime's `execute`.",
        kind: Kind::Choice,
        choices: MODES,
        basic: true,
        restart: true,
    },
    SettingSpec {
        key: "agents.mode",
        label: "Subagent mode",
        description: "`auto` inherits the parent, `off` refuses every spawn, `pinned` requires `agents.model`.",
        kind: Kind::Choice,
        choices: AGENT_MODES,
        basic: true,
        restart: true,
    },
    SettingSpec {
        key: "agents.model",
        label: "Subagent model",
        description: "The model a delegated goal runs on. Only valid with `agents.mode = pinned`.",
        kind: Kind::Model,
        choices: &[],
        basic: true,
        restart: true,
    },
    SettingSpec {
        key: "helpers.model",
        label: "Helper model",
        description: "The little-helper tier. Unset means helpers are off; they are never run unconfigured.",
        kind: Kind::Model,
        choices: &[],
        basic: true,
        restart: true,
    },
    SettingSpec {
        key: "helpers.enabled",
        label: "Helpers",
        description: "Whether configured helpers run at all.",
        kind: Kind::Bool,
        choices: &[],
        basic: true,
        restart: true,
    },
    SettingSpec {
        key: "helpers.completion",
        label: "Completion presentation",
        description: "What the completion gate says when a task is accepted: nothing, or a short recap.",
        kind: Kind::Choice,
        choices: COMPLETION,
        basic: true,
        restart: true,
    },
    // -- presentation, curated and Pane-owned -----------------------------
    SettingSpec {
        key: "ui.theme",
        label: "Theme",
        description: "Accent theme; the terminal background and its transparency are inherited.",
        kind: Kind::Choice,
        choices: THEMES,
        basic: true,
        restart: false,
    },
    SettingSpec {
        key: "ui.statusline",
        label: "Status line",
        description: "Status line layout. `/statusline` accepts `hide` as an alias for `hidden`.",
        kind: Kind::Choice,
        choices: STATUS_LINES,
        basic: true,
        restart: false,
    },
    SettingSpec {
        key: "ui.sidebar",
        label: "Sidebar",
        description: "Telemetry sidebar: shown when the terminal is wide enough, always, or never.",
        kind: Kind::Choice,
        choices: SIDEBAR,
        basic: true,
        restart: false,
    },
    SettingSpec {
        key: "ui.reduced_motion",
        label: "Reduced motion",
        description: "Stills the animated pulse and other motion.",
        kind: Kind::Bool,
        choices: &[],
        basic: true,
        restart: false,
    },
    // -- advanced runtime keys --------------------------------------------
    SettingSpec {
        key: "helpers.effort.find",
        label: "Find effort",
        description: "Hard reasoning level for the lookup helper; it never inherits `default`.",
        kind: Kind::Choice,
        choices: HARD_EFFORT,
        basic: false,
        restart: true,
    },
    SettingSpec {
        key: "helpers.effort.reduce",
        label: "Reduce effort",
        description: "Hard reasoning level for the filtering helper.",
        kind: Kind::Choice,
        choices: HARD_EFFORT,
        basic: false,
        restart: true,
    },
    SettingSpec {
        key: "helpers.effort.check",
        label: "Check effort",
        description: "Hard reasoning level for the completion check, the most consequential helper decision.",
        kind: Kind::Choice,
        choices: HARD_EFFORT,
        basic: false,
        restart: true,
    },
    SettingSpec {
        key: "helpers.preflight",
        label: "Helper preflight",
        description: "Run the Scout before the first turn, at the cost of a repository scan per request.",
        kind: Kind::Bool,
        choices: &[],
        basic: false,
        restart: true,
    },
    SettingSpec {
        key: "helpers.calls_per_cell",
        label: "Helper calls per cell",
        description: "The most helper calls one cell may make.",
        kind: Kind::Integer,
        choices: &[],
        basic: false,
        restart: true,
    },
    SettingSpec {
        key: "helpers.preflight_scope",
        label: "Preflight scope",
        description: "With preflight on: `auto` runs the Scout only for a request with an uncertainty signal; `always` runs it for every task.",
        kind: Kind::Choice,
        choices: PREFLIGHT_SCOPE,
        basic: false,
        restart: true,
    },
    SettingSpec {
        key: "helpers.completion_check",
        label: "Fresh completion check",
        description: "Run the independent checker on the terminal candidate with the original request, the diff and exact evidence only.",
        kind: Kind::Bool,
        choices: &[],
        basic: false,
        restart: true,
    },
    SettingSpec {
        key: "helpers.acceptance_list",
        label: "Request-derived acceptance list",
        description: "Derive a checklist of verifiable items from the request before the first turn and hold a completion that leaves one unmet.",
        kind: Kind::Bool,
        choices: &[],
        basic: false,
        restart: true,
    },
    SettingSpec {
        key: "helpers.reduce_above_tokens",
        label: "Reduce above tokens",
        description: "Estimated command-output tokens above which the pushed reducer is worth a cheap request.",
        kind: Kind::Integer,
        choices: &[],
        basic: false,
        restart: true,
    },
    SettingSpec {
        key: "supervisor.enabled",
        label: "Supervisor",
        description: "Whether the supervisor look runs.",
        kind: Kind::Bool,
        choices: &[],
        basic: false,
        restart: true,
    },
    SettingSpec {
        key: "supervisor.every",
        label: "Supervisor cadence",
        description: "How many turns between supervisor looks.",
        kind: Kind::Integer,
        choices: &[],
        basic: false,
        restart: true,
    },
    SettingSpec {
        key: "supervisor.model",
        label: "Supervisor model",
        description: "The model the supervisor runs on. Unset means the supervisor is off.",
        kind: Kind::Model,
        choices: &[],
        basic: false,
        restart: true,
    },
    SettingSpec {
        key: "decisions.mode",
        label: "Decisions",
        description: "`off` asks nothing. `shadow` asks and records what would hold. `on` holds a read-only request's effectful cell once.",
        kind: Kind::Choice,
        choices: DECISION_MODES,
        basic: false,
        restart: true,
    },
    SettingSpec {
        key: "decisions.model",
        label: "Decision model",
        description: "The model asked the intent question. Unset means decisions are off.",
        kind: Kind::Model,
        choices: &[],
        basic: false,
        restart: true,
    },
    SettingSpec {
        key: "decisions.hold_above",
        label: "Hold confidence",
        description: "Confidence at or above which a read-only intent holds an effectful cell or frame.",
        kind: Kind::Float,
        choices: &[],
        basic: false,
        restart: true,
    },
    SettingSpec {
        key: "decisions.scout_above",
        label: "Scout confidence",
        description: "Confidence at or above which a `needs_exploration` complexity answer adds a reason to run the preflight scout.",
        kind: Kind::Float,
        choices: &[],
        basic: false,
        restart: true,
    },
    SettingSpec {
        key: "decisions.completion_no_below",
        label: "Completion no threshold",
        description: "The completion question's noul at or below which a claimed completion gets a not-satisfied finding.",
        kind: Kind::Float,
        choices: &[],
        basic: false,
        restart: true,
    },
    SettingSpec {
        key: "decisions.completion_yes_above",
        label: "Completion yes threshold",
        description: "The completion question's noul at or above which the fresh checker is spared, when nothing else was found.",
        kind: Kind::Float,
        choices: &[],
        basic: false,
        restart: true,
    },
    SettingSpec {
        key: "decisions.hygiene_no_below",
        label: "Hygiene no threshold",
        description: "A diff-hygiene `has_tests` noul at or below which the diff is read as missing tests for the behaviour it changes.",
        kind: Kind::Float,
        choices: &[],
        basic: false,
        restart: true,
    },
    SettingSpec {
        key: "decisions.hygiene_yes_above",
        label: "Hygiene yes threshold",
        description: "A diff-hygiene noul at or above which an out-of-scope, debug-leftover, deleted-test or changed-signature question is decisive.",
        kind: Kind::Float,
        choices: &[],
        basic: false,
        restart: true,
    },
    SettingSpec {
        key: "decisions.judge_yes_above",
        label: "Judge yes threshold",
        description: "A `judge` acceptance item's noul at or above which the item counts as satisfied without the fresh checker.",
        kind: Kind::Float,
        choices: &[],
        basic: false,
        restart: true,
    },
    SettingSpec {
        key: "decisions.judge_no_below",
        label: "Judge no threshold",
        description: "A `judge` acceptance item's noul at or below which the item becomes a finding held once.",
        kind: Kind::Float,
        choices: &[],
        basic: false,
        restart: true,
    },
    SettingSpec {
        key: "decisions.drift_no_below",
        label: "Drift no threshold",
        description: "The drift question's noul at or below which an effectful cell is held once, as not doing what the plan's current step says.",
        kind: Kind::Float,
        choices: &[],
        basic: false,
        restart: true,
    },
    SettingSpec {
        key: "decisions.mode_above",
        label: "Mode proposal confidence",
        description: "Confidence at or above which a read-only intent proposes `explore` for one request, in `execute`, unpinned.",
        kind: Kind::Float,
        choices: &[],
        basic: false,
        restart: true,
    },
    SettingSpec {
        key: "modes.explore.writable",
        label: "Explore writable globs",
        description: "Project-relative globs, beside the scratchpad, that a write or edit may reach in `explore`.",
        kind: Kind::List,
        choices: &[],
        basic: false,
        restart: true,
    },
    SettingSpec {
        key: "modes.explore.commands",
        label: "Explore read-only commands",
        description: "Bash-style segment patterns, beside the fixed read-only list, that run in `explore`.",
        kind: Kind::List,
        choices: &[],
        basic: false,
        restart: true,
    },
    SettingSpec {
        key: "limits.cell_wall_clock_s",
        label: "Cell time limit",
        description: "Seconds one cell may run.",
        kind: Kind::Integer,
        choices: &[],
        basic: false,
        restart: true,
    },
    SettingSpec {
        key: "limits.response_bytes",
        label: "Response limit",
        description: "Bytes of tool response a cell may return.",
        kind: Kind::Integer,
        choices: &[],
        basic: false,
        restart: true,
    },
    SettingSpec {
        key: "limits.cells",
        label: "Cell budget",
        description: "Cells one task may run.",
        kind: Kind::Integer,
        choices: &[],
        basic: false,
        restart: true,
    },
    SettingSpec {
        key: "limits.task_tokens",
        label: "Task token cap (retired)",
        description: "Accepted so an existing project still starts; spend is accounted and never capped here.",
        kind: Kind::Integer,
        choices: &[],
        basic: false,
        restart: true,
    },
    SettingSpec {
        key: "limits.evidence_gate",
        label: "Evidence gate",
        description: "Hold a terminal return once for the deterministic final-state check and the no-progress guard's findings. Off is an ablation switch.",
        kind: Kind::Bool,
        choices: &[],
        basic: false,
        restart: true,
    },
    SettingSpec {
        key: "web.enabled",
        label: "Web broker",
        description: "The host-owned web broker. This grants no network access to shells or tools.",
        kind: Kind::Bool,
        choices: &[],
        basic: false,
        restart: true,
    },
    SettingSpec {
        key: "web.allow_domains",
        label: "Allowed domains",
        description: "Empty permits all public domains. `*.example.org` matches subdomains only.",
        kind: Kind::List,
        choices: &[],
        basic: false,
        restart: true,
    },
    SettingSpec {
        key: "web.deny_domains",
        label: "Denied domains",
        description: "Deny wins over allow. Bare names match exactly.",
        kind: Kind::List,
        choices: &[],
        basic: false,
        restart: true,
    },
    SettingSpec {
        key: "web.allow_http",
        label: "Allow plain HTTP",
        description: "Permit `http://` as well as `https://`.",
        kind: Kind::Bool,
        choices: &[],
        basic: false,
        restart: true,
    },
    SettingSpec {
        key: "web.search_endpoint",
        label: "Search endpoint",
        description: "A SearXNG-compatible JSON endpoint. No credentials belong in this value.",
        kind: Kind::Text,
        choices: &[],
        basic: false,
        restart: true,
    },
    SettingSpec {
        key: "web.max_response_bytes",
        label: "Web response limit",
        description: "Bytes one fetched document may contribute.",
        kind: Kind::Integer,
        choices: &[],
        basic: false,
        restart: true,
    },
    SettingSpec {
        key: "web.timeout_seconds",
        label: "Web timeout",
        description: "Seconds one web request may take.",
        kind: Kind::Integer,
        choices: &[],
        basic: false,
        restart: true,
    },
    // -- native permissions, and the legacy migration marker ---------------
    SettingSpec {
        key: "permissions.allow",
        label: "Allowed patterns",
        description: "Native permission patterns, compiled by the existing profile compiler. Never widens a running sandbox.",
        kind: Kind::List,
        choices: &[],
        basic: false,
        restart: true,
    },
    SettingSpec {
        key: "permissions.deny",
        label: "Denied patterns",
        description: "Denials beat every allow, and a global denial survives every project overlay.",
        kind: Kind::List,
        choices: &[],
        basic: false,
        restart: true,
    },
    SettingSpec {
        key: "legacy.imported",
        label: "Legacy file imported",
        description: "Set by `import legacy`: `.glasshouse/pane.toml` is preserved but no longer read.",
        kind: Kind::Bool,
        choices: &[],
        basic: false,
        restart: true,
    },
];

/// Every supported key, in panel order: everyday model and presentation rows
/// first, advanced rows after them.
pub fn specs() -> &'static [SettingSpec] {
    SPECS
}

/// One key's spec, or `None` for a key this build does not support.
pub fn spec(key: &str) -> Option<&'static SettingSpec> {
    SPECS.iter().find(|spec| spec.key == key)
}

/// Turns the word a person typed into the value that will be written.
///
/// Strings are written as typed -- no TOML quoting -- and every other kind is
/// parsed: `true`/`false`, an integer, or a list written either `a, b` or
/// `["a", "b"]`. Nothing here executes or expands a value.
pub fn validate(key: &str, value: &str) -> Result<toml::Value, String> {
    let spec = spec(key).ok_or_else(|| unknown_key(key))?;
    let typed =
        match spec.kind {
            Kind::Bool => match value.trim() {
                "true" => toml::Value::Boolean(true),
                "false" => toml::Value::Boolean(false),
                other => {
                    return Err(format!(
                        "settings: `{key}` must be `true` or `false`, not `{other}`"
                    ));
                }
            },
            Kind::Integer => {
                let cleaned = value.trim().replace('_', "");
                toml::Value::Integer(cleaned.parse::<i64>().map_err(|_| {
                    format!("settings: `{key}` must be a whole number, not `{value}`")
                })?)
            }
            Kind::Float => toml::Value::Float(
                value
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| format!("settings: `{key}` must be a number, not `{value}`"))?,
            ),
            Kind::List => parse_list(key, value)?,
            Kind::Choice => {
                let word = normalise_choice(key, value.trim());
                toml::Value::String(word)
            }
            Kind::Model | Kind::Text => toml::Value::String(value.trim().to_string()),
        };
    check_value(key, &typed)?;
    Ok(typed)
}

/// The same check, for a value that is already typed -- a file being loaded,
/// or an entry being imported. [`validate`] is this plus the parse.
pub fn check_value(key: &str, value: &toml::Value) -> Result<(), String> {
    let spec = spec(key).ok_or_else(|| unknown_key(key))?;
    match spec.kind {
        Kind::Bool => {
            if !value.is_bool() {
                return Err(format!("settings: `{key}` must be `true` or `false`"));
            }
        }
        Kind::Integer => {
            if !value.is_integer() {
                return Err(format!("settings: `{key}` must be a whole number"));
            }
        }
        Kind::Float => {
            if !(value.is_float() || value.is_integer()) {
                return Err(format!("settings: `{key}` must be a number"));
            }
        }
        Kind::Choice => {
            let word = value
                .as_str()
                .ok_or_else(|| format!("settings: `{key}` must be one of {}", choices(spec)))?;
            if !spec.choices.contains(&word) {
                return Err(format!(
                    "settings: `{key}` must be one of {}, not `{word}`",
                    choices(spec)
                ));
            }
        }
        Kind::Model | Kind::Text => {
            let text = value
                .as_str()
                .ok_or_else(|| format!("settings: `{key}` must be text"))?;
            if text.is_empty() {
                return Err(format!(
                    "settings: `{key}` cannot be empty; remove the override instead"
                ));
            }
            if text.chars().any(char::is_control) {
                return Err(format!("settings: `{key}` must be one line of text"));
            }
        }
        Kind::List => {
            let items = value
                .as_array()
                .ok_or_else(|| format!("settings: `{key}` must be a list"))?;
            for item in items {
                let entry = item
                    .as_str()
                    .ok_or_else(|| format!("settings: every entry of `{key}` must be text"))?;
                if entry.chars().any(char::is_control) {
                    return Err(format!("settings: `{key}` entries must be one line each"));
                }
                if key.starts_with("permissions.") {
                    permission_rule(entry)?;
                }
            }
        }
    }
    // Ranges, model ids, domain patterns and endpoint URLs are the runtime
    // parser's, never restated here. `Choice` keys are excluded because a
    // single-key document is not always a valid configuration on its own:
    // `agents.mode = "pinned"` needs the model that a second edit supplies.
    if is_runtime(key) && spec.kind != Kind::Choice {
        probe(key, value)?;
    }
    Ok(())
}

/// Whether a permission pattern is one the profile compiler understands.
///
/// This is the lexical half -- the kind and its argument. The half that needs
/// a project root (a project-relative pattern escaping the root) belongs to
/// [`crate::sandbox::profile::Profile::compile`], and the store runs it there.
pub fn permission_rule(pattern: &str) -> Result<(), String> {
    let trimmed = pattern.trim();
    if trimmed.is_empty() {
        return Err("settings: an empty permission pattern grants nothing".to_string());
    }
    let (name, argument) = match trimmed.split_once('(') {
        Some((name, rest)) => match rest.strip_suffix(')') {
            Some(argument) => (name.trim(), Some(argument)),
            None => {
                return Err(format!(
                    "settings: `{trimmed}` is missing its closing parenthesis"
                ));
            }
        },
        None => (trimmed, None),
    };
    match name {
        "Read" | "Write" | "Edit" => match argument.map(str::trim) {
            Some(argument) if !argument.is_empty() => Ok(()),
            _ => Err(format!(
                "settings: `{trimmed}` names no path; a bare `{name}` grants nothing"
            )),
        },
        "Bash" => Ok(()),
        "WebFetch" | "WebSearch" => Err(format!(
            "settings: `{trimmed}` grants nothing: network reach is never a permission pattern \
             (sandbox-grants.md §4.1); configure `web.*` instead"
        )),
        other if other.starts_with("mcp__") => {
            if argument.is_some() {
                return Err(format!(
                    "settings: `{trimmed}` takes no argument; name the MCP tool alone"
                ));
            }
            Ok(())
        }
        other => Err(format!(
            "settings: `{other}` is not a permission pattern kind Pane understands; \
             use Read, Write, Edit, Bash or mcp__server__tool"
        )),
    }
}

/// The valid words for a choice key, as an error sentence spells them.
pub fn choices(spec: &SettingSpec) -> String {
    spec.choices
        .iter()
        .map(|word| format!("`{word}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn normalise_choice(key: &str, word: &str) -> String {
    match (key, word) {
        // The runtime's own spelling of `build`, and `/statusline`'s alias.
        ("session.mode", "execute") => "build".to_string(),
        ("ui.statusline", "hide") => "hidden".to_string(),
        // `config.rs` has accepted `auto` for `default` effort since before
        // the word changed; the registry writes the current spelling.
        ("session.effort", "auto") => "default".to_string(),
        _ => word.to_string(),
    }
}

fn parse_list(key: &str, value: &str) -> Result<toml::Value, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(toml::Value::Array(Vec::new()));
    }
    if trimmed.starts_with('[') {
        let parsed: toml::Value = toml::from_str(&format!("value = {trimmed}"))
            .map_err(|error| format!("settings: `{key}` is not a valid list: {error}"))?;
        let items = parsed
            .get("value")
            .and_then(toml::Value::as_array)
            .ok_or_else(|| format!("settings: `{key}` must be a list"))?;
        return Ok(toml::Value::Array(items.clone()));
    }
    Ok(toml::Value::Array(
        trimmed
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(|entry| toml::Value::String(entry.to_string()))
            .collect(),
    ))
}

/// Builds the smallest document that sets `key` and parses it exactly as a
/// session start would, so a range lives in one place.
fn probe(key: &str, value: &toml::Value) -> Result<(), String> {
    let mut document = toml::value::Table::new();
    nest(&mut document, key, value.clone());
    let text = toml::to_string(&toml::Value::Table(document))
        .map_err(|error| format!("settings: `{key}`: {error}"))?;
    PaneConfig::parse_profile(&text, None)
        .map(|_| ())
        .map_err(|error| {
            format!(
                "settings: {}",
                error.strip_prefix("pane.toml: ").unwrap_or(&error)
            )
        })
}

fn nest(table: &mut toml::value::Table, key: &str, value: toml::Value) {
    let mut parts = key.split('.').peekable();
    let mut cursor = table;
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            cursor.insert(part.to_string(), value);
            return;
        }
        let entry = cursor
            .entry(part.to_string())
            .or_insert_with(|| toml::Value::Table(toml::value::Table::new()));
        cursor = match entry {
            toml::Value::Table(table) => table,
            _ => return,
        };
    }
}

fn unknown_key(key: &str) -> String {
    let leaf = key.rsplit('.').next().unwrap_or(key);
    let near: Vec<&str> = SPECS
        .iter()
        .map(|spec| spec.key)
        .filter(|candidate| {
            candidate.rsplit('.').next() == Some(leaf) || candidate.starts_with(key)
        })
        .take(3)
        .collect();
    if near.is_empty() {
        format!("settings: `{key}` is not a setting Pane supports")
    } else {
        format!(
            "settings: `{key}` is not a setting Pane supports; did you mean {}?",
            near.iter()
                .map(|key| format!("`{key}`"))
                .collect::<Vec<_>>()
                .join(" or ")
        )
    }
}
