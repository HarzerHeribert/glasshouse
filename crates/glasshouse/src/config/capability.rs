//! Phase 34F — a model's capability as configurable data, not router logic.
//! Widens [`super::ProviderConfig::model_ceilings`] (line 1796, one axis) to
//! the rest of lines 1475–1479, 1482–1485's neighbourhood — structured-output
//! and task-kind suitability, pairing class, calibration evidence — stored
//! beside `model_ceilings` on the same [`super::ProviderConfig`], keyed by
//! the same model identifier. [`ModelCapabilityRecord`] carries no separate
//! ceiling concept: [`resolve_ceiling`] is the one function that reads both
//! and states which wins.
//! `backend` and `model` are not fields here: `model` is already the map
//! key, and `backend` is which `[providers.<name>]` table the record lives
//! in, so a local and a hosted model of the same name are already two
//! records (line 1485). This module adds only `harness`, `launch_profile`
//! and `protocol` — narrowing fields checked by
//! [`ModelCapabilityRecord::applies_to`].
//! User assignment outranks a benchmark seed, and a seed never refuses
//! (56A ruling): [`CeilingResolution::hard_ceiling`] never lets a
//! [`CapabilityProvenance::Benchmark`] ceiling reach a hard routing
//! constraint (line 1484) — only [`CapabilityProvenance::User`] may reject
//! a candidate.
// History: design-decisions.md, "Trims: api, events, harness and config module docs, second packet", crates/glasshouse/src/config/capability.rs module doc.

use serde::{Deserialize, Serialize};

use crate::harness::WireProtocol;
use crate::harness::pairing::PairingClass;
use crate::integrations::IntegrationId;
use crate::routing::classify::WorkloadTier;

use super::{ConfiguredHarness, ConfiguredWorkloadTier, is_false};

/// A [`WireProtocol`] as it is written in a capability record — the same
/// newtype-over-a-routing-type shape as [`ConfiguredHarness`] and
/// [`ConfiguredWorkloadTier`], for the same reason: `WireProtocol` has no
/// serialised form of its own, and this is the config file's side of that
/// boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfiguredProtocol(WireProtocol);

const WIRE_PROTOCOL_SPELLINGS: [WireProtocol; 4] = [
    WireProtocol::AnthropicMessages,
    WireProtocol::OpenAiResponses,
    WireProtocol::OpenAiChat,
    WireProtocol::GeminiGenerateContent,
];

/// The compile-time guard that [`WIRE_PROTOCOL_SPELLINGS`] still lists every
/// [`WireProtocol`] variant — see `super::workload_tier_ordinal` for why
/// this is `#[cfg(test)]` and still a real gate.
#[cfg(test)]
fn wire_protocol_ordinal(protocol: WireProtocol) -> usize {
    match protocol {
        WireProtocol::AnthropicMessages => 0,
        WireProtocol::OpenAiResponses => 1,
        WireProtocol::OpenAiChat => 2,
        WireProtocol::GeminiGenerateContent => 3,
    }
}

impl ConfiguredProtocol {
    pub fn new(protocol: WireProtocol) -> Self {
        Self(protocol)
    }

    pub fn protocol(self) -> WireProtocol {
        self.0
    }

    pub fn as_str(self) -> &'static str {
        self.0.slug()
    }

    /// Exact, like [`ConfiguredWorkloadTier::parse`].
    pub fn parse(text: &str) -> Option<Self> {
        WIRE_PROTOCOL_SPELLINGS
            .into_iter()
            .find(|protocol| protocol.slug() == text)
            .map(Self)
    }
}

impl Serialize for ConfiguredProtocol {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ConfiguredProtocol {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).ok_or_else(|| {
            let known = WIRE_PROTOCOL_SPELLINGS
                .into_iter()
                .map(|protocol| protocol.slug())
                .collect::<Vec<_>>()
                .join(", ");
            serde::de::Error::custom(format!(
                "unknown protocol `{text}` — expected one of: {known}"
            ))
        })
    }
}

/// A [`PairingClass`] as it is written in a capability record — line 1483's
/// "store the harness-model pairing class". Same newtype shape as
/// [`ConfiguredProtocol`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfiguredPairingClass(PairingClass);

