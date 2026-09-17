//! Human-invoked session inspection and configuration. No model dispatch.
use super::*;
use crate::config::AgentsMode;
#[cfg(test)]
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
    // Read once, when the panel opens, from whatever `glasshouse analysis
    // --refresh` last cached. No network request on a keystroke.
    show(
        session,
        model_panel(catalogue, tier_models(session))
            .with_intelligence(crate::glasshouse::intelligence(session.glasshouse)),
    );
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
        subagent: match config.agents.mode {
            AgentsMode::Auto => None,
            AgentsMode::Off => Some("off".to_string()),
            AgentsMode::Pinned => config.agents.model.clone(),
        },
    }
}

/// Assigns a model to one tier, and persists the two that outlive the session.
///
/// All three are written to `.pane/config.toml`, under the active named
/// profile when selected. The effective configuration stays live and travels
/// to delegated agents as a snapshot.
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
        Tier::Helpers => {
            if matches!(value, "auto" | "inherit") {
                return Err(format!(
                    "helper model must be `off` or a concrete model id, not `{value}`"
                ));
            }
            ("helpers", "model", value == "off")
        }
        Tier::Subagents => (
            "agents",
            "model",
            matches!(value, "auto" | "inherit" | "off"),
        ),
    };
    let store = crate::settings::Store::new(&session.project.root)?;
    let snapshot = store.read(crate::settings::Scope::Local)?;
    let mut edits = vec![(
        format!("{section}.{key}"),
        if key_removed {
            None
        } else {
            Some(value.into())
        },
    )];
    if tier == Tier::Helpers {
        edits.push(("helpers.enabled".into(), Some((!key_removed).to_string())));
    }
    if tier == Tier::Subagents {
        let mode = match value {
            "auto" | "inherit" => "auto",
            "off" => "off",
            _ => "pinned",
        };
        edits.push(("agents.mode".into(), Some(mode.into())));
    }
    let loaded = store.save_profile(
        crate::settings::Scope::Local,
        &snapshot,
        &edits,
        session.selected_profile.as_deref(),
    )?;
    *session.config.borrow_mut() = loaded.config;
    Ok(match (tier, value) {
        (Tier::Helpers, "off") => "helpers off; no helper will run".to_string(),
        (Tier::Subagents, "auto" | "inherit") => {
            "subagents use auto mode and inherit the parent's model by default".to_string()
        }
        (Tier::Subagents, "off") => "subagents off; no subagent will run".to_string(),
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
pub(super) fn login(session: &Session<'_>, argument: Option<&str>) {
    // `/login <account> device` asks for a device code instead of a link.
    let mut words = argument.unwrap_or_default().split_whitespace();
    let account = words.next();
    let device_code = words.next() == Some("device");
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

    stream_connect(session, &provider, account, device_code);
}

/// The gateway's credential table. **Every session asks, hosted or not**: the
/// credentials belong to the gateway whoever started it, so a session
/// Glasshouse handed a gateway to sees and enters the same keys as one that
/// started its own. Empty is what a gateway that could not be asked leaves.
fn api_keys(session: &Session<'_>) -> Vec<crate::gateway::CredentialRow> {
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

/// Runs the sign-in, showing what the gateway reports as it arrives.
///
/// Streamed rather than awaited because the first line is the link a person
/// must open and the last arrives minutes later. While it runs, an address
/// pasted into the panel's prompt goes to the gateway's stdin, which is how a
/// machine with no browser finishes: open the link anywhere, sign in, paste
/// where the browser landed.
fn stream_connect(session: &Session<'_>, provider: &str, account: &str, device_code: bool) {
    use std::io::{BufRead, BufReader, Write};
    use std::process::Stdio;
    use std::sync::mpsc::RecvTimeoutError;

    let mut arguments = vec![
        "subscriptions",
        "connect",
        provider,
        "--entitlement",
        account,
        "--json",
    ];
    if device_code {
        arguments.push("--device-code");
    }
    let unreachable = |text: &str| show(session, Panel::text("Connect an account", text));
    let Some(mut command) = session.gateway.control_command(&arguments) else {
        return unreachable("The inference gateway is not reachable.");
    };
    // Its own group, so a cancelled sign-in takes the broker's login (which
    // holds the provider's callback port) down with the gateway.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let Ok(mut child) = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return unreachable("The inference gateway could not be started.");
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        return;
    };
    let mut pasted_to = child.stdin.take();
    let (lines, arrived) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if lines.send(line).is_err() {
                break;
            }
        }
    });

    let mut panel = SignIn::new(account);
    show(session, panel.render());
    // Ctrl-C cancels the sign-in, as it cancels a tool call.
    let token = crate::tools::invoke::CancellationToken::new();
    session.interrupt.arm(token.clone());
    loop {
        match arrived.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => {
                if let Some(progress) = SignInProgress::read(&line) {
                    session_println!("{}", panel.apply(progress));
                    show(session, panel.render());
                }
            }
            Err(RecvTimeoutError::Timeout) if token.is_cancelled() => {
                #[cfg(unix)]
                crate::tools::invoke::kill_group(child.id());
                let _ = child.kill();
                session.interrupt.consumed();
                session_println!("Sign-in to {account} cancelled.");
                break;
            }
            Err(RecvTimeoutError::Timeout) => {
                if let (Some(ui), Some(pipe)) = (session.ui, pasted_to.as_mut())
                    && let Some(pasted) = ui.try_secret()
                    && writeln!(pipe, "{}", pasted.trim())
                        .and_then(|()| pipe.flush())
                        .is_ok()
                {
                    panel.pasted = true;
                    show(session, panel.render());
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    drop(pasted_to);
    let _ = child.wait();
}

/// One progress line the gateway's `subscriptions connect --json` writes.
/// Unknown shapes are dropped rather than printed raw: this is another
/// program's output and the panel is not a place to echo bytes nobody
/// recognised.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SignInProgress {
    Opened { link: String, browser_opened: bool },
    DeviceCode { link: String, code: String },
    Connected(Option<String>),
    Failed(String),
}

impl SignInProgress {
    fn read(line: &str) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_str(line).ok()?;
        let text = |key: &str| value.get(key).and_then(serde_json::Value::as_str);
        Some(match text("state")? {
            "opened" => Self::Opened {
                link: text("authorize_url")?.to_owned(),
                browser_opened: value
                    .get("browser_opened")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            },
            "device_code" => Self::DeviceCode {
                link: text("verification_url")?.to_owned(),
                code: text("user_code")?.to_owned(),
            },
            "connected" => Self::Connected(text("account").map(str::to_owned)),
            "failed" => Self::Failed(text("reason").unwrap_or("").to_owned()),
            _ => return None,
        })
    }
}

