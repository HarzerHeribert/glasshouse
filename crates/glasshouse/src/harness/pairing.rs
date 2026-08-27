//! Phase 9J — what the pairing between a harness and a model actually *is*.
//!
//! # Six things, stored six ways
//!
//! [`super::Vendor`] already means **who publishes the harness executable**,
//! and its own documentation explains why collapsing that with who made the
//! model and who serves it produces a router that "ends up believing a
//! harness and a model are first-party partners because their names rhyme".
//! This module is the other half of that sentence: the model side, and the
//! relationship between the two.
//!
//! Line 554 of the capability map asks for six independent facts — harness
//! vendor, model developer, model family, serving provider, gateway, wire
//! protocol. They are six separate fields of [`Pairing`] and no one of them
//! is ever derived from another. In particular [`ServingRoute::provider`] is
//! never consulted to answer [`Pairing::developer`]: a reseller is not an
//! author, and OpenRouter serving `claude-opus-5` makes OpenRouter neither
//! Anthropic nor the model's developer.
//!
//! # `Unknown` is an answer, not a fallback
//!
//! [`ModelDeveloper::Unknown`] and [`PairingClass::Unknown`] are what
//! Glasshouse says about a stealth or insufficiently attributed model, and
//! they are deliberately reachable from the *front* of the ladder rather than
//! the end of it. A model named after a company is not evidence it was made
//! there — that is the same failure [`super::Vendor`] describes, one level
//! down — so nothing here reads a developer out of a model's name, a
//! provider's name, or a harness's branding. An id nothing attributes stays
//! unattributed until a person says otherwise.
//!
//! # Three axes, because the map says three
//!
//! Line 559 requires protocol compatibility to be treated separately from
//! model-behaviour compatibility and tool-semantic compatibility, and a
//! single "compatible" verdict would be exactly the thing it forbids. So
//! [`Pairing`] answers three questions with three types that cannot stand in
//! for one another: [`ProtocolFit`], [`ModelBehaviourFit`], and
//! [`crate::routing::ToolSemantics`]. They disagree in practice — a provider
//! can serve a harness's own wire protocol (`ProtocolFit::Native`) while
//! declaring nothing whatever about tool calls on it
//! (`ToolSemantics::Unverified`), which is the state of every built-in
//! provider template today.
//!
//! **`ModelBehaviourFit` is `Unverified` for every catalogued model**, and
//! that is not an oversight: nothing in Glasshouse observes whether a model
//! behaves the way a harness needs. Phase 33A's routing evidence ledger is
//! what would feed it. Until then the only thing that can move it is a
//! person who has run the pairing and found out — see
//! [`ModelCorrection::behaviour`].
//!
//! # Declarative, and correctable without touching a router
//!
//! Two data structures decide everything here, and neither is code a router
//! reads:
//!
//! - [`OfficialModelSupport`], declared by each adapter beside its other
//!   [`Declared`] facts. A harness that adds official support for a model is
//!   one string in one array — lines 558 and 562.
//! - [`PairingOverrides`], built by `crate::config::pairing` out of the
//!   user's own configuration file. Line 561: a correction is a
//!   configuration edit, and `classify` is the only thing that reads it.
//!
//! # This module imports no configuration
//!
//! Same rule, and the same reason, as [`mod@crate::profile`]: the caller
//! looks configuration up and hands the resolved values in. That keeps
//! [`classify`] a pure function of its arguments — no file, no environment,
//! no ambient lookup — and it is why [`PairingOverrides`] is a plain map this
//! module defines and `crate::config` fills in.

use std::collections::BTreeMap;

use crate::integrations::IntegrationId;
use crate::routing::{AssignedModel, ToolSemantics};

use super::{Declared, Vendor, WireProtocol};

/// Who developed a model.
///
/// Deliberately **not** [`Vendor`], and deliberately not the same type: a
/// value of one can never be assigned to the other, so the collapse
/// [`Vendor`]'s own documentation warns about cannot be written by accident.
/// Deliberately not the serving provider either — line 555.
///
/// The named form carries a slug rather than a variant per company, for one
/// reason that matters: line 561 requires a user to be able to correct
/// pairing metadata *without changing router code*, and an enum of companies
/// would make "my model was made by someone Glasshouse has never heard of" a
/// code change. It would also be this module inventing a list of
/// organisations from recollection, which is exactly what the rest of
/// [`mod@crate::harness`] refuses to do.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ModelDeveloper {
    /// Attributed, to the organisation this slug names.
    Named(String),
    /// Nobody has attributed this model well enough to name a developer.
    ///
    /// A first-class answer, not a fallback: a stealth model, a model behind
    /// an alias, or a model Glasshouse simply has no entry for all stop
    /// here rather than being guessed from behaviour or branding — line 560.
    Unknown,
}

impl ModelDeveloper {
    pub fn named(slug: impl Into<String>) -> Self {
        Self::Named(slug.into())
    }

    /// The developer's slug, or `None` when the model is unattributed.
    pub fn slug(&self) -> Option<&str> {
        match self {
            Self::Named(slug) => Some(slug),
            Self::Unknown => None,
        }
    }

    pub fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown)
    }

    /// For a diagnostic. Never `""`: an empty column reads as missing data,
    /// and "unknown" is a claim Glasshouse is making on purpose.
    pub fn label(&self) -> &str {
        match self {
            Self::Named(slug) => slug,
            Self::Unknown => "unknown",
        }
    }
}

impl std::fmt::Display for ModelDeveloper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.label())
    }
}

/// How a harness and a model stand to one another.
///
/// The six the capability map names, in the order the ladder resolves them.
/// The first two are statements about a *vendor's* declared support; the next
/// three are statements about the *wire*; and [`PairingClass::Unknown`] is
/// what an unattributed model gets even when its wire is perfectly ordinary,
/// because "which harness-model relationship is this" has no honest answer
/// when nobody knows who made the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingClass {
    /// The harness is operating a model family produced for its own vendor's
    /// coding environment — line 557. Both halves are required: the vendor
    /// declares the family as one of its own, *and* the model's developer is
    /// that vendor's own organisation.
    VendorNative,
    /// The harness vendor explicitly supports this model although the model
    /// and harness developers differ — line 558. Deliberately reachable for
    /// a model whose developer is unknown: Google listing a model in
    /// `agy models` is Google's statement about its own product, and it
    /// stands whether or not Glasshouse can say who wrote the weights.
    VendorSupported,
    /// No vendor relationship, but the provider serves the harness's own
    /// wire protocol, so nothing has to be translated.
    ProtocolNative,
    /// The route's protocol is not one the harness speaks, but the provider
    /// also serves one that it does — reachable by choosing the other
    /// protocol.
    ProtocolCompatible,
    /// Neither speaks the other's wire and an explicit translation adapter
    /// exists for the pair. See [`crate::provider::translation_available`],
    /// which is always `false` in V1 — so this class is representable and
    /// currently unreachable, which is the map's own stance on translation
    /// rather than a gap here.
    ProtocolTranslated,
    /// Nothing establishes a relationship. An unattributed model, a harness
    /// that declares no protocols, a launch profile that names no model.
    Unknown,
}

