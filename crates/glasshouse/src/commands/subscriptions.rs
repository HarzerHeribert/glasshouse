//! `glasshouse subscriptions` — a display of the gateway's subscription
//! accounts, and a forward for everything that would change one.
//!
//! User ruling, 2026-09-11: a subscription account, its broker and the
//! tokens behind it are the gateway's. So `status` reads — directory-entry
//! presence under the gateway's own broker layout, never a token's contents
//! — and `connect`, `login`, `logout` and `adopt-binary` are forwards to the
//! gateway binary, which owns the flows and the private directories they
//! write. Nothing here creates, moves or removes any of that state.

use std::path::Path;

use anyhow::{Context, Result};
use glasshouse::cli::SubscriptionProvider;
use glasshouse::config::{EntitlementKind, EntitlementVendor, Layers};
use glasshouse::{Runtime, RuntimePaths};

use super::gateway_forward;

pub(crate) fn status(runtime: &Runtime) -> Result<String> {
    let entitlements = configured_entitlements(runtime)?;
    let mut rows = Vec::new();
    for entitlement in entitlements {
        if entitlement.backing().subscription_broker()
            != Some(glasshouse::config::SubscriptionBroker::CliProxyApi)
        {
            continue;
        }
        let Some(provider) = provider_for_entitlement(entitlement.kind(), entitlement.vendor())
        else {
            continue;
        };
        validate_account_ancestors(runtime.paths(), entitlement.name())?;
        let state = if auth_present(
            &runtime
                .paths()
                .subscription_broker_auth_dir(entitlement.name()),
        )? {
            "present"
        } else {
            "absent"
        };
        rows.push(format!(
            "{}\t{}\t{state}",
            provider.as_str(),
            entitlement.name()
        ));
    }
    rows.sort();

    let mut out = String::from("provider\tentitlement\tstatus\n");
    for row in rows {
        out.push_str(&row);
        out.push('\n');
    }
    Ok(out)
}

/// `glasshouse subscriptions login <provider> --entitlement <name>` — the
/// broker's own browser flow, run by the gateway.
pub(crate) fn login(
    runtime: &Runtime,
    provider: SubscriptionProvider,
    entitlement: &str,
) -> Result<std::process::ExitCode> {
    gateway_forward::subscriptions(
        runtime.paths(),
        "login",
        provider.as_str(),
        entitlement,
        false,
    )
}

/// `glasshouse subscriptions connect <provider> --entitlement <name>` — the
/// gateway's own OAuth flow, reporting progress on the inherited stdout.
///
/// **The forward is why nothing here reads the child's output.** A connect
/// flow's progress lines carry an authorization URL and, at the end, an
/// account name; the exchange itself carries a code. Inheriting the pipes
/// rather than capturing them keeps every one of those out of this process.
pub(crate) fn connect(
    runtime: &Runtime,
    provider: SubscriptionProvider,
    entitlement: &str,
    json: bool,
) -> Result<std::process::ExitCode> {
    gateway_forward::subscriptions(
        runtime.paths(),
        "connect",
        provider.as_str(),
        entitlement,
        json,
    )
}

/// `glasshouse subscriptions logout <provider> --entitlement <name>`.
pub(crate) fn logout(
    runtime: &Runtime,
    provider: SubscriptionProvider,
    entitlement: &str,
) -> Result<std::process::ExitCode> {
    gateway_forward::subscriptions(
        runtime.paths(),
        "logout",
        provider.as_str(),
        entitlement,
        false,
    )
}

/// `glasshouse subscriptions adopt-binary <path>`.
pub(crate) fn adopt_binary(runtime: &Runtime, source: &Path) -> Result<std::process::ExitCode> {
    gateway_forward::adopt_binary(runtime.paths(), source.as_os_str())
}

fn configured_entitlements(
    runtime: &Runtime,
) -> Result<Vec<glasshouse::config::ResolvedEntitlement>> {
    let layers = Layers::load(runtime)?;
    Ok(layers.effective().configured_entitlements()?)
}

/// Delegates to `glasshouse::subscription`, which is where the table lives so
/// the Settings overlay in the library crate cannot offer to connect a row
/// this command would refuse.
fn provider_for_entitlement(
    kind: Option<EntitlementKind>,
    vendor: Option<EntitlementVendor>,
) -> Option<SubscriptionProvider> {
    glasshouse::subscription::provider_for_entitlement(kind, vendor)
}

/// Delegates to `glasshouse::subscription::credential_present` for the same
/// reason [`provider_for_entitlement`] does: `glasshouse subscriptions status`
/// and the Settings overlay must not be able to disagree about what `present`
/// means. The measurement — a directory entry, never the token's contents —
/// is unchanged and is documented there.
fn auth_present(dir: &Path) -> Result<bool> {
    glasshouse::subscription::credential_present(dir)
}

/// Refuse to *read* a broker directory that is not a real directory.
///
/// Still here, and still Glasshouse's, because this is a property of the
/// path a display is about to walk: a symlink where a private directory
/// belongs is a redirection of a read, and this command does the read.
fn validate_account_ancestors(paths: &RuntimePaths, entitlement: &str) -> Result<()> {
    for path in [
        paths.subscription_brokers_dir(),
        paths.subscription_broker_entitlement_dir(entitlement),
    ] {
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                anyhow::bail!(
                    "subscription broker private directory `{path:?}` is not a real directory"
                )
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("could not inspect `{path:?}`"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_paths_are_entitlement_separated_and_provider_independent() {
        let paths = RuntimePaths::new("/tmp/data", "/tmp/config");
        let a = paths.subscription_broker_auth_dir("personal");
        let b = paths.subscription_broker_auth_dir("work");
        let c = paths.subscription_broker_auth_dir("personal");
        assert_ne!(a, b);
        assert_eq!(a, c);
        assert!(a.ends_with("auth"));
        assert!(!a.to_string_lossy().contains("personal"));
        assert!(
            a.starts_with(paths.gateway_data_dir()),
            "broker state lives under the gateway's data directory: {a:?}"
        );
    }

    #[test]
    fn status_checks_presence_without_reading_token_contents() {
        let temp = tempfile::tempdir().unwrap();
        let auth = temp.path().join("auth");
        std::fs::create_dir_all(&auth).unwrap();
        assert!(!auth_present(&auth).unwrap());
        let token = auth.join("account.json");
        std::fs::write(&token, b"not even valid json").unwrap();
        assert!(auth_present(&auth).unwrap());
    }

    /// Every write verb is a forward, and a forward runs a binary — so the
    /// one thing this module must not still contain is a path that writes
    /// broker state itself. Read as a scan over this file's own source,
    /// which is how the other structural invariants in this crate are kept.
    #[test]
    fn no_write_verb_still_owns_broker_state() {
        let source = include_str!("subscriptions.rs");
        let production: String = source
            .split("#[cfg(test)]")
            .next()
            .expect("a source file has a first part")
            .to_owned();
        for forbidden in [
            "create_dir",
            "remove_dir_all",
            "OpenOptions",
            "std::fs::rename",
            "Command::new",
        ] {
            assert!(
                !production.contains(forbidden),
                "`{forbidden}` writes or runs; connecting, disconnecting and adopting are \
                 the gateway's, and this module forwards them"
            );
        }
    }
}
