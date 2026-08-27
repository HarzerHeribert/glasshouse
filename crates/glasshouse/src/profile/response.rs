//! Phase 9K — what a *response profile* is: communication policy, and nothing
//! else.
//!
//! # A response profile is not a [`LaunchProfile`](super::LaunchProfile)
//!
//! They share a module because a module is where related vocabulary lives, and
//! they share nothing else. A [`LaunchProfile`](super::LaunchProfile) says which harness runs,
//! against which backend, with which model and which approval mode — it can
//! refuse a session, spend a credential, and change what the agent is allowed
//! to do. A [`ResponseProfile`] says how the answer should read. It cannot
//! refuse anything, it holds no credential, and Phase 10's own architectural
//! requirement keeps the two separately represented rather than collapsed into
//! one identifier.
//!
//! The map's first fixed architectural requirement for this phase is the whole
//! of it:
//!
//! > Response profiles govern user-facing communication only and remain
//! > independent from reasoning depth, diligence, validation, permissions,
//! > safety, and tool use.
//!
//! So there is deliberately no field here for effort, no field for permission
//! mode, and no field for tool access. Those exist elsewhere in Glasshouse —
//! [`ApprovalSelection`](super::ApprovalSelection) is the permission one — and
//! a response profile that could set them would be the collapse the
//! requirement forbids.
//!
//! # Five axes, because the map says five, and they are independent
//!
//! Lines 588–592 name verbosity, audience, progress narration, evidence
//! presentation and final-answer format, each *independently*. They are five
//! fields of five types, and the independence is structural rather than
//! promised:
//!
//! - no `From`/`Into` exists between any two of them, so a value of one can
//!   never be assigned to another;
//! - [`ResponseProfile`]'s fields are private and every setter takes exactly
//!   one axis, so there is no constructor that derives one from another;
//! - [`ResponseProfile::directives`] contributes one sentence per axis from a
//!   function that reads only that axis.
//!
//! Phase 9J's three compatibility axes are the precedent, and the mutation
//! that matters is the same one: a build in which one axis quietly sets
//! another is killed by `the_five_dimensions_are_independent`.
//!
//! # Concision never reduces diagnostics
//!
//! The second fixed requirement says a response profile must not *"use
//! concision to suppress diagnostics, evidence, or verification"*, and line
//! 594 spells out what a concise preset still owes: changed files,
//! verification, risks and blockers.
//!
//! That is enforced by making it unable to vary. [`REQUIRED_REPORTS`] is a
//! constant, [`ResponseProfile::required_reports`] returns it without reading
//! `self`, and [`ResponseProfile::directives`] appends
//! [`floor_directive`] to *every* profile it renders,
//! whatever the five axes say. There is no combination of the 4 × 3 × 3 × 3 ×
//! 3 = 324 that can drop it, and
//! `every_profile_reports_changed_files_verification_risks_and_blockers`
//! enumerates all 324 rather than sampling.
//!
//! A sentence in a prompt would have been the other way to do this, and it is
//! the way the requirement was written to prevent.
//!
//! # This module imports no configuration and no adapter
//!
//! The same rule, and the same reason, as [`mod@super`] and
//! [`mod@crate::harness::pairing`]: the caller reads configuration, asks the
//! adapter, and hands the resolved values in. [`resolve`] is a pure function
//! of the layers it is given — no file, no environment, no ambient lookup —
//! and `crate::config::response` is the caller that rule assumes.
//!
//! In particular nothing here knows the word "output style". That vocabulary
//! belongs to one harness, it reaches Glasshouse through
//! [`crate::harness::response`], and line 603 requires it to stay an adapter
//! example rather than becoming a universal Glasshouse concept.

use std::fmt;

/// How much the answer says.
///
/// Deliberately *not* how much work was done, how much was checked, or how
/// much of it is reported as evidence — see [`EvidenceDetail`], which is a
/// separate axis for exactly that reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Verbosity {
    Terse,
    Concise,
    Standard,
    Elaborate,
}

/// Who the answer is written for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Audience {
    Plain,
    Technical,
    Executive,
}

/// How much is said *while* the work happens.
///
/// Progress narration is the axis concision is normally aimed at, and it is
/// the one that must never take diagnostics with it: [`Narration::Silent`]
/// suppresses routine step-by-step commentary and changes nothing about what
/// the final answer owes — see [`REQUIRED_REPORTS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Narration {
    Silent,
    Milestones,
    Detailed,
}

/// How much of the supporting evidence is *presented*.
///
/// Presentation only. Nothing on this axis changes what was verified, what was
/// checked, or what is known — a profile cannot ask for less checking, because
/// checking is not communication and this type is communication policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EvidenceDetail {
    Minimal,
    Standard,
    Audit,
}

