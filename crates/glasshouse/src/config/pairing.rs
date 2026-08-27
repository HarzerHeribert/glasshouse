//! Phase 9J's configuration half: how a person corrects pairing metadata,
//! and what `glasshouse pairing` prints.
//!
//! [`mod@crate::harness::pairing`] is a pure domain model that imports no
//! configuration — the same rule, and the same reason, as
//! [`mod@crate::profile`]. This module is the caller that rule assumes: it
//! reads the layered configuration, resolves providers and launch profiles
//! into [`crate::harness::pairing::PairingQuery`] values, asks
//! [`crate::harness::pairing::classify`], and renders the answers.
//!
//! # Why the report lives here and not in `main.rs`
//!
//! Because a caller only the binary can reach is a caller no test enters
//! through, and a capability proven by tests that all set the world up
//! themselves is proven against a build whose production path could be
//! deleted. [`report`] is what `main.rs`'s `pairing` arm calls, in one line,
//! and it is what `tests/pairing.rs` calls too — so a mutation to the
//! resolution below is a mutation to the path the shipped binary runs.
//!
//! # What a correction may and may not do
//!
//! A correction sets *metadata*: who developed a model, what family it
//! belongs to, what a harness vendor officially supports, and what a person
//! has actually observed about a model's behaviour. It cannot set the pairing
//! **class** directly. The class is always derived, so that "why does this
//! say vendor-native" always has an answer made of things somebody declared —
//! which is the whole point of a taxonomy whose top rung is a claim about a
//! first-party relationship.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::harness::pairing::{
    self, ModelBehaviourFit, ModelCorrection, ModelDeveloper, PairingOverrides, PairingQuery,
    ServingRoute, SupportCorrection,
};
use crate::harness::{Declared, WireProtocol};
use crate::integrations::IntegrationId;
use crate::profile::BackendResource;
use crate::routing::AssignedModel;

use super::{EffectiveConfig, Layer};

/// One `[pairing.models."<id>"]` table: a correction to what Glasshouse
/// believes about one model.
///
/// Every field is optional and corrects only what it names — a user fixing a
/// wrong family should not have to restate a developer that was already
/// right.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingModelOverride {
    /// A developer slug. Free text on purpose: line 561 requires a
    /// correction to be possible without changing router code, and an
    /// enumeration of organisations would make an unfamiliar developer a
    /// code change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    developer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    family: Option<String>,
    /// `verified`, `unverified` or `known-absent` — see
    /// [`ModelBehaviourFit`]. A value this build does not understand is
    /// ignored rather than refused, the same way a stale free-resource pin
    /// degrades visibly instead of stopping Glasshouse from loading.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    behaviour: Option<String>,
}

impl PairingModelOverride {
    pub fn developer(&self) -> Option<&str> {
        self.developer.as_deref()
    }

    pub fn set_developer(&mut self, developer: Option<String>) -> &mut Self {
        self.developer = developer;
        self
    }

    pub fn family(&self) -> Option<&str> {
        self.family.as_deref()
    }

    pub fn set_family(&mut self, family: Option<String>) -> &mut Self {
        self.family = family;
        self
    }

    pub fn behaviour(&self) -> Option<&str> {
        self.behaviour.as_deref()
    }

    pub fn set_behaviour(&mut self, behaviour: Option<String>) -> &mut Self {
        self.behaviour = behaviour;
        self
    }

    /// This entry as the domain model's own correction type.
    ///
    /// A `developer` that is present but empty clears the attribution back to
    /// [`ModelDeveloper::Unknown`], which is a correction a person may
    /// legitimately want to make: Glasshouse got it wrong, and unknown is
    /// better than wrong.
    fn to_correction(&self) -> ModelCorrection {
        ModelCorrection {
            developer: self.developer.as_deref().map(|slug| {
                if slug.trim().is_empty() {
                    ModelDeveloper::Unknown
                } else {
                    ModelDeveloper::named(slug.trim())
                }
            }),
            family: self.family.clone(),
            behaviour: self
                .behaviour
                .as_deref()
                .and_then(ModelBehaviourFit::from_slug),
        }
    }
}

/// One `[pairing.harnesses.<slug>]` table: a correction to what a harness
/// vendor is recorded as officially supporting.
///
/// The case it exists for is a harness announcing support between Glasshouse
/// releases. For an adapter, adding support is already a metadata edit —
/// line 562 — and this is the same edit for a person who cannot wait for the
/// release that carries it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingHarnessOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    native_families: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    supported_models: Option<Vec<String>>,
}

