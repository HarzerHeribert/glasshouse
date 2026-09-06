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
    let mut panel = Panel::text(
        "Models by entitlement",
        "Provider declarations; selecting a model does not pin routing to an account.",
    );
    match catalogue {
        Some(mut catalogue) if catalogue.version == 1 => {
            catalogue.accounts.sort_by(|a, b| a.account.cmp(&b.account));
            for mut account in catalogue.accounts {
                panel.rows.push(PanelRow {
                    text: format!(
                        "{} · {} · {}",
                        account.account,
                        account.provider.as_deref().unwrap_or("native harness"),
                        account.scope
                    ),
                    command: None,
                });
                account.models.sort();
                account.models.dedup();
                if account.models.is_empty() {
                    panel.rows.push(PanelRow {
                        text: "  No selectable model IDs reported".into(),
                        command: None,
                    });
                }
                for model in account.models {
                    // A catalogue row is a model ID, never an injected command argument.
                    if model.is_empty() || model.chars().any(char::is_whitespace) {
                        continue;
                    }
                    panel.rows.push(PanelRow {
                        text: format!("  {model}"),
                        command: Some(format!("/model {model}")),
                    });
                }
            }
        }
        _ => panel.rows.push(PanelRow {
            text: "Catalogue unavailable. Update Glasshouse or use /model <id>.".into(),
            command: None,
        }),
    }
    panel.rows.push(PanelRow {
        text: format!("Current: {}", session.model.borrow()),
        command: None,
    });
    panel.selected = panel
        .rows
        .iter()
        .position(|r| r.command.is_some())
        .unwrap_or(0);
    show(session, panel);
}

pub(super) fn command(
    name: &str,
    argument: Option<&str>,
    session: &Session<'_>,
    transcript: &Transcript,
) -> bool {
    match name {
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
                    "Task budget",
                    format!(
                        "Last task: {used} tokens\nLimit: {} tokens · {} cells\nConfigure limits in .glasshouse/pane.toml for the next session.",
                        session.config.limits.task_tokens, session.config.limits.cells
                    ),
                ),
            );
        }
        "context" => {
            let c = &transcript.conversation;
            let estimated = estimate_request_tokens(c, &session.model.borrow());
            let bytes: usize = c.messages.iter().map(|m| message_text(m).len()).sum();
            show(
                session,
                Panel::text(
                    "Context",
                    format!(
                        "{} messages · {} cells\nSystem: {} bytes\nMessages: {} bytes\nNext request: ~{} tokens (estimate)\nTask budget: {} tokens\nContext is retained in the rollout; no model call was made.",
                        c.messages.len(),
                        transcript.notebook.cells.len(),
                        c.system.len(),
                        bytes,
                        estimated,
                        session.config.limits.task_tokens
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
                        "Model: {}\nMode: {}\nProject: {}\nSandbox: {} path rules · {} command patterns · network {}\nTask budget: {} tokens · {} cells\nCell limit: {} seconds · response {} bytes\nSupervisor: {}\nLimits: .glasshouse/pane.toml (loaded at startup)\nPermissions: .claude/settings.json (loaded at startup)\nPresentation: /theme · /sidebar · /statusline",
                        session.model.borrow(),
                        session.mode.get().name(),
                        session.project.root.display(),
                        session.profile.rule_count(),
                        session.profile.command_pattern_count(),
                        session.profile.grants_network(),
                        session.config.limits.task_tokens,
                        session.config.limits.cells,
                        session.config.limits.cell_wall_clock_s,
                        session.config.limits.response_bytes,
                        session.config.supervisor.model.as_deref().unwrap_or("off")
                    ),
                ),
            );
        }
        "permissions" => match permissions(&session.project.root, argument) {
            Ok(text) => show(
                session,
                Panel::text("Permissions · changes apply next session", text),
            ),
            Err(error) => session_println!("ERROR: {error}"),
        },
        "entitlements" => {
            models(session);
        }
        _ => return false,
    }
    true
}

fn permissions(root: &std::path::Path, argument: Option<&str>) -> Result<String, String> {
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
        "{}\n{}\n\n/permissions allow <rule>\n/permissions remove <rule>\nThe running sandbox is unchanged; start a new session to apply edits.",
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
    fn permission_edits_preserve_other_settings_and_reject_invalid_grants() {
        let root = std::env::temp_dir().join(format!("pane-permissions-{}", std::process::id()));
        fs::create_dir_all(root.join(".claude")).unwrap();
        let path = root.join(".claude/settings.json");
        fs::write(
            &path,
            r#"{"other":{"keep":true},"permissions":{"allow":[],"deny":["Bash(rm *)"]}}"#,
        )
        .unwrap();
        permissions(&root, Some("allow Read(**)")).unwrap();
        let saved = fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&saved).unwrap();
        assert_eq!(parsed["other"]["keep"], true);
        assert_eq!(
            parsed["permissions"]["deny"],
            serde_json::json!(["Bash(rm *)"])
        );
        assert!(permissions(&root, Some("allow NotAGrant(foo)")).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), saved);
        permissions(&root, Some("remove Read(**)")).unwrap();
        let parsed: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(parsed["permissions"]["allow"], serde_json::json!([]));
        fs::remove_dir_all(root).unwrap();
    }
}
