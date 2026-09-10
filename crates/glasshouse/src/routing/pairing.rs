//! The client-neutral pairing values a routing policy ranks candidates with.
//!
//! User ruling 2026-09-10: the gateway ranks same-model resources on
//! compatibility, entitlement and cost, quota, health, cache locality,
//! stickiness and failure-domain information. *Which* client is talking to it
//! is not on that list, and a gateway that derived a routing prior from a
//! harness identity would be a gateway only Glasshouse could ship.
//!
//! [`RouteAffinity`] is how a caller that does know says what it knows, in
//! terms this side can honour without learning what a client is: a
//! preference, and the caller's own sentence for why. [`PairingAffinities`]
//! carries those judgements keyed by the two things a
//! [`crate::routing::Backend`] already shows — its provider and its model —
//! so nothing here has to be told how the caller decided. **Empty is the
//! honest default**: nothing preferred, every prior `0.0`, which is exactly
//! what a caller with nothing to say should produce.
//!
//! [`ServingRoute`], [`wire_protocol_from_slug`] and [`EvidenceKey`] live
//! here rather than in `crate::harness::pairing` for the same reason: they
//! are route identity and evidence identity, which a routing policy needs and
//! a harness model does not own. `crate::harness::pairing` re-exports all
//! three, so every existing import path stays valid.

use std::collections::BTreeMap;

use crate::harness::WireProtocol;
use crate::routing::AssignedModel;

/// Who is serving the model, and over what.
///
/// Three fields, stored apart from the model and apart from the harness,
/// because line 554 says so and because line 555 is the failure that happens
/// when they are not: a reseller in `provider` must never become an answer to
/// "who developed this".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServingRoute {
    /// The service the request is sent to. `None` for a harness running on
    /// its own vendor's first-party service.
    pub provider: Option<String>,
    /// The gateway in front of it, when there is one.
    pub gateway: Option<String>,
    /// The wire protocol the request is carried over.
    pub protocol: Option<WireProtocol>,
}

/// The reverse of [`WireProtocol::slug`], for a caller that only has the
/// slug a [`crate::routing::Backend`] carries — that type's own doc comment
/// explains why `routing` keeps the protocol as a string and never parses it
/// back; this is that parse, for the callers that need a
/// [`ServingRoute::protocol`] to identify a route.
///
/// `None` for a slug none of the four known variants produced — an affinity's
/// `preferred` never depends on it (see
/// [`crate::config::pairing::native_pairing_prior_contribution`]'s own doc),
/// so this only ever weakens a classification, never invents one.
pub fn wire_protocol_from_slug(slug: &str) -> Option<WireProtocol> {
    [
        WireProtocol::AnthropicMessages,
        WireProtocol::OpenAiResponses,
        WireProtocol::OpenAiChat,
        WireProtocol::GeminiGenerateContent,
    ]
    .into_iter()
    .find(|protocol| protocol.slug() == slug)
}

/// The four-part identity Phase 9J line 572 requires local evidence to be
/// kept apart by: client, launch profile, model, and the exact serving route.
///
/// A nominal model id is not enough — the same id reached through a different
/// gateway, quantization, revision or protocol translation is different
/// evidence, and [`ServingRoute`] is exactly the value that already carries
/// that distinction (its `gateway` and `protocol` fields), so this type reuses
/// it rather than inventing a parallel notion of "route". Two
/// [`EvidenceKey`]s compare equal only when all four parts match; nothing
/// here collapses a model to itself across two routes.
///
/// [`EvidenceKey::client`] is an **opaque slug**, never a harness identifier:
/// evidence is partitioned by whoever asked, and this side is not told what
/// the parts of that partition mean. `crate::routing::interactive::Assignment`
/// already carried the harness as a slug string for exactly this reason, and
/// this is the same string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceKey {
    client: String,
    launch_profile: String,
    model: AssignedModel,
    route: ServingRoute,
}

impl EvidenceKey {
    pub fn new(
        client: impl Into<String>,
        launch_profile: impl Into<String>,
        model: AssignedModel,
        route: ServingRoute,
    ) -> Self {
        Self {
            client: client.into(),
            launch_profile: launch_profile.into(),
            model,
            route,
        }
    }

    /// Which client's evidence this is, as the opaque slug the caller chose.
    pub fn client(&self) -> &str {
        &self.client
    }

    pub fn launch_profile(&self) -> &str {
        &self.launch_profile
    }

    pub fn model(&self) -> &AssignedModel {
        &self.model
    }

    pub fn route(&self) -> &ServingRoute {
        &self.route
    }
}

/// The caller's own judgement about one candidate, in terms the gateway can
/// honour without knowing what the caller is.
///
/// `preferred` is a **preference, never a filter** — it is worth a decaying
/// prior and nothing else (design decision 1, and map line 566's "never proof
/// of quality or a hard routing requirement"). `reason` is the caller's own
/// sentence, carried through to the routing explanation verbatim so that "why
/// this backend?" is answered in the words of whoever actually knew.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteAffinity {
    preferred: bool,
    reason: String,
}

impl RouteAffinity {
    pub fn new(preferred: bool, reason: impl Into<String>) -> Self {
        Self {
            preferred,
            reason: reason.into(),
        }
    }

