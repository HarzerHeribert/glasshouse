//! What Glasshouse can observe about a subscription account **without reading
//! a credential**, and the exact commands that change one.
//!
//! The invariant: this module reports *presence*, never *validity*. An
//! expired OAuth token is still a file on disk, so a directory entry proves
//! only that a login once happened. Reading the token to say more would put
//! account material back inside Glasshouse, which is the one thing the
//! CLIProxyAPI broker design exists to prevent — see
//! `crate::gateway::subscription_broker`. Every word this module produces is
//! therefore what was measured (`credential present`), never a verdict
//! (`valid`, `✓`).
//!
//! **Why it is in the library and not beside the command.** `glasshouse
//! subscriptions status` (`commands/subscriptions.rs`, in the binary crate)
//! and the Settings overlay's Subscriptions section (`shell::state::settings`,
//! in the library) must not be able to disagree about what `present` means:
//! on 2026-09-08 three accounts read `present`, one was expired, and the
//! failure surfaced mid-run as a routing error. One reader, two callers.
//!
//! **Connecting is deliberately not here.** `login` spawns the broker with
//! inherited stdio and blocks up to fifteen minutes on a browser OAuth flow;
//! a full-screen TUI holding the terminal in raw mode cannot host that. The
//! TUI's job is to name the command, which is what [`connect_command`] and
//! [`disconnect_command`] are for.

pub mod connect;

use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::cli::SubscriptionProvider;
use crate::config::{
    EffectiveConfig, EntitlementKind, EntitlementLookupError, EntitlementVendor, SubscriptionBroker,
};
use crate::paths::RuntimePaths;

/// The environment override `commands::subscriptions` accepts for the broker
/// binary. Named here too so [`broker_adopted`] and the login path agree about
/// when a binary is available.
const BROKER_BINARY_ENV: &str = "GLASSHOUSE_CLIPROXYAPI_BIN";

/// One configured entitlement that a CLIProxyAPI subscription can back, and
/// the one fact about it Glasshouse is allowed to know.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    /// The `[entitlements.<name>]` table this account is.
    pub entitlement: String,
    /// Which vendor's login flow connects it.
    pub provider: SubscriptionProvider,
    /// Whether the account's auth directory holds at least one file. **Not
    /// whether that file still authenticates anything** — see the module doc.
    pub credential_present: bool,
}

impl Account {
    /// The status word a surface shows, in the vocabulary the module doc
    /// fixes: what was measured, never a verdict.
    pub fn status(&self) -> &'static str {
        if self.credential_present {
            "credential present"
        } else {
            "not connected"
        }
    }

    /// The command that connects this account, spelled exactly as it must be
    /// typed.
    pub fn connect_command(&self) -> String {
        connect_command(self.provider, &self.entitlement)
    }

    /// The command that disconnects it.
    pub fn disconnect_command(&self) -> String {
        disconnect_command(self.provider, &self.entitlement)
    }
}

/// How many accounts are configured and how many carry a credential.
///
/// `configured == 0` is the first-run state the shell's landing panel names a
/// next step for. [`Summary::unknown`] is the fourth state and is distinct
/// from all three: nothing has been read yet, so nothing may be claimed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Summary {
    pub configured: usize,
    pub connected: usize,
    /// Whether a CLIProxyAPI binary is adopted, which must happen before any
    /// `login` can run.
    pub broker_adopted: bool,
    /// False until something actually read the configuration. A surface that
    /// renders an unread summary as "no accounts" would be stating a fact it
    /// does not have.
    pub read: bool,
}

impl Summary {
    /// Nothing has been read. The default, and what a failed read returns.
    pub fn unknown() -> Self {
        Self::default()
    }

    /// True only when the configuration was read and held no account at all.
    pub fn is_first_run(self) -> bool {
        self.read && self.configured == 0
    }

    /// True only when the configuration was read and no account carries a
    /// credential — including the first-run case.
    pub fn nothing_connected(self) -> bool {
        self.read && self.connected == 0
    }
}

