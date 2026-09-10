//! Glasshouse's view of the provider catalogue.
//!
//! The catalogue itself — [`Provider`], the protocol model, the built-in
//! templates and the resource registry — belongs to
//! [`inference_gateway::provider`] and is re-exported here so that every
//! existing `crate::provider::…` path still resolves. Nothing is redefined:
//! a second copy of a provider table is a second thing to be wrong.
//!
//! # What Glasshouse still owns, and why exactly this much
//!
//! One fact, and it is the fact the gateway must not hold: **which coding
//! agents exist**. The gateway describes a native subscription by an opaque
//! client slug and does not know what any client is (user ruling
//! 2026-09-10), so the fold over [`crate::integrations::IntegrationId`] that
//! turns Glasshouse's harnesses into that list lives in [`mod@registry`]
//! here, and everything downstream of it lives in the crate.
//!
//! [`pricing`] and [`resources`] stay too: both are about what Glasshouse
//! *reports* to a user, not about what a provider is.

pub mod pricing;
pub mod resources;

pub use inference_gateway::provider::{
    GENERIC_TEMPLATE_NAMES, ProtocolCompatibleProvider, ProtocolCompatibleProviders,
    ProtocolSupport, Provider, template, templates, translation_available, unverified_provider,
    unverified_support, usage_endpoint,
};
pub use inference_gateway::provider::{budget, cache, discovery, quota, telemetry};

/// Fixture providers, for tests only — the crate exports the module
/// unconditionally; Glasshouse keeps it test-scoped as it always was.
#[cfg(test)]
pub(crate) use inference_gateway::provider::fixture;

/// In scope for `tests.rs`'s `use super::*`, which builds [`Provider`]
/// literals the same way the crate's own templates do.
#[cfg(test)]
use crate::harness::{Declared, WireProtocol};

/// The resource registry, and the one half of it Glasshouse supplies.
///
/// [`inference_gateway::provider::registry`] enumerates resource *kinds* and
/// takes its native subscriptions from its caller, because a gateway that
/// folded over its own list of clients would be reproducing Glasshouse's
/// reasoning about which agents exist. This module is that caller: it hands
/// the crate Glasshouse's harnesses and re-exports the result under the path
/// `crate::provider::registry::registry()` that every consumer already uses.
pub mod registry {
    use crate::integrations::{IntegrationId, IntegrationKind};

    pub use inference_gateway::provider::registry::{
        Locality, NativeClient, QuotaModel, ResourceKind,
    };

    /// `harness` as the gateway sees it: a slug it compares and a display
    /// name it prints, neither of which it interprets.
    ///
    /// Both are handed over rather than only the slug so that
    /// `glasshouse resources` still prints "Claude Code subscription" and not
    /// "claude-code subscription" — the display name is Glasshouse's fact,
    /// and the gateway carries it without knowing what it means.
    pub fn client_for(harness: IntegrationId) -> NativeClient {
        NativeClient::new(harness.slug(), harness.display_name())
    }

    /// The resource kind one harness's own first-party sign-in describes.
    ///
    /// The bridge every launch path uses: it holds an [`IntegrationId`] and
    /// the gateway holds a slug, and this is the one line between them.
    pub fn native_subscription(harness: IntegrationId) -> ResourceKind {
        ResourceKind::NativeSubscription {
            client: client_for(harness),
        }
    }

    /// Every harness Glasshouse can hold a native subscription for, in
    /// [`IntegrationId::ALL`]'s presentation order.
    pub fn native_clients() -> Vec<NativeClient> {
        IntegrationId::ALL
            .iter()
            .filter(|id| id.kind() == IntegrationKind::Harness)
            .map(|id| client_for(*id))
            .collect()
    }

    /// The harness whose slug is `client`'s, or `None`.
    ///
    /// The reverse of [`native_clients`], and the only place a slug becomes a
    /// harness again: a consumer that needs harness-reported telemetry for a
    /// [`ResourceKind::NativeSubscription`] resolves it here rather than in
    /// the gateway, which carries the slug and never reads meaning into it.
    pub fn integration_for(client: &NativeClient) -> Option<IntegrationId> {
        IntegrationId::ALL
            .iter()
            .copied()
            .find(|id| id.slug() == client.slug)
    }

    /// Every kind of model resource Glasshouse can describe today —
    /// capability map line 1183, with lines 1186-1188's harnesses supplied by
    /// [`native_clients`].
    pub fn registry() -> Vec<ResourceKind> {
        inference_gateway::provider::registry::registry(&native_clients())
    }
}

/// Services a user may hold a credential for but which have no template in
/// [`templates`] because no endpoint has been established for them.
///
/// **This list is empty today, and that is a statement rather than a gap.**
/// It held Kilo and Nous, both of which were given real endpoints on
/// 2026-08-26 — exactly the transition this list exists to make visible. The
/// mechanism stays because an absence has to stay assertable: the next
/// credential someone holds for a service with no readable endpoint belongs
/// here, not in a guessed template. See
/// `no_template_exists_for_a_service_whose_endpoint_is_unestablished`, whose
/// control case is what keeps the check itself honest while the list is
/// empty.
#[cfg(test)]
const DELIBERATELY_UNTEMPLATED: &[(&str, &str)] = &[];

#[cfg(test)]
mod tests;
