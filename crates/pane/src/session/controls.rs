//! Human-invoked session inspection and configuration. No model dispatch.
use super::*;
use crate::config::PaneConfig;
use crate::spend::Tier;
use crate::tui::{Mode, Panel, PanelRow, TierModels};

pub(super) fn show(session: &Session<'_>, panel: Panel) {
    if let Some(ui) = session.ui {
        ui.panel(panel);
    } else {
        session_println!(
            "{}\n{}",
            panel.title,
            panel
                .rows
                .iter()
                .map(|r| r.text.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}

#[derive(serde::Deserialize)]
struct Catalogue {
    version: u32,
    accounts: Vec<Account>,
}
#[derive(serde::Deserialize)]
struct Account {
    account: String,
    provider: Option<String>,
    models: Vec<String>,
    scope: String,
    selectable: Option<bool>,
    unavailable_reason: Option<String>,
    /// Whether this account holds a credential. `None` for one that is not
    /// connectable at all, such as a provider reached with an API key.
    #[serde(default)]
    authenticated: Option<bool>,
    /// The provider whose flow would connect it.
    #[serde(default)]
    connect_with: Option<String>,
}

pub(super) fn models(session: &Session<'_>) {
    let catalogue = session
        .gateway
        .run(&["entitlements", "--json", "--refresh"], None)
        .and_then(|bytes| serde_json::from_slice::<Catalogue>(&bytes).ok());
    show(session, model_panel(catalogue, tier_models(session)));
}

/// What each tier of this session runs on right now.
///
/// Helpers report `None` when they are off *for any reason* -- no model, or
/// `enabled = false` -- because from the panel's side those are one state:
/// no helper will run.
fn tier_models(session: &Session<'_>) -> TierModels {
    let config = session.config();
    TierModels {
        parent: session.model.borrow().clone(),
        helper: config
            .helpers
            .enabled
            .then(|| config.helpers.model.clone())
            .flatten(),
        subagent: config.agents.model.clone(),
    }
}

/// Assigns a model to one tier, and persists the two that outlive the session.
///
/// All three are written to `.glasshouse/pane.toml`. The parent is there for
/// the plainest reason -- a session that forgets which model you chose makes
/// you choose it again every time -- and the other two because `agent.rs`
/// loads that file itself when a delegated goal starts, so a choice held only
/// in memory would be one a subagent could not see.
///
/// SAFETY OF THE EDIT: the text is proved to load with [`PaneConfig::parse`]
/// **before** it replaces the file, so a rejected model name -- a path, a
/// glob, a registered tool's name -- fails with the config's own sentence and
/// leaves the file as it was. There is one validator, not two.
pub(super) fn assign_model(
    session: &Session<'_>,
    tier: Tier,
    value: &str,
) -> Result<String, String> {
    let (section, key, key_removed) = match tier {
        Tier::Parent => ("model", "parent", false),
        Tier::Helpers => ("helpers", "model", value == "off"),
        Tier::Subagents => ("agents", "model", value == "inherit"),
    };
    let path = session.project.root.join(".glasshouse").join("pane.toml");
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.to_string()),
    };
    let mut document: toml::Value = if text.trim().is_empty() {
        toml::Value::Table(toml::Table::new())
    } else {
        toml::from_str(&text).map_err(|e| format!("pane.toml: {e}"))?
    };
    let table = document
        .as_table_mut()
        .ok_or_else(|| "pane.toml: must be a table".to_string())?
        .entry(section)
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .ok_or_else(|| format!("pane.toml: `[{section}]` must be a table"))?;
    if key_removed {
        table.remove(key);
    } else {
        table.insert(key.into(), toml::Value::String(value.into()));
        // Choosing a helper model in a panel IS the opt-in the fail-closed
        // default asks for, so an earlier `enabled = false` must not silently
        // swallow the choice a person just made.
        if tier == Tier::Helpers {
            table.insert("enabled".into(), toml::Value::Boolean(true));
        }
    }
    let encoded = toml::to_string_pretty(&document).map_err(|e| e.to_string())?;
    let parsed = PaneConfig::parse(&encoded)?;
    fs::create_dir_all(path.parent().expect("pane.toml has a parent"))
        .map_err(|e| e.to_string())?;
    fs::write(&path, &encoded).map_err(|e| e.to_string())?;
    *session.config.borrow_mut() = parsed;
    Ok(match (tier, key_removed) {
        (Tier::Helpers, true) => "helpers off; no helper will run".to_string(),
        (Tier::Subagents, true) => "subagents inherit the parent's model".to_string(),
        (tier, _) => format!("{} model set to {value}", tier.singular()),
    })
}

/// Connects a subscription account without leaving the session.
///
/// The gateway owns the credential from end to end. What crosses this boundary
/// is the authorization URL, a countdown and an outcome — never a token —
/// which is exactly what lets the flow be rendered here instead of handing the
/// terminal to a child process.
///
/// With no account named it lists the ones that could be connected, so
/// `/login` is discoverable on its own and not only from the model picker.
pub(super) fn login(session: &Session<'_>, account: Option<&str>) {
    let catalogue = session
        .gateway
        .run(&["entitlements", "--json"], None)
        .and_then(|bytes| serde_json::from_slice::<Catalogue>(&bytes).ok());

    let Some(catalogue) = catalogue else {
        show(
            session,
            Panel::text(
                "Connect an account",
                "The inference gateway is not reachable.",
            ),
        );
        return;
    };

    let Some(account) = account else {
        show(session, connectable_panel(&catalogue, &api_keys(session)));
        return;
    };

    let Some(entry) = catalogue
        .accounts
        .iter()
        .find(|entry| entry.account == account)
    else {
        show(
            session,
            Panel::text(
                "Connect an account",
                format!("`{account}` is not a configured account."),
            ),
        );
        return;
    };
    let Some(provider) = entry.connect_with.clone() else {
        show(
            session,
            Panel::text(
                "Connect an account",
                format!("`{account}` is not connected with a login flow."),
            ),
        );
        return;
    };

    stream_connect(session, &provider, account);
}

/// The gateway's credential table, for a session whose gateway is a binary
/// this machine runs. A hosted session's keys live where its gateway does,
/// which is not here, so it is never asked.
fn api_keys(session: &Session<'_>) -> Vec<crate::gateway::CredentialRow> {
    if !matches!(session.gateway, Gateway::Command { .. }) {
        return Vec::new();
    }
    crate::gateway::credentials(session.gateway).unwrap_or_default()
}

/// Says a startup once, and only for the one state a person must act on:
/// pane started the gateway, the gateway names providers, and not one of
/// them has a credential -- so every turn would come back 503 until a key is
/// entered.
pub(super) fn announce_missing_credential(session: &Session<'_>, started_the_gateway: bool) {
    const NOTICE: &str = "No provider credential is stored yet. Use /login to enter an API key \
                          or connect an account.";
    if !started_the_gateway || !crate::gateway::nothing_resolves(session.gateway) {
        return;
    }
    // A connected subscription is a credential the gateway holds too --
    // through its own login flow rather than a key, so `credentials list`
    // does not see it, and a session serving one must not be told nothing
    // is stored. Measured 2026-09-11 on three serving subscriptions.
    let connected = session
        .gateway
        .run(&["entitlements", "--json"], None)
        .and_then(|bytes| serde_json::from_slice::<Catalogue>(&bytes).ok())
        .is_some_and(|catalogue| {
            catalogue
                .accounts
                .iter()
                .any(|entry| entry.authenticated == Some(true))
        });
    if connected {
        return;
    }
    if session.ui.is_some() {
        session_println!("{NOTICE}");
    } else {
        // Not stdout: a session driven by a script has a caller reading its
        // answers there, and a notice is not an answer.
        eprintln!("{NOTICE}");
    }
}

/// One row per provider that declares a key, after the accounts: where the
/// key resolves from now, and `/key <provider>` to enter one.
fn key_rows(keys: &[crate::gateway::CredentialRow]) -> Vec<tui::PanelRow> {
    keys.iter()
        .map(|row| {
            let state = match (row.source.as_deref(), row.native_store.as_deref()) {
                (Some("file"), _) => "stored in the gateway's credential file".to_string(),
                (Some("native"), _) => "stored in the native store".to_string(),
                (Some("environment"), _) => "stored in the environment".to_string(),
                (Some(source), _) => format!("stored in {source}"),
                (None, Some("refused")) => {
                    "a Keychain item exists that this build may not read; enter it again"
                        .to_string()
                }
                (None, _) => "not set".to_string(),
            };
            tui::PanelRow {
                text: format!("{} · API key · {state}", row.provider),
                command: Some(format!("/key {}", row.provider)),
            }
        })
        .collect()
}

/// The accounts a login flow could connect, connected or not, and then the
/// API keys the gateway holds.
fn connectable_panel(catalogue: &Catalogue, keys: &[crate::gateway::CredentialRow]) -> Panel {
    let mut rows: Vec<tui::PanelRow> = catalogue
        .accounts
        .iter()
        .filter(|entry| entry.connect_with.is_some())
        .map(|entry| {
            let state = if entry.authenticated == Some(true) {
                "connected"
            } else {
                "not connected"
            };
            tui::PanelRow {
                text: format!("{} · {} · {state}", entry.account, entry.scope),
                command: Some(format!("/login {}", entry.account)),
            }
        })
        .collect();
    let keys = key_rows(keys);
    // The filler is for a panel with nothing in it at all: a key row is
    // something to do, so saying there is nothing would be false.
    if rows.is_empty() && keys.is_empty() {
        rows.push(tui::PanelRow {
            text: "No account in this project is connected with a login flow.".into(),
            command: None,
        });
    }
    rows.extend(keys);
    Panel::rows("Connect an account", rows)
}

/// `/key <provider>`: takes an API key without echoing it and hands it to the
/// gateway to store.
///
/// **The value is read, passed to one child's stdin, and dropped.** It is
/// never put in the editor, the conversation, the rollout, a panel or a log
/// -- the panel this ends with names the *variable*, never the key.
pub(super) fn key(session: &Session<'_>, provider: Option<&str>) {
    let Some(provider) = provider.filter(|value| !value.is_empty()) else {
        show(
            session,
            Panel::text(
                "API key",
                "/key <provider> takes an API key for one provider -- `/key anthropic`. \
                 /login lists the providers this gateway knows.",
            ),
        );
        return;
    };
    if matches!(session.gateway, Gateway::Hosted { .. }) {
        show(
            session,
            Panel::text(
                "API key",
                "API keys are entered where the gateway runs; this session's gateway is \
                 hosted by Glasshouse.",
            ),
        );
        return;
    }
    let Some(value) = entered_secret(session, provider).filter(|value| !value.is_empty()) else {
        session_println!("no key entered");
        return;
    };
    match crate::gateway::store_credential(session.gateway, provider, &value) {
        Some(variable) => show(
            session,
            Panel::text(
                "API key",
                format!("Stored the {variable} for {provider} in the gateway."),
            ),
        ),
        None => show(
            session,
            Panel::text(
                "API key",
                format!(
                    "The gateway did not store the key; run `inference-gateway credentials \
                     set {provider}` in a shell to see why."
                ),
            ),
        ),
    }
}

/// The key itself: a modal masked prompt with a terminal, the next line of
/// stdin without one. Neither path echoes what it reads.
fn entered_secret(session: &Session<'_>, provider: &str) -> Option<String> {
    match session.ui {
        Some(ui) => ui.secret(&format!(
            "API key for {provider} — Enter stores it, Esc cancels"
        )),
        None => ui::read_line().ok().flatten(),
    }
}

/// Runs the flow, showing each line the gateway reports as it arrives.
///
/// Streamed rather than awaited because the first line is the URL a person
/// must open and the last arrives minutes later: a caller that waited for the
/// exit would have nothing to show in between.
fn stream_connect(session: &Session<'_>, provider: &str, account: &str) {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;

    let Some(mut command) = session.gateway.control_command(&[
        "subscriptions",
        "connect",
        provider,
        "--entitlement",
        account,
        "--json",
    ]) else {
        show(
            session,
            Panel::text(
                "Connect an account",
                "The inference gateway is not reachable.",
            ),
        );
        return;
    };
    let spawned = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = spawned else {
        show(
            session,
            Panel::text(
                "Connect an account",
                "The inference gateway could not be started.",
            ),
        );
        return;
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        return;
    };

    let title = format!("Connecting {account}");
    let mut rows: Vec<tui::PanelRow> = Vec::new();
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        let Some(progress) = describe_progress(&line) else {
            continue;
        };
        match progress {
            // The countdown replaces itself rather than filling the panel.
            Describe::Replace(text) => {
                if matches!(rows.last(), Some(row) if row.command.is_none() && row.text.starts_with("waiting"))
                {
                    rows.pop();
                }
                rows.push(tui::PanelRow {
                    text,
                    command: None,
                });
            }
            Describe::Keep(text) => rows.push(tui::PanelRow {
                text,
                command: None,
            }),
        }
        show(session, Panel::rows(title.clone(), rows.clone()));
    }
    let _ = child.wait();
}

