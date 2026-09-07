//! Human-invoked session inspection and configuration. No model dispatch.
use super::*;
use crate::tui::{Mode, Panel, PanelRow};

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
}

pub(super) fn models(session: &Session<'_>) {
    let root = session.project.root.to_string_lossy();
    let catalogue = session
        .glasshouse
        .run(
            &["--scope", &root, "entitlements", "--json", "--refresh"],
            None,
        )
        .and_then(|bytes| serde_json::from_slice::<Catalogue>(&bytes).ok());
    show(session, model_panel(catalogue, &session.model.borrow()));
}

fn model_panel(catalogue: Option<Catalogue>, current: &str) -> Panel {
    let title = format!("Models by provider · Current: {current}");
    match catalogue {
        Some(catalogue) if catalogue.version == 1 => Panel::models(
            title,
            catalogue
                .accounts
                .into_iter()
                .map(|account| tui::ModelGroup {
                    provider: account.provider.unwrap_or_else(|| "native harness".into()),
                    account: account.account,
                    scope: account.scope,
                    models: account.models,
                    selectable: account.selectable,
                    unavailable_reason: account.unavailable_reason,
                })
                .collect(),
        ),
        _ => Panel::text(
            title,
            "Catalogue unavailable. Update Glasshouse or use /model <id>.",
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
            let used = transcript.notebook.tokens.map(|t| t.used).unwrap_or(0);
            show(
                session,
                Panel::text(
                    "Task spend",
                    format!(
                        "Last task: {used} cumulative tokens\nToken spend is telemetry and has no cap.\nCell limit: {}\nConfigure runtime limits in .glasshouse/pane.toml for the next session.",
                        session.config.limits.cells
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
                        "Model: {}\nMode: {}\nProject: {}\nSandbox: {} path rules · {} command patterns · network {}\nTask spend: tracked, uncapped\nCell limit: {} cells · {} seconds each · response {} bytes\nSupervisor: {}\nLimits: .glasshouse/pane.toml (loaded at startup)\nPermissions: .claude/settings.json (loaded at startup)\nPresentation: /theme · /sidebar · /statusline",
                        session.model.borrow(),
                        session.mode.get().name(),
                        session.project.root.display(),
                        session.profile.rule_count(),
                        session.profile.command_pattern_count(),
                        session.profile.grants_network(),
                        session.config.limits.cells,
                        session.config.limits.cell_wall_clock_s,
                        session.config.limits.response_bytes,
                        session.config.supervisor.model.as_deref().unwrap_or("off")
                    ),
                ),
            );
        }
        "supervisor" => {
            let latest = match transcript.notebook.supervisor.as_ref() {
                Some(SupervisorStatus::Nudged(reason)) => format!("nudged: {reason}"),
                Some(SupervisorStatus::LookedNoNudge) => "looked; no nudge".into(),
                Some(SupervisorStatus::Off) | None => "no look in this session".into(),
            };
            show(
                session,
                Panel::text(
                    "Supervisor",
                    format!(
                        "State: {}\nModel: {}\nCadence: every {} cells\nLatest: {}\nConfigure [supervisor] in .glasshouse/pane.toml for the next session.",
                        if session.config.supervisor.enabled
                            && session.config.supervisor.model.is_some()
                        {
                            "active"
                        } else {
                            "off"
                        },
                        session
                            .config
                            .supervisor
                            .model
                            .as_deref()
                            .unwrap_or("not configured"),
                        session.config.supervisor.every,
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
        let mut panel = model_panel(Some(catalogue), "current");
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
            mode: Cell::new(tui::Mode::Execute),
            effort: Cell::new(wire::Effort::Auto),
            project: &project,
            config: &config,
            interrupt: &interrupt,
            profile: &profile,
            glasshouse: &glasshouse,
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

    #[test]
    fn rollback_confirmation_is_interactive_and_bound_to_the_previewed_checkpoint() {
        assert!(rollback_confirmation(false, Some(1), 1).is_err());
        assert!(rollback_confirmation(true, None, 1).is_err());
        assert!(rollback_confirmation(true, Some(1), 2).is_err());
        assert_eq!(rollback_confirmation(true, Some(2), 2), Ok(()));
    }
}
