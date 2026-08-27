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
    self, ModelBehaviourFit, ModelCorrection, ModelDeveloper, PairingQuery, ServingRoute,
    SupportCorrection,
};
// `pub use`, not a plain import: `crate::gateway::session` must never name
// `crate::harness` at all — see that module's own header and
// `gateway::tests::the_gateway_imports_none_of_the_modules_that_would_make_it_a_harness`
// — so it reaches this type through `crate::config::pairing::PairingOverrides`
// instead of importing it directly.
pub use crate::harness::pairing::PairingOverrides;
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

/// Line 576 and Phase 49's line 1797: how strongly a user wants a
/// vendor-native pairing preferred, as a configuration value a policy reads —
/// never as a vendor name a policy branches on.
///
/// Four values, and the fourth is not a strength. `Strong`, `Weak` and `Off`
/// scale [`native_pairing_prior_contribution`]'s magnitude; `Pin` does not
/// convert to a magnitude at all — see [`PairingPreference::strength`] — it
/// is the one value the map allows to behave like a hard rule, because the
/// user asked for it by name for an explicitly chosen session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingPreference {
    Strong,
    Weak,
    Off,
    Pin,
}

impl Default for PairingPreference {
    /// [`EffectiveConfig::native_pairing_preference`]'s own out-of-the-box
    /// answer when nothing is configured — see that method's doc comment.
    /// Kept here, next to the type, so a caller that needs "no preference
    /// resolved yet" (Phase 9J line 576's own patch) gets the same default
    /// the configuration layer would have, rather than a second place this
    /// could drift from it.
    fn default() -> Self {
        Self::Strong
    }
}

impl PairingPreference {
    pub fn slug(self) -> &'static str {
        match self {
            Self::Strong => "strong",
            Self::Weak => "weak",
            Self::Off => "off",
            Self::Pin => "pin",
        }
    }

    /// Parse the spelling a configuration file uses, or `None`. A value this
    /// build does not understand is ignored rather than refused — the same
    /// visible-degradation rule [`ModelBehaviourFit::from_slug`] follows.
    pub fn from_slug(slug: &str) -> Option<Self> {
        match slug {
            "strong" => Some(Self::Strong),
            "weak" => Some(Self::Weak),
            "off" => Some(Self::Off),
            "pin" => Some(Self::Pin),
            _ => None,
        }
    }

    /// This value's strength, when it has one.
    ///
    /// `Pin` returns `None` on purpose: [`native_pairing_prior_contribution`]
    /// takes a [`PriorStrength`], not a [`PairingPreference`], so a caller
    /// cannot pass a pin into the additive scorer even by mistake — it has to
    /// notice the `None` and apply the pin as the hard rule it is, before any
    /// scoring runs. That is design decision 7 made structural rather than a
    /// convention a later edit could quietly break.
    pub fn strength(self) -> Option<PriorStrength> {
        match self {
            Self::Strong => Some(PriorStrength::Strong),
            Self::Weak => Some(PriorStrength::Weak),
            Self::Off => Some(PriorStrength::Off),
            Self::Pin => None,
        }
    }
}

impl std::fmt::Display for PairingPreference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.slug())
    }
}

/// [`PairingPreference`] with `Pin` removed — the type
/// [`native_pairing_prior_contribution`] actually accepts, so that the one
/// value that must never be scored cannot type-check as an argument to the
/// function that scores.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriorStrength {
    Strong,
    Weak,
    Off,
}

impl PriorStrength {
    /// The prior's magnitude at zero reliable observations, before decay.
    /// Bounded and small relative to what a handful of real observations can
    /// contribute (see [`evidence_signal`]) — design decision 1: a prior a
    /// sufficiently strong observation can always outrank.
    fn base_magnitude(self) -> f64 {
        match self {
            Self::Strong => 1.0,
            Self::Weak => 0.4,
            Self::Off => 0.0,
        }
    }
}

/// Reliable local observations after which the native-pairing prior
/// contributes exactly nothing.
///
/// Design decision 4: the prior decays to zero, not to a floor. A count at or
/// past this contributes `0.0` exactly — not merely small — which is what
/// makes it possible to write a test that asserts zero rather than "smaller
/// than before".
const FULL_DECAY_OBSERVATIONS: usize = 20;

/// The prior's decay factor at `count` reliable observations: `1.0` at zero,
/// linear down to exactly `0.0` at [`FULL_DECAY_OBSERVATIONS`] and beyond.
fn decay_factor(count: usize) -> f64 {
    if count >= FULL_DECAY_OBSERVATIONS {
        0.0
    } else {
        1.0 - (count as f64 / FULL_DECAY_OBSERVATIONS as f64)
    }
}

