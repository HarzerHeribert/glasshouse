//! User-level and optional project-level Glasshouse configuration.
//!
//! Two files, same small shape:
//!
//! - `<config_dir>/config.toml` — user-level. Onboarding decisions and
//!   per-integration enable/executable overrides. Loaded by every run;
//!   never created automatically for you to lose data to — a missing file
//!   just means the defaults apply (see [`load`]).
//! - `<project root>/.glasshouse/config.toml` — project-level, optional,
//!   and layered *over* the user file (see [`EffectiveConfig`]). It is
//!   never written except in response to an explicit user decision — see
//!   [`write_project_config_with_consent`].
//!
//! The schema is deliberately tiny. The capability map is explicit that
//! configuration should stay small until real usage demonstrates a need for
//! more (Phase 49): no provider, routing, or budget fields belong here yet —
//! those are later phases and would be speculative today. Phase 9A's launch
//! profiles are the one addition ahead of that: [`ProfileTable`] holds
//! *inert* profile configuration (which harness, which backend resource,
//! which approval mode) — never a resolved overlay, never a credential, and
//! never the project's own memory. Resolving a stored profile into something
//! that can actually launch a harness happens in [`crate::profile`], not
//! here.
//!
//! ## No secrets here — structurally, not just by convention
//!
//! [`IntegrationConfig`], [`ProfileConfig`] and [`ProviderConfig`], the
//! per-item shapes either file stores, hold onboarding decisions, executable
//! overrides, inert profile selections and *names* — never an API key,
//! token, or any other credential. That is Phase 9E's rule applied here:
//! "Never write API keys into tracked `.glasshouse` project files" and
//! "Store only secret references in provider configuration whenever
//! possible." A [`ProfileConfig::backend`] naming
//! [`ProfileBackend::DirectProvider`] carries only the provider's own
//! *name*; a [`ProviderConfig::credential_store`] carries a
//! [`StoredCredentialRef`], which is two names. Resolving any of them to a
//! credential is the separate `SecretStore` abstraction's job (not built by
//! this module), never this one's. See
//! [`tests::serialized_form_has_no_secret_capable_field`] for a structural
//! guard, not just a string search.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::integrations::IntegrationId;
use crate::paths::RuntimePaths;
use crate::project::{Project, ScopeError};

/// Configuration schema version this build of Glasshouse writes and fully
/// understands. Bump this only when the schema changes in a way that
/// matters for [`UserConfig::save`]'s forward-compatibility check below.
const CURRENT_SCHEMA_VERSION: u32 = 1;

fn current_schema_version() -> u32 {
    CURRENT_SCHEMA_VERSION
}

/// Relative path of the optional project-level configuration file, inside
/// the project root.
const PROJECT_CONFIG_RELATIVE_PATH: &str = ".glasshouse/config.toml";

/// Errors from loading or saving Glasshouse configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not read configuration file `{path}`: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// The file exists but is not valid TOML, or its shape does not match
    /// what this build expects. Deliberately never followed by a write:
    /// overwriting a file we could not parse would destroy whatever the
    /// user actually has on disk.
    #[error("configuration file `{path}` is not valid TOML: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: Box<toml::de::Error>,
    },

    #[error("could not create configuration directory `{path}`: {source}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("could not write configuration file `{path}`: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("could not serialize configuration for `{path}`: {source}")]
    Serialize {
        path: PathBuf,
        #[source]
        source: Box<toml::ser::Error>,
    },

    /// The file's `version` is newer than this build understands. Reading it
    /// (see [`load`] / [`load_project_config`]) still succeeds — refusing to
    /// even parse a file some other Glasshouse install wrote would be an
    /// unnecessary hostility. Only *writing* is refused, because this build
    /// cannot know what the newer fields mean and would otherwise silently
    /// drop them.
    #[error(
        "configuration file `{path}` was written by a newer version of Glasshouse (schema version {found}, this build understands up to {supported}); refusing to overwrite it. The file can still be read; upgrade Glasshouse to write it again."
    )]
    UnsupportedVersion {
        path: PathBuf,
        found: u32,
        supported: u32,
    },

    /// The project-level configuration path did not resolve inside the
    /// project root. See [`load_project_config`] and
    /// [`write_project_config_with_consent`] for why this can never
    /// actually point outside the project.
    #[error("project configuration path could not be resolved inside the project root: {0}")]
    Scope(#[from] ScopeError),
}

/// Per-integration configuration: whether the user turned it on, and an
/// optional explicit executable path.
///
/// `enabled` is genuinely tri-state per field: `None` means the user has
/// never recorded a decision (the key is absent), while `Some(_)` records
/// an explicit enable or disable. This distinction matters for layering —
/// see [`IntegrationTable::is_enabled`] and [`EffectiveConfig::enabled`].
///
/// Deliberately has no other fields — see the module-level "No secrets
/// here" section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntegrationConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    executable: Option<PathBuf>,
    /// Consent to write this harness's lifecycle hooks *inside the user's
    /// own project*, for a harness whose only hook mechanism reads from
    /// there (Codex's `.codex/hooks.json`; see
    /// [`crate::harness::HookDestination::ProjectLocal`]). `None` means the
    /// user has never been asked, which must be treated as consent withheld,
    /// never as consent granted.
    ///
    /// `Option<bool>` for the same reason `enabled` is: a plain `bool` here
    /// would repeat the exact defect `enabled` already caused once — a
    /// project file that overrides only one of these two fields would parse
    /// the other as its type's default rather than "not recorded", and
    /// `false` silently winning is precisely the wrong default for consent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project_hooks: Option<bool>,
    /// Acknowledgement that this harness's blanket approval bypass has been
    /// shown to and accepted by the person running Glasshouse on this
    /// machine — see [`EffectiveConfig::bypass_acknowledged`] for why this
    /// field is read from the user layer only, never the project layer.
    /// `None` means never asked, which must be treated as not acknowledged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bypass_acknowledged: Option<bool>,
}

impl IntegrationConfig {
    pub fn enabled(&self) -> Option<bool> {
        self.enabled
    }

    /// The recorded decision, or `default` when none was ever recorded.
    pub fn enabled_or(&self, default: bool) -> bool {
        self.enabled.unwrap_or(default)
    }

    pub fn executable(&self) -> Option<&Path> {
        self.executable.as_deref()
    }

    /// Whether the user has consented to project-local lifecycle hooks for
    /// this harness. `None` means never asked — see the field's own doc.
    pub fn project_hooks(&self) -> Option<bool> {
        self.project_hooks
    }

    /// The recorded consent decision, or `default` when none was ever
    /// recorded. Callers resolving this for real use must pass `false`: an
    /// unrecorded decision is withheld consent, not granted consent.
    pub fn project_hooks_or(&self, default: bool) -> bool {
        self.project_hooks.unwrap_or(default)
    }

    pub fn set_enabled(&mut self, enabled: bool) -> &mut Self {
        self.enabled = Some(enabled);
        self
    }

    pub fn set_executable(&mut self, executable: Option<PathBuf>) -> &mut Self {
        self.executable = executable;
        self
    }

    pub fn set_project_hooks(&mut self, consent: bool) -> &mut Self {
        self.project_hooks = Some(consent);
        self
    }

    /// Whether the user has acknowledged this harness's blanket approval
    /// bypass. `None` means never asked — see the field's own doc.
    pub fn bypass_acknowledged(&self) -> Option<bool> {
        self.bypass_acknowledged
    }

    pub fn set_bypass_acknowledged(&mut self, acknowledged: bool) -> &mut Self {
        self.bypass_acknowledged = Some(acknowledged);
        self
    }
}

/// A map of per-integration configuration, keyed by [`IntegrationId::slug`].
///
/// A `BTreeMap<String, _>` rather than an `IntegrationId`-keyed map so that
/// a slug this build does not recognize — written by a newer Glasshouse —
/// round-trips through load/save instead of failing to parse, and so the
/// serialized order is deterministic (stable diffs, easy manual review).
/// `#[serde(transparent)]` makes this behave exactly like the bare map for
/// (de)serialization, so the TOML shape stays the plain
/// `[integrations.claude-code]` form shown in the module docs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IntegrationTable(BTreeMap<String, IntegrationConfig>);

impl IntegrationTable {
    /// The recorded configuration for `id`, if the user has ever recorded
    /// one. `None` here is a real state, not "disabled" — see
    /// [`IntegrationTable::is_enabled`].
    pub fn get(&self, id: IntegrationId) -> Option<&IntegrationConfig> {
        self.0.get(id.slug())
    }

    /// Mutable access, creating a default (no recorded decision, no explicit
    /// executable) entry if `id` has no recorded configuration yet.
    pub fn entry(&mut self, id: IntegrationId) -> &mut IntegrationConfig {
        self.0.entry(id.slug().to_owned()).or_default()
    }

    pub fn set(&mut self, id: IntegrationId, config: IntegrationConfig) {
        self.0.insert(id.slug().to_owned(), config);
    }

    pub fn remove(&mut self, id: IntegrationId) -> Option<IntegrationConfig> {
        self.0.remove(id.slug())
    }

    /// Tri-state: `Some(true)`/`Some(false)` is an explicit user decision,
    /// `None` means the user has never been asked about `id` (including the
    /// case where an entry exists but records only, say, an executable
    /// override). Onboarding needs exactly this distinction to know whether
    /// to prompt.
    pub fn is_enabled(&self, id: IntegrationId) -> Option<bool> {
        self.get(id).and_then(IntegrationConfig::enabled)
    }

    /// Like [`IntegrationTable::is_enabled`], collapsing the never-asked
    /// case to a caller-supplied default instead of an `Option`.
    pub fn is_enabled_or_default(&self, id: IntegrationId, default: bool) -> bool {
        self.is_enabled(id).unwrap_or(default)
    }

    /// Every recorded entry, keyed by its raw slug (including slugs this
    /// build does not recognize).
    pub fn iter(&self) -> impl Iterator<Item = (&str, &IntegrationConfig)> {
        self.0.iter().map(|(slug, cfg)| (slug.as_str(), cfg))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Serializable form of [`crate::profile::BackendResource`].
///
/// Kept as its own type here, rather than deriving `Serialize`/`Deserialize`
/// directly on the domain type in [`crate::profile`], because that module is
/// deliberately free of any dependency on this crate's configuration or
/// serialization shape — a launch profile is inert configuration only once
/// it has been read *into* `crate::profile::LaunchProfile`; how it is spelled
/// in TOML is this module's concern alone.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ProfileBackend {
    #[default]
    Native,
    DirectProvider {
        provider: String,
    },
    GlasshouseGateway,
}

/// Serializable form of [`crate::profile::ApprovalSelection`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProfileApproval {
    #[default]
    Default,
    AutomaticReview,
    Bypass,
}

/// One configured launch profile, as stored in a `[profiles.<name>]` table.
///
/// The profile's *name* is its key in [`ProfileTable`], not a field here —
/// the same relationship [`IntegrationConfig`] has to its slug in
/// [`IntegrationTable`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileConfig {
    /// The harness this profile applies to, as an
    /// [`IntegrationId::slug`].
    harness: String,
    #[serde(default, skip_serializing_if = "is_native_backend")]
    backend: ProfileBackend,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    /// The expected wire protocol, as a [`crate::harness::WireProtocol`]
    /// slug (`"anthropic-messages"`, `"openai-responses"`, or
    /// `"openai-chat"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expected_protocol: Option<String>,
    #[serde(default, skip_serializing_if = "is_default_approval")]
    approval: ProfileApproval,
    /// Whether this launch profile is currently enabled. Disabling is not
    /// removal — see [`ProfileConfig::set_enabled`] and the "disable is not
    /// delete" rule Phase 2D's Settings behavioural contract requires: every
    /// other field stays exactly as configured, and re-enabling needs no
    /// retyping.
    #[serde(
        default = "enabled_by_default",
        skip_serializing_if = "is_enabled_by_default"
    )]
    enabled: bool,
}

fn is_native_backend(backend: &ProfileBackend) -> bool {
    matches!(backend, ProfileBackend::Native)
}

fn is_default_approval(approval: &ProfileApproval) -> bool {
    matches!(approval, ProfileApproval::Default)
}

/// The default for [`ProviderConfig::enabled`] and [`ProfileConfig::enabled`]
/// — a config file written before either field existed still loads every
/// entry as enabled.
fn enabled_by_default() -> bool {
    true
}

fn is_enabled_by_default(enabled: &bool) -> bool {
    *enabled
}

/// Why a stored [`ProfileConfig`] could not be turned into a
/// [`crate::profile::LaunchProfile`].
#[derive(Debug, thiserror::Error)]
pub enum ProfileConfigError {
    #[error(
        "launch profile `{name}` names harness `{harness}`, which Glasshouse does not know; \
         fix or remove the profile's `harness` key"
    )]
    UnknownHarness { name: String, harness: String },
    #[error(
        "launch profile `{name}` names protocol `{protocol}`, which Glasshouse does not know; \
         fix or remove the profile's `expected_protocol` key"
    )]
    UnknownProtocol { name: String, protocol: String },
}

