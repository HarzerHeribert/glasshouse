//! Phase 32: one place that enumerates every kind of model resource
//! Glasshouse can describe, honest about the fact that their quotas do not
//! work the same way.
//!
//! # Why this is a derived view, not a rewrite
//!
//! The kinds themselves already exist and already ship: a harness's own
//! subscription and a direct provider are [`crate::profile::BackendResource`]
//! variants resolved for every launch, and the concrete providers — routers,
//! generic templates, and the two local-inference servers — are
//! [`crate::provider::templates`]. Nothing here replaces either. This module
//! adds the one thing neither states on its own: **which quota shape each
//! entry actually has**, which is the map's own fixed requirement for this
//! phase — subscriptions, metered keys, and local inference are normalized
//! into one list without being told apart, and told apart is exactly what a
//! `BackendResource::DirectProvider { provider: "ollama" }` and a
//! `BackendResource::DirectProvider { provider: "openrouter" }` are not
//! today: both are "a direct provider" and nothing distinguishes the one
//! that cannot run out of money from the one that can. That is
//! [`Locality`] and [`QuotaModel`].
//!
//! # What this does not do
//!
//! It does not add a network call — every entry here is built from
//! [`crate::provider::templates`] and [`crate::integrations::IntegrationId`],
//! both already-declared, static catalogs. It does not read or hold a
//! credential — [`ResourceKind::DirectProvider`] carries a provider *name*,
//! the same thing [`crate::profile::BackendResource::DirectProvider`] already
//! carries. And it does not track live quota telemetry (a rolling-window
//! reset time, a spent balance, a request count) — that is Phase 32B, which
//! does not exist yet; [`QuotaModel`] names the *shape* a resource's quota
//! takes, not its current state.

use crate::integrations::{IntegrationId, IntegrationKind};
use crate::provider;

/// Whether a resource's compute runs on this machine or somewhere else.
///
/// Capability map line 1185: local inference must be represented separately
/// from remote resources, and this is the field that does it — a
/// [`ResourceKind::DirectProvider`] carries one, and it is computed from
/// [`IntegrationKind::LocalInference`] rather than guessed at from a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Locality {
    /// Served on this machine — Ollama and llama.cpp today.
    Local,
    /// Reached over the network — every configured router, gateway or
    /// metered API.
    Remote,
}

/// How a resource's capacity is known to run out, if it can at all.
///
/// This is the phase's fixed requirement in code: a registry that flattened
/// every kind of capacity to one "it has capacity or it does not" boolean
/// would have satisfied [`ResourceKind`]'s existence and broken the
/// requirement in the same motion. Each variant names a *shape*, never a
/// number — no rolling-window reset time, no spent balance, no request
/// count. Reading the live state behind any of them is quota telemetry,
/// which Phase 32B owns and which does not exist yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaModel {
    /// A harness's own first-party subscription. Its capacity resets on a
    /// rolling window Glasshouse does not measure — a request count and a
    /// dollar balance are both the wrong shape for it.
    RollingWindowSubscription,
    /// An account balance, spent per request. Whether a particular model on
    /// this provider is actually billed against it, or marked free-tier
    /// instead, is a per-model fact this registry does not carry — see
    /// [`crate::routing::Cost`] and [`crate::config::ProviderConfig`]'s
    /// `free_models`, which already own exactly that distinction.
    MeteredBalance,
    /// No metering at all. A local inference server cannot run out of
    /// money, and pretending it has a balance would be inventing a number
    /// nobody can read.
    Unmetered,
    /// The Glasshouse gateway is a local router, not a capacity of its
    /// own — its quota is whichever upstream it is currently bound to, which
    /// is a per-session fact `crate::routing::interactive::Assignment`
    /// already records. Naming that here as `MeteredBalance` would claim a
    /// shape for a resource that can, in fact, be bound to an unmetered one.
    DelegatedToUpstream,
}