/// The shape of the final answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AnswerFormat {
    Prose,
    Bullets,
    ChangeSummary,
}

/// One axis of a response profile, for a renderer that wants to print all
/// five without naming each type.
///
/// Five variants, one per axis, and no variant that means "all of them": a
/// caller that wants to set every axis sets five, which is what keeps a
/// partial layer honest about what it actually specified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Dimension {
    Verbosity,
    Audience,
    Narration,
    Evidence,
    Format,
}

impl Dimension {
    pub const ALL: [Dimension; 5] = [
        Dimension::Verbosity,
        Dimension::Audience,
        Dimension::Narration,
        Dimension::Evidence,
        Dimension::Format,
    ];

    /// The configuration key and CLI flag name for this axis.
    pub fn slug(self) -> &'static str {
        match self {
            Dimension::Verbosity => "verbosity",
            Dimension::Audience => "audience",
            Dimension::Narration => "narration",
            Dimension::Evidence => "evidence",
            Dimension::Format => "format",
        }
    }
}

impl fmt::Display for Dimension {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.slug())
    }
}

macro_rules! axis {
    ($ty:ident, $($variant:ident => $slug:literal),+ $(,)?) => {
        impl $ty {
            /// Every value, in the order the capability map lists them.
            pub const ALL: &'static [$ty] = &[$($ty::$variant),+];

            pub fn slug(self) -> &'static str {
                match self {
                    $($ty::$variant => $slug),+
                }
            }

            /// The value `slug` names, or `None`.
            ///
            /// `None` rather than a default: a configuration value this build
            /// does not understand is reported and ignored, never silently
            /// turned into something else — the rule
            /// `RoutingConfig`'s stale free-resource pin already follows.
            pub fn from_slug(slug: &str) -> Option<Self> {
                match slug {
                    $($slug => Some($ty::$variant),)+
                    _ => None,
                }
            }
        }

        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.pad(self.slug())
            }
        }
    };
}

axis!(
    Verbosity,
    Terse => "terse",
    Concise => "concise",
    Standard => "standard",
    Elaborate => "elaborate",
);
axis!(
    Audience,
    Plain => "plain",
    Technical => "technical",
    Executive => "executive",
);
axis!(
    Narration,
    Silent => "silent",
    Milestones => "milestones",
    Detailed => "detailed",
);
axis!(
    EvidenceDetail,
    Minimal => "minimal",
    Standard => "standard",
    Audit => "audit",
);
axis!(
    AnswerFormat,
    Prose => "prose",
    Bullets => "bullets",
    ChangeSummary => "change-summary",
);

/// What every response profile reports, whatever its five axes say.
///
/// Line 594 names these four for the concise-technical preset. They are not a
/// property of that preset: a profile that could drop them on any other
/// setting would be using concision to suppress diagnostics, which the phase's
/// second fixed architectural requirement forbids outright.
///
/// This is a `const`, and [`ResponseProfile::required_reports`] returns it
/// without reading `self`, so *there is no code path by which any axis reduces
/// it*. That is the point: the requirement asked for something the type system
/// or the preset table makes hard to violate, rather than a sentence in a
/// prompt that a terse setting could argue with.
pub const REQUIRED_REPORTS: [&str; 4] = ["changed files", "verification", "risks", "blockers"];

/// The sentence that carries [`REQUIRED_REPORTS`] into an instruction, plus
/// the standing prohibition the map's second fixed requirement states.
///
/// Public because `crate::harness::response` renders it into whatever additive
/// mechanism an adapter declares, and a second copy of this text somewhere
/// else is a second thing that can disagree with the constant above.
pub fn floor_directive() -> String {
    format!(
        "Whatever the settings above ask for, always report {}. Concision governs \
         presentation only: never let it reduce analysis, verification, diagnostics, error \
         reporting, or checkpoint completeness.",
        list(&REQUIRED_REPORTS)
    )
}

/// `["a", "b", "c"]` as `a, b and c`.
fn list(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Communication policy: five independent axes and nothing else.
///
/// Fields are private and each accessor answers exactly one axis, so no caller
/// can read one out of another and no constructor can derive one from another.
/// See the module documentation for why that is structural rather than a
/// convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResponseProfile {
    verbosity: Verbosity,
    audience: Audience,
    narration: Narration,
    evidence: EvidenceDetail,
    format: AnswerFormat,
}

impl ResponseProfile {
    pub const fn new(
        verbosity: Verbosity,
        audience: Audience,
        narration: Narration,
        evidence: EvidenceDetail,
        format: AnswerFormat,
    ) -> Self {
        Self {
            verbosity,
            audience,
            narration,
            evidence,
            format,
        }
    }

    pub fn verbosity(&self) -> Verbosity {
        self.verbosity
    }