impl PairingClass {
    pub fn slug(self) -> &'static str {
        match self {
            Self::VendorNative => "vendor-native",
            Self::VendorSupported => "vendor-supported",
            Self::ProtocolNative => "protocol-native",
            Self::ProtocolCompatible => "protocol-compatible",
            Self::ProtocolTranslated => "protocol-translated",
            Self::Unknown => "unknown",
        }
    }

    /// Whether this class is the vendor-native pairing a routing prior would
    /// later be attached to.
    ///
    /// Named as a question rather than exposed as an `==` so the one place
    /// that will eventually apply Phase 9J's soft prior has a function to
    /// call. It is **not** a quality claim: the map's first fixed
    /// architectural requirement says vendor alignment is "never proof of
    /// quality or a hard routing requirement".
    pub fn is_vendor_native(self) -> bool {
        matches!(self, Self::VendorNative)
    }
}

impl std::fmt::Display for PairingClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.slug())
    }
}

/// What the wire protocol alone says about a pairing.
///
/// One of the three axes line 559 requires to stay apart. This one is
/// decidable today from declarations that already exist: what the adapter
/// says it speaks, and what the provider says it serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolFit {
    /// The harness declares it speaks this wire protocol itself.
    Native,
    /// Not this one, but the provider serves another protocol the harness
    /// does speak.
    Compatible,
    /// An explicit translation adapter exists for the pair.
    Translated,
    /// The harness declares protocols, none of them is served here, and no
    /// translation exists.
    Incompatible,
    /// The harness declares no protocols, or no protocol was named for the
    /// route. Not a "no".
    Unknown,
}

impl ProtocolFit {
    pub fn slug(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Compatible => "compatible",
            Self::Translated => "translated",
            Self::Incompatible => "incompatible",
            Self::Unknown => "unknown",
        }
    }
}

impl std::fmt::Display for ProtocolFit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.slug())
    }
}

/// What is established about whether a model behaves the way a harness needs
/// — its prompt shape, its stop conditions, whether it follows the harness's
/// instructions at all.
///
/// The second of line 559's three axes, and the one nothing in Glasshouse
/// measures yet. Its three states are deliberately the same three
/// [`crate::routing::ToolSemantics`] has, and it is deliberately a *different
/// type*: a value of one must not be usable as the other, or the two axes
/// would be one axis with two names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelBehaviourFit {
    Verified,
    /// Nobody established it either way. Every catalogued model, today.
    Unverified,
    KnownAbsent,
}

impl ModelBehaviourFit {
    pub fn slug(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Unverified => "unverified",
            Self::KnownAbsent => "known absent",
        }
    }

    /// Parse the spelling a configuration file uses, or `None`.
    pub fn from_slug(slug: &str) -> Option<Self> {
        match slug {
            "verified" => Some(Self::Verified),
            "unverified" => Some(Self::Unverified),
            "known-absent" => Some(Self::KnownAbsent),
            _ => None,
        }
    }
}

impl std::fmt::Display for ModelBehaviourFit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.slug())
    }
}

/// Which models a harness's own vendor says it operates, in the two senses
/// the capability map distinguishes.
///
/// Both are [`Declared`] for the same reason every other adapter fact is:
/// "this vendor supports nothing beyond its own models" and "nobody read the
/// vendor's list" are different claims, and only one of them should stop a
/// pairing being called vendor-supported.
///
/// Adding official support for a model is an edit to one of these arrays,
/// inside the adapter that already owns every other fact about that harness
/// — line 562. Nothing in a router changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OfficialModelSupport {
    /// Model **families** this harness's vendor produces for this coding
    /// environment. Families rather than ids, because a vendor ships point
    /// releases of its own line continuously and a per-id list would be
    /// stale the week it was written.
    pub native_families: Declared<&'static [&'static str]>,
    /// Model **ids** the harness vendor explicitly supports although it did
    /// not develop them. Ids rather than families, because this is a
    /// concrete list a vendor publishes, not an open-ended line.
    pub supported_models: Declared<&'static [&'static str]>,
}

impl OfficialModelSupport {
    /// Nothing established — the default every adapter inherits until it has
    /// read its harness's own model list.
    pub const UNVERIFIED: Self = Self {
        native_families: Declared::Unverified,
        supported_models: Declared::Unverified,
    };
}

/// Where an attribution came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttributionSource {
    /// From [`catalogue`], citing the publisher's own artifact it was read
    /// from.
    Catalogue { evidence: &'static str },
    /// From a person, through configuration. `source` names which
    /// configuration file, so a surprising verdict can be traced to the file
    /// that caused it.
    Correction { source: String },
    /// Nothing attributes this model.
    Unattributed,
}

impl AttributionSource {
    /// A sentence for a report. Never empty.
    pub fn describe(&self) -> String {
        match self {
            Self::Catalogue { evidence } => (*evidence).to_owned(),
            Self::Correction { source } => format!("corrected in {source}"),
            Self::Unattributed => {
                "nothing attributes this model, and Glasshouse does not guess from its name"
                    .to_owned()
            }
        }
    }
}

/// What Glasshouse believes about one model, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelAttribution {
    pub developer: ModelDeveloper,
    /// The model line this id belongs to, or `None` when unattributed.
    pub family: Option<String>,
    pub behaviour: ModelBehaviourFit,
    pub source: AttributionSource,
}

impl ModelAttribution {
    /// The attribution of a model nobody has said anything about.
    pub fn unattributed() -> Self {
        Self {
            developer: ModelDeveloper::Unknown,
            family: None,
            behaviour: ModelBehaviourFit::Unverified,
            source: AttributionSource::Unattributed,
        }
    }
}

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
/// back; this is that parse, for the one caller (Phase 9J's routing consumer)
/// that already depends on this module and needs a [`ServingRoute::protocol`]
/// to classify a candidate.
///
/// `None` for a slug none of the three known variants produced — a
/// [`Pairing`]'s vendor-native status never depends on it (see
/// [`crate::config::pairing::native_pairing_prior_contribution`]'s own doc),
/// so this only ever weakens [`Pairing::protocol_fit`], never invents one.
pub fn wire_protocol_from_slug(slug: &str) -> Option<WireProtocol> {
    [
        WireProtocol::AnthropicMessages,
        WireProtocol::OpenAiResponses,
        WireProtocol::OpenAiChat,
    ]
    .into_iter()
    .find(|protocol| protocol.slug() == slug)
}

/// One correction a person made to a model's pairing metadata.
///
/// Every field is optional and corrects exactly what it names; anything left
/// out keeps whatever [`catalogue`] said. A correction that named a whole
/// attribution would force a user fixing one wrong family to restate a
/// developer that was already right.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelCorrection {
    pub developer: Option<ModelDeveloper>,
    pub family: Option<String>,
    /// The one axis of [`ModelBehaviourFit`] a person can move today. A user
    /// who has actually run a pairing and watched it mangle every tool call
    /// knows something Glasshouse does not, and Phase 33A is what would
    /// eventually learn it without being told.
    pub behaviour: Option<ModelBehaviourFit>,
}