impl PairingHarnessOverride {
    pub fn native_families(&self) -> Option<&[String]> {
        self.native_families.as_deref()
    }

    pub fn set_native_families(&mut self, families: Option<Vec<String>>) -> &mut Self {
        self.native_families = families;
        self
    }

    pub fn supported_models(&self) -> Option<&[String]> {
        self.supported_models.as_deref()
    }

    pub fn set_supported_models(&mut self, models: Option<Vec<String>>) -> &mut Self {
        self.supported_models = models;
        self
    }

    fn to_correction(&self) -> SupportCorrection {
        SupportCorrection {
            native_families: self.native_families.clone(),
            supported_models: self.supported_models.clone(),
        }
    }
}

/// The `[pairing]` table of one configuration layer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingConfig {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    models: BTreeMap<String, PairingModelOverride>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    harnesses: BTreeMap<String, PairingHarnessOverride>,
}

impl PairingConfig {
    /// Whether this layer has recorded nothing, so a configuration file that
    /// was never asked about pairing carries no `[pairing]` table at all —
    /// the same rule `RoutingConfig::is_unset` follows.
    pub fn is_unset(&self) -> bool {
        self.models.is_empty() && self.harnesses.is_empty()
    }

    pub fn models(&self) -> impl Iterator<Item = (&str, &PairingModelOverride)> {
        self.models.iter().map(|(id, entry)| (id.as_str(), entry))
    }

    pub fn model(&self, id: &str) -> Option<&PairingModelOverride> {
        self.models.get(id)
    }

    pub fn model_entry(&mut self, id: impl Into<String>) -> &mut PairingModelOverride {
        self.models.entry(id.into()).or_default()
    }

    pub fn remove_model(&mut self, id: &str) -> Option<PairingModelOverride> {
        self.models.remove(id)
    }

    pub fn harnesses(&self) -> impl Iterator<Item = (&str, &PairingHarnessOverride)> {
        self.harnesses
            .iter()
            .map(|(slug, entry)| (slug.as_str(), entry))
    }

    pub fn harness(&self, id: IntegrationId) -> Option<&PairingHarnessOverride> {
        self.harnesses.get(id.slug())
    }

    pub fn harness_entry(&mut self, id: IntegrationId) -> &mut PairingHarnessOverride {
        self.harnesses.entry(id.slug().to_owned()).or_default()
    }

    pub fn remove_harness(&mut self, id: IntegrationId) -> Option<PairingHarnessOverride> {
        self.harnesses.remove(id.slug())
    }
}

