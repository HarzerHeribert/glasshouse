//! How much context each subscription account is served with, per model, as
//! the provider tells the account itself: ChatGPT's Codex model list carries
//! `context_window`, Claude's `/v1/models` carries `max_input_tokens` and
//! `max_tokens`. **A window is a property of the plan, not of the model** --
//! a Max login and a Pro login see different figures for one model -- so
//! this is read with each login's own token, never from a published table.
//!
//! **The invariant: one figure per model per account, the smallest any of its
//! logins reports.** The broker pools several logins under one account and
//! moves between them mid-session, so the window a session may plan for is
//! the one every login it can land on accepts.
//!
//! Read when a broker starts (its first refresh makes the saved login
//! current), written beside the broker's auth directory, and read back by
//! `inference-gateway models --json` in another process. The token never
//! leaves this process, as in `subscription_usage`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::subscription_usage::{get, logins};

/// Every model, whatever its minimum client: the list is a filter on
/// `client_version`, and a window is wanted for every model the broker may
/// serve. The endpoint refuses a request without one.
const CODEX_MODELS: &str = "https://chatgpt.com/backend-api/codex/models?client_version=99.0.0";
const CLAUDE_MODELS: &str = "https://api.anthropic.com/v1/models?limit=1000";
const FILE: &str = "served-windows.json";
const FORMAT_VERSION: u32 = 1;
/// A reading this recent is not asked for again when a broker restarts.
const FRESH_FOR_SECONDS: i64 = 3_600;

/// What one account is served with for one model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServedLimit {
    pub context_window_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
}

impl ServedLimit {
    /// The figure both of two logins accept.
    fn smaller(self, other: Self) -> Self {
        Self {
            context_window_tokens: self.context_window_tokens.min(other.context_window_tokens),
            max_output_tokens: match (self.max_output_tokens, other.max_output_tokens) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            },
        }
    }
}

/// One account's reading, as kept on disk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServedWindows {
    version: u32,
    pub account: String,
    pub fetched_at_unix: i64,
    pub models: BTreeMap<String, ServedLimit>,
}

/// Every login's models, folded to the smallest figure per model. An error
/// only when no login answered at all.
pub fn read(auth_dir: &Path) -> Result<BTreeMap<String, ServedLimit>, String> {
    let mut folded: BTreeMap<String, ServedLimit> = BTreeMap::new();
    let mut last_error = "no saved login".to_string();
    let mut answered = false;
    for login in logins(auth_dir) {
        let token = login["access_token"].as_str().unwrap_or_default();
        let bearer = format!("Bearer {token}");
        let read = match login["type"].as_str() {
            Some("codex") => get(
                CODEX_MODELS,
                &[
                    ("authorization", bearer.as_str()),
                    (
                        "chatgpt-account-id",
                        login["account_id"].as_str().unwrap_or(""),
                    ),
                ],
            )
            .map(|body| codex_limits(&body)),
            Some("claude") => get(
                CLAUDE_MODELS,
                &[
                    ("authorization", bearer.as_str()),
                    ("anthropic-beta", "oauth-2025-04-20"),
                    ("anthropic-version", "2023-06-01"),
                ],
            )
            .map(|body| claude_limits(&body)),
            _ => continue,
        };
        match read {
            Ok(models) => {
                answered = true;
                for (model, limit) in models {
                    folded
                        .entry(model)
                        .and_modify(|held| *held = held.smaller(limit))
                        .or_insert(limit);
                }
            }
            Err(error) => last_error = error,
        }
    }
    if answered {
        Ok(folded)
    } else {
        Err(last_error)
    }
}

fn codex_limits(body: &Value) -> BTreeMap<String, ServedLimit> {
    body["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|model| {
            Some((
                model["slug"].as_str()?.to_string(),
                ServedLimit {
                    context_window_tokens: model["context_window"].as_u64()?,
                    max_output_tokens: None,
                },
            ))
        })
        .collect()
}

fn claude_limits(body: &Value) -> BTreeMap<String, ServedLimit> {
    body["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|model| {
            Some((
                model["id"].as_str()?.to_string(),
                ServedLimit {
                    context_window_tokens: model["max_input_tokens"].as_u64()?,
                    max_output_tokens: model["max_tokens"].as_u64(),
                },
            ))
        })
        .collect()
}