/// One correction a person made to what a harness vendor officially
/// supports.
///
/// The case this exists for: a harness announces support for a model between
/// Glasshouse releases. Line 562 wants that to be a metadata change; for the
/// adapter it is one, and this is the same escape hatch for a user who cannot
/// wait for the release.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SupportCorrection {
    pub native_families: Option<Vec<String>>,
    pub supported_models: Option<Vec<String>>,
}

/// Every pairing correction a person has made, resolved out of configuration
/// by the caller.
///
/// Keyed by model id and by [`IntegrationId::slug`] — strings, because these
/// are configuration keys and a configuration key that names something
/// Glasshouse does not know must degrade visibly rather than refuse to load.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PairingOverrides {
    models: BTreeMap<String, ModelCorrection>,
    harnesses: BTreeMap<String, SupportCorrection>,
    /// Which configuration file these came from, for
    /// [`AttributionSource::Correction`].
    source: String,
}

impl PairingOverrides {
    /// Corrections read from the file `source` names.
    pub fn from_parts(
        source: impl Into<String>,
        models: BTreeMap<String, ModelCorrection>,
        harnesses: BTreeMap<String, SupportCorrection>,
    ) -> Self {
        Self {
            models,
            harnesses,
            source: source.into(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.models.is_empty() && self.harnesses.is_empty()
    }

    pub fn model(&self, id: &str) -> Option<&ModelCorrection> {
        self.models.get(id)
    }

    pub fn harness(&self, id: IntegrationId) -> Option<&SupportCorrection> {
        self.harnesses.get(id.slug())
    }

    pub fn source(&self) -> &str {
        &self.source
    }
}

/// The four-part identity Phase 9J line 572 requires local evidence to be
/// kept apart by: harness, launch profile, model, and the exact serving
/// route.
///
/// A nominal model id is not enough — the same id reached through a different
/// gateway, quantization, revision or protocol translation is different
/// evidence, and [`ServingRoute`] is exactly the value that already carries
/// that distinction (its `gateway` and `protocol` fields), so this type reuses
/// it rather than inventing a parallel notion of "route". Two
/// [`EvidenceKey`]s compare equal only when all four parts match; nothing
/// here collapses a model to itself across two routes.
///
/// Deliberately pure, like the rest of this module: building one needs no
/// configuration, only the identity of a pairing that was already resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceKey {
    harness: IntegrationId,
    launch_profile: String,
    model: AssignedModel,
    route: ServingRoute,
}

impl EvidenceKey {
    pub fn new(
        harness: IntegrationId,
        launch_profile: impl Into<String>,
        model: AssignedModel,
        route: ServingRoute,
    ) -> Self {
        Self {
            harness,
            launch_profile: launch_profile.into(),
            model,
            route,
        }
    }

    pub fn harness(&self) -> IntegrationId {
        self.harness
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

/// Everything [`classify`] needs about one harness-and-model pairing.
///
/// A plain value: the caller resolves configuration, looks the provider up,
/// and hands the answers in. See this module's header for why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingQuery {
    pub harness: IntegrationId,
    /// Which model Glasshouse assigned — including
    /// [`AssignedModel::HarnessDefault`], which is not a failure and is not
    /// an invitation to assume the harness vendor's own model. Glasshouse
    /// named none, so it knows none.
    pub model: AssignedModel,
    pub route: ServingRoute,
    /// What the serving provider declares about tool calls on
    /// [`ServingRoute::protocol`].
    pub tool_calls: Declared<bool>,
    /// Every protocol the serving provider declares a usable base URL for.
    /// Empty when there is no configured provider.
    pub provider_protocols: Vec<WireProtocol>,
}

/// A classified pairing: six stored facts, three compatibility axes, one
/// class, and the sentence that explains it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pairing {
    harness: IntegrationId,
    harness_vendor: Declared<Vendor>,
    model: AssignedModel,
    attribution: ModelAttribution,
    route: ServingRoute,
    class: PairingClass,
    protocol_fit: ProtocolFit,
    tool_semantics: ToolSemantics,
    reason: String,
}

impl Pairing {
    pub fn harness(&self) -> IntegrationId {
        self.harness
    }

    /// Who publishes the harness. Never who made the model.
    pub fn harness_vendor(&self) -> Declared<Vendor> {
        self.harness_vendor
    }

    pub fn model(&self) -> &AssignedModel {
        &self.model
    }

    /// Who developed the model. Never the serving provider, never the
    /// harness vendor.
    pub fn developer(&self) -> &ModelDeveloper {
        &self.attribution.developer
    }

    pub fn family(&self) -> Option<&str> {
        self.attribution.family.as_deref()
    }

    pub fn attribution(&self) -> &ModelAttribution {
        &self.attribution
    }

    /// Who serves it, through what, over what wire.
    pub fn route(&self) -> &ServingRoute {
        &self.route
    }

    pub fn class(&self) -> PairingClass {
        self.class
    }

    /// The wire-protocol axis, on its own.
    pub fn protocol_fit(&self) -> ProtocolFit {
        self.protocol_fit
    }

    /// The model-behaviour axis, on its own — never derived from
    /// [`Pairing::protocol_fit`].
    pub fn model_behaviour(&self) -> ModelBehaviourFit {
        self.attribution.behaviour
    }

    /// The tool-semantics axis, on its own.
    pub fn tool_semantics(&self) -> ToolSemantics {
        self.tool_semantics
    }

    /// Why the class is what it is, in one sentence.
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

/// The developer organisation a harness vendor is, when Glasshouse has
/// established one.
///
/// **The only place a [`Vendor`] and a [`ModelDeveloper`] are ever compared**,
/// and it is a declared table rather than a name match, because "the company
/// that publishes this harness is the company that made this model" is
/// precisely the inference [`Vendor`]'s own documentation exists to stop
/// being made by accident.
///
/// `None` is the answer for a harness vendor whose own model line nothing
/// here established. Cursor CLI, OpenCode, Pi and Hermes Agent are all
/// `None`: none of their installations names a model that vendor developed,
/// and inventing one would be worse than the missing capability.
fn vendor_organisation(vendor: Vendor) -> Option<&'static str> {
    match vendor {
        // `claude --help` documents Anthropic authentication as its
        // first-party mechanism and names `claude-fable-5` as a model's full
        // name — the publisher and the model line are the same organisation
        // in the harness's own help text.
        Vendor::Anthropic => Some("anthropic"),
        // `codex --help` gives `-c model="o3"` as its own configuration
        // example, and Codex writes OpenAI model ids into its own
        // `[tui.model_availability_nux]` table.
        Vendor::OpenAi => Some("openai"),
        // `agy models`, run against the user's own account, lists the
        // `gemini-*` family under Google's own CLI alongside the models it
        // labels as other vendors'.
        Vendor::Google => Some("google"),
        Vendor::Cursor | Vendor::OpenCode | Vendor::Pi | Vendor::Hermes => None,
    }
}

/// One catalogued model: an id, who made it, what line it belongs to, and
/// the artifact that was read to establish it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogueEntry {
    pub id: &'static str,
    pub developer: &'static str,
    pub family: &'static str,
    pub evidence: &'static str,
}

