//! How much of each subscription's limits is used: the plan and every
//! window the provider enforces (five hours, a week, a week per model),
//! read from the provider's own usage endpoint with the account's saved
//! login -- the same figures Claude Code's `/usage` and Codex's status line
//! show.
//!
//! **The token never leaves this process.** It is read from the broker's
//! auth file, sent to the provider over TLS in one header, and dropped; what
//! is returned is plan names, percentages and reset times. A window the
//! provider does not report is absent, never a zero.

use std::path::Path;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

const TIMEOUT: Duration = Duration::from_secs(20);
const CLAUDE_USAGE: &str = "https://api.anthropic.com/api/oauth/usage";
const CLAUDE_PROFILE: &str = "https://api.anthropic.com/api/oauth/profile";
const CODEX_USAGE: &str = "https://chatgpt.com/backend-api/wham/usage";

/// One enforced window: how much is used and when it resets.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Window {
    /// `5h`, `week`, `week (Opus)`, … -- what a person calls it.
    pub name: String,
    pub used_percent: f64,
    /// RFC 3339, or Unix seconds rendered as RFC 3339; absent when the
    /// provider gave none (a window with nothing used yet).
    pub resets_at: Option<String>,
}

/// One subscription account's usage, or why it could not be read.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AccountUsage {
    pub account: String,
    /// `Max 20x`, `Pro`, `Plus`, `Pro Lite`, … as the provider names it.
    pub plan: Option<String>,
    pub email: Option<String>,
    pub windows: Vec<Window>,
    /// The provider says a limit is reached right now.
    pub limited: bool,
    pub error: Option<String>,
}

impl AccountUsage {
    fn failed(account: &str, error: impl Into<String>) -> Self {
        Self {
            account: account.to_string(),
            plan: None,
            email: None,
            windows: Vec::new(),
            limited: false,
            error: Some(error.into()),
        }
    }
}

/// The usage of every login in `auth_dir`, for the account named
/// `account`: the broker pools several logins of one provider under one
/// account and moves to the next when one is rate-limited, so each login's
/// limits are its own row. No login at all is one row saying so.
#[must_use]
pub fn read_all(account: &str, auth_dir: &Path) -> Vec<AccountUsage> {
    let logins = logins(auth_dir);
    if logins.is_empty() {
        return vec![AccountUsage::failed(account, "not connected")];
    }
    logins
        .iter()
        .map(|login| read_login(account, login))
        .collect()
}

fn read_login(account: &str, login: &Value) -> AccountUsage {
    let token = login["access_token"].as_str().unwrap_or_default();
    if token.is_empty() {
        return AccountUsage::failed(account, "the saved login holds no access token");
    }
    let email = login["email"].as_str().map(str::to_string);
    let mut usage = match login["type"].as_str() {
        Some("claude") => claude(account, token),
        Some("codex") => codex(account, token, login["account_id"].as_str().unwrap_or("")),
        Some(other) => AccountUsage::failed(account, format!("{other} publishes no usage")),
        None => AccountUsage::failed(account, "the saved login names no provider"),
    };
    if usage.email.is_none() {
        usage.email = email;
    }
    usage
}

fn logins(auth_dir: &Path) -> Vec<Value> {
    let Ok(entries) = std::fs::read_dir(auth_dir) else {
        return Vec::new();
    };
    let mut paths: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    paths.sort();
    paths
        .iter()
        .filter_map(|path| serde_json::from_slice::<Value>(&std::fs::read(path).ok()?).ok())
        .filter(|login| !login["disabled"].as_bool().unwrap_or(false))
        .collect()
}

fn get(url: &str, headers: &[(&str, &str)]) -> Result<Value, String> {
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(TIMEOUT))
            .build(),
    );
    let mut request = agent.get(url);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let mut response = request
        .call()
        .map_err(|_| "the provider did not answer".to_string())?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(match status {
            401 | 403 => {
                format!("the provider refused the saved login (HTTP {status}); sign in again")
            }
            _ => format!("the provider answered HTTP {status}"),
        });
    }
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|_| "the provider's answer could not be read".to_string())?;
    serde_json::from_str(&text).map_err(|_| "the provider's answer did not parse".to_string())
}

fn claude(account: &str, token: &str) -> AccountUsage {
    let bearer = format!("Bearer {token}");
    let headers = [
        ("authorization", bearer.as_str()),
        ("anthropic-beta", "oauth-2025-04-20"),
    ];
    let body = match get(CLAUDE_USAGE, &headers) {
        Ok(body) => body,
        Err(error) => return AccountUsage::failed(account, error),
    };
    let plan = get(CLAUDE_PROFILE, &headers)
        .ok()
        .and_then(|profile| claude_plan(&profile["organization"]));
    let windows = claude_windows(&body);
    AccountUsage {
        account: account.to_string(),
        plan,
        email: None,
        limited: windows.iter().any(|w| w.used_percent >= 100.0),
        windows,
        error: None,
    }
}

/// `five_hour` and `seven_day`, and the per-model weekly windows, in that
/// order; any other key the endpoint returns is an internal name and is left
/// out unless it has a reset time -- an active window nobody can name yet.
fn claude_windows(body: &Value) -> Vec<Window> {
    const KNOWN: [(&str, &str); 5] = [
        ("five_hour", "5h"),
        ("seven_day", "week"),
        ("seven_day_opus", "week (Opus)"),
        ("seven_day_sonnet", "week (Sonnet)"),
        ("seven_day_oauth_apps", "week (apps)"),
    ];
    let window = |value: &Value, name: String| {
        let used = value["utilization"].as_f64()?;
        Some(Window {
            name,
            used_percent: used,
            resets_at: value["resets_at"].as_str().map(str::to_string),
        })
    };
    let mut windows: Vec<Window> = KNOWN
        .iter()
        .filter_map(|(key, name)| window(&body[*key], (*name).to_string()))
        .collect();
    if let Some(map) = body.as_object() {
        for (key, value) in map {
            if KNOWN.iter().any(|(known, _)| known == key) || value["resets_at"].is_null() {
                continue;
            }
            if let Some(extra) = window(value, key.replace('_', " ")) {
                windows.push(extra);
            }
        }
    }
    windows
}

