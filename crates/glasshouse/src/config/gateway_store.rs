//! The gateway's own store, as Glasshouse **reads** it.
//!
//! User ruling, 2026-09-11: *subscription, account and broker state is the
//! gateway's; Glasshouse must neither own nor manage it.* This module is the
//! whole of Glasshouse's relationship to that state — a read of
//! `gateway.toml`, and the accounts and providers it declares. Nothing here
//! writes, and nothing here resolves a credential: an
//! [`inference_gateway::entitlement::AccountEntry`] carries a **reference**,
//! and the reference is resolved through a
//! [`crate::secret::SecretStore`] at the moment of use as it always was.
//!
//! Glasshouse keeps a *policy overlay* beside this — which harnesses an
//! account may serve, its tier and job-kind rules, its spend ceiling, its
//! context-firewall choice — in `[entitlements.<name>]` and
//! `[providers.<name>]` of its own `config.toml`. The overlay attaches **by
//! name** to what this catalogue holds and can state nothing about the
//! account itself; see [`super::entitlement::EntitlementConfig`].

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use inference_gateway::config::{GatewayConfig, ProviderEntry};
use inference_gateway::entitlement::AccountEntry;

use super::ConfigError;

/// The five directories the gateway owns since the 2026-09-11 ruling, as
/// (the name Glasshouse's data directory used, the name the gateway's uses).
///
/// Four keep their name and one does not: Glasshouse called the model
/// catalogue cache `providers/` and the gateway calls it `model-catalogues/`
/// ([`inference_gateway::config::model_cache_dir`]). Named once, here, and
/// read by both things that must agree about it — `glasshouse doctor`, which
/// reports one left behind, and `glasshouse migrate-gateway-state`, which
/// moves it.
pub const GATEWAY_OWNED_DIRECTORIES: [(&str, &str); 5] = [
    ("subscription-brokers", "subscription-brokers"),
    ("tools", "tools"),
    ("providers", "model-catalogues"),
    ("gateway-quota", "gateway-quota"),
    ("gateway-health", "gateway-health"),
];

/// The gateway's accounts and providers, already parsed.
///
/// A missing `gateway.toml` is an **empty catalogue, not an error** — the
/// gateway's own rule, kept here so that a Glasshouse run on a machine where
/// no account has been connected behaves exactly as it did before any of this
/// existed: every harness's own sign-in still resolves to a default
/// entitlement, and nothing else is configured.
#[derive(Debug, Clone, Default)]
pub struct GatewayCatalogue {
    config: GatewayConfig,
    path: Option<PathBuf>,
    present: bool,
}

/// The catalogue a caller that has not read one uses — no accounts, no
/// configured providers. `'static` so [`super::EffectiveConfig::new`] can keep
/// its two-argument signature and still hold a reference.
fn empty_catalogue() -> &'static GatewayCatalogue {
    static EMPTY: OnceLock<GatewayCatalogue> = OnceLock::new();
    EMPTY.get_or_init(GatewayCatalogue::default)
}

