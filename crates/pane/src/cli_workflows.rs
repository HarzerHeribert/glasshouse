//! CLI conveniences use the same session dispatch and permission boundary.
use clap::{CommandFactory, Parser};
use serde::Serialize;
use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};

pub fn session_options(args: &[String]) -> Vec<String> {
    let mut args = args.to_vec();
    if !args
        .iter()
        .any(|arg| arg == "--root" || arg.starts_with("--root="))
    {
        args.extend(["--root".into(), ".".into()]);
    }
    args
}

pub fn prepare(args: &[String]) -> Result<Option<Vec<String>>, String> {
    let Some(first) = args.first().map(String::as_str) else {
        return Ok(None);
    };
    let mut forwarded = match first {
        "exec" => {
            let mut rest = args[1..].to_vec();
            if rest.first().is_some_and(|arg| !arg.starts_with('-')) {
                let task = rest.remove(0);
                rest.extend(["--task".into(), task]);
            } else if !rest.iter().any(|arg| {
                matches!(arg.as_str(), "--task" | "--help" | "-h") || arg.starts_with("--task=")
            }) {
                if std::io::stdin().is_terminal() {
                    return Err("exec requires a task or piped stdin".into());
                }
                let mut task = String::new();
                std::io::stdin()
                    .read_to_string(&mut task)
                    .map_err(|e| format!("cannot read stdin: {e}"))?;
                if task.trim().is_empty() {
                    return Err("exec received an empty task on stdin".into());
                }
                rest.extend(["--task".into(), task]);
            }
            rest
        }
        "-p" | "--print" => {
            if args.len() < 2 {
                return Err("--print requires a task".into());
            }
            let mut rest = vec!["--task".into(), args[1].clone()];
            rest.extend_from_slice(&args[2..]);
            rest
        }
        "--continue" => {
            let mut rest = vec!["--resume=".into()];
            rest.extend_from_slice(&args[1..]);
            rest
        }
        option if session_option(option) => args.to_vec(),
        _ => return Ok(None),
    };
    forwarded = session_options(&forwarded);
    Ok(Some(forwarded))
}

fn session_option(option: &str) -> bool {
    let Some(name) = option.strip_prefix("--") else {
        return false;
    };
    let name = name.split('=').next().unwrap_or(name);
    pane::session::SessionArgs::command()
        .get_arguments()
        .any(|arg| arg.get_long() == Some(name))
}

#[derive(Parser)]
#[command(
    name = "pane doctor",
    about = "Read-only local diagnostics; never starts a gateway or contacts a provider"
)]
struct DoctorArgs {
    #[arg(long, default_value = ".")]
    root: PathBuf,
    #[arg(long)]
    json: bool,
}

#[derive(Serialize)]
struct Check {
    name: &'static str,
    status: &'static str,
    detail: String,
}

#[derive(Serialize)]
struct Report {
    schema_version: u32,
    version: &'static str,
    platform: &'static str,
    root: PathBuf,
    ok: bool,
    checks: Vec<Check>,
}

pub fn doctor(args: &[String]) -> i32 {
    let args = match DoctorArgs::try_parse_from(
        std::iter::once("pane doctor".into()).chain(args.iter().cloned()),
    ) {
        Ok(args) => args,
        Err(error) => {
            let code = error.exit_code();
            let _ = error.print();
            return code;
        }
    };
    let mut checks = Vec::new();
    let root = match args.root.canonicalize() {
        Ok(root) if root.is_dir() => {
            checks.push(Check {
                name: "project",
                status: "ok",
                detail: "Project directory exists".into(),
            });
            root
        }
        _ => {
            checks.push(Check {
                name: "project",
                status: "error",
                detail: "Project root does not exist or is not a directory".into(),
            });
            args.root
        }
    };
    let store = pane::settings::Store::new(&root);
    match store.as_ref().map_err(|e| e.clone()).and_then(|store|store.load(None)) {
        Ok(loaded)=>checks.push(Check {name:"config",status:if loaded.config.model.parent.is_some(){"ok"}else{"warning"},detail:if loaded.config.model.parent.is_some(){"Native/legacy configuration parses; parent model configured".into()}else{"Configuration parses; supply --model or pane config local model.parent to start a task".into()}}),
        Err(_)=>checks.push(Check{name:"config",status:"error",detail:"Configuration is invalid or unreadable; no values exposed".into()}),
    }
    let mut project = pane::project::load(&root);
    project.settings = store
        .as_ref()
        .ok()
        .and_then(|store| store.permissions().ok())
        .flatten();
    let profile = pane::sandbox::profile::Profile::from_project(&project);
    checks.push(Check {
        name: "permissions",
        status: if profile.diagnostics().is_empty() {
            "ok"
        } else {
            "warning"
        },
        detail: format!(
            "{} file rules, {} command patterns, {} MCP grants, {} diagnostics",
            profile.rule_count(),
            profile.command_pattern_count(),
            profile.mcp_tool_count(),
            profile.diagnostics().len()
        ),
    });
    for program in ["glasshouse", "inference-gateway", "git", "rg", "fd", "jq"] {
        let found = find_executable(program);
        let attached =
            program == "inference-gateway" && std::env::var_os("ANTHROPIC_BASE_URL").is_some();
        checks.push(Check {
            name: program,
            status: if found.is_some() || attached {
                "ok"
            } else {
                "warning"
            },
            detail: if attached {
                "Gateway endpoint supplied in environment (value redacted; connectivity not probed)"
                    .into()
            } else {
                found
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "Not found on PATH".into())
            },
        });
    }
    checks.push(sandbox_check());
    let report = Report {
        schema_version: 1,
        version: env!("CARGO_PKG_VERSION"),
        platform: std::env::consts::OS,
        root,
        ok: !checks.iter().any(|check| check.status == "error"),
        checks,
    };
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("report is serializable")
        );
    } else {
        println!(
            "Pane {} diagnostics ({})\nProject: {}",
            report.version,
            report.platform,
            report.root.display()
        );
        for check in &report.checks {
            println!("{} {}: {}", check.status, check.name, check.detail);
        }
    }
    if report.ok { 0 } else { 1 }
}

fn find_executable(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).find_map(|dir| {
        let path = dir.join(if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.into()
        });
        executable(&path).then_some(path)
    })
}

fn executable(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn sandbox_check() -> Check {
    #[cfg(target_os = "linux")]
    {
        let abi = pane::sandbox::linux::landlock_abi();
        Check {
            name: "sandbox",
            status: if abi >= 3 { "ok" } else { "error" },
            detail: format!(
                "Landlock ABI {abi}; at least ABI 3 is required for tool spawning. Network enforcement is reported separately by the session."
            ),
        }
    }
    #[cfg(target_os = "macos")]
    {
        Check {
            name: "sandbox",
            status: "ok",
            detail: "Seatbelt backend compiled; per-command enforcement occurs at spawn".into(),
        }
    }
    #[cfg(target_os = "windows")]
    {
        Check {
            name: "sandbox",
            status: "warning",
            detail: "AppContainer backend compiled; verify Windows Firewall for network isolation"
                .into(),
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        Check {
            name: "sandbox",
            status: "error",
            detail: "No supported OS sandbox backend; tool spawning is refused".into(),
        }
    }
}