    pub fn audience(&self) -> Audience {
        self.audience
    }

    pub fn narration(&self) -> Narration {
        self.narration
    }

    pub fn evidence(&self) -> EvidenceDetail {
        self.evidence
    }

    pub fn format(&self) -> AnswerFormat {
        self.format
    }

    /// What this profile reports no matter what.
    ///
    /// Takes `&self` and ignores it, on purpose — see [`REQUIRED_REPORTS`].
    /// A build in which this consults an axis is killed by
    /// `no_axis_can_reduce_the_required_reports`.
    pub fn required_reports(&self) -> &'static [&'static str] {
        &REQUIRED_REPORTS
    }

    /// The value each axis holds, for a renderer.
    pub fn axes(&self) -> [(Dimension, &'static str); 5] {
        [
            (Dimension::Verbosity, self.verbosity.slug()),
            (Dimension::Audience, self.audience.slug()),
            (Dimension::Narration, self.narration.slug()),
            (Dimension::Evidence, self.evidence.slug()),
            (Dimension::Format, self.format.slug()),
        ]
    }

    /// This profile as communication instructions, one sentence per axis and
    /// then the floor.
    ///
    /// Every sentence here is about how to *say* things. None of them mentions
    /// effort, permissions, tools or how much work to do, and
    /// `directives_never_mention_effort_permissions_or_tools` fails on a build
    /// where one starts to.
    ///
    /// The floor is last and unconditional: see [`REQUIRED_REPORTS`].
    pub fn directives(&self) -> Vec<String> {
        vec![
            verbosity_directive(self.verbosity).to_owned(),
            audience_directive(self.audience).to_owned(),
            narration_directive(self.narration).to_owned(),
            evidence_directive(self.evidence).to_owned(),
            format_directive(self.format).to_owned(),
            floor_directive(),
        ]
    }

    /// The instructions as one block of text.
    pub fn instruction(&self) -> String {
        self.directives().join(" ")
    }
}

impl Default for ResponseProfile {
    /// The `standard` preset — see [`presets`].
    fn default() -> Self {
        Self::new(
            Verbosity::Standard,
            Audience::Technical,
            Narration::Milestones,
            EvidenceDetail::Standard,
            AnswerFormat::Prose,
        )
    }
}

/// One sentence about verbosity, reading only the verbosity axis.
///
/// Each of the five below takes exactly its own axis's type as its only
/// argument. That is the independence requirement expressed where a compiler
/// can hold it: `verbosity_directive` *cannot* consult narration, because it
/// was never given one.
fn verbosity_directive(value: Verbosity) -> &'static str {
    match value {
        Verbosity::Terse => "Answer in as few words as carry the meaning.",
        Verbosity::Concise => "Lead with the outcome and keep the answer short.",
        Verbosity::Standard => "Answer at ordinary length.",
        Verbosity::Elaborate => "Explain the reasoning and the alternatives considered.",
    }
}

fn audience_directive(value: Audience) -> &'static str {
    match value {
        Audience::Plain => "Write for a reader who does not know this codebase.",
        Audience::Technical => "Write for an engineer working in this codebase.",
        Audience::Executive => "Write for a reader who needs the decision and its consequences.",
    }
}

fn narration_directive(value: Narration) -> &'static str {
    match value {
        Narration::Silent => "Do not narrate routine steps while working.",
        Narration::Milestones => "Say what you are doing at each milestone.",
        Narration::Detailed => "Narrate each step as you take it.",
    }
}

fn evidence_directive(value: EvidenceDetail) -> &'static str {
    match value {
        EvidenceDetail::Minimal => "Present supporting evidence only where it is asked for.",
        EvidenceDetail::Standard => "Present the evidence a reader needs to check the answer.",
        EvidenceDetail::Audit => "Present every command run and every result observed.",
    }
}

fn format_directive(value: AnswerFormat) -> &'static str {
    match value {
        AnswerFormat::Prose => "Give the final answer as prose.",
        AnswerFormat::Bullets => "Give the final answer as bullets.",
        AnswerFormat::ChangeSummary => "Give the final answer as a summary of what changed.",
    }
}

/// A named combination of the five axes.
///
/// Line 593: presets exist so a user can say `concise-technical` once instead
/// of five values, *without forcing every harness to expose the same native
/// vocabulary*. So a preset holds five Glasshouse axis values and nothing
/// else: there is no field here for a harness's own style name, and no way to
/// add one without changing this type. Translating a preset into whatever one
/// harness calls it is [`crate::harness::response`]'s job — line 603.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preset {
    pub name: &'static str,
    pub description: &'static str,
    pub profile: ResponseProfile,
}