impl GatewayCatalogue {
    /// The empty catalogue — what a layering that was never handed a gateway
    /// configuration reads from.
    pub fn empty() -> &'static Self {
        empty_catalogue()
    }

    /// Read `gateway.toml` from `path`.
    ///
    /// A file that is not there yields an empty catalogue that still
    /// remembers where it looked, so a refusal can name the file the user
    /// needs to write. A file that is there and will not parse is an error:
    /// the user wrote it, and quietly serving from a catalogue nobody
    /// intended is the failure this refuses.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let loaded = inference_gateway::config::load(Some(path)).map_err(|error| {
            ConfigError::GatewayStore {
                path: path.to_path_buf(),
                message: crate::secret::redact(&format!("{error:#}")),
            }
        })?;
        Ok(Self {
            config: loaded.config,
            path: loaded.path,
            present: loaded.present,
        })
    }

    /// The catalogue `paths` names, for a caller that has a
    /// [`crate::RuntimePaths`] rather than a bare path.
    pub fn for_paths(paths: &crate::RuntimePaths) -> Result<Self, ConfigError> {
        Self::load(paths.gateway_config_path())
    }

    /// Parse one `gateway.toml` text. The shape a test writes, and what
    /// `glasshouse migrate-gateway-state` re-reads to check its own output.
    pub fn from_toml(text: &str) -> Result<Self, ConfigError> {
        let config =
            inference_gateway::config::parse(text).map_err(|error| ConfigError::GatewayStore {
                path: PathBuf::from("gateway.toml"),
                message: crate::secret::redact(&format!("{error:#}")),
            })?;
        Ok(Self {
            config,
            path: None,
            present: true,
        })
    }

    /// Where this catalogue was read from, when it was read from a file.
    /// Named in every refusal about an overlay with no account behind it, so
    /// the user is told which file to add the account to.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The path a message should name — the file itself when one is known,
    /// and a stand-in otherwise, so no message ever renders an empty path.
    pub fn describe_path(&self) -> String {
        match &self.path {
            Some(path) => path.display().to_string(),
            None => "the gateway's gateway.toml".to_owned(),
        }
    }

    /// Whether the file actually exists. `false` is the first-run case, and
    /// the reason a refusal about a missing account can say *connect it with
    /// `glasshouse subscriptions connect`* rather than *fix your file*.
    pub fn present(&self) -> bool {
        self.present
    }

    /// One account by name, or `None`.
    pub fn account(&self, name: &str) -> Option<&AccountEntry> {
        self.config.accounts.get(name)
    }

    /// Every account, in name order — the gateway's own `BTreeMap` order.
    pub fn accounts(&self) -> impl Iterator<Item = (&str, &AccountEntry)> {
        self.config
            .accounts
            .iter()
            .map(|(name, entry)| (name.as_str(), entry))
    }

    pub fn account_names(&self) -> impl Iterator<Item = &str> {
        self.config.accounts.keys().map(String::as_str)
    }

    /// Every provider an account may name: the ones configured in
    /// `gateway.toml` and the built-in templates a configured entry did not
    /// override. [`inference_gateway::config::providers`] decides this, not
    /// Glasshouse — the list is the gateway's.
    pub fn providers(&self) -> Vec<crate::provider::Provider> {
        inference_gateway::config::providers(&self.config)
    }

    /// One provider by name, resolved the same way.
    pub fn provider(&self, name: &str) -> Option<crate::provider::Provider> {
        self.providers().into_iter().find(|p| p.name == name)
    }

    /// Only the providers `gateway.toml` itself configures — not the
    /// built-in templates. The set a listing calls *configured*, so a user
    /// who wrote nothing sees nothing rather than every template this build
    /// happens to ship.
    pub fn configured_provider_names(&self) -> impl Iterator<Item = &str> {
        self.config.providers.keys().map(String::as_str)
    }

    /// The configured entry for `name`, when `gateway.toml` has one.
    pub fn provider_entry(&self, name: &str) -> Option<&ProviderEntry> {
        self.config.providers.get(name)
    }

    /// Whether an overlay may attach to `name` at all — configured here, or
    /// a built-in template. The check behind the *overlay for a provider the
    /// gateway does not have* refusal.
    pub fn has_provider(&self, name: &str) -> bool {
        self.config.providers.contains_key(name)
            || inference_gateway::provider::template(name).is_some()
            || name == "anthropic"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A catalogue read from text carries the accounts and the configured
    /// providers, and the empty one carries neither.
    #[test]
    fn a_catalogue_carries_the_gateways_accounts_and_configured_providers() {
        let catalogue = GatewayCatalogue::from_toml(
            r#"
[accounts.claude-a]
kind = "claude"
vendor = "claude"
subscription_broker = "cliproxyapi"

[providers.alpha]
base_url = "http://127.0.0.1:4010"
protocol = "openai-chat"
credential_env = ["ALPHA_KEY"]
"#,
        )
        .expect("the gateway's own shape parses");

        assert_eq!(catalogue.account_names().collect::<Vec<_>>(), ["claude-a"]);
        assert_eq!(
            catalogue.account("claude-a").and_then(AccountEntry::kind),
            Some(crate::config::EntitlementKind::Claude)
        );
        assert_eq!(
            catalogue.configured_provider_names().collect::<Vec<_>>(),
            ["alpha"]
        );
        assert!(catalogue.has_provider("alpha"));
        assert!(
            catalogue.has_provider("openrouter"),
            "a built-in template is a provider an overlay may attach to"
        );
        assert!(!catalogue.has_provider("nothing-named-this"));

        let alpha = catalogue.provider("alpha").expect("resolves");
        assert_eq!(alpha.protocols[0].base_url, "http://127.0.0.1:4010");

        let empty = GatewayCatalogue::empty();
        assert_eq!(empty.account_names().count(), 0);
        assert_eq!(empty.configured_provider_names().count(), 0);
    }

    /// A missing file is an empty catalogue that still remembers where it
    /// looked — the first-run case, not a failure.
    #[test]
    fn a_missing_gateway_config_is_an_empty_catalogue_that_names_its_path() {
        let scratch = tempfile::tempdir().expect("a scratch directory");
        let path = scratch.path().join("gateway.toml");
        let catalogue = GatewayCatalogue::load(&path).expect("a missing file is not an error");
        assert!(!catalogue.present());
        assert_eq!(catalogue.account_names().count(), 0);
        assert_eq!(catalogue.path(), Some(path.as_path()));
    }
}