/// What local observation has established about one [`pairing::EvidenceKey`],
/// if anything.
///
/// Line 571 names five kinds of evidence and a user override; this is that
/// list, reduced to a bounded summary rather than one opaque score. Every
/// field is independent and optional on purpose — lines 573 and 574 each need
/// exactly one component to move while the others say nothing, and a single
/// pre-blended number could not be driven that way by a test.
///
/// **No production source of this exists.** Phase 33A (the routing evidence
/// ledger) is 0 of 15 and unbuilt; this struct is what it would eventually
/// fill in, and today only a test double ever constructs one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ObservedEvidence {
    /// How many reliable observations back the rest of this struct. Also
    /// what the native-pairing prior decays against — the same count answers
    /// "how much do we trust this" for both the prior's decay and the
    /// evidence signal's confidence.
    pub reliable_observation_count: usize,
    /// `0.0..=1.0`. Higher is better.
    pub task_success_rate: Option<f64>,
    /// `0.0..=1.0`. Higher is better.
    pub usable_tool_call_rate: Option<f64>,
    /// `0.0..=1.0`. Lower is better — a repair is a correction the harness
    /// needed after the model's own turn.
    pub repair_rate: Option<f64>,
    /// Effective time-to-first-content, as a ratio to a baseline pairing.
    /// Below `1.0` is faster than the baseline; above is slower.
    pub effective_ttfc_ratio: Option<f64>,
    /// `0.0..=1.0`. Higher is better.
    pub reliability: Option<f64>,
    /// An explicit user override, `-1.0..=1.0`: negative is "I moved away
    /// from this pairing", positive is "I chose it and kept it".
    pub user_override_signal: Option<f64>,
}

impl ObservedEvidence {
    /// No local evidence at all — the state every pairing starts in before
    /// Phase 33A ever records anything for it.
    pub fn none() -> Self {
        Self {
            reliable_observation_count: 0,
            task_success_rate: None,
            usable_tool_call_rate: None,
            repair_rate: None,
            effective_ttfc_ratio: None,
            reliability: None,
            user_override_signal: None,
        }
    }
}

/// A source of local observations for one evidence key.
///
/// A trait rather than a concrete store, on purpose: Phase 33A (the routing
/// evidence ledger this would eventually read) does not exist, verified by
/// `grep -rn 'fn score\|Score' crates/glasshouse/src` finding no match and by
/// `docs/product/evidence/phase-9j.md`'s own account of the two routing
/// callers, neither of which ranks anything. Scoring against a trait means
/// the policy below compiles and is provable with a test double today, and
/// gets a real implementation the day Phase 33A lands — without this file
/// changing.
pub trait ObservationSource {
    /// What has been observed for exactly this evidence key, or `None` when
    /// nothing has.
    fn observed(&self, key: &pairing::EvidenceKey) -> Option<ObservedEvidence>;
}

/// An [`ObservationSource`] that answers from nothing, for a caller with no
/// evidence store yet. Every pairing prior computed against this decays
/// exactly like a fresh session's, because a fresh session is exactly what
/// this represents.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoObservations;

impl ObservationSource for NoObservations {
    fn observed(&self, _key: &pairing::EvidenceKey) -> Option<ObservedEvidence> {
        None
    }
}

/// How many reliable observations it takes for [`evidence_signal`] to speak
/// at full confidence. Below this, a real but thin observation record still
/// contributes — scaled down, never zeroed — because "no evidence yet" and
/// "one data point" must not read identically to a routing explanation.
const CONFIDENT_AT_OBSERVATIONS: f64 = 5.0;

/// Reduce one [`ObservedEvidence`] to a single signed number: positive means
/// the observations support this pairing, negative means they contradict it.
///
/// Unbounded, deliberately, unlike [`PriorStrength::base_magnitude`] — this
/// is design decision 1's escape hatch. A handful of real observations must
/// be able to outrank the prior's bounded maximum, and a signal that saturated
/// at the same ceiling as the prior could never do that no matter how bad or
/// good the evidence was.
fn evidence_signal(observed: &ObservedEvidence) -> f64 {
    let mut signal = 0.0;
    if let Some(rate) = observed.task_success_rate {
        signal += (rate - 0.5) * 2.0;
    }
    if let Some(rate) = observed.usable_tool_call_rate {
        signal += (rate - 0.5) * 2.0;
    }
    if let Some(rate) = observed.repair_rate {
        // Lower is better, so the sign flips relative to the rates above.
        signal += (0.5 - rate) * 2.0;
    }
    if let Some(ratio) = observed.effective_ttfc_ratio {
        signal += (1.0 - ratio).clamp(-1.0, 1.0);
    }
    if let Some(rate) = observed.reliability {
        signal += (rate - 0.5) * 2.0;
    }
    if let Some(override_signal) = observed.user_override_signal {
        signal += override_signal;
    }
    let confidence =
        (observed.reliable_observation_count as f64 / CONFIDENT_AT_OBSERVATIONS).min(1.0);
    signal * confidence
}

