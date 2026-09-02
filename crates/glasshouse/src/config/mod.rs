//! User-level and optional project-level Glasshouse configuration.
//!
//! Two files, same small shape:
//!
//! - `<config_dir>/config.toml` — user-level. Onboarding decisions and
//!   per-integration enable/executable overrides. Loaded by every run;
//!   never created automatically for you to lose data to — a missing file
//!   just means the defaults apply (see [`UserConfig::load`]).
//! - `<project root>/.glasshouse/config.toml` — project-level, optional,
//!   and layered *over* the user file (see [`EffectiveConfig`]). It is
//!   never written except in response to an explicit user decision — see
//!   [`write_project_config_with_consent`].
//!
//! The schema is deliberately tiny. The capability map is explicit that
//! configuration should stay small until real usage demonstrates a need for
//! more (Phase 49): a field belongs here once a user can actually make the
//! decision it records, and not before. [`RoutingConfig`] is the newest such
//! addition and shows where the line is — it stores *which* routing model
//! the user picked in the first-run wizard, plus the bounded routing-policy
//! preferences the Phase 2D settings view lets them change. It deliberately
//! stores no health observations, live prices, or fallback decisions: those
//! belong to the later router that consumes these preferences. Phase 9A's launch
//! profiles are the same shape: [`ProfileTable`] holds
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
//! `tests::serialized_form_has_no_secret_capable_field` for a structural
//! guard, not just a string search.

pub mod capability;
pub mod firewall;
pub mod pairing;
pub mod response;

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::guardrails::{self, BlockingCategory, GuardrailMode};
use crate::integrations::IntegrationId;
use crate::paths::RuntimePaths;
use crate::project::{Project, ScopeError};
use crate::secret::SecretRef;

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
    ///
    /// The rendering goes through [`crate::secret::redact`], and the inner
    /// error is deliberately **not** `#[source]`: `toml`'s own `Display`
    /// quotes the whole offending line of the file under a caret, and
    /// `main.rs` prints this with `{err:#}`, which walks the chain. A file
    /// that carried a pasted key on the line that failed to parse would
    /// otherwise copy it to stderr and into `glasshouse.log` — the case
    /// `crate::secret::redact` documents itself as existing for.
    #[error(
        "configuration file `{path}` is not valid TOML: {}",
        crate::secret::redact(&.source.to_string())
    )]
    Parse {
        path: PathBuf,
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
    /// (see [`UserConfig::load`] / [`load_project_config`]) still succeeds — refusing to
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
    /// Phase 9H line 518: pin a gateway-backed session started through this
    /// profile to the provider it is assigned, and turn automatic failover
    /// off. See [`crate::profile::LaunchProfile::pin_gateway_backend`] for
    /// why the pin lives on the profile rather than on a live command.
    ///
    /// A boolean, so a file written before this field existed loads as "not
    /// pinned" — which is the behaviour those files already had.
    #[serde(default, skip_serializing_if = "is_false")]
    pin_gateway_backend: bool,
    /// Line 353's sixth axis: a named [`crate::profile::response::Preset`]
    /// this profile asks for, or unset for a profile that says nothing about
    /// communication policy and leaves the response profile to whatever the
    /// session's other layers (role, project, user default) decide — see
    /// [`crate::config::response::PrecedenceLayer`].
    ///
    /// `None` here, on a file written before this field existed, loads as
    /// "this profile names no preset" — the behaviour those files already
    /// had.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    response_preset: Option<String>,
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
    #[error(
        "launch profile `{name}` names response preset `{preset}`, which this build does not \
         know; the presets are: {}",
        crate::profile::response::preset_names()
    )]
    UnknownResponsePreset { name: String, preset: String },
}

impl ProfileConfig {
    pub fn new(harness: IntegrationId) -> Self {
        Self {
            harness: harness.slug().to_owned(),
            backend: ProfileBackend::default(),
            model: None,
            expected_protocol: None,
            approval: ProfileApproval::default(),
            pin_gateway_backend: false,
            response_preset: None,
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

    /// Whether a gateway-backed session started through this profile is
    /// pinned to its assigned provider — Phase 9H line 518.
    pub fn pin_gateway_backend(&self) -> bool {
        self.pin_gateway_backend
    }

    pub fn set_pin_gateway_backend(&mut self, pinned: bool) -> &mut Self {
        self.pin_gateway_backend = pinned;
        self
    }

    /// The named response preset this profile asks for — line 353's sixth
    /// axis.
    pub fn response_preset(&self) -> Option<&str> {
        self.response_preset.as_deref()
    }

    pub fn set_response_preset(&mut self, preset: Option<String>) -> &mut Self {
        self.response_preset = preset;
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

        // Validated against the real preset table now, the same way
        // `expected_protocol` is validated against real protocols above,
        // rather than waiting for a launch to discover a typo.
        if let Some(preset) = &self.response_preset
            && crate::profile::response::preset(preset).is_none()
        {
            return Err(ProfileConfigError::UnknownResponsePreset {
                name: name.to_owned(),
                preset: preset.clone(),
            });
        }

        Ok(crate::profile::LaunchProfile {
            name: name.to_owned(),
            harness,
            backend,
            model: self.model.clone(),
            expected_protocol,
            approval,
            pin_gateway_backend: self.pin_gateway_backend,
            response_preset: self.response_preset.clone(),
        })
    }
}

/// A `bool` that is only worth writing when it is `true`.
///
/// Used by `serde`'s `skip_serializing_if` so that an unpinned profile — which
/// is every profile that has never been pinned — serialises to exactly what it
/// did before the field existed.
fn is_false(value: &bool) -> bool {
    !*value
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

/// A [`crate::routing::classify::WorkloadTier`] as it is written in a
/// configuration file, and the only place this crate turns a spelling back
/// into that type.
///
/// # Why a newtype rather than `serde` on `WorkloadTier` itself
///
/// `WorkloadTier` is a routing type with no serialised form of its own —
/// `routing::request` parses one out of a routing model's JSON answer, and
/// giving the enum a `Deserialize` impl would make that answer and a user's
/// config file the same surface. They are not: one is untrusted output from a
/// model, the other is a file the user wrote. This newtype is the config
/// file's side of that boundary and nothing else reads it.
///
/// # Why the spellings come from `as_str` rather than a second list
///
/// `WORKLOAD_TIER_SPELLINGS` holds every variant, and
/// `workload_tier_ordinal`'s exhaustive `match` is what makes adding a
/// sixth variant a **compile error** here rather than a spelling that
/// silently fails to parse. The strings themselves are always
/// `WorkloadTier::as_str`'s, so a renamed tier renames its config spelling
/// with it and cannot drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfiguredWorkloadTier(crate::routing::classify::WorkloadTier);

/// Every [`crate::routing::classify::WorkloadTier`], in the type's own order.
/// Kept complete by [`workload_tier_ordinal`].
const WORKLOAD_TIER_SPELLINGS: [crate::routing::classify::WorkloadTier; 5] = {
    use crate::routing::classify::WorkloadTier as T;
    [
        T::Deterministic,
        T::Leaf,
        T::Standard,
        T::Heavy,
        T::Frontier,
    ]
};

/// Where a tier sits in [`WORKLOAD_TIER_SPELLINGS`]. The `match` is
/// exhaustive on purpose: it is the compile-time guard that the array above
/// still lists every variant, and `every_workload_tier_spelling_round_trips`
/// is the run-time half that checks the two agree.
///
/// `#[cfg(test)]` because the guard is the `match` itself and nothing in the
/// shipped binary needs an ordinal. It is still a real gate: the local gate
/// and `cargo clippy --all-targets` both compile this module's tests, so a
/// sixth [`crate::routing::classify::WorkloadTier`] variant fails the build
/// there rather than becoming a spelling no configuration file can name.
#[cfg(test)]
fn workload_tier_ordinal(tier: crate::routing::classify::WorkloadTier) -> usize {
    use crate::routing::classify::WorkloadTier as T;
    match tier {
        T::Deterministic => 0,
        T::Leaf => 1,
        T::Standard => 2,
        T::Heavy => 3,
        T::Frontier => 4,
    }
}

impl ConfiguredWorkloadTier {
    pub fn new(tier: crate::routing::classify::WorkloadTier) -> Self {
        Self(tier)
    }

    pub fn tier(self) -> crate::routing::classify::WorkloadTier {
        self.0
    }

    /// The spelling a user writes, which is the tier's own `as_str`.
    pub fn as_str(self) -> &'static str {
        self.0.as_str()
    }

    /// The tier a spelling names, or `None` for one no variant answers to.
    /// Case-sensitive and untrimmed: a configuration value is compared
    /// exactly as written, the same way `ProviderConfig::cost_of` compares a
    /// model name, so a value that does not parse is reported rather than
    /// guessed at.
    pub fn parse(text: &str) -> Option<Self> {
        WORKLOAD_TIER_SPELLINGS
            .into_iter()
            .find(|tier| tier.as_str() == text)
            .map(Self)
    }
}

impl Serialize for ConfiguredWorkloadTier {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ConfiguredWorkloadTier {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).ok_or_else(|| {
            let known = WORKLOAD_TIER_SPELLINGS
                .into_iter()
                .map(|tier| tier.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            serde::de::Error::custom(format!(
                "unknown workload tier `{text}` — expected one of: {known}"
            ))
        })
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
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "deserialize_credential_env_names"
    )]
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
    /// Model identifiers on this provider the user has marked free-tier or
    /// zero-marginal-cost — Phase 9I lines 527 and 531. **Names only,
    /// exactly like [`ProviderConfig::credential_env`]'s variable names.**
    ///
    /// Unmarked means metered. There is no inference here — no prefix match
    /// on `:free`, no heuristic — because [`crate::routing::Cost::Metered`]
    /// is the fail-closed default: a router that guessed a model was free
    /// and was wrong spends the user's money. See
    /// [`ProviderConfig::cost_of`], the one place that answer is computed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    free_models: Vec<String>,
    /// Model identifiers on this provider the user has explicitly named as
    /// eligible for metered (paid) fallback on Glasshouse's own bounded
    /// support work — the decision recorded in
    /// `docs/product/design-decisions.md`, *"Metered capacity for background
    /// jobs"*: ordinary support work may spend quota as a last resort when no
    /// free resource can serve.
    ///
    /// **This list is the control, not a switch beside it.** Empty (the
    /// default) means no metered fallback for this provider — the coherent
    /// off state, and the only honest default: Glasshouse never invents a
    /// model name, so a provider nobody named a paid model for contributes
    /// nothing metered. Naming one here is the user's decision already made;
    /// nothing above this list asks permission again. **Names only**, exactly
    /// like [`ProviderConfig::free_models`] — a model in both lists resolves
    /// through [`ProviderConfig::cost_of`], which still answers `Free` for
    /// it, because [`ProviderConfig::free_models`] is checked first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    metered_models: Vec<String>,
    /// Whether this provider's protocols carry tool calls — capability map
    /// line 1513's tool-semantics half, and the producer
    /// [`ProviderConfig::to_provider`] applies to every
    /// [`crate::provider::ProtocolSupport`] this provider serves.
    ///
    /// `None` (the default) leaves every protocol's `tool_calls` exactly as
    /// its own template declares — every built-in template's own answer is
    /// [`crate::harness::Declared::Unverified`] — so a provider nobody has
    /// said anything about excludes nothing new. `Some(value)` is the
    /// user's own word: [`ProviderConfig::to_provider`] turns it into
    /// [`crate::harness::Declared::verified`], citing the `[providers.<name>]`
    /// table it came from — the user, having read their own provider's
    /// documentation, is the verifier here, exactly as a `--help` line is
    /// the verifier for an adapter's own declarations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool_calls: Option<bool>,
    /// Per-model resource facts the user has declared for this provider —
    /// capability map line 1517's producer, keyed by the same model
    /// identifier [`ProviderConfig::model_ceilings`] uses, and read through
    /// [`ProviderConfig::resource_facts_of`].
    ///
    /// A model absent from this map, or an axis absent from its own table,
    /// stays [`crate::harness::Declared::Unverified`] — the same "nobody has
    /// said" rule [`ProviderConfig::model_ceilings`]'s own doc states, and
    /// for the same reason: an empty declaration must never read as an
    /// established absence.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    model_facts: BTreeMap<String, ConfiguredModelFacts>,
    /// The highest workload tier the user is willing to trust an individual
    /// model on this provider with — capability map line 1796, and the one
    /// production producer of [`crate::routing::session::Destination::with_tier_ceiling`].
    ///
    /// Keyed by the same model identifier [`ProviderConfig::free_models`] and
    /// [`ProviderConfig::metered_models`] name, and valued by
    /// [`crate::routing::classify::WorkloadTier`]'s own spellings —
    /// `deterministic`, `leaf`, `standard`, `heavy`, `frontier` — parsed
    /// through [`ConfiguredWorkloadTier`], which refuses an unknown spelling
    /// at load rather than silently reading it as no ceiling at all. A
    /// misspelt ceiling that read as absent would be exactly the failure this
    /// project keeps paying for: an empty result indistinguishable from
    /// success (practice §68).
    ///
    /// **A model absent from this map has no ceiling, and that is not a low
    /// one.** `super::routing::session::hard_constraint` rejects only a
    /// destination whose ceiling is *established* below the task's required
    /// tier; "nobody has said" is never "cannot". So the empty default — every
    /// project that has not configured this — leaves every destination exactly
    /// as eligible as it was before this field existed.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    model_ceilings: BTreeMap<String, ConfiguredWorkloadTier>,
    /// Calibrated model-capability records — Phase 34F, capability map
    /// lines 1475–1479 and 1482–1485, widening [`ProviderConfig::model_ceilings`]
    /// to the rest of that neighbourhood rather than duplicating it. Keyed
    /// by the same model identifier `model_ceilings` uses; this provider
    /// entry is the `backend` axis capability map line 1482 asks calibration
    /// to stay local to. See [`capability::ModelCapabilityRecord`] for the
    /// record's other narrowing fields and [`capability::resolve_ceiling`]
    /// for how its ceiling and this field's own override reconcile.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    model_capabilities: BTreeMap<String, capability::ModelCapabilityRecord>,
    /// A prompt transformation this backend performs on the way through —
    /// Phase 9K line 609.
    ///
    /// **Backend metadata a person records, never something Glasshouse does.**
    /// Line 608 forbids gateway-side system-prompt rewriting from being the
    /// default way a response profile is applied, and nothing in Glasshouse
    /// writes this field: `crate::harness::response::apply` cannot reach a
    /// gateway at all. What this exists for is the case where a user's own
    /// gateway or router already rewrites prompts, and a session's
    /// instructions therefore arrive at the model altered by something
    /// Glasshouse did not do. `glasshouse response` surfaces it with that
    /// warning attached, which is the whole of line 609: an unsurfaced
    /// transformation is exactly how a harness's own instructions get
    /// silently rewritten and nobody can tell.
    ///
    /// Free text, because it describes somebody else's software.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prompt_transform: Option<String>,
    /// What the user told Glasshouse about this provider's quota when the
    /// provider's own telemetry does not say — capability map lines 1233,
    /// 1203 and 1237.
    ///
    /// As safe to write into a tracked project file as
    /// [`ProviderConfig::credential_env`]'s variable names: a plan name, an
    /// integer number of microdollars, and an integer number of seconds.
    /// Nothing here is resolved through [`crate::secret`] and nothing here
    /// names a credential.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    quota: Option<QuotaOverride>,
}

/// How long one provider's quota telemetry stays current — capability map
/// line 1237's "provider-specific configurable age".
///
/// Seconds, as a human-editable integer, matching
/// [`RouterCostMicroUsd`]'s own reasoning about exactness in policy. There is
/// no one right value and that is the point of the line: a credit balance
/// changes when somebody pays a bill and a requests-per-minute ceiling is a
/// contract that changes when a plan does, so the same age would be wrong for
/// both on the same provider, let alone across providers.
///
/// The default is fifteen minutes. It is a *default*, not a claim about any
/// provider — long enough that a resource view opened twice in a row does not
/// call a reading from a minute ago stale, short enough that a balance read
/// before lunch is not still presented as current after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct QuotaStaleAfterSeconds(u32);

impl QuotaStaleAfterSeconds {
    /// Thirty days. A ceiling rather than a policy: an age limit longer than
    /// this describes a reading nobody should be routing on under any
    /// definition, and accepting `u32::MAX` silently would let a typo disable
    /// staleness entirely without saying so.
    pub const MAX: u32 = 30 * 24 * 60 * 60;
    pub const DEFAULT: Self = Self(15 * 60);

    pub fn get(self) -> u32 {
        self.0
    }

    /// As the `i64` [`crate::provider::quota::Reading::freshness`] takes.
    pub fn seconds(self) -> i64 {
        i64::from(self.0)
    }
}

impl TryFrom<u32> for QuotaStaleAfterSeconds {
    type Error = QuotaValueError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        if value <= Self::MAX {
            Ok(Self(value))
        } else {
            Err(QuotaValueError::StaleAfter {
                seconds: value,
                max_seconds: Self::MAX,
            })
        }
    }
}

impl From<QuotaStaleAfterSeconds> for u32 {
    fn from(value: QuotaStaleAfterSeconds) -> Self {
        value.0
    }
}

/// The period a monetary budget covers — capability map line 1203's
/// "monthly **or rolling**".
///
/// Two answers because they are genuinely different promises: a calendar
/// month's budget is spent and forgiven on the first, and a rolling window's
/// is never fully forgiven at all. Nothing in Glasshouse counts spend against
/// either yet — see [`MonetaryBudget`] — but a ceiling recorded without its
/// period is a number that cannot be checked, and this project does not store
/// those.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BudgetPeriod {
    /// Resets at the start of each calendar month.
    CalendarMonth,
    /// A trailing thirty days that never fully resets.
    RollingThirtyDays,
}

impl BudgetPeriod {
    pub fn as_str(self) -> &'static str {
        match self {
            BudgetPeriod::CalendarMonth => "calendar month",
            BudgetPeriod::RollingThirtyDays => "rolling thirty days",
        }
    }
}

/// A spending ceiling the user set for one metered provider — capability map
/// line 1203.
///
/// # Not [`RouterCostMicroUsd`], and the difference is the whole line
///
/// [`RouterCostMicroUsd`] caps the price of **one routing decision**. This
/// caps **cumulative spend over a period**. A user who set the first to a
/// tenth of a cent has said nothing at all about how many such calls they are
/// willing to pay for in a month, which is why Phase 32A recorded that the
/// existing field does not satisfy this line.
///
/// # What it does not do, stated rather than implied
///
/// Nothing in Glasshouse counts money spent. This is the ceiling half of
/// capability map line 1209 and the ceiling half only: it reaches
/// [`crate::provider::quota::CapacityState::user_budget`] as the pool's
/// *limit*, with the remaining half left unmeasured, so a resource view can
/// honestly say "you set a ten dollar monthly ceiling and Glasshouse does not
/// know what you have spent against it" instead of implying a balance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonetaryBudget {
    #[serde(rename = "amount_micro_usd")]
    amount_micro_usd: u64,
    period: BudgetPeriod,
}

impl MonetaryBudget {
    /// Ten thousand dollars. A unit-mistake guard in the same spirit as
    /// [`RouterCostMicroUsd::MAX`]: somebody who writes `1000` meaning ten
    /// dollars has made an error this cannot catch, but somebody who writes
    /// a dollar figure where microdollars belong is off by a million and this
    /// does.
    pub const MAX_MICRO_USD: u64 = 10_000 * 1_000_000;

    pub fn new(amount_micro_usd: u64, period: BudgetPeriod) -> Result<Self, QuotaValueError> {
        if amount_micro_usd > Self::MAX_MICRO_USD {
            return Err(QuotaValueError::Budget {
                micro_usd: amount_micro_usd,
                max_micro_usd: Self::MAX_MICRO_USD,
            });
        }
        Ok(Self {
            amount_micro_usd,
            period,
        })
    }

    pub fn amount_micro_usd(self) -> u64 {
        self.amount_micro_usd
    }

    pub fn period(self) -> BudgetPeriod {
        self.period
    }
}

/// What the user told Glasshouse about one provider's quota when its own
/// telemetry does not say — capability map lines 1233, 1203 and 1237.
///
/// Every field is optional and absent means "the user said nothing", never
/// "the user said none": a provider with no `[providers.x.quota]` table at
/// all and one with an empty table are the same, and neither asserts that the
/// provider has no plan.
///
/// This is configuration, so everything here is [`Layer`]-resolved the same
/// way every other provider field is, and everything here becomes a
/// [`crate::provider::quota::ReadingSource::UserConfiguration`] reading —
/// which [`crate::provider::quota::Capacity::prefer`] then ranks *below* a
/// provider's or a harness's own word, per capability map line 1228. A user's
/// manual entry fills a gap; it does not override a measurement.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaOverride {
    /// The plan this account is on, as the provider names it — `"max"`,
    /// `"pro"`, `"team"`. Capability map line 1233's "known plan".
    ///
    /// Free text, for [`crate::provider::quota::KnownPlan`]'s own reason: every
    /// vendor names its own tiers and a closed enumeration here would be
    /// wrong within a quarter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    plan: Option<String>,
    /// A cumulative spending ceiling for this provider — capability map
    /// line 1203.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    budget: Option<MonetaryBudget>,
    /// How long this provider's telemetry stays current — capability map
    /// line 1237. `None` means [`QuotaStaleAfterSeconds::DEFAULT`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stale_after_seconds: Option<QuotaStaleAfterSeconds>,
    /// This provider's own protected reserve percentage — capability map
    /// line 1288, Phase 32F's *"allow each premium resource to define a
    /// protected reserve percentage"*. Reuses [`PremiumReservePercent`]
    /// rather than a second 0–100 type: it is the same question
    /// (`crate::provider::quota::CapacityBandThresholds::with_resource_reserve`
    /// is where this reaches the band the packet's design decision 6 asks
    /// for), asked per provider instead of once globally.
    /// [`EffectiveConfig::reserve_percent`]'s own default is
    /// [`EffectiveConfig::premium_reserve`] — the existing global routing
    /// preference — when a provider states none of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reserve_percent: Option<PremiumReservePercent>,
}

impl QuotaOverride {
    pub fn plan(&self) -> Option<&str> {
        self.plan.as_deref()
    }

    pub fn set_plan(&mut self, plan: Option<String>) -> &mut Self {
        self.plan = plan;
        self
    }

    pub fn budget(&self) -> Option<MonetaryBudget> {
        self.budget
    }

    pub fn set_budget(&mut self, budget: Option<MonetaryBudget>) -> &mut Self {
        self.budget = budget;
        self
    }

    /// This layer's configured age, or `None` for "never decided".
    pub fn stale_after(&self) -> Option<QuotaStaleAfterSeconds> {
        self.stale_after_seconds
    }

    pub fn set_stale_after(&mut self, value: Option<QuotaStaleAfterSeconds>) -> &mut Self {
        self.stale_after_seconds = value;
        self
    }

    /// This provider's own protected reserve percentage, or `None` for
    /// "never decided" — capability map line 1288.
    pub fn reserve_percent(&self) -> Option<PremiumReservePercent> {
        self.reserve_percent
    }

    pub fn set_reserve_percent(&mut self, value: Option<PremiumReservePercent>) -> &mut Self {
        self.reserve_percent = value;
        self
    }

    /// Whether the user recorded anything at all here.
    pub fn is_empty(&self) -> bool {
        self.plan.is_none()
            && self.budget.is_none()
            && self.stale_after_seconds.is_none()
            && self.reserve_percent.is_none()
    }
}

/// A quota configuration value outside the range its field accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum QuotaValueError {
    #[error(
        "a quota staleness age of {seconds}s is longer than the maximum {max_seconds}s; a          reading older than that should not be routed on under any policy"
    )]
    StaleAfter { seconds: u32, max_seconds: u32 },

    #[error(
        "a monetary budget of {micro_usd} microdollars is above the maximum {max_micro_usd};          this field is in millionths of a US dollar, so a plain dollar figure here is off by a          million"
    )]
    Budget { micro_usd: u64, max_micro_usd: u64 },

    /// Capability map line 1270: capacity-band thresholds must be ascending
    /// and are refused outright rather than sorted into shape. Wraps
    /// [`crate::provider::quota::CapacityBandThresholdsError`] rather than
    /// repeating its fields, so the two can never say different things about
    /// the same refusal.
    #[error("{0}")]
    BandThresholds(#[from] crate::provider::quota::CapacityBandThresholdsError),
}

/// The raw, on-disk shape [`CapacityBandThresholdsConfig`] validates itself
/// out of. A private intermediate rather than a public one: nothing outside
/// `serde`'s `try_from` machinery should ever hold an unvalidated set of
/// thresholds.
#[derive(Debug, Clone, Copy, Deserialize)]
struct RawCapacityBandThresholds {
    exhausted_percent: u8,
    reserve_percent: u8,
    tight_percent: u8,
    healthy_percent: u8,
}

/// User-configurable capacity-band thresholds — capability map line 1270.
///
/// Four ascending percentages, validated as one unit at deserialization time
/// via `#[serde(try_from = "RawCapacityBandThresholds")]` — the same
/// fail-closed idiom [`QuotaStaleAfterSeconds`], [`RouterCostMicroUsd`] and
/// [`PremiumReservePercent`] already use for a single field, applied here
/// across four so a non-monotonic set is refused at `UserConfig::load` /
/// `load_project_config` time rather than sorted silently or discovered only
/// when a band is computed. See
/// [`crate::provider::quota::CapacityBandThresholds`], the domain type this
/// converts to via [`CapacityBandThresholdsConfig::to_domain`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawCapacityBandThresholds")]
pub struct CapacityBandThresholdsConfig {
    exhausted_percent: u8,
    reserve_percent: u8,
    tight_percent: u8,
    healthy_percent: u8,
}

impl TryFrom<RawCapacityBandThresholds> for CapacityBandThresholdsConfig {
    type Error = QuotaValueError;

    fn try_from(raw: RawCapacityBandThresholds) -> Result<Self, Self::Error> {
        crate::provider::quota::CapacityBandThresholds::new(
            raw.exhausted_percent,
            raw.reserve_percent,
            raw.tight_percent,
            raw.healthy_percent,
        )?;
        Ok(Self {
            exhausted_percent: raw.exhausted_percent,
            reserve_percent: raw.reserve_percent,
            tight_percent: raw.tight_percent,
            healthy_percent: raw.healthy_percent,
        })
    }
}

impl From<crate::provider::quota::CapacityBandThresholds> for CapacityBandThresholdsConfig {
    /// A domain value is already known-monotonic by its own constructor, so
    /// this is a plain field copy rather than a second validation pass.
    fn from(domain: crate::provider::quota::CapacityBandThresholds) -> Self {
        Self {
            exhausted_percent: domain.exhausted_percent(),
            reserve_percent: domain.reserve_percent(),
            tight_percent: domain.tight_percent(),
            healthy_percent: domain.healthy_percent(),
        }
    }
}

impl CapacityBandThresholdsConfig {
    /// The validated domain value — see
    /// [`crate::provider::quota::CapacityBandThresholds::band_for_percent`].
    pub fn to_domain(self) -> crate::provider::quota::CapacityBandThresholds {
        crate::provider::quota::CapacityBandThresholds::new(
            self.exhausted_percent,
            self.reserve_percent,
            self.tight_percent,
            self.healthy_percent,
        )
        .expect("validated once already at deserialization")
    }
}

/// A `[routing.score_weights]` value with a non-finite field — capability map
/// lines 1357/1358's own "fail closed" requirement, stated in code rather
/// than clamped around, the same shape
/// [`crate::provider::quota::CapacityBandThresholdsError`] states for its
/// four fields.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
#[error(
    "routing score weights must be finite numbers (quota_pressure_weight {quota_pressure_weight}, \
     health_failure_penalty {health_failure_penalty}, health_penalty_floor {health_penalty_floor}, \
     health_unavailable_penalty {health_unavailable_penalty}); refusing rather than substituting a \
     value nobody wrote"
)]
pub struct ScoreWeightsError {
    pub quota_pressure_weight: f64,
    pub health_failure_penalty: f64,
    pub health_penalty_floor: f64,
    pub health_unavailable_penalty: f64,
}

/// The raw, on-disk shape [`ScoreWeightsConfig`] validates itself out of. A
/// private intermediate rather than a public one, for
/// [`RawCapacityBandThresholds`]'s own reason: nothing outside `serde`'s
/// `try_from` machinery should ever hold an unvalidated set of weights.
#[derive(Debug, Clone, Copy, Deserialize)]
struct RawScoreWeights {
    quota_pressure_weight: f64,
    health_failure_penalty: f64,
    health_penalty_floor: f64,
    health_unavailable_penalty: f64,
}

/// User-configurable routing score weights — capability map lines 1357/1358:
/// the four weights [`crate::routing::session::ScoreWeights`]'s doc comment
/// names are an observed starting policy, not a universal constant, and this
/// is where a user overrides them.
///
/// Validated as one unit at deserialization time via
/// `#[serde(try_from = "RawScoreWeights")]` — the same fail-closed idiom
/// [`CapacityBandThresholdsConfig`] uses, refusing a non-finite field (`NaN`
/// or infinite) outright rather than substituting a default silently, so a
/// malformed config file is refused at `UserConfig::load` /
/// `load_project_config` time rather than producing a routing decision
/// nobody could predict. Unlike the capacity-band thresholds, there is no
/// ordering between these four fields to enforce: each prices an independent
/// term (line 1598's quota weight and the three line-1599 health terms), so
/// finiteness is the whole of what "fail closed" means here.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawScoreWeights")]
pub struct ScoreWeightsConfig {
    quota_pressure_weight: f64,
    health_failure_penalty: f64,
    health_penalty_floor: f64,
    health_unavailable_penalty: f64,
}

/// Sound because `TryFrom<RawScoreWeights>` is the only way to build one and
/// refuses every non-finite field: `f64::eq`'s single deviation from an
/// equivalence relation is `NaN != NaN`, and no `ScoreWeightsConfig` can ever
/// hold one. `RoutingConfig` derives `Eq` across every field it carries,
/// [`CapacityBandThresholdsConfig`] among them, and this is what lets a
/// `[routing.score_weights]` value sit beside it rather than forcing that
/// derive apart.
impl Eq for ScoreWeightsConfig {}

impl TryFrom<RawScoreWeights> for ScoreWeightsConfig {
    type Error = ScoreWeightsError;

    fn try_from(raw: RawScoreWeights) -> Result<Self, Self::Error> {
        if raw.quota_pressure_weight.is_finite()
            && raw.health_failure_penalty.is_finite()
            && raw.health_penalty_floor.is_finite()
            && raw.health_unavailable_penalty.is_finite()
        {
            Ok(Self {
                quota_pressure_weight: raw.quota_pressure_weight,
                health_failure_penalty: raw.health_failure_penalty,
                health_penalty_floor: raw.health_penalty_floor,
                health_unavailable_penalty: raw.health_unavailable_penalty,
            })
        } else {
            Err(ScoreWeightsError {
                quota_pressure_weight: raw.quota_pressure_weight,
                health_failure_penalty: raw.health_failure_penalty,
                health_penalty_floor: raw.health_penalty_floor,
                health_unavailable_penalty: raw.health_unavailable_penalty,
            })
        }
    }
}

impl From<crate::routing::session::ScoreWeights> for ScoreWeightsConfig {
    /// A domain value is already known-finite by its own constructor (or is
    /// [`crate::routing::session::ScoreWeights::default`], which is a
    /// compile-time constant), so this is a plain field copy rather than a
    /// second validation pass — the same reasoning
    /// [`CapacityBandThresholdsConfig`]'s own `From` impl states.
    fn from(domain: crate::routing::session::ScoreWeights) -> Self {
        Self {
            quota_pressure_weight: domain.quota_pressure_weight,
            health_failure_penalty: domain.health_failure_penalty,
            health_penalty_floor: domain.health_penalty_floor,
            health_unavailable_penalty: domain.health_unavailable_penalty,
        }
    }
}

impl ScoreWeightsConfig {
    /// The validated domain value — see
    /// [`crate::routing::session::ScoreWeights`].
    pub fn to_domain(self) -> crate::routing::session::ScoreWeights {
        crate::routing::session::ScoreWeights {
            quota_pressure_weight: self.quota_pressure_weight,
            health_failure_penalty: self.health_failure_penalty,
            health_penalty_floor: self.health_penalty_floor,
            health_unavailable_penalty: self.health_unavailable_penalty,
        }
    }
}