/// The vendor login flow an entitlement's `kind`/`vendor` selects, or `None`
/// for an entitlement no subscription broker can connect.
///
/// One table, read by `glasshouse subscriptions status`, by `login`'s
/// validation and by the Settings overlay, so a row the TUI offers to connect
/// is a row the CLI will accept.
pub fn provider_for_entitlement(
    kind: Option<EntitlementKind>,
    vendor: Option<EntitlementVendor>,
) -> Option<SubscriptionProvider> {
    match (kind, vendor) {
        (Some(EntitlementKind::Claude), None | Some(EntitlementVendor::Claude))
        | (None, Some(EntitlementVendor::Claude)) => Some(SubscriptionProvider::Anthropic),
        (Some(EntitlementKind::ChatGpt), None | Some(EntitlementVendor::OpenAi))
        | (None, Some(EntitlementVendor::OpenAi)) => Some(SubscriptionProvider::Openai),
        (Some(EntitlementKind::Gemini), None | Some(EntitlementVendor::Google))
        | (None, Some(EntitlementVendor::Google)) => Some(SubscriptionProvider::Google),
        _ => None,
    }
}

/// Whether `dir` holds at least one regular file, which is the whole of what
/// "connected" can honestly mean here.
///
/// Refuses a symlink or a non-directory rather than following it: the auth
/// directory is private state, and a surface that reported through a symlink
/// would be reporting about a location the user did not choose.
pub fn credential_present(dir: &Path) -> Result<bool> {
    let metadata = match std::fs::symlink_metadata(dir) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err).with_context(|| format!("could not inspect `{dir:?}`")),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("subscription auth location `{dir:?}` is not a private directory");
    }
    for entry in std::fs::read_dir(dir).with_context(|| format!("could not inspect `{dir:?}`"))? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether a CLIProxyAPI binary is available to run a login with.
///
/// The same two doors `commands::subscriptions::resolve_broker_binary` opens —
/// the environment override, or the `sha256-…` version marker written by
/// `adopt-binary` — so a surface that says "adopted" is saying that a login
/// would get past its first step. Never an error: an unreadable marker is
/// simply not adopted, and there is no user act to report it to here.
pub fn broker_adopted(paths: &RuntimePaths) -> bool {
    if std::env::var_os(BROKER_BINARY_ENV).is_some_and(|value| !value.is_empty()) {
        return true;
    }
    let root = paths.managed_tools_dir().join("cliproxyapi");
    let Ok(version) = std::fs::read_to_string(root.join("current")) else {
        return false;
    };
    if !version.strip_prefix("sha256-").is_some_and(|digest| {
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    }) {
        return false;
    }
    let name = if cfg!(windows) {
        "cliproxyapi.exe"
    } else {
        "cliproxyapi"
    };
    root.join(version).join(name).is_file()
}

/// Every configured entitlement a CLIProxyAPI subscription backs, with its
/// credential presence read once each.
///
/// Sorted by entitlement name so the TUI's cursor and the CLI's table walk the
/// same order. An entitlement whose auth location is not a private directory
/// is reported as not connected rather than failing the whole listing: one
/// broken account must not hide the other three.
pub fn accounts(paths: &RuntimePaths, effective: &EffectiveConfig<'_>) -> Result<Vec<Account>> {
    let configured = effective
        .configured_entitlements()
        .map_err(|err: EntitlementLookupError| anyhow::anyhow!(err))?;
    let mut accounts = Vec::new();
    for entitlement in configured {
        if entitlement.backing().subscription_broker() != Some(SubscriptionBroker::CliProxyApi) {
            continue;
        }
        let Some(provider) = provider_for_entitlement(entitlement.kind(), entitlement.vendor())
        else {
            continue;
        };
        let dir = paths.subscription_broker_auth_dir(entitlement.name());
        accounts.push(Account {
            entitlement: entitlement.name().to_owned(),
            provider,
            credential_present: credential_present(&dir).unwrap_or(false),
        });
    }
    accounts.sort_by(|a, b| a.entitlement.cmp(&b.entitlement));
    Ok(accounts)
}

/// [`accounts`] counted, for a surface that needs the shape of the answer
/// rather than the rows.
pub fn summarise(paths: &RuntimePaths, effective: &EffectiveConfig<'_>) -> Summary {
    let Ok(accounts) = accounts(paths, effective) else {
        return Summary::unknown();
    };
    Summary {
        configured: accounts.len(),
        connected: accounts
            .iter()
            .filter(|account| account.credential_present)
            .count(),
        broker_adopted: broker_adopted(paths),
        read: true,
    }
}

