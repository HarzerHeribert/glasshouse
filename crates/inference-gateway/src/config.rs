//! The standalone gateway's own configuration file: which accounts exist,
//! and which providers they name.
//!
//! Two tables, and the split between them is the same one the library draws.
//! `[accounts.<name>]` deserialises straight into
//! [`crate::entitlement::AccountEntry`] — the *catalogue* half, six keys,
//! `deny_unknown_fields`, and a credential that is a **reference and never a
//! value**. `[providers.<name>]` is the destination half: a base URL per
//! protocol, the environment variable names a key may come from, and any
//! extra headers. Neither table may hold a secret; the reference in an
//! account is resolved through [`crate::secret::SecretStore`] at the moment
//! of use and never earlier.
//!
//! **A missing file is an empty catalogue, not an error.** A gateway with
//! nothing configured is a gateway that will refuse to serve for a reason it
//! can name, which is a better first run than a parse error about a file the
//! user has never opened.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::entitlement::{AccountEntry, deserialize_credential_env_names};
use crate::gateway::subscription_broker::BrokerPaths;
use crate::provider::{ProtocolSupport, Provider, unverified_support};
use crate::routing::wire::WireProtocol;

/// The qualifier/organisation/application triple every platform location is
/// derived from. Empty qualifier and organisation, so the layout is
/// `~/.config/inference-gateway` on Linux and
/// `~/Library/Application Support/inference-gateway` on macOS.
const APPLICATION: &str = "inference-gateway";

/// The configuration file's name inside the platform configuration
/// directory.
const CONFIG_FILE: &str = "gateway.toml";

/// The protocol a `[providers.<name>]` entry serves when it names none.
///
/// Anthropic Messages, because that is the ingress Pane points at: it sets
/// `ANTHROPIC_BASE_URL` to this gateway and its client sends
/// `POST /v1/messages`. A default of anything else would make the common
/// configuration the one that has to say the most.
const DEFAULT_PROTOCOL: WireProtocol = WireProtocol::AnthropicMessages;

/// The whole of `gateway.toml`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatewayConfig {
    /// `[accounts.<name>]` — the catalogue this gateway serves from.
    #[serde(default)]
    pub accounts: BTreeMap<String, AccountEntry>,
    /// `[providers.<name>]` — destinations an account may name. A name that
    /// matches a built-in template overrides it.
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderEntry>,
}

/// One `[providers.<name>]` table: where requests for this provider go.
///
/// `base_url` with an optional `protocol` is the shorthand for the single
/// protocol case; `protocols` is the table form for a provider that serves
/// more than one. Both may be present, and the shorthand is merged in first,
/// so the ingress order is the shorthand's protocol followed by the rest in
/// slug order.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderEntry {
    /// The single protocol's base URL. Absent when `protocols` says it all.
    #[serde(default)]
    pub base_url: Option<String>,
    /// Which protocol `base_url` serves. Defaults to `anthropic-messages`.
    #[serde(default)]
    pub protocol: Option<String>,
    /// Protocol slug to base URL, for a provider serving several.
    #[serde(default)]
    pub protocols: BTreeMap<String, String>,
    /// Environment variable **names** a credential for this provider may
    /// come from. Deserialised through the catalogue's own shape check, so a
    /// key pasted where a name belongs is refused without being echoed.
    #[serde(default, deserialize_with = "deserialize_credential_env_names")]
    pub credential_env: Vec<String>,
    /// Extra request headers this provider needs. Configuration, not
    /// credentials — a header value here is written by the user and is not
    /// resolved through a secret store.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

/// What a load produced, and where it came from.
///
/// `path` is `None` only when no platform configuration directory could be
/// determined at all. `present` is false when the file simply is not there,
/// which is the empty-catalogue case and not a failure.
pub struct Loaded {
    pub config: GatewayConfig,
    pub path: Option<PathBuf>,
    pub present: bool,
}