/// `[routing.reserve]` — capability map line 1577: the reserve policy for
/// interactive work and the one for background support jobs, recorded
/// separately.
///
/// Each field is `None` for "this layer never recorded one", matching
/// [`RoutingConfig::model`]'s reasoning, and the two resolve **per field**
/// through [`EffectiveConfig::reserve_policy`] — a project that sets only
/// `background` leaves `interactive` to the user layer and then the default,
/// exactly as [`CapacityBandThresholdsConfig`]'s siblings do. The values are
/// [`crate::routing::pressure::ReservePolicy`]'s own, deserialized directly,
/// so there is no second spelling of `protect`/`spend` to keep in step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ReservePoliciesConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    interactive: Option<crate::routing::pressure::ReservePolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    background: Option<crate::routing::pressure::ReservePolicy>,
}

impl ReservePoliciesConfig {
    pub fn interactive(&self) -> Option<crate::routing::pressure::ReservePolicy> {
        self.interactive
    }

    pub fn set_interactive(
        &mut self,
        value: Option<crate::routing::pressure::ReservePolicy>,
    ) -> &mut Self {
        self.interactive = value;
        self
    }

    pub fn background(&self) -> Option<crate::routing::pressure::ReservePolicy> {
        self.background
    }

    pub fn set_background(
        &mut self,
        value: Option<crate::routing::pressure::ReservePolicy>,
    ) -> &mut Self {
        self.background = value;
        self
    }

    /// This layer's recorded policy for `scope`, or `None` for "never
    /// decided" — the one place the scope selects a field.
    pub fn for_scope(
        &self,
        scope: crate::routing::pressure::ReserveScope,
    ) -> Option<crate::routing::pressure::ReservePolicy> {
        match scope {
            crate::routing::pressure::ReserveScope::Interactive => self.interactive,
            crate::routing::pressure::ReserveScope::Background => self.background,
        }
    }
}

// ---------------------------------------------------------------------------
// Phase 56/56A — `[entitlements.<name>]`: an entitlement — a specific
// subscription or API-credit account — as the configured unit of capacity,
// with rules of its own (map lines 1946, 1947, 1954, 1962, 1963, 1973).
// ---------------------------------------------------------------------------

/// Which plan an entitlement is — map line 1946's four: *"a Claude,
/// ChatGPT/Codex, or Gemini plan, or an API key"*.
///
/// Descriptive, and read by exactly one consumer: the launch announcement
/// that says which entitlement will serve a session. No rule depends on it —
/// [`EntitlementConfig`]'s rules are about harnesses, tiers and job kinds,
/// never about what kind of plan is paying — so a wrong `kind` misdescribes
/// an entitlement and never misroutes one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EntitlementKind {
    Claude,
    #[serde(rename = "chatgpt")]
    ChatGpt,
    Gemini,
    ApiKey,
}

impl EntitlementKind {
    /// The spelling a configuration file uses.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::ChatGpt => "chatgpt",
            Self::Gemini => "gemini",
            Self::ApiKey => "api-key",
        }
    }

    /// How the announcement names the plan.
    pub fn describe(self) -> &'static str {
        match self {
            Self::Claude => "Claude plan",
            Self::ChatGpt => "ChatGPT plan",
            Self::Gemini => "Gemini plan",
            Self::ApiKey => "API key",
        }
    }
}

/// The billing vendor behind an entitlement — map line 1962's *"distinct
/// from the vendor"*: the account that pays is one fact, who bills it is
/// another, and two entitlements of one vendor are still two accounts.
///
/// Descriptive, like [`EntitlementKind`], and read by the same one consumer:
/// the launch announcement ([`ResolvedEntitlement::describe`]). **No rule and
/// no resolution step keys on it** — map line 1963's coexistence is the point,
/// and nothing anywhere dedupes entitlements by vendor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EntitlementVendor {
    Claude,
    #[serde(rename = "openai")]
    OpenAi,
    Google,
    #[serde(rename = "openrouter")]
    OpenRouter,
    /// Any vendor the four names above do not cover — a self-hosted router,
    /// a reseller, an employer's own gateway.
    Custom,
}

impl EntitlementVendor {
    /// The spelling a configuration file uses.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::OpenAi => "openai",
            Self::Google => "google",
            Self::OpenRouter => "openrouter",
            Self::Custom => "custom",
        }
    }
}

/// An entitlement's own authentication — map lines 1962 and 1973: **a
/// reference, never a value**, in exactly [`crate::secret::SecretRef`]'s two
/// shapes.
///
/// ```toml
/// credential = { env = "CLAUDE_A_OAUTH_TOKEN" }            # an environment variable NAME
/// credential = { service = "glasshouse", account = "a" }   # an OS-credential reference
/// ```
///
/// The `Deserialize` impl is manual so that nothing else can ever parse: a
/// bare string is refused with a sentence naming the rule — and deliberately
/// **without echoing what was written**, because the one thing a value-shaped
/// mistake must not do is copy the value into an error message — and a map
/// carrying any other key (`value`, `token`, `key`, …) is refused by that
/// key's name. This is the config-file side of Phase 9E's boundary; the
/// serde impls live here and not on [`SecretRef`] itself because
/// `crate::secret`'s own tests hold that module to naming no serde at all.
#[derive(Clone, PartialEq, Eq)]
pub struct EntitlementCredential(SecretRef);

impl EntitlementCredential {
    pub fn environment(var: impl Into<String>) -> Self {
        Self(SecretRef::Environment { var: var.into() })
    }

    pub fn os_credential(service: impl Into<String>, account: impl Into<String>) -> Self {
        Self(SecretRef::OsCredential {
            service: service.into(),
            account: account.into(),
        })
    }

    /// The reference this credential names. A caller resolves it through a
    /// [`crate::secret::SecretStore`] at the moment of use, never earlier.
    pub fn secret_ref(&self) -> &SecretRef {
        &self.0
    }
}

/// Names only — the variable's, the service's, the account's — exactly what
/// [`SecretRef`]'s own `Debug` prints. Manual so the shape is pinned by
/// `tests/entitlement_pool.rs` rather than drifting with a derive.
impl std::fmt::Debug for EntitlementCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            SecretRef::Environment { var } => write!(f, "environment variable `{var}`"),
            SecretRef::OsCredential { service, account } => {
                write!(f, "OS credential `{service}`/`{account}`")
            }
        }
    }
}

/// The sentence every value-shaped mistake gets. One spelling, no echo.
const CREDENTIAL_IS_A_REFERENCE: &str = "an entitlement credential is a reference, never a value: \
     write `credential = { env = \"VAR_NAME\" }` for an environment variable, or `credential = \
     { service = \"...\", account = \"...\" }` for the operating system's credential store. A \
     secret does not belong in a configuration file, so what was written here is not repeated";

/// The sentence a value pasted into the `env` slot gets — the same mistake
/// as a bare string, one nesting level deeper, and refused the same way:
/// by the rule's name, never by repeating what was written.
const ENV_NAME_IS_NOT_A_VALUE: &str = "an entitlement credential's `env` is the NAME of an \
     environment variable, not its value: a name may use letters, digits and `_` only and may \
     not start with a digit, and what was written here is neither a name nor repeated";

/// Whether `name` can be an environment variable name at all.
///
/// The portable (POSIX) character set, deliberately narrower than what
/// `std::env::var_os` would accept: every credential shape
/// [`crate::secret::redact`] knows about — `sk-`, `sk-or-v1-`, `ghp_` with
/// its dots, a JWT's `.` and `=` — carries a character this refuses, so a
/// value pasted where a name belongs is caught by shape rather than by
/// guessing at prefixes.
fn is_environment_variable_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Deserializes [`ProviderConfig::credential_env`], refusing any entry that
/// cannot be an environment variable name — the same shape check
/// [`EntitlementCredential`]'s `env` applies, and the same hole: this field
/// is documented as "names only — never a value" but nothing enforced it, so
/// a pasted key would be stored verbatim and later copied wherever this list
/// is rendered.
fn deserialize_credential_env_names<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error as _;
    let names: Vec<String> = Vec::deserialize(deserializer)?;
    for name in &names {
        if !is_environment_variable_name(name) {
            return Err(D::Error::custom(ENV_NAME_IS_NOT_A_VALUE));
        }
    }
    Ok(names)
}

impl Serialize for EntitlementCredential {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        match &self.0 {
            SecretRef::Environment { var } => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("env", var)?;
                map.end()
            }
            SecretRef::OsCredential { service, account } => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("service", service)?;
                map.serialize_entry("account", account)?;
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for EntitlementCredential {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ReferenceOnly;

        impl<'de> serde::de::Visitor<'de> for ReferenceOnly {
            type Value = EntitlementCredential;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(CREDENTIAL_IS_A_REFERENCE)
            }

            // Every non-map shape lands in one of these, and none of them
            // repeats what it was handed.
            fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<Self::Value, E> {
                Err(E::custom(CREDENTIAL_IS_A_REFERENCE))
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut access: A,
            ) -> Result<Self::Value, A::Error> {
                use serde::de::Error as _;
                let mut env: Option<String> = None;
                let mut service: Option<String> = None;
                let mut account: Option<String> = None;
                while let Some(key) = access.next_key::<String>()? {
                    match key.as_str() {
                        "env" => env = Some(access.next_value()?),
                        "service" => service = Some(access.next_value()?),
                        "account" => account = Some(access.next_value()?),
                        other => {
                            return Err(A::Error::custom(format!(
                                "an entitlement credential does not take a key named \
                                 `{other}` — {CREDENTIAL_IS_A_REFERENCE}"
                            )));
                        }
                    }
                }
                match (env, service, account) {
                    (Some(var), None, None) => {
                        if !is_environment_variable_name(&var) {
                            return Err(A::Error::custom(ENV_NAME_IS_NOT_A_VALUE));
                        }
                        Ok(EntitlementCredential::environment(var))
                    }
                    (None, Some(service), Some(account)) => {
                        Ok(EntitlementCredential::os_credential(service, account))
                    }
                    (Some(_), _, _) => Err(A::Error::custom(
                        "an entitlement credential names `env` alone, or `service` and \
                         `account` together — not both shapes at once",
                    )),
                    (None, _, _) => Err(A::Error::custom(
                        "an entitlement credential names `env` alone, or `service` and \
                         `account` together",
                    )),
                }
            }
        }

        deserializer.deserialize_any(ReferenceOnly)
    }
}

/// A harness as it is written in a `[entitlements]` rule — the
/// [`IntegrationId::slug`], parsed against the **harnesses** this build
/// knows. A local inference runtime (`ollama`, `llama-cpp`) or the terminal
/// multiplexer is refused by the loader: an entitlement serves a harness,
/// and a rule naming something that is not one would be a rule nothing can
/// ever match — the silent kind of wrong this project keeps finding.
///
/// The same newtype-over-a-routing-type shape as [`ConfiguredWorkloadTier`],
/// for the same reason: `IntegrationId` has no serialised form of its own,
/// and this is the config file's side of that boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfiguredHarness(IntegrationId);

impl ConfiguredHarness {
    pub fn new(id: IntegrationId) -> Self {
        Self(id)
    }

    pub fn id(self) -> IntegrationId {
        self.0
    }

    pub fn as_str(self) -> &'static str {
        self.0.slug()
    }

    /// Every integration an entitlement can serve, in presentation order.
    fn harnesses() -> impl Iterator<Item = IntegrationId> {
        IntegrationId::ALL
            .iter()
            .copied()
            .filter(|id| id.kind() == crate::integrations::IntegrationKind::Harness)
    }

    /// The harness a slug names, or `None` for one that is not a harness.
    /// Exact, like [`ConfiguredWorkloadTier::parse`].
    pub fn parse(text: &str) -> Option<Self> {
        Self::harnesses().find(|id| id.slug() == text).map(Self)
    }
}

impl Serialize for ConfiguredHarness {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ConfiguredHarness {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).ok_or_else(|| {
            let known = Self::harnesses()
                .map(IntegrationId::slug)
                .collect::<Vec<_>>()
                .join(", ");
            serde::de::Error::custom(format!(
                "unknown harness `{text}` — an entitlement rule names one of: {known}"
            ))
        })
    }
}

/// A [`crate::routing::disposable::JobKind`] as it is written in a
/// `[entitlements]` rule — the spelling is the kind's own `as_str`, and
/// `JOB_KIND_SPELLINGS` is kept complete by `job_kind_ordinal`'s
/// exhaustive `match`, exactly as [`ConfiguredWorkloadTier`] is kept honest
/// by `workload_tier_ordinal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfiguredJobKind(crate::routing::disposable::JobKind);

/// Every [`crate::routing::disposable::JobKind`], in the type's own order.
/// Kept complete by `job_kind_ordinal`.
const JOB_KIND_SPELLINGS: [crate::routing::disposable::JobKind; 5] = {
    use crate::routing::disposable::JobKind as J;
    [
        J::Classification,
        J::MemoryExtraction,
        J::Reranking,
        J::Evaluation,
        J::ContextReduction,
    ]
};

/// The compile-time guard that `JOB_KIND_SPELLINGS` still lists every
/// variant — see `workload_tier_ordinal` for why this is `#[cfg(test)]`
/// and still a real gate.
#[cfg(test)]
fn job_kind_ordinal(kind: crate::routing::disposable::JobKind) -> usize {
    use crate::routing::disposable::JobKind as J;
    match kind {
        J::Classification => 0,
        J::MemoryExtraction => 1,
        J::Reranking => 2,
        J::Evaluation => 3,
        J::ContextReduction => 4,
    }
}

impl ConfiguredJobKind {
    pub fn new(kind: crate::routing::disposable::JobKind) -> Self {
        Self(kind)
    }

    pub fn kind(self) -> crate::routing::disposable::JobKind {
        self.0
    }

    pub fn as_str(self) -> &'static str {
        self.0.as_str()
    }

    /// Exact, like [`ConfiguredWorkloadTier::parse`].
    pub fn parse(text: &str) -> Option<Self> {
        JOB_KIND_SPELLINGS
            .into_iter()
            .find(|kind| kind.as_str() == text)
            .map(Self)
    }
}

impl Serialize for ConfiguredJobKind {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ConfiguredJobKind {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).ok_or_else(|| {
            let known = JOB_KIND_SPELLINGS
                .into_iter()
                .map(|kind| kind.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            serde::de::Error::custom(format!(
                "unknown job kind `{text}` — expected one of: {known}"
            ))
        })
    }
}

/// A [`crate::routing::evidence::HeadroomBand`] as it is written in a
/// configuration file — map line 1252's override. Same shape and same
/// reason as [`ConfiguredWorkloadTier`] just above: `HeadroomBand` is a
/// routing type this crate derives from evidence it reads itself, and
/// giving it a `Deserialize` impl directly would make that derived value and
/// a user's typed-in correction the same surface. This newtype is the
/// config file's side of that boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfiguredHeadroomBand(crate::routing::evidence::HeadroomBand);

/// Every [`crate::routing::evidence::HeadroomBand`], in the type's own
/// presentation order — the same order `main.rs::entitlement_facets` renders
/// them in.
const HEADROOM_BAND_SPELLINGS: [crate::routing::evidence::HeadroomBand; 4] = {
    use crate::routing::evidence::HeadroomBand as B;
    [B::Exhausted, B::Low, B::Moderate, B::Ample]
};

fn headroom_band_spelling(band: crate::routing::evidence::HeadroomBand) -> &'static str {
    use crate::routing::evidence::HeadroomBand as B;
    match band {
        B::Exhausted => "exhausted",
        B::Low => "low",
        B::Moderate => "moderate",
        B::Ample => "ample",
    }
}

impl ConfiguredHeadroomBand {
    pub fn new(band: crate::routing::evidence::HeadroomBand) -> Self {
        Self(band)
    }

    pub fn band(self) -> crate::routing::evidence::HeadroomBand {
        self.0
    }

    /// The spelling a user writes.
    pub fn as_str(self) -> &'static str {
        headroom_band_spelling(self.0)
    }

    /// The band a spelling names, or `None` for one no variant answers to.
    /// Case-sensitive and untrimmed, the same discipline
    /// [`ConfiguredWorkloadTier::parse`] applies.
    pub fn parse(text: &str) -> Option<Self> {
        HEADROOM_BAND_SPELLINGS
            .into_iter()
            .find(|band| headroom_band_spelling(*band) == text)
            .map(Self)
    }
}

impl Serialize for ConfiguredHeadroomBand {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ConfiguredHeadroomBand {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).ok_or_else(|| {
            let known = HEADROOM_BAND_SPELLINGS
                .into_iter()
                .map(headroom_band_spelling)
                .collect::<Vec<_>>()
                .join(", ");
            serde::de::Error::custom(format!(
                "unknown headroom band `{text}` — expected one of: {known}"
            ))
        })
    }
}

/// One configured entitlement — a specific subscription or API-credit
/// account, the unit of capacity — as stored in an `[entitlements.<name>]`
/// table. Map lines 1946, 1947, 1962 and 1963.
///
/// The entitlement's *name* is its key in [`EntitlementTable`], as a
/// provider's is in [`ProviderTable`]. What it is (`kind`), who bills it
/// (`vendor`), what backs it (`native_harness` **or** `provider`, never
/// both), its own authentication (`credential` — a **reference**, never a
/// value), and six rule lists in three allow/deny pairs.
///
/// # The five layers, and which field is which (map line 1964)
///
/// An entitlement sits in a stack of five separately replaceable layers,
/// and this entry deliberately owns only its own two:
///
/// 1. **harness** — [`IntegrationId`], chosen per launch profile
///    ([`crate::profile::LaunchProfile::harness`]); an entitlement's rules
///    may refuse one, but the choice is the user's.
/// 2. **protocol adapter** — [`crate::harness::WireProtocol`], declared by
///    the provider template a backing names, never by this entry.
/// 3. **authentication** — the `credential` reference on this entry: which
///    key or token proves the account. Two entries of one vendor differ
///    here and nowhere else, and that is enough to make them two accounts.
/// 4. **entitlement** — this entry itself: the named account whose capacity
///    is spent.
/// 5. **inference model** — [`crate::profile::LaunchProfile::model`], again
///    per profile.
///
/// Replacing any one layer leaves the other four standing: the same
/// entitlement can serve two harnesses, the same harness can run under two
/// entitlements, the same entitlement can serve two models, and one vendor
/// and protocol can stand behind two credentials — which is what makes the
/// entitlement, not the vendor or the harness, the unit of capacity.
///
/// # Backing
///
/// `native_harness = "claude-code"` says *this entry is Claude Code's own
/// sign-in* — the resource `crate::provider::registry::ResourceKind::
/// NativeSubscription` describes — and replaces the default entry
/// [`EffectiveConfig::entitlements`] would otherwise supply for that
/// harness. Such an entry carries **no `credential` of its own**
/// ([`EntitlementLookupError::NativeSignInWithOwnCredential`]): the harness
/// authenticates itself, and what the registry's `NativeSubscription` names
/// is exactly *one shape of an entitlement, not the shape*. `provider =
/// "<name>"` says *this entry is the account behind that configured
/// provider*, which is how an API key becomes an entitlement with rules.
/// Naming both is refused when resolved
/// ([`EntitlementLookupError::TwoBackings`]). Naming neither is allowed: an
/// account with its own `credential` and no backing is a pool member —
/// listed by [`EffectiveConfig::entitlement_resources`], carrying its own
/// capacity and reset slots — that no launch profile charges yet; the 56A
/// broker packages are what will place work on it.
///
/// # Rules
///
/// Resolved by [`crate::routing::EntitlementRules`] and nowhere else: deny
/// wins over allow, an empty allow-list admits everything not denied. The
/// spellings are the routing types' own — [`IntegrationId::slug`],
/// [`crate::routing::classify::WorkloadTier::as_str`],
/// [`crate::routing::disposable::JobKind::as_str`] — through the three
/// `Configured*` newtypes above, so an unknown spelling is refused by the
/// loader rather than read as "no rule".
///
/// `deny_unknown_fields` is load-bearing for the same reason those newtypes
/// are, one level up: the fields are plural, `deny_harness` for
/// `deny_harnesses` is the natural typo, and a rule that silently does not
/// exist is not a cosmetic default — an empty deny-list *admits*. The
/// forward-compatibility story `ConfigError::UnsupportedVersion` tells is
/// about `version`, and it already refuses to *write* a file it does not
/// understand; refusing to read a rule it does not understand is the same
/// fail-closed choice applied to the one table that grants capacity.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntitlementConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kind: Option<EntitlementKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    vendor: Option<EntitlementVendor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    credential: Option<EntitlementCredential>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    native_harness: Option<ConfiguredHarness>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    allow_harnesses: Vec<ConfiguredHarness>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    deny_harnesses: Vec<ConfiguredHarness>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    allow_tiers: Vec<ConfiguredWorkloadTier>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    deny_tiers: Vec<ConfiguredWorkloadTier>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    allow_job_kinds: Vec<ConfiguredJobKind>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    deny_job_kinds: Vec<ConfiguredJobKind>,
    /// Map line 1971's fourth axis: the cumulative token spend past which
    /// this entitlement may not be charged. Absent means *the user stated
    /// no ceiling*, never *zero*, exactly as every other absent field in
    /// this table does.
    ///
    /// **Tokens, not money, and that is not this field's own decision.**
    /// `routing_observations.cost_micro_usd` has one producer now — map line
    /// 1307, `main.rs::record_entitlement_fallback` — but it writes only on
    /// an entitlement-fallback event, so a ceiling stated in money could
    /// almost never be reached and the broker could almost never be held to
    /// it — see [`crate::routing::evidence::CredentialSpend`], and map line
    /// 1465's reader, which already answers the same question the same way
    /// in production. `[providers.<name>.quota] budget` remains the money
    /// ceiling (map line 1203) and remains, by its own documentation,
    /// uncounted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    spend_ceiling_tokens: Option<u64>,
    /// Map line 1252: a user's own correction of an obviously incorrect
    /// subscription-headroom estimate. Authoritative over the derived
    /// band the moment it is set — that is the whole point of the line —
    /// but [`main.rs::entitlement_facets`] renders it in its own distinct
    /// vocabulary ("your reading", never the confidence-and-basis phrasing
    /// the derived estimate uses) so a substitution is never silent.
    /// Expressed as a [`crate::routing::evidence::HeadroomBand`], the same
    /// vocabulary the estimate itself uses — never a percentage or a token
    /// figure, so 1250/1251's honesty rules are not weakened by the one
    /// value a person, not evidence, supplies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    headroom_override: Option<ConfiguredHeadroomBand>,
    /// Map line 1255: skip the subscription-headroom estimator for this
    /// entitlement entirely, for a user who wants only authoritative usage
    /// data. `false` on a file written before this field existed, the same
    /// "absent reads as off" contract [`ProfileConfig::pin_gateway_backend`]
    /// already keeps. Per-entitlement rather than global: two entitlements
    /// in one config can disagree, which is exactly what
    /// `tests/subscription_estimator.rs`'s 1255 acceptance test proves side
    /// by side. Disabling touches nothing else this entry renders —
    /// `capacity`, `reset`, `throttling` and `models` are populated earlier
    /// in [`ResolvedEntitlement::populate_provider_facets`] and this field
    /// is read only afterward, to skip the estimator call alone.
    #[serde(default, skip_serializing_if = "is_false")]
    disable_headroom_estimate: bool,
}

impl EntitlementConfig {
    pub fn kind(&self) -> Option<EntitlementKind> {
        self.kind
    }

    pub fn set_kind(&mut self, value: Option<EntitlementKind>) -> &mut Self {
        self.kind = value;
        self
    }

    pub fn vendor(&self) -> Option<EntitlementVendor> {
        self.vendor
    }

    pub fn set_vendor(&mut self, value: Option<EntitlementVendor>) -> &mut Self {
        self.vendor = value;
        self
    }

    pub fn credential(&self) -> Option<&EntitlementCredential> {
        self.credential.as_ref()
    }

    pub fn set_credential(&mut self, value: Option<EntitlementCredential>) -> &mut Self {
        self.credential = value;
        self
    }

    pub fn native_harness(&self) -> Option<IntegrationId> {
        self.native_harness.map(ConfiguredHarness::id)
    }

    pub fn set_native_harness(&mut self, value: Option<IntegrationId>) -> &mut Self {
        self.native_harness = value.map(ConfiguredHarness::new);
        self
    }

    pub fn provider(&self) -> Option<&str> {
        self.provider.as_deref()
    }

    pub fn set_provider(&mut self, value: Option<String>) -> &mut Self {
        self.provider = value;
        self
    }

    pub fn set_allow_harnesses(
        &mut self,
        value: impl IntoIterator<Item = IntegrationId>,
    ) -> &mut Self {
        self.allow_harnesses = value.into_iter().map(ConfiguredHarness::new).collect();
        self
    }

    pub fn set_deny_harnesses(
        &mut self,
        value: impl IntoIterator<Item = IntegrationId>,
    ) -> &mut Self {
        self.deny_harnesses = value.into_iter().map(ConfiguredHarness::new).collect();
        self
    }

    pub fn set_allow_tiers(
        &mut self,
        value: impl IntoIterator<Item = crate::routing::classify::WorkloadTier>,
    ) -> &mut Self {
        self.allow_tiers = value.into_iter().map(ConfiguredWorkloadTier::new).collect();
        self
    }

    pub fn set_deny_tiers(
        &mut self,
        value: impl IntoIterator<Item = crate::routing::classify::WorkloadTier>,
    ) -> &mut Self {
        self.deny_tiers = value.into_iter().map(ConfiguredWorkloadTier::new).collect();
        self
    }

    pub fn set_allow_job_kinds(
        &mut self,
        value: impl IntoIterator<Item = crate::routing::disposable::JobKind>,
    ) -> &mut Self {
        self.allow_job_kinds = value.into_iter().map(ConfiguredJobKind::new).collect();
        self
    }

    pub fn set_deny_job_kinds(
        &mut self,
        value: impl IntoIterator<Item = crate::routing::disposable::JobKind>,
    ) -> &mut Self {
        self.deny_job_kinds = value.into_iter().map(ConfiguredJobKind::new).collect();
        self
    }

    /// Map line 1971's spend ceiling, in tokens, or `None` for *none
    /// stated*.
    pub fn spend_ceiling_tokens(&self) -> Option<u64> {
        self.spend_ceiling_tokens
    }

    pub fn set_spend_ceiling_tokens(&mut self, value: Option<u64>) -> &mut Self {
        self.spend_ceiling_tokens = value;
        self
    }

    /// Map line 1252's user override, or `None` for *the estimator's own
    /// reading stands*.
    pub fn headroom_override(&self) -> Option<crate::routing::evidence::HeadroomBand> {
        self.headroom_override.map(ConfiguredHeadroomBand::band)
    }

    pub fn set_headroom_override(
        &mut self,
        value: Option<crate::routing::evidence::HeadroomBand>,
    ) -> &mut Self {
        self.headroom_override = value.map(ConfiguredHeadroomBand::new);
        self
    }

    /// Map line 1255: `true` when this entitlement asked to skip the
    /// subscription-headroom estimator entirely.
    pub fn disable_headroom_estimate(&self) -> bool {
        self.disable_headroom_estimate
    }

    pub fn set_disable_headroom_estimate(&mut self, value: bool) -> &mut Self {
        self.disable_headroom_estimate = value;
        self
    }

    /// This entry's six lists and its spend ceiling as the router's one
    /// rules value.
    pub fn rules(&self) -> crate::routing::EntitlementRules {
        crate::routing::EntitlementRules::UNRESTRICTED
            .allow_harnesses(self.allow_harnesses.iter().map(|h| h.id()))
            .deny_harnesses(self.deny_harnesses.iter().map(|h| h.id()))
            .allow_tiers(self.allow_tiers.iter().map(|t| t.tier()))
            .deny_tiers(self.deny_tiers.iter().map(|t| t.tier()))
            .allow_job_kinds(self.allow_job_kinds.iter().map(|k| k.kind()))
            .deny_job_kinds(self.deny_job_kinds.iter().map(|k| k.kind()))
            .with_spend_ceiling_tokens(self.spend_ceiling_tokens)
    }

    /// The resolved value, named `name` — the key this entry was stored
    /// under — and attributed to `layer`.
    pub fn to_resolved(
        &self,
        name: &str,
        layer: Layer,
    ) -> Result<ResolvedEntitlement, EntitlementLookupError> {
        let backing = match (self.native_harness, &self.provider) {
            (Some(_), Some(_)) => {
                return Err(EntitlementLookupError::TwoBackings {
                    name: name.to_owned(),
                });
            }
            (Some(harness), None) => EntitlementBacking::NativeHarness(harness.id()),
            (None, Some(provider)) => EntitlementBacking::Provider(provider.clone()),
            (None, None) => EntitlementBacking::Unstated,
        };
        // A harness's own sign-in authenticates through the harness itself;
        // an entry claiming to be one while naming its own credential would
        // be two accounts wearing one name — map line 1973's isolation,
        // refused rather than resolved by guessing which authentication
        // counts.
        if matches!(backing, EntitlementBacking::NativeHarness(_)) && self.credential.is_some() {
            return Err(EntitlementLookupError::NativeSignInWithOwnCredential {
                name: name.to_owned(),
            });
        }
        Ok(ResolvedEntitlement {
            name: name.to_owned(),
            kind: self.kind,
            vendor: self.vendor,
            credential: self.credential.as_ref().map(|c| c.secret_ref().clone()),
            backing,
            rules: self.rules(),
            layer,
            remaining_capacity: None,
            seconds_until_reset: None,
            capacity_scope: None,
            throttling: None,
            models: None,
            spend: None,
            headroom_estimate: None,
            headroom_override: self.headroom_override(),
            disable_headroom_estimate: self.disable_headroom_estimate,
        })
    }
}

/// A map of configured entitlements, keyed by name — `[entitlements.<name>]`.
///
/// Configuration, never a credential store: see [`EntitlementConfig`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntitlementTable(BTreeMap<String, EntitlementConfig>);

impl EntitlementTable {
    pub fn get(&self, name: &str) -> Option<&EntitlementConfig> {
        self.0.get(name)
    }

    pub fn set(&mut self, name: impl Into<String>, config: EntitlementConfig) {
        self.0.insert(name.into(), config);
    }