/// The literal command that connects one account.
///
/// Spelled out in full, with the entitlement's real name substituted, because
/// a user who is told precisely what to type is helped and a user shown a
/// placeholder is not.
pub fn connect_command(provider: SubscriptionProvider, entitlement: &str) -> String {
    format!(
        "glasshouse subscriptions login {} --entitlement {entitlement}",
        provider.as_str()
    )
}

/// The literal command that disconnects one account.
pub fn disconnect_command(provider: SubscriptionProvider, entitlement: &str) -> String {
    format!(
        "glasshouse subscriptions logout {} --entitlement {entitlement}",
        provider.as_str()
    )
}

/// The literal command that must run before any `login` can: Glasshouse ships
/// no broker binary and downloads none, so the user supplies a verified one.
pub const ADOPT_BINARY_COMMAND: &str = "glasshouse subscriptions adopt-binary <PATH>";

/// The one sentence explaining what an entitlement is for, shown wherever an
/// account list is empty. Kept here rather than in the renderer so the CLI can
/// print the same words.
pub const NO_ACCOUNTS_HINT: &str = "No subscription account is configured. An account is an [entitlements.<name>] \
     table with subscription_broker = \"cliproxyapi\".";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presence_is_a_file_in_the_directory_and_nothing_is_read_from_it() {
        let dir = tempdir();
        let auth = dir.join("auth");
        std::fs::create_dir_all(&auth).unwrap();
        assert!(!credential_present(&auth).unwrap());
        std::fs::write(auth.join("token.json"), b"not even valid json").unwrap();
        assert!(
            credential_present(&auth).unwrap(),
            "presence is a directory entry; validity is deliberately unknowable here"
        );
    }

    #[test]
    fn a_missing_directory_is_not_connected_rather_than_an_error() {
        let dir = tempdir();
        assert!(!credential_present(&dir.join("never-created")).unwrap());
    }

    /// The vocabulary rule, asserted so a later edit cannot reintroduce a
    /// verdict word into a surface that only ever measured presence.
    #[test]
    fn no_status_word_claims_validity() {
        for present in [true, false] {
            let account = Account {
                entitlement: "claude-max".to_owned(),
                provider: SubscriptionProvider::Anthropic,
                credential_present: present,
            };
            let status = account.status();
            assert!(
                !status.contains("valid") && !status.contains("✓"),
                "`{status}` claims more than a directory entry can prove"
            );
        }
    }

    #[test]
    fn the_connect_command_names_the_account_it_is_shown_beside() {
        let account = Account {
            entitlement: "claude-max".to_owned(),
            provider: SubscriptionProvider::Anthropic,
            credential_present: false,
        };
        assert_eq!(
            account.connect_command(),
            "glasshouse subscriptions login anthropic --entitlement claude-max"
        );
        assert_eq!(
            account.disconnect_command(),
            "glasshouse subscriptions logout anthropic --entitlement claude-max"
        );
    }

    #[test]
    fn an_unread_summary_never_reads_as_a_first_run() {
        let unknown = Summary::unknown();
        assert!(!unknown.is_first_run());
        assert!(!unknown.nothing_connected());
        let read = Summary {
            configured: 0,
            connected: 0,
            broker_adopted: false,
            read: true,
        };
        assert!(read.is_first_run());
        assert!(read.nothing_connected());
    }

    #[test]
    fn every_kind_and_vendor_pair_the_cli_accepts_maps_to_one_provider() {
        assert_eq!(
            provider_for_entitlement(Some(EntitlementKind::Claude), None),
            Some(SubscriptionProvider::Anthropic)
        );
        assert_eq!(
            provider_for_entitlement(Some(EntitlementKind::ChatGpt), None),
            Some(SubscriptionProvider::Openai)
        );
        assert_eq!(
            provider_for_entitlement(Some(EntitlementKind::Gemini), None),
            Some(SubscriptionProvider::Google)
        );
        assert_eq!(
            provider_for_entitlement(None, Some(EntitlementVendor::Claude)),
            Some(SubscriptionProvider::Anthropic)
        );
        assert_eq!(provider_for_entitlement(None, None), None);
    }

    fn tempdir() -> std::path::PathBuf {
        let base = std::env::temp_dir().join(format!(
            "glasshouse-subscription-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&base).unwrap();
        base
    }
}