enum Describe {
    Keep(String),
    Replace(String),
}

/// One progress line, as a person reads it.
///
/// Unknown shapes are dropped rather than printed raw: this is another
/// program's output and the panel is not a place to echo bytes nobody
/// recognised.
fn describe_progress(line: &str) -> Option<Describe> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    match value.get("state")?.as_str()? {
        "opened" => Some(Describe::Keep(format!(
            "open this to continue:\n{}",
            value.get("authorize_url")?.as_str()?
        ))),
        "waiting" => Some(Describe::Replace(format!(
            "waiting for the browser ({}s left)",
            value.get("seconds_remaining").and_then(|v| v.as_u64())?
        ))),
        "connected" => Some(Describe::Keep(format!(
            "connected{}",
            value
                .get("account")
                .and_then(|v| v.as_str())
                .map(|a| format!(" as {a}"))
                .unwrap_or_default()
        ))),
        "failed" => Some(Describe::Keep(format!(
            "failed: {}",
            value.get("reason").and_then(|v| v.as_str()).unwrap_or("")
        ))),
        _ => None,
    }
}

fn model_panel(catalogue: Option<Catalogue>, tiers: TierModels) -> Panel {
    let title = "Models".to_string();
    match catalogue {
        Some(catalogue) if catalogue.version == 1 => Panel::models(
            title,
            catalogue
                .accounts
                .into_iter()
                .map(|account| {
                    // An account that could be connected and is not is the row
                    // a person most wants to act on, so it says so and offers
                    // the flow rather than sitting empty.
                    let connect = match (account.authenticated, &account.connect_with) {
                        (Some(false), Some(provider)) => Some(provider.clone()),
                        _ => None,
                    };
                    let unavailable_reason = match (&connect, account.unavailable_reason) {
                        (Some(_), _) => Some("not connected — press enter to connect".into()),
                        (None, existing) => existing,
                    };
                    tui::ModelGroup {
                        provider: account.provider.unwrap_or_else(|| "native harness".into()),
                        account: account.account,
                        scope: account.scope,
                        models: account.models,
                        selectable: account.selectable,
                        unavailable_reason,
                        connect,
                    }
                })
                .collect(),
            tiers,
        ),
        _ => Panel::text(
            title,
            "Catalogue unavailable: no gateway answered. Use /model <id>.",
        ),
    }
}