/// The sign-in panel: what to do next, and every way to do it.
#[derive(Debug, Default)]
struct SignIn {
    account: String,
    link: Option<(String, bool)>,
    device: Option<(String, String)>,
    pasted: bool,
    outcome: Option<String>,
}

impl SignIn {
    fn new(account: &str) -> Self {
        Self {
            account: account.to_owned(),
            ..Self::default()
        }
    }

    /// Records `progress` and returns the line the chat keeps for it: the
    /// whole link or code, so it can be read and selected after the panel is
    /// gone.
    fn apply(&mut self, progress: SignInProgress) -> String {
        let account = self.account.clone();
        match progress {
            SignInProgress::Opened {
                link,
                browser_opened,
            } => {
                let note = format!("Sign-in link for {account}:\n{link}");
                self.link = Some((link, browser_opened));
                note
            }
            SignInProgress::DeviceCode { link, code } => {
                let note = format!("Sign in to {account}: open {link} and enter the code {code}");
                self.device = Some((link, code));
                note
            }
            SignInProgress::Connected(label) => {
                let said = match label {
                    Some(label) => format!("{account} is connected as {label}."),
                    None => format!("{account} is connected."),
                };
                self.outcome = Some(said.clone());
                said
            }
            SignInProgress::Failed(reason) => {
                self.outcome = Some(format!("failed: {reason}"));
                format!("ERROR: signing in to {account} failed: {reason}")
            }
        }
    }