impl EffectiveConfig<'_> {
    /// Every pairing correction in effect, with the layers they came from
    /// named.
    ///
    /// Merged per key rather than per layer: a project that corrects one
    /// model does not discard a user's corrections to every other one. Where
    /// both layers name the same key the project's wins, matching every other
    /// lookup on [`EffectiveConfig`] except `bypass_acknowledged`, which is a
    /// safety attestation and is not this.
    pub fn pairing_overrides(&self) -> PairingOverrides {
        let mut models: BTreeMap<String, ModelCorrection> = BTreeMap::new();
        let mut harnesses: BTreeMap<String, SupportCorrection> = BTreeMap::new();
        let mut layers: Vec<&str> = Vec::new();

        if !self.user.pairing().is_unset() {
            layers.push("the user configuration file");
            for (id, entry) in self.user.pairing().models() {
                models.insert(id.to_owned(), entry.to_correction());
            }
            for (slug, entry) in self.user.pairing().harnesses() {
                harnesses.insert(slug.to_owned(), entry.to_correction());
            }
        }
        if let Some(project) = self.project
            && !project.pairing().is_unset()
        {
            layers.push("this project's configuration file");
            for (id, entry) in project.pairing().models() {
                models.insert(id.to_owned(), entry.to_correction());
            }
            for (slug, entry) in project.pairing().harnesses() {
                harnesses.insert(slug.to_owned(), entry.to_correction());
            }
        }

        let source = match layers.as_slice() {
            [] => "no configuration file".to_owned(),
            [one] => (*one).to_owned(),
            many => many.join(" and "),
        };
        PairingOverrides::from_parts(source, models, harnesses)
    }

    /// One pairing question per configured launch profile.
    ///
    /// The implied Native profile is deliberately not here. It exists for
    /// every harness and names no model and no provider, so it would produce
    /// one identical "nothing was assigned, so nothing is known" row per
    /// harness — noise standing in front of the rows a user configured. A
    /// person who wants that answer asks for it with `--model`.
    pub fn pairing_queries(&self) -> Vec<ConfiguredPairing> {
        let mut names: BTreeMap<String, Layer> = BTreeMap::new();
        for (name, _) in self.user.profiles().iter() {
            names.insert(name.to_owned(), Layer::User);
        }
        if let Some(project) = self.project {
            for (name, _) in project.profiles().iter() {
                names.insert(name.to_owned(), Layer::Project);
            }
        }

        names
            .into_iter()
            .map(|(name, layer)| self.pairing_for_profile(&name, layer))
            .collect()
    }

    fn pairing_for_profile(&self, name: &str, layer: Layer) -> ConfiguredPairing {
        let config = match layer {
            Layer::Project => self.project.and_then(|p| p.profiles().get(name)),
            Layer::User | Layer::Default => self.user.profiles().get(name),
        };
        let Some(config) = config else {
            return ConfiguredPairing::unresolved(
                name,
                layer,
                "the profile disappeared".to_owned(),
            );
        };
        let profile = match config.to_launch_profile(name) {
            Ok(profile) => profile,
            Err(err) => return ConfiguredPairing::unresolved(name, layer, err.to_string()),
        };

        let model = match &profile.model {
            Some(model) => AssignedModel::named(model),
            None => AssignedModel::HarnessDefault,
        };

        let mut route = ServingRoute {
            provider: None,
            gateway: None,
            protocol: profile.expected_protocol,
        };
        let mut tool_calls = Declared::Unverified;
        let mut provider_protocols: Vec<WireProtocol> = Vec::new();
        let mut note = None;

        match &profile.backend {
            BackendResource::Native => {
                // A Native profile runs on the harness vendor's own service,
                // over the harness's own wire. Taking the protocol from the
                // adapter's own declaration is reading what it said, not
                // inferring: it is used only when the profile itself names
                // none, and only when the adapter declares exactly one.
                if route.protocol.is_none() {
                    route.protocol = sole_declared_protocol(profile.harness);
                }
            }
            BackendResource::DirectProvider { provider } => {
                route.provider = Some(provider.clone());
                match self.configured_provider(provider) {
                    Ok(resolved) => {
                        let resolved = resolved.value;
                        provider_protocols = resolved
                            .protocols
                            .iter()
                            .filter(|support| !support.base_url.is_empty())
                            .map(|support| support.protocol)
                            .collect();
                        if route.protocol.is_none() && provider_protocols.len() == 1 {
                            route.protocol = Some(provider_protocols[0]);
                        }
                        if let Some(protocol) = route.protocol
                            && let Some(support) = resolved.serves(protocol)
                        {
                            tool_calls = support.tool_calls;
                        }
                    }
                    Err(err) => note = Some(err.to_string()),
                }
            }
            BackendResource::GlasshouseGateway => {
                route.gateway = Some("the Glasshouse gateway".to_owned());
                note = Some(
                    "a gateway-backed profile is assigned its provider when the session starts, \
                     so the serving provider is not known here"
                        .to_owned(),
                );
            }
        }

        let query = PairingQuery {
            harness: profile.harness,
            model,
            route,
            tool_calls,
            provider_protocols,
        };
        ConfiguredPairing {
            name: name.to_owned(),
            layer,
            query: Some(query),
            note,
        }
    }
}

/// One configured launch profile, turned into a pairing question.
#[derive(Debug, Clone)]
pub struct ConfiguredPairing {
    name: String,
    layer: Layer,
    /// `None` when the profile could not be resolved at all — a harness or
    /// protocol name this build does not know.
    query: Option<PairingQuery>,
    /// Anything the reader needs to know about how far resolution got.
    note: Option<String>,
}

impl ConfiguredPairing {
    fn unresolved(name: &str, layer: Layer, note: String) -> Self {
        Self {
            name: name.to_owned(),
            layer,
            query: None,
            note: Some(note),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn layer(&self) -> Layer {
        self.layer
    }

    pub fn query(&self) -> Option<&PairingQuery> {
        self.query.as_ref()
    }

    pub fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }
}

/// The one protocol `harness` declares it speaks, when it declares exactly
/// one.
fn sole_declared_protocol(harness: IntegrationId) -> Option<WireProtocol> {
    let declared = crate::harness::adapter_for(harness)?
        .describe()
        .backends
        .protocols;
    match declared.value().map(|protocols| &protocols[..]) {
        Some([only]) => Some(*only),
        _ => None,
    }
}

fn layer_name(layer: Layer) -> &'static str {
    match layer {
        Layer::Project => "this project's configuration",
        Layer::User => "the user configuration",
        Layer::Default => "a Glasshouse default",
    }
}