/// Evidence read on 2026-08-27 from Claude Code 2.1.246's own help.
const CLAUDE_HELP: &str = "`claude --help` (Claude Code 2.1.246, read 2026-08-27): \"--model \
                           <model>  Model for the current session. Provide an alias for the \
                           latest model (e.g. 'fable', 'opus', or 'sonnet') or a model's full \
                           name (e.g. 'claude-fable-5').\"";

/// Evidence read on 2026-08-27 from the Antigravity CLI 1.1.21.
const AGY_MODELS: &str = "`agy models` (Antigravity CLI 1.1.21, run 2026-08-27) lists it among \
                          the models Google's own CLI offers, under that display name";

/// The models this project has actually read an attribution for.
///
/// Small on purpose. Every entry names the artifact it came from, in the same
/// way [`super::HarnessAdapter::describe`]'s declarations do, and an id
/// nobody read stays out — where it becomes [`ModelDeveloper::Unknown`]
/// rather than a guess. A user who needs one Glasshouse has not read adds it
/// in configuration; that is the escape hatch line 561 asks for, and it is
/// why this table does not have to be a catalogue of the world.
///
/// The weakest entries here are the cross-vendor ones — `claude-sonnet-4-6`,
/// `claude-opus-4-6-thinking` — where a third party's list is what names the
/// model. They are attributed anyway, because a vendor naming a competitor's
/// product line inside its own product is that vendor stating where the model
/// came from, which is a different thing from a model merely being *named*
/// after a company. `gpt-oss-120b-medium` is the case where that reasoning
/// runs out: Antigravity lists it, nothing here says who wrote it, and it
/// stays unattributed.
pub fn catalogue() -> &'static [CatalogueEntry] {
    &[
        CatalogueEntry {
            id: "claude-fable-5",
            developer: "anthropic",
            family: "fable",
            evidence: CLAUDE_HELP,
        },
        CatalogueEntry {
            id: "fable",
            developer: "anthropic",
            family: "fable",
            evidence: CLAUDE_HELP,
        },
        CatalogueEntry {
            id: "opus",
            developer: "anthropic",
            family: "opus",
            evidence: CLAUDE_HELP,
        },
        CatalogueEntry {
            id: "sonnet",
            developer: "anthropic",
            family: "sonnet",
            evidence: CLAUDE_HELP,
        },
        CatalogueEntry {
            id: "gemini-3.1-pro-high",
            developer: "google",
            family: "gemini",
            evidence: AGY_MODELS,
        },
        CatalogueEntry {
            id: "gemini-3.7-flash-high",
            developer: "google",
            family: "gemini",
            evidence: AGY_MODELS,
        },
        CatalogueEntry {
            id: "claude-sonnet-4-6",
            developer: "anthropic",
            family: "sonnet",
            evidence: "`agy models` (Antigravity CLI 1.1.21, run 2026-08-27) lists it as \
                       \"Claude Sonnet 4.6 (Thinking)\", and `claude --help` documents \
                       `claude-<family>-<n>` as Anthropic's own full model-name form — a \
                       vendor naming another vendor's product line in its own listing, not a \
                       name that merely rhymes",
        },
        CatalogueEntry {
            id: "claude-opus-4-6-thinking",
            developer: "anthropic",
            family: "opus",
            evidence: "`agy models` (Antigravity CLI 1.1.21, run 2026-08-27) lists it as \
                       \"Claude Opus 4.6 (Thinking)\", and `claude --help` documents \
                       `claude-<family>-<n>` as Anthropic's own full model-name form",
        },
        CatalogueEntry {
            id: "gpt-5.6-sol",
            developer: "openai",
            family: "gpt-5",
            evidence: "Codex 0.149.1 wrote `\"gpt-5.6-sol\"` into the \
                       `[tui.model_availability_nux]` table of its own `~/.codex/config.toml`, \
                       read 2026-08-27",
        },
        CatalogueEntry {
            id: "gpt-5.5",
            developer: "openai",
            family: "gpt-5",
            evidence: "Codex 0.149.1 wrote `\"gpt-5.5\"` into the \
                       `[tui.model_availability_nux]` table of its own `~/.codex/config.toml`, \
                       read 2026-08-27",
        },
        CatalogueEntry {
            id: "o3",
            developer: "openai",
            family: "o-series",
            evidence: "`codex --help` (codex-cli 0.149.1, read 2026-08-27) gives \
                       `-c model=\"o3\"` as its own configuration-override example",
        },
    ]
}

/// The catalogue entry for `id`, or `None`.
///
/// Exact match, and only exact. A `vendor/model` routing prefix is **not**
/// stripped and a family is **not** inferred from a common stem: both would
/// be reading a developer out of a name, which line 560 forbids and which is
/// the whole failure mode this phase exists to prevent.
pub fn catalogued(id: &str) -> Option<&'static CatalogueEntry> {
    catalogue().iter().find(|entry| entry.id == id)
}

/// What a provider declares about tool calls on the route's protocol, as the
/// three states routing already speaks in.
///
/// The same translation `profile::tool_semantics` makes, deliberately not
/// shared with it: that one is private to a module this one must not import,
/// and [`Declared::is_known_present`] cannot be used for either, because it
/// collapses "verified absent" into "nobody checked" and this axis turns on
/// exactly that difference.
fn tool_semantics(declared: Declared<bool>) -> ToolSemantics {
    match declared {
        Declared::Verified { value: true, .. } => ToolSemantics::Verified,
        Declared::Verified { value: false, .. } => ToolSemantics::KnownAbsent,
        Declared::Unverified => ToolSemantics::Unverified,
    }
}

/// The wire-protocol axis, alone.
fn protocol_fit(
    spoken: Declared<&'static [WireProtocol]>,
    route: Option<WireProtocol>,
    served: &[WireProtocol],
) -> ProtocolFit {
    let (Some(spoken), Some(route)) = (spoken.value(), route) else {
        return ProtocolFit::Unknown;
    };
    if spoken.contains(&route) {
        return ProtocolFit::Native;
    }
    if served.iter().any(|other| spoken.contains(other)) {
        return ProtocolFit::Compatible;
    }
    // The one caller of the translation seam. V1 answers `false` for every
    // pair — see `provider::translation_available` — so this arm is
    // representable and unreachable today, which is the map's stance rather
    // than a gap.
    if spoken
        .iter()
        .any(|to| crate::provider::translation_available(route, *to))
    {
        return ProtocolFit::Translated;
    }
    ProtocolFit::Incompatible
}