    /// What a route the caller said nothing about is worth: no preference,
    /// and an explanation line that says the caller was silent rather than
    /// implying it judged and declined.
    pub fn none() -> Self {
        Self {
            preferred: false,
            reason: "the caller stated no affinity for this route, so no preference is claimed \
                     for it"
                .to_owned(),
        }
    }

    /// Whether the caller would rather this candidate served.
    pub fn preferred(&self) -> bool {
        self.preferred
    }

    /// The caller's own words, for the routing explanation.
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl Default for RouteAffinity {
    fn default() -> Self {
        Self::none()
    }
}

/// The caller's judgements, keyed by what the gateway can already see on a
/// [`crate::routing::Backend`]: its provider and its model.
///
/// Keyed by those two rather than handed in per candidate because the caller
/// resolves them once, at setup, and the ranking that consumes them happens
/// later — at a provider failure the caller is not present for. A route with
/// no entry gets [`RouteAffinity::none`], so an empty value scores every
/// candidate at `0.0` and reproduces "first compatible candidate" exactly.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PairingAffinities {
    by_route: BTreeMap<(String, String), RouteAffinity>,
}

impl PairingAffinities {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record what the caller thinks of the route `provider` serves `model`
    /// on. A second call for the same route replaces the first.
    pub fn set(&mut self, provider: &str, model: &AssignedModel, affinity: RouteAffinity) {
        self.by_route.insert(route_key(provider, model), affinity);
    }

    /// The builder form, for a caller assembling a whole set in one
    /// expression.
    #[must_use]
    pub fn with(mut self, provider: &str, model: &AssignedModel, affinity: RouteAffinity) -> Self {
        self.set(provider, model, affinity);
        self
    }

    /// What the caller said about this route, or [`RouteAffinity::none`] when
    /// it said nothing. Never `None`: a silent caller is an answer.
    pub fn for_route(&self, provider: &str, model: &AssignedModel) -> RouteAffinity {
        self.by_route
            .get(&route_key(provider, model))
            .cloned()
            .unwrap_or_else(RouteAffinity::none)
    }

    pub fn is_empty(&self) -> bool {
        self.by_route.is_empty()
    }

    pub fn len(&self) -> usize {
        self.by_route.len()
    }
}

/// The two strings a route is keyed by. [`AssignedModel::label`] rather than
/// the value itself so the key is printable and so
/// [`AssignedModel::HarnessDefault`] keys as the one thing it is, rather than
/// needing an ordering on a type that has no natural one.
fn route_key(provider: &str, model: &AssignedModel) -> (String, String) {
    (provider.to_owned(), model.label().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_route_nobody_judged_is_not_preferred_and_says_so() {
        let affinities = PairingAffinities::new();
        let affinity = affinities.for_route("openrouter", &AssignedModel::named("the-model"));
        assert!(!affinity.preferred());
        assert!(affinity.reason().contains("no affinity"));
    }

    #[test]
    fn an_affinity_is_found_by_provider_and_model_together() {
        let model = AssignedModel::named("the-model");
        let other = AssignedModel::named("another-model");
        let affinities = PairingAffinities::new().with(
            "openrouter",
            &model,
            RouteAffinity::new(true, "the caller's own words"),
        );

        assert!(affinities.for_route("openrouter", &model).preferred());
        assert_eq!(
            affinities.for_route("openrouter", &model).reason(),
            "the caller's own words"
        );
        assert!(
            !affinities.for_route("nous", &model).preferred(),
            "a different provider is a different route"
        );
        assert!(
            !affinities.for_route("openrouter", &other).preferred(),
            "a different model is a different route"
        );
    }

    #[test]
    fn the_harness_default_model_keys_as_itself() {
        let affinities = PairingAffinities::new().with(
            "openrouter",
            &AssignedModel::HarnessDefault,
            RouteAffinity::new(true, "declared"),
        );
        assert!(
            affinities
                .for_route("openrouter", &AssignedModel::HarnessDefault)
                .preferred()
        );
        assert!(
            !affinities
                .for_route("openrouter", &AssignedModel::named("the-model"))
                .preferred()
        );
    }

    #[test]
    fn an_evidence_key_separates_two_routes_to_the_same_model() {
        let one = EvidenceKey::new(
            "claude-code",
            "default",
            AssignedModel::named("the-model"),
            ServingRoute {
                provider: Some("openrouter".to_owned()),
                gateway: None,
                protocol: Some(WireProtocol::AnthropicMessages),
            },
        );
        let two = EvidenceKey::new(
            "claude-code",
            "default",
            AssignedModel::named("the-model"),
            ServingRoute {
                provider: Some("nous".to_owned()),
                gateway: None,
                protocol: Some(WireProtocol::AnthropicMessages),
            },
        );
        assert_ne!(one, two);
        assert_eq!(one.client(), "claude-code");
    }

    #[test]
    fn an_unknown_protocol_slug_is_none_rather_than_a_guess() {
        assert_eq!(
            wire_protocol_from_slug("anthropic-messages"),
            Some(WireProtocol::AnthropicMessages)
        );
        assert_eq!(wire_protocol_from_slug("something-nobody-ships"), None);
    }
}