const PAIRING_CLASS_SPELLINGS: [PairingClass; 6] = [
    PairingClass::VendorNative,
    PairingClass::VendorSupported,
    PairingClass::ProtocolNative,
    PairingClass::ProtocolCompatible,
    PairingClass::ProtocolTranslated,
    PairingClass::Unknown,
];

/// The compile-time guard that [`PAIRING_CLASS_SPELLINGS`] still lists every
/// [`PairingClass`] variant.
#[cfg(test)]
fn pairing_class_ordinal(class: PairingClass) -> usize {
    match class {
        PairingClass::VendorNative => 0,
        PairingClass::VendorSupported => 1,
        PairingClass::ProtocolNative => 2,
        PairingClass::ProtocolCompatible => 3,
        PairingClass::ProtocolTranslated => 4,
        PairingClass::Unknown => 5,
    }
}

impl ConfiguredPairingClass {
    pub fn new(class: PairingClass) -> Self {
        Self(class)
    }

    pub fn class(self) -> PairingClass {
        self.0
    }

    pub fn as_str(self) -> &'static str {
        self.0.slug()
    }

    /// Exact, like [`ConfiguredWorkloadTier::parse`].
    pub fn parse(text: &str) -> Option<Self> {
        PAIRING_CLASS_SPELLINGS
            .into_iter()
            .find(|class| class.slug() == text)
            .map(Self)
    }
}

impl Serialize for ConfiguredPairingClass {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ConfiguredPairingClass {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).ok_or_else(|| {
            let known = PAIRING_CLASS_SPELLINGS
                .into_iter()
                .map(|class| class.slug())
                .collect::<Vec<_>>()
                .join(", ");
            serde::de::Error::custom(format!(
                "unknown pairing class `{text}` — expected one of: {known}"
            ))
        })
    }
}

/// Who stated this record — line 1484's distinction, and the reason
/// [`CeilingResolution::hard_ceiling`] treats the two differently.
///
/// `User` is the default: an entry that omits `provenance` is read as the
/// user's own statement rather than silently downgraded to an unproven
/// seed, which is the fail-closed direction the 56A ruling asks for — a
/// misread here should make a record *less* powerful, never more.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CapabilityProvenance {
    /// The user assigned this themselves — as authoritative as
    /// [`super::ProviderConfig::model_ceilings`].
    #[default]
    User,
    /// Seeded from a published benchmark table or same-vendor alignment —
    /// "a baseline", in the user's own words, never proof of performance in
    /// this harness.
    Benchmark,
}

/// Line 1478: whether a model is trusted with code editing, debugging, and
/// architecture work, or only with support tasks — classification,
/// extraction, reranking, formatting, the same shape of work
/// [`WorkloadTier::Leaf`] already names.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskSuitability {
    /// Trusted with routine coding, debugging, and architecture-sensitive
    /// work — no cap applied.
    #[default]
    CoreEngineering,
    /// Support tasks only. Never invents a ceiling where none is stated —
    /// see [`Self::cap`] — but never lets a stated ceiling claim more than
    /// [`WorkloadTier::Leaf`], the tier support work already lives at.
    SupportOnly,
}

impl TaskSuitability {
    /// Apply this suitability to an already-resolved ceiling. `None` stays
    /// `None`: a support-only model with no stated ceiling is still simply
    /// *unknown*, not manufactured into `Leaf` from nothing — the same
    /// "nobody has said is not cannot" rule [`super::ProviderConfig::ceiling_of`]
    /// documents for its own field.
    pub fn cap(self, ceiling: Option<WorkloadTier>) -> Option<WorkloadTier> {
        match (self, ceiling) {
            (Self::SupportOnly, Some(tier)) => Some(tier.min(WorkloadTier::Leaf)),
            (_, ceiling) => ceiling,
        }
    }
}