impl ProfileConfig {
    pub fn new(harness: IntegrationId) -> Self {
        Self {
            harness: harness.slug().to_owned(),
            backend: ProfileBackend::default(),
            model: None,
            expected_protocol: None,
            approval: ProfileApproval::default(),
            enabled: true,
        }
    }

    pub fn harness_slug(&self) -> &str {
        &self.harness
    }

    pub fn backend(&self) -> &ProfileBackend {
        &self.backend
    }

    pub fn set_backend(&mut self, backend: ProfileBackend) -> &mut Self {
        self.backend = backend;
        self
    }

    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    pub fn set_model(&mut self, model: Option<String>) -> &mut Self {
        self.model = model;
        self
    }

    pub fn expected_protocol(&self) -> Option<&str> {
        self.expected_protocol.as_deref()
    }

    pub fn set_expected_protocol(&mut self, protocol: Option<String>) -> &mut Self {
        self.expected_protocol = protocol;
        self
    }

    pub fn approval(&self) -> ProfileApproval {
        self.approval
    }

    pub fn set_approval(&mut self, approval: ProfileApproval) -> &mut Self {
        self.approval = approval;
        self
    }

    /// Whether this launch profile is currently enabled. `true` for a
    /// profile no one has ever disabled, including one loaded from a file
    /// written before this field existed.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Disabling keeps every other field untouched and is fully reversible —
    /// see the field's own doc comment.
    pub fn set_enabled(&mut self, enabled: bool) -> &mut Self {
        self.enabled = enabled;
        self
    }

    /// Turn this stored configuration into the resolvable domain type,
    /// naming it `name` — the key this entry was stored under.
    pub fn to_launch_profile(
        &self,
        name: &str,
    ) -> Result<crate::profile::LaunchProfile, ProfileConfigError> {
        let harness = IntegrationId::ALL
            .iter()
            .copied()
            .find(|id| id.slug() == self.harness)
            .ok_or_else(|| ProfileConfigError::UnknownHarness {
                name: name.to_owned(),
                harness: self.harness.clone(),
            })?;

        let expected_protocol = self
            .expected_protocol
            .as_deref()
            .map(|slug| {
                wire_protocol_from_slug(slug).ok_or_else(|| ProfileConfigError::UnknownProtocol {
                    name: name.to_owned(),
                    protocol: slug.to_owned(),
                })
            })
            .transpose()?;

        let backend = match &self.backend {
            ProfileBackend::Native => crate::profile::BackendResource::Native,
            ProfileBackend::DirectProvider { provider } => {
                crate::profile::BackendResource::DirectProvider {
                    provider: provider.clone(),
                }
            }
            ProfileBackend::GlasshouseGateway => crate::profile::BackendResource::GlasshouseGateway,
        };

        let approval = match self.approval {
            ProfileApproval::Default => crate::profile::ApprovalSelection::Default,
            ProfileApproval::AutomaticReview => crate::profile::ApprovalSelection::AutomaticReview,
            ProfileApproval::Bypass => crate::profile::ApprovalSelection::Bypass,
        };

        Ok(crate::profile::LaunchProfile {
            name: name.to_owned(),
            harness,
            backend,
            model: self.model.clone(),
            expected_protocol,
            approval,
        })
    }
}

/// The reverse of [`crate::harness::WireProtocol::slug`]. Kept here, rather
/// than as a method on that type, because `crate::harness` is the settled
/// adapter contract (see its module doc) and parsing a *configuration*
/// string is this module's concern, not an adapter's.
fn wire_protocol_from_slug(slug: &str) -> Option<crate::harness::WireProtocol> {
    use crate::harness::WireProtocol;
    match slug {
        "anthropic-messages" => Some(WireProtocol::AnthropicMessages),
        "openai-responses" => Some(WireProtocol::OpenAiResponses),
        "openai-chat" => Some(WireProtocol::OpenAiChat),
        _ => None,
    }
}

/// One configured provider, as stored in a `[providers.<name>]` table.
///
/// The provider's *name* is its key in [`ProviderTable`], not a field here —
/// the same relationship [`ProfileConfig`] has to its name in [`ProfileTable`].
///
/// Deliberately holds only a template slug, an optional base-URL override,
/// and credential variable *names* — see the module-level "No secrets here"
/// section. [`ProviderConfig::to_provider`] is the only thing that turns this
/// into a [`crate::provider::Provider`], and it never reads an environment
/// variable's value while doing so.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// The built-in template this provider is based on — a
    /// [`crate::provider::Provider::name`] from [`crate::provider::templates`].
    /// Required: the two generic templates (`openai-compatible`,
    /// `anthropic-compatible`) are exactly how a fully custom provider gets
    /// configured, so there is no separate template-less shape to support.
    template: String,
    /// Override for the template's base URL. Required, in practice, for the
    /// two generic templates — their own base URL is empty because it is
    /// user-supplied — and optional for the rest, where it lets a configured
    /// provider point at a mirror or self-hosted instance of a known router
    /// (line 423).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    base_url: Option<String>,
    /// Environment variable names this provider's credential may come from.
    /// **Names only — never a value.** Non-empty here replaces the
    /// template's own default credential names entirely, which is what lets
    /// a user hold several keys for the same router.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    credential_env: Vec<String>,
    /// Where this provider's credential is kept, when the user has put it in
    /// the operating system's own secure store. **A reference — a service
    /// and an account name — never a value.**
    ///
    /// This is the serialised shape of
    /// [`crate::secret::SecretRef::OsCredential`], and it is exactly what
    /// Phase 9E's "store only secret references in provider configuration"
    /// means: the two names here are as safe to write into a tracked project
    /// file as [`ProviderConfig::credential_env`]'s variable names already
    /// are.
    ///
    /// # It records intent; it is not what makes resolution work
    ///
    /// [`crate::secret::native::PreferNativeSecretStore`] finds a stored
    /// credential by the variable name a harness expects it in, whether or
    /// not this field was ever saved. So a configuration file that has
    /// drifted out of step with the keychain — a credential deleted with
    /// this field still written, or the reverse — cannot cause a wrong
    /// launch; the store is asked at the moment of use either way. What this
    /// field is for is telling the *user* where their key is, and giving
    /// deletion something to remove.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    credential_store: Option<StoredCredentialRef>,
    /// Extra HTTP headers this provider needs, as name/value pairs — see
    /// [`crate::provider::Provider::headers`]. Configuration, not a
    /// credential: a header value here is written by the user into their own
    /// config file and never resolved through `SecretStore`. Non-empty here
    /// replaces the template's own headers entirely, matching
    /// [`ProviderConfig::credential_env`]'s own replace-not-merge rule.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    headers: Vec<(String, String)>,
    /// Whether this provider is currently enabled. Disabling is not
    /// removal — see [`ProviderConfig::set_enabled`] and the "disable is not
    /// delete" rule Phase 2D's Settings behavioural contract requires: every
    /// other field stays exactly as configured, and re-enabling needs no
    /// retyping. Deciding whether routing may actually use a disabled
    /// provider is a later phase's job; [`ProviderConfig::to_provider`]
    /// never consults this field.
    #[serde(
        default = "enabled_by_default",
        skip_serializing_if = "is_enabled_by_default"
    )]
    enabled: bool,
}

/// Why a stored [`ProviderConfig`] could not be turned into a
/// [`crate::provider::Provider`].
#[derive(Debug, thiserror::Error)]
pub enum ProviderConfigError {
    #[error(
        "provider `{name}` names template `{template}`, which Glasshouse does not know; fix \
         the provider's `template` key or remove the entry"
    )]
    UnknownTemplate { name: String, template: String },

    /// A header *name* would reach an `ANTHROPIC_CUSTOM_HEADERS` line or a
    /// Codex `-c model_providers.<id>.http_headers=…` TOML literal carrying
    /// a character neither would parse as part of the name itself.
    #[error(
        "provider `{name}` names a header {header_name:?} that contains {offending:?}; a \
         header name may use letters, digits and `-` only"
    )]
    InvalidHeaderName {
        name: String,
        header_name: String,
        offending: char,
    },

    /// A header *value* containing a control character — most importantly
    /// `\r` or `\n`, which would let a header value inject a second header of
    /// its own choosing into every request this provider's child process
    /// sends. Refused, never escaped.
    #[error(
        "provider `{name}`'s header {header_name:?} has a value that contains {offending:?}, a \
         control character; a header value must not contain one"
    )]
    InvalidHeaderValue {
        name: String,
        header_name: String,
        offending: char,
    },
}

/// The on-disk shape of a [`crate::secret::SecretRef::OsCredential`].
///
/// Two names and nothing else, which is why it may be serialized at all —
/// see [`ProviderConfig::credential_store`]. It lives here rather than in
/// [`mod@crate::secret`] on purpose: that module's own tests forbid the word
/// `Serialize` anywhere in its production code, so the shape a *reference*
/// takes on disk is a configuration-schema decision and the value handling
/// stays somewhere that can never be serialized. The two halves cannot
/// drift, because [`StoredCredentialRef::to_secret_ref`] is the only bridge
/// between them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredCredentialRef {
    service: String,
    account: String,
}

impl StoredCredentialRef {
    pub fn new(service: impl Into<String>, account: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            account: account.into(),
        }
    }

    pub fn service(&self) -> &str {
        &self.service
    }

    pub fn account(&self) -> &str {
        &self.account
    }

    /// The reference a [`crate::secret::SecretStore`] can be asked with.
    pub fn to_secret_ref(&self) -> crate::secret::SecretRef {
        crate::secret::SecretRef::OsCredential {
            service: self.service.clone(),
            account: self.account.clone(),
        }
    }

    /// The stored shape of `reference`, or `None` for one that names no OS
    /// credential.
    ///
    /// `None` rather than an invented service name: an
    /// [`crate::secret::SecretRef::Environment`] reference is not a stored
    /// credential, and writing one here as though it were would claim
    /// something about where a key lives that nobody established.
    pub fn from_secret_ref(reference: &crate::secret::SecretRef) -> Option<Self> {
        match reference {
            crate::secret::SecretRef::OsCredential { service, account } => {
                Some(Self::new(service.clone(), account.clone()))
            }
            crate::secret::SecretRef::Environment { .. } => None,
        }
    }
}

impl ProviderConfig {
    pub fn new(template: impl Into<String>) -> Self {
        Self {
            template: template.into(),
            base_url: None,
            credential_env: Vec::new(),
            credential_store: None,
            headers: Vec::new(),
            enabled: true,
        }
    }

    pub fn template(&self) -> &str {
        &self.template
    }

    pub fn set_template(&mut self, template: impl Into<String>) -> &mut Self {
        self.template = template.into();
        self
    }

    pub fn base_url(&self) -> Option<&str> {
        self.base_url.as_deref()
    }

    pub fn set_base_url(&mut self, base_url: Option<String>) -> &mut Self {
        self.base_url = base_url;
        self
    }

    pub fn credential_env(&self) -> &[String] {
        &self.credential_env
    }

    pub fn set_credential_env(&mut self, names: Vec<String>) -> &mut Self {
        self.credential_env = names;
        self
    }

    /// Where this provider's credential is stored, when the user put it in
    /// the OS's own secure store — see the field's own doc.
    pub fn credential_store(&self) -> Option<&StoredCredentialRef> {
        self.credential_store.as_ref()
    }

    /// `None` clears the record, which is the configuration half of
    /// deleting a stored credential.
    pub fn set_credential_store(&mut self, reference: Option<StoredCredentialRef>) -> &mut Self {
        self.credential_store = reference;
        self
    }

    pub fn headers(&self) -> &[(String, String)] {
        &self.headers
    }

    pub fn set_headers(&mut self, headers: Vec<(String, String)>) -> &mut Self {
        self.headers = headers;
        self
    }

    /// Whether this provider is currently enabled. `true` for a provider no
    /// one has ever disabled, including one loaded from a file written
    /// before this field existed.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Disabling keeps every other field untouched and is fully reversible —
    /// see the field's own doc comment.
    pub fn set_enabled(&mut self, enabled: bool) -> &mut Self {
        self.enabled = enabled;
        self
    }

    /// Turn this stored configuration into the resolvable domain type,
    /// naming it `name` — the key this entry was stored under.
    ///
    /// The template's own base URL is overridden, on every protocol it
    /// declares, when [`ProviderConfig::base_url`] is set — see the field's
    /// own doc for why there is exactly one override rather than one per
    /// protocol: every built-in template today declares exactly one
    /// protocol. Likewise, a non-empty [`ProviderConfig::credential_env`]
    /// replaces the template's own credential names rather than adding to
    /// them.
    pub fn to_provider(
        &self,
        name: &str,
    ) -> Result<crate::provider::Provider, ProviderConfigError> {
        let mut provider = crate::provider::template(&self.template).ok_or_else(|| {
            ProviderConfigError::UnknownTemplate {
                name: name.to_owned(),
                template: self.template.clone(),
            }
        })?;

        provider.name = name.to_owned();
        if let Some(base_url) = &self.base_url {
            for protocol in &mut provider.protocols {
                protocol.base_url = base_url.clone();
            }
        }
        if !self.credential_env.is_empty() {
            provider.credential_env = self.credential_env.clone();
        }
        if !self.headers.is_empty() {
            for (header_name, value) in &self.headers {
                if let Some(offending) = unsafe_header_name_char(header_name) {
                    return Err(ProviderConfigError::InvalidHeaderName {
                        name: name.to_owned(),
                        header_name: header_name.clone(),
                        offending,
                    });
                }
                if let Some(offending) = unsafe_header_value_char(value) {
                    return Err(ProviderConfigError::InvalidHeaderValue {
                        name: name.to_owned(),
                        header_name: header_name.clone(),
                        offending,
                    });
                }
            }
            provider.headers = self.headers.clone();
        }

        Ok(provider)
    }
}

