//! Shared launch-profile support for every process-owning presentation.
//!
//! The CLI and interactive shell differ in who owns the child's terminal,
//! while entitlement, gateway, secret scope, and pairing decisions must be
//! identical before either one starts a harness.

use crate::config::effective::ProfileLookupError;
use crate::config::{EffectiveConfig, ProjectConfig, ResolvedEntitlement, UserConfig};
use crate::integrations::IntegrationId;
use crate::secret::{Secret, SecretRef, SecretStore};

use crate::profile::{self, GatewayPairing, LaunchProfile};

pub fn gateway_pairing(effective: &EffectiveConfig<'_>) -> GatewayPairing {
    let (preference, _source) = effective.native_pairing_preference();
    GatewayPairing {
        preference_slug: preference.slug(),
        overrides: effective.pairing_overrides(),
    }
}

pub fn gateway_entitlement(
    effective: &EffectiveConfig<'_>,
    profile: &LaunchProfile,
    exact: Option<&str>,
) -> anyhow::Result<Option<ResolvedEntitlement>> {
    let requested = exact.or(profile.entitlement.as_deref());
    let Some(name) = requested else {
        return Ok(None);
    };
    let all = effective.configured_entitlements()?;
    if let (Some(stored), Some(pinned)) = (exact, profile.entitlement.as_deref())
        && stored != pinned
    {
        anyhow::bail!(
            "stored gateway entitlement `{stored}` no longer matches profile `{}` entitlement `{pinned}`",
            profile.name
        );
    }
    let entry = all
        .into_iter()
        .find(|entry| entry.name() == name)
        .ok_or_else(|| anyhow::anyhow!("gateway entitlement `{name}` is not configured"))?;
    if entry.backing().subscription_broker().is_none() {
        anyhow::bail!("gateway entitlement `{name}` is not backed by a subscription broker");
    }
    if !entry.rules().serves_harness(profile.harness) {
        anyhow::bail!(
            "gateway entitlement `{name}` does not permit harness `{}`",
            profile.harness.slug()
        );
    }
    if let (Some(model), Some(crate::config::EntitlementModels::Declared { models, .. })) =
        (profile.model.as_deref(), entry.models())
        && !models.iter().any(|candidate| candidate == model)
    {
        anyhow::bail!("gateway entitlement `{name}` does not serve requested model `{model}`");
    }
    Ok(Some(entry))
}

pub fn gateway_upstream(
    user: &UserConfig,
    project: Option<&ProjectConfig>,
    effective: &EffectiveConfig<'_>,
    secrets: &dyn SecretStore,
    entitlement: Option<&ResolvedEntitlement>,
    paths: &crate::RuntimePaths,
) -> anyhow::Result<crate::gateway::Upstream> {
    if let Some(entitlement) = entitlement {
        let broker = crate::gateway::subscription_broker::RunningSubscriptionBroker::start(
            &paths.subscription_broker_paths(entitlement.name()),
            entitlement.name(),
        )?;
        return Ok(profile::subscription_broker_upstream(broker)?);
    }
    let mut providers = Vec::new();
    for name in effective.provider_names() {
        providers.push(effective.configured_provider(&name)?.value);
    }
    let free = |name: &str| -> bool {
        project
            .and_then(|project| project.providers().get(name))
            .or_else(|| user.providers().get(name))
            .is_some_and(|config| !config.free_models().is_empty())
    };
    Ok(profile::gateway_upstream(&providers, secrets, &free)?)
}

pub fn session_pairing(
    effective: &EffectiveConfig<'_>,
    profile: &LaunchProfile,
) -> crate::harness::pairing::Pairing {
    use crate::harness::Declared;
    use crate::harness::pairing::{PairingQuery, ServingRoute, classify};
    use crate::routing::AssignedModel;

    let configured = effective
        .pairing_queries()
        .into_iter()
        .find(|configured| configured.name() == profile.name)
        .and_then(|configured| configured.query().cloned());
    let query = configured.unwrap_or_else(|| PairingQuery {
        harness: profile.harness,
        model: profile
            .model
            .as_deref()
            .map(AssignedModel::named)
            .unwrap_or(AssignedModel::HarnessDefault),
        route: ServingRoute {
            provider: None,
            gateway: None,
            protocol: profile.expected_protocol,
        },
        tool_calls: Declared::Unverified,
        provider_protocols: Vec::new(),
    });
    classify(&query, &effective.pairing_overrides())
}

pub struct EntitlementScopedSecrets<'a> {
    inner: &'a dyn SecretStore,
    foreign: Vec<SecretRef>,
}

impl<'a> EntitlementScopedSecrets<'a> {
    pub fn new(
        inner: &'a dyn SecretStore,
        effective: &EffectiveConfig<'_>,
        serving: Option<&str>,
    ) -> Self {
        Self {
            inner,
            foreign: effective.foreign_entitlement_credential_refs(serving),
        }
    }
}

impl SecretStore for EntitlementScopedSecrets<'_> {
    fn resolve(&self, reference: &SecretRef) -> Option<Secret> {
        if self.foreign.contains(reference) {
            return None;
        }
        self.inner.resolve(reference)
    }

    fn is_present(&self, reference: &SecretRef) -> bool {
        !self.foreign.contains(reference) && self.inner.is_present(reference)
    }

    fn describe(&self) -> &'static str {
        self.inner.describe()
    }
}

pub fn enabled_profiles(
    effective: &EffectiveConfig<'_>,
    harness: IntegrationId,
) -> anyhow::Result<Vec<LaunchProfile>> {
    let mut profiles = vec![LaunchProfile::native(harness)];
    for name in effective.profile_names() {
        if name == profile::NATIVE_PROFILE_NAME || !effective.profile_enabled(&name).value {
            continue;
        }
        match effective.launch_profile(&name, harness) {
            Ok(profile) => profiles.push(profile.value),
            Err(ProfileLookupError::HarnessMismatch { .. }) => {}
            Err(err) => return Err(err.into()),
        }
    }
    Ok(profiles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ProfileBackend, ProfileConfig};

    #[test]
    fn enabled_profiles_are_native_plus_enabled_profiles_for_the_selected_harness() {
        let mut user = UserConfig::default();
        let mut enabled = ProfileConfig::new(IntegrationId::ClaudeCode);
        enabled
            .set_backend(ProfileBackend::DirectProvider {
                provider: "alpha".to_owned(),
            })
            .set_model(Some("claude".to_owned()));
        user.profiles_mut().set("economy", enabled);

        let mut disabled = ProfileConfig::new(IntegrationId::ClaudeCode);
        disabled.set_enabled(false);
        user.profiles_mut().set("disabled", disabled);
        user.profiles_mut()
            .set("other-harness", ProfileConfig::new(IntegrationId::Codex));

        let effective = EffectiveConfig::new(&user, None);
        let profiles = enabled_profiles(&effective, IntegrationId::ClaudeCode).unwrap();
        let names = profiles
            .iter()
            .map(|profile| profile.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(names, [profile::NATIVE_PROFILE_NAME, "economy"]);
        assert!(
            profiles
                .iter()
                .all(|profile| profile.harness == IntegrationId::ClaudeCode)
        );
    }
}