/// One kind of model resource the registry can describe — independent of
/// whether any [`crate::profile::LaunchProfile`] is currently using it.
///
/// Deliberately named distinctly from [`crate::profile::BackendResource`]
/// rather than reusing it: a `BackendResource` is *what one profile is
/// pointed at*, resolved and launched; a [`ResourceKind`] is *a kind of
/// capacity Glasshouse knows how to describe*, whether or not a profile
/// exists for it yet. [`ResourceKind::from_direct_provider`] is the bridge
/// between the two, used at the one place a `BackendResource` is actually
/// resolved for a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceKind {
    /// A harness's own first-party authentication —
    /// [`crate::profile::BackendResource::Native`], named by which harness.
    NativeSubscription { harness: IntegrationId },
    /// A provider or router reached directly by a harness —
    /// [`crate::profile::BackendResource::DirectProvider`]. Carries the
    /// same provider *name* that variant does, never a credential.
    DirectProvider {
        provider: String,
        locality: Locality,
    },
    /// The local Glasshouse gateway —
    /// [`crate::profile::BackendResource::GlasshouseGateway`]. One entry:
    /// there is exactly one gateway process, whatever it is currently
    /// forwarding to.
    GlasshouseGateway,
}

impl ResourceKind {
    /// Where this resource's compute runs.
    ///
    /// A harness's own subscription always reaches a remote model, and the
    /// gateway is answered as [`Locality::Local`] for the process itself —
    /// the loopback listener a harness is pointed at — which is a different
    /// question from where the upstream *it* forwards to runs; that is
    /// exactly what [`QuotaModel::DelegatedToUpstream`] exists to avoid
    /// pretending this method answers.
    pub fn locality(&self) -> Locality {
        match self {
            ResourceKind::NativeSubscription { .. } => Locality::Remote,
            ResourceKind::DirectProvider { locality, .. } => *locality,
            ResourceKind::GlasshouseGateway => Locality::Local,
        }
    }

    /// The shape of this resource's quota — see [`QuotaModel`].
    ///
    /// Phase 32A: projected out of
    /// [`crate::provider::quota::CapacityState`] rather than computed beside
    /// it. The shape a resource's quota takes is one fact *about* its
    /// capacity, and deriving it twice is how the two would come to
    /// disagree — a resource whose capacity model said "nothing can exhaust
    /// this" while its quota shape said `MeteredBalance` would be a bug no
    /// test of either half alone could see.
    ///
    /// This is also what puts the capacity model on the production launch
    /// path without a new caller: `profile::apply_direct_provider` and
    /// `profile::apply_gateway` already call this method for every session's
    /// `"resource kind"` mechanism note, so every launch now builds a
    /// [`crate::provider::quota::CapacityState`] and reads its shape out.
    pub fn quota(&self) -> QuotaModel {
        self.capacity().model()
    }

    /// The [`ResourceKind`] a resolved
    /// [`crate::profile::BackendResource::DirectProvider`]'s provider name
    /// describes.
    ///
    /// This is the one function the launch path actually calls — see
    /// `crate::profile::apply_direct_provider`'s "resource kind" mechanism
    /// note — which is what keeps this type from being a registry nothing
    /// consults: every direct-provider session records what it resolved to.
    pub fn from_direct_provider(provider: impl Into<String>) -> Self {
        let provider = provider.into();
        let locality = locality_of(&provider);
        ResourceKind::DirectProvider { provider, locality }
    }

    /// A short, stable label for a diagnostic — the launch mechanism note
    /// and the acceptance tests both read this rather than formatting the
    /// variant by hand, so the two cannot drift.
    pub fn label(&self) -> String {
        match self {
            ResourceKind::NativeSubscription { harness } => {
                format!("{} subscription", harness.display_name())
            }
            ResourceKind::DirectProvider { provider, locality } => {
                format!("{provider} ({})", locality.as_str())
            }
            ResourceKind::GlasshouseGateway => "glasshouse gateway".to_owned(),
        }
    }
}

impl Locality {
    pub fn as_str(self) -> &'static str {
        match self {
            Locality::Local => "local",
            Locality::Remote => "remote",
        }
    }
}