fn path(entitlement_dir: &Path) -> PathBuf {
    entitlement_dir.join(FILE)
}

/// The reading kept for one account, or `None` for every way it can fail.
#[must_use]
pub fn load(entitlement_dir: &Path) -> Option<ServedWindows> {
    let stored: ServedWindows =
        serde_json::from_slice(&std::fs::read(path(entitlement_dir)).ok()?).ok()?;
    (stored.version == FORMAT_VERSION).then_some(stored)
}

/// Writes one account's reading, through a temporary file and a rename.
pub fn store(
    entitlement_dir: &Path,
    account: &str,
    models: BTreeMap<String, ServedLimit>,
    now: i64,
) -> std::io::Result<()> {
    let stored = ServedWindows {
        version: FORMAT_VERSION,
        account: account.to_string(),
        fetched_at_unix: now,
        models,
    };
    let bytes = serde_json::to_vec_pretty(&stored).map_err(std::io::Error::other)?;
    let temporary = entitlement_dir.join(format!("{FILE}.tmp"));
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(temporary, path(entitlement_dir))
}

/// Every account's reading under `brokers_dir`.
#[must_use]
pub fn load_all(brokers_dir: &Path) -> Vec<ServedWindows> {
    let Ok(entries) = std::fs::read_dir(brokers_dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| load(&entry.path()))
        .collect()
}

/// Reads and stores one account's windows on a thread of its own, unless a
/// reading under an hour old is already kept. A saved login may be stale
/// until the broker that just started refreshes it, so a refused read is
/// tried again for up to a minute. Every failure leaves the kept reading
/// standing; nothing here is ever an error for the caller.
pub fn refresh_in_background(entitlement_dir: PathBuf, auth_dir: PathBuf, account: String) {
    let now = super::cache::now_unix_seconds();
    if load(&entitlement_dir).is_some_and(|kept| now - kept.fetched_at_unix < FRESH_FOR_SECONDS) {
        return;
    }
    std::thread::spawn(move || {
        for _ in 0..12 {
            if let Ok(models) = read(&auth_dir) {
                if !models.is_empty() {
                    let _ = store(
                        &entitlement_dir,
                        &account,
                        models,
                        super::cache::now_unix_seconds(),
                    );
                }
                return;
            }
            std::thread::sleep(Duration::from_secs(5));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_providers_list_is_read_for_its_window_and_the_smaller_of_two_logins_wins() {
        let codex = codex_limits(&serde_json::json!({"models": [
            {"slug": "gpt-6-sol", "context_window": 272000, "max_context_window": 872000},
            {"slug": "no-window"}
        ]}));
        assert_eq!(
            codex,
            BTreeMap::from([(
                "gpt-6-sol".to_string(),
                ServedLimit {
                    context_window_tokens: 272_000,
                    max_output_tokens: None
                }
            )])
        );
        let claude = claude_limits(&serde_json::json!({"data": [
            {"id": "claude-opus-5-5", "max_input_tokens": 1000000, "max_tokens": 128000}
        ]}));
        let max = claude["claude-opus-5-5"];
        assert_eq!(max.context_window_tokens, 1_000_000);
        assert_eq!(max.max_output_tokens, Some(128_000));

        let pro = ServedLimit {
            context_window_tokens: 200_000,
            max_output_tokens: Some(64_000),
        };
        assert_eq!(
            max.smaller(pro),
            pro,
            "a pooled Pro login bounds the account"
        );
        assert_eq!(pro.smaller(max), pro);
    }

    #[test]
    fn a_stored_reading_is_read_back_and_found_under_the_brokers_dir() {
        let brokers = tempfile::tempdir().unwrap();
        let entitlement = brokers.path().join("entitlement-6161");
        std::fs::create_dir_all(&entitlement).unwrap();
        let models = BTreeMap::from([(
            "gpt-6-sol".to_string(),
            ServedLimit {
                context_window_tokens: 272_000,
                max_output_tokens: None,
            },
        )]);
        store(
            &entitlement,
            "chatgpt-subscription",
            models.clone(),
            1_789_000_000,
        )
        .unwrap();

        let all = load_all(brokers.path());
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].account, "chatgpt-subscription");
        assert_eq!(all[0].models, models);
        assert_eq!(all[0].fetched_at_unix, 1_789_000_000);
    }
}