/// A sentence naming which of [`ObservedEvidence`]'s components were
/// actually established, for a routing explanation's evidence text.
fn describe_observed(observed: &ObservedEvidence) -> String {
    let mut parts = Vec::new();
    if let Some(v) = observed.task_success_rate {
        parts.push(format!("task success {v:.2}"));
    }
    if let Some(v) = observed.usable_tool_call_rate {
        parts.push(format!("usable tool calls {v:.2}"));
    }
    if let Some(v) = observed.repair_rate {
        parts.push(format!("repair rate {v:.2}"));
    }
    if let Some(v) = observed.effective_ttfc_ratio {
        parts.push(format!("effective TTFC {v:.2}x baseline"));
    }
    if let Some(v) = observed.reliability {
        parts.push(format!("reliability {v:.2}"));
    }
    if let Some(v) = observed.user_override_signal {
        parts.push(format!("user override signal {v:.2}"));
    }
    if parts.is_empty() {
        format!(
            "{} reliable observation(s), no component established",
            observed.reliable_observation_count
        )
    } else {
        format!(
            "{} reliable observation(s): {}",
            observed.reliable_observation_count,
            parts.join(", ")
        )
    }
}

/// Line 566 through 575, as one function: what the native-pairing prior and
/// the local evidence for `key` contribute to routing `candidate`.
///
/// `candidate` must already have survived every hard protocol, tool,
/// capability, privacy and user constraint — [`crate::routing::EligibleCandidate`]
/// is what makes that structural (design decision 2) rather than a comment
/// asking a future caller to remember the order.
///
/// The explanation always carries the pairing class and the evidence count
/// (line 575's first two terms, magnitude `0.0`, informational), then either:
/// - a `pinned` line, when `preference` is [`PairingPreference::Pin`] — the
///   prior is not scored at all, because a pin is a hard rule; or
/// - a `native-pairing prior` contribution (line 575's third term), zero
///   unless the pairing is vendor-native, decayed toward zero as reliable
///   observations accumulate; plus
/// - a `local observed evidence` contribution, present only when at least one
///   reliable observation exists, and unbounded — so a strong enough
///   observation can always outrank the prior (design decision 1), and enough
///   bad observations against a vendor-native pairing can make its total
///   lower than a neutral candidate's (line 574).
///
/// **The production caller is `InteractiveRouting::on_provider_failure`**,
/// by way of its own `score_candidate` helper, reached from
/// `crate::gateway::session::SessionRouting::observe_exchange`. `preference`
/// and `evidence` both come from that caller now — see
/// `SessionRouting::set_pairing_preference`, called beside `Self::bind` by
/// `crate::profile`'s gateway path, for where `preference` is actually
/// resolved from configuration. `DisposableRouting` still does not rank
/// candidates at all (Phase 9J line 566 needs a different caller for that,
/// per this package's report).
pub fn native_pairing_prior_contribution(
    candidate: &crate::routing::EligibleCandidate<pairing::Pairing>,
    key: &pairing::EvidenceKey,
    preference: PairingPreference,
    evidence: &dyn ObservationSource,
) -> crate::routing::RoutingExplanation {
    use crate::routing::Contribution;

    let pairing = candidate.value();
    let observed = evidence.observed(key);
    let count = observed
        .as_ref()
        .map(|o| o.reliable_observation_count)
        .unwrap_or(0);

    let mut explanation = crate::routing::RoutingExplanation::new();
    explanation.push(Contribution::new(
        "pairing class",
        0.0,
        format!("{} — {}", pairing.class(), pairing.reason()),
    ));
    explanation.push(Contribution::new(
        "local evidence strength",
        0.0,
        format!(
            "{count} reliable observation(s) for this exact harness, launch profile, model and \
             backend combination"
        ),
    ));

    let Some(strength) = preference.strength() else {
        explanation.push(Contribution::new(
            "native-pairing preference",
            0.0,
            "pinned: this session was explicitly chosen, so the preference is applied as a hard \
             rule before scoring rather than as a prior contribution"
                .to_owned(),
        ));
        return explanation;
    };

    let is_native = pairing.class().is_vendor_native();
    let magnitude = if is_native {
        strength.base_magnitude() * decay_factor(count)
    } else {
        0.0
    };
    explanation.push(Contribution::new(
        "native-pairing prior",
        magnitude,
        format!(
            "{preference} preference, {} vendor-native, decayed for {count} reliable \
             observation(s) (fully decayed at {FULL_DECAY_OBSERVATIONS})",
            if is_native { "is" } else { "is not" }
        ),
    ));

    if let Some(observed) = observed
        && observed.reliable_observation_count > 0
    {
        explanation.push(Contribution::new(
            "local observed evidence",
            evidence_signal(&observed),
            describe_observed(&observed),
        ));
    }

    explanation
}