    pub fn remove(&mut self, name: &str) -> Option<EntitlementConfig> {
        self.0.remove(name)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &EntitlementConfig)> {
        self.0.iter().map(|(name, cfg)| (name.as_str(), cfg))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// What a resolved entitlement is backed by — the resource it stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntitlementBacking {
    /// A harness's own first-party sign-in
    /// ([`crate::profile::BackendResource::Native`] on that harness).
    NativeHarness(IntegrationId),
    /// The account behind a configured `[providers.<name>]` entry
    /// ([`crate::profile::BackendResource::DirectProvider`] naming it).
    Provider(String),
    /// The entry names neither. Listed, never matched, never charged.
    Unstated,
}

impl EntitlementBacking {
    /// What pays for this account, as the **router** may branch on it — map
    /// line 1970's *"subscription to subscription to API credits"*, and the
    /// user's ruling of 2026-08-31: *"A api key or a subscription isn't that
    /// the distinction?"* It is, and the distinction is already structural
    /// here rather than a field somebody typed:
    /// [`Self::NativeHarness`] authenticates **through the harness**, which
    /// is a subscription, and [`Self::Provider`] carries a credential of its
    /// own, which is an API key. The loader **enforces** the separation —
    /// an entry that is both is refused as
    /// [`EntitlementLookupError::NativeSignInWithOwnCredential`], map line
    /// 1973's isolation rule — so nothing here is a guess.
    ///
    /// This is why [`EntitlementKind`]'s invariant survives Phase 56A step
    /// 5 intact: routing branches on the *backing*, never on the *kind*.
    pub fn source(&self) -> crate::routing::EntitlementSource {
        match self {
            Self::NativeHarness(_) => crate::routing::EntitlementSource::Subscription,
            Self::Provider(_) => crate::routing::EntitlementSource::ApiCredits,
            Self::Unstated => crate::routing::EntitlementSource::Unstated,
        }
    }
}

/// An entitlement as configuration resolved it — [`EntitlementConfig`] with
/// its name, its layer, and its rules already turned into the router's
/// [`crate::routing::EntitlementRules`].
///
/// [`Self::to_routing`] is the bridge to the value a
/// `crate::routing::session::Destination` carries, and it drops everything
/// the router does not decide on: the kind, the vendor, the credential
/// reference, the backing and the layer stay here, where the announcement
/// and the launch path that read them live.
///
/// `PartialEq` without `Eq`: the remaining-capacity slot is a score over an
/// `f64` once 56A package 2 populates it.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedEntitlement {
    name: String,
    kind: Option<EntitlementKind>,
    vendor: Option<EntitlementVendor>,
    /// This account's own authentication — a **reference**, never a value.
    /// Safe to hold and to `Debug` because every field of a [`SecretRef`] is
    /// a name.
    credential: Option<SecretRef>,
    backing: EntitlementBacking,
    rules: crate::routing::EntitlementRules,
    layer: Layer,
    /// Map line 1963's remaining-capacity slot — map line 1965's producer is
    /// [`Self::with_telemetry`], which populates it from a gateway-captured
    /// per-provider reading. `None` until that resolver runs, and `None`
    /// thereafter for an entitlement whose provider exposes nothing: an
    /// entitlement nothing has read is *unknown*, never full and never
    /// empty.
    remaining_capacity: Option<crate::provider::quota::RemainingCapacityScore>,
    /// Map line 1963's reset-time slot, in seconds. Same contract as
    /// `remaining_capacity`: [`Self::with_telemetry`] populates it, `None`
    /// is unknown.
    seconds_until_reset: Option<i64>,
    /// Whose reading `remaining_capacity` and `seconds_until_reset` are —
    /// `Some` exactly when either slot is populated. One scope for the pair,
    /// because both come from the same cached provider reading.
    capacity_scope: Option<TelemetryScope>,
    /// Map line 1965's recent-throttling facet. `None` until
    /// [`Self::with_telemetry`] runs with the ledger's rows in hand —
    /// *unknown*, never "none observed": an absence may only be reported by
    /// a resolver that actually looked.
    throttling: Option<EntitlementThrottleReading>,
    /// Map line 1965's models facet. `None` is unknown, same rule as above.
    models: Option<EntitlementModels>,
    /// Map line 1971's observed-spend facet, against which this entry's own
    /// [`crate::routing::EntitlementRules::spend_ceiling_tokens`] is
    /// compared. Same contract as the four facets above: `None` until
    /// [`Self::with_telemetry`] runs with the ledger's rows in hand, and
    /// `None` thereafter when no row carried a token count — *unknown*,
    /// never "nothing spent".
    spend: Option<EntitlementSpendReading>,
    /// Map lines 1244/1245/1246/1250/1251/1254's subscription-headroom
    /// estimate — [`Self::populate_provider_facets`]'s own producer,
    /// [`crate::routing::evidence::estimate_subscription_headroom`]. `None`
    /// is unknown, the same rule as every facet above; **also** `None` once
    /// [`Self::capacity_scope`] is [`TelemetryScope::PerAccount`] — an
    /// authoritative per-account reading is never displaced by an estimate
    /// (56A-3+'s own ground; this build's own gateway cache can never
    /// produce that scope, so the estimate populates in every reachable case
    /// today).
    headroom_estimate: Option<crate::routing::evidence::SubscriptionHeadroomEstimate>,
    /// Map line 1252 — the user's own stated correction, read straight from
    /// `[entitlements.<name>] headroom_override` at load time, not touched
    /// by [`Self::populate_provider_facets`]. `None` is "no correction
    /// stated", never "the estimate is confirmed correct".
    headroom_override: Option<crate::routing::evidence::HeadroomBand>,
    /// Map line 1255 — `true` when this entry's config asked the
    /// subscription-headroom estimator to stay off. Read only inside
    /// [`Self::populate_provider_facets`], after `capacity`/`reset` are
    /// already populated, so disabling never touches those facets.
    disable_headroom_estimate: bool,
}

impl ResolvedEntitlement {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn kind(&self) -> Option<EntitlementKind> {
        self.kind
    }

    pub fn vendor(&self) -> Option<EntitlementVendor> {
        self.vendor
    }

    /// The credential reference this account authenticates with, when the
    /// entry states one. Resolved to a value only through a
    /// [`crate::secret::SecretStore`], at the moment of use, by whatever
    /// launches against this account — never here.
    pub fn credential(&self) -> Option<&SecretRef> {
        self.credential.as_ref()
    }

    /// Remaining capacity, when telemetry has read one — `None` until
    /// [`Self::with_telemetry`] runs, and `None` thereafter for an
    /// entitlement whose provider exposes nothing. Unknown, never
    /// fabricated.
    pub fn remaining_capacity(&self) -> Option<&crate::provider::quota::RemainingCapacityScore> {
        self.remaining_capacity.as_ref()
    }

    /// Seconds until this account's allowance resets, when telemetry has
    /// read one — the same contract as [`Self::remaining_capacity`].
    pub fn seconds_until_reset(&self) -> Option<i64> {
        self.seconds_until_reset
    }

    /// Whose reading the capacity and reset slots carry — `Some` exactly
    /// when either slot is populated, and [`TelemetryScope::ProviderWide`]
    /// for every reading this build can take: the gateway's quota cache is
    /// keyed by provider, so both entitlements of one provider share it.
    pub fn capacity_scope(&self) -> Option<TelemetryScope> {
        self.capacity_scope
    }

    /// Map line 1965's recent-throttling facet — `None` means *unknown*
    /// (nothing consulted the ledger for this entry), never "none observed".
    pub fn throttling(&self) -> Option<&EntitlementThrottleReading> {
        self.throttling.as_ref()
    }

    /// Map line 1965's models facet — `None` means *unknown*.
    pub fn models(&self) -> Option<&EntitlementModels> {
        self.models.as_ref()
    }

    /// Map line 1971's observed-spend facet — `None` means *unknown*
    /// (nothing consulted the ledger, or no row it holds carried a token
    /// count), never "nothing spent".
    pub fn spend(&self) -> Option<&EntitlementSpendReading> {
        self.spend.as_ref()
    }

    /// Map lines 1244/1245/1246/1250/1251/1254's subscription-headroom
    /// estimate — `None` is unknown (nothing consulted the ledger, or
    /// nothing at all was available to estimate from), and also `None`
    /// whenever [`Self::capacity_scope`] already reads
    /// [`TelemetryScope::PerAccount`]: an authoritative per-account reading
    /// is never displaced by an estimate. See
    /// [`crate::routing::evidence::estimate_subscription_headroom`] for what
    /// this reads and [`crate::routing::evidence::SubscriptionHeadroomEstimate`]
    /// for why it is never a bare number.
    pub fn headroom_estimate(
        &self,
    ) -> Option<&crate::routing::evidence::SubscriptionHeadroomEstimate> {
        self.headroom_estimate.as_ref()
    }

    /// Map line 1252's user override — `[entitlements.<name>]
    /// headroom_override`, read at load time. Authoritative over
    /// [`Self::headroom_estimate`] at the one consumer,
    /// `main.rs::entitlement_facets`, but this accessor hands both back
    /// unmixed so a caller decides how to combine them rather than this
    /// type silently doing it.
    pub fn headroom_override(&self) -> Option<crate::routing::evidence::HeadroomBand> {
        self.headroom_override
    }

    /// This account's key in the ledger's `quota_context` column — the
    /// [`crate::routing::CredentialId::label`] the gateway stamps on every
    /// exchange it forwards for this credential. `None` for an entry with no
    /// credential of its own or no provider backing: such an account has no
    /// per-account rows to be narrowed to.
    pub fn credential_label(&self) -> Option<String> {
        let EntitlementBacking::Provider(provider) = &self.backing else {
            return None;
        };
        self.credential
            .as_ref()
            .map(|reference| crate::routing::CredentialId::new(provider, reference.clone()).label())
    }

    /// Map line 1965's producer — populate the four telemetry facets from
    /// what `telemetry` actually holds, each reading carrying its scope.
    ///
    /// - **Capacity and reset**, for a remote-provider backing: the
    ///   gateway-captured per-provider reading
    ///   ([`crate::provider::telemetry::GatewayQuotaCache`]) folded into the
    ///   provider's own capacity shape. The cache is keyed by provider and
    ///   the gateway's write is settled, so the reading cannot be narrowed
    ///   to one credential: it is [`TelemetryScope::ProviderWide`], shared
    ///   verbatim by every entitlement of that provider. A local-inference
    ///   provider is skipped outright — a local server has no account
    ///   allowance, and the capacity model's local-inference estimate is not
    ///   a reading about *this account*.
    /// - **Recent throttling**: the ledger rows' informative throttles for
    ///   the provider, narrowed to this account's own
    ///   ([`crate::routing::evidence::recent_credential_throttles`]) when
    ///   every throttle row names its account, provider-wide otherwise.
    /// - **Models**: the provider's own declared catalogue
    ///   ([`crate::provider::cache::ModelCache`]), when one was ever
    ///   fetched; a native sign-in is [`EntitlementModels::HarnessDecided`]
    ///   — the harness picks, and Glasshouse does not know the plan's
    ///   models, so no list is ever invented for one.
    ///
    /// Every facet a source cannot answer stays `None` — unknown, never
    /// full, never empty, never zero-observed.
    pub fn with_telemetry(mut self, telemetry: &EntitlementTelemetry<'_>) -> Self {
        match self.backing.clone() {
            EntitlementBacking::Provider(provider) => {
                self.populate_provider_facets(&provider, telemetry);
            }
            EntitlementBacking::NativeHarness(_) => {
                self.models = Some(EntitlementModels::HarnessDecided);
            }
            EntitlementBacking::Unstated => {}
        }
        self
    }

    fn populate_provider_facets(&mut self, provider: &str, telemetry: &EntitlementTelemetry<'_>) {
        use crate::provider::registry::{Locality, ResourceKind};

        let kind = ResourceKind::from_direct_provider(provider);
        if kind.locality() == Locality::Remote
            && let Some(cache) = telemetry.gateway_quota
            && let Some((headers, observed_at_unix)) = cache.load(provider)
        {
            let state = headers.apply_to(
                crate::provider::quota::CapacityState::for_resource(&kind),
                observed_at_unix,
            );
            self.remaining_capacity = state.remaining_capacity_score();
            self.seconds_until_reset = state.seconds_until_reset(telemetry.now_unix);
            if self.remaining_capacity.is_some() || self.seconds_until_reset.is_some() {
                self.capacity_scope = Some(TelemetryScope::ProviderWide);
            }
        }

        if let Some(observations) = telemetry.observations {
            let label = self.credential_label();
            let counted = crate::routing::evidence::recent_credential_throttles(
                observations,
                provider,
                label.as_deref(),
            );
            self.throttling = Some(EntitlementThrottleReading {
                throttled: counted.throttled,
                scope: if counted.account_narrowed {
                    TelemetryScope::PerAccount
                } else {
                    TelemetryScope::ProviderWide
                },
            });
        }

        if let Some(observations) = telemetry.observations {
            // Map line 1971's spend half, read from the same rows and
            // narrowed by the same rule as the throttle facet above — one
            // ledger pass' worth of arithmetic, and `None` when no row
            // carried a count, which is what keeps a stated ceiling from
            // being judged reached by a build that measured nothing.
            let label = self.credential_label();
            let counted = crate::routing::evidence::recent_credential_spend(
                observations,
                provider,
                label.as_deref(),
            );
            self.spend = counted.tokens.map(|tokens| EntitlementSpendReading {
                tokens,
                scope: if counted.account_narrowed {
                    TelemetryScope::PerAccount
                } else {
                    TelemetryScope::ProviderWide
                },
            });
        }

        // Map lines 1244/1245/1246/1250/1251/1254 — the subscription
        // headroom estimator. Guarded on `capacity_scope`, not skipped
        // outright: this build's gateway cache can only ever narrow capacity
        // to `TelemetryScope::ProviderWide` (56A-2's own recorded limit), so
        // this populates in every reachable case today, exactly the
        // "resolver populates the per-account capacity facet from the
        // estimator where the provider-wide reading is all headers gave"
        // the packet asks for — and the moment a future per-account reading
        // exists (56A-3+), this stays inert rather than displacing it.
        //
        // Map line 1255 sits in front of that guard, not behind it: a
        // disabled entitlement leaves `headroom_estimate` at its default
        // `None` and never calls the estimator at all — `capacity`, `reset`,
        // `throttling` and `models` above are already populated by the time
        // this runs, so disabling touches nothing but this one facet.
        if self.disable_headroom_estimate {
            self.headroom_estimate = None;
        } else if self.capacity_scope != Some(TelemetryScope::PerAccount) {
            let label = self.credential_label();
            let session_count = telemetry
                .session_counts
                .and_then(|counts| counts.get(self.name.as_str()))
                .copied();
            self.headroom_estimate = crate::routing::evidence::estimate_subscription_headroom(
                telemetry.observations.unwrap_or(&[]),
                provider,
                label.as_deref(),
                telemetry.now_unix,
                self.seconds_until_reset,
                session_count,
            );
        }

        if let Some(catalogues) = telemetry.model_catalogues {
            self.models = catalogues
                .load(provider)
                .map(|catalogue| EntitlementModels::Declared {
                    models: catalogue
                        .models()
                        .iter()
                        .map(|model| model.id().to_owned())
                        .collect(),
                    scope: TelemetryScope::ProviderWide,
                });
        }
    }

    pub fn backing(&self) -> &EntitlementBacking {
        &self.backing
    }

    pub fn rules(&self) -> &crate::routing::EntitlementRules {
        &self.rules
    }

    pub fn layer(&self) -> Layer {
        self.layer
    }

    /// The router's view: name, rules, and whether a user or project
    /// actually wrote the entry — the synthesised harness-default carries
    /// `configured = false`, which is what keeps the router's pool terms
    /// inert for a user who configured nothing (56A step 3's preservation
    /// clause). The 56A-2 telemetry facets are attached by the caller that
    /// resolved them (`main.rs::routing_entitlement`), because the band they
    /// carry is derived against the user's own thresholds, which this
    /// method does not hold.
    pub fn to_routing(&self) -> crate::routing::Entitlement {
        crate::routing::Entitlement::new(self.name.clone(), self.rules.clone())
            .with_configured(self.layer != Layer::Default)
            // Map line 1970's work item 1, and the two facets that need no
            // threshold to derive, so they are carried **here** rather than
            // by the caller: the backing discriminant is structural (see
            // [`EntitlementBacking::source`]) and the spend reading is a
            // raw token count. The four 56A-2 facets stay with the caller
            // for the reason above — a band is derived against the user's
            // own thresholds, which this method does not hold.
            .with_source(self.backing.source())
            .with_spend(self.spend.map(|reading| {
                crate::routing::EntitlementSpendFacet::new(
                    reading.tokens,
                    reading.scope == TelemetryScope::PerAccount,
                )
            }))
            // The headroom estimate needs no threshold either — unlike the
            // capacity band, [`Self::headroom_estimate`] is already a
            // finished value the moment telemetry ran.
            .with_headroom_estimate(self.headroom_estimate)
    }

    /// What the announcement says inside the parentheses after the name:
    /// the plan when one was stated, the billing vendor when one was stated,
    /// and what backs it. Never a credential — a harness's display name, a
    /// provider's *name*, a vendor's spelling.
    pub fn describe(&self) -> String {
        let backing = match &self.backing {
            EntitlementBacking::NativeHarness(harness) => {
                format!("{}'s own sign-in", harness.display_name())
            }
            EntitlementBacking::Provider(provider) => format!("behind provider `{provider}`"),
            EntitlementBacking::Unstated => "no backing stated".to_owned(),
        };
        let mut parts = Vec::new();
        if let Some(kind) = self.kind {
            parts.push(kind.describe().to_owned());
        }
        if let Some(vendor) = self.vendor {
            parts.push(format!("vendor `{}`", vendor.as_str()));
        }
        parts.push(backing);
        parts.join(", ")
    }
}

/// Whose reading a telemetry facet is — map line 1965's scope discipline:
/// telemetry keyed by this account's own credential is one thing, telemetry
/// the whole provider shares is another, and a display that showed the
/// second as the first would be claiming per-account knowledge nothing
/// measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelemetryScope {
    /// Keyed by this entitlement's own credential — the reading is about
    /// this account and no other.
    PerAccount,
    /// Keyed by the provider — every entitlement of that provider shares
    /// this same reading.
    ProviderWide,
}

impl TelemetryScope {
    /// The display's scope word.
    pub fn as_str(self) -> &'static str {
        match self {
            TelemetryScope::PerAccount => "this account",
            TelemetryScope::ProviderWide => "provider-wide",
        }
    }
}

/// Map line 1965's recent-throttling facet: how many informative throttles
/// the evidence window records against this entitlement, and whose count it
/// is. A count of zero from a resolver that looked is "none observed" — a
/// different fact from the `None` an unresolved entry carries, which is
/// *unknown*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntitlementThrottleReading {
    throttled: usize,
    scope: TelemetryScope,
}

impl EntitlementThrottleReading {
    /// Informative throttles in the window — this account's own when
    /// [`Self::scope`] is [`TelemetryScope::PerAccount`], the provider's
    /// total otherwise.
    pub fn throttled(&self) -> usize {
        self.throttled
    }

    pub fn scope(&self) -> TelemetryScope {
        self.scope
    }
}

/// Map line 1971's observed-spend facet: how many tokens the evidence
/// window recorded against this entitlement, and whose reading that is.
///
/// **Tokens, not money** — see
/// [`EntitlementConfig::spend_ceiling_tokens`] and
/// [`crate::routing::evidence::CredentialSpend`] for why the only currency
/// this ledger holds is the one a ceiling can be checked against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntitlementSpendReading {
    tokens: u64,
    scope: TelemetryScope,
}

impl EntitlementSpendReading {
    /// Input plus output tokens in the window — this account's own when
    /// [`Self::scope`] is [`TelemetryScope::PerAccount`], the provider's
    /// total otherwise.
    pub fn tokens(&self) -> u64 {
        self.tokens
    }

    pub fn scope(&self) -> TelemetryScope {
        self.scope
    }
}

/// Map line 1965's models facet: which models this entitlement can serve,
/// from what its backing actually declares — never an invented list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntitlementModels {
    /// The provider's own declared model list — the fetched
    /// [`crate::provider::cache::ModelCatalogue`], which is per provider,
    /// so the scope is stated on the value.
    Declared {
        models: Vec<String>,
        scope: TelemetryScope,
    },
    /// A native sign-in: the harness picks its own models, and Glasshouse
    /// does not know the plan's list — an answer, not an absence.
    HarnessDecided,
}

/// The telemetry sources [`ResolvedEntitlement::with_telemetry`] reads —
/// each one optional and each one already opened or loaded by the caller,
/// so this resolver performs no I/O beyond the caches' own fail-soft file
/// reads: never a probe, never a network call, never a database write
/// (design-decisions §56A step 2's Cluster E discipline). A source left
/// unset leaves its facets `None` — unknown — rather than fabricating an
/// observation nothing took.
pub struct EntitlementTelemetry<'a> {
    gateway_quota: Option<&'a crate::provider::telemetry::GatewayQuotaCache>,
    model_catalogues: Option<&'a crate::provider::cache::ModelCache>,
    /// The evidence window's rows, when the caller read them —
    /// `None` keeps the throttling facet unknown, because "none observed"
    /// may only be said by a resolver that actually looked.
    observations: Option<&'a [crate::routing::evidence::RoutingObservation]>,
    /// Map line 1245's "historical sessions" input to the headroom
    /// estimator: how many of this project's own sessions
    /// (`sessions.entitlement`, migration 22) were charged to each
    /// entitlement, keyed by entitlement **name** — not by provider, unlike
    /// every other source here, because a session names the account that
    /// served it directly. `None` leaves the estimator without this input,
    /// exactly like every other absent source.
    session_counts: Option<&'a std::collections::BTreeMap<String, usize>>,
    now_unix: i64,
}

impl<'a> EntitlementTelemetry<'a> {
    /// No sources at all — every facet stays unknown until a `with_*`
    /// supplies one.
    pub fn new(now_unix: i64) -> Self {
        Self {
            gateway_quota: None,
            model_catalogues: None,
            observations: None,
            session_counts: None,
            now_unix,
        }
    }

    /// The gateway-captured per-provider rate-limit readings.
    pub fn with_gateway_quota(
        mut self,
        cache: &'a crate::provider::telemetry::GatewayQuotaCache,
    ) -> Self {
        self.gateway_quota = Some(cache);
        self
    }

    /// The fetched provider model catalogues.
    pub fn with_model_catalogues(mut self, cache: &'a crate::provider::cache::ModelCache) -> Self {
        self.model_catalogues = Some(cache);
        self
    }

    /// The evidence window's observation rows, read from the project's
    /// ledger by the caller.
    pub fn with_observations(
        mut self,
        observations: &'a [crate::routing::evidence::RoutingObservation],
    ) -> Self {
        self.observations = Some(observations);
        self
    }

    /// How many of this project's own sessions were charged to each
    /// entitlement, keyed by entitlement name — map line 1245's "historical
    /// sessions" input to the subscription-headroom estimator.
    pub fn with_session_counts(
        mut self,
        counts: &'a std::collections::BTreeMap<String, usize>,
    ) -> Self {
        self.session_counts = Some(counts);
        self
    }
}

/// Why the `[entitlements]` tables could not be resolved. Each is a
/// contradiction only the two layers together can show, so none is a
/// deserialisation error; each is refused rather than resolved by guessing.
#[derive(Debug, thiserror::Error)]
pub enum EntitlementLookupError {
    #[error(
        "entitlement `{name}` names both `native_harness` and `provider`; an entitlement is \
         a harness's own sign-in or the account behind a provider, not both"
    )]
    TwoBackings { name: String },
    #[error(
        "entitlement `{name}` takes the name reserved for {}'s own sign-in without being it; \
         set `native_harness = \"{name}\"` on it or rename it",
        .harness.display_name()
    )]
    NameReservedForHarness {
        name: String,
        harness: IntegrationId,
    },
    #[error(
        "entitlements {} all claim to be {}'s own sign-in; a harness signs in to one account, \
         so keep one",
        .names.join(", "), .harness.display_name()
    )]
    AmbiguousNativeHarness {
        harness: IntegrationId,
        names: Vec<String>,
    },
    #[error(
        "entitlements {} all claim provider `{provider}`; a configured provider is one account, \
         so keep one",
        .names.join(", ")
    )]
    AmbiguousProvider {
        provider: String,
        names: Vec<String>,
    },
    #[error(
        "entitlement `{name}` claims to be a harness's own sign-in and names its own \
         `credential`; a harness authenticates its own sign-in itself, and an entitlement with \
         its own credential is a separate account — drop one of the two"
    )]
    NativeSignInWithOwnCredential { name: String },
    #[error(
        "entitlements {} all name the same credential ({reference}); one credential is one \
         account, and map line 1963 gives each entitlement its own — give each entry its own \
         reference",
        .names.join(", ")
    )]
    SharedCredential {
        names: Vec<String>,
        /// The reference's *names* — a variable name, or a service and
        /// account — never a value.
        reference: String,
    },
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

/// One model's declared resource facts, as stored under
/// `[providers.<name>.model_facts.<model>]` — the serialisable, per-axis
/// mirror of [`crate::routing::capability::ResourceFacts`]'s seven axes.
///
/// Every field optional and independent: `None` on an axis means the user
/// has not declared it, exactly as an absent model in
/// [`ProviderConfig::model_facts`] means the same thing one level up. See
/// [`ProviderConfig::resource_facts_of`], the only place these become a
/// [`crate::routing::capability::ResourceFacts`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfiguredModelFacts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_edit: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_tool_use: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser_use: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub large_context: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast_cheap_analysis: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_review: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp: Option<bool>,
}

/// Which configuration table a user-declared fact came from — the two
/// places a fact can be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeclaredIn {
    /// `[providers.<name>] tool_calls = …`
    ProviderToolCalls,
    /// `[providers.<name>.model_facts.<model>] <axis> = …`
    ModelFacts,
}

/// Turn a user's config-time declaration into the `'static` evidence
/// [`crate::harness::Declared::verified`] requires: four literals, one per
/// (layer, table) pair, and **no allocation**. The provider and model names
/// are deliberately not in the text. `Declared`'s evidence is `&'static str`
/// everywhere it is constructed, and the alternative — leaking a formatted
/// string per resolved fact — is unbounded in `glasshouse api serve`, which
/// answers `RecommendRoute` requests for as long as it runs and resolves
/// configuration for each one. The destination a reason is printed beside
/// already names its provider and model, so the text stays re-checkable
/// without repeating them.
fn declared_from_config(layer: Layer, table: DeclaredIn) -> &'static str {
    match (layer, table) {
        (Layer::Project, DeclaredIn::ProviderToolCalls) => {
            "declared as tool_calls in the project config's [providers] table"
        }
        (Layer::User | Layer::Default, DeclaredIn::ProviderToolCalls) => {
            "declared as tool_calls in the user config's [providers] table"
        }
        (Layer::Project, DeclaredIn::ModelFacts) => {
            "declared in the project config's [providers.*.model_facts] table"
        }
        (Layer::User | Layer::Default, DeclaredIn::ModelFacts) => {
            "declared in the user config's [providers.*.model_facts] table"
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
            free_models: Vec::new(),
            metered_models: Vec::new(),
            tool_calls: None,
            model_facts: BTreeMap::new(),
            model_ceilings: BTreeMap::new(),
            model_capabilities: BTreeMap::new(),
            prompt_transform: None,
            quota: None,
        }
    }

    pub fn template(&self) -> &str {
        &self.template
    }

    pub fn set_template(&mut self, template: impl Into<String>) -> &mut Self {
        self.template = template.into();
        self
    }

    /// This layer's quota overrides, or `None` when this layer recorded
    /// none — capability map lines 1233, 1203 and 1237.
    pub fn quota(&self) -> Option<&QuotaOverride> {
        self.quota.as_ref()
    }

    pub fn set_quota(&mut self, quota: Option<QuotaOverride>) -> &mut Self {
        self.quota = quota;
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

    /// The model identifiers on this provider marked free-tier or
    /// zero-marginal-cost. See the field's own doc.
    pub fn free_models(&self) -> &[String] {
        &self.free_models
    }

    pub fn set_free_models(&mut self, models: Vec<String>) -> &mut Self {
        self.free_models = models;
        self
    }

    /// The model identifiers the user has explicitly named as eligible for
    /// metered disposable-job fallback. See the field's own doc for why an
    /// empty list is the off state rather than a separate flag.
    pub fn metered_models(&self) -> &[String] {
        &self.metered_models
    }

    pub fn set_metered_models(&mut self, models: Vec<String>) -> &mut Self {
        self.metered_models = models;
        self
    }

    /// Whether this provider's protocols carry tool calls, as the user
    /// declared it — `None` when nobody has. See the field's own doc.
    pub fn tool_calls(&self) -> Option<bool> {
        self.tool_calls
    }

    pub fn set_tool_calls(&mut self, tool_calls: Option<bool>) -> &mut Self {
        self.tool_calls = tool_calls;
        self
    }

    /// Overlay [`ProviderConfig::tool_calls`] onto every protocol `provider`
    /// declares — [`ProviderConfig::to_provider`]'s own doc explains why
    /// this is a step its caller takes rather than something that method
    /// does itself: the evidence string needs `layer`, which a `Provider`
    /// resolved from a bare template has no way to supply.
    ///
    /// `None` (nobody declared) touches nothing, leaving whatever
    /// `to_provider` already produced — the template's own `Unverified` on
    /// every real provider today. `Some(value)` overrides every protocol's
    /// `tool_calls` with [`crate::harness::Declared::verified`], citing
    /// `layer` and the `[providers]` table exactly as
    /// [`ProviderConfig::resource_facts_of`]'s reason does one level down.
    pub fn declare_tool_calls(&self, provider: &mut crate::provider::Provider, layer: Layer) {
        if let Some(declared) = self.tool_calls {
            let reason = declared_from_config(layer, DeclaredIn::ProviderToolCalls);
            for protocol in &mut provider.protocols {
                protocol.tool_calls = crate::harness::Declared::verified(declared, reason);
            }
        }
    }

    /// The per-model resource facts the user configured — map line 1517.
    /// See [`ProviderConfig::resource_facts_of`] for how a lookup by model
    /// name reads these.
    pub fn model_facts(&self) -> &BTreeMap<String, ConfiguredModelFacts> {
        &self.model_facts
    }

    pub fn set_model_facts(&mut self, facts: BTreeMap<String, ConfiguredModelFacts>) -> &mut Self {
        self.model_facts = facts;
        self
    }

    /// `model`'s declared resource facts on this provider — map line 1517's
    /// producer, turning [`ProviderConfig::model_facts`]'s per-axis
    /// `Option<bool>`s into [`crate::routing::capability::ResourceFacts`]'s
    /// `Declared<bool>`s. `layer` is which configuration layer is asking,
    /// for the evidence string's own `[providers.*.model_facts]` table.
    ///
    /// A model absent from [`ProviderConfig::model_facts`] answers
    /// [`crate::routing::capability::ResourceFacts::UNVERIFIED`] outright.
    /// An axis absent from a present model's table stays
    /// [`crate::harness::Declared::Unverified`] on that axis alone — a
    /// missing key never upgrades to `Verified`.
    pub fn resource_facts_of(
        &self,
        model: &str,
        layer: Layer,
    ) -> crate::routing::capability::ResourceFacts {
        use crate::routing::capability::ResourceFacts;

        let Some(config) = self.model_facts.get(model) else {
            return ResourceFacts::UNVERIFIED;
        };
        let reason = declared_from_config(layer, DeclaredIn::ModelFacts);
        let axis = |value: Option<bool>| match value {
            Some(v) => crate::harness::Declared::verified(v, reason),
            None => crate::harness::Declared::Unverified,
        };
        ResourceFacts {
            code_edit: axis(config.code_edit),
            shell_tool_use: axis(config.shell_tool_use),
            browser_use: axis(config.browser_use),
            large_context: axis(config.large_context),
            fast_cheap_analysis: axis(config.fast_cheap_analysis),
            repository_review: axis(config.repository_review),
            mcp: axis(config.mcp),
        }
    }

    /// The per-model workload-tier ceilings the user configured — map line
    /// 1796. See the field's own doc for why an absent model is not a low
    /// ceiling.
    pub fn model_ceilings(&self) -> &BTreeMap<String, ConfiguredWorkloadTier> {
        &self.model_ceilings
    }

    pub fn set_model_ceilings(
        &mut self,
        ceilings: BTreeMap<String, ConfiguredWorkloadTier>,
    ) -> &mut Self {
        self.model_ceilings = ceilings;
        self
    }

    /// The highest workload tier `model` on this provider is established to
    /// serve, or `None` when nobody has stated one.
    ///
    /// The one place this lookup lives, so no call site re-implements it —
    /// [`ProviderConfig::cost_of`]'s own rule. There is no inference and no
    /// default: a model nobody named here answers `None`, which the router
    /// reads as *not established* rather than as a refusal.
    pub fn ceiling_of(&self, model: &str) -> Option<crate::routing::classify::WorkloadTier> {
        self.model_ceilings
            .get(model)
            .map(|configured| configured.tier())
    }

    /// The calibrated capability records the user configured for this
    /// provider — Phase 34F, capability map line 1475. See the field's own
    /// doc for why `model` and `backend` are not fields of the record
    /// itself.
    pub fn model_capabilities(&self) -> &BTreeMap<String, capability::ModelCapabilityRecord> {
        &self.model_capabilities
    }

    pub fn set_model_capabilities(
        &mut self,
        records: BTreeMap<String, capability::ModelCapabilityRecord>,
    ) -> &mut Self {
        self.model_capabilities = records;
        self
    }

    /// The calibrated record for `model` on this provider, or `None` when
    /// nobody has recorded one — capability map line 1475's "configurable
    /// data" read back.
    pub fn model_capability(&self, model: &str) -> Option<&capability::ModelCapabilityRecord> {
        self.model_capabilities.get(model)
    }

    /// The one lookup capability map lines 1476, 1478, 1479, and 1484 share:
    /// [`ProviderConfig::ceiling_of`]'s own override always wins; failing
    /// that, `model`'s capability record contributes its initial ceiling
    /// (capped by its task-kind suitability) when the user assigned it
    /// themselves, or only a non-binding prior when a benchmark seeded it.
    /// See [`capability::resolve_ceiling`] for why a benchmark-derived
    /// record can rank but never refuse.
    ///
    /// **This lookup is context-blind** — it knows only `model` and the
    /// provider `self` is — and only a record that states no
    /// harness/launch-profile/protocol narrowing at all
    /// (`ModelCapabilityRecord::is_context_general`, which is deliberately
    /// not public — this module is its only honest caller) is
    /// eligible here. A record that narrows to even one of those axes is
    /// filtered out rather than applied without checking them: this path
    /// has no harness, launch profile, or protocol in hand to check a
    /// narrowed record's [`capability::ModelCapabilityRecord::applies_to`]
    /// against — that context exists only in `main.rs`'s destination
    /// construction — so honouring a narrowed record here would leak its
    /// calibration onto every destination sharing this provider and model,
    /// including ones on a harness the record was never calibrated for.
    /// Capability map line 1482.
    pub fn resolved_ceiling(&self, model: &str) -> capability::CeilingResolution {
        let record = self
            .model_capability(model)
            .filter(|record| record.is_context_general());
        capability::resolve_ceiling(self.ceiling_of(model), record)
    }

    /// [`Self::resolved_ceiling`], for a caller that actually has the
    /// harness/launch-profile/protocol context a destination is built with —
    /// capability map line 1482's closing half. Where [`Self::resolved_ceiling`]
    /// may only trust a record that narrows nothing at all, this checks each
    /// record's own [`capability::ModelCapabilityRecord::applies_to`] against
    /// `query` instead: a record scoped to a harness, profile, or protocol the
    /// query does not name is filtered out exactly as before, and a record
    /// that narrows to axes `query` *does* state is now honoured rather than
    /// blanket-excluded.
    ///
    /// **The production caller is `main.rs::destination_tier_ceiling`**,
    /// which builds `query` from the same launch context — harness, launch
    /// profile, protocol — that `main.rs::routing_destinations` already has
    /// in hand for every destination it constructs.
    pub fn resolved_ceiling_for(
        &self,
        model: &str,
        query: &capability::CapabilityQuery<'_>,
    ) -> capability::CeilingResolution {
        let record = self
            .model_capability(model)
            .filter(|record| record.applies_to(query));
        capability::resolve_ceiling(self.ceiling_of(model), record)
    }

    /// What this backend does to a prompt on the way through, as the user
    /// described it — Phase 9K line 609. `None` means nothing was declared,
    /// which is not the same as "nothing happens".
    pub fn prompt_transform(&self) -> Option<&str> {
        self.prompt_transform.as_deref()
    }

    pub fn set_prompt_transform(&mut self, transform: Option<String>) -> &mut Self {
        self.prompt_transform = transform;
        self
    }

    /// Whether `model` costs the user anything at the margin.
    ///
    /// The one place this lookup lives, so no call site re-implements it. A
    /// model this provider has not named in [`ProviderConfig::free_models`]
    /// answers [`crate::routing::Cost::Metered`] — including a model whose
    /// name happens to end in `:free` or look free by any other convention.
    /// There is no inference here; see the field's own doc for why.
    pub fn cost_of(&self, model: &str) -> crate::routing::Cost {
        if self.free_models.iter().any(|marked| marked == model) {
            crate::routing::Cost::Free
        } else {
            crate::routing::Cost::Metered
        }
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
    ///
    /// **Does not apply [`ProviderConfig::tool_calls`].** That declaration
    /// needs the configuration *layer* this entry was read from, for its
    /// evidence string, and this method's callers outside this module have
    /// no layer to give it — so the override is applied one layer up, in
    /// [`EffectiveConfig::configured_provider`], the one caller that reads
    /// this entry with a layer already in hand. Every other caller of this
    /// method sees exactly what it saw before [`ProviderConfig::tool_calls`]
    /// existed.
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

/// A free resource by name, as configuration stores it — the serialisable
/// counterpart to [`crate::routing::free::FreeResourceKey`].
///
/// That type is frozen in `crate::routing` and derives neither `Serialize`
/// nor `Deserialize`, the same reason [`StoredCredentialRef`] exists beside
/// [`crate::secret::SecretRef`] rather than that type growing serde impls of
/// its own: the shape a *reference* takes on disk is a configuration-schema
/// decision, kept separate from the domain type routing policy reasons
/// about. [`FreeResourceRef::to_key`] and [`FreeResourceRef::from_key`] are
/// the only bridge between the two, so they cannot drift.
///
/// Two names — a provider and a model — and nothing else, for the same
/// reason [`RoutingModelChoice::Pinned`] holds only names: both are as safe
/// to write into a tracked configuration file as
/// [`ProviderConfig::credential_env`]'s variable names already are.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreeResourceRef {
    provider: String,
    model: String,
}

impl FreeResourceRef {
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
        }
    }

    pub fn provider(&self) -> &str {
        &self.provider
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn to_key(&self) -> crate::routing::free::FreeResourceKey {
        crate::routing::free::FreeResourceKey::new(self.provider.clone(), self.model.clone())
    }

    pub fn from_key(key: &crate::routing::free::FreeResourceKey) -> Self {
        Self::new(key.provider.clone(), key.model.clone())
    }
}