/// Line 1483: how much evidence backs a calibration, independent of who
/// stated it. `Asserted` is the honest default — a record nobody has
/// actually run yet is still worth keeping, just not worth trusting as
/// heavily as one that has been observed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EvidenceStrength {
    /// Stated with no supporting run — a belief, recorded so it can be
    /// corrected rather than lost.
    #[default]
    Asserted,
    /// Backed by a published benchmark table or vendor alignment.
    Benchmarked,
    /// Backed by the user's own observed runs in this harness.
    Observed,
}

/// One calibrated model-capability record, stored at
/// `providers.<name>.model_capabilities.<model>` — the same table shape as
/// [`super::ProviderConfig::model_ceilings`], keyed by the same model
/// identifier, with `provider` supplying the `backend` axis line 1482 asks
/// for. `harness`, `launch_profile`, and `protocol` are the axes a provider
/// entry does not already give: unset means "no narrower than this
/// provider and model", not "applies to nothing" — see [`Self::applies_to`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelCapabilityRecord {
    /// Scope this record to one harness. `None` applies regardless of
    /// harness — line 1482's isolation is enforced only when stated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    harness: Option<ConfiguredHarness>,
    /// Scope this record to one named `[profiles.<name>]` launch profile.
    /// Not validated against [`super::ProfileTable`] at load — a profile
    /// created after this record, or in a layer this file cannot see, must
    /// not make the record fail to parse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    launch_profile: Option<String>,
    /// Scope this record to one wire protocol path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    protocol: Option<ConfiguredProtocol>,
    /// Line 1476: the initial expected workload ceiling — a seed
    /// [`resolve_ceiling`] weighs against
    /// [`super::ProviderConfig::model_ceilings`]'s override, never a
    /// second override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ceiling: Option<ConfiguredWorkloadTier>,
    /// Line 1477: whether this model is suitable for structured routing
    /// output.
    #[serde(default, skip_serializing_if = "is_false")]
    structured_output_suitable: bool,
    /// Line 1478.
    #[serde(default, skip_serializing_if = "is_core_engineering")]
    task_suitability: TaskSuitability,
    /// Line 1484: required, never defaulted away from meaning something —
    /// see the type's own doc for why `User` is still the fail-closed
    /// default when the key is present but unrecognised elsewhere.
    provenance: CapabilityProvenance,
    /// Line 1483: the harness-model pairing class this calibration was
    /// measured under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pairing_class: Option<ConfiguredPairingClass>,
    /// Line 1483: how much evidence backs this record.
    #[serde(default)]
    evidence_strength: EvidenceStrength,
}

fn is_core_engineering(suitability: &TaskSuitability) -> bool {
    matches!(suitability, TaskSuitability::CoreEngineering)
}

impl ModelCapabilityRecord {
    pub fn new(provenance: CapabilityProvenance) -> Self {
        Self {
            harness: None,
            launch_profile: None,
            protocol: None,
            ceiling: None,
            structured_output_suitable: false,
            task_suitability: TaskSuitability::default(),
            provenance,
            pairing_class: None,
            evidence_strength: EvidenceStrength::default(),
        }
    }

    pub fn harness(&self) -> Option<IntegrationId> {
        self.harness.map(ConfiguredHarness::id)
    }

    pub fn set_harness(&mut self, harness: Option<IntegrationId>) -> &mut Self {
        self.harness = harness.map(ConfiguredHarness::new);
        self
    }

    pub fn launch_profile(&self) -> Option<&str> {
        self.launch_profile.as_deref()
    }

    pub fn set_launch_profile(&mut self, profile: Option<String>) -> &mut Self {
        self.launch_profile = profile;
        self
    }

    pub fn protocol(&self) -> Option<WireProtocol> {
        self.protocol.map(ConfiguredProtocol::protocol)
    }

    pub fn set_protocol(&mut self, protocol: Option<WireProtocol>) -> &mut Self {
        self.protocol = protocol.map(ConfiguredProtocol::new);
        self
    }

    pub fn ceiling(&self) -> Option<WorkloadTier> {
        self.ceiling.map(ConfiguredWorkloadTier::tier)
    }

    pub fn set_ceiling(&mut self, ceiling: Option<WorkloadTier>) -> &mut Self {
        self.ceiling = ceiling.map(ConfiguredWorkloadTier::new);
        self
    }