/// The `[pairing]` table of one configuration layer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingConfig {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    models: BTreeMap<String, PairingModelOverride>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    harnesses: BTreeMap<String, PairingHarnessOverride>,
    /// `strong`, `weak`, `off` or `pin` — see [`PairingPreference`]. A value
    /// this build does not understand is ignored rather than refused, the
    /// same visible-degradation rule every other field in this file follows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    native_pairing_preference: Option<String>,
}

impl PairingConfig {
    /// Whether this layer has recorded nothing, so a configuration file that
    /// was never asked about pairing carries no `[pairing]` table at all —
    /// the same rule `RoutingConfig::is_unset` follows.
    pub fn is_unset(&self) -> bool {
        self.models.is_empty()
            && self.harnesses.is_empty()
            && self.native_pairing_preference.is_none()
    }

    /// This layer's own native-pairing preference, parsed — `None` when
    /// unset or when the layer names a spelling this build does not
    /// recognise.
    pub fn native_pairing_preference(&self) -> Option<PairingPreference> {
        self.native_pairing_preference_raw()
            .and_then(PairingPreference::from_slug)
    }

    /// The spelling this layer actually stored, parsed or not.
    ///
    /// [`PairingConfig::native_pairing_preference`] cannot distinguish "unset"
    /// from "set to something this build cannot use", and the difference is
    /// the whole content of the visible-degradation rule this file follows:
    /// every other field here shows an unrecognised value back to the user
    /// (`behaviour=nonsense`) rather than swallowing it. A resolver that
    /// reported a misspelled preference as *nothing configured* would be
    /// telling the user their file is empty when it is not.
    pub fn native_pairing_preference_raw(&self) -> Option<&str> {
        self.native_pairing_preference.as_deref()
    }