/// The first character of `name` that must not reach a header rendered into
/// an `ANTHROPIC_CUSTOM_HEADERS` line or a Codex `-c` TOML literal, or `None`
/// when every character is safe.
///
/// A header field-name is narrower than [`crate::shim`]'s `check_name`:
/// letters, digits and `-` only — no `_` and no `.`, neither of which an HTTP
/// header name uses. See that function for the shape this one follows.
fn unsafe_header_name_char(name: &str) -> Option<char> {
    name.chars()
        .find(|c| !(c.is_ascii_alphanumeric() || *c == '-'))
}

/// The first character of `value` that must not reach a header value line,
/// or `None` when every character is safe.
///
/// Any control character is refused, which already covers `\r` and `\n`:
/// a header *value* carrying either would let it inject a second header of
/// the attacker's own choosing into every request this provider's child
/// process sends — the exact class [`crate::shim`]'s `check_name` already
/// refuses for profile names, applied here to a header value instead of a
/// name. Refused, never escaped.
fn unsafe_header_value_char(value: &str) -> Option<char> {
    value.chars().find(|c| c.is_control())
}

/// A map of configured providers, keyed by provider name.
///
/// Providers are configuration, never a credential store: every value this
/// table can hold is a template slug, a base-URL override, or credential
/// variable *names* — see [`ProviderConfig`] and the module-level "No
/// secrets here" section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderTable(BTreeMap<String, ProviderConfig>);

impl ProviderTable {
    pub fn get(&self, name: &str) -> Option<&ProviderConfig> {
        self.0.get(name)
    }

    pub fn set(&mut self, name: impl Into<String>, config: ProviderConfig) {
        self.0.insert(name.into(), config);
    }

    pub fn remove(&mut self, name: &str) -> Option<ProviderConfig> {
        self.0.remove(name)
    }

    /// Every configured provider name in this table.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &ProviderConfig)> {
        self.0.iter().map(|(name, cfg)| (name.as_str(), cfg))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// A map of configured launch profiles, keyed by profile name.
///
/// Profiles are configuration, never project memory: nothing here touches
/// the project database, matching [`crate::profile`]'s own rule. The
/// implied Native profile (see [`crate::profile::NATIVE_PROFILE_NAME`]) is
/// never stored here — it exists for every harness by construction — so this
/// table only ever holds profiles a user or project explicitly configured.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProfileTable(BTreeMap<String, ProfileConfig>);

impl ProfileTable {
    pub fn get(&self, name: &str) -> Option<&ProfileConfig> {
        self.0.get(name)
    }

    pub fn set(&mut self, name: impl Into<String>, config: ProfileConfig) {
        self.0.insert(name.into(), config);
    }

    pub fn remove(&mut self, name: &str) -> Option<ProfileConfig> {
        self.0.remove(name)
    }

    /// Every configured profile name in this table (not including the
    /// implied Native profile, which is never stored).
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &ProfileConfig)> {
        self.0.iter().map(|(name, cfg)| (name.as_str(), cfg))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Onboarding progress, persisted so the first-run wizard runs at most once
/// per user (Phase 2C: "Persist onboarding choices in user-level Glasshouse
/// configuration").
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnboardingState {
    #[serde(default)]
    completed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    completed_at_version: Option<String>,
}

impl OnboardingState {
    pub fn completed(&self) -> bool {
        self.completed
    }

    /// The Glasshouse version that was running when onboarding was last
    /// completed, if known. Informational only (e.g. for deciding whether a
    /// changelog-driven "what's new" prompt applies) — nothing in this
    /// module gates behavior on it.
    pub fn completed_at_version(&self) -> Option<&str> {
        self.completed_at_version.as_deref()
    }

    pub fn mark_completed(&mut self, version: impl Into<String>) {
        self.completed = true;
        self.completed_at_version = Some(version.into());
    }

    /// Reset onboarding so the wizard runs again. Phase 2C: "Allow the
    /// onboarding wizard to be reopened later from settings."
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// User-level Glasshouse configuration: `<config_dir>/config.toml`.
///
/// Unknown top-level keys and unknown fields inside known tables are
/// ignored on load rather than rejected, so a file written by a newer
/// Glasshouse still loads here (see [`ConfigError::UnsupportedVersion`] for
/// what still gets refused: writing it back).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserConfig {
    #[serde(default = "current_schema_version")]
    version: u32,
    #[serde(default)]
    onboarding: OnboardingState,
    #[serde(default)]
    integrations: IntegrationTable,
    #[serde(default)]
    profiles: ProfileTable,
    #[serde(default)]
    providers: ProviderTable,
}

impl Default for UserConfig {
    fn default() -> Self {
        Self {
            version: CURRENT_SCHEMA_VERSION,
            onboarding: OnboardingState::default(),
            integrations: IntegrationTable::default(),
            profiles: ProfileTable::default(),
            providers: ProviderTable::default(),
        }
    }
}

impl UserConfig {
    pub fn version(&self) -> u32 {
        self.version
    }

    pub fn onboarding(&self) -> &OnboardingState {
        &self.onboarding
    }

    pub fn onboarding_mut(&mut self) -> &mut OnboardingState {
        &mut self.onboarding
    }

    pub fn integrations(&self) -> &IntegrationTable {
        &self.integrations
    }

    pub fn integrations_mut(&mut self) -> &mut IntegrationTable {
        &mut self.integrations
    }

    pub fn profiles(&self) -> &ProfileTable {
        &self.profiles
    }

    pub fn profiles_mut(&mut self) -> &mut ProfileTable {
        &mut self.profiles
    }

    pub fn providers(&self) -> &ProviderTable {
        &self.providers
    }

    pub fn providers_mut(&mut self) -> &mut ProviderTable {
        &mut self.providers
    }

    /// Load the user-level configuration file named by `paths`.
    ///
    /// A missing file is not an error: it returns [`UserConfig::default`]
    /// (onboarding not completed, no integration decisions recorded). This
    /// is what makes "no initialization command required" true for a normal
    /// first run. A malformed file *is* an error — see [`ConfigError::Parse`].
    pub fn load(paths: &RuntimePaths) -> Result<Self, ConfigError> {
        load_toml_or_default(&paths.user_config_file())
    }

    /// Atomically write this configuration to the user-level configuration
    /// file named by `paths`, creating the configuration directory
    /// (owner-only on Unix) if it does not exist yet.
    ///
    /// Refuses to write, without touching the file, if `version` is newer
    /// than this build understands — see [`ConfigError::UnsupportedVersion`].
    /// That situation only arises by loading a newer file and saving it back
    /// unmodified or by constructing a [`UserConfig`] with an inflated
    /// version by hand; a config this build created itself always carries
    /// [`CURRENT_SCHEMA_VERSION`] and never hits it.
    pub fn save(&self, paths: &RuntimePaths) -> Result<(), ConfigError> {
        let path = paths.user_config_file();
        if self.version > CURRENT_SCHEMA_VERSION {
            return Err(ConfigError::UnsupportedVersion {
                path,
                found: self.version,
                supported: CURRENT_SCHEMA_VERSION,
            });
        }
        write_atomic_toml(paths.config_dir(), &path, self)
    }
}

/// Optional project-level Glasshouse configuration:
/// `<project root>/.glasshouse/config.toml`.
///
/// Same overridable shape as [`UserConfig`]'s integrations — see
/// [`EffectiveConfig`] for how the two are layered together.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectConfig {
    #[serde(default = "current_schema_version")]
    version: u32,
    #[serde(default)]
    integrations: IntegrationTable,
    #[serde(default)]
    profiles: ProfileTable,
    #[serde(default)]
    providers: ProviderTable,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            version: CURRENT_SCHEMA_VERSION,
            integrations: IntegrationTable::default(),
            profiles: ProfileTable::default(),
            providers: ProviderTable::default(),
        }
    }
}

impl ProjectConfig {
    pub fn version(&self) -> u32 {
        self.version
    }

    pub fn integrations(&self) -> &IntegrationTable {
        &self.integrations
    }

    pub fn integrations_mut(&mut self) -> &mut IntegrationTable {
        &mut self.integrations
    }

    pub fn profiles(&self) -> &ProfileTable {
        &self.profiles
    }

    pub fn profiles_mut(&mut self) -> &mut ProfileTable {
        &mut self.profiles
    }

    pub fn providers(&self) -> &ProviderTable {
        &self.providers
    }

    pub fn providers_mut(&mut self) -> &mut ProviderTable {
        &mut self.providers
    }
}

/// Resolve the project-level configuration file path inside `project`'s
/// scope.
///
/// Going through [`crate::project::ProjectScope::resolve`] rather than a
/// plain `project.root().join(...)` is the point: even though the relative
/// path here is a fixed constant we control, resolving it through the scope
/// guard means the write path can never end up outside the project root
/// through a symlink planted at `.glasshouse` (or anywhere along it), and it
/// keeps this module honest with every other component that touches a
/// project-relative path.
fn project_config_path(project: &Project) -> Result<PathBuf, ConfigError> {
    project
        .scope()
        .resolve(PROJECT_CONFIG_RELATIVE_PATH)
        .map_err(ConfigError::Scope)
}

/// Load the optional project-level configuration for `project`, if the user
/// has ever created one.
///
/// Returns `Ok(None)` when no such file exists. This function never creates
/// one — see [`write_project_config_with_consent`] for the only way this
/// file comes into existence.
pub fn load_project_config(project: &Project) -> Result<Option<ProjectConfig>, ConfigError> {
    let path = project_config_path(project)?;
    match std::fs::read_to_string(&path) {
        Ok(contents) => parse_toml(&path, &contents).map(Some),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ConfigError::Read { path, source }),
    }
}

/// Write `config` into `project`'s `.glasshouse/config.toml`, creating the
/// `.glasshouse` directory (owner-only on Unix) if needed.
///
/// # This requires the user's explicit consent
///
/// This writes inside the user's project tree — Phase 2D requires "explicit
/// confirmation before writing project-level configuration into the
/// repository," and this function performs none of that confirmation
/// itself; it is the caller's (the settings UI's) job to have obtained it
/// first. The `_with_consent` suffix exists so this is never reached for
/// unconditionally, by-default, or "just in case" writes — a project that
/// has not opted in must never grow a `.glasshouse/config.toml` on its own.
pub fn write_project_config_with_consent(
    project: &Project,
    config: &ProjectConfig,
) -> Result<(), ConfigError> {
    let path = project_config_path(project)?;
    if config.version > CURRENT_SCHEMA_VERSION {
        return Err(ConfigError::UnsupportedVersion {
            path,
            found: config.version,
            supported: CURRENT_SCHEMA_VERSION,
        });
    }
    // `path` is `<root>/.glasshouse/config.toml`, so it always has a parent;
    // the fallback only guards a `PROJECT_CONFIG_RELATIVE_PATH` that no
    // longer names a nested file, which would be a bug in this module, not
    // something a caller can trigger.
    let dir = path.parent().unwrap_or(&path).to_path_buf();
    write_atomic_toml(&dir, &path, config)
}

/// Which configuration layer supplied a resolved value. Surfaced so the
/// Phase 2D settings view can visibly distinguish a user-level default from
/// a project-level override, as required.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    /// Read from the project-level file.
    Project,
    /// Read from the user-level file.
    User,
    /// Neither layer had a recorded value; this is a hardcoded fallback.
    Default,
}

/// A resolved value together with which layer produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layered<T> {
    pub value: T,
    pub layer: Layer,
}

impl<T> Layered<T> {
    pub fn new(value: T, layer: Layer) -> Self {
        Self { value, layer }
    }
}

/// User configuration layered with an optional project-level override.
///
/// Kept intentionally small — a couple of lookup methods, not a generic
/// layering framework — because today only per-integration `enabled` and
/// `executable` need layering. Project always wins when it has recorded a
/// value; otherwise the user-level value applies; otherwise a caller
/// supplied default applies. Each lookup reports which of those three
/// happened via [`Layer`].
#[derive(Debug, Clone, Copy)]
pub struct EffectiveConfig<'a> {
    user: &'a UserConfig,
    project: Option<&'a ProjectConfig>,
}

impl<'a> EffectiveConfig<'a> {
    pub fn new(user: &'a UserConfig, project: Option<&'a ProjectConfig>) -> Self {
        Self { user, project }
    }

    /// Resolve whether `id` is enabled, reporting which layer decided it.
    /// Falls back to `default_enabled` (reported as [`Layer::Default`]) when
    /// neither layer has ever recorded a decision.
    pub fn enabled(&self, id: IntegrationId, default_enabled: bool) -> Layered<bool> {
        if let Some(enabled) = self.project.and_then(|p| p.integrations().is_enabled(id)) {
            return Layered::new(enabled, Layer::Project);
        }
        if let Some(enabled) = self.user.integrations().is_enabled(id) {
            return Layered::new(enabled, Layer::User);
        }
        Layered::new(default_enabled, Layer::Default)
    }