/// Both official-support lists for `harness`, after any user correction.
fn resolved_support(
    harness: IntegrationId,
    overrides: &PairingOverrides,
) -> (Vec<String>, Vec<String>, bool) {
    let declared = super::adapter_for(harness)
        .map(|adapter| adapter.official_model_support())
        .unwrap_or(OfficialModelSupport::UNVERIFIED);

    let mut native: Vec<String> = declared
        .native_families
        .value()
        .map(|families| families.iter().map(|f| (*f).to_owned()).collect())
        .unwrap_or_default();
    let mut supported: Vec<String> = declared
        .supported_models
        .value()
        .map(|models| models.iter().map(|m| (*m).to_owned()).collect())
        .unwrap_or_default();

    let mut corrected = false;
    if let Some(correction) = overrides.harness(harness) {
        if let Some(families) = &correction.native_families {
            native = families.clone();
            corrected = true;
        }
        if let Some(models) = &correction.supported_models {
            supported = models.clone();
            corrected = true;
        }
    }
    (native, supported, corrected)
}

/// What Glasshouse believes about `id`, after any user correction.
fn attribute(id: &str, overrides: &PairingOverrides) -> ModelAttribution {
    let mut attribution = match catalogued(id) {
        Some(entry) => ModelAttribution {
            developer: ModelDeveloper::named(entry.developer),
            family: Some(entry.family.to_owned()),
            behaviour: ModelBehaviourFit::Unverified,
            source: AttributionSource::Catalogue {
                evidence: entry.evidence,
            },
        },
        None => ModelAttribution::unattributed(),
    };

    if let Some(correction) = overrides.model(id) {
        let mut changed = false;
        if let Some(developer) = &correction.developer {
            attribution.developer = developer.clone();
            changed = true;
        }
        if let Some(family) = &correction.family {
            attribution.family = Some(family.clone());
            changed = true;
        }
        if let Some(behaviour) = correction.behaviour {
            attribution.behaviour = behaviour;
            changed = true;
        }
        if changed {
            attribution.source = AttributionSource::Correction {
                source: overrides.source().to_owned(),
            };
        }
    }
    attribution
}