    pub fn set_native_pairing_preference(
        &mut self,
        preference: Option<PairingPreference>,
    ) -> &mut Self {
        self.native_pairing_preference = preference.map(|p| p.slug().to_owned());
        self
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

    /// The native-pairing preference in effect, project-over-user like every
    /// other lookup on [`EffectiveConfig`] except `bypass_acknowledged`, and
    /// the layer it came from for a trace.
    ///
    /// Defaults to [`PairingPreference::Strong`] when nothing is configured
    /// — line 566 asks for "a positive initial routing prior for a fresh
    /// session with little local evidence" as the out-of-the-box behaviour,
    /// not something a user must opt into first.
    /// A spelling this build does not understand is **ignored, and said so**.
    /// Every other field in this module degrades visibly — a bad `behaviour`
    /// prints back as `behaviour=nonsense` — and a preference that fell back
    /// silently while reporting "nothing configured" would be the one field
    /// that lies about the user's own file. The layer and the unusable
    /// spelling both travel with the answer.
    pub fn native_pairing_preference(&self) -> (PairingPreference, String) {
        let layers: [(&str, Option<&PairingConfig>); 2] = [
            (
                "this project's configuration file",
                self.project.map(|p| p.pairing()),
            ),
            ("the user configuration file", Some(self.user.pairing())),
        ];

        let mut ignored = Vec::new();
        for (layer, config) in layers {
            let Some(raw) = config.and_then(PairingConfig::native_pairing_preference_raw) else {
                continue;
            };
            match PairingPreference::from_slug(raw) {
                Some(preference) => {
                    return (preference, describe_preference_source(layer, &ignored));
                }
                None => ignored.push(format!("{layer} set `{raw}`")),
            }
        }

        (
            PairingPreference::Strong,
            describe_preference_source("the default — nothing configured", &ignored),
        )
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
/// Where a resolved native-pairing preference came from, naming any layer
/// whose spelling had to be ignored on the way.
///
/// Separate from the resolver so the sentence is written once: the ignored
/// list is almost always empty, and when it is not, it is the more important
/// half of the answer.
fn describe_preference_source(source: &str, ignored: &[String]) -> String {
    if ignored.is_empty() {
        return source.to_owned();
    }
    format!(
        "{source}; ignoring {} — not one of strong, weak, off, pin",
        ignored.join(" and ")
    )
}

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

    let (preference, source) = effective.native_pairing_preference();
    let _ = writeln!(
        out,
        "Native-pairing preference: {preference} (from {source})"
    );
    let _ = writeln!(
        out,
        "  no router in this build reads this yet — see Phase 35B — but it is stored and \n  layered project-over-user like every other pairing setting, and a user can set it \n  today with `[pairing]\\nnative_pairing_preference = \"strong\" | \"weak\" | \"off\" | \"pin\"`."
    );
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Design decision 7: `Pin` is not a strength, and it must not
    /// type-check where a strength is required. This does not compile if
    /// `strength()` is ever widened to return `Some` for `Pin`, and it is
    /// checked at runtime too, since a `match` arm could still be edited to
    /// agree with a widened signature.
    #[test]
    fn pin_is_the_one_preference_with_no_strength() {
        assert_eq!(
            PairingPreference::Strong.strength(),
            Some(PriorStrength::Strong)
        );
        assert_eq!(
            PairingPreference::Weak.strength(),
            Some(PriorStrength::Weak)
        );
        assert_eq!(PairingPreference::Off.strength(), Some(PriorStrength::Off));
        assert_eq!(PairingPreference::Pin.strength(), None);
    }

    #[test]
    fn preference_slugs_round_trip() {
        for preference in [
            PairingPreference::Strong,
            PairingPreference::Weak,
            PairingPreference::Off,
            PairingPreference::Pin,
        ] {
            assert_eq!(
                PairingPreference::from_slug(preference.slug()),
                Some(preference)
            );
        }
        assert_eq!(PairingPreference::from_slug("aggressive"), None);
    }

    /// Design decision 4: the decay reaches exactly zero at the stated
    /// count, not asymptotically close to it.
    #[test]
    fn the_prior_decays_to_exactly_zero_not_a_floor() {
        assert_eq!(decay_factor(0), 1.0);
        assert!(decay_factor(FULL_DECAY_OBSERVATIONS / 2) > 0.0);
        assert!(decay_factor(FULL_DECAY_OBSERVATIONS / 2) < 1.0);
        assert_eq!(decay_factor(FULL_DECAY_OBSERVATIONS), 0.0);
        assert_eq!(decay_factor(FULL_DECAY_OBSERVATIONS * 10), 0.0);
    }

    /// Bad observations must pull the signal negative, and good ones
    /// positive — line 574 rests on this half existing at all.
    #[test]
    fn evidence_signal_has_both_signs() {
        let mut good = ObservedEvidence::none();
        good.reliable_observation_count = 20;
        good.task_success_rate = Some(1.0);
        good.reliability = Some(1.0);
        assert!(evidence_signal(&good) > 0.0);

        let mut bad = ObservedEvidence::none();
        bad.reliable_observation_count = 20;
        bad.task_success_rate = Some(0.0);
        bad.reliability = Some(0.0);
        assert!(evidence_signal(&bad) < 0.0);
    }

    /// A single observation must not speak as loudly as twenty. Confidence
    /// scales with the count, or a routing explanation would treat one data
    /// point as settled fact.
    #[test]
    fn evidence_signal_scales_with_how_many_observations_back_it() {
        let mut thin = ObservedEvidence::none();
        thin.reliable_observation_count = 1;
        thin.task_success_rate = Some(1.0);

        let mut thick = thin;
        thick.reliable_observation_count = 20;

        assert!(evidence_signal(&thin).abs() < evidence_signal(&thick).abs());
    }

    /// `NoObservations` answers `None` for anything asked of it — the state
    /// every caller starts in until Phase 33A exists.
    #[test]
    fn no_observations_source_establishes_nothing() {
        let key = pairing::EvidenceKey::new(
            IntegrationId::ClaudeCode,
            "default",
            AssignedModel::named("claude-fable-5"),
            ServingRoute::default(),
        );
        assert!(NoObservations.observed(&key).is_none());
    }
}
