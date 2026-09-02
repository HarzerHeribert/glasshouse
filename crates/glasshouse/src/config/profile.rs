//! Launch profile configuration: inert, named profiles a session can launch (backend, approval mode, executable).
//!

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::integrations::IntegrationId;

use super::*;

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
    pub(super) harness: String,
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
    /// `[profiles.<name>.context_firewall]` — map line 2024's explicit
    /// override, outranking the serving entitlement's kind and the flat
    /// `[context_firewall]` table, and itself outranked by nothing: the
    /// profile is the more specific choice, one profile serving one launch.
    /// `None` here, on a file written before this field existed, loads as
    /// "this profile states no override" — the behaviour those files
    /// already had.
    #[serde(
        default,
        skip_serializing_if = "firewall::ContextFirewallOverride::is_unset"
    )]
    context_firewall: firewall::ContextFirewallOverride,
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
pub(super) fn enabled_by_default() -> bool {
    true
}
pub(super) fn is_enabled_by_default(enabled: &bool) -> bool {
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
            context_firewall: firewall::ContextFirewallOverride::default(),
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

    /// This profile's `[profiles.<name>.context_firewall]` override — see
    /// the field's own doc.
    pub fn context_firewall(&self) -> &firewall::ContextFirewallOverride {
        &self.context_firewall
    }

    pub fn context_firewall_mut(&mut self) -> &mut firewall::ContextFirewallOverride {
        &mut self.context_firewall
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
        "gemini-generate-content" => Some(WireProtocol::GeminiGenerateContent),
        _ => None,
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