pub(super) fn command(
    name: &str,
    argument: Option<&str>,
    session: &Session<'_>,
    transcript: &Transcript,
) -> bool {
    match name {
        "handlers" => {
            if argument.is_some() {
                session_println!("No active task; handlers are released when its task ends.");
            }
            show(session, tui::handlers_panel(&transcript.notebook.handlers));
        }
        "effort" => {
            if let Some(value) = argument {
                if let Some(effort) = wire::Effort::parse(value) {
                    if !session.model.borrow().contains("claude")
                        && matches!(effort, wire::Effort::Xhigh | wire::Effort::Max)
                    {
                        session_println!(
                            "This route supports auto|low|medium|high; xhigh/max need a compatible Claude model."
                        );
                        return true;
                    }
                    session.effort.set(effort);
                    if let Some(ui) = session.ui {
                        ui.effort(effort);
                    }
                    session_println!("Effort: {} · applied to the next request", effort.name());
                } else {
                    session_println!("Use /effort auto|low|medium|high|xhigh|max");
                }
            } else {
                let rows = ["auto", "low", "medium", "high", "xhigh", "max"]
                    .iter()
                    .map(|value| PanelRow {
                        text: value.to_string(),
                        command: Some(format!("/effort {value}")),
                    })
                    .collect();
                show(
                    session,
                    Panel {
                        title: format!("Effort · current {}", session.effort.get().name()),
                        rows,
                        selected: 0,
                        ..Panel::default()
                    },
                );
            }
        }
        "mode" => {
            let mode = match argument {
                Some("plan") => Mode::Plan,
                Some("execute") => Mode::Execute,
                None => session.mode.get().next(),
                _ => {
                    session_println!("Use /mode execute|plan");
                    return true;
                }
            };
            session.mode.set(mode);
            if let Some(ui) = session.ui {
                ui.mode(mode);
            }
            session_println!(
                "Mode: {}{}",
                mode.name(),
                if mode == Mode::Plan {
                    " · code and tools do not execute"
                } else {
                    " · session sandbox applies"
                }
            );
        }
        "handles" => {
            let table = transcript
                .notebook
                .cells
                .last()
                .and_then(|cell| cell.table.as_deref())
                .unwrap_or("No handles recorded yet.");
            show(session, Panel::text("Last handle preview", table));
        }
        "budget" => {
            let used = transcript
                .notebook
                .tokens
                .as_ref()
                .map(|tokens| tokens.used)
                .unwrap_or(0);
            show(
                session,
                Panel::text(
                    "Task spend",
                    format!(
                        "Last task: {used} cumulative tokens\nToken spend is telemetry and has no cap.\nCell limit: {}\nConfigure runtime limits in .glasshouse/pane.toml for the next session.",
                        session.config().limits.cells
                    ),
                ),
            );
        }
        "context" => {
            let c = &transcript.conversation;
            let estimated = estimate_request_tokens(c, &session.model.borrow());
            let bytes: usize = c.messages.iter().map(|m| message_text(m).len()).sum();
            let measured = transcript
                .notebook
                .context
                .map(|context| match context.cap {
                    Some(cap) => format!(
                        "Current request context: {}/{} tokens ({}%) · {}",
                        context.used,
                        cap,
                        context.used.min(cap).saturating_mul(100) / cap.max(1),
                        context.counted.as_str()
                    ),
                    None => format!(
                        "Current request context: {} tokens · window unknown · {}",
                        context.used,
                        context.counted.as_str()
                    ),
                })
                .unwrap_or_else(|| "Current request context: no request yet".into());
            show(
                session,
                Panel::text(
                    "Context",
                    format!(
                        "{} messages · {} cells\n{}\nSystem: {} bytes\nMessages: {} bytes\nNext request: ~{} tokens (estimate)\nTask spend: cumulative telemetry, no cap\nContext is retained in the rollout; no model call was made.",
                        c.messages.len(),
                        transcript.notebook.cells.len(),
                        measured,
                        c.system.len(),
                        bytes,
                        estimated
                    ),
                ),
            );
        }
        "status" | "config" => {
            show(
                session,
                Panel::text(
                    "Session configuration",
                    format!(
                        "Model: {}\nMode: {}\nProject: {}\nSandbox: {} path rules · {} command patterns · network {}\nTask spend: tracked, uncapped\nCell limit: {} cells · {} seconds each · response {} bytes\nSupervisor: {}\nLimits: .glasshouse/pane.toml (loaded at startup)\nPermissions: .claude/settings.json (loaded at startup)\nPresentation: /theme · /sidebar · /statusline · /fullscreen",
                        session.model.borrow(),
                        session.mode.get().name(),
                        session.project.root.display(),
                        session.profile.rule_count(),
                        session.profile.command_pattern_count(),
                        session.profile.grants_network(),
                        session.config().limits.cells,
                        session.config().limits.cell_wall_clock_s,
                        session.config().limits.response_bytes,
                        session
                            .config()
                            .supervisor
                            .model
                            .as_deref()
                            .unwrap_or("off")
                    ),
                ),
            );
        }
        "supervisor" => {
            let latest = match transcript.notebook.supervisor.as_ref() {
                Some(SupervisorStatus::Nudged(reason)) => format!("nudged: {reason}"),
                Some(SupervisorStatus::LookedNoNudge) => "looked; no nudge".into(),
                Some(SupervisorStatus::LookFailed(reason)) => format!("look failed: {reason}"),
                Some(SupervisorStatus::Off) | None => "no look in this session".into(),
            };
            show(
                session,
                Panel::text(
                    "Supervisor",
                    format!(
                        "State: {}\nModel: {}\nCadence: every {} cells\nLatest: {}\nConfigure [supervisor] in .glasshouse/pane.toml for the next session.",
                        if session.config().supervisor.enabled
                            && session.config().supervisor.model.is_some()
                        {
                            "active"
                        } else {
                            "off"
                        },
                        session
                            .config()
                            .supervisor
                            .model
                            .clone()
                            .unwrap_or_else(|| "not configured".to_string()),
                        session.config().supervisor.every,
                        latest
                    ),
                ),
            );
        }
        "rollback" => rollback(session, argument),
        "permissions" => match permissions(session, argument) {
            Ok(text) => show(session, Panel::text("Permissions", text)),
            Err(error) => session_println!("ERROR: {error}"),
        },
        "entitlements" => {
            models(session);
        }
        "login" => login(session, argument),
        "key" => key(session, argument),
        _ => return false,
    }
    true
}