impl Loaded {
    /// The one line a caller prints to stderr about where its configuration
    /// came from. Never printed on stdout: `serve`'s stdout carries exactly
    /// one line and it is the ready line.
    pub fn note(&self) -> String {
        match (&self.path, self.present) {
            (Some(path), true) => format!(
                "configuration: {} ({} account(s), {} provider(s))",
                path.display(),
                self.config.accounts.len(),
                self.config.providers.len()
            ),
            (Some(path), false) => format!(
                "configuration: {} does not exist; starting with an empty catalogue",
                path.display()
            ),
            (None, _) => "configuration: no platform configuration directory could be \
                          determined; starting with an empty catalogue"
                .to_owned(),
        }
    }
}

/// Parses one configuration text. **The refusal carries toml's message and
/// never its source excerpt**: a credential pasted as a value is refused by
/// `deserialize_credential` without being echoed, and toml's default
/// rendering would print the offending line back — which is the one thing
/// the refusal exists to avoid.
pub fn parse(text: &str) -> Result<GatewayConfig> {
    toml::from_str(text).map_err(|error: toml::de::Error| anyhow::anyhow!("{}", error.message()))
}

/// Read the configuration from `explicit`, or from the platform location.
///
/// A file that is not there yields an empty catalogue. A file that is there
/// and will not parse is an error: the user wrote it, and silently ignoring
/// what they wrote is how a gateway ends up serving from a catalogue nobody
/// intended.
pub fn load(explicit: Option<&Path>) -> Result<Loaded> {
    let path = match explicit {
        Some(path) => Some(path.to_path_buf()),
        None => default_config_path(),
    };
    let Some(path) = path else {
        return Ok(Loaded {
            config: GatewayConfig::default(),
            path: None,
            present: false,
        });
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let config = parse(&text)
                .with_context(|| format!("could not read the gateway configuration {path:?}"))?;
            Ok(Loaded {
                config,
                path: Some(path),
                present: true,
            })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Loaded {
            config: GatewayConfig::default(),
            path: Some(path),
            present: false,
        }),
        Err(error) => {
            Err(error).with_context(|| format!("could not read the gateway configuration {path:?}"))
        }
    }
}

/// `<platform config dir>/gateway.toml`, or `None` when no such directory
/// can be determined.
/// `INFERENCE_GATEWAY_CONFIG` names the file outright — the override a
/// caller that spawns this binary without passing `--config` (pane) and a
/// test that must not touch the user's own catalogue both need.
pub fn default_config_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("INFERENCE_GATEWAY_CONFIG") {
        return Some(PathBuf::from(path));
    }
    directories::ProjectDirs::from("", "", APPLICATION)
        .map(|dirs| dirs.config_dir().join(CONFIG_FILE))
}

/// The private state root brokers, auth directories and caches hang off.
/// `INFERENCE_GATEWAY_DATA_DIR` overrides the platform location, for the
/// same two callers as [`default_config_path`].
pub fn default_data_dir() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("INFERENCE_GATEWAY_DATA_DIR") {
        return Some(PathBuf::from(path));
    }
    directories::ProjectDirs::from("", "", APPLICATION).map(|dirs| dirs.data_dir().to_path_buf())
}

/// The four directories one entitlement's subscription broker needs,
/// derived from `data_dir`.
///
/// Laid out exactly as the host lays them out, hex-encoding the entitlement
/// name: that preserves identity without putting a user-controlled path
/// separator into a filesystem path, and it means a host and a standalone
/// gateway pointed at the same data directory find the same login.
pub fn broker_paths(data_dir: &Path, entitlement: &str) -> BrokerPaths {
    let brokers_dir = data_dir.join("subscription-brokers");
    let entitlement_dir = brokers_dir.join(format!(
        "entitlement-{}",
        hex::encode(entitlement.as_bytes())
    ));
    let auth_dir = entitlement_dir.join("auth");
    BrokerPaths {
        brokers_dir,
        entitlement_dir,
        auth_dir,
        executable: cliproxyapi_executable(data_dir),
    }
}