    /// Resolve whether `id` has the user's consent to write its lifecycle
    /// hooks inside the project itself, reporting which layer decided it.
    /// Falls back to `false` (reported as [`Layer::Default`]) when neither
    /// layer has ever recorded a decision — unlike
    /// [`EffectiveConfig::enabled`], callers never get to choose that
    /// default, because a session with no consent on record must run without
    /// project-local hooks rather than assume the answer either way.
    pub fn project_hooks(&self, id: IntegrationId) -> Layered<bool> {
        if let Some(consent) = self
            .project
            .and_then(|p| p.integrations().get(id))
            .and_then(IntegrationConfig::project_hooks)
        {
            return Layered::new(consent, Layer::Project);
        }
        if let Some(consent) = self
            .user
            .integrations()
            .get(id)
            .and_then(IntegrationConfig::project_hooks)
        {
            return Layered::new(consent, Layer::User);
        }
        Layered::new(false, Layer::Default)
    }

    /// Resolve whether the user has acknowledged `id`'s blanket approval
    /// bypass, reporting which layer decided it. Falls back to `false`
    /// (reported as [`Layer::Default`]) when the user layer has never
    /// recorded a decision.
    ///
    /// **This deliberately consults `self.user` only — never
    /// `self.project`.** Every other lookup on this type checks the project
    /// layer first; this one must not, because a repository that could
    /// pre-acknowledge a blanket permission bypass would be acknowledging it
    /// on behalf of whoever cloned it, who has been shown nothing. The
    /// acknowledgement this field records is a statement by a person about a
    /// harness on *their own machine*, not a property of the project, so a
    /// project-level `bypass_acknowledged = true` checked into a repository
    /// must have no effect at all. Say this plainly in code, because the
    /// deviation from every other lookup here reads as an oversight
    /// otherwise — it is not one.
    pub fn bypass_acknowledged(&self, id: IntegrationId) -> Layered<bool> {
        if let Some(acknowledged) = self
            .user
            .integrations()
            .get(id)
            .and_then(IntegrationConfig::bypass_acknowledged)
        {
            return Layered::new(acknowledged, Layer::User);
        }
        Layered::new(false, Layer::Default)
    }

    /// Resolve the explicit executable override for `id`, if any layer has
    /// recorded one. `None` means neither layer has an override, i.e. normal
    /// `PATH` discovery applies — there is no "default" executable path to
    /// report here, so unlike [`EffectiveConfig::enabled`] this has no
    /// [`Layer::Default`] case.
    pub fn executable(&self, id: IntegrationId) -> Option<Layered<PathBuf>> {
        if let Some(exe) = self
            .project
            .and_then(|p| p.integrations().get(id))
            .and_then(IntegrationConfig::executable)
        {
            return Some(Layered::new(exe.to_path_buf(), Layer::Project));
        }
        if let Some(exe) = self
            .user
            .integrations()
            .get(id)
            .and_then(IntegrationConfig::executable)
        {
            return Some(Layered::new(exe.to_path_buf(), Layer::User));
        }
        None
    }

    /// Every launch profile name available: the implied
    /// [`crate::profile::NATIVE_PROFILE_NAME`], plus every name either layer
    /// has configured. Where both layers configure the same name, this still
    /// lists it once — see [`EffectiveConfig::launch_profile`] for which
    /// layer's definition wins.
    pub fn profile_names(&self) -> Vec<String> {
        let mut names: BTreeSet<String> = BTreeSet::new();
        names.insert(crate::profile::NATIVE_PROFILE_NAME.to_owned());
        names.extend(self.user.profiles().names().map(str::to_owned));
        if let Some(project) = self.project {
            names.extend(project.profiles().names().map(str::to_owned));
        }
        names.into_iter().collect()
    }

    /// Resolve `name` to a [`crate::profile::LaunchProfile`] for `harness`,
    /// reporting which layer supplied it.
    ///
    /// The implied Native profile is available for every harness regardless
    /// of either layer — by construction rather than a configuration entry,
    /// so adding gateway profiles can never remove it — and is built
    /// directly for `harness` without a table lookup. For any other name,
    /// the project layer's definition wins over the user layer's, matching
    /// every other lookup on this type; and a profile that names a harness
    /// other than `harness` is refused rather than silently substituted,
    /// because that harness is what the caller has already selected (or the
    /// user explicitly typed on the command line).
    pub fn launch_profile(
        &self,
        name: &str,
        harness: IntegrationId,
    ) -> Result<Layered<crate::profile::LaunchProfile>, ProfileLookupError> {
        if name == crate::profile::NATIVE_PROFILE_NAME {
            return Ok(Layered::new(
                crate::profile::LaunchProfile::native(harness),
                Layer::Default,
            ));
        }

        let found = if let Some(config) = self.project.and_then(|p| p.profiles().get(name)) {
            Some((config, Layer::Project))
        } else {
            self.user
                .profiles()
                .get(name)
                .map(|config| (config, Layer::User))
        };

        let Some((config, layer)) = found else {
            return Err(ProfileLookupError::Unknown {
                name: name.to_owned(),
                known: self.profile_names(),
            });
        };

        let profile = config.to_launch_profile(name)?;
        if profile.harness != harness {
            return Err(ProfileLookupError::HarnessMismatch {
                name: name.to_owned(),
                profile_harness: profile.harness,
                requested_harness: harness,
            });
        }
        Ok(Layered::new(profile, layer))
    }

    /// Every configured provider name available, from either layer.
    ///
    /// Unlike [`EffectiveConfig::profile_names`], there is no implied entry:
    /// a provider only exists here because a user or project explicitly
    /// configured one.
    pub fn provider_names(&self) -> Vec<String> {
        let mut names: BTreeSet<String> = BTreeSet::new();
        names.extend(self.user.providers().names().map(str::to_owned));
        if let Some(project) = self.project {
            names.extend(project.providers().names().map(str::to_owned));
        }
        names.into_iter().collect()
    }

    /// Resolve `name` to a [`crate::provider::Provider`], reporting which
    /// layer supplied it. The project layer's definition wins over the user
    /// layer's, matching every other lookup on this type.
    pub fn configured_provider(
        &self,
        name: &str,
    ) -> Result<Layered<crate::provider::Provider>, ProviderLookupError> {
        let found = if let Some(config) = self.project.and_then(|p| p.providers().get(name)) {
            Some((config, Layer::Project))
        } else {
            self.user
                .providers()
                .get(name)
                .map(|config| (config, Layer::User))
        };

        let Some((config, layer)) = found else {
            return Err(ProviderLookupError::Unknown {
                name: name.to_owned(),
                known: self.provider_names(),
            });
        };

        let provider = config.to_provider(name)?;
        Ok(Layered::new(provider, layer))
    }
}

/// Why a provider named on the command line, or looked up for `glasshouse
/// doctor`, could not be resolved.
#[derive(Debug, thiserror::Error)]
pub enum ProviderLookupError {
    #[error("`{name}` is not a configured provider; valid names are: {}", .known.join(", "))]
    Unknown { name: String, known: Vec<String> },
    #[error(transparent)]
    Invalid(#[from] ProviderConfigError),
}

/// Why a launch profile named on the command line could not be resolved.
#[derive(Debug, thiserror::Error)]
pub enum ProfileLookupError {
    #[error("`{name}` is not a known launch profile; valid names are: {}", .known.join(", "))]
    Unknown { name: String, known: Vec<String> },
    #[error(
        "launch profile `{name}` is for {}, not {}; name the harness the profile itself \
         belongs to, or choose a different profile",
        .profile_harness.display_name(), .requested_harness.display_name()
    )]
    HarnessMismatch {
        name: String,
        profile_harness: IntegrationId,
        requested_harness: IntegrationId,
    },
    #[error(transparent)]
    Invalid(#[from] ProfileConfigError),
}