    fn render(&self) -> Panel {
        let row = |text: String, command: Option<String>| tui::PanelRow { text, command };
        let mut rows = Vec::new();
        if let Some((link, browser_opened)) = &self.link {
            rows.push(row(
                if *browser_opened {
                    "Sign in in the browser that just opened.".into()
                } else {
                    "Open the sign-in link in a browser.".into()
                },
                None,
            ));
            rows.push(row(
                "⏎ open the sign-in link in your default browser".into(),
                Some(format!("/open-link {link}")),
            ));
            rows.push(row(
                "⏎ copy the sign-in link".into(),
                Some(format!("/copy {link}")),
            ));
            rows.push(row(
                "⏎ no browser here? paste the address the browser ended on".into(),
                Some("/paste-callback".into()),
            ));
        }
        if let Some((link, code)) = &self.device {
            rows.push(row(format!("On any device, open {link}"), None));
            rows.push(row(format!("and enter the code {code}"), None));
            rows.push(row("⏎ copy the code".into(), Some(format!("/copy {code}"))));
            rows.push(row(
                "⏎ open the link in your default browser".into(),
                Some(format!("/open-link {link}")),
            ));
        }
        if self.pasted && self.outcome.is_none() {
            rows.push(row("pasted; finishing the sign-in…".into(), None));
        }
        rows.push(row(
            self.outcome
                .clone()
                .unwrap_or_else(|| "waiting for the sign-in…".into()),
            None,
        ));
        let mut panel = Panel::rows(format!("Connecting {}", self.account), rows);
        panel.selected = panel
            .rows
            .iter()
            .position(|row| row.command.is_some())
            .unwrap_or(0);
        panel
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
                            "This route supports default|low|medium|high; xhigh/max need a compatible Claude model."
                        );
                        return true;
                    }
                    session.effort.set(effort);
                    if let Some(ui) = session.ui {
                        ui.effort(effort);
                    }
                    session_println!("Effort: {} · applied to the next request", effort.name());
                } else {
                    session_println!("Use /effort default|low|medium|high|xhigh|max");
                }
            } else {
                let rows = ["default", "low", "medium", "high", "xhigh", "max"]
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
        "mode" => match argument.map(str::trim) {
            None => {
                session_println!(
                    "Mode: {}{}",
                    session.mode.get().name(),
                    if session.mode_pinned.get() {
                        " · pinned"
                    } else {
                        " · not pinned; a confident read-only request may propose explore"
                    }
                );
            }
            Some("auto") => {
                session.mode_pinned.set(false);
                session_println!(
                    "Mode: {} · unpinned; a confident read-only request may propose explore",
                    session.mode.get().name()
                );
            }
            Some(word) => match Mode::parse(word) {
                None => session_println!("Use /mode execute|explore|plan|auto"),
                Some(mode) => {
                    session.mode.set(mode);
                    session.mode_pinned.set(true);
                    if let Some(ui) = session.ui {
                        ui.mode(mode);
                    }
                    session_println!(
                        "Mode: {} · pinned · {} · applies from the next request",
                        mode.name(),
                        match mode {
                            Mode::Execute => "session sandbox applies",
                            Mode::Explore =>
                                "reads run; writes only under agent scratch and documentation globs; the shell is read-only",
                            Mode::Plan => "reads run; no change executes; the shell is read-only",
                        }
                    );
                }
            },
        },
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
                        "Last task: {used} cumulative tokens\nToken spend is telemetry and has no cap.\nCell limit: {}\nConfigure runtime limits in .pane/config.toml for the next session.",
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
        "config" => {
            let args = argument
                .unwrap_or("")
                .split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>();
            match crate::settings_commands::execute(&session.project.root, &args) {
                Ok(text) => show(
                    session,
                    Panel::text(
                        "Pane configuration",
                        format!(
                            "{text}\nRuntime changes require a new session; live sandbox unchanged."
                        ),
                    ),
                ),
                Err(error) => session_println!("ERROR: {error}"),
            }
        }
        "settings" => show(
            session,
            Panel::text(
                "Settings",
                "Open /settings in an interactive terminal. CLI: pane config --help",
            ),
        ),
        "status" => {
            show(
                session,
                Panel::text(
                    "Session configuration",
                    format!(
                        "Model: {}\nMode: {}\nProject: {}\nSandbox: {} path rules · {} command patterns · network {}\nWeb: {}\nTask spend: tracked, uncapped\nCell limit: {} cells · {} seconds each · response {} bytes\nSupervisor: {}\nHelper effort: find {} · reduce {} · check {}\nLimits and helper effort: .pane/config.toml (loaded at startup)\nPermissions: native global/project config (loaded at startup)\nPresentation: /theme · /sidebar · /statusline · /fullscreen",
                        session.model.borrow(),
                        session.mode.get().name(),
                        session.project.root.display(),
                        session.profile.rule_count(),
                        session.profile.command_pattern_count(),
                        session.profile.grants_network(),
                        session.config().web.describe(),
                        session.config().limits.cells,
                        session.config().limits.cell_wall_clock_s,
                        session.config().limits.response_bytes,
                        session
                            .config()
                            .supervisor
                            .model
                            .as_deref()
                            .unwrap_or("off"),
                        session.config().helpers.effort.find.name(),
                        session.config().helpers.effort.reduce.name(),
                        session.config().helpers.effort.check.name(),
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
                        "State: {}\nModel: {}\nCadence: every {} cells\nLatest: {}\nConfigure [supervisor] in .pane/config.toml for the next session.",
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
        "models" | "entitlements" => {
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
    let saved = crate::settings_session::permissions(&session.project.root, argument)?;
    Ok(format!(
        "Effective current session (immutable): {} path rules · {} command patterns · {} MCP patterns\n{saved}",
        session.profile.rule_count(),
        session.profile.command_pattern_count(),
        session.profile.mcp_tool_count()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gateway's sign-in lines become progress; the link and code arrive
    /// whole, and a line of another shape is dropped.
    #[test]
    fn sign_in_progress_reads_the_gateway_lines_whole() {
        let link = "https://claude.ai/oauth/authorize?client_id=x&scope=user%3Aprofile&state=s";
        assert_eq!(
            SignInProgress::read(&format!(
                r#"{{"state":"opened","authorize_url":"{link}","browser_opened":true}}"#
            )),
            Some(SignInProgress::Opened {
                link: link.into(),
                browser_opened: true
            })
        );
        assert_eq!(
            SignInProgress::read(
                r#"{"state":"device_code","verification_url":"https://auth.openai.com/codex/device","user_code":"ABCD-EFGH"}"#
            ),
            Some(SignInProgress::DeviceCode {
                link: "https://auth.openai.com/codex/device".into(),
                code: "ABCD-EFGH".into()
            })
        );
        assert_eq!(
            SignInProgress::read(r#"{"state":"connected","account":"me@example.com"}"#),
            Some(SignInProgress::Connected(Some("me@example.com".into())))
        );
        assert_eq!(SignInProgress::read("waiting for the browser"), None);
    }

    /// The panel offers every way through with the whole link behind each
    /// row, starts on the first thing to do, and the chat keeps the link.
    #[test]
    fn the_sign_in_panel_opens_copies_or_takes_a_pasted_address() {
        let link = "https://claude.ai/oauth/authorize?client_id=x&scope=user%3Aprofile&state=s";
        let mut sign_in = SignIn::new("claude-max");
        let note = sign_in.apply(SignInProgress::Opened {
            link: link.into(),
            browser_opened: false,
        });
        assert_eq!(note, format!("Sign-in link for claude-max:\n{link}"));
        let panel = sign_in.render();
        let commands: Vec<_> = panel
            .rows
            .iter()
            .filter_map(|row| row.command.clone())
            .collect();
        assert_eq!(
            commands,
            vec![
                format!("/open-link {link}"),
                format!("/copy {link}"),
                "/paste-callback".to_string(),
            ]
        );
        assert_eq!(
            panel.rows[panel.selected].command.as_deref(),
            Some(&*format!("/open-link {link}"))
        );
        assert_eq!(
            sign_in.apply(SignInProgress::Failed("status 400".into())),
            "ERROR: signing in to claude-max failed: status 400"
        );
        assert_eq!(
            sign_in.render().rows.last().unwrap().text,
            "failed: status 400"
        );
    }

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
        fs::create_dir_all(root.join(".pane")).unwrap();
        let path = root.join(".pane/config.toml");
        fs::write(
            &path,
            "# keep this comment\n[ui]\ntheme='amber'\n[permissions]\nallow=[]\ndeny=['Bash(rm *)']\n",
        )
        .unwrap();
        let project = ProjectConfig {
            root: root.clone(),
            ..ProjectConfig::default()
        };
        let config = PaneConfig::default();
        let profile = Profile::compile(
            &root,
            crate::settings::Store::new(&root)
                .unwrap()
                .permissions()
                .unwrap()
                .as_deref(),
        );
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
            selected_profile: None,
            pending_images: RefCell::new(Vec::new()),
            approval_gate: None,
            inbox: RefCell::new(crate::events::inbox::Inbox::discover(&glasshouse, &root)),
            window: RefCell::new(crate::events::window::Window::new(Default::default())),
            messages: std::rc::Rc::new(RefCell::new(std::collections::HashMap::new())),
            roster: Vec::new(),
            ui: None,
            model: RefCell::new("test".into()),
            context_window: None,
            interface: Cell::new(crate::abi::Interface::default()),
            manifest: crate::manifest::Manifest::default(),
            mode: Cell::new(tui::Mode::Execute),
            mode_pinned: Cell::new(false),
            overlay: ModeOverlay::default(),
            effort: Cell::new(wire::Effort::Default),
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
            plan: RefCell::new(None),
        };
        permissions(&session, Some("allow Read(**)")).unwrap();
        let saved = fs::read_to_string(&path).unwrap();
        let parsed: toml::Value = toml::from_str(&saved).unwrap();
        assert_eq!(parsed["ui"]["theme"].as_str(), Some("amber"));
        assert!(saved.contains("# keep this comment"));
        assert_eq!(
            parsed["permissions"]["deny"],
            toml::Value::Array(vec![toml::Value::String("Bash(rm *)".into())])
        );
        assert!(permissions(&session, Some("allow NotAGrant(foo)")).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), saved);
        permissions(&session, Some("remove Read(**)")).unwrap();
        let parsed: toml::Value = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(parsed["permissions"]["allow"], toml::Value::Array(vec![]));
        fs::remove_dir_all(root).unwrap();
    }

    /// Builds a session rooted at `root` and runs `body` against it.
    fn with_session(root: &std::path::Path, body: impl FnOnce(&Session<'_>)) {
        with_selected_session(root, None, body)
    }

    fn with_selected_session(
        root: &std::path::Path,
        selected: Option<&str>,
        body: impl FnOnce(&Session<'_>),
    ) {
        let project = ProjectConfig {
            root: root.to_path_buf(),
            ..ProjectConfig::default()
        };
        let config =
            RefCell::new(PaneConfig::load_profile(root, selected).expect("the fixture parses"));
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
            selected_profile: selected.map(str::to_string),
            pending_images: RefCell::new(Vec::new()),
            approval_gate: None,
            inbox: RefCell::new(crate::events::inbox::Inbox::discover(&glasshouse, root)),
            window: RefCell::new(crate::events::window::Window::new(Default::default())),
            messages: std::rc::Rc::new(RefCell::new(std::collections::HashMap::new())),
            roster: Vec::new(),
            ui: None,
            model: RefCell::new("opus-5".into()),
            context_window: None,
            interface: Cell::new(crate::abi::Interface::default()),
            manifest: crate::manifest::Manifest::default(),
            mode: Cell::new(tui::Mode::Execute),
            mode_pinned: Cell::new(false),
            overlay: ModeOverlay::default(),
            effort: Cell::new(wire::Effort::Default),
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
            plan: RefCell::new(None),
        };
        body(&session);
    }

    /// A tier assignment takes effect now and survives the session, and an
    /// unrelated setting in the same file is not collateral damage.
    #[test]
    fn assigning_a_tier_is_live_persisted_and_reversible() {
        let root = std::env::temp_dir().join(format!("pane-tier-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join(".pane")).unwrap();
        let file = root.join(".pane").join("config.toml");
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
            let saved = PaneConfig::load(&root).unwrap();
            assert_eq!(saved.agents.mode, AgentsMode::Pinned);
            assert_eq!(saved.agents.model.as_deref(), Some("claude-sonnet-5"));

            assign_model(session, Tier::Subagents, "off").unwrap();
            let saved = PaneConfig::load(&root).unwrap();
            assert_eq!(saved.agents.mode, AgentsMode::Off);
            assert_eq!(saved.agents.model, None);
            assert_eq!(tier_models(session).subagent.as_deref(), Some("off"));

            // Reversible, which is what makes the panel safe to press.
            assign_model(session, Tier::Helpers, "off").unwrap();
            assert_eq!(tier_models(session).helper, None);
            assert_eq!(PaneConfig::load(&root).unwrap().helpers.model, None);
            assign_model(session, Tier::Subagents, "inherit").unwrap();
            assert_eq!(tier_models(session).subagent, None);
            assert_eq!(
                PaneConfig::load(&root).unwrap().agents.mode,
                AgentsMode::Auto
            );

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
            let before = fs::read_to_string(&file).unwrap();
            assert!(assign_model(session, Tier::Parent, "auto").is_err());
            assert_eq!(fs::read_to_string(&file).unwrap(), before);
        });
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn tier_changes_preserve_selected_profile_and_leave_base_settings_unchanged() {
        let root = std::env::temp_dir().join(format!("pane-profile-tier-{}", std::process::id()));
        fs::create_dir_all(root.join(".pane")).unwrap();
        let file = root.join(".pane/config.toml");
        let text = "[model]\nparent='base-parent'\n[limits]\ncells=42\n[helpers]\nmodel='base-helper'\nenabled=true\n[agents]\nmodel='base-agent'\n[profiles.review.limits]\ncells=17\n[profiles.review.web]\nenabled=true\n[profiles.review.helpers]\nmodel='review-helper'\n";
        fs::write(&file, text).unwrap();
        let base = PaneConfig::parse(text).unwrap();
        with_selected_session(&root, Some("review"), |session| {
            assign_model(session, Tier::Helpers, "changed-helper").unwrap();
            assert_eq!(session.config().limits.cells, 17);
            assert!(session.config().web.enabled);
            assert_eq!(
                session.config().helpers.model.as_deref(),
                Some("changed-helper")
            );
            assign_model(session, Tier::Helpers, "off").unwrap();
            assert!(!session.config().helpers.enabled);
            assign_model(session, Tier::Subagents, "off").unwrap();
            assert_eq!(session.config().agents.mode, crate::config::AgentsMode::Off);
            assign_model(session, Tier::Subagents, "auto").unwrap();
            assert_eq!(
                session.config().agents.mode,
                crate::config::AgentsMode::Auto
            );
            assign_model(session, Tier::Parent, "review-parent").unwrap();
            assert_eq!(
                session.config().model.parent.as_deref(),
                Some("review-parent")
            );
            assert_eq!(PaneConfig::load(&root).unwrap(), base);
            assert_eq!(
                PaneConfig::load_profile(&root, Some("review")).unwrap(),
                *session.config()
            );
        });
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rollback_confirmation_is_interactive_and_bound_to_the_previewed_checkpoint() {
        assert!(rollback_confirmation(false, Some(1), 1).is_err());
        assert!(rollback_confirmation(true, None, 1).is_err());
        assert!(rollback_confirmation(true, Some(1), 2).is_err());
        assert_eq!(rollback_confirmation(true, Some(2), 2), Ok(()));
    }
}