/// The stable OAuth directory for one entitlement — the half of
/// [`broker_paths`] that a login writes and a status read looks at.
pub fn broker_auth_dir(data_dir: &Path, entitlement: &str) -> PathBuf {
    broker_paths(data_dir, entitlement).auth_dir
}

/// The managed CLIProxyAPI executable, unless `GLASSHOUSE_CLIPROXYAPI_BIN`
/// names one — which the broker itself checks, so this is only the fallback.
fn cliproxyapi_executable(data_dir: &Path) -> PathBuf {
    let name = if cfg!(windows) {
        "CLIProxyAPI.exe"
    } else {
        "CLIProxyAPI"
    };
    data_dir.join("tools").join(name)
}

/// Where a model catalogue read back by `entitlements --json` is cached.
pub fn model_cache_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("model-catalogues")
}

/// Every provider an account may name: the configured entries first, then
/// the built-in templates a configured entry did not override.
///
/// Configured first because an entry naming a template's name is an
/// override, and an override that lost to the thing it overrides would be
/// the opposite of what was written.
pub fn providers(config: &GatewayConfig) -> Vec<Provider> {
    let mut out: Vec<Provider> = config
        .providers
        .iter()
        .map(|(name, entry)| entry.to_provider(name))
        .collect();
    // The provider a standalone gateway is most often pointed at with no
    // catalogue written yet: Anthropic's own API through `ANTHROPIC_API_KEY`.
    // A configured `[providers.anthropic]` replaces it; the shared templates
    // below stay what every host ships.
    if !out.iter().any(|provider| provider.name == "anthropic") {
        out.push(
            ProviderEntry {
                base_url: Some("https://api.anthropic.com".to_owned()),
                protocol: Some("anthropic-messages".to_owned()),
                protocols: BTreeMap::new(),
                credential_env: vec!["ANTHROPIC_API_KEY".to_owned()],
                headers: Default::default(),
            }
            .to_provider("anthropic"),
        );
    }
    for template in crate::provider::templates() {
        if !out.iter().any(|provider| provider.name == template.name) {
            out.push(template);
        }
    }
    out
}

impl ProviderEntry {
    /// This entry as the catalogue type the pool builds routes from.
    ///
    /// Everything is `Declared::Unverified`: nothing probed this provider,
    /// and recording a capability nobody checked is the one thing the
    /// provider model refuses.
    pub fn to_provider(&self, name: &str) -> Provider {
        let mut protocols: Vec<ProtocolSupport> = Vec::new();
        if let Some(base_url) = &self.base_url {
            let protocol = self
                .protocol
                .as_deref()
                .and_then(protocol_from_slug)
                .unwrap_or(DEFAULT_PROTOCOL);
            protocols.push(unverified_support(protocol, base_url));
        }
        for (slug, base_url) in &self.protocols {
            let Some(protocol) = protocol_from_slug(slug) else {
                continue;
            };
            if protocols.iter().any(|s| s.protocol == protocol) {
                continue;
            }
            protocols.push(unverified_support(protocol, base_url));
        }
        Provider {
            name: name.to_owned(),
            protocols,
            model_list_endpoint: crate::routing::wire::Declared::Unverified,
            usage_telemetry: crate::routing::wire::Declared::Unverified,
            credential_env: self.credential_env.clone(),
            headers: self
                .headers
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        }
    }
}