/// Load a TOML-serialized `T` from `path`, or `T::default()` if the file
/// does not exist.
fn load_toml_or_default<T: Default + serde::de::DeserializeOwned>(
    path: &Path,
) -> Result<T, ConfigError> {
    match std::fs::read_to_string(path) {
        Ok(contents) => parse_toml(path, &contents),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(T::default()),
        Err(source) => Err(ConfigError::Read {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn parse_toml<T: serde::de::DeserializeOwned>(
    path: &Path,
    contents: &str,
) -> Result<T, ConfigError> {
    toml::from_str(contents).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source: Box::new(source),
    })
}

/// Monotonic counter mixed into temporary file names so that two saves
/// racing inside the same process (as can happen across test threads, which
/// share a process id) never pick the same temporary path.
static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Serialize `value` as pretty TOML and atomically replace `path` with it.
///
/// Writes to a fresh temporary file inside `dir` (which must be the same
/// directory as `path`, so the following rename stays on one filesystem)
/// with owner-only permissions, then `rename`s it over `path`. `rename` is
/// atomic on both POSIX and Windows when source and destination share a
/// filesystem, so a crash or power loss during the write can only ever
/// leave the previous file intact or the new file complete — never a
/// half-written config on disk. `dir` is created first (owner-only on Unix,
/// mirroring `create_state_dir` in `lib.rs`) if it does not exist yet.
fn write_atomic_toml<T: Serialize>(dir: &Path, path: &Path, value: &T) -> Result<(), ConfigError> {
    create_secure_dir(dir).map_err(|source| ConfigError::CreateDir {
        path: dir.to_path_buf(),
        source,
    })?;

    let contents = toml::to_string_pretty(value).map_err(|source| ConfigError::Serialize {
        path: path.to_path_buf(),
        source: Box::new(source),
    })?;

    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config.toml".to_owned());
    let unique = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp_path = dir.join(format!(".{file_name}.{}.{unique}.tmp", std::process::id()));

    write_secure_file(&tmp_path, contents.as_bytes()).map_err(|source| ConfigError::Write {
        path: tmp_path.clone(),
        source,
    })?;

    if let Err(source) = std::fs::rename(&tmp_path, path) {
        // Best-effort cleanup: leaving the temp file behind on a failed
        // rename is better than leaving nothing, but never mask the real
        // error with a cleanup failure.
        let _ = std::fs::remove_file(&tmp_path);
        return Err(ConfigError::Write {
            path: path.to_path_buf(),
            source,
        });
    }

    Ok(())
}

/// Create `dir` (and parents) restricted to its owner on Unix. Mirrors
/// `create_state_dir` in `lib.rs`: a config file can carry integration
/// executable paths and, later, other user-specific detail, so default
/// (typically world-readable) directory permissions are not appropriate.
/// When `dir` already exists as something else, this keeps whatever
/// permissions it already had — it neither widens nor narrows a directory
/// it did not create.
#[cfg(unix)]
fn create_secure_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

#[cfg(not(unix))]
fn create_secure_dir(dir: &Path) -> io::Result<()> {
    std::fs::DirBuilder::new().recursive(true).create(dir)
}

/// Write `contents` to a new file at `path`, restricted to its owner on
/// Unix. `path` is always a fresh temporary file name (see
/// [`write_atomic_toml`]), so `create_new` semantics are not required here;
/// `create + truncate` is enough.
#[cfg(unix)]
fn write_secure_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(contents)?;
    file.sync_all()
}

#[cfg(not(unix))]
fn write_secure_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
    file.write_all(contents)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::Project;

    /// Build a `Project` rooted at `root` for tests. `root` must already
    /// exist; a plain (non-Git) temp directory falls back to
    /// `RootSource::WorkingDirectory`, which is exactly what these tests
    /// want — no `.git` scaffolding needed.
    fn test_project(root: &Path) -> Project {
        Project::discover(root, None, false).expect("test project root must be usable")
    }

    /// A [`crate::secret::SecretStore`] holding exactly one credential — for
    /// tests here that just need a direct-provider profile to resolve,
    /// rather than exercising secret resolution itself (that belongs to
    /// `crate::profile`'s own tests).
    struct OneShotSecrets(&'static str, &'static str);

    impl crate::secret::SecretStore for OneShotSecrets {
        fn resolve(&self, reference: &crate::secret::SecretRef) -> Option<crate::secret::Secret> {
            let crate::secret::SecretRef::Environment { var } = reference else {
                return None;
            };
            (var == self.0).then(|| crate::secret::Secret::mint_for_test(self.1))
        }

        fn is_present(&self, reference: &crate::secret::SecretRef) -> bool {
            self.resolve(reference).is_some()
        }

        fn describe(&self) -> &'static str {
            "one-shot test store"
        }
    }

    fn fully_populated_user_config() -> UserConfig {
        let mut config = UserConfig::default();
        config.onboarding_mut().mark_completed("0.1.0".to_owned());
        config
            .integrations_mut()
            .entry(IntegrationId::ClaudeCode)
            .set_enabled(true)
            .set_executable(Some(PathBuf::from("/opt/claude-code/bin/claude")));
        config
            .integrations_mut()
            .entry(IntegrationId::Codex)
            .set_enabled(false);
        config
            .integrations_mut()
            .entry(IntegrationId::Hermes)
            .set_bypass_acknowledged(true);
        let mut profile = ProfileConfig::new(IntegrationId::ClaudeCode);
        profile.set_approval(ProfileApproval::AutomaticReview);
        config.profiles_mut().set("fast", profile);
        config
    }

    #[test]
    fn missing_file_loads_as_default() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));

        let config = UserConfig::load(&paths).unwrap();
        assert_eq!(config, UserConfig::default());
        assert!(!config.onboarding().completed());
        assert!(config.integrations().is_empty());
        // Loading must not have created anything.
        assert!(!paths.user_config_file().exists());
    }

    #[test]
    fn round_trip_save_load_preserves_every_field() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));

        let original = fully_populated_user_config();
        original.save(&paths).unwrap();

        let loaded = UserConfig::load(&paths).unwrap();
        assert_eq!(loaded, original);
        assert_eq!(loaded.onboarding().completed_at_version(), Some("0.1.0"));
        assert_eq!(
            loaded
                .integrations()
                .get(IntegrationId::ClaudeCode)
                .unwrap()
                .executable(),
            Some(Path::new("/opt/claude-code/bin/claude"))
        );
        assert_eq!(
            loaded.integrations().is_enabled(IntegrationId::Codex),
            Some(false)
        );
        assert_eq!(
            loaded
                .integrations()
                .get(IntegrationId::Hermes)
                .unwrap()
                .bypass_acknowledged(),
            Some(true)
        );
        let profile = loaded.profiles().get("fast").unwrap();
        assert_eq!(profile.harness_slug(), "claude-code");
        assert_eq!(profile.approval(), ProfileApproval::AutomaticReview);
    }

    #[test]
    fn a_file_written_by_a_newer_version_loads_but_refuses_to_save() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        std::fs::write(
            paths.user_config_file(),
            "version = 999\n\n[onboarding]\ncompleted = true\n",
        )
        .unwrap();

        let loaded = UserConfig::load(&paths).unwrap();
        assert_eq!(loaded.version(), 999);
        assert!(loaded.onboarding().completed());

        let err = loaded.save(&paths).unwrap_err();
        assert!(matches!(
            err,
            ConfigError::UnsupportedVersion {
                found: 999,
                supported: CURRENT_SCHEMA_VERSION,
                ..
            }
        ));
        let msg = err.to_string();
        assert!(msg.contains("newer version"), "{msg}");

        // The file on disk must be untouched by the failed save.
        let raw = std::fs::read_to_string(paths.user_config_file()).unwrap();
        assert!(raw.contains("999"));
    }

    #[test]
    fn unknown_keys_and_fields_do_not_break_parsing() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        std::fs::write(
            paths.user_config_file(),
            r#"
                version = 1
                some_future_top_level_key = "ignored"

                [onboarding]
                completed = true
                completed_at_version = "9.9.9"
                some_future_onboarding_field = 42

                [integrations.claude-code]
                enabled = true
                some_future_integration_field = true

                [integrations.a-future-harness-this-build-does-not-know]
                enabled = true
            "#,
        )
        .unwrap();

        let config = UserConfig::load(&paths).unwrap();
        assert!(config.onboarding().completed());
        assert_eq!(
            config.integrations().is_enabled(IntegrationId::ClaudeCode),
            Some(true)
        );
        // The unrecognized slug round-trips through the map even though no
        // `IntegrationId` variant names it.
        assert_eq!(
            config
                .integrations()
                .iter()
                .find(|(slug, _)| *slug == "a-future-harness-this-build-does-not-know")
                .map(|(_, cfg)| cfg.enabled()),
            Some(Some(true))
        );
    }

    #[test]
    fn missing_version_field_defaults_to_current_schema_version() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        std::fs::write(paths.user_config_file(), "[onboarding]\ncompleted = true\n").unwrap();

        let config = UserConfig::load(&paths).unwrap();
        assert_eq!(config.version(), CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn malformed_toml_is_an_error_naming_the_path_and_does_not_overwrite() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        let broken = "version = 1\n[onboarding\ncompleted = true\n";
        std::fs::write(paths.user_config_file(), broken).unwrap();

        let err = UserConfig::load(&paths).unwrap_err();
        assert!(matches!(err, ConfigError::Parse { .. }));
        let msg = err.to_string();
        assert!(
            msg.contains(&paths.user_config_file().display().to_string()),
            "{msg}"
        );

        // Nothing must have touched the file: same content, no temp files.
        let raw = std::fs::read_to_string(paths.user_config_file()).unwrap();
        assert_eq!(raw, broken);
        let entries: Vec<_> = std::fs::read_dir(paths.config_dir())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(entries, vec![std::ffi::OsString::from("config.toml")]);
    }

    #[test]
    fn atomic_save_leaves_no_temporary_file_behind() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));

        fully_populated_user_config().save(&paths).unwrap();

        let entries: Vec<_> = std::fs::read_dir(paths.config_dir())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, vec!["config.toml".to_owned()], "{entries:?}");
    }

    #[cfg(unix)]
    #[test]
    fn unix_permissions_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));
        UserConfig::default().save(&paths).unwrap();

        let dir_mode = std::fs::metadata(paths.config_dir())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(dir_mode, 0o700, "config dir mode was {dir_mode:o}");

        let file_mode = std::fs::metadata(paths.user_config_file())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode, 0o600, "config file mode was {file_mode:o}");
    }

    #[test]
    fn tri_state_enabled_distinguishes_never_asked_from_a_decision() {
        let mut config = UserConfig::default();
        assert_eq!(
            config.integrations().is_enabled(IntegrationId::ClaudeCode),
            None,
            "never asked"
        );
        assert!(
            config
                .integrations()
                .is_enabled_or_default(IntegrationId::ClaudeCode, true)
        );
        assert!(
            !config
                .integrations()
                .is_enabled_or_default(IntegrationId::ClaudeCode, false)
        );

        config
            .integrations_mut()
            .entry(IntegrationId::ClaudeCode)
            .set_enabled(false);
        assert_eq!(
            config.integrations().is_enabled(IntegrationId::ClaudeCode),
            Some(false),
            "explicitly declined"
        );
        assert!(
            !config
                .integrations()
                .is_enabled_or_default(IntegrationId::ClaudeCode, true)
        );

        config
            .integrations_mut()
            .entry(IntegrationId::ClaudeCode)
            .set_enabled(true);
        assert_eq!(
            config.integrations().is_enabled(IntegrationId::ClaudeCode),
            Some(true),
            "explicitly accepted"
        );
        assert!(
            config
                .integrations()
                .is_enabled_or_default(IntegrationId::ClaudeCode, false)
        );
    }

    #[test]
    fn tri_state_project_hooks_consent_distinguishes_never_asked_from_a_decision() {
        let mut config = UserConfig::default();
        assert_eq!(
            config.integrations().get(IntegrationId::Codex),
            None,
            "never asked"
        );

        config
            .integrations_mut()
            .entry(IntegrationId::Codex)
            .set_project_hooks(false);
        assert_eq!(
            config
                .integrations()
                .get(IntegrationId::Codex)
                .unwrap()
                .project_hooks(),
            Some(false),
            "explicitly declined"
        );

        config
            .integrations_mut()
            .entry(IntegrationId::Codex)
            .set_project_hooks(true);
        assert_eq!(
            config
                .integrations()
                .get(IntegrationId::Codex)
                .unwrap()
                .project_hooks(),
            Some(true),
            "explicitly consented"
        );

        // Recording a decision about `enabled` must not silently record one
        // about `project_hooks` too — the whole reason this is a second
        // `Option<bool>` field rather than folded into `enabled`.
        let mut only_enabled = UserConfig::default();
        only_enabled
            .integrations_mut()
            .entry(IntegrationId::Codex)
            .set_enabled(true);
        assert_eq!(
            only_enabled
                .integrations()
                .get(IntegrationId::Codex)
                .unwrap()
                .project_hooks(),
            None
        );
    }

    #[test]
    fn effective_config_defaults_project_hooks_consent_to_withheld() {
        // Absent consent must resolve to `false`, never `true` — a session
        // with no recorded decision must run without project-local hooks.
        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, None);
        let consent = effective.project_hooks(IntegrationId::Codex);
        assert!(!consent.value);
        assert_eq!(consent.layer, Layer::Default);
    }

    #[test]
    fn effective_config_project_hooks_consent_layers_like_enabled() {
        let mut user = UserConfig::default();
        user.integrations_mut()
            .entry(IntegrationId::Codex)
            .set_project_hooks(true);

        let mut project = ProjectConfig::default();
        project
            .integrations_mut()
            .entry(IntegrationId::Codex)
            .set_project_hooks(false);

        let effective = EffectiveConfig::new(&user, Some(&project));
        let consent = effective.project_hooks(IntegrationId::Codex);
        assert!(!consent.value, "the project layer withdraws consent");
        assert_eq!(consent.layer, Layer::Project);

        let effective_without_project = EffectiveConfig::new(&user, None);
        let consent = effective_without_project.project_hooks(IntegrationId::Codex);
        assert!(consent.value, "the user layer's consent still applies");
        assert_eq!(consent.layer, Layer::User);
    }

    #[test]
    fn effective_config_defaults_bypass_acknowledgement_to_withheld() {
        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, None);
        let acknowledged = effective.bypass_acknowledged(IntegrationId::Hermes);
        assert!(!acknowledged.value);
        assert_eq!(acknowledged.layer, Layer::Default);
    }

    /// Phase 9A: "Keep native-subscription profiles available even when
    /// gateway providers are configured."
    ///
    /// The Native profile is implied rather than stored, so no amount of
    /// configuration in either layer can displace it. This is the test that
    /// fails if someone ever "unifies" the lookup by moving Native into the
    /// table alongside everything else.
    #[test]
    fn a_configured_gateway_profile_never_displaces_the_native_one() {
        let mut user = UserConfig::default();
        let mut gateway = ProfileConfig::new(IntegrationId::ClaudeCode);
        gateway.set_backend(ProfileBackend::DirectProvider {
            provider: "openrouter".to_owned(),
        });
        user.profiles_mut().set("gateway", gateway);

        let mut project = ProjectConfig::default();
        let mut local = ProfileConfig::new(IntegrationId::Codex);
        local.set_backend(ProfileBackend::GlasshouseGateway);
        project.profiles_mut().set("local", local);

        let effective = EffectiveConfig::new(&user, Some(&project));

        let names = effective.profile_names();
        assert!(
            names
                .iter()
                .any(|n| n == crate::profile::NATIVE_PROFILE_NAME),
            "the native profile must survive every configured profile: {names:?}"
        );
        assert!(names.iter().any(|n| n == "gateway"), "{names:?}");
        assert!(names.iter().any(|n| n == "local"), "{names:?}");

        // And it still resolves for a harness that has a gateway profile of
        // its own configured — the case where a lookup that consulted the
        // table first would go wrong.
        let native = effective
            .launch_profile(
                crate::profile::NATIVE_PROFILE_NAME,
                IntegrationId::ClaudeCode,
            )
            .expect("the native profile is available for every harness");
        assert!(matches!(
            native.value.backend,
            crate::profile::BackendResource::Native
        ));
    }

    #[test]
    fn a_project_layer_cannot_acknowledge_a_bypass() {
        // Unlike every other lookup on `EffectiveConfig`, a project-level
        // acknowledgement must have no effect at all: acknowledging a
        // blanket bypass is a statement by a person about a harness on their
        // own machine, and a repository cannot make that statement on behalf
        // of whoever cloned it.
        let mut project = ProjectConfig::default();
        project
            .integrations_mut()
            .entry(IntegrationId::Hermes)
            .set_bypass_acknowledged(true);

        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, Some(&project));
        let acknowledged = effective.bypass_acknowledged(IntegrationId::Hermes);
        assert!(
            !acknowledged.value,
            "a project-level acknowledgement must not count"
        );
        assert_eq!(acknowledged.layer, Layer::Default);

        // The user layer's own acknowledgement still applies, and still only
        // for the harness it named.
        let mut user_with_ack = UserConfig::default();
        user_with_ack
            .integrations_mut()
            .entry(IntegrationId::Hermes)
            .set_bypass_acknowledged(true);
        let effective = EffectiveConfig::new(&user_with_ack, Some(&project));
        let acknowledged = effective.bypass_acknowledged(IntegrationId::Hermes);
        assert!(acknowledged.value);
        assert_eq!(acknowledged.layer, Layer::User);

        let other = effective.bypass_acknowledged(IntegrationId::Antigravity);
        assert!(
            !other.value,
            "acknowledging Hermes must not acknowledge Antigravity"
        );
        assert_eq!(other.layer, Layer::Default);
    }

    #[test]
    fn project_config_layering_reports_the_correct_source_layer() {
        let mut user = UserConfig::default();
        user.integrations_mut()
            .entry(IntegrationId::ClaudeCode)
            .set_enabled(true);
        user.integrations_mut()
            .entry(IntegrationId::Codex)
            .set_enabled(true)
            .set_executable(Some(PathBuf::from("/usr/local/bin/codex")));

        let mut project = ProjectConfig::default();
        // Project explicitly disables what the user enabled: project wins.
        project
            .integrations_mut()
            .entry(IntegrationId::ClaudeCode)
            .set_enabled(false);

        let effective = EffectiveConfig::new(&user, Some(&project));

        // Case 1: project overrides user.
        let claude = effective.enabled(IntegrationId::ClaudeCode, true);
        assert!(!claude.value);
        assert_eq!(claude.layer, Layer::Project);

        // Case 2: only user has a decision.
        let codex = effective.enabled(IntegrationId::Codex, false);
        assert!(codex.value);
        assert_eq!(codex.layer, Layer::User);
        let codex_exe = effective.executable(IntegrationId::Codex).unwrap();
        assert_eq!(codex_exe.value, PathBuf::from("/usr/local/bin/codex"));
        assert_eq!(codex_exe.layer, Layer::User);

        // Case 3: neither layer has a decision, so the caller default wins.
        let ollama = effective.enabled(IntegrationId::Ollama, true);
        assert!(ollama.value);
        assert_eq!(ollama.layer, Layer::Default);
        assert!(effective.executable(IntegrationId::Ollama).is_none());
    }

    #[test]
    fn effective_config_without_a_project_file_falls_back_to_user_then_default() {
        let mut user = UserConfig::default();
        user.integrations_mut()
            .entry(IntegrationId::ClaudeCode)
            .set_enabled(true);

        let effective = EffectiveConfig::new(&user, None);
        let claude = effective.enabled(IntegrationId::ClaudeCode, false);
        assert!(claude.value);
        assert_eq!(claude.layer, Layer::User);

        let codex = effective.enabled(IntegrationId::Codex, false);
        assert!(!codex.value);
        assert_eq!(codex.layer, Layer::Default);
    }

    #[test]
    fn project_config_is_never_created_automatically() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("project");
        std::fs::create_dir_all(&root).unwrap();
        let project = test_project(&root);

        let loaded = load_project_config(&project).unwrap();
        assert!(loaded.is_none());
        assert!(!root.join(".glasshouse").exists());
    }

    #[test]
    fn project_config_round_trips_and_requires_the_consent_named_call() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("project");
        std::fs::create_dir_all(&root).unwrap();
        let project = test_project(&root);

        let mut config = ProjectConfig::default();
        config
            .integrations_mut()
            .entry(IntegrationId::ClaudeCode)
            .set_enabled(true)
            .set_executable(Some(PathBuf::from("./vendored/claude")));

        write_project_config_with_consent(&project, &config).unwrap();

        assert!(root.join(".glasshouse/config.toml").is_file());
        let loaded = load_project_config(&project).unwrap().unwrap();
        assert_eq!(loaded, config);
    }

    // The relative path this module resolves (`.glasshouse/config.toml`) is a
    // fixed constant, not caller-controlled input, so there is no untrusted
    // string that could ever literally spell its way outside the project
    // root. The one honest way to make `ProjectScope::resolve` actually
    // reject it is the scenario its own doc comment names: a symlink planted
    // at (or under) `.glasshouse` that resolves outside the root. A raw
    // `root.join(".glasshouse/config.toml")` would happily write through
    // such a symlink; going through the scope guard must not.
    //
    // Symlinks are POSIX-only in this test; `std::os::windows::fs::symlink_dir`
    // requires a privilege this sandbox does not reliably have, and the
    // `resolve` codepath under test is exercised identically on every
    // platform (see `crate::project::scope`'s own cross-platform tests), so
    // one platform is enough to prove this module wires it up correctly.
    #[cfg(unix)]
    #[test]
    fn project_config_path_is_resolved_through_the_project_scope() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("project");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        // `.glasshouse` itself is a symlink escaping the project root.
        std::os::unix::fs::symlink(&outside, root.join(".glasshouse")).unwrap();
        let project = test_project(&root);

        let err = load_project_config(&project).unwrap_err();
        assert!(matches!(err, ConfigError::Scope(_)), "{err:?}");

        let err =
            write_project_config_with_consent(&project, &ProjectConfig::default()).unwrap_err();
        assert!(matches!(err, ConfigError::Scope(_)), "{err:?}");
        // And critically: the write must not have gone through to the
        // symlink target either.
        assert!(!outside.join("config.toml").exists());
    }

    #[test]
    fn project_executable_only_override_falls_through_to_user_enabled_decision() {
        let mut user = UserConfig::default();
        user.integrations_mut()
            .entry(IntegrationId::ClaudeCode)
            .set_enabled(true);

        let mut project = ProjectConfig::default();
        project
            .integrations_mut()
            .entry(IntegrationId::ClaudeCode)
            .set_executable(Some(PathBuf::from("/opt/bin/claude")));

        let effective = EffectiveConfig::new(&user, Some(&project));

        let enabled = effective.enabled(IntegrationId::ClaudeCode, true);
        assert!(enabled.value);
        assert_eq!(enabled.layer, Layer::User);

        let executable = effective.executable(IntegrationId::ClaudeCode).unwrap();
        assert_eq!(executable.value, PathBuf::from("/opt/bin/claude"));
        assert_eq!(executable.layer, Layer::Project);
    }

    #[test]
    fn explicit_project_disable_still_wins_over_user_enable() {
        let mut user = UserConfig::default();
        user.integrations_mut()
            .entry(IntegrationId::ClaudeCode)
            .set_enabled(true);

        let mut project = ProjectConfig::default();
        project
            .integrations_mut()
            .entry(IntegrationId::ClaudeCode)
            .set_enabled(false);

        let effective = EffectiveConfig::new(&user, Some(&project));
        let enabled = effective.enabled(IntegrationId::ClaudeCode, true);
        assert!(!enabled.value);
        assert_eq!(enabled.layer, Layer::Project);
    }

    #[test]
    fn enabled_key_parses_to_some_and_its_absence_parses_to_none() {
        let enabled_true: IntegrationConfig =
            toml::from_str("enabled = true\nexecutable = \"/x/y\"").unwrap();
        assert_eq!(enabled_true.enabled(), Some(true));

        let explicit_false: ProjectConfig = toml::from_str(
            r#"
                [integrations.claude-code]
                enabled = false
            "#,
        )
        .unwrap();
        assert_eq!(
            explicit_false
                .integrations()
                .is_enabled(IntegrationId::ClaudeCode),
            Some(false)
        );

        let omitted: ProjectConfig = toml::from_str(
            r#"
                [integrations.claude-code]
                executable = "/opt/bin/claude"
            "#,
        )
        .unwrap();
        assert_eq!(
            omitted
                .integrations()
                .get(IntegrationId::ClaudeCode)
                .unwrap()
                .enabled(),
            None
        );
        assert_eq!(
            omitted.integrations().is_enabled(IntegrationId::ClaudeCode),
            None,
            "an entry without a recorded decision is None, not Some(false)"
        );
    }

    #[test]
    fn serializing_no_decision_omits_the_enabled_key() {
        let no_decision = IntegrationConfig {
            enabled: None,
            executable: Some(PathBuf::from("/opt/bin/claude")),
            project_hooks: None,
            bypass_acknowledged: None,
        };
        let toml_text = toml::to_string_pretty(&no_decision).unwrap();
        assert!(
            !toml_text.contains("enabled"),
            "no-decision entry must not serialize an `enabled` key:\n{toml_text}"
        );
        assert!(
            !toml_text.contains("project_hooks"),
            "no-decision entry must not serialize a `project_hooks` key:\n{toml_text}"
        );
        assert!(
            !toml_text.contains("bypass_acknowledged"),
            "no-decision entry must not serialize a `bypass_acknowledged` key:\n{toml_text}"
        );

        let explicit_false = IntegrationConfig {
            enabled: Some(false),
            executable: None,
            project_hooks: None,
            bypass_acknowledged: None,
        };
        let toml_text = toml::to_string_pretty(&explicit_false).unwrap();
        assert!(
            toml_text.contains("enabled = false"),
            "explicit disable must serialize `enabled = false`:\n{toml_text}"
        );
    }

    #[test]
    fn enabled_or_returns_recorded_decision_or_supplied_default() {
        let decided = IntegrationConfig {
            enabled: Some(true),
            executable: None,
            project_hooks: None,
            bypass_acknowledged: None,
        };
        assert!(decided.enabled_or(false));

        let declined = IntegrationConfig {
            enabled: Some(false),
            executable: None,
            project_hooks: None,
            bypass_acknowledged: None,
        };
        assert!(!declined.enabled_or(true));

        let undecided = IntegrationConfig::default();
        assert!(undecided.enabled_or(true));
        assert!(!undecided.enabled_or(false));
    }

    // ---------------------------------------------------------------
    // Launch profiles.
    // ---------------------------------------------------------------

    #[test]
    fn the_native_profile_is_always_available_for_every_harness() {
        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, None);

        assert!(
            effective
                .profile_names()
                .contains(&crate::profile::NATIVE_PROFILE_NAME.to_owned())
        );

        let resolved = effective
            .launch_profile(crate::profile::NATIVE_PROFILE_NAME, IntegrationId::Codex)
            .unwrap();
        assert_eq!(resolved.layer, Layer::Default);
        assert_eq!(resolved.value.harness, IntegrationId::Codex);
        assert_eq!(
            resolved.value.backend,
            crate::profile::BackendResource::Native
        );
    }

    /// Phase 2D: "disable is not delete" for launch profiles too — disabling
    /// keeps every other field intact and is reversible without retyping.
    /// Both halves are asserted.
    #[test]
    fn disabling_a_launch_profile_keeps_its_configuration_and_is_reversible() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));

        assert!(
            ProfileConfig::new(IntegrationId::ClaudeCode).enabled(),
            "a freshly created profile is enabled by default"
        );

        let mut profile = ProfileConfig::new(IntegrationId::ClaudeCode);
        profile.set_model(Some("claude-opus".to_owned()));

        let mut user = UserConfig::default();
        let mut disabled = profile.clone();
        disabled.set_enabled(false);
        user.profiles_mut().set("fast", disabled);
        user.save(&paths).unwrap();

        let loaded = UserConfig::load(&paths).unwrap();
        let loaded_profile = loaded.profiles().get("fast").unwrap();
        assert!(!loaded_profile.enabled(), "the profile must be disabled");
        assert_eq!(
            loaded_profile.model(),
            Some("claude-opus"),
            "disabling must not touch the model"
        );
        assert_eq!(loaded_profile.harness_slug(), "claude-code");

        let mut re_enabled = loaded_profile.clone();
        re_enabled.set_enabled(true);
        let mut user = loaded;
        user.profiles_mut().set("fast", re_enabled);
        user.save(&paths).unwrap();
        let reloaded = UserConfig::load(&paths).unwrap();
        let reloaded_profile = reloaded.profiles().get("fast").unwrap();
        assert!(reloaded_profile.enabled());
        assert_eq!(
            reloaded_profile.model(),
            Some("claude-opus"),
            "re-enabling must not have required retyping the model"
        );
    }

    #[test]
    fn an_unknown_profile_name_lists_the_known_names() {
        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, None);

        let err = effective
            .launch_profile("does-not-exist", IntegrationId::ClaudeCode)
            .unwrap_err();
        match err {
            ProfileLookupError::Unknown { name, known } => {
                assert_eq!(name, "does-not-exist");
                assert!(known.contains(&crate::profile::NATIVE_PROFILE_NAME.to_owned()));
            }
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    #[test]
    fn a_project_configured_profile_wins_over_a_user_configured_one_of_the_same_name() {
        let mut user = UserConfig::default();
        user.profiles_mut()
            .set("fast", ProfileConfig::new(IntegrationId::ClaudeCode));

        let mut project = ProjectConfig::default();
        let mut project_profile = ProfileConfig::new(IntegrationId::ClaudeCode);
        project_profile.set_approval(ProfileApproval::AutomaticReview);
        project.profiles_mut().set("fast", project_profile);

        let effective = EffectiveConfig::new(&user, Some(&project));
        let resolved = effective
            .launch_profile("fast", IntegrationId::ClaudeCode)
            .unwrap();
        assert_eq!(resolved.layer, Layer::Project);
        assert_eq!(
            resolved.value.approval,
            crate::profile::ApprovalSelection::AutomaticReview
        );

        let without_project = EffectiveConfig::new(&user, None);
        let resolved = without_project
            .launch_profile("fast", IntegrationId::ClaudeCode)
            .unwrap();
        assert_eq!(resolved.layer, Layer::User);
        assert_eq!(
            resolved.value.approval,
            crate::profile::ApprovalSelection::Default
        );
    }

    #[test]
    fn a_profile_naming_a_different_harness_than_requested_is_refused() {
        let mut user = UserConfig::default();
        user.profiles_mut()
            .set("fast", ProfileConfig::new(IntegrationId::ClaudeCode));
        let effective = EffectiveConfig::new(&user, None);

        let err = effective
            .launch_profile("fast", IntegrationId::Codex)
            .unwrap_err();
        match err {
            ProfileLookupError::HarnessMismatch {
                name,
                profile_harness,
                requested_harness,
            } => {
                assert_eq!(name, "fast");
                assert_eq!(profile_harness, IntegrationId::ClaudeCode);
                assert_eq!(requested_harness, IntegrationId::Codex);
            }
            other => panic!("expected HarnessMismatch, got {other:?}"),
        }
    }

    #[test]
    fn a_profile_naming_an_unknown_harness_slug_is_reported_rather_than_guessed() {
        let mut user = UserConfig::default();
        let mut profile = ProfileConfig::new(IntegrationId::ClaudeCode);
        profile.harness = "not-a-real-harness".to_owned();
        user.profiles_mut().set("broken", profile);
        let effective = EffectiveConfig::new(&user, None);

        let err = effective
            .launch_profile("broken", IntegrationId::ClaudeCode)
            .unwrap_err();
        assert!(matches!(
            err,
            ProfileLookupError::Invalid(ProfileConfigError::UnknownHarness { .. })
        ));
    }

    // ---------------------------------------------------------------
    // Providers.
    // ---------------------------------------------------------------

    #[test]
    fn a_configured_provider_may_override_a_template_base_url() {
        let mut user = UserConfig::default();
        let mut provider = ProviderConfig::new("openrouter");
        provider.set_base_url(Some("https://mirror.example.com/v1".to_owned()));
        user.providers_mut().set("my-openrouter", provider);

        let effective = EffectiveConfig::new(&user, None);
        let resolved = effective.configured_provider("my-openrouter").unwrap();
        assert_eq!(resolved.layer, Layer::User);

        let protocol = resolved
            .value
            .serves(crate::harness::WireProtocol::OpenAiChat)
            .expect("openrouter serves openai-chat");
        assert_eq!(protocol.base_url, "https://mirror.example.com/v1");

        // The unconfigured template still has its own base URL — the
        // override is per configured provider, not global to the template.
        let template = crate::provider::template("openrouter").unwrap();
        let template_protocol = template
            .serves(crate::harness::WireProtocol::OpenAiChat)
            .unwrap();
        assert_eq!(template_protocol.base_url, "https://openrouter.ai/api/v1");
    }

    #[test]
    fn a_configured_provider_without_a_base_url_override_keeps_the_templates_own() {
        let mut user = UserConfig::default();
        user.providers_mut()
            .set("plain-openrouter", ProviderConfig::new("openrouter"));

        let effective = EffectiveConfig::new(&user, None);
        let resolved = effective.configured_provider("plain-openrouter").unwrap();
        let protocol = resolved
            .value
            .serves(crate::harness::WireProtocol::OpenAiChat)
            .unwrap();
        assert_eq!(protocol.base_url, "https://openrouter.ai/api/v1");
        // And the template's own default credential name is kept too, since
        // this configuration declared no override.
        assert_eq!(resolved.value.credential_env, vec!["OPENROUTER_API_KEY"]);
    }

    #[test]
    fn a_provider_may_declare_several_credential_variable_names() {
        let mut user = UserConfig::default();
        let mut provider = ProviderConfig::new("openrouter");
        provider.set_credential_env(vec![
            "OPENROUTER_API_KEY".to_owned(),
            "OPENROUTER_API_KEY_BACKUP".to_owned(),
        ]);
        user.providers_mut().set("multi-key", provider);

        let effective = EffectiveConfig::new(&user, None);
        let resolved = effective.configured_provider("multi-key").unwrap();
        assert_eq!(
            resolved.value.credential_env,
            vec!["OPENROUTER_API_KEY", "OPENROUTER_API_KEY_BACKUP"]
        );
    }

    #[test]
    fn a_provider_naming_an_unknown_template_is_reported_rather_than_guessed() {
        let mut user = UserConfig::default();
        user.providers_mut()
            .set("broken", ProviderConfig::new("not-a-real-template"));
        let effective = EffectiveConfig::new(&user, None);

        let err = effective.configured_provider("broken").unwrap_err();
        assert!(matches!(
            err,
            ProviderLookupError::Invalid(ProviderConfigError::UnknownTemplate { .. })
        ));
    }

    #[test]
    fn an_unknown_provider_name_lists_the_known_names() {
        let mut user = UserConfig::default();
        user.providers_mut()
            .set("configured-one", ProviderConfig::new("openrouter"));
        let effective = EffectiveConfig::new(&user, None);

        let err = effective.configured_provider("does-not-exist").unwrap_err();
        match err {
            ProviderLookupError::Unknown { name, known } => {
                assert_eq!(name, "does-not-exist");
                assert_eq!(known, vec!["configured-one".to_owned()]);
            }
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    #[test]
    fn a_project_configured_provider_wins_over_a_user_configured_one_of_the_same_name() {
        let mut user = UserConfig::default();
        user.providers_mut()
            .set("router", ProviderConfig::new("openrouter"));

        let mut project = ProjectConfig::default();
        let mut project_provider = ProviderConfig::new("openrouter");
        project_provider.set_base_url(Some("https://project-mirror.example.com/v1".to_owned()));
        project.providers_mut().set("router", project_provider);

        let effective = EffectiveConfig::new(&user, Some(&project));
        let resolved = effective.configured_provider("router").unwrap();
        assert_eq!(resolved.layer, Layer::Project);
        let protocol = resolved
            .value
            .serves(crate::harness::WireProtocol::OpenAiChat)
            .unwrap();
        assert_eq!(protocol.base_url, "https://project-mirror.example.com/v1");

        let without_project = EffectiveConfig::new(&user, None);
        let resolved = without_project.configured_provider("router").unwrap();
        assert_eq!(resolved.layer, Layer::User);
    }

    #[test]
    fn provider_table_round_trips_through_save_load() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));

        let mut user = UserConfig::default();
        let mut provider = ProviderConfig::new("zai");
        provider
            .set_base_url(Some("https://mirror.example.com/paas/v4".to_owned()))
            .set_credential_env(vec!["ZAI_API_KEY".to_owned(), "ZAI_API_KEY_2".to_owned()])
            .set_headers(vec![("X-Org-Id".to_owned(), "acme".to_owned())]);
        user.providers_mut().set("my-zai", provider);
        user.save(&paths).unwrap();

        let loaded = UserConfig::load(&paths).unwrap();
        assert_eq!(loaded, user);
        let loaded_provider = loaded.providers().get("my-zai").unwrap();
        assert_eq!(loaded_provider.template(), "zai");
        assert_eq!(
            loaded_provider.base_url(),
            Some("https://mirror.example.com/paas/v4")
        );
        assert_eq!(
            loaded_provider.credential_env(),
            &["ZAI_API_KEY".to_owned(), "ZAI_API_KEY_2".to_owned()]
        );
        assert_eq!(
            loaded_provider.headers(),
            &[("X-Org-Id".to_owned(), "acme".to_owned())]
        );
    }

    /// Phase 2D: "disable is not delete" — disabling a provider keeps every
    /// other field intact and is reversible without retyping anything, and
    /// the decision survives a save/load round trip. Both halves are
    /// asserted: the disabled state, and that nothing else moved.
    #[test]
    fn disabling_a_provider_keeps_its_configuration_and_is_reversible() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));

        let mut user = UserConfig::default();
        assert!(
            ProviderConfig::new("openrouter").enabled(),
            "a freshly created provider is enabled by default"
        );

        let mut provider = ProviderConfig::new("openrouter");
        provider
            .set_base_url(Some("https://mirror.example.com/v1".to_owned()))
            .set_credential_env(vec!["OPENROUTER_API_KEY".to_owned()]);
        user.providers_mut().set("my-router", provider.clone());

        // Disable: the rest of the configuration must not move.
        let mut disabled = provider.clone();
        disabled.set_enabled(false);
        user.providers_mut().set("my-router", disabled.clone());
        user.save(&paths).unwrap();

        let loaded = UserConfig::load(&paths).unwrap();
        let loaded_provider = loaded.providers().get("my-router").unwrap();
        assert!(!loaded_provider.enabled(), "the provider must be disabled");
        assert_eq!(
            loaded_provider.base_url(),
            Some("https://mirror.example.com/v1"),
            "disabling must not touch the base URL"
        );
        assert_eq!(
            loaded_provider.credential_env(),
            &["OPENROUTER_API_KEY".to_owned()],
            "disabling must not touch the credential variable names"
        );

        // Re-enable: reversible without retyping anything already configured.
        let mut re_enabled = loaded_provider.clone();
        re_enabled.set_enabled(true);
        let mut user = loaded;
        user.providers_mut().set("my-router", re_enabled);
        user.save(&paths).unwrap();
        let reloaded = UserConfig::load(&paths).unwrap();
        let reloaded_provider = reloaded.providers().get("my-router").unwrap();
        assert!(reloaded_provider.enabled());
        assert_eq!(
            reloaded_provider.base_url(),
            Some("https://mirror.example.com/v1"),
            "re-enabling must not have required retyping the base URL"
        );
    }

    /// A file written before `enabled` existed has no `enabled` key at all —
    /// it must still load as enabled, not as a parse failure or a silent
    /// disable.
    #[test]
    fn a_provider_with_no_enabled_key_loads_as_enabled() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        std::fs::write(
            paths.user_config_file(),
            "version = 1\n[providers.legacy]\ntemplate = \"openrouter\"\n",
        )
        .unwrap();

        let loaded = UserConfig::load(&paths).unwrap();
        assert!(loaded.providers().get("legacy").unwrap().enabled());
    }

    #[test]
    fn a_configured_base_url_override_is_what_reaches_a_launched_child_process() {
        // Line 423, all the way through: a base-URL override is not just a
        // config-layer value (`a_configured_provider_may_override_a_template_base_url`
        // already proves that) — it is what a real launch actually points
        // the harness at.
        let mut user = UserConfig::default();
        let mut provider_cfg = ProviderConfig::new("openrouter");
        provider_cfg.set_base_url(Some("https://mirror.example.com/api".to_owned()));
        user.providers_mut().set("my-openrouter", provider_cfg);

        let effective = EffectiveConfig::new(&user, None);
        let provider = effective
            .configured_provider("my-openrouter")
            .unwrap()
            .value;

        let adapter = crate::harness::adapter_for(IntegrationId::ClaudeCode).expect("a harness");
        let mut profile = crate::profile::LaunchProfile::native(IntegrationId::ClaudeCode);
        profile.backend = crate::profile::BackendResource::DirectProvider {
            provider: provider.name.clone(),
        };
        let secrets = OneShotSecrets("OPENROUTER_API_KEY", "sk-test-not-a-real-key");
        let resolution = crate::profile::Resolution {
            adapter,
            acknowledged_bypass: false,
            provider: Some(&provider),
            secrets: &secrets,
        };

        let overlay = crate::profile::resolve(&profile, &resolution).expect(
            "a configured openrouter provider now backs Claude Code via anthropic-messages",
        );
        let base_url = overlay
            .env()
            .iter()
            .find(|(key, _)| key == std::ffi::OsStr::new("ANTHROPIC_BASE_URL"))
            .map(|(_, value)| value.to_string_lossy().into_owned())
            .expect("ANTHROPIC_BASE_URL must be set");
        assert_eq!(
            base_url, "https://mirror.example.com/api",
            "the configured override must reach the child, not openrouter's own default \
             (https://openrouter.ai/api)"
        );
    }

    /// Line 353, closed by a test: a `claude-code` profile backed by a
    /// *configured* OpenRouter provider (no override at all) resolves, and
    /// its `ANTHROPIC_BASE_URL` is the root OpenRouter now also serves
    /// Anthropic Messages at — no `/v1`. That suffix is the exact mistake
    /// the reference implementation (`~/projects/openrouter-clis/bin/claude-or`)
    /// had to write a comment about: Claude Code appends `/v1/messages`
    /// itself, so a base URL still carrying `/v1` would double it up.
    #[test]
    fn a_configured_openrouter_provider_backs_claude_code_at_the_v1_less_api_root() {
        let mut user = UserConfig::default();
        user.providers_mut()
            .set("openrouter-configured", ProviderConfig::new("openrouter"));

        let effective = EffectiveConfig::new(&user, None);
        let provider = effective
            .configured_provider("openrouter-configured")
            .unwrap()
            .value;

        let adapter = crate::harness::adapter_for(IntegrationId::ClaudeCode).expect("a harness");
        let mut profile = crate::profile::LaunchProfile::native(IntegrationId::ClaudeCode);
        profile.backend = crate::profile::BackendResource::DirectProvider {
            provider: provider.name.clone(),
        };
        let secrets = OneShotSecrets("OPENROUTER_API_KEY", "sk-test-not-a-real-key");
        let resolution = crate::profile::Resolution {
            adapter,
            acknowledged_bypass: false,
            provider: Some(&provider),
            secrets: &secrets,
        };

        let overlay = crate::profile::resolve(&profile, &resolution)
            .expect("claude-code + a configured openrouter provider must now resolve");
        let base_url = overlay
            .env()
            .iter()
            .find(|(key, _)| key == std::ffi::OsStr::new("ANTHROPIC_BASE_URL"))
            .map(|(_, value)| value.to_string_lossy().into_owned())
            .expect("ANTHROPIC_BASE_URL must be set");
        assert_eq!(base_url, "https://openrouter.ai/api");
        assert!(
            !base_url.ends_with("/v1"),
            "ANTHROPIC_BASE_URL must not carry a /v1 suffix: Claude Code appends \
             /v1/messages itself, so a URL of {base_url:?} would double it up"
        );
    }

    #[test]
    fn a_configured_provider_may_declare_custom_headers_that_reach_the_provider() {
        let mut user = UserConfig::default();
        let mut provider_cfg = ProviderConfig::new("openrouter");
        provider_cfg.set_headers(vec![
            ("X-Org-Id".to_owned(), "acme".to_owned()),
            ("X-Trace".to_owned(), "on".to_owned()),
        ]);
        user.providers_mut().set("headered", provider_cfg);

        let effective = EffectiveConfig::new(&user, None);
        let provider = effective.configured_provider("headered").unwrap().value;
        assert_eq!(
            provider.headers,
            vec![
                ("X-Org-Id".to_owned(), "acme".to_owned()),
                ("X-Trace".to_owned(), "on".to_owned()),
            ]
        );
    }

    #[test]
    fn a_header_name_with_an_unsafe_character_is_refused_and_named() {
        for (name, offending) in [("Bad:Name", ':'), ("Bad Name", ' ')] {
            let mut provider_cfg = ProviderConfig::new("openrouter");
            provider_cfg.set_headers(vec![(name.to_owned(), "value".to_owned())]);

            let err = provider_cfg
                .to_provider("headered")
                .expect_err("an unsafe header name must be refused");
            match &err {
                ProviderConfigError::InvalidHeaderName {
                    header_name,
                    offending: found,
                    ..
                } => {
                    assert_eq!(header_name, name);
                    assert_eq!(*found, offending);
                }
                other => panic!("expected InvalidHeaderName for `{name}`, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_header_value_with_a_control_character_is_refused_and_named() {
        for (value, offending) in [("line-one\r\nline-two", '\r'), ("has\ttab", '\t')] {
            let mut provider_cfg = ProviderConfig::new("openrouter");
            provider_cfg.set_headers(vec![("X-Glasshouse".to_owned(), value.to_owned())]);

            let err = provider_cfg
                .to_provider("headered")
                .expect_err("a control character in a header value must be refused");
            match &err {
                ProviderConfigError::InvalidHeaderValue {
                    header_name,
                    offending: found,
                    ..
                } => {
                    assert_eq!(header_name, "X-Glasshouse");
                    assert_eq!(*found, offending);
                }
                other => panic!("expected InvalidHeaderValue for {value:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_header_carrying_crlf_is_refused_rather_than_escaped() {
        // The concrete injection this whole check exists to stop: a header
        // value containing a newline would otherwise let a configured
        // provider inject a second header of its own choosing into every
        // request Claude Code or Codex sends.
        let mut provider_cfg = ProviderConfig::new("openrouter");
        provider_cfg.set_headers(vec![(
            "X-Glasshouse".to_owned(),
            "legit\r\nX-Injected: evil".to_owned(),
        )]);

        let err = provider_cfg
            .to_provider("headered")
            .expect_err("a newline in a header value must be refused, never escaped");
        assert!(matches!(
            err,
            ProviderConfigError::InvalidHeaderValue { .. }
        ));
    }

    /// Structural guard, not a string search: enumerate every field this
    /// module's config types can hold and assert none of them is
    /// credential-shaped. If a future edit adds a field, this test forces a
    /// conscious look rather than an accidental secret leaking into a
    /// tracked `.glasshouse` file or the user config.
    #[test]
    fn serialized_form_has_no_secret_capable_field() {
        // `IntegrationConfig` has exactly these four fields.
        let cfg = IntegrationConfig {
            enabled: Some(true),
            executable: Some(PathBuf::from("/usr/bin/example")),
            project_hooks: Some(true),
            bypass_acknowledged: Some(true),
        };
        let value = toml::Value::try_from(&cfg).unwrap();
        let table = value.as_table().unwrap();
        let mut keys: Vec<&str> = table.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "bypass_acknowledged",
                "enabled",
                "executable",
                "project_hooks"
            ],
            "IntegrationConfig grew a field — confirm it cannot hold a credential \
             before widening this list"
        );

        // `ProfileConfig` — the other per-item shape this module stores —
        // likewise. `backend`'s `DirectProvider { provider }` payload is a
        // provider *name*, not a credential; there is still no field here
        // that could hold one.
        let mut profile_cfg = ProfileConfig::new(IntegrationId::ClaudeCode);
        profile_cfg
            .set_backend(ProfileBackend::DirectProvider {
                provider: "openrouter".to_owned(),
            })
            .set_model(Some("claude-opus".to_owned()))
            .set_expected_protocol(Some("anthropic-messages".to_owned()))
            .set_approval(ProfileApproval::Bypass)
            // Non-default, so the field actually appears below — see
            // `enabled_by_default`/`is_enabled_by_default`.
            .set_enabled(false);
        let profile_value = toml::Value::try_from(&profile_cfg).unwrap();
        let profile_table = profile_value.as_table().unwrap();
        let mut profile_keys: Vec<&str> = profile_table.keys().map(String::as_str).collect();
        profile_keys.sort_unstable();
        assert_eq!(
            profile_keys,
            vec![
                "approval",
                "backend",
                "enabled",
                "expected_protocol",
                "harness",
                "model"
            ],
            "ProfileConfig grew a field — confirm it cannot hold a credential before \
             widening this list"
        );

        // `ProviderConfig` — the shape that comes closest to a credential,
        // since it is the one a provider's key is configured through. Its
        // `credential_store` holds a `StoredCredentialRef`, which is two
        // NAMES; there is still no field here that could hold a value.
        let mut provider_cfg = ProviderConfig::new("openrouter");
        provider_cfg
            .set_base_url(Some("https://example.invalid".to_owned()))
            .set_credential_env(vec!["OPENROUTER_API_KEY".to_owned()])
            .set_credential_store(Some(StoredCredentialRef::new(
                "glasshouse",
                "OPENROUTER_API_KEY",
            )))
            .set_headers(vec![("X-Test".to_owned(), "1".to_owned())])
            .set_enabled(false);
        let provider_value = toml::Value::try_from(&provider_cfg).unwrap();
        let provider_table = provider_value.as_table().unwrap();
        let mut provider_keys: Vec<&str> = provider_table.keys().map(String::as_str).collect();
        provider_keys.sort_unstable();
        assert_eq!(
            provider_keys,
            vec![
                "base_url",
                "credential_env",
                "credential_store",
                "enabled",
                "headers",
                "template"
            ],
            "ProviderConfig grew a field — confirm it cannot hold a credential before \
             widening this list"
        );
        // ... and the one field that names a secret store really does hold
        // only names.
        let stored = provider_table["credential_store"].as_table().unwrap();
        let mut stored_keys: Vec<&str> = stored.keys().map(String::as_str).collect();
        stored_keys.sort_unstable();
        assert_eq!(
            stored_keys,
            vec!["account", "service"],
            "StoredCredentialRef grew a field — a reference is a service and an account, \
             and nothing else"
        );

        // `UserConfig`'s top level, likewise.
        let user = fully_populated_user_config();
        let user_value = toml::Value::try_from(&user).unwrap();
        let mut user_keys: Vec<&str> = user_value
            .as_table()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        user_keys.sort_unstable();
        assert_eq!(
            user_keys,
            vec![
                "integrations",
                "onboarding",
                "profiles",
                "providers",
                "version"
            ]
        );

        // And the serialized TOML text itself contains none of the names a
        // secret field would plausibly carry, as a cheap extra check on top
        // of the structural one above.
        let serialized = toml::to_string_pretty(&user).unwrap();
        for forbidden in ["key", "token", "secret", "password", "credential"] {
            assert!(
                !serialized.to_lowercase().contains(forbidden),
                "serialized UserConfig unexpectedly contains `{forbidden}`:\n{serialized}"
            );
        }
    }

    /// Structural guard for [`ProviderConfig`] specifically, alongside
    /// [`serialized_form_has_no_secret_capable_field`]'s coverage of the
    /// rest of this module's config types.
    ///
    /// `credential_env` holds environment variable *names* (e.g.
    /// `"OPENROUTER_API_KEY"`), which legitimately contain words like "key"
    /// as part of a name — that is exactly what the field is for. So unlike
    /// the sibling test's broad word-scan (which only ever runs against a
    /// fixture with no provider entries), what proves this type cannot hold
    /// a secret *value* is structural: `credential_env`'s type is
    /// `Vec<String>` of names, and this list pins that `ProviderConfig` has
    /// no field beyond that, `base_url`, and `template` — nothing shaped to
    /// carry an actual credential.
    #[test]
    fn no_provider_type_can_hold_a_credential_value() {
        let mut provider_cfg = ProviderConfig::new("openrouter");
        provider_cfg
            .set_base_url(Some("https://mirror.example.com/v1".to_owned()))
            .set_credential_env(vec!["OPENROUTER_API_KEY".to_owned()])
            // Set, so the field is actually serialized and this list pins
            // it: `skip_serializing_if = "Option::is_none"` means an unset
            // one would be invisible here and the guard would pass without
            // ever having seen it.
            .set_credential_store(Some(StoredCredentialRef::new(
                "glasshouse",
                "OPENROUTER_API_KEY",
            )))
            .set_headers(vec![("X-Org-Id".to_owned(), "acme".to_owned())])
            // Non-default, so the field actually appears below — see
            // `enabled_by_default`/`is_enabled_by_default`.
            .set_enabled(false);
        let provider_value = toml::Value::try_from(&provider_cfg).unwrap();
        let provider_table = provider_value.as_table().unwrap();
        let mut provider_keys: Vec<&str> = provider_table.keys().map(String::as_str).collect();
        provider_keys.sort_unstable();
        assert_eq!(
            provider_keys,
            vec![
                "base_url",
                "credential_env",
                "credential_store",
                "enabled",
                "headers",
                "template"
            ],
            "ProviderConfig grew a field — confirm it cannot hold a credential value \
             (as opposed to a variable name) before widening this list. `headers` holds \
             name/value pairs that are themselves configuration, never a credential — see \
             ProviderConfig::set_headers's own doc for why that is safe; \
             `credential_store` holds a service and an account NAME — see \
             StoredCredentialRef."
        );

        // `ProviderTable` itself adds nothing beyond the map: every entry it
        // can hold is one of the five fields just checked.
        let mut table = ProviderTable::default();
        table.set("mine", provider_cfg);
        let table_value = toml::Value::try_from(&table).unwrap();
        let entry = table_value.as_table().unwrap().get("mine").unwrap();
        let mut entry_keys: Vec<&str> = entry
            .as_table()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        entry_keys.sort_unstable();
        assert_eq!(
            entry_keys,
            vec![
                "base_url",
                "credential_env",
                "credential_store",
                "enabled",
                "headers",
                "template"
            ]
        );
    }

    /// Acceptance 4: a `SecretRef` naming an OS credential survives a real
    /// save/load round trip through a configuration file, and the file's own
    /// text carries no value.
    ///
    /// A known credential is planted in the environment variable the
    /// reference is named after *and* handed to the store, so a
    /// serialisation that reached for either would be caught. Asserted with
    /// `!contains`, never `assert_eq!` on the secret material — a failing
    /// `assert_eq!` prints both sides.
    #[test]
    fn an_os_credential_reference_round_trips_through_configuration_without_its_value() {
        const VAR: &str = "GLASSHOUSE_CONFIG_TEST_ONLY_STORED_VAR";
        const VALUE: &str = "sk-config-round-trip-test-0123456789abcdef";

        let tmp = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));

        let reference = crate::secret::native::os_credential_for_variable(VAR);
        let stored = StoredCredentialRef::from_secret_ref(&reference)
            .expect("an OsCredential reference has a stored shape");

        let mut provider = ProviderConfig::new("openrouter");
        provider
            .set_credential_env(vec![VAR.to_owned()])
            .set_credential_store(Some(stored.clone()));

        let mut user = UserConfig::default();
        user.providers_mut().set("stored", provider);

        // SAFETY: `VAR` is unique to this test and removed again below.
        // Planted so that a serializer which resolved the reference — the
        // failure this test exists to catch — would have something to leak.
        unsafe {
            std::env::set_var(VAR, VALUE);
        }
        let saved = user.save(&paths);
        unsafe {
            std::env::remove_var(VAR);
        }
        saved.unwrap();

        let path = paths.config_dir().join("config.toml");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            !text.contains(VALUE),
            "a credential value reached the configuration file:\n{text}"
        );
        assert!(text.contains(VAR), "the NAME must be there:\n{text}");
        assert!(text.contains("glasshouse"), "got:\n{text}");

        let loaded = UserConfig::load(&paths).unwrap();
        let loaded_provider = loaded.providers().get("stored").unwrap();
        assert_eq!(loaded_provider.credential_store(), Some(&stored));
        assert_eq!(loaded_provider.credential_store().unwrap().account(), VAR);
        assert_eq!(
            loaded_provider.credential_store().unwrap().to_secret_ref(),
            reference,
            "the stored shape and the reference it came from must be the same thing"
        );

        // An environment reference has no stored shape — writing one here
        // would claim something about where a key lives that nobody
        // established.
        assert_eq!(
            StoredCredentialRef::from_secret_ref(&crate::secret::SecretRef::Environment {
                var: VAR.to_owned()
            }),
            None
        );
    }
}