/// The provider and model a user has chosen to perform memory extraction —
/// Phase 21's *"allow a configurable cheap or local model to perform memory
/// extraction."*
///
/// # Why this is its own field and not a reuse of the routing preferences
///
/// [`FreeResourceRef`] is the same two strings, and reusing it was the
/// tempting move. It would have been wrong: the free-routing preferences say
/// *which resource to prefer when Glasshouse routes*, and a user who has
/// written them has not thereby asked Glasshouse to start making outbound
/// requests from a hook that runs inside their coding session. This field is
/// that request, made once and explicitly, and `None` — the default — is
/// exactly today's behaviour.
///
/// # Names only, exactly like every other provider field here
///
/// A provider name and a model identifier. The base URL, the credential
/// variable names and any extra headers all come from the
/// [`ProviderConfig`] this names, which is where they already live.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractionModelRef {
    provider: String,
    model: String,
}

impl ExtractionModelRef {
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
        }
    }

    pub fn provider(&self) -> &str {
        &self.provider
    }

    pub fn model(&self) -> &str {
        &self.model
    }
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

/// Which routing model classifies a request, as recorded in configuration.
///
/// The routing model is the cheap, fast, replaceable component the capability
/// map describes in its preamble: before spending premium agent capacity,
/// Glasshouse may ask it to classify a request and estimate the capability
/// tier the work needs. Phase 2C's job — and this type's — is only to record
/// *which* of three answers the user gave. Actually asking a model anything
/// is Phase 34B, and choosing one for [`RoutingModelChoice::Automatic`] is
/// Phase 34C; neither is built here, and this type is deliberately shaped so
/// neither has to be rewritten to read it.
///
/// # Why `Automatic` stores an intent and not a model
///
/// Phase 2C line 2 asks for a choice that "selects the cheapest sufficiently
/// fast configured resource". That selection depends on provider health,
/// rate-limit headroom, latency and price *at the moment of use* — every
/// filter in Phase 34C is a live condition — so resolving it once during a
/// first-run wizard and writing the winner down would freeze a decision the
/// map explicitly wants re-evaluated ("Re-evaluate the automatic
/// routing-model choice when its provider becomes degraded or
/// rate-limited"). [`RoutingModelChoice::Automatic`] therefore carries no
/// payload at all: it is the user saying "you pick", not a cached answer.
///
/// # This is a reference, never a credential
///
/// [`RoutingModelChoice::Pinned`] holds a provider *name* — a key into
/// [`ProviderTable`] — and a model *name*. Both are as safe to write into a
/// tracked project file as [`ProviderConfig::credential_env`]'s variable
/// names already are, which is the same rule [`StoredCredentialRef`]
/// follows. Resolving the named provider to an actual credential stays
/// `SecretStore`'s job. See `tests::serialized_form_has_no_secret_capable_field`
/// for the structural guard.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RoutingModelChoice {
    /// No model classifies anything: deterministic routing heuristics do.
    ///
    /// This is the default, and it is a first-class outcome rather than an
    /// absence — Phase 2C line 4 ("use deterministic routing heuristics
    /// until configured") and Phase 2D's "deterministic-only classification"
    /// are the same state, reached from opposite directions: the wizard's
    /// "Do later" simply records nothing at all (see
    /// [`RoutingConfig::model`]), and a settings screen that wants to say
    /// "deterministic, on purpose, do not ask again" writes this variant
    /// explicitly. Both resolve identically — see
    /// [`RoutingModelChoice::resolve`].
    #[default]
    Deterministic,
    /// Let Glasshouse choose among the configured resources at the moment it
    /// needs one. Stored as an intent; Phase 34C does the choosing.
    Automatic,
    /// Classify with exactly this model, from exactly this configured
    /// provider. Two names, no credential.
    Pinned { provider: String, model: String },
}

impl RoutingModelChoice {
    /// What this choice actually resolves to, given the provider names that
    /// are configured right now.
    ///
    /// # A vanished provider is not a startup failure
    ///
    /// This is the one lookup in this module that refuses to return an
    /// error. [`EffectiveConfig::configured_provider`] answers an unknown
    /// name with [`ProviderLookupError::Unknown`], because a user who typed
    /// `--provider nope` on the command line asked for something specific
    /// and must be told it does not exist. A routing model is not that:
    /// nobody asked for it this run, it is an optimisation over a system
    /// that already works without it, and providers legitimately come and go
    /// as keys are rotated and configuration is edited. So a
    /// [`RoutingModelChoice::Pinned`] naming a provider that is no longer
    /// configured degrades to [`RoutingModelResolution::Heuristics`] — with
    /// a [`RoutingFallback`] that says which provider went missing, so the
    /// degrade is visible rather than silent — instead of making Glasshouse
    /// fail to start. Phase 34B's "Allow deterministic heuristics to remain
    /// the final fallback when every routing model is unavailable" is the
    /// same instinct one phase earlier.
    ///
    /// `configured` is provider *names* — [`EffectiveConfig::provider_names`]
    /// in production. Whether a named provider is currently
    /// [`ProviderConfig::enabled`] is deliberately not consulted: that field's
    /// own documentation records that "deciding whether routing may actually
    /// use a disabled provider is a later phase's job", and answering it here
    /// would be that phase arriving early.
    pub fn resolve(&self, configured: &[String]) -> RoutingModelResolution {
        match self {
            Self::Deterministic => {
                RoutingModelResolution::Heuristics(RoutingFallback::DeterministicChosen)
            }
            Self::Automatic => RoutingModelResolution::Automatic,
            Self::Pinned { provider, model } => {
                if configured.iter().any(|name| name == provider) {
                    RoutingModelResolution::Pinned {
                        provider: provider.clone(),
                        model: model.clone(),
                    }
                } else {
                    RoutingModelResolution::Heuristics(RoutingFallback::ProviderNotConfigured {
                        provider: provider.clone(),
                        model: model.clone(),
                    })
                }
            }
        }
    }
}

/// What will actually classify a request, after a recorded
/// [`RoutingModelChoice`] has been checked against the providers that exist.
///
/// Three outcomes, and only one of them names a model. [`Self::Automatic`]
/// is passed through unresolved on purpose — picking the cheapest
/// sufficiently fast resource is Phase 34C's whole job, and this type
/// refuses to guess at it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutingModelResolution {
    /// Deterministic routing heuristics classify the request. The
    /// [`RoutingFallback`] says why, and is worth showing to the user when
    /// it is not simply "you never configured one".
    Heuristics(RoutingFallback),
    /// Glasshouse picks a resource when it needs one (Phase 34C).
    Automatic,
    /// This exact provider and model, both still configured.
    Pinned { provider: String, model: String },
}

impl RoutingModelResolution {
    /// The reason deterministic heuristics are answering, or `None` when
    /// they are not.
    pub fn fallback(&self) -> Option<&RoutingFallback> {
        match self {
            Self::Heuristics(reason) => Some(reason),
            Self::Automatic | Self::Pinned { .. } => None,
        }
    }
}

/// Why deterministic routing heuristics are classifying requests instead of
/// a model.
///
/// Not an error type, deliberately: two of these three are ordinary,
/// expected, fully working states, and giving them `std::error::Error` would
/// invite a caller to treat "the user has not configured a routing model"
/// as a failure. It carries a [`std::fmt::Display`] because the degrade must
/// be *sayable* — Phase 2C's behavioural contract requires a configuration
/// naming a model that has disappeared to degrade "and say so".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutingFallback {
    /// Nothing has ever been recorded — the wizard's "Do later", or a
    /// configuration written before this field existed.
    NotConfigured,
    /// [`RoutingModelChoice::Deterministic`] was recorded explicitly.
    DeterministicChosen,
    /// A [`RoutingModelChoice::Pinned`] model names a provider that is not
    /// in configuration any more.
    ProviderNotConfigured { provider: String, model: String },
}

/// A routing-policy scalar outside the range Glasshouse can use honestly.
///
/// These bounds are intentionally generous. They reject values that are
/// almost certainly unit mistakes while leaving policy, including a
/// zero-cost/free-only ceiling and a disabled zero-percent reserve, under the
/// user's control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RoutingValueError {
    #[error("router latency must be between {min_ms}ms and {max_ms}ms, not {value_ms}ms")]
    Latency {
        value_ms: u32,
        min_ms: u32,
        max_ms: u32,
    },
    #[error(
        "router marginal cost must be at most {max_micro_usd} micro-USD per decision, not {value_micro_usd} micro-USD"
    )]
    Cost {
        value_micro_usd: u32,
        max_micro_usd: u32,
    },
    #[error("premium reserve must be between 0% and 100%, not {value}%")]
    Reserve { value: u16 },
}

/// Maximum acceptable routing-model latency, in milliseconds.
///
/// Ten milliseconds is below any realistic end-to-end model decision but
/// still permits a very fast local classifier. Sixty seconds is already far
/// beyond interactive routing; a larger value is almost certainly seconds
/// entered as milliseconds (or a policy that should disable model routing).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct RouterLatencyMs(u32);

impl RouterLatencyMs {
    pub const MIN: u32 = 10;
    pub const MAX: u32 = 60_000;
    pub const DEFAULT: Self = Self(2_000);

    pub fn get(self) -> u32 {
        self.0
    }
}

impl TryFrom<u32> for RouterLatencyMs {
    type Error = RoutingValueError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        if (Self::MIN..=Self::MAX).contains(&value) {
            Ok(Self(value))
        } else {
            Err(RoutingValueError::Latency {
                value_ms: value,
                min_ms: Self::MIN,
                max_ms: Self::MAX,
            })
        }
    }
}

impl From<RouterLatencyMs> for u32 {
    fn from(value: RouterLatencyMs) -> Self {
        value.0
    }
}

/// Maximum marginal price of one routing decision, in millionths of a US
/// dollar.
///
/// Fixed-point microdollars keep a human-editable TOML integer exact and
/// avoid floating-point comparisons in policy. One dollar is a deliberately
/// high ceiling for a bounded classification call; larger values are treated
/// as a unit mistake rather than accepted silently. Zero is valid and means
/// that only zero-marginal-cost candidates satisfy the price policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct RouterCostMicroUsd(u32);

impl RouterCostMicroUsd {
    pub const MAX: u32 = 1_000_000;
    pub const DEFAULT: Self = Self(1_000);

    pub fn get(self) -> u32 {
        self.0
    }
}

impl TryFrom<u32> for RouterCostMicroUsd {
    type Error = RoutingValueError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        if value <= Self::MAX {
            Ok(Self(value))
        } else {
            Err(RoutingValueError::Cost {
                value_micro_usd: value,
                max_micro_usd: Self::MAX,
            })
        }
    }
}

impl From<RouterCostMicroUsd> for u32 {
    fn from(value: RouterCostMicroUsd) -> Self {
        value.0
    }
}

/// Remaining premium capacity below which routing protects the subscription.
/// Zero disables the reserve and one hundred protects all remaining premium
/// capacity from lower-priority routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub struct PremiumReservePercent(u8);

impl PremiumReservePercent {
    pub const DEFAULT: Self = Self(20);

    pub fn get(self) -> u8 {
        self.0
    }
}

impl TryFrom<u16> for PremiumReservePercent {
    type Error = RoutingValueError;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        if value <= 100 {
            Ok(Self(value as u8))
        } else {
            Err(RoutingValueError::Reserve { value })
        }
    }
}

impl From<PremiumReservePercent> for u16 {
    fn from(value: PremiumReservePercent) -> Self {
        value.0.into()
    }
}

impl std::fmt::Display for RoutingFallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotConfigured => f.write_str(
                "no routing model is configured; requests are classified by deterministic \
                 routing heuristics",
            ),
            Self::DeterministicChosen => f.write_str(
                "routing is set to deterministic-only; requests are classified by \
                 deterministic routing heuristics",
            ),
            Self::ProviderNotConfigured { provider, model } => write!(
                f,
                "routing model `{model}` names provider `{provider}`, which is not \
                 configured; requests are classified by deterministic routing heuristics \
                 until that provider is configured again"
            ),
        }
    }
}

/// The `[routing]` table: how requests get classified.
///
/// The model choice and four bounded policy preferences belong together here:
/// they describe how routing should classify, not live observations about any
/// provider. Every field is optional so project and user layers can override
/// one preference without copying the rest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingConfig {
    /// The recorded routing-model choice, or `None` for "never decided".
    ///
    /// `None` is what the wizard's "Do later" writes, and it is why that
    /// choice leaves *no routing model configured* in the literal sense:
    /// `skip_serializing_if` means a first run that tabs straight through
    /// produces a configuration file with no `[routing]` table in it at all.
    /// It resolves exactly like [`RoutingModelChoice::Deterministic`] — see
    /// [`EffectiveConfig::routing_model`] — so the two are behaviourally one
    /// state with two spellings, and only the *reason* string tells them
    /// apart.
    ///
    /// Keeping it an `Option` rather than collapsing it into
    /// [`RoutingModelChoice::Deterministic`] is what makes layering work:
    /// [`EffectiveConfig`] needs three states per layer — "this layer says
    /// automatic", "this layer says deterministic", and "this layer says
    /// nothing, ask the next one" — exactly like
    /// [`IntegrationConfig::executable`]. A project that wants
    /// deterministic-only classification *over* a user-level `automatic` can
    /// then say so, which a collapsed shape could not express.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<RoutingModelChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_router_latency_ms: Option<RouterLatencyMs>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_marginal_cost_micro_usd: Option<RouterCostMicroUsd>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prefer_free: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    premium_reserve_percent: Option<PremiumReservePercent>,
    /// The user's preferred order over free resources — Phase 9I line 536.
    /// `None` is "this layer never recorded one", matching
    /// [`RoutingConfig::model`]'s own reasoning, not "the user cleared it to
    /// empty"; a project layer that wants to record an explicit empty order
    /// writes `Some(Vec::new())`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    free_resource_order: Option<Vec<FreeResourceRef>>,
    /// Free resources the user has disabled — Phase 9I line 536. Same
    /// `None`-means-undecided reasoning as
    /// [`RoutingConfig::free_resource_order`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    free_resource_disabled: Option<Vec<FreeResourceRef>>,
    /// The user's pinned free resource, if any — Phase 9I line 536.
    ///
    /// A pin naming a provider that is no longer configured must not stop
    /// Glasshouse from starting. This module never validates the name
    /// against configured providers when loading or saving it — the same
    /// reasoning [`RoutingModelChoice::resolve`] documents for
    /// [`RoutingModelChoice::Pinned`]: providers legitimately come and go as
    /// keys are rotated, and a stale pin degrades visibly, through
    /// [`crate::routing::disposable::NoResource::PinnedResourceUnavailable`],
    /// rather than refusing to load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    free_resource_pin: Option<FreeResourceRef>,
    /// User-overridden capacity-band thresholds — capability map line 1270.
    /// `None` means the defaults in
    /// [`crate::provider::quota::CapacityBandThresholds::DEFAULT`] apply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    capacity_band_thresholds: Option<CapacityBandThresholdsConfig>,
    /// User-overridden routing score weights — capability map lines
    /// 1357/1358. `None` means the defaults in
    /// [`crate::routing::session::ScoreWeights::default`] apply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    score_weights: Option<ScoreWeightsConfig>,
    /// The sessions whose work may spend protected quota reserve whatever the
    /// reserve policy would otherwise decide — capability map line 1290.
    ///
    /// A list of session identifiers and **not a boolean**, which is the
    /// whole of the design: line 1290 asks for an override *"for a specific
    /// task or session"*, so there is no spelling of this setting that means
    /// "always". The empty list and `None` both mean no override, and
    /// [`crate::routing::disposable::ReserveOverride`] documents why the
    /// scope travels with the value rather than being checked once at a call
    /// site.
    ///
    /// Same `None`-means-undecided reasoning as
    /// [`RoutingConfig::free_resource_order`]: a layer that has recorded
    /// nothing defers to the next one, and a layer that wants to record an
    /// explicit "no sessions, ignore the layer below" writes `Some(vec![])`.
    ///
    /// Not validated against the session store when loading or saving, for
    /// [`RoutingConfig::free_resource_pin`]'s reason: a session that has since
    /// been closed must not stop Glasshouse from starting, and a stale entry
    /// here is inert — it can only ever match a session with that identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reserve_override_sessions: Option<Vec<String>>,
    /// The routing-model fallback chain — capability map lines 1423 and
    /// 1795: the resources `glasshouse classify` tries next, in this order,
    /// when the model it chose (automatic or pinned) cannot be reached or
    /// does not answer in the schema. Each is tried at most once per
    /// classification, and an entry naming the model that already failed is
    /// skipped rather than retried.
    ///
    /// Two names per entry — exactly [`FreeResourceRef`]'s on-disk shape —
    /// because a chain entry is a reference to a provider and model
    /// configured elsewhere, never a base URL or a credential. An entry may
    /// name a metered model: like a pin, it is the user's own explicit
    /// instruction. Same `None`-means-undecided reasoning as
    /// [`RoutingConfig::free_resource_order`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model_fallback: Option<Vec<FreeResourceRef>>,
    /// Capability map line 1427: confine classification to local inference.
    /// When `true`, automatic selection admits only candidates the provider
    /// registry knows to be local, the fallback chain skips remote entries,
    /// and a pinned remote model is not called — nothing about a request
    /// leaves the machine for classification, and deterministic heuristics
    /// answer when no local model can. `None` defers to the next layer;
    /// the default is `false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    classification_local_only: Option<bool>,
    /// `[routing.reserve]` — capability map line 1577. `None` means this
    /// layer recorded neither scope's policy; see [`ReservePoliciesConfig`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reserve: Option<ReservePoliciesConfig>,
    /// Whether the session router decides where a launch's work goes —
    /// capability map line 1712, *"allow the user to disable automatic
    /// routing for the current Glasshouse instance."*
    ///
    /// `None` means this layer never said, exactly like every other field
    /// here; `Some(false)` is a person saying *stop deciding for me*, and
    /// [`EffectiveConfig::automatic_routing`] resolves it project over user
    /// over a default of `true`.
    ///
    /// # This is not [`RoutingModelChoice::Deterministic`]
    ///
    /// The two are easy to confuse and turn off different things.
    /// [`RoutingConfig::model`] chooses **what classifies a request** — a
    /// model, or deterministic heuristics — and a launch is ranked either
    /// way. This field turns the **ranking on the launch path** off
    /// altogether: `glasshouse launch` stops asking whether this project
    /// already has a session worth continuing, and starts the session the
    /// person's own flags describe.
    ///
    /// Off means off, including the *diagnosis*: see
    /// `main.rs::launch_session` for why a launch with routing disabled does
    /// not compute the ranking in order to report what it would have chosen.
    /// `glasshouse route` still answers that question on demand, because
    /// asking it is a thing a person does deliberately.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    automatic: Option<bool>,
}

impl RoutingConfig {
    /// The recorded choice, or `None` when this layer has never recorded one.
    pub fn model(&self) -> Option<&RoutingModelChoice> {
        self.model.as_ref()
    }

    /// Record `choice`, or `None` to return this layer to "never decided".
    pub fn set_model(&mut self, choice: Option<RoutingModelChoice>) -> &mut Self {
        self.model = choice;
        self
    }

    pub fn max_router_latency(&self) -> Option<RouterLatencyMs> {
        self.max_router_latency_ms
    }

    pub fn set_max_router_latency(&mut self, value: Option<RouterLatencyMs>) -> &mut Self {
        self.max_router_latency_ms = value;
        self
    }

    pub fn max_marginal_cost(&self) -> Option<RouterCostMicroUsd> {
        self.max_marginal_cost_micro_usd
    }

    pub fn set_max_marginal_cost(&mut self, value: Option<RouterCostMicroUsd>) -> &mut Self {
        self.max_marginal_cost_micro_usd = value;
        self
    }

    pub fn prefer_free(&self) -> Option<bool> {
        self.prefer_free
    }

    pub fn set_prefer_free(&mut self, value: Option<bool>) -> &mut Self {
        self.prefer_free = value;
        self
    }

    /// Whether this layer recorded an answer to *"may the router decide where
    /// a launch goes"* — map line 1712.
    pub fn automatic(&self) -> Option<bool> {
        self.automatic
    }

    pub fn set_automatic(&mut self, value: Option<bool>) -> &mut Self {
        self.automatic = value;
        self
    }

    pub fn premium_reserve(&self) -> Option<PremiumReservePercent> {
        self.premium_reserve_percent
    }

    pub fn set_premium_reserve(&mut self, value: Option<PremiumReservePercent>) -> &mut Self {
        self.premium_reserve_percent = value;
        self
    }

    /// This layer's recorded reserve overrides, or `None` for "never
    /// decided" — capability map line 1290.
    pub fn reserve_override_sessions(&self) -> Option<&[String]> {
        self.reserve_override_sessions.as_deref()
    }

    pub fn set_reserve_override_sessions(&mut self, value: Option<Vec<String>>) -> &mut Self {
        self.reserve_override_sessions = value;
        self
    }

    /// This layer's recorded fallback chain, or `None` for "never decided"
    /// — capability map lines 1423 and 1795.
    pub fn model_fallback(&self) -> Option<&[FreeResourceRef]> {
        self.model_fallback.as_deref()
    }

    pub fn set_model_fallback(&mut self, value: Option<Vec<FreeResourceRef>>) -> &mut Self {
        self.model_fallback = value;
        self
    }

    /// This layer's recorded local-only confinement, or `None` for "never
    /// decided" — capability map line 1427.
    pub fn classification_local_only(&self) -> Option<bool> {
        self.classification_local_only
    }

    pub fn set_classification_local_only(&mut self, value: Option<bool>) -> &mut Self {
        self.classification_local_only = value;
        self
    }

    /// This layer's recorded reserve policies, or `None` for "never
    /// decided" — capability map line 1577.
    pub fn reserve(&self) -> Option<ReservePoliciesConfig> {
        self.reserve
    }

    pub fn set_reserve(&mut self, value: Option<ReservePoliciesConfig>) -> &mut Self {
        self.reserve = value;
        self
    }

    /// This layer's recorded free-resource order, or `None` for "never
    /// decided".
    pub fn free_resource_order(&self) -> Option<&[FreeResourceRef]> {
        self.free_resource_order.as_deref()
    }

    pub fn set_free_resource_order(&mut self, value: Option<Vec<FreeResourceRef>>) -> &mut Self {
        self.free_resource_order = value;
        self
    }

    /// This layer's recorded disabled list, or `None` for "never decided".
    pub fn free_resource_disabled(&self) -> Option<&[FreeResourceRef]> {
        self.free_resource_disabled.as_deref()
    }

    pub fn set_free_resource_disabled(&mut self, value: Option<Vec<FreeResourceRef>>) -> &mut Self {
        self.free_resource_disabled = value;
        self
    }

    /// This layer's recorded pin, or `None` — either because no pin was
    /// recorded, or because none was ever chosen. See the field's own doc.
    pub fn free_resource_pin(&self) -> Option<&FreeResourceRef> {
        self.free_resource_pin.as_ref()
    }

    pub fn set_free_resource_pin(&mut self, value: Option<FreeResourceRef>) -> &mut Self {
        self.free_resource_pin = value;
        self
    }

    /// This layer's recorded capacity-band thresholds, or `None` for "never
    /// decided" — capability map line 1270.
    pub fn capacity_band_thresholds(&self) -> Option<CapacityBandThresholdsConfig> {
        self.capacity_band_thresholds
    }

    pub fn set_capacity_band_thresholds(
        &mut self,
        value: Option<CapacityBandThresholdsConfig>,
    ) -> &mut Self {
        self.capacity_band_thresholds = value;
        self
    }

    /// This layer's recorded routing score weights, or `None` for "never
    /// decided" — capability map lines 1357/1358.
    pub fn score_weights(&self) -> Option<ScoreWeightsConfig> {
        self.score_weights
    }

    pub fn set_score_weights(&mut self, value: Option<ScoreWeightsConfig>) -> &mut Self {
        self.score_weights = value;
        self
    }

    /// This layer's three free-resource preferences, folded into the shape
    /// [`crate::routing::disposable::DisposableRouting`] actually consumes.
    /// A layer that recorded nothing produces
    /// [`crate::routing::free::FreePreferences::new`]'s empty default for
    /// whichever of the three it never decided.
    pub fn free_preferences(&self) -> crate::routing::free::FreePreferences {
        crate::routing::free::FreePreferences::new()
            .with_order(
                self.free_resource_order
                    .as_ref()
                    .map(|order| order.iter().map(FreeResourceRef::to_key).collect())
                    .unwrap_or_default(),
            )
            .with_disabled(
                self.free_resource_disabled
                    .as_ref()
                    .map(|disabled| disabled.iter().map(FreeResourceRef::to_key).collect())
                    .unwrap_or_default(),
            )
            .with_pin(self.free_resource_pin.as_ref().map(FreeResourceRef::to_key))
    }

    /// Whether this table would serialize to nothing at all.
    fn is_unset(&self) -> bool {
        self.model.is_none()
            && self.max_router_latency_ms.is_none()
            && self.max_marginal_cost_micro_usd.is_none()
            && self.prefer_free.is_none()
            && self.premium_reserve_percent.is_none()
            && self.free_resource_order.is_none()
            && self.free_resource_disabled.is_none()
            && self.free_resource_pin.is_none()
            && self.capacity_band_thresholds.is_none()
            && self.score_weights.is_none()
            && self.reserve_override_sessions.is_none()
            && self.model_fallback.is_none()
            && self.classification_local_only.is_none()
            && self.reserve.is_none()
            && self.automatic.is_none()
    }
}

/// The `[guardrails]` table — Phase 21K, capability map lines 1008, 1052.
///
/// Two preferences, both optional so that a project can override one
/// without restating the other, and both `None` for "this layer never
/// decided" — the same three-state reasoning [`RoutingConfig::model`] gives.
/// The vocabularies are [`crate::guardrails`]' own, so a value this file
/// accepts is a value the gate understands, and the shipped defaults live
/// with the gate (`Policy::default_policy`) rather than here.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardrailsConfig {
    /// `off`, `advisory` (the default) or `risk_gated`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mode: Option<GuardrailMode>,
    /// Which categories may answer `gated` under `risk_gated`. Only the four
    /// [`BlockingCategory`] names parse; anything else is a load error that
    /// names the vocabulary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    blocking: Option<Vec<BlockingCategory>>,
}

impl GuardrailsConfig {
    /// Whether this layer recorded nothing at all — the `skip_serializing_if`
    /// predicate, so a user who never touched the guardrails has no
    /// `[guardrails]` table in their file.
    pub fn is_unset(&self) -> bool {
        self.mode.is_none() && self.blocking.is_none()
    }

    pub fn mode(&self) -> Option<GuardrailMode> {
        self.mode
    }

    pub fn set_mode(&mut self, mode: Option<GuardrailMode>) -> &mut Self {
        self.mode = mode;
        self
    }