/// Every named preset.
///
/// `concise-technical` is required by line 594 and is the one whose wording
/// the map dictates: it *"leads with outcomes, suppresses routine narration,
/// and still reports changed files, verification, risks, and blockers"*. The
/// first two clauses are its `Concise` verbosity and `Silent` narration; the
/// third is not a property of this row at all — it is [`REQUIRED_REPORTS`],
/// which every row carries because no row can do otherwise.
pub fn presets() -> &'static [Preset] {
    const PRESETS: &[Preset] = &[
        Preset {
            name: "concise-technical",
            description: "leads with outcomes and suppresses routine narration, for an engineer \
                          working in this codebase",
            profile: ResponseProfile::new(
                Verbosity::Concise,
                Audience::Technical,
                Narration::Silent,
                EvidenceDetail::Standard,
                AnswerFormat::ChangeSummary,
            ),
        },
        Preset {
            name: "standard",
            description: "ordinary length, milestone narration, prose",
            profile: ResponseProfile::new(
                Verbosity::Standard,
                Audience::Technical,
                Narration::Milestones,
                EvidenceDetail::Standard,
                AnswerFormat::Prose,
            ),
        },
        Preset {
            name: "audit",
            description: "every command and result presented, for reviewing work after the fact",
            profile: ResponseProfile::new(
                Verbosity::Standard,
                Audience::Technical,
                Narration::Detailed,
                EvidenceDetail::Audit,
                AnswerFormat::ChangeSummary,
            ),
        },
        Preset {
            name: "explainer",
            description: "reasoning and alternatives, written for a reader new to this codebase",
            profile: ResponseProfile::new(
                Verbosity::Elaborate,
                Audience::Plain,
                Narration::Milestones,
                EvidenceDetail::Standard,
                AnswerFormat::Prose,
            ),
        },
        Preset {
            name: "brief",
            description: "as few words as carry the meaning, decision first",
            profile: ResponseProfile::new(
                Verbosity::Terse,
                Audience::Executive,
                Narration::Silent,
                EvidenceDetail::Minimal,
                AnswerFormat::Bullets,
            ),
        },
    ];
    PRESETS
}

/// The preset `name` names, or `None`.
pub fn preset(name: &str) -> Option<&'static Preset> {
    presets().iter().find(|preset| preset.name == name)
}

/// Every preset name, for an error message that lists the real ones.
pub fn preset_names() -> String {
    presets()
        .iter()
        .map(|preset| preset.name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// What a session is *for*, which is not what it is allowed to do.
///
/// Line 595 asks for separate defaults for these five. A role is communication
/// context only — an `Orchestrator` session is not granted anything a `Worker`
/// session is not, and nothing on this enum reaches
/// [`ApprovalSelection`](super::ApprovalSelection).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    Orchestrator,
    Worker,
    Reviewer,
    Explainer,
    Interactive,
}

impl Role {
    pub const ALL: [Role; 5] = [
        Role::Orchestrator,
        Role::Worker,
        Role::Reviewer,
        Role::Explainer,
        Role::Interactive,
    ];

    pub fn slug(self) -> &'static str {
        match self {
            Role::Orchestrator => "orchestrator",
            Role::Worker => "worker",
            Role::Reviewer => "reviewer",
            Role::Explainer => "explainer",
            Role::Interactive => "interactive",
        }
    }

    pub fn from_slug(slug: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|role| role.slug() == slug)
    }

    /// The preset this role gets when nothing else says otherwise.
    ///
    /// Five separate answers, and `every_role_has_its_own_default` fails on a
    /// build where two roles are made to share one by a change that was meant
    /// to be tidying. Line 595 asks for *separate* defaults; a table where
    /// every row returns the same preset would satisfy the type and not the
    /// line.
    pub fn default_preset(self) -> &'static str {
        match self {
            Role::Orchestrator => "concise-technical",
            Role::Worker => "audit",
            Role::Reviewer => "standard",
            Role::Explainer => "explainer",
            Role::Interactive => "standard",
        }
    }

    pub fn names() -> String {
        Self::ALL
            .iter()
            .map(|role| role.slug())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.slug())
    }
}

/// Where a resolved axis value came from.
///
/// The six of line 596, in the order that line states them, highest priority
/// first. [`PrecedenceLayer::ALL`] is that order, [`resolve`] is the only
/// thing that walks it, and a build that skips one of the six is killed by
/// six separate mutations — one per layer.
///
/// Deliberately not [`crate::config::Layer`], which has three variants and
/// answers a different question ("which file recorded this"). Two of these
/// six — [`PrecedenceLayer::Project`] and [`PrecedenceLayer::UserDefault`] —
/// happen to correspond to that type's two file layers; the other four have no
/// file at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PrecedenceLayer {
    TaskOverride,
    Session,
    Role,
    Project,
    UserDefault,
    HarnessDefault,
}