    pub fn structured_output_suitable(&self) -> bool {
        self.structured_output_suitable
    }

    pub fn set_structured_output_suitable(&mut self, suitable: bool) -> &mut Self {
        self.structured_output_suitable = suitable;
        self
    }

    pub fn task_suitability(&self) -> TaskSuitability {
        self.task_suitability
    }

    pub fn set_task_suitability(&mut self, suitability: TaskSuitability) -> &mut Self {
        self.task_suitability = suitability;
        self
    }

    pub fn provenance(&self) -> CapabilityProvenance {
        self.provenance
    }

    pub fn set_provenance(&mut self, provenance: CapabilityProvenance) -> &mut Self {
        self.provenance = provenance;
        self
    }

    pub fn pairing_class(&self) -> Option<PairingClass> {
        self.pairing_class.map(ConfiguredPairingClass::class)
    }

    pub fn set_pairing_class(&mut self, class: Option<PairingClass>) -> &mut Self {
        self.pairing_class = class.map(ConfiguredPairingClass::new);
        self
    }

    pub fn evidence_strength(&self) -> EvidenceStrength {
        self.evidence_strength
    }

    pub fn set_evidence_strength(&mut self, strength: EvidenceStrength) -> &mut Self {
        self.evidence_strength = strength;
        self
    }

    /// Line 1482: whether this record's stated scope covers `query`. A
    /// field this record leaves unset matches anything — narrowing applies
    /// only where the record actually narrows. A field this record *does*
    /// state must equal the query's, so a calibration recorded for one
    /// harness, profile, or protocol path never leaks onto another the same
    /// model happens to share a provider entry with.
    pub fn applies_to(&self, query: &CapabilityQuery<'_>) -> bool {
        if let Some(harness) = self.harness()
            && Some(harness) != query.harness
        {
            return false;
        }
        if let Some(profile) = self.launch_profile()
            && Some(profile) != query.launch_profile
        {
            return false;
        }
        if let Some(protocol) = self.protocol()
            && Some(protocol) != query.protocol
        {
            return false;
        }
        true
    }

    /// Whether this record states **no** narrowing axis at all — the only
    /// shape a context-blind caller may safely consume.
    ///
    /// `resolved_ceiling`'s own path (via `EffectiveConfig::model_ceiling`)
    /// reaches a record by `(provider, model)` alone: it has no harness,
    /// launch profile, or protocol in hand to check against
    /// [`Self::applies_to`] — that context lives only in `main.rs`, which
    /// this package does not touch (see the module doc's "backend and model
    /// are not fields here"). A record that states even one narrowing axis
    /// must therefore stay **invisible** to that path: reading it anyway
    /// would leak a harness-scoped (or profile- or protocol-scoped)
    /// calibration onto every destination that shares this provider and
    /// model, including ones on a different harness entirely — exactly the
    /// isolation line 1482 forbids, "unknown reads as unknown" applied to a
    /// record the caller cannot honestly evaluate rather than to a model
    /// nobody calibrated.
    pub(super) fn is_context_general(&self) -> bool {
        self.harness.is_none() && self.launch_profile.is_none() && self.protocol.is_none()
    }
}

/// The context a caller has about the destination it wants a capability
/// record for — line 1482's other three axes, `model` and `backend` (the
/// provider) already being how a caller reached this record in the first
/// place. Any field left `None` means the caller does not know or does not
/// care, and [`ModelCapabilityRecord::applies_to`] only rejects a record
/// that states a value the query contradicts.
#[derive(Debug, Clone, Copy, Default)]
pub struct CapabilityQuery<'a> {
    pub harness: Option<IntegrationId>,
    pub launch_profile: Option<&'a str>,
    pub protocol: Option<WireProtocol>,
}