fn rollback(session: &Session<'_>, argument: Option<&str>) {
    let count = session.rollbacks.borrow().len();
    let Some(last) = session
        .rollbacks
        .borrow()
        .last()
        .map(|checkpoint| checkpoint.before.rollback_plan(&checkpoint.after))
    else {
        session_println!("/rollback: no file-changing cell is available in this session");
        return;
    };
    let plan = match last {
        Ok(plan) => plan,
        Err(error) => {
            session_println!("/rollback unavailable: {error}");
            return;
        }
    };

    match argument.filter(|value| !value.is_empty()) {
        None => {
            session.rollback_pending.set(Some(count));
            let mut panel = Panel::text(
                "Rollback preview · confirmation required",
                format!(
                    "The latest file-changing cell affected:\n{}",
                    plan.preview()
                ),
            );
            panel.rows.push(PanelRow {
                text: "Confirm rollback".into(),
                command: Some("/rollback confirm".into()),
            });
            panel.rows.push(PanelRow {
                text: "Cancel".into(),
                command: Some("/rollback cancel".into()),
            });
            panel.selected = panel.rows.len().saturating_sub(2);
            show(session, panel);
        }
        Some("cancel") => {
            session.rollback_pending.set(None);
            session_println!("Rollback cancelled; no files were changed.");
        }
        Some("confirm") => {
            if let Err(message) =
                rollback_confirmation(session.ui.is_some(), session.rollback_pending.get(), count)
            {
                session_println!("{message}");
                return;
            }
            match plan.apply(session.profile) {
                Ok(()) => {
                    session.rollbacks.borrow_mut().pop();
                    session.rollback_pending.set(None);
                    session_println!("Rollback complete:\n{}", plan.preview());
                }
                Err(error) => {
                    session.rollback_pending.set(None);
                    session_println!("Rollback refused: {error}");
                }
            }
        }
        Some(_) => session_println!("Use /rollback, /rollback confirm, or /rollback cancel"),
    }
}