impl PrecedenceLayer {
    /// Line 596's order, highest priority first.
    pub const ALL: [PrecedenceLayer; 6] = [
        PrecedenceLayer::TaskOverride,
        PrecedenceLayer::Session,
        PrecedenceLayer::Role,
        PrecedenceLayer::Project,
        PrecedenceLayer::UserDefault,
        PrecedenceLayer::HarnessDefault,
    ];

    pub fn slug(self) -> &'static str {
        match self {
            PrecedenceLayer::TaskOverride => "task override",
            PrecedenceLayer::Session => "session",
            PrecedenceLayer::Role => "role",
            PrecedenceLayer::Project => "project",
            PrecedenceLayer::UserDefault => "user default",
            PrecedenceLayer::HarnessDefault => "harness default",
        }
    }
}

impl fmt::Display for PrecedenceLayer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.slug())
    }
}

/// What one precedence layer says, which may be nothing.
///
/// Every axis is optional and independent: a project that wants silent
/// narration and nothing else records one axis and inherits four. That is why
/// resolution is per axis rather than per profile — a whole-profile precedence
/// would make "set one thing" mean "restate five".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProfileLayer {
    /// A named preset, supplying all five axes at this layer.
    pub preset: Option<&'static Preset>,
    pub verbosity: Option<Verbosity>,
    pub audience: Option<Audience>,
    pub narration: Option<Narration>,
    pub evidence: Option<EvidenceDetail>,
    pub format: Option<AnswerFormat>,
}

impl ProfileLayer {
    /// A layer that says nothing.
    pub fn empty() -> Self {
        Self::default()
    }

    /// A layer that names `preset` and nothing finer.
    pub fn from_preset(preset: &'static Preset) -> Self {
        Self {
            preset: Some(preset),
            ..Self::default()
        }
    }

    /// A layer that fixes all five axes explicitly.
    pub fn from_profile(profile: ResponseProfile) -> Self {
        Self {
            preset: None,
            verbosity: Some(profile.verbosity()),
            audience: Some(profile.audience()),
            narration: Some(profile.narration()),
            evidence: Some(profile.evidence()),
            format: Some(profile.format()),
        }
    }

    /// Whether this layer says nothing at all.
    pub fn is_empty(&self) -> bool {
        self.preset.is_none()
            && self.verbosity.is_none()
            && self.audience.is_none()
            && self.narration.is_none()
            && self.evidence.is_none()
            && self.format.is_none()
    }

    /// What this layer says about `dimension`, as a slug, or `None`.
    ///
    /// An explicit axis beats this layer's own preset, because within one
    /// layer the finer statement is the more specific one and a user who wrote
    /// both meant the one they spelled out.
    fn says(&self, dimension: Dimension) -> Option<&'static str> {
        let explicit = match dimension {
            Dimension::Verbosity => self.verbosity.map(Verbosity::slug),
            Dimension::Audience => self.audience.map(Audience::slug),
            Dimension::Narration => self.narration.map(Narration::slug),
            Dimension::Evidence => self.evidence.map(EvidenceDetail::slug),
            Dimension::Format => self.format.map(AnswerFormat::slug),
        };
        explicit.or_else(|| {
            self.preset.map(|preset| match dimension {
                Dimension::Verbosity => preset.profile.verbosity.slug(),
                Dimension::Audience => preset.profile.audience.slug(),
                Dimension::Narration => preset.profile.narration.slug(),
                Dimension::Evidence => preset.profile.evidence.slug(),
                Dimension::Format => preset.profile.format.slug(),
            })
        })
    }
}

/// The six layers of line 596, in that order, ready to be resolved.
///
/// A fixed-size array rather than a `Vec`, so a caller cannot hand [`resolve`]
/// five layers or seven, and so the mapping from index to
/// [`PrecedenceLayer`] cannot drift.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrecedenceStack {
    layers: [ProfileLayer; 6],
}

impl PrecedenceStack {
    /// A stack in which every layer says nothing.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Record what `layer` says.
    pub fn set(&mut self, layer: PrecedenceLayer, value: ProfileLayer) -> &mut Self {
        self.layers[Self::index(layer)] = value;
        self
    }

    pub fn get(&self, layer: PrecedenceLayer) -> &ProfileLayer {
        &self.layers[Self::index(layer)]
    }

    fn index(layer: PrecedenceLayer) -> usize {
        PrecedenceLayer::ALL
            .iter()
            .position(|candidate| *candidate == layer)
            .expect("PrecedenceLayer::ALL is total over PrecedenceLayer")
    }
}

/// One axis, its value, and which of the six layers supplied it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedAxis {
    pub dimension: Dimension,
    pub value: &'static str,
    pub source: PrecedenceLayer,
}