/// What `glasshouse pairing` prints.
///
/// The production caller of [`crate::harness::pairing::classify`], and the
/// function `main.rs`'s `pairing` arm calls. `model` and `harness` are the
/// command's two optional arguments.
pub fn report(
    effective: &EffectiveConfig<'_>,
    model: Option<&str>,
    harness: Option<&str>,
) -> String {
    use std::fmt::Write as _;

    let overrides = effective.pairing_overrides();
    let mut out = String::new();

    let _ = writeln!(out, "Glasshouse harness-model pairing");
    let _ = writeln!(out, "================================");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "Who publishes a harness, who developed a model, and who serves it are three \
         different\nquestions. Glasshouse keeps them apart, and says `unknown` rather than \
         reading an\nanswer out of a name."
    );
    let _ = writeln!(out);

    let _ = writeln!(out, "Harness pairing metadata");
    for adapter in crate::harness::all() {
        write_harness_metadata(&mut out, adapter, &overrides);
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "Pairing corrections in effect");
    write_corrections(&mut out, effective);
    let _ = writeln!(out);

    let requested_harness = harness.map(|slug| {
        IntegrationId::ALL
            .iter()
            .copied()
            .find(|id| id.slug() == slug)
            .ok_or_else(|| slug.to_owned())
    });

    if let Some(model) = model {
        let _ = writeln!(out, "Model `{model}`");
        match &requested_harness {
            Some(Err(slug)) => {
                let _ = writeln!(
                    out,
                    "  `{slug}` is not a harness Glasshouse knows; valid names are: {}",
                    crate::harness::all()
                        .map(|adapter| adapter.id().slug())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            Some(Ok(id)) => write_ad_hoc(&mut out, *id, model, &overrides),
            None => {
                for adapter in crate::harness::all() {
                    write_ad_hoc(&mut out, adapter.id(), model, &overrides);
                }
            }
        }
        let _ = writeln!(out);
    }

    let _ = writeln!(out, "Configured launch profiles");
    let configured = effective.pairing_queries();
    if configured.is_empty() {
        let _ = writeln!(
            out,
            "  (none configured) — a launch profile is what names a harness, a provider and a \
             model\n  together, so it is what a pairing can be reported for. Ask about one \
             model with\n  `glasshouse pairing --model <id>`."
        );
    } else {
        for entry in &configured {
            write_configured(&mut out, entry, &overrides);
        }
    }

    out
}

fn write_harness_metadata(
    out: &mut String,
    adapter: &'static dyn crate::harness::HarnessAdapter,
    overrides: &PairingOverrides,
) {
    use std::fmt::Write as _;

    let id = adapter.id();
    let vendor = adapter.describe().vendor;
    let _ = writeln!(
        out,
        "  {} — publisher: {}",
        id.display_name(),
        match vendor.value() {
            Some(vendor) => vendor.display_name(),
            None => "unverified",
        }
    );
    let support = adapter.official_model_support();
    for (label, declared) in [
        (
            "native families ",
            (
                support
                    .native_families
                    .value()
                    .map(|families| families.join(", ")),
                support.native_families.evidence(),
            ),
        ),
        (
            "supported models",
            (
                support
                    .supported_models
                    .value()
                    .map(|models| models.join(", ")),
                support.supported_models.evidence(),
            ),
        ),
    ] {
        match declared {
            (Some(value), evidence) => {
                let value = if value.is_empty() {
                    "(declared empty)".to_owned()
                } else {
                    value
                };
                let _ = writeln!(out, "    {label}: {value}");
                if let Some(evidence) = evidence {
                    let _ = writeln!(out, "      evidence: {evidence}");
                }
            }
            (None, _) => {
                let _ = writeln!(out, "    {label}: unverified — nobody read this list");
            }
        }
    }
    if let Some(correction) = overrides.harness(id) {
        if let Some(families) = &correction.native_families {
            let _ = writeln!(
                out,
                "    corrected native families: {}",
                if families.is_empty() {
                    "(none)".to_owned()
                } else {
                    families.join(", ")
                }
            );
        }
        if let Some(models) = &correction.supported_models {
            let _ = writeln!(
                out,
                "    corrected supported models: {}",
                if models.is_empty() {
                    "(none)".to_owned()
                } else {
                    models.join(", ")
                }
            );
        }
    }
}

fn write_corrections(out: &mut String, effective: &EffectiveConfig<'_>) {
    use std::fmt::Write as _;

    let mut any = false;
    for (layer, config) in effective.pairing_layers() {
        for (id, entry) in config.models() {
            any = true;
            let _ = writeln!(
                out,
                "  model `{id}` ({}): developer={} family={} behaviour={}",
                layer_name(layer),
                entry.developer().unwrap_or("(unchanged)"),
                entry.family().unwrap_or("(unchanged)"),
                entry.behaviour().unwrap_or("(unchanged)")
            );
        }
        for (slug, entry) in config.harnesses() {
            any = true;
            let _ = writeln!(
                out,
                "  harness `{slug}` ({}): native families={} supported models={}",
                layer_name(layer),
                entry
                    .native_families()
                    .map(|f| f.join(", "))
                    .unwrap_or_else(|| "(unchanged)".to_owned()),
                entry
                    .supported_models()
                    .map(|m| m.join(", "))
                    .unwrap_or_else(|| "(unchanged)".to_owned()),
            );
        }
    }
    if !any {
        let _ = writeln!(
            out,
            "  (none) — correct one with a `[pairing.models.\"<model id>\"]` table in the \
             configuration\n  file, giving `developer`, `family`, or `behaviour`."
        );
    }
}

fn write_ad_hoc(
    out: &mut String,
    harness: IntegrationId,
    model: &str,
    overrides: &PairingOverrides,
) {
    use std::fmt::Write as _;

    let query = PairingQuery {
        harness,
        model: AssignedModel::named(model),
        route: ServingRoute::default(),
        tool_calls: Declared::Unverified,
        provider_protocols: Vec::new(),
    };
    let pairing = pairing::classify(&query, overrides);
    let _ = writeln!(
        out,
        "  in {}: {} ({})",
        harness.display_name(),
        pairing.class(),
        pairing.reason()
    );
}

fn write_configured(out: &mut String, entry: &ConfiguredPairing, overrides: &PairingOverrides) {
    use std::fmt::Write as _;

    let _ = writeln!(
        out,
        "  profile `{}` (from {})",
        entry.name(),
        layer_name(entry.layer())
    );
    let Some(query) = entry.query() else {
        let _ = writeln!(
            out,
            "    unresolved: {}",
            entry.note().unwrap_or("no reason recorded")
        );
        return;
    };

    let pairing = pairing::classify(query, overrides);
    let row = |out: &mut String, label: &str, value: &str| {
        let _ = writeln!(out, "    {label:<18}{value}");
    };

    row(
        out,
        "harness:",
        &format!(
            "{} (publisher {})",
            pairing.harness().display_name(),
            pairing
                .harness_vendor()
                .value()
                .map(|vendor| vendor.display_name())
                .unwrap_or("unverified")
        ),
    );
    row(out, "model:", pairing.model().label());
    row(out, "developer:", pairing.developer().label());
    row(out, "family:", pairing.family().unwrap_or("unknown"));
    row(
        out,
        "serving provider:",
        pairing
            .route()
            .provider
            .as_deref()
            .unwrap_or("the harness's own first-party service"),
    );
    row(
        out,
        "gateway:",
        pairing.route().gateway.as_deref().unwrap_or("none"),
    );
    row(
        out,
        "wire protocol:",
        &pairing
            .route()
            .protocol
            .map(|protocol| protocol.slug().to_owned())
            .unwrap_or_else(|| "unknown".to_owned()),
    );
    row(out, "pairing class:", pairing.class().slug());
    row(out, "protocol fit:", pairing.protocol_fit().slug());
    row(out, "model behaviour:", pairing.model_behaviour().slug());
    row(
        out,
        "tool semantics:",
        match pairing.tool_semantics() {
            crate::routing::ToolSemantics::Verified => "verified",
            crate::routing::ToolSemantics::Unverified => "unverified",
            crate::routing::ToolSemantics::KnownAbsent => "known absent",
        },
    );
    row(
        out,
        "attribution:",
        &pairing.attribution().source.describe(),
    );
    row(out, "why:", pairing.reason());
    if let Some(note) = entry.note() {
        row(out, "note:", note);
    }
}