/// What decided a model's effective workload ceiling — line 1479's
/// precedence and line 1484's hard/soft split, in one place so no call site
/// re-derives either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CeilingResolution {
    /// [`super::ProviderConfig::model_ceilings`]'s own override — the
    /// strongest statement possible, and untouched by this module.
    UserOverride(WorkloadTier),
    /// A capability record whose [`CapabilityProvenance`] is `User` — as
    /// authoritative as an override, just written in the newer place.
    UserCapabilityRecord(WorkloadTier),
    /// A capability record whose provenance is `Benchmark`. Carries the
    /// seeded ceiling for ranking, but [`Self::hard_ceiling`] never returns
    /// it — line 1484 forbids a prior from refusing what the user never
    /// restricted.
    Prior(Option<WorkloadTier>),
    /// Nothing has established a ceiling.
    Unknown,
}

impl CeilingResolution {
    /// The ceiling safe to use as a **hard routing constraint** — the value
    /// [`super::ProviderConfig::ceiling_of`] used to be the only source of.
    /// `Prior` never appears here, by construction: a benchmark seed may
    /// rank candidates but must never be the reason one is refused.
    pub fn hard_ceiling(self) -> Option<WorkloadTier> {
        match self {
            Self::UserOverride(tier) | Self::UserCapabilityRecord(tier) => Some(tier),
            Self::Prior(_) | Self::Unknown => None,
        }
    }

    /// Whether this decision rested on a benchmark/vendor prior rather than
    /// on something the user assigned — line 1484's "the rendered
    /// explanation must say when a decision rested on a prior".
    pub fn rested_on_prior(self) -> bool {
        matches!(self, Self::Prior(_))
    }

    /// A sentence naming what decided the ceiling. Never states a prior as
    /// proof of performance — line 1484.
    pub fn explain(self) -> String {
        match self {
            Self::UserOverride(tier) => {
                format!("the user's own override established `{tier}`")
            }
            Self::UserCapabilityRecord(tier) => {
                format!("the user's own capability assignment established `{tier}`")
            }
            Self::Prior(Some(tier)) => format!(
                "a benchmark-derived prior suggests `{tier}` — a starting point, not proof of \
                 performance in this harness"
            ),
            Self::Prior(None) => {
                "a benchmark-derived record exists here but states no ceiling".to_owned()
            }
            Self::Unknown => "nothing has established a ceiling for this model".to_owned(),
        }
    }
}

/// The one lookup line 1479 asks for: an explicit override always beats a
/// capability record's initial ceiling, and a capability record beats
/// nothing. `override_ceiling` is
/// [`super::ProviderConfig::ceiling_of`]'s own answer — this function does
/// not re-implement it, only sequences it ahead of the newer mechanism.
pub fn resolve_ceiling(
    override_ceiling: Option<WorkloadTier>,
    record: Option<&ModelCapabilityRecord>,
) -> CeilingResolution {
    if let Some(tier) = override_ceiling {
        return CeilingResolution::UserOverride(tier);
    }
    match record.map(|record| {
        (
            record.provenance(),
            record.task_suitability().cap(record.ceiling()),
        )
    }) {
        Some((CapabilityProvenance::User, Some(tier))) => {
            CeilingResolution::UserCapabilityRecord(tier)
        }
        Some((CapabilityProvenance::User, None)) => CeilingResolution::Unknown,
        Some((CapabilityProvenance::Benchmark, capped)) => CeilingResolution::Prior(capped),
        None => CeilingResolution::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_wire_protocol_spelling_round_trips() {
        for protocol in WIRE_PROTOCOL_SPELLINGS {
            let _ = wire_protocol_ordinal(protocol);
            let spelled = ConfiguredProtocol::new(protocol).as_str();
            assert_eq!(
                ConfiguredProtocol::parse(spelled).map(ConfiguredProtocol::protocol),
                Some(protocol)
            );
        }
        assert_eq!(ConfiguredProtocol::parse("carrier-pigeon"), None);
    }

    #[test]
    fn every_pairing_class_spelling_round_trips() {
        for class in PAIRING_CLASS_SPELLINGS {
            let _ = pairing_class_ordinal(class);
            let spelled = ConfiguredPairingClass::new(class).as_str();
            assert_eq!(
                ConfiguredPairingClass::parse(spelled).map(ConfiguredPairingClass::class),
                Some(class)
            );
        }
        assert_eq!(ConfiguredPairingClass::parse("telepathic"), None);
    }
}