/// Ask what the pairing between a harness and a model is.
///
/// The one function that answers it. Every rung of the ladder is a statement
/// somebody declared — an adapter's [`OfficialModelSupport`], the
/// [`catalogue`], a user's [`PairingOverrides`], a provider's protocol list —
/// and none of it is read out of a name.
///
/// The order matters and is the map's, not this module's:
///
/// 1. **vendor-native** needs both halves of line 557 — the vendor declares
///    the family as one of its own *and* the developer is that vendor's
///    organisation. Either half alone is not enough, and the second half is
///    what stops a reseller's model line being mistaken for a first-party one.
/// 2. **vendor-supported** needs only the vendor's own list, because line 558
///    is a claim by the harness vendor and stands whether or not the model's
///    developer is known.
/// 3. **unknown** for an unattributed model — line 560, and it comes *before*
///    the protocol rungs on purpose. The wire is still described, separately,
///    in [`Pairing::protocol_fit`]; what cannot be described is the
///    relationship between a harness and a model nobody can name the author
///    of.
/// 4. the protocol rungs, for an attributed model with no vendor
///    relationship.
pub fn classify(query: &PairingQuery, overrides: &PairingOverrides) -> Pairing {
    let adapter = super::adapter_for(query.harness);
    let harness_vendor = adapter
        .map(|adapter| adapter.describe().vendor)
        .unwrap_or(Declared::Unverified);
    let spoken = adapter
        .map(|adapter| adapter.describe().backends.protocols)
        .unwrap_or(Declared::Unverified);

    let fit = protocol_fit(spoken, query.route.protocol, &query.provider_protocols);

    let Some(id) = query.model.name() else {
        // Line 554's quietest case and the one most likely to be faked. The
        // profile named no model, so the harness's own default serves the
        // session — and Glasshouse does not know which model that is. It is
        // *not* the harness vendor's own model just because the harness is
        // that vendor's program.
        return Pairing {
            harness: query.harness,
            harness_vendor,
            model: query.model.clone(),
            attribution: ModelAttribution::unattributed(),
            route: query.route.clone(),
            class: PairingClass::Unknown,
            protocol_fit: fit,
            tool_semantics: tool_semantics(query.tool_calls),
            reason: "the launch profile names no model, so the harness's own default serves \
                     the session and Glasshouse assigned none — which model that is was not \
                     established, and the harness's publisher is not evidence of it"
                .to_owned(),
        };
    };

    let attribution = attribute(id, overrides);
    let (native_families, supported_models, support_corrected) =
        resolved_support(query.harness, overrides);

    let (class, reason) = if let (Some(family), Some(developer), Some(vendor)) = (
        attribution.family.as_deref(),
        attribution.developer.slug(),
        harness_vendor.value(),
    ) && native_families.iter().any(|declared| declared == family)
        && vendor_organisation(*vendor) == Some(developer)
    {
        (
            PairingClass::VendorNative,
            format!(
                "{} declares the `{family}` family as its own, and `{id}` was developed by \
                 {developer}{}",
                vendor.display_name(),
                if support_corrected {
                    " (support list corrected in configuration)"
                } else {
                    ""
                }
            ),
        )
    } else if supported_models.iter().any(|declared| declared == id) {
        (
            PairingClass::VendorSupported,
            format!(
                "{} explicitly supports `{id}`, whose developer is {}{}",
                harness_vendor
                    .value()
                    .map(|v| v.display_name())
                    .unwrap_or("this harness's publisher"),
                attribution.developer.label(),
                if support_corrected {
                    " (support list corrected in configuration)"
                } else {
                    ""
                }
            ),
        )
    } else if attribution.developer.is_unknown() {
        (
            PairingClass::Unknown,
            format!(
                "`{id}` is not attributed to a developer, so what this harness and this model \
                 are to one another is unknown; Glasshouse will not read a developer out of a \
                 model's name, a provider's name, or a harness's branding"
            ),
        )
    } else {
        let developer = attribution.developer.label();
        match fit {
            ProtocolFit::Native => (
                PairingClass::ProtocolNative,
                format!(
                    "no vendor relationship — `{id}` is {developer}'s — but the route's \
                     protocol is one this harness speaks itself"
                ),
            ),
            ProtocolFit::Compatible => (
                PairingClass::ProtocolCompatible,
                format!(
                    "no vendor relationship — `{id}` is {developer}'s — and the route's \
                     protocol is not one this harness speaks, but the provider serves one \
                     that it does"
                ),
            ),
            ProtocolFit::Translated => (
                PairingClass::ProtocolTranslated,
                format!(
                    "no vendor relationship — `{id}` is {developer}'s — and an explicit \
                     translation adapter exists for this protocol pair"
                ),
            ),
            ProtocolFit::Incompatible => (
                PairingClass::Unknown,
                format!(
                    "`{id}` is {developer}'s, this harness's vendor declares no support for \
                     it, and nothing carries the route's protocol to a protocol this harness \
                     speaks"
                ),
            ),
            ProtocolFit::Unknown => (
                PairingClass::Unknown,
                format!(
                    "`{id}` is {developer}'s, this harness's vendor declares no support for \
                     it, and no wire protocol was established for either side"
                ),
            ),
        }
    };

    Pairing {
        harness: query.harness,
        harness_vendor,
        model: query.model.clone(),
        attribution,
        route: query.route.clone(),
        class,
        protocol_fit: fit,
        tool_semantics: tool_semantics(query.tool_calls),
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(harness: IntegrationId, model: &str) -> PairingQuery {
        PairingQuery {
            harness,
            model: AssignedModel::named(model),
            route: ServingRoute::default(),
            tool_calls: Declared::Unverified,
            provider_protocols: Vec::new(),
        }
    }

    fn none() -> PairingOverrides {
        PairingOverrides::default()
    }

    /// [`wire_protocol_from_slug`] round-trips every slug [`WireProtocol`]
    /// actually produces, and refuses one none of them did rather than
    /// guessing.
    #[test]
    fn wire_protocol_from_slug_round_trips_every_known_slug_and_refuses_an_unknown_one() {
        for protocol in [
            WireProtocol::AnthropicMessages,
            WireProtocol::OpenAiResponses,
            WireProtocol::OpenAiChat,
        ] {
            assert_eq!(wire_protocol_from_slug(protocol.slug()), Some(protocol));
        }
        assert_eq!(wire_protocol_from_slug("google-gemini"), None);
    }

    /// Line 557: both halves, and the second half is the one that matters.
    #[test]
    fn a_vendor_native_pairing_needs_the_family_and_the_developer() {
        let pairing = classify(&query(IntegrationId::ClaudeCode, "claude-fable-5"), &none());
        assert_eq!(pairing.class(), PairingClass::VendorNative);
        assert_eq!(pairing.developer().slug(), Some("anthropic"));
        assert_eq!(pairing.family(), Some("fable"));
    }

    /// The same family, in a harness whose vendor does not declare it. Google
    /// publishes Antigravity; `sonnet` is not a family Antigravity declares
    /// as its own, so this is not vendor-native however Anthropic-shaped the
    /// name is.
    #[test]
    fn another_vendors_model_is_never_vendor_native_however_its_name_reads() {
        let pairing = classify(
            &query(IntegrationId::Antigravity, "claude-sonnet-4-6"),
            &none(),
        );
        assert_ne!(pairing.class(), PairingClass::VendorNative);
        assert_eq!(pairing.class(), PairingClass::VendorSupported);
    }

    /// Line 557's second half, which a family list alone cannot enforce: a
    /// model that *calls itself* part of a vendor's family, developed by
    /// somebody else, is not a first-party pairing. Nothing in a name gets
    /// to promote a model into a vendor's own line.
    #[test]
    fn a_family_name_alone_does_not_make_a_pairing_vendor_native() {
        let mut models = BTreeMap::new();
        models.insert(
            "acme/gemini-clone-1".to_owned(),
            ModelCorrection {
                developer: Some(ModelDeveloper::named("acme")),
                family: Some("gemini".to_owned()),
                behaviour: None,
            },
        );
        let overrides =
            PairingOverrides::from_parts("the user configuration file", models, BTreeMap::new());

        let pairing = classify(
            &query(IntegrationId::Antigravity, "acme/gemini-clone-1"),
            &overrides,
        );
        assert_eq!(pairing.family(), Some("gemini"));
        assert_ne!(
            pairing.class(),
            PairingClass::VendorNative,
            "Antigravity declares `gemini` as its own family, but acme is not Google and this \
             is not a first-party pairing"
        );
        assert_eq!(pairing.class(), PairingClass::Unknown);
    }

    /// Line 558: the harness vendor's own statement, and it does not need to
    /// know who wrote the weights. `gpt-oss-120b-medium` is in Antigravity's
    /// own model list and in nobody's attribution.
    #[test]
    fn vendor_supported_stands_without_an_attributed_developer() {
        let pairing = classify(
            &query(IntegrationId::Antigravity, "gpt-oss-120b-medium"),
            &none(),
        );
        assert_eq!(pairing.class(), PairingClass::VendorSupported);
        assert!(
            pairing.developer().is_unknown(),
            "a vendor's support list says nothing about who developed the model, and must not \
             be allowed to fill the developer in: {:?}",
            pairing.developer()
        );
    }

    /// Line 560, and the mutation this phase exists to fail on: an
    /// unattributed model must not answer `vendor-native`, and must not be
    /// promoted by the wire either.
    #[test]
    fn an_unattributed_model_is_unknown_even_on_the_harnesss_own_wire() {
        let mut q = query(IntegrationId::ClaudeCode, "stealth-alpha");
        q.route.provider = Some("openrouter".to_owned());
        q.route.protocol = Some(WireProtocol::AnthropicMessages);
        let pairing = classify(&q, &none());

        assert_eq!(pairing.class(), PairingClass::Unknown);
        assert!(pairing.developer().is_unknown());
        assert_eq!(pairing.family(), None);
        // The wire is still described. The two answers are separate, and the
        // separation is line 559.
        assert_eq!(pairing.protocol_fit(), ProtocolFit::Native);
    }

    /// Line 560 again, in the form that actually shows up: a model whose name
    /// carries a company's. Nothing may read `anthropic/` as an attribution.
    #[test]
    fn a_model_named_after_a_company_is_not_attributed_to_it() {
        let pairing = classify(
            &query(IntegrationId::ClaudeCode, "anthropic/claude-fable-5"),
            &none(),
        );
        assert!(
            pairing.developer().is_unknown(),
            "a routing prefix is a name, not an attribution: {:?}",
            pairing.developer()
        );
        assert_eq!(pairing.class(), PairingClass::Unknown);
    }

    /// Line 555, from the other side: the serving provider is stored, and it
    /// is never the answer to who developed the model.
    #[test]
    fn the_serving_provider_never_becomes_the_developer() {
        let mut q = query(IntegrationId::ClaudeCode, "unlisted-model-v1");
        q.route.provider = Some("anthropic".to_owned());
        q.route.protocol = Some(WireProtocol::AnthropicMessages);
        let pairing = classify(&q, &none());

        assert_eq!(pairing.route().provider.as_deref(), Some("anthropic"));
        assert!(
            pairing.developer().is_unknown(),
            "a provider called `anthropic` is a service, not an author: {:?}",
            pairing.developer()
        );
        assert_eq!(pairing.class(), PairingClass::Unknown);
    }

    /// The same model, reached two ways. The class is a fact about the
    /// harness and the model; the route is stored beside it and does not move
    /// the class — which is line 554's independence made observable.
    #[test]
    fn a_reseller_in_front_of_a_native_model_does_not_change_the_class() {
        let direct = classify(&query(IntegrationId::ClaudeCode, "claude-fable-5"), &none());

        let mut resold = query(IntegrationId::ClaudeCode, "claude-fable-5");
        resold.route.provider = Some("openrouter".to_owned());
        resold.route.gateway = Some("glasshouse".to_owned());
        resold.route.protocol = Some(WireProtocol::AnthropicMessages);
        let resold = classify(&resold, &none());

        assert_eq!(direct.class(), PairingClass::VendorNative);
        assert_eq!(resold.class(), PairingClass::VendorNative);
        assert_eq!(resold.route().provider.as_deref(), Some("openrouter"));
        assert_eq!(resold.route().gateway.as_deref(), Some("glasshouse"));
        assert_eq!(resold.developer().slug(), Some("anthropic"));
    }

    /// Line 559: three axes, and they disagree. Every built-in provider
    /// template declares tool calls `Unverified`, so a pairing on a harness's
    /// own wire is `Native` on one axis and unestablished on the other two.
    #[test]
    fn the_three_compatibility_axes_are_answered_separately() {
        let mut q = query(IntegrationId::ClaudeCode, "claude-fable-5");
        q.route.protocol = Some(WireProtocol::AnthropicMessages);
        q.tool_calls = Declared::Unverified;
        let pairing = classify(&q, &none());

        assert_eq!(pairing.protocol_fit(), ProtocolFit::Native);
        assert_eq!(pairing.model_behaviour(), ModelBehaviourFit::Unverified);
        assert_eq!(pairing.tool_semantics(), ToolSemantics::Unverified);
    }

    /// And they disagree in the other direction too: a provider that is known
    /// *not* to carry tool calls on a protocol the harness speaks natively.
    /// A single "compatible" verdict could not say this.
    #[test]
    fn a_native_protocol_does_not_make_tool_calls_or_behaviour_verified() {
        let mut q = query(IntegrationId::ClaudeCode, "claude-fable-5");
        q.route.protocol = Some(WireProtocol::AnthropicMessages);
        q.tool_calls = Declared::verified(false, "the provider's own documentation says so");
        let pairing = classify(&q, &none());

        assert_eq!(pairing.protocol_fit(), ProtocolFit::Native);
        assert_eq!(pairing.tool_semantics(), ToolSemantics::KnownAbsent);
        assert_eq!(pairing.model_behaviour(), ModelBehaviourFit::Unverified);
    }

    /// The protocol rungs, for an attributed model with no vendor
    /// relationship. Codex speaks OpenAI Responses; a provider serving only
    /// OpenAI chat completions is compatible by way of nothing here, but a
    /// provider that also serves Responses is.
    #[test]
    fn the_protocol_rungs_separate_native_from_compatible_from_incompatible() {
        let mut native = query(IntegrationId::Codex, "claude-fable-5");
        native.route.protocol = Some(WireProtocol::OpenAiResponses);
        assert_eq!(
            classify(&native, &none()).class(),
            PairingClass::ProtocolNative
        );

        let mut compatible = query(IntegrationId::Codex, "claude-fable-5");
        compatible.route.protocol = Some(WireProtocol::OpenAiChat);
        compatible.provider_protocols =
            vec![WireProtocol::OpenAiChat, WireProtocol::OpenAiResponses];
        let compatible = classify(&compatible, &none());
        assert_eq!(compatible.protocol_fit(), ProtocolFit::Compatible);
        assert_eq!(compatible.class(), PairingClass::ProtocolCompatible);

        let mut incompatible = query(IntegrationId::Codex, "claude-fable-5");
        incompatible.route.protocol = Some(WireProtocol::AnthropicMessages);
        incompatible.provider_protocols = vec![WireProtocol::AnthropicMessages];
        let incompatible = classify(&incompatible, &none());
        assert_eq!(incompatible.protocol_fit(), ProtocolFit::Incompatible);
        assert_eq!(incompatible.class(), PairingClass::Unknown);
    }

    /// V1 translates nothing, and the class that would describe a translation
    /// is unreachable because of that rather than because it is missing. If
    /// this ever fails, an adapter was added and this phase's report is what
    /// should be re-read.
    #[test]
    fn no_pair_of_protocols_is_translated_today() {
        for from in [
            WireProtocol::AnthropicMessages,
            WireProtocol::OpenAiResponses,
            WireProtocol::OpenAiChat,
        ] {
            for to in [
                WireProtocol::AnthropicMessages,
                WireProtocol::OpenAiResponses,
                WireProtocol::OpenAiChat,
            ] {
                assert!(
                    !crate::provider::translation_available(from, to),
                    "{from} -> {to} claims a translation adapter"
                );
            }
        }
    }

    /// The translation seam is *asked*, not assumed absent. A classifier that
    /// hard-coded "nothing translates" would pass every other test in this
    /// file and would silently ignore the first adapter anyone adds.
    #[test]
    fn the_classifier_asks_the_one_function_that_owns_translation() {
        let code = include_str!("pairing.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("split always yields at least one part");
        assert!(
            code.contains("crate::provider::translation_available("),
            "harness/pairing.rs no longer calls provider::translation_available: the \
             protocol-translated class has stopped being decided by the seam that owns it"
        );
    }

    /// A launch profile that names no model. Nothing may fill it in from the
    /// harness's publisher.
    #[test]
    fn a_harness_default_model_is_not_the_harness_vendors_model() {
        let q = PairingQuery {
            harness: IntegrationId::ClaudeCode,
            model: AssignedModel::HarnessDefault,
            route: ServingRoute {
                provider: None,
                gateway: None,
                protocol: Some(WireProtocol::AnthropicMessages),
            },
            tool_calls: Declared::Unverified,
            provider_protocols: Vec::new(),
        };
        let pairing = classify(&q, &none());

        assert_eq!(pairing.class(), PairingClass::Unknown);
        assert!(pairing.developer().is_unknown());
        assert_eq!(
            pairing.harness_vendor().value(),
            Some(&Vendor::Anthropic),
            "the harness's publisher is still known; it is simply not an attribution"
        );
    }

    /// Line 561: a correction in configuration changes the answer, and
    /// nothing in the classifier had to be edited to make it.
    #[test]
    fn a_user_correction_attributes_a_model_the_catalogue_never_read() {
        let mut q = query(IntegrationId::ClaudeCode, "z-ai/glm-4.6");
        q.route.provider = Some("openrouter".to_owned());
        q.route.protocol = Some(WireProtocol::AnthropicMessages);

        let before = classify(&q, &none());
        assert_eq!(before.class(), PairingClass::Unknown);

        let mut models = BTreeMap::new();
        models.insert(
            "z-ai/glm-4.6".to_owned(),
            ModelCorrection {
                developer: Some(ModelDeveloper::named("zhipu-ai")),
                family: Some("glm".to_owned()),
                behaviour: None,
            },
        );
        let overrides =
            PairingOverrides::from_parts("the user configuration file", models, BTreeMap::new());

        let after = classify(&q, &overrides);
        assert_eq!(after.class(), PairingClass::ProtocolNative);
        assert_eq!(after.developer().slug(), Some("zhipu-ai"));
        assert_eq!(after.family(), Some("glm"));
        assert!(matches!(
            after.attribution().source,
            AttributionSource::Correction { .. }
        ));
    }

    /// The other half of line 561, and of 562: a harness's official support
    /// list is data, and a person can correct it when a release outruns
    /// Glasshouse.
    #[test]
    fn a_user_correction_can_add_official_support_a_release_has_not_shipped() {
        let q = query(IntegrationId::ClaudeCode, "opus");
        assert_eq!(classify(&q, &none()).class(), PairingClass::VendorNative);

        let mut harnesses = BTreeMap::new();
        harnesses.insert(
            "claude-code".to_owned(),
            SupportCorrection {
                native_families: Some(Vec::new()),
                supported_models: Some(vec!["opus".to_owned()]),
            },
        );
        let overrides =
            PairingOverrides::from_parts("the user configuration file", BTreeMap::new(), harnesses);

        let corrected = classify(&q, &overrides);
        assert_eq!(corrected.class(), PairingClass::VendorSupported);
        assert_eq!(corrected.developer().slug(), Some("anthropic"));
    }

    /// A person can record what a pairing actually did, on the axis nothing
    /// measures yet — and it must not move the other two.
    #[test]
    fn a_behaviour_correction_moves_one_axis_and_only_one() {
        let mut q = query(IntegrationId::ClaudeCode, "claude-fable-5");
        q.route.protocol = Some(WireProtocol::AnthropicMessages);

        let mut models = BTreeMap::new();
        models.insert(
            "claude-fable-5".to_owned(),
            ModelCorrection {
                developer: None,
                family: None,
                behaviour: Some(ModelBehaviourFit::KnownAbsent),
            },
        );
        let overrides = PairingOverrides::from_parts(
            "this project's configuration file",
            models,
            BTreeMap::new(),
        );

        let pairing = classify(&q, &overrides);
        assert_eq!(pairing.model_behaviour(), ModelBehaviourFit::KnownAbsent);
        assert_eq!(pairing.protocol_fit(), ProtocolFit::Native);
        assert_eq!(pairing.tool_semantics(), ToolSemantics::Unverified);
        // The class is about the vendor relationship, which a behaviour note
        // does not change.
        assert_eq!(pairing.class(), PairingClass::VendorNative);
    }

    /// Line 562, mechanically: every harness's declared support is data an
    /// adapter states with evidence, exactly like every other declaration in
    /// this module.
    #[test]
    fn every_declared_support_list_cites_its_evidence() {
        for adapter in super::super::all() {
            let support = adapter.official_model_support();
            for (what, declared) in [
                ("native families", support.native_families.evidence()),
                ("supported models", support.supported_models.evidence()),
            ] {
                if let Some(evidence) = declared {
                    assert!(
                        evidence.len() > 30,
                        "{:?}'s {what} declaration cites `{evidence}`, which is too short to \
                         be a citation anybody could re-check",
                        adapter.id()
                    );
                }
            }
        }
    }

    /// A vendor whose own model line nothing established can never produce a
    /// vendor-native pairing, whatever its adapter declares. The table is the
    /// only comparison between a harness vendor and a model developer, and it
    /// is empty for four of the seven.
    #[test]
    fn a_vendor_with_no_established_model_line_is_never_native() {
        for vendor in [Vendor::Cursor, Vendor::OpenCode, Vendor::Pi, Vendor::Hermes] {
            assert_eq!(
                vendor_organisation(vendor),
                None,
                "{vendor} claims a model-developing organisation nothing established"
            );
        }
        assert_eq!(vendor_organisation(Vendor::Anthropic), Some("anthropic"));
        assert_eq!(vendor_organisation(Vendor::OpenAi), Some("openai"));
        assert_eq!(vendor_organisation(Vendor::Google), Some("google"));
    }

    /// Cursor CLI names three models it supports and no family of its own, so
    /// its best answer is vendor-supported and never vendor-native.
    #[test]
    fn a_harness_with_no_native_family_still_reaches_vendor_supported() {
        let pairing = classify(&query(IntegrationId::Cursor, "claude-opus-4-8"), &none());
        assert_eq!(pairing.class(), PairingClass::VendorSupported);
        assert!(
            pairing.developer().is_unknown(),
            "nothing here read who developed `claude-opus-4-8`"
        );
    }

    /// Every catalogue entry is exact, and nothing in it was derived from
    /// another entry's stem.
    #[test]
    fn the_catalogue_matches_ids_exactly_and_cites_every_one() {
        for entry in catalogue() {
            assert!(
                entry.evidence.len() > 30,
                "`{}` cites `{}`, which is too short to be a citation",
                entry.id,
                entry.evidence
            );
            assert_eq!(catalogued(entry.id).map(|e| e.id), Some(entry.id));
        }
        assert!(catalogued("claude-fable-5-turbo").is_none());
        assert!(catalogued("openrouter/opus").is_none());
    }

    /// Line 572: the same nominal model through a different gateway is
    /// different evidence. A key built from two routes that differ only in
    /// `gateway` must not compare equal.
    #[test]
    fn an_evidence_key_separates_the_same_model_across_gateways() {
        let direct = ServingRoute {
            provider: Some("openrouter".to_owned()),
            ..ServingRoute::default()
        };
        let mut gatewayed = direct.clone();
        gatewayed.gateway = Some("glasshouse".to_owned());

        let a = EvidenceKey::new(
            IntegrationId::ClaudeCode,
            "default",
            AssignedModel::named("claude-fable-5"),
            direct,
        );
        let b = EvidenceKey::new(
            IntegrationId::ClaudeCode,
            "default",
            AssignedModel::named("claude-fable-5"),
            gatewayed,
        );
        assert_ne!(
            a, b,
            "the same model through a gateway must be a different evidence key"
        );
    }

    /// The same line, for a protocol translation rather than a gateway: two
    /// routes that differ only in wire protocol are different evidence too.
    #[test]
    fn an_evidence_key_separates_the_same_model_across_protocols() {
        let anthropic = ServingRoute {
            protocol: Some(WireProtocol::AnthropicMessages),
            ..ServingRoute::default()
        };
        let openai = ServingRoute {
            protocol: Some(WireProtocol::OpenAiChat),
            ..ServingRoute::default()
        };

        let a = EvidenceKey::new(
            IntegrationId::Codex,
            "default",
            AssignedModel::named("some-model"),
            anthropic,
        );
        let b = EvidenceKey::new(
            IntegrationId::Codex,
            "default",
            AssignedModel::named("some-model"),
            openai,
        );
        assert_ne!(a, b);
    }

    /// And the identical route, harness, profile and model produce an equal
    /// key — the positive case that guards against an over-eager distinction.
    #[test]
    fn an_evidence_key_is_equal_for_an_identical_route() {
        let route = ServingRoute {
            provider: Some("openrouter".to_owned()),
            gateway: None,
            protocol: Some(WireProtocol::AnthropicMessages),
        };
        let a = EvidenceKey::new(
            IntegrationId::ClaudeCode,
            "default",
            AssignedModel::named("claude-fable-5"),
            route.clone(),
        );
        let b = EvidenceKey::new(
            IntegrationId::ClaudeCode,
            "default",
            AssignedModel::named("claude-fable-5"),
            route,
        );
        assert_eq!(a, b);
    }
}