    pub fn blocking(&self) -> Option<&[BlockingCategory]> {
        self.blocking.as_deref()
    }

    pub fn set_blocking(&mut self, blocking: Option<Vec<BlockingCategory>>) -> &mut Self {
        self.blocking = blocking;
        self
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
    /// `[entitlements.<name>]` — Phase 56. Skipped when empty: a user who
    /// configured none has no table, and every harness's own sign-in still
    /// resolves to a default entry through
    /// [`EffectiveConfig::entitlements`].
    #[serde(default, skip_serializing_if = "EntitlementTable::is_empty")]
    entitlements: EntitlementTable,
    /// Skipped when empty so a first run that declines the routing-model
    /// step writes no `[routing]` table at all — see [`RoutingConfig::model`].
    #[serde(default, skip_serializing_if = "RoutingConfig::is_unset")]
    routing: RoutingConfig,
    /// Pairing metadata corrections — Phase 9J line 561. Skipped when empty
    /// for the same reason `routing` is: a user who never corrected a
    /// pairing has no `[pairing]` table in their file at all.
    #[serde(default, skip_serializing_if = "pairing::PairingConfig::is_unset")]
    pairing: pairing::PairingConfig,
    /// Response-profile configuration — Phase 9K lines 593–597. Skipped when
    /// empty for the same reason `pairing` is: a user who never chose a
    /// response profile has no `[response]` table, and Glasshouse applies
    /// nothing to their harness.
    #[serde(default, skip_serializing_if = "response::ResponseConfig::is_unset")]
    response: response::ResponseConfig,
    /// Whether Glasshouse's automatic post-turn memory-extraction trigger
    /// (Phase 21) may run in this project. `None` means "never decided" and
    /// resolves to enabled — the same reasoning [`RoutingConfig::model`]
    /// documents for why this stays an `Option` rather than a plain `bool`:
    /// a project that wants to record an explicit "off" over a user-level
    /// "on" needs a third state to override, not just two.
    ///
    /// Independent of [`RoutingConfig::model`] and
    /// [`response::ResponseConfig`]'s own `enabled` field by construction —
    /// each lives in its own table/field and is read by
    /// [`EffectiveConfig::memory_extraction_enabled`] alone, so setting one
    /// never touches another. See
    /// `tests::the_three_automatic_behaviours_disable_independently`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    memory_extraction: Option<bool>,
    /// Whether Glasshouse may take a checkpoint automatically at a task
    /// boundary (Phase 19 line 802), without being asked. `None` means
    /// "never decided" and resolves to enabled, for the same reason
    /// [`Self::memory_extraction`] stays an `Option` rather than a plain
    /// `bool`.
    ///
    /// Independent of [`Self::memory_extraction`] and every other automatic
    /// behaviour by construction — this is its own field, read by
    /// [`EffectiveConfig::automatic_checkpoint_enabled`] alone, so disabling
    /// memory extraction never disables this and vice versa.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    automatic_checkpoint: Option<bool>,
    /// Whether Glasshouse delivers its own project implementation policy
    /// (`crate::policy`) to an agent it briefs. `None` means "never decided"
    /// and resolves to enabled, for the same reason
    /// [`Self::memory_extraction`] stays an `Option` rather than a plain
    /// `bool`.
    ///
    /// **The default is on, and that is the decision rather than the safe
    /// choice**: a policy nobody receives is not a policy, and an off default
    /// silently wins every comparison nobody runs. A team that does not want
    /// Glasshouse speaking into its agents' context turns it off here, and
    /// what remains is coherent — the briefing and the task arrive exactly as
    /// they did before the policy existed.
    ///
    /// Independent of [`Self::memory_extraction`] and every other automatic
    /// behaviour by construction: its own field, read by
    /// [`EffectiveConfig::implementation_policy_enabled`] alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    implementation_policy: Option<bool>,
    /// Which model performs memory extraction, or `None` for "the user has
    /// not chosen one" — Phase 21 line 834. See [`ExtractionModelRef`] for
    /// why this is a field of its own rather than a reading of the routing
    /// preferences, and [`EffectiveConfig::memory_extraction_model`] for how
    /// a project may override it.
    ///
    /// **`None` is the default and means no model is ever called.** This is
    /// the whole of the consent: nothing else in this file, and nothing in
    /// the provider table, turns memory extraction into an outbound request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    memory_extraction_model: Option<ExtractionModelRef>,
    /// The assumption guardrail's mode and blocking list — Phase 21K. Skipped
    /// when empty for the same reason `routing` is. Read by
    /// [`EffectiveConfig::guardrail_mode`] and
    /// [`EffectiveConfig::guardrail_blocking`] alone, so setting it never
    /// touches another automatic behaviour.
    #[serde(default, skip_serializing_if = "GuardrailsConfig::is_unset")]
    guardrails: GuardrailsConfig,
    /// The context firewall's mode and thresholds — Phase 57 map lines
    /// 1991-1996. Skipped when empty for the same reason `guardrails` is: a
    /// user who never touched it has no `[context_firewall]` table, and
    /// [`EffectiveConfig::context_firewall_mode`] resolves the missing case
    /// to `off`.
    #[serde(
        default,
        skip_serializing_if = "firewall::ContextFirewallConfig::is_unset"
    )]
    context_firewall: firewall::ContextFirewallConfig,
}