/// A resolved response profile, and the layer each of its five axes came from.
///
/// The per-axis sources are the point. A resolution that answered only "the
/// profile is concise-technical" could not tell a user *why*, and line 596 is
/// a chain whose whole content is which layer wins — so the chain has to be
/// visible in the answer, not only in the code that produced it.
///
/// # One representation, not two
///
/// The five *values* live in [`ResolvedProfile::profile`] and nowhere else;
/// [`ResolvedProfile::axes`] reads them back out of it and pairs each with its
/// source. An earlier shape of this type stored the values a second time
/// alongside the sources, and a surviving mutation showed what that costs: a
/// build in which `ResponseProfile::new` quietly forced one axis from another
/// printed the *stored* value in the report and shipped the *mutated* value to
/// the harness, and nothing could tell. There is now no second copy to
/// disagree with the first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedProfile {
    profile: ResponseProfile,
    sources: [(Dimension, PrecedenceLayer); 5],
}

impl ResolvedProfile {
    pub fn profile(&self) -> ResponseProfile {
        self.profile
    }

    /// Each axis's resolved value and the layer that supplied it.
    ///
    /// The value comes from [`ResolvedProfile::profile`] — see this type's
    /// documentation for why that is not an implementation detail.
    pub fn axes(&self) -> [ResolvedAxis; 5] {
        let values = self.profile.axes();
        let mut resolved = self.sources.map(|(dimension, source)| ResolvedAxis {
            dimension,
            value: "",
            source,
        });
        for axis in &mut resolved {
            axis.value = values
                .iter()
                .find(|(dimension, _)| *dimension == axis.dimension)
                .expect("ResponseProfile::axes is total over Dimension")
                .1;
        }
        resolved
    }

    pub fn source_of(&self, dimension: Dimension) -> PrecedenceLayer {
        self.sources
            .iter()
            .find(|(candidate, _)| *candidate == dimension)
            .expect("every dimension is resolved")
            .1
    }

    /// Whether nothing above the harness default asked for anything.
    ///
    /// This is what keeps an unconfigured Glasshouse from injecting a
    /// communication policy nobody asked for. When it is true,
    /// [`crate::harness::response::apply`] applies nothing and says so, and
    /// the harness's own default communication behaviour stands untouched.
    pub fn is_harness_default(&self) -> bool {
        self.sources
            .iter()
            .all(|(_, source)| *source == PrecedenceLayer::HarnessDefault)
    }
}