impl QuotaModel {
    pub fn as_str(self) -> &'static str {
        match self {
            QuotaModel::RollingWindowSubscription => "rolling-window subscription",
            QuotaModel::MeteredBalance => "metered balance",
            QuotaModel::Unmetered => "unmetered",
            QuotaModel::DelegatedToUpstream => "delegated to its assigned upstream",
        }
    }
}

/// Whether `provider_name` names a local-inference server rather than a
/// remote one.
///
/// Matched against [`IntegrationId::slug`] for exactly the integrations
/// [`IntegrationKind::LocalInference`] names — Ollama and llama.cpp — rather
/// than against a prefix or a hostname, so a user-configured provider that
/// merely happens to point its base URL at `localhost` (a self-hosted
/// LiteLLM proxy, say) is not silently reclassified as local inference: this
/// answers "which server is this", not "which address does it use today".
fn locality_of(provider_name: &str) -> Locality {
    let is_local = IntegrationId::ALL
        .iter()
        .any(|id| id.kind() == IntegrationKind::LocalInference && id.slug() == provider_name);
    if is_local {
        Locality::Local
    } else {
        Locality::Remote
    }
}

/// Every kind of model resource Glasshouse can describe today.
///
/// Capability map line 1183: this is the registry. It enumerates —
///
/// - a [`ResourceKind::NativeSubscription`] for every
///   [`IntegrationKind::Harness`] integration, which covers Claude Code
///   (line 1186), Codex (line 1187) and Antigravity (line 1188) by
///   construction rather than by naming them specially — any harness this
///   project adds a native profile for is described the same way;
/// - a [`ResourceKind::DirectProvider`] for every
///   [`crate::provider::templates`] entry, which covers OpenRouter (line
///   1189), the user-configured routers UnoRouter/AnyRouter/Kilo/Nous plus
///   the two generic templates a user's own gateway is configured through
///   (line 1190), and Ollama and llama.cpp (lines 1191, 1192) — the last two
///   distinguished from every other entry by [`Locality::Local`];
/// - one [`ResourceKind::GlasshouseGateway`].
///
/// This lists what Glasshouse can describe, not what a user has configured
/// — a template with no credential is still a resource *kind* the registry
/// knows about, the same way [`crate::provider::templates`] itself lists
/// providers nobody has necessarily set up.
pub fn registry() -> Vec<ResourceKind> {
    let mut out: Vec<ResourceKind> = IntegrationId::ALL
        .iter()
        .filter(|id| id.kind() == IntegrationKind::Harness)
        .map(|&harness| ResourceKind::NativeSubscription { harness })
        .collect();

    out.extend(
        provider::templates()
            .into_iter()
            .map(|provider| ResourceKind::from_direct_provider(provider.name)),
    );

    out.push(ResourceKind::GlasshouseGateway);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find_direct<'a>(entries: &'a [ResourceKind], name: &str) -> &'a ResourceKind {
        entries
            .iter()
            .find(|entry| matches!(entry, ResourceKind::DirectProvider { provider, .. } if provider == name))
            .unwrap_or_else(|| panic!("registry() has no entry for `{name}`"))
    }

    // --- line 1183: the registry exists and lists something ---------------

    #[test]
    fn the_registry_is_not_empty() {
        assert!(!registry().is_empty());
    }

    // --- line 1184: native subscription is a different kind than a direct
    // provider or a gateway, at the quota level, not only the type level ---

    #[test]
    fn a_native_subscription_and_a_direct_provider_have_different_quota_shapes() {
        let native = ResourceKind::NativeSubscription {
            harness: IntegrationId::ClaudeCode,
        };
        let direct = ResourceKind::from_direct_provider("openrouter");
        assert_eq!(native.quota(), QuotaModel::RollingWindowSubscription);
        assert_eq!(direct.quota(), QuotaModel::MeteredBalance);
        assert_ne!(native.quota(), direct.quota());
    }

    #[test]
    fn the_gateway_is_a_third_kind_delegated_rather_than_flattened_into_either() {
        let gateway = ResourceKind::GlasshouseGateway;
        assert_eq!(gateway.quota(), QuotaModel::DelegatedToUpstream);
        assert_ne!(gateway.quota(), QuotaModel::RollingWindowSubscription);
        assert_ne!(gateway.quota(), QuotaModel::MeteredBalance);
    }

    // --- line 1185: local inference is locality-tagged, and by which
    // server it is rather than by the address it happens to use today -----

    #[test]
    fn ollama_and_llama_cpp_are_local_and_unmetered() {
        let entries = registry();
        for name in ["ollama", "llama-cpp"] {
            let entry = find_direct(&entries, name);
            assert_eq!(entry.locality(), Locality::Local, "{name}");
            assert_eq!(entry.quota(), QuotaModel::Unmetered, "{name}");
        }
    }

    #[test]
    fn every_router_and_generic_template_is_remote_and_metered() {
        let entries = registry();
        for name in [
            "openrouter",
            "unorouter",
            "anyrouter",
            "kilo",
            "nous",
            "zai",
            "opencode-zen",
            "nvidia",
            "litellm",
            "openai-compatible",
            "anthropic-compatible",
        ] {
            let entry = find_direct(&entries, name);
            assert_eq!(entry.locality(), Locality::Remote, "{name}");
            assert_eq!(entry.quota(), QuotaModel::MeteredBalance, "{name}");
        }
    }

    /// A provider whose base URL happens to be `localhost` — a self-hosted
    /// LiteLLM proxy, which this project's own template points at
    /// `http://0.0.0.0:4000` — must not be reclassified as local inference
    /// on that basis. Locality is decided by which server this is, not by
    /// which address it answers on today.
    #[test]
    fn a_localhost_base_url_does_not_by_itself_make_a_provider_local_inference() {
        let litellm = provider::template("litellm").expect("litellm is a built-in template");
        assert!(
            litellm
                .protocols
                .iter()
                .any(|p| p.base_url.contains("0.0.0.0") || p.base_url.contains("localhost")),
            "this test's premise requires litellm's base URL to be loopback-shaped"
        );
        assert_eq!(locality_of("litellm"), Locality::Remote);
    }

    // --- lines 1186-1192: every named resource kind is actually reachable
    // through the registry, by the name the map uses for it -----------------

    #[test]
    fn claude_code_codex_and_antigravity_are_native_subscriptions() {
        let entries = registry();
        for harness in [
            IntegrationId::ClaudeCode,
            IntegrationId::Codex,
            IntegrationId::Antigravity,
        ] {
            assert!(
                entries
                    .iter()
                    .any(|entry| matches!(entry, ResourceKind::NativeSubscription { harness: h } if *h == harness)),
                "{harness:?} is missing a native-subscription entry"
            );
        }
    }

    #[test]
    fn openrouter_unorouter_anyrouter_kilo_and_nous_are_all_describable() {
        let entries = registry();
        for name in ["openrouter", "unorouter", "anyrouter", "kilo", "nous"] {
            find_direct(&entries, name);
        }
    }

    // --- `from_direct_provider` is the bridge a real launch uses ----------

    #[test]
    fn from_direct_provider_agrees_with_the_registrys_own_classification() {
        let entries = registry();
        for name in ["ollama", "llama-cpp", "openrouter", "nvidia"] {
            let via_registry = find_direct(&entries, name);
            let via_bridge = ResourceKind::from_direct_provider(name);
            assert_eq!(via_registry.locality(), via_bridge.locality());
            assert_eq!(via_registry.quota(), via_bridge.quota());
        }
    }

    #[test]
    fn a_label_never_contains_a_credential_shaped_string() {
        // The label is the text a mechanism note carries into a launch log —
        // it must never be able to grow one from a provider name a user
        // configured, which is why it is built from fixed phrases and the
        // name alone, never from anything resolved through `crate::secret`.
        let entry = ResourceKind::from_direct_provider("openrouter");
        assert_eq!(entry.label(), "openrouter (remote)");
    }
}