impl Default for UserConfig {
    fn default() -> Self {
        Self {
            version: CURRENT_SCHEMA_VERSION,
            onboarding: OnboardingState::default(),
            integrations: IntegrationTable::default(),
            profiles: ProfileTable::default(),
            providers: ProviderTable::default(),
            entitlements: EntitlementTable::default(),
            routing: RoutingConfig::default(),
            pairing: pairing::PairingConfig::default(),
            response: response::ResponseConfig::default(),
            memory_extraction: None,
            automatic_checkpoint: None,
            implementation_policy: None,
            memory_extraction_model: None,
            guardrails: GuardrailsConfig::default(),
            context_firewall: firewall::ContextFirewallConfig::default(),
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

    pub fn entitlements(&self) -> &EntitlementTable {
        &self.entitlements
    }

    pub fn entitlements_mut(&mut self) -> &mut EntitlementTable {
        &mut self.entitlements
    }

    pub fn routing(&self) -> &RoutingConfig {
        &self.routing
    }

    pub fn routing_mut(&mut self) -> &mut RoutingConfig {
        &mut self.routing
    }

    pub fn pairing(&self) -> &pairing::PairingConfig {
        &self.pairing
    }

    pub fn pairing_mut(&mut self) -> &mut pairing::PairingConfig {
        &mut self.pairing
    }

    pub fn response(&self) -> &response::ResponseConfig {
        &self.response
    }

    pub fn response_mut(&mut self) -> &mut response::ResponseConfig {
        &mut self.response
    }

    /// This layer's recorded decision on the automatic memory-extraction
    /// trigger, or `None` for "never decided". See the field's own doc.
    pub fn memory_extraction(&self) -> Option<bool> {
        self.memory_extraction
    }

    /// The user's decision on delivering the implementation policy, or `None`
    /// for "never decided". See [`Self::implementation_policy`].
    pub fn implementation_policy(&self) -> Option<bool> {
        self.implementation_policy
    }

    pub fn set_implementation_policy(&mut self, enabled: Option<bool>) -> &mut Self {
        self.implementation_policy = enabled;
        self
    }

    pub fn set_memory_extraction(&mut self, enabled: Option<bool>) -> &mut Self {
        self.memory_extraction = enabled;
        self
    }
    /// The model this user has chosen to perform memory extraction, or
    /// `None` — see [`UserConfig::memory_extraction_model`].
    pub fn memory_extraction_model(&self) -> Option<&ExtractionModelRef> {
        self.memory_extraction_model.as_ref()
    }

    pub fn set_memory_extraction_model(&mut self, model: Option<ExtractionModelRef>) -> &mut Self {
        self.memory_extraction_model = model;
        self
    }

    /// This layer's `[guardrails]` table — see [`GuardrailsConfig`].
    pub fn guardrails(&self) -> &GuardrailsConfig {
        &self.guardrails
    }

    pub fn guardrails_mut(&mut self) -> &mut GuardrailsConfig {
        &mut self.guardrails
    }

    /// This layer's `[context_firewall]` table — see
    /// [`firewall::ContextFirewallConfig`].
    pub fn context_firewall(&self) -> &firewall::ContextFirewallConfig {
        &self.context_firewall
    }

    pub fn context_firewall_mut(&mut self) -> &mut firewall::ContextFirewallConfig {
        &mut self.context_firewall
    }

    /// This layer's recorded decision on automatic task-boundary
    /// checkpoints, or `None` for "never decided". See the field's own doc.
    pub fn automatic_checkpoint(&self) -> Option<bool> {
        self.automatic_checkpoint
    }

    pub fn set_automatic_checkpoint(&mut self, enabled: Option<bool>) -> &mut Self {
        self.automatic_checkpoint = enabled;
        self
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
    /// `CURRENT_SCHEMA_VERSION` and never hits it.
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
    /// A project's `[entitlements.<name>]` entries replace the user's
    /// **by name**, whole — the rule [`ProviderTable`] follows — see
    /// [`EffectiveConfig::entitlements`].
    #[serde(default, skip_serializing_if = "EntitlementTable::is_empty")]
    entitlements: EntitlementTable,
    /// A project may override the routing-model choice, unlike
    /// [`IntegrationConfig::bypass_acknowledged`] — see
    /// [`EffectiveConfig::routing_model`] for why this is a preference and
    /// that one is not.
    #[serde(default, skip_serializing_if = "RoutingConfig::is_unset")]
    routing: RoutingConfig,
    /// A project may correct pairing metadata for the models its own work
    /// uses, and its corrections win per key over the user's — see
    /// [`EffectiveConfig::pairing_overrides`].
    #[serde(default, skip_serializing_if = "pairing::PairingConfig::is_unset")]
    pairing: pairing::PairingConfig,
    /// A project may set its own response profile, and it reaches no other
    /// project — line 597. See `crate::config::response`'s own header.
    #[serde(default, skip_serializing_if = "response::ResponseConfig::is_unset")]
    response: response::ResponseConfig,
    /// A project may override the user's decision on the automatic
    /// memory-extraction trigger — see [`UserConfig::memory_extraction`] for
    /// the field this mirrors and [`EffectiveConfig::memory_extraction_enabled`]
    /// for how the two layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    memory_extraction: Option<bool>,
    /// A project may override the user's decision on automatic
    /// task-boundary checkpoints — see [`UserConfig::automatic_checkpoint`]
    /// for the field this mirrors and
    /// [`EffectiveConfig::automatic_checkpoint_enabled`] for how the two
    /// layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    automatic_checkpoint: Option<bool>,
    /// A project may override the user's decision on delivering the
    /// implementation policy — see [`UserConfig::implementation_policy`] for
    /// the field this mirrors and
    /// [`EffectiveConfig::implementation_policy_enabled`] for how the two
    /// layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    implementation_policy: Option<bool>,
    /// A project may name its own extraction model — see
    /// [`UserConfig::memory_extraction_model`] for the field this mirrors and
    /// [`EffectiveConfig::memory_extraction_model`] for how the two layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    memory_extraction_model: Option<ExtractionModelRef>,
    /// A project may set its own guardrail mode and blocking list — see
    /// [`UserConfig::guardrails`] for the table this mirrors and
    /// [`EffectiveConfig::guardrail_mode`] for how the two layer.
    #[serde(default, skip_serializing_if = "GuardrailsConfig::is_unset")]
    guardrails: GuardrailsConfig,
    /// A project may set its own context-firewall mode and thresholds — see
    /// [`UserConfig::context_firewall`] for the table this mirrors and
    /// [`EffectiveConfig::context_firewall_mode`] for how the two layer.
    #[serde(
        default,
        skip_serializing_if = "firewall::ContextFirewallConfig::is_unset"
    )]
    context_firewall: firewall::ContextFirewallConfig,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            version: CURRENT_SCHEMA_VERSION,
            integrations: IntegrationTable::default(),
            profiles: ProfileTable::default(),
            providers: ProviderTable::default(),
            entitlements: EntitlementTable::default(),
            routing: RoutingConfig::default(),
            pairing: pairing::PairingConfig::default(),
            response: response::ResponseConfig::default(),
            memory_extraction: None,
            automatic_checkpoint: None,
            implementation_policy: None,
            memory_extraction_model: None,
            guardrails: GuardrailsConfig::default(),
            context_firewall: firewall::ContextFirewallConfig::default(),
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

    pub fn entitlements(&self) -> &EntitlementTable {
        &self.entitlements
    }

    pub fn entitlements_mut(&mut self) -> &mut EntitlementTable {
        &mut self.entitlements
    }

    pub fn routing(&self) -> &RoutingConfig {
        &self.routing
    }

    pub fn routing_mut(&mut self) -> &mut RoutingConfig {
        &mut self.routing
    }

    pub fn pairing(&self) -> &pairing::PairingConfig {
        &self.pairing
    }

    pub fn pairing_mut(&mut self) -> &mut pairing::PairingConfig {
        &mut self.pairing
    }

    pub fn response(&self) -> &response::ResponseConfig {
        &self.response
    }

    pub fn response_mut(&mut self) -> &mut response::ResponseConfig {
        &mut self.response
    }

    /// This layer's recorded decision on the automatic memory-extraction
    /// trigger, or `None` for "never decided". See [`UserConfig::memory_extraction`].
    pub fn memory_extraction(&self) -> Option<bool> {
        self.memory_extraction
    }

    /// The project's decision on delivering the implementation policy, or
    /// `None` for "never decided". See [`UserConfig::implementation_policy`].
    pub fn implementation_policy(&self) -> Option<bool> {
        self.implementation_policy
    }

    pub fn set_implementation_policy(&mut self, enabled: Option<bool>) -> &mut Self {
        self.implementation_policy = enabled;
        self
    }

    pub fn set_memory_extraction(&mut self, enabled: Option<bool>) -> &mut Self {
        self.memory_extraction = enabled;
        self
    }
    /// The model this project has chosen to perform memory extraction, or
    /// `None` — see [`ProjectConfig::memory_extraction_model`].
    pub fn memory_extraction_model(&self) -> Option<&ExtractionModelRef> {
        self.memory_extraction_model.as_ref()
    }

    pub fn set_memory_extraction_model(&mut self, model: Option<ExtractionModelRef>) -> &mut Self {
        self.memory_extraction_model = model;
        self
    }

    /// This layer's `[guardrails]` table — see [`GuardrailsConfig`].
    pub fn guardrails(&self) -> &GuardrailsConfig {
        &self.guardrails
    }

    pub fn guardrails_mut(&mut self) -> &mut GuardrailsConfig {
        &mut self.guardrails
    }

    /// This layer's `[context_firewall]` table — see
    /// [`UserConfig::context_firewall`] for the table this mirrors and
    /// [`EffectiveConfig::context_firewall_mode`] for how the two layer.
    pub fn context_firewall(&self) -> &firewall::ContextFirewallConfig {
        &self.context_firewall
    }

    pub fn context_firewall_mut(&mut self) -> &mut firewall::ContextFirewallConfig {
        &mut self.context_firewall
    }

    /// This layer's recorded decision on automatic task-boundary
    /// checkpoints, or `None` for "never decided". See
    /// [`UserConfig::automatic_checkpoint`].
    pub fn automatic_checkpoint(&self) -> Option<bool> {
        self.automatic_checkpoint
    }

    pub fn set_automatic_checkpoint(&mut self, enabled: Option<bool>) -> &mut Self {
        self.automatic_checkpoint = enabled;
        self
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

impl Layer {
    /// Where a value this layer supplied came from, as a phrase that reads
    /// inside a sentence — "disabled *in your configuration*". Deliberately
    /// says nothing about *where on disk*: a refusal a person reads on their
    /// terminal must not print an absolute path (`crate::secret`'s own rule
    /// about what a message may carry), and the file a layer means is
    /// already `glasshouse doctor`'s job to name.
    pub fn describe_source(self) -> &'static str {
        match self {
            Self::Project => "in this project's configuration",
            Self::User => "in your configuration",
            Self::Default => "by default",
        }
    }
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

    /// Whether the launch profile `name` may be *started*, reporting which
    /// layer decided it. See [`ProfileConfig::enabled`], the field this
    /// reads.
    ///
    /// # Why this is a separate query rather than a filter inside `profile_names`
    ///
    /// [`EffectiveConfig::profile_names`] means *every configured profile
    /// name*, and the surfaces that list profiles need exactly that: a user
    /// has to be able to see a disabled profile in order to re-enable it,
    /// which is the whole of that field's "disable is not delete" rule.
    /// Filtering inside a general accessor would make a disabled profile
    /// invisible rather than unavailable, and would do it to every caller at
    /// once — including [`ProfileLookupError::Unknown`]'s own list of valid
    /// names, where a missing name reads as a typo. So the filter belongs at
    /// the one site that decides which profiles the router may *consider*
    /// (`main.rs`'s `routing_destinations`, under its `Everything` scope),
    /// and this is the question that site asks.
    ///
    /// # Which layer wins, and why the whole definition decides it
    ///
    /// Project first, then user, then [`Layer::Default`] — the same order as
    /// [`EffectiveConfig::enabled`] and every other lookup on this type
    /// except [`EffectiveConfig::bypass_acknowledged`].
    ///
    /// The layer is picked exactly as [`EffectiveConfig::launch_profile`]
    /// picks it, and that is not a free choice. `launch_profile` takes the
    /// winning layer's [`ProfileConfig`] **whole** — harness, backend,
    /// model, approval and preset all come out of one file — so resolving
    /// `enabled` on its own could produce a profile whose body came from the
    /// project and whose enable decision came from the user, which is a
    /// profile neither layer ever wrote.
    ///
    /// The consequence, recorded because it is real: a project that defines
    /// `[profiles.foo]` at all supplies `enabled` for it, defaulting to
    /// `true` — so it re-enables a `foo` the user disabled.
    /// [`ProfileConfig::enabled`] is a plain `bool` rather than
    /// [`IntegrationConfig::enabled`]'s tri-state `Option<bool>`, so a
    /// project has no way to say "I define this profile and leave the enable
    /// decision alone". This grants a project nothing it did not already
    /// have — it can define `[profiles.anything-else]` and have that offered
    /// — and it cannot escalate approval, because
    /// [`ProfileApproval::Bypass`] still needs
    /// [`EffectiveConfig::bypass_acknowledged`], which is read from the user
    /// layer alone.
    ///
    /// # The implied Native profile is always enabled
    ///
    /// [`crate::profile::NATIVE_PROFILE_NAME`] answers `true` at
    /// [`Layer::Default`] without consulting either table, mirroring
    /// `launch_profile`'s own short circuit: the Native profile exists for
    /// every harness *by construction* rather than as a configuration entry
    /// — see [`ProfileTable`], which never stores it — so there is no entry
    /// to disable. That is what keeps a person from configuring their way
    /// into having nowhere to launch: `profile_names` always contains it,
    /// `launch_profile` always resolves it, so the enabled candidate set is
    /// never empty and the "you have disabled everything" refusal this would
    /// otherwise need is unreachable rather than merely unwritten.
    ///
    /// An unknown name answers `true` at [`Layer::Default`] too. "Disabled"
    /// and "never configured" are different facts and `launch_profile`
    /// already reports the second one as [`ProfileLookupError::Unknown`];
    /// answering "disabled" here would hand a caller a second, wronger
    /// refusal for the same typo.
    pub fn profile_enabled(&self, name: &str) -> Layered<bool> {
        if name == crate::profile::NATIVE_PROFILE_NAME {
            return Layered::new(true, Layer::Default);
        }
        if let Some(config) = self.project.and_then(|p| p.profiles().get(name)) {
            return Layered::new(config.enabled(), Layer::Project);
        }
        if let Some(config) = self.user.profiles().get(name) {
            return Layered::new(config.enabled(), Layer::User);
        }
        Layered::new(true, Layer::Default)
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

    /// Whether Glasshouse's automatic post-turn memory-extraction trigger
    /// (Phase 21) may run, reporting which layer decided it. Project first,
    /// then user, then [`Layer::Default`], matching every other lookup on
    /// this type except [`EffectiveConfig::bypass_acknowledged`].
    ///
    /// Deliberately independent of [`EffectiveConfig::routing_model`] and
    /// response-profile injection: each reads its own field, so disabling one
    /// automatic behaviour never disables another. See
    /// `tests::the_three_automatic_behaviours_disable_independently`.
    pub fn memory_extraction_enabled(&self) -> Layered<bool> {
        if let Some(value) = self.project.and_then(ProjectConfig::memory_extraction) {
            return Layered::new(value, Layer::Project);
        }
        if let Some(value) = self.user.memory_extraction() {
            return Layered::new(value, Layer::User);
        }
        Layered::new(true, Layer::Default)
    }

    /// Whether Glasshouse delivers its own implementation policy
    /// (`crate::policy`, capability map lines 955-990) to an agent it briefs,
    /// reporting which layer decided it. Project first, then user, then
    /// [`Layer::Default`], matching [`Self::memory_extraction_enabled`]'s own
    /// layering.
    ///
    /// [`Layer::Default`] carries `true`: the policy is what Glasshouse has
    /// to say about how work is implemented here, and a default of off would
    /// ship the opposite of the decision that put it in the product.
    ///
    /// Deliberately independent of every other automatic behaviour: each
    /// reads its own field, so turning memory extraction off never turns this
    /// off and vice versa — the property
    /// `config::response::tests::the_three_automatic_behaviours_disable_independently`
    /// asserts for the three that came before it, and
    /// `implementation_policy::the_policy_is_not_delivered_when_turned_off_and_never_repeated_to_the_same_session`
    /// asserts end to end for this one.
    pub fn implementation_policy_enabled(&self) -> Layered<bool> {
        if let Some(value) = self.project.and_then(ProjectConfig::implementation_policy) {
            return Layered::new(value, Layer::Project);
        }
        if let Some(value) = self.user.implementation_policy() {
            return Layered::new(value, Layer::User);
        }
        Layered::new(true, Layer::Default)
    }

    /// Which model may perform memory extraction, and which layer chose it —
    /// Phase 21 line 834. Project first, then user, then [`Layer::Default`],
    /// matching [`Self::memory_extraction_enabled`]'s own layering.
    ///
    /// [`Layer::Default`] carries `None`, and `None` is the answer that keeps
    /// today's behaviour: no model is called. **A project naming a model
    /// overrides a user who named none** — the same direction every other
    /// lookup on this type resolves, and what lets one repository use a local
    /// runner without the user turning it on everywhere.
    ///
    /// Deliberately independent of [`Self::memory_extraction_enabled`]: that
    /// one is whether the trigger may fire at all, this one is what it asks
    /// when it does, and a user who turns the trigger off has turned the
    /// model off with it whatever this says.
    pub fn memory_extraction_model(&self) -> Layered<Option<ExtractionModelRef>> {
        if let Some(value) = self
            .project
            .and_then(ProjectConfig::memory_extraction_model)
        {
            return Layered::new(Some(value.clone()), Layer::Project);
        }
        if let Some(value) = self.user.memory_extraction_model() {
            return Layered::new(Some(value.clone()), Layer::User);
        }
        Layered::new(None, Layer::Default)
    }

    /// Whether Glasshouse may take a checkpoint automatically at a task
    /// boundary (Phase 19 line 802), reporting which layer decided it.
    /// Project first, then user, then [`Layer::Default`], matching
    /// [`Self::memory_extraction_enabled`]'s own layering.
    ///
    /// Deliberately independent of [`Self::memory_extraction_enabled`] and
    /// every other automatic behaviour: each reads its own field, so
    /// disabling one never disables another.
    pub fn automatic_checkpoint_enabled(&self) -> Layered<bool> {
        if let Some(value) = self.project.and_then(ProjectConfig::automatic_checkpoint) {
            return Layered::new(value, Layer::Project);
        }
        if let Some(value) = self.user.automatic_checkpoint() {
            return Layered::new(value, Layer::User);
        }
        Layered::new(true, Layer::Default)
    }

    /// `guardrails.mode`, and which layer set it — Phase 21K line 1052.
    /// Project first, then user, then [`Layer::Default`] carrying
    /// `advisory`, matching every other lookup on this type.
    ///
    /// Deliberately independent of every other automatic behaviour: it
    /// reads its own table, so turning the guardrail off never turns off
    /// memory extraction or checkpoints, and vice versa.
    pub fn guardrail_mode(&self) -> Layered<GuardrailMode> {
        if let Some(value) = self.project.and_then(|p| p.guardrails().mode()) {
            return Layered::new(value, Layer::Project);
        }
        if let Some(value) = self.user.guardrails().mode() {
            return Layered::new(value, Layer::User);
        }
        Layered::new(GuardrailMode::Advisory, Layer::Default)
    }

    /// `guardrails.blocking`, and which layer set it. [`Layer::Default`]
    /// carries the design ruling's list, [`guardrails::DEFAULT_BLOCKING`].
    /// A layer that records an explicit empty list means *nothing may
    /// block*, which is a different fact from recording nothing.
    pub fn guardrail_blocking(&self) -> Layered<Vec<BlockingCategory>> {
        if let Some(value) = self.project.and_then(|p| p.guardrails().blocking()) {
            return Layered::new(value.to_vec(), Layer::Project);
        }
        if let Some(value) = self.user.guardrails().blocking() {
            return Layered::new(value.to_vec(), Layer::User);
        }
        Layered::new(guardrails::DEFAULT_BLOCKING.to_vec(), Layer::Default)
    }

    /// Both guardrail preferences as the gate reads them, each with the
    /// phrase naming its layer, and no per-task override — the door adds
    /// that from the session's own ledger.
    pub fn guardrail_policy(&self) -> guardrails::Policy {
        let mode = self.guardrail_mode();
        let blocking = self.guardrail_blocking();
        guardrails::Policy {
            mode: mode.value,
            mode_source: mode.layer.describe_source(),
            blocking: blocking.value,
            blocking_source: blocking.layer.describe_source(),
            override_: None,
        }
    }

    /// `context_firewall.mode`, and which layer set it — Phase 57 map line
    /// 1991. Project first, then user, then [`Layer::Default`] carrying
    /// [`firewall::FirewallMode::Off`], matching every other lookup on this
    /// type except [`EffectiveConfig::bypass_acknowledged`].
    ///
    /// [`Layer::Default`] carrying `off` is the decision, not merely the
    /// safe choice: a session nobody configured must launch with a command
    /// line byte-identical to one built before this phase existed, and that
    /// is only true if the missing case never registers a hook.
    pub fn context_firewall_mode(&self) -> Layered<firewall::FirewallMode> {
        if let Some(value) = self.project.and_then(|p| p.context_firewall().mode()) {
            return Layered::new(value, Layer::Project);
        }
        if let Some(value) = self.user.context_firewall().mode() {
            return Layered::new(value, Layer::User);
        }
        Layered::new(firewall::FirewallMode::Off, Layer::Default)
    }

    /// The passthrough-token threshold for `mode`, and which layer set it.
    ///
    /// Reads a different field per mode — `aggressive_passthrough_tokens`
    /// under [`firewall::FirewallMode::Aggressive`],
    /// `passthrough_tokens` under every other mode — so aggressive's own
    /// threshold can move without safe's changing underneath it (map line
    /// 1991's one permitted difference between the two). `off` and `shadow`
    /// still resolve a value here because `shadow` runs the full pipeline
    /// and needs a real threshold even though it never emits reduced text.
    pub fn context_firewall_passthrough_tokens(
        &self,
        mode: firewall::FirewallMode,
    ) -> Layered<u64> {
        let (project_value, user_value, default_value) =
            if mode == firewall::FirewallMode::Aggressive {
                (
                    self.project
                        .and_then(|p| p.context_firewall().aggressive_passthrough_tokens()),
                    self.user.context_firewall().aggressive_passthrough_tokens(),
                    firewall::DEFAULT_AGGRESSIVE_PASSTHROUGH_TOKENS,
                )
            } else {
                (
                    self.project
                        .and_then(|p| p.context_firewall().passthrough_tokens()),
                    self.user.context_firewall().passthrough_tokens(),
                    firewall::DEFAULT_PASSTHROUGH_TOKENS,
                )
            };
        if let Some(value) = project_value {
            return Layered::new(value, Layer::Project);
        }
        if let Some(value) = user_value {
            return Layered::new(value, Layer::User);
        }
        Layered::new(default_value, Layer::Default)
    }

    /// `context_firewall.reducer` — map line 1997's opt-in: `None` (the
    /// [`Layer::Default`] case, and the only state a user who never
    /// configured a reducer has) disables semantic reduction in every mode,
    /// per map line 1992's guarantee.
    pub fn context_firewall_reducer(&self) -> Layered<Option<String>> {
        if let Some(value) = self.project.and_then(|p| p.context_firewall().reducer()) {
            return Layered::new(Some(value.to_owned()), Layer::Project);
        }
        if let Some(value) = self.user.context_firewall().reducer() {
            return Layered::new(Some(value.to_owned()), Layer::User);
        }
        Layered::new(None, Layer::Default)
    }

    /// `context_firewall.reducer_model` — map line 2002's pin. `None` lets
    /// `DisposableRouting` choose among whatever
    /// [`EffectiveConfig::context_firewall_reducer`] names.
    pub fn context_firewall_reducer_model(&self) -> Layered<Option<String>> {
        if let Some(value) = self
            .project
            .and_then(|p| p.context_firewall().reducer_model())
        {
            return Layered::new(Some(value.to_owned()), Layer::Project);
        }
        if let Some(value) = self.user.context_firewall().reducer_model() {
            return Layered::new(Some(value.to_owned()), Layer::User);
        }
        Layered::new(None, Layer::Default)
    }

    /// `context_firewall.min_semantic_tokens` — map line 1997's gate,
    /// defaulting to [`firewall::DEFAULT_MIN_SEMANTIC_TOKENS`].
    pub fn context_firewall_min_semantic_tokens(&self) -> Layered<u64> {
        if let Some(value) = self
            .project
            .and_then(|p| p.context_firewall().min_semantic_tokens())
        {
            return Layered::new(value, Layer::Project);
        }
        if let Some(value) = self.user.context_firewall().min_semantic_tokens() {
            return Layered::new(value, Layer::User);
        }
        Layered::new(firewall::DEFAULT_MIN_SEMANTIC_TOKENS, Layer::Default)
    }

    /// `context_firewall.aggressive_drops_uncertain` — map line 2000's
    /// explicit opt-in for aggressive mode to drop `uncertain` candidates.
    /// Defaults to `false`: bias to inclusion is the state nobody had to ask
    /// for.
    pub fn context_firewall_aggressive_drops_uncertain(&self) -> Layered<bool> {
        if let Some(value) = self
            .project
            .and_then(|p| p.context_firewall().aggressive_drops_uncertain())
        {
            return Layered::new(value, Layer::Project);
        }
        if let Some(value) = self.user.context_firewall().aggressive_drops_uncertain() {
            return Layered::new(value, Layer::User);
        }
        Layered::new(false, Layer::Default)
    }

    /// `context_firewall.reducer_local_only` — map line 2003's local-only
    /// operation. Defaults to `false`, matching every reducer field's
    /// "nobody configured anything" state.
    pub fn context_firewall_reducer_local_only(&self) -> Layered<bool> {
        if let Some(value) = self
            .project
            .and_then(|p| p.context_firewall().reducer_local_only())
        {
            return Layered::new(value, Layer::Project);
        }
        if let Some(value) = self.user.context_firewall().reducer_local_only() {
            return Layered::new(value, Layer::User);
        }
        Layered::new(false, Layer::Default)
    }

    /// Resolve which routing model classifies requests, reporting which
    /// layer decided it.
    ///
    /// Project first, then user, then [`Layer::Default`] — the ordinary
    /// layering every lookup on this type uses except
    /// [`EffectiveConfig::bypass_acknowledged`], which consults the user
    /// layer alone. This is deliberately *not* that exception. A bypass
    /// acknowledgement is a safety attestation a person makes about their
    /// own machine, so a repository must not be able to pre-acknowledge one
    /// on behalf of whoever cloned it. A routing-model choice is a
    /// preference about which cheap classifier to ask, it grants nothing and
    /// attests to nothing, and a project that wants its own — deterministic
    /// only in a repository whose work is uniform, say — is making an
    /// ordinary configuration statement. So the normal rule applies.
    ///
    /// The [`Layer::Default`] case is [`RoutingModelChoice::Deterministic`]:
    /// with nothing recorded anywhere, deterministic heuristics classify,
    /// which is exactly Phase 2C line 4.
    pub fn routing_model(&self) -> Layered<RoutingModelChoice> {
        if let Some(choice) = self.project.and_then(|p| p.routing().model()) {
            return Layered::new(choice.clone(), Layer::Project);
        }
        if let Some(choice) = self.user.routing().model() {
            return Layered::new(choice.clone(), Layer::User);
        }
        Layered::new(RoutingModelChoice::Deterministic, Layer::Default)
    }

    /// Maximum router latency, resolved per field so a project can override
    /// this limit without copying any other routing preference.
    pub fn max_router_latency(&self) -> Layered<RouterLatencyMs> {
        if let Some(value) = self.project.and_then(|p| p.routing().max_router_latency()) {
            return Layered::new(value, Layer::Project);
        }
        if let Some(value) = self.user.routing().max_router_latency() {
            return Layered::new(value, Layer::User);
        }
        Layered::new(RouterLatencyMs::DEFAULT, Layer::Default)
    }

    /// Maximum marginal cost of one routing decision, resolved per field.
    pub fn max_router_cost(&self) -> Layered<RouterCostMicroUsd> {
        if let Some(value) = self.project.and_then(|p| p.routing().max_marginal_cost()) {
            return Layered::new(value, Layer::Project);
        }
        if let Some(value) = self.user.routing().max_marginal_cost() {
            return Layered::new(value, Layer::User);
        }
        Layered::new(RouterCostMicroUsd::DEFAULT, Layer::Default)
    }

    /// Whether zero-marginal-cost resources are preferred after capability,
    /// health, rate-limit, and latency requirements are satisfied.
    pub fn prefer_free_routing(&self) -> Layered<bool> {
        if let Some(value) = self.project.and_then(|p| p.routing().prefer_free()) {
            return Layered::new(value, Layer::Project);
        }
        if let Some(value) = self.user.routing().prefer_free() {
            return Layered::new(value, Layer::User);
        }
        Layered::new(true, Layer::Default)
    }

    /// Whether `glasshouse launch` may rank destinations at all — capability
    /// map line 1712.
    ///
    /// Project over user over a default of `true`, like every other routing
    /// preference: the ranking is what Glasshouse has always done, so the
    /// default has to be the behaviour that existed before this switch did.
    ///
    /// `false` does not mean *"route badly"* — it means the launch path takes
    /// no routing decision. What the person's own flags say still happens:
    /// see `main.rs::launch_session`, which reads this before it opens
    /// anything the ranking would have needed.
    pub fn automatic_routing(&self) -> Layered<bool> {
        if let Some(value) = self.project.and_then(|p| p.routing().automatic()) {
            return Layered::new(value, Layer::Project);
        }
        if let Some(value) = self.user.routing().automatic() {
            return Layered::new(value, Layer::User);
        }
        Layered::new(true, Layer::Default)
    }

    /// The sessions the user overrode reserve protection for — capability
    /// map line 1290 — resolved per field, project over user over the empty
    /// default, exactly like [`Self::free_resource_disabled`].
    ///
    /// First layer wins outright rather than the two being unioned. A union
    /// would mean a project could add sessions a user's own configuration
    /// never named and the user had no single place to look to see what was
    /// overridden; every other list on this type resolves the same way, and
    /// this one is a *spending* control, which is the last place to invent a
    /// second resolution rule.
    pub fn reserve_override_sessions(&self) -> Layered<Vec<String>> {
        if let Some(value) = self
            .project
            .and_then(|p| p.routing().reserve_override_sessions())
        {
            return Layered::new(value.to_vec(), Layer::Project);
        }
        if let Some(value) = self.user.routing().reserve_override_sessions() {
            return Layered::new(value.to_vec(), Layer::User);
        }
        Layered::new(Vec::new(), Layer::Default)
    }

    /// The routing-model fallback chain — capability map lines 1423 and
    /// 1795 — resolved per field, project over user over the empty default,
    /// exactly like [`Self::free_resource_order`]. First layer wins outright
    /// rather than the two being concatenated, for
    /// [`Self::reserve_override_sessions`]'s reason: a chain is a list of
    /// models Glasshouse may *call on the user's behalf*, and a project must
    /// not be able to append one the user's own configuration never named.
    pub fn routing_model_fallback(&self) -> Layered<Vec<FreeResourceRef>> {
        if let Some(value) = self.project.and_then(|p| p.routing().model_fallback()) {
            return Layered::new(value.to_vec(), Layer::Project);
        }
        if let Some(value) = self.user.routing().model_fallback() {
            return Layered::new(value.to_vec(), Layer::User);
        }
        Layered::new(Vec::new(), Layer::Default)
    }

    /// Whether classification is confined to local inference — capability
    /// map line 1427 — resolved per field; `false` when neither layer
    /// decided.
    pub fn classification_local_only(&self) -> Layered<bool> {
        if let Some(value) = self
            .project
            .and_then(|p| p.routing().classification_local_only())
        {
            return Layered::new(value, Layer::Project);
        }
        if let Some(value) = self.user.routing().classification_local_only() {
            return Layered::new(value, Layer::User);
        }
        Layered::new(false, Layer::Default)
    }

    /// Premium remaining-capacity threshold below which reserve protection
    /// applies, resolved per field.
    pub fn premium_reserve(&self) -> Layered<PremiumReservePercent> {
        if let Some(value) = self.project.and_then(|p| p.routing().premium_reserve()) {
            return Layered::new(value, Layer::Project);
        }
        if let Some(value) = self.user.routing().premium_reserve() {
            return Layered::new(value, Layer::User);
        }
        Layered::new(PremiumReservePercent::DEFAULT, Layer::Default)
    }

    /// Capacity-band thresholds, resolved per field — capability map line
    /// 1270. [`crate::provider::quota::CapacityBandThresholds::DEFAULT`] when
    /// neither layer recorded any, converted through
    /// [`CapacityBandThresholdsConfig::to_domain`] rather than re-validated
    /// here: it was already validated once, at deserialization.
    pub fn capacity_band_thresholds(
        &self,
    ) -> Layered<crate::provider::quota::CapacityBandThresholds> {
        if let Some(value) = self
            .project
            .and_then(|p| p.routing().capacity_band_thresholds())
        {
            return Layered::new(value.to_domain(), Layer::Project);
        }
        if let Some(value) = self.user.routing().capacity_band_thresholds() {
            return Layered::new(value.to_domain(), Layer::User);
        }
        Layered::new(
            crate::provider::quota::CapacityBandThresholds::DEFAULT,
            Layer::Default,
        )
    }

    /// Routing score weights, resolved per field — capability map lines
    /// 1357/1358. [`crate::routing::session::ScoreWeights::default`] when
    /// neither layer recorded any — today's compile-time constants,
    /// unchanged — converted through [`ScoreWeightsConfig::to_domain`] rather
    /// than re-validated here: it was already validated once, at
    /// deserialization.
    pub fn score_weights(&self) -> Layered<crate::routing::session::ScoreWeights> {
        if let Some(value) = self.project.and_then(|p| p.routing().score_weights()) {
            return Layered::new(value.to_domain(), Layer::Project);
        }
        if let Some(value) = self.user.routing().score_weights() {
            return Layered::new(value.to_domain(), Layer::User);
        }
        Layered::new(
            crate::routing::session::ScoreWeights::default(),
            Layer::Default,
        )
    }

    /// One resource's own protected reserve percentage — capability map line
    /// 1288 — or the global [`EffectiveConfig::premium_reserve`] preference
    /// when the provider has not stated one of its own.
    pub fn reserve_percent(&self, name: &str) -> Layered<PremiumReservePercent> {
        let configured = self.quota_override(name);
        match configured.value.reserve_percent() {
            Some(value) => Layered::new(value, configured.layer),
            None => self.premium_reserve(),
        }
    }

    /// The reserve policy for `scope` — capability map line 1577 — resolved
    /// per field, project over user over
    /// [`crate::routing::pressure::ReservePolicy::Protect`], the fail-closed
    /// default for a spending protection. Per field, so a project that
    /// records only the background policy inherits the user's interactive
    /// one rather than resetting it.
    pub fn reserve_policy(
        &self,
        scope: crate::routing::pressure::ReserveScope,
    ) -> Layered<crate::routing::pressure::ReservePolicy> {
        if let Some(value) = self
            .project
            .and_then(|p| p.routing().reserve())
            .and_then(|reserve| reserve.for_scope(scope))
        {
            return Layered::new(value, Layer::Project);
        }
        if let Some(value) = self
            .user
            .routing()
            .reserve()
            .and_then(|reserve| reserve.for_scope(scope))
        {
            return Layered::new(value, Layer::User);
        }
        Layered::new(
            crate::routing::pressure::ReservePolicy::default(),
            Layer::Default,
        )
    }

    /// Both scopes' policies at once, for a router that carries them whole.
    pub fn reserve_policies(&self) -> crate::routing::pressure::ReservePolicies {
        crate::routing::pressure::ReservePolicies {
            interactive: self
                .reserve_policy(crate::routing::pressure::ReserveScope::Interactive)
                .value,
            background: self
                .reserve_policy(crate::routing::pressure::ReserveScope::Background)
                .value,
        }
    }

    /// Every entitlement this configuration describes — Phase 56 line 1946
    /// — with the rules of each already resolved (line 1947).
    ///
    /// Two sources, in this order:
    ///
    /// 1. **The configured entries**, by name, project over user — a project's
    ///    `[entitlements.<name>]` replaces the user's entry of that name
    ///    whole, exactly as [`EffectiveConfig::configured_provider`] reads a
    ///    provider. Not per field: an allow-list that merged across layers
    ///    would have no readable answer to "does the project's list replace or
    ///    extend mine".
    /// 2. **A default entry for every harness's own sign-in** that no
    ///    configured entry claims through `native_harness` — named by the
    ///    harness's slug, with no `kind` (Glasshouse does not know which plan a
    ///    person signed a harness in with, and *unknown is an answer*) and
    ///    [`crate::routing::EntitlementRules::UNRESTRICTED`]. This is what
    ///    keeps a user who configured nothing exactly where they were: every
    ///    native launch has an entitlement to announce, and none has a rule.
    ///
    /// Refused rather than resolved by guessing when the two layers together
    /// contradict — see [`EntitlementLookupError`].
    pub fn entitlements(&self) -> Result<Vec<ResolvedEntitlement>, EntitlementLookupError> {
        let mut names: BTreeSet<&str> = self.user.entitlements().names().collect();
        if let Some(project) = self.project {
            names.extend(project.entitlements().names());
        }
        let mut resolved = Vec::with_capacity(names.len());
        for name in names {
            let (config, layer) = match self.project.and_then(|p| p.entitlements().get(name)) {
                Some(config) => (config, Layer::Project),
                None => (
                    self.user
                        .entitlements()
                        .get(name)
                        .expect("a name collected from the user table is in the user table"),
                    Layer::User,
                ),
            };
            resolved.push(config.to_resolved(name, layer)?);
        }

        // Map line 1973: one credential is one account. Two entries naming
        // the same reference would be two names drawing on one account —
        // exactly the mixing the line forbids — so the contradiction is
        // refused by name, like every other one here. The comparison is over
        // references (names), never values: nothing is resolved.
        for (index, entry) in resolved.iter().enumerate() {
            let Some(reference) = entry.credential() else {
                continue;
            };
            let sharers: Vec<String> = resolved[index..]
                .iter()
                .filter(|other| other.credential() == Some(reference))
                .map(|other| other.name.clone())
                .collect();
            if sharers.len() > 1 {
                return Err(EntitlementLookupError::SharedCredential {
                    names: sharers,
                    reference: match reference {
                        SecretRef::Environment { var } => format!("environment variable `{var}`"),
                        SecretRef::OsCredential { service, account } => {
                            format!("OS credential `{service}`/`{account}`")
                        }
                    },
                });
            }
        }

        for harness in IntegrationId::ALL
            .iter()
            .copied()
            .filter(|id| id.kind() == crate::integrations::IntegrationKind::Harness)
        {
            let claimed = resolved
                .iter()
                .any(|entry| entry.backing == EntitlementBacking::NativeHarness(harness));
            if claimed {
                continue;
            }
            if let Some(taken) = resolved.iter().find(|entry| entry.name == harness.slug()) {
                return Err(EntitlementLookupError::NameReservedForHarness {
                    name: taken.name.clone(),
                    harness,
                });
            }
            resolved.push(ResolvedEntitlement {
                name: harness.slug().to_owned(),
                kind: None,
                vendor: None,
                credential: None,
                backing: EntitlementBacking::NativeHarness(harness),
                rules: crate::routing::EntitlementRules::UNRESTRICTED,
                layer: Layer::Default,
                remaining_capacity: None,
                seconds_until_reset: None,
                capacity_scope: None,
                throttling: None,
                models: None,
                spend: None,
                headroom_estimate: None,
                headroom_override: None,
                disable_headroom_estimate: false,
            });
        }
        Ok(resolved)
    }

    /// Every entitlement the *user or project actually wrote* — the resolved
    /// list without the per-harness defaults [`Self::entitlements`]
    /// synthesises. The defaults exist so an unconfigured launch has an
    /// entitlement to announce; a listing of the user's configured accounts
    /// that included eight synthetic entries would bury the two real ones.
    pub fn configured_entitlements(
        &self,
    ) -> Result<Vec<ResolvedEntitlement>, EntitlementLookupError> {
        Ok(self
            .entitlements()?
            .into_iter()
            .filter(|entry| entry.layer() != Layer::Default)
            .collect())
    }

    /// Map line 1965's resolver: the configured pool with every telemetry
    /// facet populated from what `telemetry` actually holds — see
    /// [`ResolvedEntitlement::with_telemetry`] for what each facet reads and
    /// the scope every reading carries. One resolver for all entries, so two
    /// entitlements of one provider cannot be handed different provider-wide
    /// readings.
    pub fn configured_entitlements_with_telemetry(
        &self,
        telemetry: &EntitlementTelemetry<'_>,
    ) -> Result<Vec<ResolvedEntitlement>, EntitlementLookupError> {
        Ok(self
            .configured_entitlements()?
            .into_iter()
            .map(|entry| entry.with_telemetry(telemetry))
            .collect())
    }

    /// Every configured entitlement as a resource the registry can name —
    /// map line 1963's *"several entitlements of the same vendor and plan
    /// coexist in one pool"*, as the enumeration `glasshouse status` prints.
    ///
    /// One [`crate::provider::registry::ResourceKind::Entitlement`] per
    /// configured **entry, keyed by name and by nothing else** — never
    /// deduplicated by vendor, kind or backing, because two accounts of one
    /// vendor being two resources is the entire point of the pool.
    pub fn entitlement_resources(
        &self,
    ) -> Result<Vec<crate::provider::registry::ResourceKind>, EntitlementLookupError> {
        Ok(self
            .configured_entitlements()?
            .into_iter()
            .map(|entry| crate::provider::registry::ResourceKind::Entitlement { name: entry.name })
            .collect())
    }

    /// The entitlement a session on `harness` over `backend` would be charged
    /// to — line 1954's *which subscription*, resolved once here for the
    /// router (`main.rs::routing_destinations` attaches it to every
    /// destination) and for the announcement, so the two cannot disagree.
    ///
    /// `Ok(None)` is a real answer and means *no entitlement describes this
    /// resource*: a direct provider no entry names, or the Glasshouse gateway,
    /// whose upstream is assigned when the session starts. No rule can refuse
    /// a resource no rule describes, and the announcement says so rather than
    /// naming one. A harness's own sign-in is never `None`, because
    /// [`Self::entitlements`] supplies its default.
    pub fn entitlement_for(
        &self,
        harness: IntegrationId,
        backend: &crate::profile::BackendResource,
    ) -> Result<Option<ResolvedEntitlement>, EntitlementLookupError> {
        use crate::profile::BackendResource;

        let wanted = match backend {
            BackendResource::Native => EntitlementBacking::NativeHarness(harness),
            BackendResource::DirectProvider { provider } => {
                EntitlementBacking::Provider(provider.clone())
            }
            BackendResource::GlasshouseGateway => return Ok(None),
        };
        let mut matching: Vec<ResolvedEntitlement> = self
            .entitlements()?
            .into_iter()
            .filter(|entry| entry.backing == wanted)
            .collect();
        match matching.len() {
            0 => Ok(None),
            1 => Ok(matching.pop()),
            _ => {
                let names = matching.into_iter().map(|entry| entry.name).collect();
                Err(match wanted {
                    EntitlementBacking::NativeHarness(harness) => {
                        EntitlementLookupError::AmbiguousNativeHarness { harness, names }
                    }
                    EntitlementBacking::Provider(provider) => {
                        EntitlementLookupError::AmbiguousProvider { provider, names }
                    }
                    EntitlementBacking::Unstated => {
                        unreachable!("`wanted` is built from a backend and is never Unstated")
                    }
                })
            }
        }
    }

    /// Map line 1973's isolation, on the path that actually leaks: the child
    /// process inherits this whole process's environment, so every *other*
    /// entitlement's environment-variable credential would ride along into a
    /// session charged to one account — two accounts' keys mixed in one
    /// child, which is exactly what the line forbids. This names the
    /// variables to remove from a launch: the environment-shaped credential
    /// reference of every entitlement that is not the one serving this
    /// session. The serving entitlement's own variable stays; an
    /// OS-credential reference has no variable to leak; and a user with no
    /// `[entitlements]` entries gets an empty list, so nothing about an
    /// unconfigured launch changes.
    ///
    /// Called with the launch's resolved entitlement name, or `None` for a
    /// session no entitlement describes — which scrubs every entitlement's
    /// variable, because a session charged to no account has no business
    /// carrying any account's key. Tables that do not resolve scrub nothing:
    /// the launch path has already refused or reported that error before any
    /// process exists.
    pub fn foreign_entitlement_credential_vars(&self, serving: Option<&str>) -> Vec<String> {
        let Ok(entitlements) = self.entitlements() else {
            return Vec::new();
        };
        entitlements
            .iter()
            .filter(|entry| serving != Some(entry.name()))
            .filter_map(|entry| match entry.credential() {
                Some(SecretRef::Environment { var }) => Some(var.clone()),
                _ => None,
            })
            .collect()
    }

    /// The credential **references** of every entitlement that is not the
    /// one serving — [`Self::foreign_entitlement_credential_vars`]'s twin
    /// for the resolution side (56A line 1969): a launch bound to the
    /// pool's chosen account must not *resolve* a sibling account's
    /// credential either, or the overlay's "first reference that resolves"
    /// rule would quietly re-bind the process to whichever account is
    /// listed first. Same filter, same `None`-scrubs-everything contract,
    /// and both reference shapes rather than only the environment one — an
    /// OS-credential reference has no variable to scrub from a child, but
    /// it can absolutely be resolved, so the resolution side must know it.
    pub fn foreign_entitlement_credential_refs(&self, serving: Option<&str>) -> Vec<SecretRef> {
        let Ok(entitlements) = self.entitlements() else {
            return Vec::new();
        };
        entitlements
            .iter()
            .filter(|entry| serving != Some(entry.name()))
            .filter_map(|entry| entry.credential().cloned())
            .collect()
    }

    /// The entitlement charged for work sent to `provider` — map line 1947's
    /// job-kind clause, for the disposable router: a bounded support job has
    /// no harness and no launch profile, only the provider its candidate
    /// names, so this is [`Self::entitlement_for`]'s provider arm with no
    /// harness in the question. `Ok(None)` means no entry names the
    /// provider, and no rule can refuse a resource no rule describes.
    pub fn entitlement_for_provider(
        &self,
        provider: &str,
    ) -> Result<Option<ResolvedEntitlement>, EntitlementLookupError> {
        self.entitlement_for(
            // Any harness id serves: the provider arm below never reads it.
            IntegrationId::ClaudeCode,
            &crate::profile::BackendResource::DirectProvider {
                provider: provider.to_owned(),
            },
        )
    }

    /// Whether `model` on `provider` costs the user anything at the margin —
    /// [`ProviderConfig::cost_of`], read from the layer that configures the
    /// provider (project over user), and
    /// [`crate::routing::Cost::Metered`] when neither does: a provider nobody
    /// configured is not one anybody marked free.
    pub fn model_cost(&self, provider: &str, model: &str) -> Layered<crate::routing::Cost> {
        if let Some(config) = self.project.and_then(|p| p.providers().get(provider)) {
            return Layered::new(config.cost_of(model), Layer::Project);
        }
        if let Some(config) = self.user.providers().get(provider) {
            return Layered::new(config.cost_of(model), Layer::User);
        }
        Layered::new(crate::routing::Cost::Metered, Layer::Default)
    }

    /// `model`'s declared resource facts on `provider` — map line 1517's
    /// producer, read from the layer that configures the provider (project
    /// over user), exactly as [`EffectiveConfig::model_cost`] and
    /// [`EffectiveConfig::model_ceiling`] read beside it.
    ///
    /// [`crate::routing::capability::ResourceFacts::UNVERIFIED`] when
    /// neither layer configures the provider, or when the configuring layer
    /// declares no facts for this model — both are *not established*, the
    /// same "nobody has said" reading [`EffectiveConfig::model_ceiling`]'s
    /// own doc gives a `None` ceiling.
    pub fn model_facts(
        &self,
        provider: &str,
        model: &str,
    ) -> Layered<crate::routing::capability::ResourceFacts> {
        if let Some(config) = self.project.and_then(|p| p.providers().get(provider)) {
            return Layered::new(
                config.resource_facts_of(model, Layer::Project),
                Layer::Project,
            );
        }
        if let Some(config) = self.user.providers().get(provider) {
            return Layered::new(config.resource_facts_of(model, Layer::User), Layer::User);
        }
        Layered::new(
            crate::routing::capability::ResourceFacts::UNVERIFIED,
            Layer::Default,
        )
    }

    /// The highest workload tier `model` on `provider` is established to
    /// serve — map line 1796, read from the layer that configures the
    /// provider (project over user), exactly as
    /// [`EffectiveConfig::model_cost`] reads the cost beside it.
    ///
    /// `None` when the configuring layer states no ceiling for that model,
    /// and `None` when no layer configures the provider at all. Both are
    /// *not established*, and the tier gate does nothing to a destination
    /// carrying one — a provider nobody configured is not a provider anybody
    /// capped. The layer is still reported, so a reader can tell "the project
    /// layer states no ceiling for this model" from "nothing configures this
    /// provider"; the value is the same either way.
    ///
    /// Reads through [`ProviderConfig::resolved_ceiling`] rather than
    /// [`ProviderConfig::ceiling_of`] directly — Phase 34F widens this same
    /// call, the one `main.rs::destination_tier_ceiling` makes for every
    /// destination the shipped binary builds, to also honour a
    /// capability-record ceiling once no override states one.
    /// [`capability::CeilingResolution::hard_ceiling`] is what keeps a
    /// benchmark-provenance record out of this value: only the user's own
    /// word, override or capability record, may narrow what a destination is
    /// established to serve.
    pub fn model_ceiling(
        &self,
        provider: &str,
        model: &str,
    ) -> Layered<Option<crate::routing::classify::WorkloadTier>> {
        if let Some(config) = self.project.and_then(|p| p.providers().get(provider)) {
            return Layered::new(
                config.resolved_ceiling(model).hard_ceiling(),
                Layer::Project,
            );
        }
        if let Some(config) = self.user.providers().get(provider) {
            return Layered::new(config.resolved_ceiling(model).hard_ceiling(), Layer::User);
        }
        Layered::new(None, Layer::Default)
    }

    /// [`Self::model_ceiling`], through [`ProviderConfig::resolved_ceiling_for`]
    /// rather than [`ProviderConfig::resolved_ceiling`] — capability map line
    /// 1482's closing half, for a caller that has harness, launch-profile, or
    /// protocol context to narrow a capability record by. Layer precedence
    /// and every other rule match [`Self::model_ceiling`] exactly; only the
    /// record filter differs.
    pub fn model_ceiling_for(
        &self,
        provider: &str,
        model: &str,
        query: &capability::CapabilityQuery<'_>,
    ) -> Layered<Option<crate::routing::classify::WorkloadTier>> {
        if let Some(config) = self.project.and_then(|p| p.providers().get(provider)) {
            return Layered::new(
                config.resolved_ceiling_for(model, query).hard_ceiling(),
                Layer::Project,
            );
        }
        if let Some(config) = self.user.providers().get(provider) {
            return Layered::new(
                config.resolved_ceiling_for(model, query).hard_ceiling(),
                Layer::User,
            );
        }
        Layered::new(None, Layer::Default)
    }

    /// Every `(provider, model)` pair a calibrated [`capability::ModelCapabilityRecord`]
    /// actually decides the ceiling for, from the layer that configures that
    /// provider — project over user, matching every other lookup on this
    /// type. Map line 1481's own enumeration: the calibrated data a
    /// suggestion compares observed outcomes against.
    ///
    /// A pair whose resolution is [`capability::CeilingResolution::UserOverride`]
    /// is left out: [`ProviderConfig::model_ceilings`]'s own override
    /// is what actually governs that destination, so a capability record
    /// sitting unused beside it is not this line's to suggest changes to.
    /// Context-blind on purpose — a project-wide report has no one
    /// destination's harness or launch profile to narrow by, the same
    /// "genuinely no context" case [`ProviderConfig::resolved_ceiling`]
    /// documents for its own caller.
    pub fn calibrated_model_ceilings(
        &self,
    ) -> Vec<(String, String, capability::CeilingResolution)> {
        let mut out = Vec::new();
        for name in self.provider_names() {
            let config = self
                .project
                .and_then(|p| p.providers().get(&name))
                .or_else(|| self.user.providers().get(&name));
            let Some(config) = config else { continue };
            for model in config.model_capabilities().keys() {
                let resolution = config.resolved_ceiling(model);
                if matches!(
                    resolution,
                    capability::CeilingResolution::UserCapabilityRecord(_)
                        | capability::CeilingResolution::Prior(_)
                ) {
                    out.push((name.clone(), model.clone(), resolution));
                }
            }
        }
        out
    }

    /// The user's preferred order over free resources, resolved per field —
    /// Phase 9I line 536.
    pub fn free_resource_order(&self) -> Layered<Vec<FreeResourceRef>> {
        if let Some(value) = self.project.and_then(|p| p.routing().free_resource_order()) {
            return Layered::new(value.to_vec(), Layer::Project);
        }
        if let Some(value) = self.user.routing().free_resource_order() {
            return Layered::new(value.to_vec(), Layer::User);
        }
        Layered::new(Vec::new(), Layer::Default)
    }

    /// Free resources the user has disabled, resolved per field.
    pub fn free_resource_disabled(&self) -> Layered<Vec<FreeResourceRef>> {
        if let Some(value) = self
            .project
            .and_then(|p| p.routing().free_resource_disabled())
        {
            return Layered::new(value.to_vec(), Layer::Project);
        }
        if let Some(value) = self.user.routing().free_resource_disabled() {
            return Layered::new(value.to_vec(), Layer::User);
        }
        Layered::new(Vec::new(), Layer::Default)
    }

    /// The user's pinned free resource, resolved per field.
    pub fn free_resource_pin(&self) -> Layered<Option<FreeResourceRef>> {
        if let Some(value) = self.project.and_then(|p| p.routing().free_resource_pin()) {
            return Layered::new(Some(value.clone()), Layer::Project);
        }
        if let Some(value) = self.user.routing().free_resource_pin() {
            return Layered::new(Some(value.clone()), Layer::User);
        }
        Layered::new(None, Layer::Default)
    }

    /// What will actually classify a request: the recorded choice from
    /// [`EffectiveConfig::routing_model`], checked against the providers
    /// that are configured right now.
    ///
    /// This never fails. A pinned model whose provider has since been
    /// removed degrades to [`RoutingModelResolution::Heuristics`] carrying a
    /// [`RoutingFallback`] that says which one went missing — see
    /// [`RoutingModelChoice::resolve`] for why this is the one lookup here
    /// that will not return an error. The [`Layer`] reported is the layer the
    /// *choice* came from, not a claim about where the degrade was decided.
    ///
    /// A choice nothing was ever recorded for reports
    /// [`RoutingFallback::NotConfigured`] rather than
    /// [`RoutingFallback::DeterministicChosen`], so a user who declined the
    /// wizard's routing step and a user who deliberately picked
    /// deterministic-only are told different, accurate things.
    pub fn routing_model_resolution(&self) -> Layered<RoutingModelResolution> {
        let Layered { value, layer } = self.routing_model();
        let mut resolution = value.resolve(&self.provider_names());
        if layer == Layer::Default
            && let RoutingModelResolution::Heuristics(reason) = &mut resolution
        {
            *reason = RoutingFallback::NotConfigured;
        }
        Layered::new(resolution, layer)
    }

    /// Each layer's `[pairing]` table, in the order corrections are applied
    /// — user first, project second, so a report reads in the order
    /// [`EffectiveConfig::pairing_overrides`] merges them.
    ///
    /// The [`EffectiveConfig`] fields are private and this type is `Copy`,
    /// so a caller outside this module cannot reach a layer's table any
    /// other way; a report that wants to show *which file* a correction came
    /// from needs exactly this.
    pub fn pairing_layers(&self) -> Vec<(Layer, &pairing::PairingConfig)> {
        let mut layers = vec![(Layer::User, self.user.pairing())];
        if let Some(project) = self.project {
            layers.push((Layer::Project, project.pairing()));
        }
        layers
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

        let mut provider = config.to_provider(name)?;
        config.declare_tool_calls(&mut provider, layer);
        Ok(Layered::new(provider, layer))
    }

    /// What the user configured about `name`'s quota, reporting which layer
    /// supplied it — capability map lines 1233, 1203 and 1237.
    ///
    /// # Whole-table precedence, deliberately
    ///
    /// The project layer's `[providers.<name>.quota]` table replaces the
    /// user layer's rather than merging field by field, matching
    /// [`ProviderConfig::credential_env`]'s and
    /// [`ProviderConfig::headers`]'s own replace-not-merge rule and the
    /// [`EffectiveConfig::configured_provider`] lookup this sits beside — a
    /// project that states a quota table has stated the whole of what it
    /// believes about that provider's quota. A per-field merge would let a
    /// project's `plan` silently inherit a user's `budget`, which is a
    /// spending ceiling arriving somewhere nobody wrote it.
    ///
    /// [`Layer::Default`] with an empty override when neither layer says
    /// anything, which is not the same as the provider being unknown: this
    /// answers a question about configuration and an unconfigured provider
    /// has, correctly, no overrides.
    pub fn quota_override(&self, name: &str) -> Layered<QuotaOverride> {
        if let Some(quota) = self
            .project
            .and_then(|p| p.providers().get(name))
            .and_then(ProviderConfig::quota)
        {
            return Layered::new(quota.clone(), Layer::Project);
        }
        if let Some(quota) = self
            .user
            .providers()
            .get(name)
            .and_then(ProviderConfig::quota)
        {
            return Layered::new(quota.clone(), Layer::User);
        }
        Layered::new(QuotaOverride::default(), Layer::Default)
    }

    /// How long `name`'s quota telemetry stays current — capability map
    /// line 1237.
    ///
    /// Resolved through [`EffectiveConfig::quota_override`], so a project
    /// layer's age wins over a user layer's, and
    /// [`QuotaStaleAfterSeconds::DEFAULT`] when neither said. **The value is
    /// per provider**, which is the whole of the line's "provider-specific":
    /// asking this for two providers may legitimately give two answers, and a
    /// caller that read it once and reused it would have flattened exactly
    /// the distinction the line asks for.
    pub fn quota_stale_after(&self, name: &str) -> Layered<QuotaStaleAfterSeconds> {
        let configured = self.quota_override(name);
        match configured.value.stale_after() {
            Some(value) => Layered::new(value, configured.layer),
            None => Layered::new(QuotaStaleAfterSeconds::DEFAULT, Layer::Default),
        }
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

/// A launch profile a person named explicitly is disabled.
///
/// Deliberately **not** a [`ProfileLookupError`] variant.
/// [`EffectiveConfig::launch_profile`] never returns this and must not: it is
/// the resolver *every* path uses, and a session already running under a
/// profile since disabled has to stay resumable — `enabled` decides what may
/// be **started**, not what may be continued, which is the same reading
/// `resume`'s own profile fallback already took. So this is a separate
/// refusal raised at the one place a session is started from a name a person
/// typed.
///
/// The message carries the profile name and the two ways to undo it, and no
/// path: see [`Layer::describe_source`].
#[derive(Debug, thiserror::Error)]
#[error(
    "launch profile `{name}` is disabled {}; re-enable it in the Settings screen's \
     Launch Profiles section, or set `enabled = true` under `[profiles.{name}]`",
    .layer.describe_source()
)]
pub struct ProfileDisabled {
    pub name: String,
    pub layer: Layer,
}

impl ProfileDisabled {
    pub fn new(name: impl Into<String>, layer: Layer) -> Self {
        Self {
            name: name.into(),
            layer,
        }
    }
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
        config.set_memory_extraction(Some(false));
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
        assert_eq!(loaded.memory_extraction(), Some(false));
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

    /// Box 1800: cmux may be disabled even when it is detected. This module
    /// has no concept of "detected" at all — that lives in `integrations::`,
    /// which is exactly why an explicit decision here is immune to it: the
    /// same generic tri-state `enabled` this file gives every integration
    /// (see [`tri_state_enabled_distinguishes_never_asked_from_a_decision`])
    /// applies to [`IntegrationId::Cmux`] with no special case, and
    /// `onboarding::state::build_rows` reads this exact field to seed the
    /// wizard's cmux row regardless of what live detection found.
    #[test]
    fn cmux_can_be_explicitly_disabled_and_the_decision_is_ordinary_configuration() {
        let mut config = UserConfig::default();
        assert_eq!(
            config.integrations().is_enabled(IntegrationId::Cmux),
            None,
            "never asked, whether or not cmux is present on this machine"
        );

        config
            .integrations_mut()
            .entry(IntegrationId::Cmux)
            .set_enabled(false);

        // Nothing in configuration ever consults "is cmux detected" — the
        // decision persists exactly like any other integration's, and a
        // caller must never fall back to treating detection as an override.
        assert_eq!(
            config.integrations().is_enabled(IntegrationId::Cmux),
            Some(false)
        );
        assert!(
            !config
                .integrations()
                .is_enabled_or_default(IntegrationId::Cmux, true),
            "an explicit disable must win over any default, including one a detector would suggest"
        );

        let tmp = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));
        config.save(&paths).unwrap();
        let loaded = UserConfig::load(&paths).unwrap();
        assert_eq!(
            loaded.integrations().is_enabled(IntegrationId::Cmux),
            Some(false),
            "the disable survives a save and load, so a later run still honours it"
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
    /// Phase 9H line 518, the storage half: a pin recorded in configuration
    /// reaches the launch profile that applies it, and survives a save and a
    /// load.
    ///
    /// **This test exists because a mutation survived without it.** Replacing
    /// `to_launch_profile`'s `pin_gateway_backend` with a hard-coded `false`
    /// broke nothing: the profile-side test that proves a pin turns failover
    /// off builds its `LaunchProfile` by hand, so the one hop between stored
    /// configuration and the value `apply_gateway` reads was uncovered.
    #[test]
    fn a_pin_recorded_in_configuration_reaches_the_launch_profile_and_round_trips() {
        let mut stored = ProfileConfig::new(IntegrationId::ClaudeCode);
        stored.set_backend(ProfileBackend::GlasshouseGateway);
        assert!(
            !stored.pin_gateway_backend(),
            "a profile nobody pinned is not pinned"
        );
        stored.set_pin_gateway_backend(true);

        let profile = stored
            .to_launch_profile("pinned")
            .expect("a known harness and backend");
        assert!(
            profile.pin_gateway_backend,
            "the stored pin must reach the value `apply_gateway` reads"
        );

        // And a file written before the field existed loads as not pinned,
        // which is the behaviour those files already had.
        let toml = toml::to_string(&stored).expect("serializable");
        assert!(toml.contains("pin_gateway_backend"), "{toml}");
        let reloaded: ProfileConfig = toml::from_str(&toml).expect("round-trips");
        assert!(reloaded.pin_gateway_backend());

        let legacy: ProfileConfig =
            toml::from_str("harness = \"claude-code\"").expect("a file without the field loads");
        assert!(!legacy.pin_gateway_backend());
        let legacy_toml = toml::to_string(&legacy).expect("serializable");
        assert!(
            !legacy_toml.contains("pin_gateway_backend"),
            "an unpinned profile writes exactly what it wrote before: {legacy_toml}"
        );
    }

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

        // `RoutingModelChoice::Pinned` — the newest stored shape, and the
        // only variant of it that carries a payload. Both halves are NAMES:
        // `provider` is a key into `ProviderTable` and `model` is a model
        // name, exactly like `ProfileConfig`'s `backend`/`model` pair above.
        // Turning either into an actual credential stays `SecretStore`'s job.
        let pinned_routing = RoutingModelChoice::Pinned {
            provider: "openrouter".to_owned(),
            model: "gpt-5.6-luna".to_owned(),
        };
        let pinned_routing_value = toml::Value::try_from(&pinned_routing).unwrap();
        let pinned_routing_table = pinned_routing_value.as_table().unwrap();
        let mut pinned_routing_keys: Vec<&str> =
            pinned_routing_table.keys().map(String::as_str).collect();
        pinned_routing_keys.sort_unstable();
        assert_eq!(
            pinned_routing_keys,
            vec!["kind", "model", "provider"],
            "RoutingModelChoice::Pinned grew a field — confirm it cannot hold a credential \
             before widening this list"
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
            .set_enabled(false)
            .set_free_models(vec!["nvidia/nemotron-nano-9b-v2:free".to_owned()])
            // A model NAME and a workload-tier spelling. Line 1796's field is
            // in this guard for the same reason `free_models` is: it is keyed
            // by the same identifier, and neither half can hold a value.
            .set_model_ceilings(BTreeMap::from([(
                "nvidia/nemotron-nano-9b-v2:free".to_owned(),
                ConfiguredWorkloadTier::new(crate::routing::classify::WorkloadTier::Leaf),
            )]));
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
                "free_models",
                "headers",
                "model_ceilings",
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
                "memory_extraction",
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

    /// [`an_os_credential_reference_round_trips_through_configuration_without_its_value`]'s
    /// sibling for the *project*-level file: box 1789 is specifically about
    /// what a project may write into its own tracked `.glasshouse/config.toml`
    /// — a file real repositories check in — so the guarantee needs its own
    /// proof at [`write_project_config_with_consent`] rather than resting on
    /// the user-file test alone.
    ///
    /// "Wide", here, is comprehensiveness rather than a TUI viewport (§17's
    /// truncation risk does not apply to a TOML file: nothing elides it) — a
    /// project config populated across every component table this module
    /// exposes (providers with headers and a credential store, profiles,
    /// pairing corrections, a response profile, routing), so a leak in any
    /// one of them would show up here rather than only in a narrow fixture.
    #[test]
    fn project_config_file_never_contains_a_planted_secret_value_across_every_table() {
        const VAR: &str = "GLASSHOUSE_PROJECT_CONFIG_TEST_ONLY_SECRET_VAR";
        const VALUE: &str = "sk-project-config-test-only-0123456789abcdef";

        let workspace = tempfile::tempdir().unwrap();
        let project = test_project(workspace.path());

        let mut provider = ProviderConfig::new("openrouter");
        provider
            .set_base_url(Some("https://example.invalid".to_owned()))
            .set_credential_env(vec![VAR.to_owned()])
            .set_credential_store(Some(StoredCredentialRef::new("glasshouse", VAR)))
            .set_headers(vec![("X-Test".to_owned(), "1".to_owned())])
            .set_free_models(vec!["nvidia/nemotron-nano-9b-v2:free".to_owned()]);

        let mut profile = ProfileConfig::new(IntegrationId::ClaudeCode);
        profile.set_backend(ProfileBackend::DirectProvider {
            provider: "wide".to_owned(),
        });

        let mut config = ProjectConfig::default();
        config.providers_mut().set("wide", provider);
        config.profiles_mut().set("wide", profile);
        config
            .integrations_mut()
            .entry(IntegrationId::Codex)
            .set_executable(Some(PathBuf::from("/opt/codex/bin/codex")));
        config
            .pairing_mut()
            .model_entry("gpt-5.6-luna")
            .set_developer(Some("openai".to_owned()));
        config
            .response_mut()
            .default_entry_mut()
            .set_preset(Some("audit".to_owned()));
        config
            .routing_mut()
            .set_model(Some(RoutingModelChoice::Pinned {
                provider: "wide".to_owned(),
                model: "gpt-5.6-luna".to_owned(),
            }));

        // SAFETY: `VAR` is unique to this test and removed again below.
        // Planted so that a serializer which resolved the reference would
        // have something to leak.
        unsafe {
            std::env::set_var(VAR, VALUE);
        }
        let written = write_project_config_with_consent(&project, &config);
        unsafe {
            std::env::remove_var(VAR);
        }
        written.unwrap();

        let path = project.root().join(PROJECT_CONFIG_RELATIVE_PATH);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            !text.contains(VALUE),
            "a credential value reached the project configuration file:\n{text}"
        );
        assert!(text.contains(VAR), "the NAME must be there:\n{text}");

        // The same broad word-scan the user-file structural test runs,
        // applied to a project file that actually populates every table —
        // unlike that test's fixture, this one legitimately writes
        // `credential_env`/`credential_store` as *keys*, so the scan is
        // narrowed to lines that are not those two keys' own declarations.
        for line in text.lines() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("credential_env")
                || trimmed.starts_with("credential_store")
                || trimmed.starts_with("service =")
                || trimmed.starts_with("account =")
            {
                continue;
            }
            for forbidden in ["token", "secret", "password"] {
                assert!(
                    !line.to_lowercase().contains(forbidden),
                    "project configuration file unexpectedly contains `{forbidden}` outside a \
                     credential reference's own keys: {line}"
                );
            }
        }
    }

    /// Phase 2C's whole job is to *record* the choice, so the thing worth
    /// proving is that it survives the process that made it. Each of the
    /// three answers the wizard offers goes to disk through the real `save`
    /// and comes back through the real `load` — a `toml::to_string` in
    /// isolation would pass even if `UserConfig`'s `[routing]` table were
    /// never wired into the file that is actually written.
    #[test]
    fn every_routing_model_choice_survives_a_real_save_and_load() {
        fn round_trip(choice: Option<RoutingModelChoice>) -> UserConfig {
            let tmp = tempfile::tempdir().unwrap();
            let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));

            let mut user = fully_populated_user_config();
            user.routing_mut().set_model(choice);
            user.save(&paths).unwrap();

            let loaded = UserConfig::load(&paths).unwrap();
            assert_eq!(
                loaded, user,
                "recording a routing model must not disturb anything else in the file"
            );
            loaded
        }

        assert_eq!(
            round_trip(Some(RoutingModelChoice::Automatic))
                .routing()
                .model(),
            Some(&RoutingModelChoice::Automatic)
        );

        let pinned = RoutingModelChoice::Pinned {
            provider: "openrouter".to_owned(),
            model: "gpt-5.6-luna".to_owned(),
        };
        assert_eq!(
            round_trip(Some(pinned.clone())).routing().model(),
            Some(&pinned)
        );

        assert_eq!(
            round_trip(Some(RoutingModelChoice::Deterministic))
                .routing()
                .model(),
            Some(&RoutingModelChoice::Deterministic)
        );

        // "Do later" must read back as *nothing recorded* rather than as an
        // explicit deterministic choice: the two resolve the same way but
        // say different, accurate things about what the user decided.
        let declined = round_trip(None);
        assert_eq!(declined.routing().model(), None);
        assert_eq!(
            EffectiveConfig::new(&declined, None)
                .routing_model_resolution()
                .value,
            RoutingModelResolution::Heuristics(RoutingFallback::NotConfigured)
        );
    }