fn claude_plan(organization: &Value) -> Option<String> {
    let tier = organization["rate_limit_tier"].as_str().unwrap_or_default();
    let kind = organization["organization_type"]
        .as_str()
        .unwrap_or_default();
    Some(if tier.contains("max_20x") {
        "Max 20x".to_string()
    } else if tier.contains("max_5x") {
        "Max 5x".to_string()
    } else if kind.contains("max") {
        "Max".to_string()
    } else if kind.contains("pro") || tier.contains("pro") {
        "Pro".to_string()
    } else if kind.contains("team") {
        "Team".to_string()
    } else if kind.contains("enterprise") {
        "Enterprise".to_string()
    } else if kind.is_empty() {
        return None;
    } else {
        kind.replace('_', " ")
    })
}

fn codex(account: &str, token: &str, account_id: &str) -> AccountUsage {
    let bearer = format!("Bearer {token}");
    let headers = [
        ("authorization", bearer.as_str()),
        ("chatgpt-account-id", account_id),
    ];
    let body = match get(CODEX_USAGE, &headers) {
        Ok(body) => body,
        Err(error) => return AccountUsage::failed(account, error),
    };
    let windows = codex_windows(&body);
    AccountUsage {
        account: account.to_string(),
        plan: body["plan_type"].as_str().map(codex_plan),
        email: body["email"].as_str().map(str::to_string),
        limited: body["rate_limit"]["limit_reached"]
            .as_bool()
            .unwrap_or(false),
        windows,
        error: None,
    }
}

fn codex_windows(body: &Value) -> Vec<Window> {
    let mut windows = Vec::new();
    let mut push = |value: &Value, suffix: Option<&str>| {
        let Some(used) = value["used_percent"].as_f64() else {
            return;
        };
        let seconds = value["limit_window_seconds"].as_u64().unwrap_or(0);
        let span = match seconds {
            18_000 => "5h".to_string(),
            604_800 => "week".to_string(),
            0 => "window".to_string(),
            s if s % 86_400 == 0 => format!("{}d", s / 86_400),
            s => format!("{}h", s.div_ceil(3_600)),
        };
        windows.push(Window {
            name: suffix.map_or(span.clone(), |model| format!("{span} ({model})")),
            used_percent: used,
            resets_at: value["reset_at"].as_i64().map(unix_to_rfc3339),
        });
    };
    let limits = &body["rate_limit"];
    push(&limits["primary_window"], None);
    push(&limits["secondary_window"], None);
    if let Some(extra) = body["additional_rate_limits"].as_array() {
        for limit in extra {
            let model = limit["limit_name"]
                .as_str()
                .or_else(|| limit["model"].as_str());
            push(&limit["rate_limit"]["primary_window"], model);
            push(&limit["rate_limit"]["secondary_window"], model);
        }
    }
    windows
}

fn codex_plan(plan: &str) -> String {
    match plan {
        "prolite" => "Pro Lite".to_string(),
        "pro" => "Pro".to_string(),
        "plus" => "Plus".to_string(),
        "team" => "Team".to_string(),
        "business" => "Business".to_string(),
        "enterprise" => "Enterprise".to_string(),
        other => other.to_string(),
    }
}

/// Unix seconds as `YYYY-MM-DDTHH:MM:SSZ`, without a date crate.
fn unix_to_rfc3339(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let rest = seconds.rem_euclid(86_400);
    // Howard Hinnant's days-from-civil, inverted.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shapes both endpoints answered with on 2026-09-23, trimmed.
    #[test]
    fn both_providers_windows_are_read_and_an_unnamed_idle_window_is_left_out() {
        let claude = serde_json::json!({
            "five_hour": {"utilization": 4.0, "resets_at": "2026-09-23T10:30:00+00:00"},
            "seven_day": {"utilization": 16.0, "resets_at": "2026-09-28T22:00:00+00:00"},
            "seven_day_opus": null,
            "nimbus_quill": {"utilization": 0.0, "resets_at": null},
        });
        let windows = claude_windows(&claude);
        assert_eq!(
            windows
                .iter()
                .map(|w| (w.name.as_str(), w.used_percent))
                .collect::<Vec<_>>(),
            vec![("5h", 4.0), ("week", 16.0)]
        );
        assert_eq!(
            claude_plan(
                &serde_json::json!({"organization_type": "claude_max", "rate_limit_tier": "default_claude_max_20x"})
            ),
            Some("Max 20x".into())
        );

        let codex = serde_json::json!({
            "plan_type": "prolite",
            "rate_limit": {"limit_reached": false, "primary_window": {"used_percent": 84, "limit_window_seconds": 604800, "reset_at": 1790414522}, "secondary_window": null},
        });
        let windows = codex_windows(&codex);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].name, "week");
        assert_eq!(windows[0].used_percent, 84.0);
        assert_eq!(
            windows[0].resets_at.as_deref(),
            Some("2026-09-26T09:22:02Z")
        );
        assert_eq!(codex_plan("prolite"), "Pro Lite");
    }
}