/// A protocol slug back to its [`WireProtocol`], or `None`.
///
/// The inverse of [`WireProtocol::slug`], written out rather than derived so
/// that a slug this build does not know is `None` — an entry skipped — and
/// never a protocol guessed from a neighbouring spelling.
pub fn protocol_from_slug(slug: &str) -> Option<WireProtocol> {
    match slug {
        "anthropic-messages" => Some(WireProtocol::AnthropicMessages),
        "openai-responses" => Some(WireProtocol::OpenAiResponses),
        "openai-chat" => Some(WireProtocol::OpenAiChat),
        "gemini-generate-content" => Some(WireProtocol::GeminiGenerateContent),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    /// With no configuration at all, Anthropic's API is a provider through
    /// `ANTHROPIC_API_KEY`; a configured `[providers.anthropic]` replaces it.
    #[test]
    fn anthropics_api_is_a_provider_out_of_the_box_and_a_configured_one_replaces_it() {
        let bare = providers(&GatewayConfig::default());
        let anthropic = bare
            .iter()
            .find(|provider| provider.name == "anthropic")
            .expect("anthropic is a provider with no configuration");
        assert_eq!(anthropic.protocols[0].base_url, "https://api.anthropic.com");
        assert_eq!(
            anthropic.credential_env,
            vec!["ANTHROPIC_API_KEY".to_owned()]
        );

        let config: GatewayConfig = toml::from_str(
            r#"
[providers.anthropic]
base_url = "http://127.0.0.1:4321"
protocol = "anthropic-messages"
credential_env = ["MY_KEY"]
"#,
        )
        .expect("parses");
        let configured = providers(&config);
        let anthropic: Vec<_> = configured
            .iter()
            .filter(|provider| provider.name == "anthropic")
            .collect();
        assert_eq!(
            anthropic.len(),
            1,
            "one anthropic provider, the configured one"
        );
        assert_eq!(anthropic[0].protocols[0].base_url, "http://127.0.0.1:4321");
    }

    use super::*;

    /// The two tables parse, and an account's credential arrives as a
    /// reference.
    #[test]
    fn both_tables_parse() {
        let config: GatewayConfig = toml::from_str(
            r#"
[providers.fake]
base_url = "http://127.0.0.1:1234"
credential_env = ["FAKE_KEY"]

[accounts.work]
kind = "claude"
provider = "fake"
credential = { env = "FAKE_KEY" }
"#,
        )
        .expect("the documented shape parses");
        assert_eq!(config.accounts.len(), 1);
        let provider = config.providers["fake"].to_provider("fake");
        assert_eq!(provider.protocols.len(), 1);
        assert_eq!(provider.protocols[0].protocol, DEFAULT_PROTOCOL);
        assert_eq!(provider.protocols[0].base_url, "http://127.0.0.1:1234");
        assert!(
            config.accounts["work"].credential().is_some(),
            "a credential reference survives the round trip"
        );
    }

    /// A credential written as a value rather than a reference is refused,
    /// and the refusal does not repeat what was written.
    #[test]
    fn a_pasted_credential_is_refused_without_being_echoed() {
        let error = parse(
            r#"
[accounts.work]
credential = "sk-ant-notarealkey-000"
"#,
        )
        .expect_err("a bare string is not a reference");
        let rendered = error.to_string();
        assert!(!rendered.contains("sk-ant-notarealkey-000"), "{rendered}");
        assert!(rendered.contains("never a value"), "{rendered}");
    }

    /// A path that does not exist is an empty catalogue, not a crash.
    #[test]
    fn a_missing_config_is_an_empty_catalogue() {
        let loaded = load(Some(Path::new("/nonexistent/gateway.toml")))
            .expect("a missing file is not an error");
        assert!(loaded.config.accounts.is_empty());
        assert!(!loaded.present);
        assert!(
            loaded.note().contains("does not exist"),
            "{}",
            loaded.note()
        );
    }

    /// A configured provider overrides the built-in template of the same
    /// name rather than sitting behind it.
    #[test]
    fn a_configured_provider_overrides_its_template() {
        let config: GatewayConfig = toml::from_str(
            r#"
[providers.openrouter]
base_url = "http://127.0.0.1:1"
protocol = "openai-chat"
"#,
        )
        .expect("parses");
        let providers = providers(&config);
        let openrouter: Vec<_> = providers
            .iter()
            .filter(|p| p.name == "openrouter")
            .collect();
        assert_eq!(openrouter.len(), 1, "one entry, not two");
        assert_eq!(openrouter[0].protocols[0].base_url, "http://127.0.0.1:1");
    }
}