    /// Phase 2C line 4: declining the routing step leaves no routing model
    /// configured *and* the system keeps working. Both halves are asserted,
    /// and the second is the one that matters — "nothing crashed" is not the
    /// contract, "deterministic heuristics are what answer" is.
    #[test]
    fn declining_the_routing_step_writes_no_routing_table_and_still_resolves() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));

        let mut user = fully_populated_user_config();
        user.routing_mut().set_model(None);
        user.save(&paths).unwrap();

        let text = std::fs::read_to_string(paths.user_config_file()).unwrap();
        assert!(
            !text.contains("routing"),
            "\"Do later\" must leave no `[routing]` table at all, not an empty one:\n{text}"
        );

        let loaded = UserConfig::load(&paths).unwrap();
        let effective = EffectiveConfig::new(&loaded, None);
        let resolution = effective.routing_model_resolution();
        assert_eq!(
            resolution.value,
            RoutingModelResolution::Heuristics(RoutingFallback::NotConfigured),
            "deterministic heuristics must be what answers, and must say they are \
             answering because nothing was ever configured"
        );
        assert_eq!(resolution.layer, Layer::Default);
        assert_eq!(
            effective.routing_model().value,
            RoutingModelChoice::Deterministic
        );
    }

    /// Phase 2C's behavioural contract: a configuration naming a routing
    /// model whose provider has disappeared must degrade "and say so". It is
    /// the one lookup in this module that refuses to return an error — a
    /// routing model is an optimisation over a system that already works
    /// without it, so a rotated key must not stop Glasshouse from starting.
    #[test]
    fn a_pinned_routing_model_whose_provider_is_gone_degrades_and_names_it() {
        let mut user = UserConfig::default();
        user.providers_mut()
            .set("openrouter", ProviderConfig::new("openrouter"));
        user.routing_mut()
            .set_model(Some(RoutingModelChoice::Pinned {
                provider: "retired-mirror".to_owned(),
                model: "gpt-5.6-luna".to_owned(),
            }));

        let effective = EffectiveConfig::new(&user, None);
        let resolution = effective.routing_model_resolution();
        assert_eq!(
            resolution.value,
            RoutingModelResolution::Heuristics(RoutingFallback::ProviderNotConfigured {
                provider: "retired-mirror".to_owned(),
                model: "gpt-5.6-luna".to_owned(),
            })
        );
        assert_eq!(
            resolution.layer,
            Layer::User,
            "the layer reported is where the CHOICE came from, not a claim about \
             where the degrade was decided"
        );

        // The degrade has to be *sayable*, and saying "your routing model is
        // unavailable" without naming which one is not saying it.
        let said = resolution.value.fallback().unwrap().to_string();
        assert!(said.contains("`retired-mirror`"), "{said}");
        assert!(said.contains("`gpt-5.6-luna`"), "{said}");
        assert!(said.contains("which is not configured"), "{said}");
        assert!(
            said.contains("deterministic routing heuristics"),
            "the message must say what is answering instead:\n{said}"
        );

        // The contrast that proves the degrade is a lookup and not a
        // blanket refusal: the same shape pinned to a provider that *is*
        // configured resolves to that model.
        user.routing_mut()
            .set_model(Some(RoutingModelChoice::Pinned {
                provider: "openrouter".to_owned(),
                model: "gpt-5.6-luna".to_owned(),
            }));
        assert_eq!(
            EffectiveConfig::new(&user, None)
                .routing_model_resolution()
                .value,
            RoutingModelResolution::Pinned {
                provider: "openrouter".to_owned(),
                model: "gpt-5.6-luna".to_owned(),
            }
        );
    }

    /// A routing-model choice grants nothing and attests to nothing, so it
    /// layers by the ordinary rule rather than following
    /// `bypass_acknowledged`'s user-layer-only exception. The first case is
    /// the reason the stored field is an `Option` and not a plain enum: a
    /// project saying "deterministic, on purpose" has to be able to override
    /// a user-level `automatic`, which a collapsed shape could not express.
    #[test]
    fn a_routing_choice_layers_project_over_user_and_reports_the_deciding_layer() {
        let mut user = UserConfig::default();
        user.routing_mut()
            .set_model(Some(RoutingModelChoice::Automatic));

        let mut project = ProjectConfig::default();
        project
            .routing_mut()
            .set_model(Some(RoutingModelChoice::Deterministic));

        // Case 1: the project's explicit deterministic-only beats the user's
        // automatic, and the reason given is "chosen", not "never set".
        let effective = EffectiveConfig::new(&user, Some(&project));
        let chosen = effective.routing_model();
        assert_eq!(chosen.value, RoutingModelChoice::Deterministic);
        assert_eq!(chosen.layer, Layer::Project);
        let resolution = effective.routing_model_resolution();
        assert_eq!(
            resolution.value,
            RoutingModelResolution::Heuristics(RoutingFallback::DeterministicChosen)
        );
        assert_eq!(resolution.layer, Layer::Project);

        // Case 2: a project that has recorded nothing falls through to the
        // user layer rather than shadowing it with a default.
        let silent = ProjectConfig::default();
        let effective = EffectiveConfig::new(&user, Some(&silent));
        let chosen = effective.routing_model();
        assert_eq!(chosen.value, RoutingModelChoice::Automatic);
        assert_eq!(chosen.layer, Layer::User);
        let resolution = effective.routing_model_resolution();
        assert_eq!(resolution.value, RoutingModelResolution::Automatic);
        assert_eq!(resolution.layer, Layer::User);

        // Case 3: neither layer decided, so the default answers — and says
        // so with `NotConfigured`, not `DeterministicChosen`.
        let undecided = UserConfig::default();
        let effective = EffectiveConfig::new(&undecided, Some(&silent));
        let chosen = effective.routing_model();
        assert_eq!(chosen.value, RoutingModelChoice::Deterministic);
        assert_eq!(chosen.layer, Layer::Default);
        let resolution = effective.routing_model_resolution();
        assert_eq!(
            resolution.value,
            RoutingModelResolution::Heuristics(RoutingFallback::NotConfigured)
        );
        assert_eq!(resolution.layer, Layer::Default);
    }

    #[test]
    fn memory_extraction_enabled_layers_project_over_user_over_default() {
        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, None);
        assert_eq!(
            effective.memory_extraction_enabled(),
            Layered::new(true, Layer::Default),
            "nothing recorded anywhere must resolve to enabled"
        );

        let mut user = UserConfig::default();
        user.set_memory_extraction(Some(false));
        let effective = EffectiveConfig::new(&user, None);
        assert_eq!(
            effective.memory_extraction_enabled(),
            Layered::new(false, Layer::User)
        );

        let mut project = ProjectConfig::default();
        project.set_memory_extraction(Some(true));
        let effective = EffectiveConfig::new(&user, Some(&project));
        assert_eq!(
            effective.memory_extraction_enabled(),
            Layered::new(true, Layer::Project),
            "a project's explicit re-enable must win over the user's disable"
        );

        let silent_project = ProjectConfig::default();
        let effective = EffectiveConfig::new(&user, Some(&silent_project));
        assert_eq!(
            effective.memory_extraction_enabled(),
            Layered::new(false, Layer::User),
            "a project that recorded nothing must fall through to the user layer"
        );
    }

    #[test]
    fn context_firewall_reducer_layers_project_over_user_and_defaults_to_none() {
        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, None);
        assert_eq!(
            effective.context_firewall_reducer(),
            Layered::new(None, Layer::Default),
            "nobody who never configured a reducer has one"
        );

        let mut user = UserConfig::default();
        user.context_firewall_mut()
            .set_reducer(Some("openrouter".to_owned()));
        let effective = EffectiveConfig::new(&user, None);
        assert_eq!(
            effective.context_firewall_reducer(),
            Layered::new(Some("openrouter".to_owned()), Layer::User)
        );

        let mut project = ProjectConfig::default();
        project
            .context_firewall_mut()
            .set_reducer(Some("a-project-entitlement".to_owned()));
        let effective = EffectiveConfig::new(&user, Some(&project));
        assert_eq!(
            effective.context_firewall_reducer(),
            Layered::new(Some("a-project-entitlement".to_owned()), Layer::Project),
            "a project's own reducer choice must win over the user's"
        );
    }

    #[test]
    fn context_firewall_min_semantic_tokens_defaults_and_layers() {
        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, None);
        assert_eq!(
            effective.context_firewall_min_semantic_tokens(),
            Layered::new(firewall::DEFAULT_MIN_SEMANTIC_TOKENS, Layer::Default)
        );

        let mut user = UserConfig::default();
        user.context_firewall_mut()
            .set_min_semantic_tokens(Some(500));
        let effective = EffectiveConfig::new(&user, None);
        assert_eq!(
            effective.context_firewall_min_semantic_tokens(),
            Layered::new(500, Layer::User)
        );
    }

    #[test]
    fn context_firewall_aggressive_drops_uncertain_defaults_to_false() {
        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, None);
        assert_eq!(
            effective.context_firewall_aggressive_drops_uncertain(),
            Layered::new(false, Layer::Default),
            "bias to inclusion is the default nobody had to ask for"
        );

        let mut user = UserConfig::default();
        user.context_firewall_mut()
            .set_aggressive_drops_uncertain(Some(true));
        let effective = EffectiveConfig::new(&user, None);
        assert_eq!(
            effective.context_firewall_aggressive_drops_uncertain(),
            Layered::new(true, Layer::User)
        );
    }

    #[test]
    fn context_firewall_reducer_local_only_defaults_to_false() {
        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, None);
        assert_eq!(
            effective.context_firewall_reducer_local_only(),
            Layered::new(false, Layer::Default)
        );

        let mut user = UserConfig::default();
        user.context_firewall_mut()
            .set_reducer_local_only(Some(true));
        let effective = EffectiveConfig::new(&user, None);
        assert_eq!(
            effective.context_firewall_reducer_local_only(),
            Layered::new(true, Layer::User)
        );
    }

    #[test]
    fn automatic_checkpoint_enabled_layers_project_over_user_over_default() {
        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, None);
        assert_eq!(
            effective.automatic_checkpoint_enabled(),
            Layered::new(true, Layer::Default),
            "nothing recorded anywhere must resolve to enabled"
        );

        let mut user = UserConfig::default();
        user.set_automatic_checkpoint(Some(false));
        let effective = EffectiveConfig::new(&user, None);
        assert_eq!(
            effective.automatic_checkpoint_enabled(),
            Layered::new(false, Layer::User)
        );

        let mut project = ProjectConfig::default();
        project.set_automatic_checkpoint(Some(true));
        let effective = EffectiveConfig::new(&user, Some(&project));
        assert_eq!(
            effective.automatic_checkpoint_enabled(),
            Layered::new(true, Layer::Project),
            "a project's explicit re-enable must win over the user's disable"
        );

        let silent_project = ProjectConfig::default();
        let effective = EffectiveConfig::new(&user, Some(&silent_project));
        assert_eq!(
            effective.automatic_checkpoint_enabled(),
            Layered::new(false, Layer::User),
            "a project that recorded nothing must fall through to the user layer"
        );
    }

    /// The independence half of the automatic-checkpoint switch:
    /// [`EffectiveConfig::automatic_checkpoint_enabled`] must depend only on
    /// its own field, never on [`UserConfig::memory_extraction`] or any other
    /// automatic behaviour, and vice versa.
    #[test]
    fn automatic_checkpoint_and_memory_extraction_disable_independently() {
        for (checkpoint_off, memory_off) in
            [(false, false), (true, false), (false, true), (true, true)]
        {
            let mut user = UserConfig::default();
            user.set_automatic_checkpoint(Some(!checkpoint_off));
            user.set_memory_extraction(Some(!memory_off));

            let effective = EffectiveConfig::new(&user, None);

            assert_eq!(
                effective.automatic_checkpoint_enabled().value,
                !checkpoint_off,
                "checkpoint state must depend only on its own field, case {checkpoint_off} {memory_off}"
            );
            assert_eq!(
                effective.memory_extraction_enabled().value,
                !memory_off,
                "memory-extraction state must depend only on its own field, case {checkpoint_off} {memory_off}"
            );
        }
    }

    /// A pin is two names — a key into `ProviderTable` and a model name —
    /// and never a credential, alongside
    /// [`serialized_form_has_no_secret_capable_field`]'s structural guard on
    /// the same shape. This is the behavioural half: a real key is planted
    /// in the environment the pinned provider's `credential_env` points at,
    /// so a serializer that resolved the pin to a usable credential — the
    /// failure this test exists to catch — would have something to leak.
    #[test]
    fn a_pinned_routing_model_persists_names_and_never_a_credential_value() {
        const VAR: &str = "GLASSHOUSE_CONFIG_TEST_ONLY_ROUTING_PIN_VAR";
        const VALUE: &str = "sk-or-v1-routingpin0123456789abcdef0123456789abcdef01234567";

        let tmp = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));

        let mut provider = ProviderConfig::new("openrouter");
        provider
            .set_credential_env(vec![VAR.to_owned()])
            .set_credential_store(Some(StoredCredentialRef::new("glasshouse", VAR)));

        let pinned = RoutingModelChoice::Pinned {
            provider: "openrouter".to_owned(),
            model: "gpt-5.6-luna".to_owned(),
        };
        let mut user = UserConfig::default();
        user.providers_mut().set("openrouter", provider);
        user.routing_mut().set_model(Some(pinned.clone()));

        // SAFETY: `VAR` is unique to this test and removed again below.
        unsafe {
            std::env::set_var(VAR, VALUE);
        }
        let saved = user.save(&paths);
        unsafe {
            std::env::remove_var(VAR);
        }
        saved.unwrap();

        let text = std::fs::read_to_string(paths.user_config_file()).unwrap();
        assert!(
            !text.contains(VALUE),
            "a credential value reached the configuration file:\n{text}"
        );
        assert!(
            !text.contains("sk-or-v1-"),
            "not even a prefix of a key belongs in a tracked configuration file:\n{text}"
        );

        // ... and the two names a pin is made of really are what got
        // written, so the assertion above is not passing on an empty file.
        assert!(text.contains("gpt-5.6-luna"), "{text}");
        assert!(text.contains("openrouter"), "{text}");
        assert!(text.contains("pinned"), "{text}");
        assert!(text.contains(VAR), "the NAME must be there:\n{text}");

        let loaded = UserConfig::load(&paths).unwrap();
        assert_eq!(loaded.routing().model(), Some(&pinned));
    }

    /// Every configuration file on disk today was written before this field
    /// existed, so the missing `[routing]` table is the ordinary case, not an
    /// edge one — the same treatment this module already gives unknown and
    /// missing keys. Written by hand rather than saved, because a config this
    /// build produced could never be missing a key this build knows about.
    #[test]
    fn a_configuration_written_before_routing_existed_loads_with_nothing_recorded() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        std::fs::write(
            paths.user_config_file(),
            r#"
                version = 1

                [onboarding]
                completed = true
                completed_at_version = "0.1.0"

                [integrations.claude-code]
                enabled = true

                [providers.openrouter]
                template = "openrouter"
                credential_env = ["OPENROUTER_API_KEY"]
            "#,
        )
        .unwrap();

        let loaded = UserConfig::load(&paths).unwrap();
        assert!(loaded.onboarding().completed());
        assert_eq!(
            loaded.routing().model(),
            None,
            "an older file must load as \"never decided\", not as some invented choice"
        );

        let effective = EffectiveConfig::new(&loaded, None);
        let resolution = effective.routing_model_resolution();
        assert_eq!(
            resolution.value,
            RoutingModelResolution::Heuristics(RoutingFallback::NotConfigured)
        );
        assert_eq!(resolution.layer, Layer::Default);
    }

    /// Phase 2D routing preferences are exact, bounded, independently
    /// layered values. A real save/load proves their serde wiring; mixed
    /// layers prove one project override does not copy its siblings; invalid
    /// TOML proves absurd scalar values cannot enter through hand editing.
    #[test]
    fn routing_policy_values_round_trip_layer_independently_and_reject_absurd_inputs() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));

        let latency_user = RouterLatencyMs::try_from(1_500).unwrap();
        let cost_user = RouterCostMicroUsd::try_from(2_500).unwrap();
        let reserve_user = PremiumReservePercent::try_from(15).unwrap();
        let mut user = UserConfig::default();
        user.routing_mut()
            .set_max_router_latency(Some(latency_user))
            .set_max_marginal_cost(Some(cost_user))
            .set_prefer_free(Some(false))
            .set_premium_reserve(Some(reserve_user));
        user.save(&paths).unwrap();
        let loaded = UserConfig::load(&paths).unwrap();
        assert_eq!(loaded.routing(), user.routing());

        let latency_project = RouterLatencyMs::try_from(350).unwrap();
        let mut project = ProjectConfig::default();
        project
            .routing_mut()
            .set_max_router_latency(Some(latency_project))
            .set_prefer_free(Some(true));
        let effective = EffectiveConfig::new(&loaded, Some(&project));
        assert_eq!(
            effective.max_router_latency(),
            Layered::new(latency_project, Layer::Project)
        );
        assert_eq!(
            effective.max_router_cost(),
            Layered::new(cost_user, Layer::User)
        );
        assert_eq!(
            effective.prefer_free_routing(),
            Layered::new(true, Layer::Project)
        );
        assert_eq!(
            effective.premium_reserve(),
            Layered::new(reserve_user, Layer::User)
        );

        for invalid in [
            "max_router_latency_ms = 0",
            "max_router_latency_ms = 60001",
            "max_marginal_cost_micro_usd = 1000001",
            "premium_reserve_percent = 101",
        ] {
            let text = format!("version = 1\n[routing]\n{invalid}\n");
            assert!(
                toml::from_str::<UserConfig>(&text).is_err(),
                "absurd routing policy was accepted: {invalid}"
            );
        }
        assert_eq!(RouterLatencyMs::DEFAULT.get(), 2_000);
        assert_eq!(RouterCostMicroUsd::DEFAULT.get(), 1_000);
        assert_eq!(PremiumReservePercent::DEFAULT.get(), 20);
    }

    /// Capability map line 1270: capacity-band thresholds are user-
    /// configurable, and a non-ascending set is refused at load time rather
    /// than sorted into shape — the same fail-closed idiom
    /// `routing_policy_values_round_trip_layer_independently_and_reject_absurd_inputs`
    /// already proves for the single-field routing values, applied here to a
    /// value validated across four fields at once.
    #[test]
    fn capacity_band_thresholds_round_trip_and_reject_a_non_monotonic_set() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));

        let mut user = UserConfig::default();
        assert_eq!(user.routing().capacity_band_thresholds(), None);
        user.routing_mut().set_capacity_band_thresholds(Some(
            crate::provider::quota::CapacityBandThresholds::new(1, 10, 30, 60)
                .unwrap()
                .into(),
        ));
        user.save(&paths).unwrap();
        let loaded = UserConfig::load(&paths).unwrap();
        assert_eq!(
            loaded.routing().capacity_band_thresholds(),
            user.routing().capacity_band_thresholds()
        );

        let effective = EffectiveConfig::new(&loaded, None);
        let resolved = effective.capacity_band_thresholds();
        assert_eq!(resolved.layer, Layer::User);
        assert_eq!(resolved.value.reserve_percent(), 10);

        // §35-adjacent: prove the *loader* itself is the fail-closed gate,
        // not merely `CapacityBandThresholds::new` in isolation — this
        // parses through the exact path `UserConfig::load` uses.
        for invalid in [
            // reserve (50) above tight (30): not ascending.
            "[routing.capacity_band_thresholds]\nexhausted_percent = 2\nreserve_percent = 50\n\
             tight_percent = 30\nhealthy_percent = 70\n",
            // healthy_percent above 100.
            "[routing.capacity_band_thresholds]\nexhausted_percent = 2\nreserve_percent = 10\n\
             tight_percent = 30\nhealthy_percent = 150\n",
        ] {
            let text = format!("version = 1\n{invalid}");
            assert!(
                toml::from_str::<UserConfig>(&text).is_err(),
                "a non-monotonic set of capacity-band thresholds was accepted: {invalid}"
            );
        }

        // With nothing recorded, the domain default applies.
        let empty = UserConfig::default();
        let effective = EffectiveConfig::new(&empty, None);
        assert_eq!(
            effective.capacity_band_thresholds().value,
            crate::provider::quota::CapacityBandThresholds::DEFAULT
        );
        assert_eq!(effective.capacity_band_thresholds().layer, Layer::Default);
    }

    /// Capability map lines 1357/1358: routing score weights are
    /// user-configurable, round-trip through the loader, resolve project
    /// over user over [`crate::routing::session::ScoreWeights::default`] —
    /// the same layering [`CapacityBandThresholdsConfig`]'s own test proves
    /// — and a non-finite field is refused at load time rather than
    /// substituted silently.
    #[test]
    fn score_weights_round_trip_layer_project_over_user_and_reject_non_finite_fields() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));

        let mut user = UserConfig::default();
        assert_eq!(user.routing().score_weights(), None);
        let user_weights = crate::routing::session::ScoreWeights {
            quota_pressure_weight: 0.4,
            health_failure_penalty: -0.5,
            health_penalty_floor: -1.2,
            health_unavailable_penalty: -2.0,
        };
        user.routing_mut()
            .set_score_weights(Some(user_weights.into()));
        user.save(&paths).unwrap();
        let loaded = UserConfig::load(&paths).unwrap();
        assert_eq!(
            loaded.routing().score_weights(),
            user.routing().score_weights()
        );

        let effective = EffectiveConfig::new(&loaded, None);
        let resolved = effective.score_weights();
        assert_eq!(resolved.layer, Layer::User);
        assert_eq!(resolved.value, user_weights);

        let mut project = ProjectConfig::default();
        let project_weights = crate::routing::session::ScoreWeights {
            quota_pressure_weight: 0.1,
            ..user_weights
        };
        project
            .routing_mut()
            .set_score_weights(Some(project_weights.into()));
        let effective = EffectiveConfig::new(&loaded, Some(&project));
        let resolved = effective.score_weights();
        assert_eq!(resolved.layer, Layer::Project);
        assert_eq!(resolved.value, project_weights);

        // §35-adjacent: the loader itself is the fail-closed gate, not
        // merely a hypothetical caller of `ScoreWeights` in isolation — this
        // parses through the exact path `UserConfig::load` uses.
        for invalid in [
            "[routing.score_weights]\nquota_pressure_weight = nan\n\
             health_failure_penalty = -0.3\nhealth_penalty_floor = -0.9\n\
             health_unavailable_penalty = -1.5\n",
            "[routing.score_weights]\nquota_pressure_weight = 0.8\n\
             health_failure_penalty = -0.3\nhealth_penalty_floor = -0.9\n\
             health_unavailable_penalty = inf\n",
        ] {
            let text = format!("version = 1\n{invalid}");
            assert!(
                toml::from_str::<UserConfig>(&text).is_err(),
                "a non-finite score weight was accepted: {invalid}"
            );
        }

        // With nothing recorded, the domain default applies — today's
        // compile-time constants, unchanged.
        let empty = UserConfig::default();
        let effective = EffectiveConfig::new(&empty, None);
        assert_eq!(
            effective.score_weights().value,
            crate::routing::session::ScoreWeights::default()
        );
        assert_eq!(effective.score_weights().layer, Layer::Default);
    }

    /// Capability map line 1577: `[routing.reserve]` carries two policies,
    /// they round-trip through the loader, they resolve **per field** with
    /// the project layer first, and a layer that recorded neither leaves the
    /// fail-closed `protect` default in place for both scopes.
    #[test]
    fn reserve_policies_round_trip_and_resolve_per_scope_with_protect_as_the_default() {
        use crate::routing::pressure::{ReservePolicy, ReserveScope};

        let tmp = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));

        let mut user = UserConfig::default();
        assert_eq!(user.routing().reserve(), None);
        let mut reserve = ReservePoliciesConfig::default();
        reserve
            .set_interactive(Some(ReservePolicy::Spend))
            .set_background(Some(ReservePolicy::Protect));
        user.routing_mut().set_reserve(Some(reserve));
        user.save(&paths).unwrap();
        let loaded = UserConfig::load(&paths).unwrap();
        assert_eq!(loaded.routing().reserve(), user.routing().reserve());

        // The on-disk spelling is the enum's own, kebab-case.
        let text = std::fs::read_to_string(paths.user_config_file()).unwrap();
        assert!(text.contains("interactive = \"spend\""), "{text}");
        assert!(text.contains("background = \"protect\""), "{text}");

        let effective = EffectiveConfig::new(&loaded, None);
        let interactive = effective.reserve_policy(ReserveScope::Interactive);
        assert_eq!(
            (interactive.value, interactive.layer),
            (ReservePolicy::Spend, Layer::User)
        );
        let background = effective.reserve_policy(ReserveScope::Background);
        assert_eq!(
            (background.value, background.layer),
            (ReservePolicy::Protect, Layer::User)
        );

        // A project that records only the background policy wins that field
        // and leaves the interactive one to the user layer.
        let project: ProjectConfig =
            toml::from_str("version = 1\n\n[routing.reserve]\nbackground = \"spend\"\n").unwrap();
        let effective = EffectiveConfig::new(&loaded, Some(&project));
        let interactive = effective.reserve_policy(ReserveScope::Interactive);
        assert_eq!(
            (interactive.value, interactive.layer),
            (ReservePolicy::Spend, Layer::User)
        );
        let background = effective.reserve_policy(ReserveScope::Background);
        assert_eq!(
            (background.value, background.layer),
            (ReservePolicy::Spend, Layer::Project)
        );
        assert_eq!(
            effective.reserve_policies(),
            crate::routing::pressure::ReservePolicies {
                interactive: ReservePolicy::Spend,
                background: ReservePolicy::Spend,
            }
        );

        // Nothing recorded anywhere: protect, for both, from the default layer.
        let empty = UserConfig::default();
        let effective = EffectiveConfig::new(&empty, None);
        for scope in [ReserveScope::Interactive, ReserveScope::Background] {
            let resolved = effective.reserve_policy(scope);
            assert_eq!(
                (resolved.value, resolved.layer),
                (ReservePolicy::Protect, Layer::Default)
            );
        }

        // An unknown spelling is refused by the loader rather than defaulted.
        assert!(
            toml::from_str::<UserConfig>(
                "version = 1\n\n[routing.reserve]\ninteractive = \"exclude\"\n"
            )
            .is_err(),
            "an unknown reserve policy must be refused, not read as a default"
        );
    }

    /// Phase 56 lines 1946 and 1947: `[entitlements.<name>]` round-trips
    /// through the loader with the routing types' own spellings, resolves
    /// **by name** with the project layer replacing the user's entry whole,
    /// supplies an unrestricted default for every harness's own sign-in that
    /// nobody claimed, and refuses every unknown spelling rather than reading
    /// it as "no rule".
    #[test]
    fn entitlements_round_trip_and_resolve_project_over_user_with_a_native_default() {
        use crate::profile::BackendResource;
        use crate::routing::classify::WorkloadTier;
        use crate::routing::disposable::JobKind;

        let tmp = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));

        let mut user = UserConfig::default();
        assert!(user.entitlements().is_empty());
        let mut max = EntitlementConfig::default();
        max.set_kind(Some(EntitlementKind::Claude))
            .set_native_harness(Some(IntegrationId::ClaudeCode))
            .set_deny_tiers([WorkloadTier::Leaf])
            .set_allow_job_kinds([JobKind::MemoryExtraction]);
        user.entitlements_mut().set("max", max);
        let mut team = EntitlementConfig::default();
        team.set_kind(Some(EntitlementKind::ApiKey))
            .set_provider(Some("openrouter".to_owned()))
            .set_allow_harnesses([IntegrationId::Codex])
            .set_deny_harnesses([IntegrationId::ClaudeCode]);
        user.entitlements_mut().set("team-key", team);
        user.save(&paths).unwrap();
        let loaded = UserConfig::load(&paths).unwrap();
        assert_eq!(loaded.entitlements(), user.entitlements());

        // The on-disk spellings are the routing types' own.
        let text = std::fs::read_to_string(paths.user_config_file()).unwrap();
        for expected in [
            "[entitlements.max]",
            "kind = \"claude\"",
            "native_harness = \"claude-code\"",
            "deny_tiers = [\"leaf\"]",
            "allow_job_kinds = [\"memory extraction\"]",
            "[entitlements.team-key]",
            "kind = \"api-key\"",
            "provider = \"openrouter\"",
            "allow_harnesses = [\"codex\"]",
            "deny_harnesses = [\"claude-code\"]",
        ] {
            assert!(text.contains(expected), "missing `{expected}` in:\n{text}");
        }

        // The user layer alone: `max` is Claude Code's sign-in, every other
        // harness gets its unrestricted default, and the API key is found by
        // the provider it backs.
        let effective = EffectiveConfig::new(&loaded, None);
        let claude = effective
            .entitlement_for(IntegrationId::ClaudeCode, &BackendResource::Native)
            .unwrap()
            .expect("a harness's own sign-in always resolves to an entitlement");
        assert_eq!((claude.name(), claude.layer()), ("max", Layer::User));
        assert_eq!(claude.kind(), Some(EntitlementKind::Claude));
        assert!(!claude.rules().serves_tier(WorkloadTier::Leaf));
        assert!(claude.rules().serves_tier(WorkloadTier::Heavy));
        assert!(claude.rules().serves_job_kind(JobKind::MemoryExtraction));
        assert!(!claude.rules().serves_job_kind(JobKind::Classification));
        assert_eq!(claude.describe(), "Claude plan, Claude Code's own sign-in");

        let codex = effective
            .entitlement_for(IntegrationId::Codex, &BackendResource::Native)
            .unwrap()
            .unwrap();
        assert_eq!((codex.name(), codex.layer()), ("codex", Layer::Default));
        assert_eq!(codex.kind(), None);
        assert!(codex.rules().is_unrestricted());
        assert_eq!(codex.describe(), "Codex's own sign-in");

        let key = effective
            .entitlement_for(
                IntegrationId::Codex,
                &BackendResource::DirectProvider {
                    provider: "openrouter".to_owned(),
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(key.name(), "team-key");
        assert!(key.rules().serves_harness(IntegrationId::Codex));
        assert!(!key.rules().serves_harness(IntegrationId::ClaudeCode));
        assert!(
            !key.rules().serves_harness(IntegrationId::Cursor),
            "a non-empty allow-list admits only what it names"
        );
        assert_eq!(key.describe(), "API key, behind provider `openrouter`");

        // No entry names this provider, and the gateway assigns its upstream
        // at session start: both are `None`, not a guess.
        assert_eq!(
            effective
                .entitlement_for(
                    IntegrationId::ClaudeCode,
                    &BackendResource::DirectProvider {
                        provider: "nobody-configured".to_owned(),
                    },
                )
                .unwrap(),
            None
        );
        assert_eq!(
            effective
                .entitlement_for(
                    IntegrationId::ClaudeCode,
                    &BackendResource::GlasshouseGateway
                )
                .unwrap(),
            None
        );

        // A project entry of the same name replaces the user's whole: `max`
        // becomes Codex's sign-in with different rules, so Claude Code falls
        // back to its default and Codex is served by the project's `max`.
        let project: ProjectConfig = toml::from_str(
            "version = 1\n\n[entitlements.max]\nnative_harness = \"codex\"\n\
             allow_tiers = [\"heavy\", \"frontier\"]\n",
        )
        .unwrap();
        let effective = EffectiveConfig::new(&loaded, Some(&project));
        let claude = effective
            .entitlement_for(IntegrationId::ClaudeCode, &BackendResource::Native)
            .unwrap()
            .unwrap();
        assert_eq!(
            (claude.name(), claude.layer()),
            ("claude-code", Layer::Default)
        );
        let codex = effective
            .entitlement_for(IntegrationId::Codex, &BackendResource::Native)
            .unwrap()
            .unwrap();
        assert_eq!((codex.name(), codex.layer()), ("max", Layer::Project));
        assert_eq!(
            codex.kind(),
            None,
            "the project's entry replaced the kind too"
        );
        assert!(codex.rules().serves_tier(WorkloadTier::Heavy));
        assert!(!codex.rules().serves_tier(WorkloadTier::Leaf));

        // Every harness — and only a harness — has an entry.
        let all = effective.entitlements().unwrap();
        for id in IntegrationId::ALL {
            let has_default = all
                .iter()
                .any(|s| s.backing() == &EntitlementBacking::NativeHarness(*id));
            assert_eq!(
                has_default,
                id.kind() == crate::integrations::IntegrationKind::Harness,
                "{}",
                id.slug()
            );
        }

        // Unknown spellings are refused by the loader, never read as "no rule".
        for bad in [
            "[entitlements.x]\ndeny_tiers = [\"huge\"]\n",
            "[entitlements.x]\nallow_harnesses = [\"ollama\"]\n",
            "[entitlements.x]\nallow_harnesses = [\"Claude Code\"]\n",
            "[entitlements.x]\nkind = \"netflix\"\n",
            "[entitlements.x]\nallow_job_kinds = [\"laundry\"]\n",
        ] {
            assert!(
                toml::from_str::<UserConfig>(&format!("version = 1\n\n{bad}")).is_err(),
                "must be refused: {bad}"
            );
        }
    }

    /// The contradictions only the resolved set can show, each refused by
    /// name rather than settled by picking one.
    #[test]
    fn contradictory_entitlement_tables_are_refused_by_name() {
        use crate::profile::BackendResource;

        let both: UserConfig = toml::from_str(
            "version = 1\n\n[entitlements.x]\nnative_harness = \"codex\"\nprovider = \"openrouter\"\n",
        )
        .unwrap();
        let err = EffectiveConfig::new(&both, None)
            .entitlements()
            .unwrap_err();
        assert!(matches!(err, EntitlementLookupError::TwoBackings { ref name } if name == "x"));

        let two_claim: UserConfig = toml::from_str(
            "version = 1\n\n[entitlements.a]\nnative_harness = \"codex\"\n\n\
             [entitlements.b]\nnative_harness = \"codex\"\n",
        )
        .unwrap();
        let err = EffectiveConfig::new(&two_claim, None)
            .entitlement_for(IntegrationId::Codex, &BackendResource::Native)
            .unwrap_err();
        assert!(
            matches!(
                &err,
                EntitlementLookupError::AmbiguousNativeHarness { harness: IntegrationId::Codex, names }
                    if names == &["a".to_owned(), "b".to_owned()]
            ),
            "{err}"
        );
        // Claude Code is untouched by Codex's contradiction.
        assert!(
            EffectiveConfig::new(&two_claim, None)
                .entitlement_for(IntegrationId::ClaudeCode, &BackendResource::Native)
                .is_ok()
        );

        let two_providers: UserConfig = toml::from_str(
            "version = 1\n\n[entitlements.a]\nprovider = \"openrouter\"\n\n\
             [entitlements.b]\nprovider = \"openrouter\"\n",
        )
        .unwrap();
        let err = EffectiveConfig::new(&two_providers, None)
            .entitlement_for(
                IntegrationId::Codex,
                &BackendResource::DirectProvider {
                    provider: "openrouter".to_owned(),
                },
            )
            .unwrap_err();
        assert!(
            matches!(&err, EntitlementLookupError::AmbiguousProvider { provider, .. } if provider == "openrouter"),
            "{err}"
        );

        let reserved: UserConfig =
            toml::from_str("version = 1\n\n[entitlements.codex]\nprovider = \"openrouter\"\n")
                .unwrap();
        let err = EffectiveConfig::new(&reserved, None)
            .entitlements()
            .unwrap_err();
        assert!(
            matches!(&err, EntitlementLookupError::NameReservedForHarness { name, harness: IntegrationId::Codex } if name == "codex"),
            "{err}"
        );

        // An entry that names neither backing is listed and matches nothing.
        let unstated: UserConfig =
            toml::from_str("version = 1\n\n[entitlements.someday]\nkind = \"gemini\"\n").unwrap();
        let all = EffectiveConfig::new(&unstated, None)
            .entitlements()
            .unwrap();
        let someday = all.iter().find(|s| s.name() == "someday").unwrap();
        assert_eq!(someday.backing(), &EntitlementBacking::Unstated);
        assert_eq!(someday.describe(), "Gemini plan, no backing stated");
    }

    /// Every [`crate::routing::disposable::JobKind`] is listed in
    /// [`JOB_KIND_SPELLINGS`] exactly once and round-trips through its
    /// spelling — the run-time half of the guard `job_kind_ordinal`'s
    /// exhaustive `match` provides at compile time.
    #[test]
    fn every_job_kind_spelling_round_trips() {
        for (index, kind) in JOB_KIND_SPELLINGS.into_iter().enumerate() {
            assert_eq!(job_kind_ordinal(kind), index, "{kind} is out of order");
            let configured = ConfiguredJobKind::parse(kind.as_str())
                .unwrap_or_else(|| panic!("`{}` must parse", kind.as_str()));
            assert_eq!(configured.kind(), kind);
            let json = serde_json::to_string(&configured).unwrap();
            let back: ConfiguredJobKind = serde_json::from_str(&json).unwrap();
            assert_eq!(back, configured);
        }
        assert_eq!(
            ConfiguredJobKind::parse("Classification"),
            None,
            "exact, not case-folded"
        );
        assert_eq!(
            ConfiguredHarness::parse("cmux"),
            None,
            "cmux is not a harness"
        );
        assert_eq!(
            ConfiguredHarness::parse("claude-code").map(|h| h.id()),
            Some(IntegrationId::ClaudeCode)
        );
    }

    /// Map line 1796, the spelling half. Every
    /// [`crate::routing::classify::WorkloadTier`] is listed in
    /// [`WORKLOAD_TIER_SPELLINGS`] exactly once and round-trips through
    /// [`ConfiguredWorkloadTier`]'s parse and its serialised form — so the
    /// config file's vocabulary is the tier type's own `as_str` and cannot
    /// drift from it.
    ///
    /// [`workload_tier_ordinal`]'s exhaustive `match` is the compile-time
    /// half of the same guard; this is the run-time half that checks the
    /// array and the match still agree.
    #[test]
    fn every_workload_tier_spelling_round_trips() {
        use crate::routing::classify::WorkloadTier;

        assert_eq!(
            WORKLOAD_TIER_SPELLINGS.len(),
            5,
            "a `WorkloadTier` variant was added or removed without updating this array"
        );
        for tier in WORKLOAD_TIER_SPELLINGS {
            assert_eq!(
                WORKLOAD_TIER_SPELLINGS[workload_tier_ordinal(tier)],
                tier,
                "`{tier}` is not at its own ordinal in WORKLOAD_TIER_SPELLINGS"
            );
            let configured = ConfiguredWorkloadTier::new(tier);
            assert_eq!(configured.as_str(), tier.as_str());
            assert_eq!(
                ConfiguredWorkloadTier::parse(tier.as_str()),
                Some(configured),
                "`{tier}` does not parse back from its own spelling"
            );
        }
        // The spellings are the tier type's, not a second vocabulary.
        assert_eq!(
            ConfiguredWorkloadTier::parse("heavy").map(ConfiguredWorkloadTier::tier),
            Some(WorkloadTier::Heavy)
        );
        // And nothing else parses — in particular nothing that would read as
        // a *lower* ceiling than the user wrote.
        for unknown in ["Heavy", "heavy ", "", "tier-3", "premium"] {
            assert_eq!(
                ConfiguredWorkloadTier::parse(unknown),
                None,
                "`{unknown}` must not parse as a workload tier"
            );
        }
    }

    /// Map line 1796, the fail-closed half — practice §68's family. A
    /// misspelt ceiling must be a **load error**, never a silently absent
    /// one: an absent ceiling is what the router reads as *not established*,
    /// so a typo that read as absent would quietly widen the set of
    /// destinations a task may go to and nothing anywhere would say so.
    #[test]
    fn an_unknown_model_ceiling_spelling_is_refused_at_load_rather_than_read_as_absent() {
        let good = "version = 1\n\n[providers.alpha]\ntemplate = \"openrouter\"\n\n\
                    [providers.alpha.model_ceilings]\nsmall = \"leaf\"\n";
        let parsed: UserConfig = toml::from_str(good).expect("a known spelling must load");
        assert_eq!(
            parsed
                .providers()
                .get("alpha")
                .expect("the provider was configured")
                .ceiling_of("small"),
            Some(crate::routing::classify::WorkloadTier::Leaf)
        );

        let typo = "version = 1\n\n[providers.alpha]\ntemplate = \"openrouter\"\n\n\
                    [providers.alpha.model_ceilings]\nsmall = \"lite\"\n";
        let err = toml::from_str::<UserConfig>(typo)
            .expect_err("an unknown workload tier must be refused, not read as no ceiling");
        let rendered = err.to_string();
        assert!(
            rendered.contains("lite") && rendered.contains("leaf"),
            "the refusal must name what was written and what is accepted:\n{rendered}"
        );
    }

    /// Map line 1796's lookup, and the three shapes of *not established*
    /// that must never read as a low ceiling: an unnamed model, an
    /// unconfigured provider, and a provider configured with no ceilings at
    /// all. Layered project-over-user, exactly as
    /// [`EffectiveConfig::model_cost`] is.
    #[test]
    fn model_ceiling_is_layered_and_absent_where_nobody_stated_one() {
        use crate::routing::classify::WorkloadTier;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("workspace");
        std::fs::create_dir_all(&root).unwrap();
        let project_root = test_project(&root);

        let mut user = UserConfig::default();
        let mut user_alpha = ProviderConfig::new("openrouter");
        user_alpha.set_model_ceilings(BTreeMap::from([
            (
                "small".to_owned(),
                ConfiguredWorkloadTier::new(WorkloadTier::Leaf),
            ),
            (
                "big".to_owned(),
                ConfiguredWorkloadTier::new(WorkloadTier::Frontier),
            ),
        ]));
        user.providers_mut().set("alpha", user_alpha);
        // A configured provider that states no ceiling at all.
        user.providers_mut()
            .set("beta", ProviderConfig::new("openrouter"));

        let effective = EffectiveConfig::new(&user, None);
        assert_eq!(
            effective.model_ceiling("alpha", "small"),
            Layered::new(Some(WorkloadTier::Leaf), Layer::User)
        );
        assert_eq!(
            effective.model_ceiling("alpha", "big"),
            Layered::new(Some(WorkloadTier::Frontier), Layer::User)
        );
        assert_eq!(
            effective.model_ceiling("alpha", "unnamed").value,
            None,
            "a model nobody named a ceiling for is not established, not capped"
        );
        assert_eq!(
            effective.model_ceiling("beta", "small").value,
            None,
            "a provider with no ceilings states nothing about any of its models"
        );
        assert_eq!(
            effective.model_ceiling("nowhere", "small"),
            Layered::new(None, Layer::Default),
            "a provider nobody configured is not a provider anybody capped"
        );

        // The project layer wins over the user layer, per provider, the same
        // way `model_cost` resolves beside it.
        let mut project = ProjectConfig::default();
        let mut project_alpha = ProviderConfig::new("openrouter");
        project_alpha.set_model_ceilings(BTreeMap::from([(
            "small".to_owned(),
            ConfiguredWorkloadTier::new(WorkloadTier::Standard),
        )]));
        project.providers_mut().set("alpha", project_alpha);
        let effective = EffectiveConfig::new(&user, Some(&project));
        assert_eq!(
            effective.model_ceiling("alpha", "small"),
            Layered::new(Some(WorkloadTier::Standard), Layer::Project)
        );
        assert_eq!(
            effective.model_ceiling("alpha", "big").value,
            None,
            "the project layer replaces the user's map for that provider rather than \
             merging into it — the same replace-not-merge rule `credential_env` follows"
        );
        drop(project_root);
    }

    // --- GH-CAPABILITY-FACTS: map lines 1517 and 1513 -----------------------

    /// A missing `tool_calls` key must leave `declare_tool_calls`'s output
    /// byte-identical to before the field existed — the census's mutation
    /// (`upgrade-by-association`) is a missing key upgrading to
    /// `Verified{true}`, and this is the test that must fail it.
    #[test]
    fn a_missing_tool_calls_key_leaves_the_templates_declaration_untouched() {
        let config = ProviderConfig::new("openrouter");
        let mut provider = config
            .to_provider("probe")
            .expect("a known template must resolve");
        let before = provider.clone();
        config.declare_tool_calls(&mut provider, Layer::User);

        assert_eq!(
            provider, before,
            "a `ProviderConfig` whose `tool_calls` is `None` must leave `declare_tool_calls`'s \
             output untouched"
        );
        for protocol in &provider.protocols {
            assert_eq!(
                protocol.tool_calls,
                crate::harness::Declared::Unverified,
                "the openrouter template's own tool_calls declaration must survive \
                 untouched when nobody configured tool_calls"
            );
        }
    }

    /// `Some(false)` becomes `Declared::Verified { value: false, .. }` on
    /// every protocol the provider serves, citing the layer and the exact
    /// `[providers.<name>]` table the declaration came from.
    #[test]
    fn a_declared_tool_calls_false_becomes_verified_absent_with_a_layer_reason() {
        let mut config = ProviderConfig::new("openrouter");
        config.set_tool_calls(Some(false));
        let mut provider = config
            .to_provider("probe")
            .expect("a known template must resolve");
        config.declare_tool_calls(&mut provider, Layer::Project);

        assert!(
            !provider.protocols.is_empty(),
            "the openrouter template must declare at least one protocol for this to prove \
             anything"
        );
        for protocol in &provider.protocols {
            match protocol.tool_calls {
                crate::harness::Declared::Verified { value, evidence } => {
                    assert!(!value, "a declared `Some(false)` must verify absent");
                    assert!(
                        evidence.contains("project config") && evidence.contains("[providers]"),
                        "the evidence must name the layer and the [providers] table: {evidence:?}"
                    );
                }
                crate::harness::Declared::Unverified => {
                    panic!("a declared tool_calls value must not stay Unverified")
                }
            }
        }
    }

    /// `Some(true)` becomes `Declared::Verified { value: true, .. }` — the
    /// same producer, the other declared value.
    #[test]
    fn a_declared_tool_calls_true_becomes_verified_present_with_a_layer_reason() {
        let mut config = ProviderConfig::new("openrouter");
        config.set_tool_calls(Some(true));
        let mut provider = config
            .to_provider("probe")
            .expect("a known template must resolve");
        config.declare_tool_calls(&mut provider, Layer::User);

        for protocol in &provider.protocols {
            assert_eq!(
                protocol.tool_calls,
                crate::harness::Declared::verified(
                    true,
                    declared_from_config(Layer::User, DeclaredIn::ProviderToolCalls)
                ),
                "a declared `Some(true)` must verify present, citing the user layer and the \
                 [providers.probe] table"
            );
        }
    }

    /// `resource_facts_of`: an axis absent from a declared model's table
    /// stays `Unverified` — a missing key must never upgrade to `Verified`,
    /// the same rule `tool_calls` follows above.
    #[test]
    fn an_axis_absent_from_a_declared_models_table_stays_unverified() {
        let mut config = ProviderConfig::new("openrouter");
        config.set_model_facts(BTreeMap::from([(
            "small".to_owned(),
            ConfiguredModelFacts {
                shell_tool_use: Some(false),
                ..Default::default()
            },
        )]));

        let facts = config.resource_facts_of("small", Layer::User);
        assert_eq!(
            facts.shell_tool_use,
            crate::harness::Declared::verified(
                false,
                declared_from_config(Layer::User, DeclaredIn::ModelFacts)
            )
        );
        assert_eq!(
            facts.code_edit,
            crate::harness::Declared::Unverified,
            "an axis the user never set on a declared model must stay Unverified, not \
             upgrade because a sibling axis was declared"
        );
        assert_eq!(facts.browser_use, crate::harness::Declared::Unverified);
        assert_eq!(facts.large_context, crate::harness::Declared::Unverified);
        assert_eq!(
            facts.fast_cheap_analysis,
            crate::harness::Declared::Unverified
        );
        assert_eq!(
            facts.repository_review,
            crate::harness::Declared::Unverified
        );
        assert_eq!(facts.mcp, crate::harness::Declared::Unverified);
    }

    /// [`EffectiveConfig::model_facts`]: layered project-over-user exactly as
    /// [`EffectiveConfig::model_cost`] and [`EffectiveConfig::model_ceiling`]
    /// resolve beside it, and the three shapes of *not established* that
    /// must never read as an established absence: an unnamed model, an
    /// unconfigured provider, and a provider that declares no facts at all.
    #[test]
    fn model_facts_is_layered_and_unverified_where_nobody_declared_a_fact() {
        use crate::routing::capability::ResourceFacts;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("workspace");
        std::fs::create_dir_all(&root).unwrap();
        let project_root = test_project(&root);

        let mut user = UserConfig::default();
        let mut user_alpha = ProviderConfig::new("openrouter");
        user_alpha.set_model_facts(BTreeMap::from([(
            "small".to_owned(),
            ConfiguredModelFacts {
                shell_tool_use: Some(false),
                ..Default::default()
            },
        )]));
        user.providers_mut().set("alpha", user_alpha);
        // A configured provider that declares no facts at all.
        user.providers_mut()
            .set("beta", ProviderConfig::new("openrouter"));

        let effective = EffectiveConfig::new(&user, None);
        let small = effective.model_facts("alpha", "small");
        assert_eq!(small.layer, Layer::User);
        assert_eq!(
            small.value.shell_tool_use,
            crate::harness::Declared::verified(
                false,
                declared_from_config(Layer::User, DeclaredIn::ModelFacts)
            )
        );
        assert_eq!(
            small.value.code_edit,
            crate::harness::Declared::Unverified,
            "an undeclared axis on a declared model stays Unverified"
        );
        assert_eq!(
            effective.model_facts("alpha", "unnamed").value,
            ResourceFacts::UNVERIFIED,
            "a model nobody declared facts for is not established, not absent"
        );
        assert_eq!(
            effective.model_facts("beta", "small").value,
            ResourceFacts::UNVERIFIED,
            "a provider that declares no facts states nothing about any of its models"
        );
        assert_eq!(
            effective.model_facts("nowhere", "small"),
            Layered::new(ResourceFacts::UNVERIFIED, Layer::Default),
            "a provider nobody configured is not a provider anybody declared facts for"
        );

        // The project layer replaces the user's map for that provider,
        // exactly as `model_ceiling` resolves beside it.
        let mut project = ProjectConfig::default();
        let mut project_alpha = ProviderConfig::new("openrouter");
        project_alpha.set_model_facts(BTreeMap::from([(
            "small".to_owned(),
            ConfiguredModelFacts {
                shell_tool_use: Some(true),
                ..Default::default()
            },
        )]));
        project.providers_mut().set("alpha", project_alpha);
        let effective = EffectiveConfig::new(&user, Some(&project));
        let small = effective.model_facts("alpha", "small");
        assert_eq!(small.layer, Layer::Project);
        assert_eq!(
            small.value.shell_tool_use,
            crate::harness::Declared::verified(
                true,
                declared_from_config(Layer::Project, DeclaredIn::ModelFacts)
            )
        );
        drop(project_root);
    }

    /// [`EffectiveConfig::configured_provider`]: a project-layer `tool_calls`
    /// declaration wins over a user-layer one for the same provider name —
    /// the same project-over-user precedence
    /// [`EffectiveConfig::model_cost`] and [`EffectiveConfig::model_facts`]
    /// apply beside it.
    #[test]
    fn configured_provider_layers_tool_calls_project_over_user() {
        let mut user = UserConfig::default();
        let mut user_alpha = ProviderConfig::new("openrouter");
        user_alpha.set_tool_calls(Some(false));
        user.providers_mut().set("alpha", user_alpha);

        let mut project = ProjectConfig::default();
        let mut project_alpha = ProviderConfig::new("openrouter");
        project_alpha.set_tool_calls(Some(true));
        project.providers_mut().set("alpha", project_alpha);

        let effective = EffectiveConfig::new(&user, Some(&project));
        let resolved = effective
            .configured_provider("alpha")
            .expect("a configured provider must resolve");
        assert_eq!(resolved.layer, Layer::Project);
        for protocol in &resolved.value.protocols {
            match protocol.tool_calls {
                crate::harness::Declared::Verified { value, evidence } => {
                    assert!(
                        value,
                        "the project layer's `tool_calls = true` must win over the user \
                         layer's `false`"
                    );
                    assert!(
                        evidence.contains("project"),
                        "the evidence must attribute the winning declaration to the \
                         project layer: {evidence:?}"
                    );
                }
                crate::harness::Declared::Unverified => {
                    panic!("the project layer's declared tool_calls must not read as Unverified")
                }
            }
        }
    }
}