/// Resolve `stack` into one profile, per axis, highest layer first.
///
/// Pure: it reads the stack it is given and nothing else. See the module
/// documentation for why that matters and who the caller is.
///
/// # The bottom of the chain
///
/// [`PrecedenceLayer::HarnessDefault`] is filled in from
/// [`ResponseProfile::default`] for any axis no layer specified, so resolution
/// is total — every axis always has a value and a source. An axis that reaches
/// the bottom is *reported* as having come from the harness default, which is
/// how [`ResolvedProfile::is_harness_default`] can tell a configured profile
/// from an unconfigured one.
pub fn resolve(stack: &PrecedenceStack) -> ResolvedProfile {
    let fallback = ResponseProfile::default();

    let mut resolved: Vec<ResolvedAxis> = Vec::with_capacity(5);
    for dimension in Dimension::ALL {
        let mut answer = None;
        for layer in PrecedenceLayer::ALL {
            if let Some(value) = stack.get(layer).says(dimension) {
                answer = Some(ResolvedAxis {
                    dimension,
                    value,
                    source: layer,
                });
                break;
            }
        }
        resolved.push(answer.unwrap_or(ResolvedAxis {
            dimension,
            value: match dimension {
                Dimension::Verbosity => fallback.verbosity.slug(),
                Dimension::Audience => fallback.audience.slug(),
                Dimension::Narration => fallback.narration.slug(),
                Dimension::Evidence => fallback.evidence.slug(),
                Dimension::Format => fallback.format.slug(),
            },
            source: PrecedenceLayer::HarnessDefault,
        }));
    }

    let axes: [ResolvedAxis; 5] = resolved
        .try_into()
        .expect("one resolved axis per Dimension::ALL");

    let slug_of = |dimension: Dimension| {
        axes.iter()
            .find(|axis| axis.dimension == dimension)
            .expect("every dimension is resolved")
            .value
    };

    let profile = ResponseProfile::new(
        Verbosity::from_slug(slug_of(Dimension::Verbosity)).unwrap_or(fallback.verbosity),
        Audience::from_slug(slug_of(Dimension::Audience)).unwrap_or(fallback.audience),
        Narration::from_slug(slug_of(Dimension::Narration)).unwrap_or(fallback.narration),
        EvidenceDetail::from_slug(slug_of(Dimension::Evidence)).unwrap_or(fallback.evidence),
        AnswerFormat::from_slug(slug_of(Dimension::Format)).unwrap_or(fallback.format),
    );

    ResolvedProfile {
        profile,
        sources: axes.map(|axis| (axis.dimension, axis.source)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every one of the 4 × 3 × 3 × 3 × 3 combinations.
    fn every_profile() -> Vec<ResponseProfile> {
        let mut all = Vec::new();
        for verbosity in Verbosity::ALL {
            for audience in Audience::ALL {
                for narration in Narration::ALL {
                    for evidence in EvidenceDetail::ALL {
                        for format in AnswerFormat::ALL {
                            all.push(ResponseProfile::new(
                                *verbosity, *audience, *narration, *evidence, *format,
                            ));
                        }
                    }
                }
            }
        }
        all
    }

    #[test]
    fn every_profile_reports_changed_files_verification_risks_and_blockers() {
        let all = every_profile();
        assert_eq!(
            all.len(),
            324,
            "the five axes have 4 × 3 × 3 × 3 × 3 values"
        );
        for profile in all {
            let text = profile.instruction();
            for required in REQUIRED_REPORTS {
                assert!(
                    text.contains(required),
                    "`{required}` is missing from {profile:?}:\n{text}"
                );
            }
        }
    }

    #[test]
    fn no_axis_can_reduce_the_required_reports() {
        for profile in every_profile() {
            assert_eq!(
                profile.required_reports(),
                &REQUIRED_REPORTS,
                "{profile:?} answered a different floor"
            );
        }
    }

    #[test]
    fn the_five_dimensions_are_independent() {
        // Vary one axis across all its values, holding the other four fixed,
        // and check the other four never move. This is the mutation that
        // matters: a build where terse quietly sets narration to silent, or
        // where concise reduces the evidence axis, dies here.
        let base = ResponseProfile::default();
        for value in Verbosity::ALL {
            let profile = ResponseProfile::new(
                *value,
                base.audience(),
                base.narration(),
                base.evidence(),
                base.format(),
            );
            assert_eq!(profile.audience(), base.audience());
            assert_eq!(profile.narration(), base.narration());
            assert_eq!(profile.evidence(), base.evidence());
            assert_eq!(profile.format(), base.format());
        }
        for value in Narration::ALL {
            let profile = ResponseProfile::new(
                base.verbosity(),
                base.audience(),
                *value,
                base.evidence(),
                base.format(),
            );
            assert_eq!(profile.verbosity(), base.verbosity());
            assert_eq!(profile.evidence(), base.evidence());
        }
        for value in EvidenceDetail::ALL {
            let profile = ResponseProfile::new(
                base.verbosity(),
                base.audience(),
                base.narration(),
                *value,
                base.format(),
            );
            assert_eq!(profile.verbosity(), base.verbosity());
            assert_eq!(profile.narration(), base.narration());
        }
    }

    #[test]
    fn each_axis_contributes_its_own_sentence_and_only_its_own() {
        // A stronger statement than "the axes are separate fields": the
        // rendered text changes on exactly the axis that moved. Two profiles
        // differing only in verbosity must differ in the verbosity sentence
        // and agree on the other five lines.
        let base = ResponseProfile::default();
        let moved = ResponseProfile::new(
            Verbosity::Terse,
            base.audience(),
            base.narration(),
            base.evidence(),
            base.format(),
        );
        let a = base.directives();
        let b = moved.directives();
        assert_ne!(a[0], b[0], "the verbosity sentence must move");
        assert_eq!(&a[1..], &b[1..], "no other sentence may move with it");
    }

    #[test]
    fn directives_never_mention_effort_permissions_or_tools() {
        // The phase's first fixed architectural requirement, as a property of
        // the text that actually reaches a harness. A response profile that
        // started saying "think harder" or "skip permission prompts" would be
        // the collapse the requirement forbids.
        const FORBIDDEN: &[&str] = &[
            "effort",
            "permission",
            "tool",
            "sandbox",
            "approve",
            "think harder",
            "reasoning budget",
            "skip",
        ];
        for profile in every_profile() {
            let text = profile.instruction().to_lowercase();
            for word in FORBIDDEN {
                assert!(
                    !text.contains(word),
                    "{profile:?} mentions `{word}`, which is not communication policy:\n{text}"
                );
            }
        }
    }

    #[test]
    fn the_concise_technical_preset_is_the_one_line_594_describes() {
        let preset = preset("concise-technical").expect("line 594 requires this preset");
        assert_eq!(
            preset.profile.verbosity(),
            Verbosity::Concise,
            "it must lead with outcomes"
        );
        assert_eq!(
            preset.profile.narration(),
            Narration::Silent,
            "it must suppress routine narration"
        );
        assert_eq!(
            preset.profile.audience(),
            Audience::Technical,
            "`technical` is the other half of its name"
        );
        for required in REQUIRED_REPORTS {
            assert!(
                preset.profile.instruction().contains(required),
                "it must still report {required}"
            );
        }
    }

    #[test]
    fn every_role_has_its_own_default() {
        let mut seen: Vec<&str> = Vec::new();
        for role in Role::ALL {
            let name = role.default_preset();
            assert!(
                preset(name).is_some(),
                "{role}'s default `{name}` is not a preset"
            );
            seen.push(name);
        }
        assert_eq!(seen.len(), 5);
        // Not "all five differ" — `reviewer` and `interactive` may honestly
        // share one. What line 595 forbids is one answer for all five, which
        // is a table that stopped distinguishing roles at all.
        let distinct: std::collections::BTreeSet<&str> = seen.iter().copied().collect();
        assert!(
            distinct.len() >= 4,
            "five roles collapsed onto {} preset(s): {seen:?}",
            distinct.len()
        );
    }

    #[test]
    fn precedence_runs_task_session_role_project_user_then_harness() {
        // Every layer sets verbosity; the highest must win. Then remove them
        // one at a time from the top and check the next one down takes over,
        // which is the whole of line 596 in six assertions.
        let values = [
            (PrecedenceLayer::TaskOverride, Verbosity::Terse),
            (PrecedenceLayer::Session, Verbosity::Concise),
            (PrecedenceLayer::Role, Verbosity::Standard),
            (PrecedenceLayer::Project, Verbosity::Elaborate),
            (PrecedenceLayer::UserDefault, Verbosity::Terse),
        ];
        for skip in 0..=values.len() {
            let mut stack = PrecedenceStack::empty();
            for (layer, verbosity) in &values[skip..] {
                stack.set(
                    *layer,
                    ProfileLayer {
                        verbosity: Some(*verbosity),
                        ..ProfileLayer::empty()
                    },
                );
            }
            let resolved = resolve(&stack);
            let expected = values
                .get(skip)
                .map(|(layer, _)| *layer)
                .unwrap_or(PrecedenceLayer::HarnessDefault);
            assert_eq!(
                resolved.source_of(Dimension::Verbosity),
                expected,
                "with the top {skip} layer(s) silent, {expected} should have answered"
            );
        }
    }

    #[test]
    fn one_layer_can_set_one_axis_and_inherit_the_rest() {
        let mut stack = PrecedenceStack::empty();
        stack.set(
            PrecedenceLayer::UserDefault,
            ProfileLayer::from_preset(preset("audit").unwrap()),
        );
        stack.set(
            PrecedenceLayer::Project,
            ProfileLayer {
                narration: Some(Narration::Silent),
                ..ProfileLayer::empty()
            },
        );
        let resolved = resolve(&stack);
        assert_eq!(resolved.profile().narration(), Narration::Silent);
        assert_eq!(
            resolved.source_of(Dimension::Narration),
            PrecedenceLayer::Project
        );
        assert_eq!(resolved.profile().evidence(), EvidenceDetail::Audit);
        assert_eq!(
            resolved.source_of(Dimension::Evidence),
            PrecedenceLayer::UserDefault
        );
    }

    #[test]
    fn an_explicit_axis_beats_a_preset_in_the_same_layer() {
        let mut stack = PrecedenceStack::empty();
        stack.set(
            PrecedenceLayer::UserDefault,
            ProfileLayer {
                preset: Some(preset("brief").unwrap()),
                verbosity: Some(Verbosity::Elaborate),
                ..ProfileLayer::empty()
            },
        );
        let resolved = resolve(&stack);
        assert_eq!(resolved.profile().verbosity(), Verbosity::Elaborate);
        assert_eq!(resolved.profile().format(), AnswerFormat::Bullets);
    }

    #[test]
    fn an_unconfigured_stack_is_the_harness_default_on_every_axis() {
        let resolved = resolve(&PrecedenceStack::empty());
        assert!(resolved.is_harness_default());
        for axis in resolved.axes() {
            assert_eq!(axis.source, PrecedenceLayer::HarnessDefault);
        }
    }

    #[test]
    fn one_configured_axis_is_enough_to_stop_being_the_harness_default() {
        let mut stack = PrecedenceStack::empty();
        stack.set(
            PrecedenceLayer::Project,
            ProfileLayer {
                format: Some(AnswerFormat::Bullets),
                ..ProfileLayer::empty()
            },
        );
        assert!(!resolve(&stack).is_harness_default());
    }

    #[test]
    fn an_unknown_slug_is_none_rather_than_a_default() {
        assert!(Verbosity::from_slug("chatty").is_none());
        assert!(Narration::from_slug("").is_none());
        assert!(Role::from_slug("architect").is_none());
        assert!(preset("verbose-technical").is_none());
    }
}