fn rollback_confirmation(
    interactive: bool,
    previewed: Option<usize>,
    current: usize,
) -> Result<(), &'static str> {
    if !interactive {
        return Err(
            "/rollback confirm is refused outside an interactive TUI; no files were changed",
        );
    }
    if previewed != Some(current) {
        return Err("/rollback confirm requires a current preview; run /rollback first");
    }
    Ok(())
}

fn permissions(session: &Session<'_>, argument: Option<&str>) -> Result<String, String> {
    let root = &session.project.root;
    let path = root.join(".claude/settings.json");
    let mut settings: serde_json::Value = match fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).map_err(|e| format!("settings.json: {e}"))?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => serde_json::json!({}),
        Err(e) => return Err(e.to_string()),
    };
    let mut changed = false;
    if let Some(argument) = argument.filter(|s| !s.is_empty()) {
        let (action, rule) = argument
            .split_once(' ')
            .ok_or("Use /permissions allow|remove <rule>")?;
        if !matches!(action, "allow" | "remove") || rule.trim().is_empty() {
            return Err("Use /permissions allow|remove <rule>".into());
        }
        let root_map = settings
            .as_object_mut()
            .ok_or("settings.json must be an object")?;
        let permissions = root_map
            .entry("permissions")
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
            .ok_or("permissions must be an object")?;
        let allow = permissions
            .entry("allow")
            .or_insert_with(|| serde_json::json!([]))
            .as_array_mut()
            .ok_or("permissions.allow must be an array")?;
        let rule = serde_json::Value::String(rule.trim().into());
        if action == "allow" {
            if !allow.contains(&rule) {
                allow.push(rule);
                changed = true;
            }
        } else {
            let old = allow.len();
            allow.retain(|v| v != &rule);
            changed = old != allow.len();
        }
        if changed {
            let encoded = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
            let profile = Profile::compile(root, Some(&encoded));
            if !profile.diagnostics().is_empty() {
                return Err(format!("Not saved: {}", profile.diagnostics().join("; ")));
            }
            fs::create_dir_all(path.parent().expect("settings parent"))
                .map_err(|e| e.to_string())?;
            fs::write(&path, encoded + "\n").map_err(|e| e.to_string())?;
        }
    }
    let permissions = settings
        .get("permissions")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({"allow":[]}));
    Ok(format!(
        "Effective current session (immutable):\n{} path rules · {} command patterns · {} MCP patterns · network {}\n{}\n\nPersisted next-session settings:\n{}\n{}\n\n/permissions allow <rule>\n/permissions remove <rule>\nPersisted edits never change the running sandbox.",
        session.profile.rule_count(),
        session.profile.command_pattern_count(),
        session.profile.mcp_tool_count(),
        if session.profile.grants_network() {
            "on"
        } else {
            "off"
        },
        if session.profile.diagnostics().is_empty() {
            "No permission diagnostics.".to_string()
        } else {
            format!("Diagnostics: {}", session.profile.diagnostics().join("; "))
        },
        if changed {
            "Saved .claude/settings.json"
        } else {
            ".claude/settings.json"
        },
        serde_json::to_string_pretty(&permissions).map_err(|e| e.to_string())?
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn model_catalogue_groups_by_provider_then_account_and_preserves_model_ids() {
        let catalogue = serde_json::from_value(serde_json::json!({
            "version": 1,
            "accounts": [
                {"account":"a-account", "provider":"z-provider", "scope":"declared", "models":["shared/id"]},
                {"account":"z-account", "provider":"a-provider", "scope":"declared", "models":["shared/id", "B/model", "shared/id", "bad id", ""]},
                {"account":"b-account", "provider":"a-provider", "scope":"declared", "models":["shared/id"]}
            ]
        })).unwrap();
        let mut panel = model_panel(Some(catalogue), TierModels::default());
        let headings: Vec<_> = panel
            .rows
            .iter()
            .filter(|row| row.command.is_none())
            .map(|row| row.text.as_str())
            .collect();
        assert_eq!(
            headings,
            [
                "a-provider · b-account · declared",
                "a-provider · z-account · declared"
            ]
        );
        let commands: Vec<_> = panel
            .rows
            .iter()
            .filter_map(|row| row.command.as_deref())
            .collect();
        assert_eq!(
            commands,
            ["/model shared/id", "/model B/model", "/model shared/id"]
        );
        assert_eq!(panel.selected, 1);
        panel.move_provider(true);
        assert_eq!(panel.rows[0].text, "z-provider · a-account · declared");
        assert_eq!(panel.rows[1].command.as_deref(), Some("/model shared/id"));
    }

    #[test]
    fn permission_edits_preserve_other_settings_and_reject_invalid_grants() {
        let root = std::env::temp_dir().join(format!("pane-permissions-{}", std::process::id()));
        fs::create_dir_all(root.join(".claude")).unwrap();
        let path = root.join(".claude/settings.json");
        fs::write(
            &path,
            r#"{"other":{"keep":true},"permissions":{"allow":[],"deny":["Bash(rm *)"]}}"#,
        )
        .unwrap();
        let project = ProjectConfig {
            root: root.clone(),
            ..ProjectConfig::default()
        };
        let config = PaneConfig::default();
        let profile = Profile::compile(&root, fs::read_to_string(&path).ok().as_deref());
        let glasshouse = Glasshouse::Command {
            glasshouse: root.join("absent"),
        };
        let gateway = crate::gateway::Gateway::Command {
            gateway: root.join("absent-gateway"),
        };
        let id = SessionId::new("permission-test");
        let memory = LocalMemory::new(&root);
        let interrupt = Interrupter::new(id.clone());
        let session = Session {
            inbox: RefCell::new(crate::events::inbox::Inbox::discover(&glasshouse, &root)),
            window: RefCell::new(crate::events::window::Window::new(Default::default())),
            messages: std::rc::Rc::new(RefCell::new(std::collections::HashMap::new())),
            ui: None,
            model: RefCell::new("test".into()),
            context_window: None,
            interface: Cell::new(crate::abi::Interface::default()),
            mode: Cell::new(tui::Mode::Execute),
            effort: Cell::new(wire::Effort::Auto),
            project: &project,
            config: &RefCell::new(config),
            interrupt: &interrupt,
            profile: &profile,
            glasshouse: &glasshouse,
            gateway: &gateway,
            id: &id,
            memory: &memory,
            rollbacks: RefCell::new(Vec::new()),
            rollback_pending: Cell::new(None),
        };
        permissions(&session, Some("allow Read(**)")).unwrap();
        let saved = fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&saved).unwrap();
        assert_eq!(parsed["other"]["keep"], true);
        assert_eq!(
            parsed["permissions"]["deny"],
            serde_json::json!(["Bash(rm *)"])
        );
        assert!(permissions(&session, Some("allow NotAGrant(foo)")).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), saved);
        permissions(&session, Some("remove Read(**)")).unwrap();
        let parsed: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(parsed["permissions"]["allow"], serde_json::json!([]));
        fs::remove_dir_all(root).unwrap();
    }

    /// Builds a session rooted at `root` and runs `body` against it.
    fn with_session(root: &std::path::Path, body: impl FnOnce(&Session<'_>)) {
        let project = ProjectConfig {
            root: root.to_path_buf(),
            ..ProjectConfig::default()
        };
        let config = RefCell::new(PaneConfig::load(root).expect("the fixture parses"));
        let profile = Profile::compile(root, None);
        let glasshouse = Glasshouse::Command {
            glasshouse: root.join("absent"),
        };
        let gateway = crate::gateway::Gateway::Command {
            gateway: root.join("absent-gateway"),
        };
        let id = SessionId::new("tier-test");
        let memory = LocalMemory::new(root);
        let interrupt = Interrupter::new(id.clone());
        let session = Session {
            inbox: RefCell::new(crate::events::inbox::Inbox::discover(&glasshouse, root)),
            window: RefCell::new(crate::events::window::Window::new(Default::default())),
            messages: std::rc::Rc::new(RefCell::new(std::collections::HashMap::new())),
            ui: None,
            model: RefCell::new("opus-5".into()),
            context_window: None,
            interface: Cell::new(crate::abi::Interface::default()),
            mode: Cell::new(tui::Mode::Execute),
            effort: Cell::new(wire::Effort::Auto),
            project: &project,
            config: &config,
            interrupt: &interrupt,
            profile: &profile,
            glasshouse: &glasshouse,
            gateway: &gateway,
            id: &id,
            memory: &memory,
            rollbacks: RefCell::new(Vec::new()),
            rollback_pending: Cell::new(None),
        };
        body(&session);
    }

    /// A tier assignment takes effect now and survives the session, and an
    /// unrelated setting in the same file is not collateral damage.
    #[test]
    fn assigning_a_tier_is_live_persisted_and_reversible() {
        let root = std::env::temp_dir().join(format!("pane-tier-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join(".glasshouse")).unwrap();
        let file = root.join(".glasshouse").join("pane.toml");
        fs::write(
            &file,
            "[limits]\ncells = 42\n\n[helpers]\nenabled = false\n",
        )
        .unwrap();

        with_session(&root, |session| {
            assert_eq!(tier_models(session).parent, "opus-5");
            assert_eq!(tier_models(session).helper, None, "helpers ship off");

            assign_model(session, Tier::Helpers, "gpt-5.6-luna").unwrap();
            // Live, with no restart -- the next cell's runtime is built from
            // this.
            assert_eq!(tier_models(session).helper.as_deref(), Some("gpt-5.6-luna"));
            // Persisted, because `agent.rs` loads this file itself when a
            // delegated goal starts.
            let saved = PaneConfig::load(&root).unwrap();
            assert_eq!(saved.helpers.model.as_deref(), Some("gpt-5.6-luna"));
            // Choosing a model IS the opt-in, so an earlier `enabled = false`
            // does not silently swallow it.
            assert!(saved.helpers.enabled);
            // And an unrelated setting survived the edit.
            assert_eq!(saved.limits.cells, 42);

            assign_model(session, Tier::Subagents, "claude-sonnet-5").unwrap();
            assert_eq!(
                PaneConfig::load(&root).unwrap().agents.model.as_deref(),
                Some("claude-sonnet-5")
            );

            // Reversible, which is what makes the panel safe to press.
            assign_model(session, Tier::Helpers, "off").unwrap();
            assert_eq!(tier_models(session).helper, None);
            assert_eq!(PaneConfig::load(&root).unwrap().helpers.model, None);
            assign_model(session, Tier::Subagents, "inherit").unwrap();
            assert_eq!(tier_models(session).subagent, None);

            // A value the config refuses fails with the config's own sentence
            // and leaves the file byte-identical: one validator, not two.
            let before = fs::read_to_string(&file).unwrap();
            assert!(assign_model(session, Tier::Helpers, "../etc/passwd").is_err());
            assert_eq!(fs::read_to_string(&file).unwrap(), before);

            // The parent is remembered too: the tier a person changes most
            // was the only one that used to forget.
            assign_model(session, Tier::Parent, "claude-opus-4-8").unwrap();
            assert_eq!(
                PaneConfig::load(&root).unwrap().model.parent.as_deref(),
                Some("claude-opus-4-8")
            );
        });
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn rollback_confirmation_is_interactive_and_bound_to_the_previewed_checkpoint() {
        assert!(rollback_confirmation(false, Some(1), 1).is_err());
        assert!(rollback_confirmation(true, None, 1).is_err());
        assert!(rollback_confirmation(true, Some(1), 2).is_err());
        assert_eq!(rollback_confirmation(true, Some(2), 2), Ok(()));
    }
}
