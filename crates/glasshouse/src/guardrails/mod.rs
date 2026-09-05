//! Phase 21K — assumption guardrails: the few premises a change rests on,
//! **stated by the agent through the door**, recorded, tracked and surfaced.
//!
//! Capability map line 996 names a model-independent failure mode: an
//! uncertain inference silently becomes the premise of a large
//! implementation, disproven only after substantial work. **Glasshouse never
//! infers an assumption** (line 998); every record here was *said* by an
//! agent, treated as untrusted text with no column for a rationale.
//!
//! [`classify`] is a fixed, model-free ladder over the factors the agent
//! states (line 1004); trivial, local, reversible edits are never gated
//! (line 1005). [`decide`] turns the class into a [`Verdict`] — advisory by
//! default, [`Verdict::Gated`] only for `guardrails.blocking` categories
//! under `risk_gated`, every gate carrying who decided it and the override
//! that lifts it. The preflight answers with **at most three** prompts.
//!
//! [`store`] holds the state: two tables, append-only transitions, never
//! `UPDATE`d. History: design-decisions.md, "Trims: the remaining module
//! docs, second packet", guardrails/mod.rs module doc.

pub mod store;

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::session::{FileClaim, SessionId};

pub use store::{
    AssumptionId, AssumptionRecord, AssumptionStore, AssumptionView, GuardrailError, NewAssumption,
    NewTransition, Retention, Transition,
};

/// One stored spelling per variant, and both directions — the wire, the
/// database and the terminal all use the same word, so none of them can
/// drift from the others. `Serialize`/`Deserialize` go through the same
/// spelling on purpose: a value the door accepts is a value the store writes.
macro_rules! vocabulary {
    (
        $(#[$meta:meta])*
        $vis:vis enum $ty:ident {
            $( $(#[$vmeta:meta])* $variant:ident => $stored:literal ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        $vis enum $ty {
            $( $(#[$vmeta])* $variant, )+
        }

        impl $ty {
            /// Every variant, in declaration order.
            pub const ALL: &'static [Self] = &[$(Self::$variant,)+];

            /// The one spelling this value has on the wire, in the database
            /// and on a terminal.
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $stored,)+
                }
            }

            /// The inverse of [`Self::as_str`]. [`None`] is *"a spelling this
            /// build does not know"*, never a neighbouring variant.
            pub fn from_stored(value: &str) -> Option<Self> {
                match value {
                    $($stored => Some(Self::$variant),)+
                    _ => None,
                }
            }

            /// The vocabulary, for an error message.
            pub fn spellings() -> String {
                Self::ALL
                    .iter()
                    .map(|value| format!("`{}`", value.as_str()))
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        }

        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.pad(self.as_str())
            }
        }

        impl Serialize for $ty {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let value = String::deserialize(deserializer)?;
                Self::from_stored(&value).ok_or_else(|| {
                    serde::de::Error::custom(format!(
                        "`{value}` is not one of {}",
                        Self::spellings()
                    ))
                })
            }
        }
    };
}

vocabulary! {
    /// Where a task assumption stands — capability map line 1018's six
    /// states, exactly, and no seventh.
    ///
    /// The current state of an assumption is the `state` of its latest
    /// transition. Any state may follow any other: a refuted premise can be
    /// re-probed, a supported one can be refuted by later evidence. The one
    /// rule is who may write [`Self::WaivedByUser`] — see
    /// [`store::AssumptionStore::transition`].
    pub enum AssumptionState {
        /// Stated, not yet examined.
        Proposed => "proposed",
        /// Being verified — the cheapest verification step is under way.
        Probing => "probing",
        /// Direct evidence supports it.
        Supported => "supported",
        /// Direct evidence contradicts it.
        Refuted => "refuted",
        /// Verification was not available or was time-boxed out; the
        /// assumption stays an inference, labelled as one (line 1029).
        Unresolved => "unresolved",
        /// A person said not to verify it. Written only with a user origin.
        WaivedByUser => "waived_by_user",
    }
}

vocabulary! {
    /// What kind of evidence a claim rests on — capability map line 1015's
    /// six classes, kept apart so that an inference can never be recorded as
    /// an observed fact by leaving a field blank.
    pub enum EvidenceSource {
        /// Observed directly — runtime behaviour, a command's output.
        Observed => "observed",
        /// The user said so, explicitly.
        UserRequirement => "user_requirement",
        /// Read from the current repository: source, tests, configuration,
        /// schemas, primary documentation.
        Repository => "repository",
        /// Verified against an external primary source.
        External => "external",
        /// The result of a bounded experiment.
        Experiment => "experiment",
        /// Unverified inference. The class the guardrail exists for.
        Inference => "inference",
    }
}

vocabulary! {
    /// How sure the agent is, in three coarse steps. Coarse on purpose: a
    /// finer scale would be a number the agent made up.
    pub enum Uncertainty {
        Low => "low",
        Medium => "medium",
        High => "high",
    }
}

vocabulary! {
    /// What a row in `assumption_transitions` is.
    ///
    /// The ledger records four kinds of event, and only the first is a
    /// state change of an assumption. The other three are **session-level**
    /// rows — `assumption_id` is `NULL` and `session_id` is not — and they
    /// are what lets `glasshouse assumptions` show that a gate fired and
    /// which factor fired it (line 1049), that a person overrode the
    /// guardrail for a task (line 1008), and that a budget was exceeded
    /// (line 1050), without a third table.
    pub enum TransitionKind {
        /// An assumption moved to (or was re-stated in) a state.
        Transition => "transition",
        /// A preflight answered for this session; `subject` is
        /// `<risk>/<factor>/<verdict>`.
        Gate => "gate",
        /// A per-task override was recorded; `subject` is the override.
        Override => "override",
        /// A preflight found the stated budget exceeded; `subject` is the
        /// axis.
        BudgetExceeded => "budget_exceeded",
    }
}

vocabulary! {
    /// Who wrote a row. Attribution, not authentication — the same boundary
    /// `api::protocol::RequestOrigin` draws, and for the same reason: the
    /// door cannot tell a person from an orchestrator acting for them, so the
    /// caller states it, and the honest callers stop being indistinguishable.
    pub enum Origin {
        /// The agent working in the session.
        Agent => "agent",
        /// A person, or a request that said it was one.
        User => "user",
        /// Glasshouse itself — a gate it answered, a budget it compared.
        Glasshouse => "glasshouse",
    }
}

vocabulary! {
    /// The seven explicit responses to a guardrail event — capability map
    /// line 1051, in its own words and order.
    ///
    /// Recorded on a transition when the agent chooses one, so the ledger
    /// shows not only what was believed but what was done about it.
    pub enum GuardrailResponse {
        /// Read more before deciding — a read-only inspection.
        Inspect => "inspect",
        /// Go on as planned, with the assumptions as recorded.
        Continue => "continue",
        /// Run the cheapest verification step now.
        Verify => "verify",
        /// Take a checkpoint before going further.
        Checkpoint => "checkpoint",
        /// Hand the work to another session or harness.
        Handoff => "handoff",
        /// Re-plan from the premise that was refuted.
        RePlan => "re-plan",
        /// Stop and ask the person.
        Stop => "stop",
    }
}

vocabulary! {
    /// `guardrails.mode` — how a preflight's verdict is decided.
    pub enum GuardrailMode {
        /// Every preflight answers `proceed`. Nothing is recorded differently;
        /// the ledger still takes assumptions an agent states.
        Off => "off",
        /// The default (line 1052). Non-trivial changes get the prompts and
        /// the guidance; nothing blocks.
        Advisory => "advisory",
        /// A substantial change whose factor is in `guardrails.blocking`
        /// answers `gated`; every other change answers as `advisory` does.
        RiskGated => "risk_gated",
    }
}

vocabulary! {
    /// The categories that may block under [`GuardrailMode::RiskGated`] —
    /// the design ruling's three plus capability map line 1052's
    /// data-integrity policy. Nothing else can ever answer
    /// [`Verdict::Gated`], whatever the configuration says.
    pub enum BlockingCategory {
        Security => "security",
        Destructive => "destructive",
        Migration => "migration",
        DataIntegrity => "data_integrity",
    }
}

vocabulary! {
    /// `--guardrail <force|skip|lower>` — a person's decision for one task
    /// (line 1008). Recorded as a session-level row and read by every later
    /// preflight for that session, so it outranks the configured mode.
    pub enum GuardrailOverride {
        /// Gate every substantial change, whatever the mode and the
        /// blocking list say. Trivial still never gates.
        Force => "force",
        /// Waive the gate: every preflight answers `proceed`, and the
        /// session carries a `waived_by_user` row saying who did that.
        Skip => "skip",
        /// One step down: substantial answers at most `advisory`, ordinary
        /// answers `proceed`.
        Lower => "lower",
    }
}

vocabulary! {
    /// The risk class of an intended change — line 1004's classification,
    /// reduced to the three answers the gate actually needs.
    pub enum RiskClass {
        /// Local, reversible, small. Never gated (line 1005).
        Trivial => "trivial",
        /// Neither. Prompts and guidance, never a gate.
        Ordinary => "ordinary",
        /// One of line 1006's triggers fired.
        Substantial => "substantial",
    }
}

vocabulary! {
    /// Which factor decided the class — line 1049's *"which risk factor
    /// triggered it"*. In ladder order: the first that matches is the one
    /// named.
    pub enum RiskFactor {
        Migration => "migration",
        Destructive => "destructive",
        Security => "security",
        DataIntegrity => "data_integrity",
        UnfamiliarIntegration => "unfamiliar_integration",
        Architecture => "architecture",
        BroadRefactor => "broad_refactor",
        BlastRadius => "blast_radius",
        Irreversible => "irreversible",
        WeakPremise => "weak_premise",
        Footprint => "footprint",
    }
}

impl RiskFactor {
    /// The blocking category this factor belongs to, when it belongs to one.
    /// Only these four can gate; the rest are advisory by construction.
    pub fn category(self) -> Option<BlockingCategory> {
        match self {
            Self::Migration => Some(BlockingCategory::Migration),
            Self::Destructive => Some(BlockingCategory::Destructive),
            Self::Security => Some(BlockingCategory::Security),
            Self::DataIntegrity => Some(BlockingCategory::DataIntegrity),
            Self::UnfamiliarIntegration
            | Self::Architecture
            | Self::BroadRefactor
            | Self::BlastRadius
            | Self::Irreversible
            | Self::WeakPremise
            | Self::Footprint => None,
        }
    }
}

vocabulary! {
    /// What the agent is told to do about the class.
    pub enum Verdict {
        /// Go ahead; nothing was asked.
        Proceed => "proceed",
        /// Here are the prompts and the guidance; nothing blocks.
        Advisory => "advisory",
        /// Record and probe the critical assumptions before broadening the
        /// edit; a person can lift this, and the answer says how.
        Gated => "gated",
    }
}

vocabulary! {
    /// How far a change reaches, as the agent states it.
    pub enum BlastRadius {
        /// One place.
        Local => "local",
        /// One module or component.
        Module => "module",
        /// Several subsystems.
        CrossSubsystem => "cross_subsystem",
        /// The whole system, or its public surface.
        System => "system",
    }
}

vocabulary! {
    /// What a supported assumption may be promoted to — the ruling's
    /// *"decision, constraint or finding"* (line 1020), and nothing else.
    pub enum PromotionKind {
        Decision => "decision",
        Constraint => "constraint",
        Finding => "finding",
    }
}

vocabulary! {
    /// The three coarse axes of an implementation budget — line 1037.
    pub enum BudgetAxis {
        Footprint => "footprint",
        ToolRounds => "tool_rounds",
        ElapsedMinutes => "elapsed_minutes",
    }
}

// ---------------------------------------------------------------------------
// Bounds on untrusted text
// ---------------------------------------------------------------------------

/// The longest claim the store accepts. A claim is meant to be one sentence;
/// a longer one is refused rather than cut, because a truncated claim can
/// mean something its author did not say.
pub const MAX_CLAIM_CHARS: usize = 280;

/// The ceiling on every other free-text field — evidence, scope,
/// verification, a note. These are cut, visibly, with `…`.
pub const MAX_FIELD_CHARS: usize = 600;

/// The ceiling on a change's description, which is stored on the gate row for
/// a person to read and never classified.
pub const MAX_DESCRIPTION_CHARS: usize = 200;

/// Text that went through [`sanitize`]: what was kept, and whether anything
/// beyond the budget was dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bounded {
    pub text: String,
    pub truncated: bool,
}

/// Make untrusted text safe to *store*: the second and third of
/// `memory::inject::quote`'s three rules.
///
/// Anything that could act on a terminal becomes a space — control
/// characters (`\r` submits a line; `\u{1b}` opens an escape sequence), the
/// Unicode line and paragraph separators, and the bidirectional overrides
/// that reorder a rendered line — then runs of whitespace collapse and the
/// result is trimmed. The cut is by `char`, never by byte, and a cut string
/// ends in `…`.
///
/// Square brackets are **kept** here: they are stored faithfully and
/// rewritten by [`quote`] at the one place that matters, which is when text
/// is rendered into a block another agent reads.
pub fn sanitize(text: &str, budget: usize) -> Bounded {
    bound(text, budget, false)
}

/// Make untrusted text safe to *render into a block an agent reads*: all
/// three of `memory::inject::quote`'s rules, including `[` → `(` and `]` →
/// `)`, so that no stored claim can forge a labelled block's head or close a
/// block it sits inside.
pub fn quote(text: &str, budget: usize) -> String {
    bound(text, budget, true).text
}

fn bound(text: &str, budget: usize, rewrite_brackets: bool) -> Bounded {
    let mut out = String::with_capacity(text.len().min(budget * 4));
    let mut pending_space = false;
    let mut taken = 0usize;
    let mut truncated = false;

    for character in text.chars() {
        let mapped = match character {
            '[' if rewrite_brackets => '(',
            ']' if rewrite_brackets => ')',
            c if c.is_control() => ' ',
            '\u{2028}' | '\u{2029}' => ' ',
            '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => ' ',
            c => c,
        };
        if mapped.is_whitespace() {
            pending_space = !out.is_empty();
            continue;
        }
        if taken == budget {
            truncated = true;
            break;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        out.push(mapped);
        taken += 1;
    }

    if truncated {
        out.push('…');
    }
    Bounded {
        text: out,
        truncated,
    }
}

// ---------------------------------------------------------------------------
// The change, as the agent states it
// ---------------------------------------------------------------------------

/// A coarse implementation budget — capability map line 1037: *"files
/// touched, expected tool rounds, elapsed-time class"*. Every axis optional,
/// because most changes state one or two.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Budget {
    pub footprint: Option<u32>,
    pub tool_rounds: Option<u32>,
    pub elapsed_minutes: Option<u32>,
}

impl Budget {
    fn axis(&self, axis: BudgetAxis) -> Option<u32> {
        match axis {
            BudgetAxis::Footprint => self.footprint,
            BudgetAxis::ToolRounds => self.tool_rounds,
            BudgetAxis::ElapsedMinutes => self.elapsed_minutes,
        }
    }
}

/// The factors an agent states about an intended change — the whole input
/// to [`classify`], and deliberately nothing else.
///
/// `deny_unknown_fields` is load-bearing: a request carrying `reasoning`, a
/// `transcript` or an `output` field is refused, not ignored, because line
/// 998 is a promise about what this door will accept as well as about what
/// it stores.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ChangeFactors {
    /// One line for a person to read on the gate row. Never classified.
    pub description: Option<String>,
    /// How many files the change touches — its footprint. Named for what it
    /// counts rather than `files`, because no argument on the MCP door may
    /// carry a name that could hold a path.
    pub footprint: u32,
    /// Which subsystems it touches, by the agent's own names.
    pub subsystems: Vec<String>,
    /// Whether it can be undone easily.
    pub reversible: bool,
    pub blast_radius: BlastRadius,
    /// What the change's premise rests on. Absent means *"not stated"*, which
    /// is not the same as *"inference"*: the ladder only treats a premise as
    /// weak when the agent says it is one.
    pub premise_evidence: Option<EvidenceSource>,
    pub security: bool,
    pub data_integrity: bool,
    pub migration: bool,
    pub destructive: bool,
    pub unfamiliar_integration: bool,
    pub architecture: bool,
    pub broad_refactor: bool,
    /// The initial budget (line 1037).
    pub budget: Option<Budget>,
    /// What has been spent so far, when the agent is re-running the
    /// preflight to re-evaluate (line 1039).
    pub spent: Option<Budget>,
}

impl Default for ChangeFactors {
    /// A change nothing was said about is a one-file, local, reversible edit
    /// with no flags: the shape that passes with no gate. The gate is only
    /// as honest as what the agent states, which is the design — nothing is
    /// inferred to make it stricter.
    fn default() -> Self {
        Self {
            description: None,
            footprint: 1,
            subsystems: Vec::new(),
            reversible: true,
            blast_radius: BlastRadius::Local,
            premise_evidence: None,
            security: false,
            data_integrity: false,
            migration: false,
            destructive: false,
            unfamiliar_integration: false,
            architecture: false,
            broad_refactor: false,
            budget: None,
            spent: None,
        }
    }
}

/// At most this many files is still *trivial* — an implementation and its
/// test.
pub const TRIVIAL_MAX_FILES: u32 = 2;

/// From this many files up, a change is a broad refactor whatever it calls
/// itself.
pub const BROAD_MIN_FILES: u32 = 8;

/// The class and the rung that decided it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Classification {
    pub class: RiskClass,
    /// `None` exactly when the class is [`RiskClass::Trivial`]: nothing
    /// triggered.
    pub factor: Option<RiskFactor>,
}

/// Classify an intended change — line 1004, as a fixed ladder.
///
/// Substantial rungs first, in the order [`RiskFactor`] declares them; the
/// first that matches names the factor. Then the trivial test (line 1005),
/// then the ordinary rungs. Deterministic, total, and cheap enough to run on
/// every call.
pub fn classify(change: &ChangeFactors) -> Classification {
    let substantial = |factor| Classification {
        class: RiskClass::Substantial,
        factor: Some(factor),
    };
    let ordinary = |factor| Classification {
        class: RiskClass::Ordinary,
        factor: Some(factor),
    };
    let weak_premise = change.premise_evidence == Some(EvidenceSource::Inference);

    if change.migration {
        return substantial(RiskFactor::Migration);
    }
    if change.destructive {
        return substantial(RiskFactor::Destructive);
    }
    if change.security {
        return substantial(RiskFactor::Security);
    }
    if change.data_integrity {
        return substantial(RiskFactor::DataIntegrity);
    }
    if change.unfamiliar_integration {
        return substantial(RiskFactor::UnfamiliarIntegration);
    }
    if change.architecture {
        return substantial(RiskFactor::Architecture);
    }
    if change.broad_refactor || change.footprint >= BROAD_MIN_FILES {
        return substantial(RiskFactor::BroadRefactor);
    }
    if change.blast_radius >= BlastRadius::CrossSubsystem {
        return substantial(RiskFactor::BlastRadius);
    }
    if !change.reversible && change.footprint > TRIVIAL_MAX_FILES {
        return substantial(RiskFactor::Irreversible);
    }
    if weak_premise
        && (change.blast_radius >= BlastRadius::Module || change.footprint > TRIVIAL_MAX_FILES)
    {
        return substantial(RiskFactor::WeakPremise);
    }

    // Line 1005: trivial, local, easily reversible.
    if change.footprint <= TRIVIAL_MAX_FILES
        && change.reversible
        && change.blast_radius == BlastRadius::Local
        && !weak_premise
    {
        return Classification {
            class: RiskClass::Trivial,
            factor: None,
        };
    }

    if !change.reversible {
        return ordinary(RiskFactor::Irreversible);
    }
    if weak_premise {
        return ordinary(RiskFactor::WeakPremise);
    }
    if change.blast_radius > BlastRadius::Local {
        return ordinary(RiskFactor::BlastRadius);
    }
    ordinary(RiskFactor::Footprint)
}

// ---------------------------------------------------------------------------
// The verdict
// ---------------------------------------------------------------------------

/// A per-task override as read back from the ledger: what it was, who
/// recorded it, and the row that says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AppliedOverride {
    pub kind: GuardrailOverride,
    pub origin: Origin,
    pub seq: i64,
}

/// Everything [`decide`] reads: the configured mode and blocking list, each
/// with the phrase naming the layer that set it, and the task's override if
/// one was recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    pub mode: GuardrailMode,
    /// `config::Layer::describe_source`'s phrase, e.g. *"by default"*.
    pub mode_source: &'static str,
    pub blocking: Vec<BlockingCategory>,
    pub blocking_source: &'static str,
    pub override_: Option<AppliedOverride>,
}

impl Policy {
    /// The shipped defaults with no override: advisory, and the ruling's
    /// three blocking categories should anyone switch to `risk_gated`.
    pub fn default_policy() -> Self {
        Self {
            mode: GuardrailMode::Advisory,
            mode_source: "by default",
            blocking: DEFAULT_BLOCKING.to_vec(),
            blocking_source: "by default",
            override_: None,
        }
    }
}

/// `guardrails.blocking` when nothing is configured — the ruling's list.
pub const DEFAULT_BLOCKING: [BlockingCategory; 3] = [
    BlockingCategory::Security,
    BlockingCategory::Destructive,
    BlockingCategory::Migration,
];

/// Who or what decided the verdict — line 1053's attribution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecidedBy {
    /// Trivial never gates, whatever else is configured.
    TrivialNeverGates,
    /// A per-task override outranked the mode.
    Override(GuardrailOverride),
    /// The configured mode.
    Mode(GuardrailMode),
}

impl DecidedBy {
    pub fn describe(self) -> String {
        match self {
            Self::TrivialNeverGates => "trivial changes never gate".to_owned(),
            Self::Override(kind) => format!("per-task override `--guardrail {kind}`"),
            Self::Mode(mode) => format!("guardrails.mode = {mode}"),
        }
    }
}

/// The verdict and its attribution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub verdict: Verdict,
    pub decided_by: DecidedBy,
    /// For a gate: the overrides that lift it, as a sentence a person can
    /// act on. `None` when nothing is blocking.
    pub lifts: Option<String>,
}

/// Turn a classification into a verdict — lines 1005, 1008, 1052, 1053.
///
/// Precedence: trivial never gates; then the per-task override; then the
/// mode. Under `risk_gated` a substantial change gates only when its factor's
/// category is in the blocking list — a factor with no category (an
/// architectural change, a broad refactor) is advisory by construction.
pub fn decide(classification: &Classification, policy: &Policy) -> Decision {
    let no_gate = |verdict, decided_by| Decision {
        verdict,
        decided_by,
        lifts: None,
    };

    if classification.class == RiskClass::Trivial {
        return no_gate(Verdict::Proceed, DecidedBy::TrivialNeverGates);
    }

    let lifts = || {
        Some(format!(
            "`--guardrail skip` or `--guardrail lower` on this task, or `guardrails.mode = \
             advisory` in place of the mode set {}",
            policy.mode_source
        ))
    };

    if let Some(applied) = policy.override_ {
        let decided_by = DecidedBy::Override(applied.kind);
        return match (applied.kind, classification.class) {
            (GuardrailOverride::Skip, _) => no_gate(Verdict::Proceed, decided_by),
            (GuardrailOverride::Force, RiskClass::Substantial) => Decision {
                verdict: Verdict::Gated,
                decided_by,
                lifts: lifts(),
            },
            (GuardrailOverride::Force, _) => no_gate(Verdict::Advisory, decided_by),
            (GuardrailOverride::Lower, RiskClass::Substantial) => {
                no_gate(Verdict::Advisory, decided_by)
            }
            (GuardrailOverride::Lower, _) => no_gate(Verdict::Proceed, decided_by),
        };
    }

    let decided_by = DecidedBy::Mode(policy.mode);
    match policy.mode {
        GuardrailMode::Off => no_gate(Verdict::Proceed, decided_by),
        GuardrailMode::Advisory => no_gate(Verdict::Advisory, decided_by),
        GuardrailMode::RiskGated => {
            let blocks = classification.class == RiskClass::Substantial
                && classification
                    .factor
                    .and_then(RiskFactor::category)
                    .is_some_and(|category| policy.blocking.contains(&category));
            if blocks {
                Decision {
                    verdict: Verdict::Gated,
                    decided_by,
                    lifts: lifts(),
                }
            } else {
                no_gate(Verdict::Advisory, decided_by)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The template: prompts, guidance, responses, budget
// ---------------------------------------------------------------------------

/// How many critical-assumption prompts a preflight may carry — line 1013's
/// *"small set"* and line 1007's *"short enough"*, as a number.
pub const MAX_PROMPTS: usize = 3;

/// One prompt: a stable key and the question.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Prompt {
    pub key: &'static str,
    pub ask: &'static str,
}

const PROMPT_MIGRATION: Prompt = Prompt {
    key: "migration-undo",
    ask: "Which rows written by the previous build must this migration read correctly, and \
          what is the exact undo?",
};
const PROMPT_DESTRUCTIVE: Prompt = Prompt {
    key: "destructive-loss",
    ask: "What is lost irreversibly, and what evidence says nothing still needs it?",
};
const PROMPT_SECURITY: Prompt = Prompt {
    key: "security-boundary",
    ask: "Which trust boundary does the change cross, and what does an untrusted input look \
          like at it?",
};
const PROMPT_DATA_INTEGRITY: Prompt = Prompt {
    key: "data-invariant",
    ask: "Which invariant over stored data must hold afterwards, and how is it checked?",
};
const PROMPT_INTEGRATION: Prompt = Prompt {
    key: "integration-behaviour",
    ask: "Which behaviour of the unfamiliar integration is assumed, and where is it documented \
          or probed?",
};
const PROMPT_PREMISE: Prompt = Prompt {
    key: "single-premise",
    ask: "Which single premise, if false, invalidates the whole change — and what is the \
          cheapest probe of it?",
};
const PROMPT_WEAK_PREMISE: Prompt = Prompt {
    key: "inference-to-evidence",
    ask: "The premise is stated as inference: what direct evidence — source, test, schema, \
          runtime — could replace it before the edit broadens?",
};
const PROMPT_IRREVERSIBLE: Prompt = Prompt {
    key: "undo-path",
    ask: "How would the change be undone if the premise fails after it lands?",
};
const PROMPT_BASELINE: Prompt = Prompt {
    key: "baseline",
    ask: "What baseline distinguishes success from the pre-existing state?",
};

/// The critical-assumption prompts for a change — at most [`MAX_PROMPTS`],
/// chosen from the factors that fired, most severe first, with the baseline
/// question filling the last slot when there is room.
///
/// Empty for a trivial change: a template that asked a one-line edit three
/// questions would be the *"speculative over-planning"* line 1007 forbids.
pub fn prompts(classification: &Classification, change: &ChangeFactors) -> Vec<Prompt> {
    if classification.class == RiskClass::Trivial {
        return Vec::new();
    }

    let mut chosen: Vec<Prompt> = Vec::new();
    let mut push = |prompt: Prompt| {
        if chosen.len() < MAX_PROMPTS && !chosen.iter().any(|p| p.key == prompt.key) {
            chosen.push(prompt);
        }
    };

    if change.migration {
        push(PROMPT_MIGRATION);
    }
    if change.destructive {
        push(PROMPT_DESTRUCTIVE);
    }
    if change.security {
        push(PROMPT_SECURITY);
    }
    if change.data_integrity {
        push(PROMPT_DATA_INTEGRITY);
    }
    if change.unfamiliar_integration {
        push(PROMPT_INTEGRATION);
    }
    if change.premise_evidence == Some(EvidenceSource::Inference) {
        push(PROMPT_WEAK_PREMISE);
    }
    if change.architecture
        || change.broad_refactor
        || change.footprint > TRIVIAL_MAX_FILES
        || change.blast_radius > BlastRadius::Local
    {
        push(PROMPT_PREMISE);
    }
    if !change.reversible {
        push(PROMPT_IRREVERSIBLE);
    }
    push(PROMPT_PREMISE);
    push(PROMPT_BASELINE);

    chosen
}

/// One line of guidance, keyed by the capability-map line it renders, so a
/// test can assert that the line's text reaches the agent through the door.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Guidance {
    pub line: u32,
    pub text: &'static str,
}

/// Guidance every non-trivial preflight carries.
pub const GUIDANCE_CORE: &[Guidance] = &[
    Guidance {
        line: 997,
        text: "Confidence, repetition, eloquence and the length of reasoning are presentation, \
               not evidence.",
    },
    Guidance {
        line: 1009,
        text: "A long plan is not a substitute for validating the few premises it depends on.",
    },
    Guidance {
        line: 1024,
        text: "Prefer direct evidence — current requirements, source code, executable tests, \
               configuration, schemas, runtime behaviour, primary documentation, a bounded \
               experiment — over a narrative explanation.",
    },
    Guidance {
        line: 1027,
        text: "Prefer a read-only inspection, a minimal reproduction, an executable probe, a \
               failing test, a walking skeleton or a narrow vertical slice before a large \
               implementation.",
    },
    Guidance {
        line: 1038,
        text: "Prefer the smallest implementation slice that can confirm or falsify the \
               approach.",
    },
    Guidance {
        line: 1028,
        text: "Establish a baseline first where later success would otherwise be hard to tell \
               from the pre-existing state.",
    },
    Guidance {
        line: 1029,
        text: "Label an unresolved inference honestly as inference, and time-box exploratory \
               work when direct verification is unavailable.",
    },
    Guidance {
        line: 1039,
        text: "Re-run this preflight with `spent` beside `budget` when the planned footprint \
               expands materially, a verification contradicts the premise, or the budget is \
               exceeded — and re-evaluate the recorded assumptions then.",
    },
];

/// Guidance a substantial preflight adds — the lines about verification
/// independence and about what to do when a premise is refuted.
pub const GUIDANCE_SUBSTANTIAL: &[Guidance] = &[
    Guidance {
        line: 1025,
        text: "The higher the implementation cost, irreversibility, security impact, data risk \
               or architectural blast radius, the stronger the evidence owed.",
    },
    Guidance {
        line: 1026,
        text: "Verify the highest-leverage premise before broadening the edit across many files \
               or subsystems, whenever verification is practical.",
    },
    Guidance {
        line: 1030,
        text: "Do not ask a second model merely whether the first sounds correct: a verifier \
               must cite independent repository, runtime, test or primary-source evidence.",
    },
    Guidance {
        line: 1031,
        text: "For high-impact adversarial verification, a fresh session or a different harness \
               buys independence; spend it when independence is worth its additional cost.",
    },
    Guidance {
        line: 1032,
        text: "Reviewer agreement without new evidence is weak confirmation — different agents \
               can share the same mistaken premise.",
    },
    Guidance {
        line: 1040,
        text: "Pause expansion when you begin adding adapters, compatibility layers or \
               secondary mechanisms mainly to protect an unverified premise.",
    },
    Guidance {
        line: 1041,
        text: "When a critical premise is refuted, stop compounding the implementation and \
               explicitly choose rollback, repair, re-plan, preserve as an experiment, or ask \
               the user.",
    },
    Guidance {
        line: 1044,
        text: "Before reverting anything, exclude every path the reply lists under preserve — \
               another live session or the user owns it; Glasshouse names them and reverts \
               nothing.",
    },
    Guidance {
        line: 1042,
        text: "Preserve useful evidence and a concise failed-approach record even when the \
               implementation itself is discarded.",
    },
    Guidance {
        line: 1043,
        text: "Never silently rewrite the task history to make a failed premise appear as \
               though it had always been understood correctly.",
    },
];

/// The guidance for a class: none for trivial, the core page for ordinary,
/// and the core page plus the substantial page for substantial.
pub fn guidance(class: RiskClass) -> Vec<Guidance> {
    match class {
        RiskClass::Trivial => Vec::new(),
        RiskClass::Ordinary => GUIDANCE_CORE.to_vec(),
        RiskClass::Substantial => GUIDANCE_CORE
            .iter()
            .chain(GUIDANCE_SUBSTANTIAL.iter())
            .copied()
            .collect(),
    }
}

/// One offered response, with what choosing it means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ResponseOption {
    pub response: GuardrailResponse,
    pub means: &'static str,
}

/// Line 1051's seven responses, offered on every verdict that is not
/// `proceed`. Each is accepted back by `update_assumption` as the
/// transition's `response`.
pub const RESPONSES: &[ResponseOption] = &[
    ResponseOption {
        response: GuardrailResponse::Inspect,
        means: "read more before deciding — a read-only inspection of the premise",
    },
    ResponseOption {
        response: GuardrailResponse::Continue,
        means: "go on as planned with the assumptions as recorded",
    },
    ResponseOption {
        response: GuardrailResponse::Verify,
        means: "run the cheapest verification step now and record the result",
    },
    ResponseOption {
        response: GuardrailResponse::Checkpoint,
        means: "take a checkpoint before going further (take_checkpoint)",
    },
    ResponseOption {
        response: GuardrailResponse::Handoff,
        means: "hand the work to another session or harness, with a checkpoint",
    },
    ResponseOption {
        response: GuardrailResponse::RePlan,
        means: "re-plan from the premise that was refuted",
    },
    ResponseOption {
        response: GuardrailResponse::Stop,
        means: "stop and ask the person",
    },
];

/// The two [`RESPONSES`] choices line 1044 answers for — this table's own
/// words: [`GuardrailResponse::RePlan`] discards the work built on the
/// refuted premise ("re-plan from the premise that was refuted"), which is
/// the rollback line 1041 names; [`GuardrailResponse::Handoff`] is the
/// isolate/preserve-as-experiment choice — its own `means` already says "with
/// a checkpoint", so handing off is how the invalidated experiment is kept
/// rather than deleted. Neither `GuardrailResponse` variant is spelled
/// "rollback" or "isolate"; these are the closest real ones, by the meaning
/// the table already gives them, and this is where that reading lives so it
/// is made once.
const PRESERVING_RESPONSES: [GuardrailResponse; 2] =
    [GuardrailResponse::RePlan, GuardrailResponse::Handoff];

/// Whether an appended transition is one line 1044 answers for: a move to
/// `refuted`, or one of the two rollback/isolate responses.
pub fn transition_wants_preserve(
    state: Option<AssumptionState>,
    response: Option<GuardrailResponse>,
) -> bool {
    state == Some(AssumptionState::Refuted)
        || response.is_some_and(|response| PRESERVING_RESPONSES.contains(&response))
}

/// The paths a rollback or isolation must not touch — capability map line
/// 1044. Computed at the moment of the transition from three facts already
/// on hand: which paths another live session has declared it is changing
/// (`claims`), which paths the working tree currently reports changed
/// (`changed`, `None` when that could not be read), and which session is
/// doing the choosing (`session`). See
/// `docs/product/design-decisions.md`, *Rollback preserves what is not
/// yours*, for the ruling this implements.
///
/// `claimed_elsewhere` is exact — every entry is another session's own
/// declared claim, never the transitioning session's. `unclaimed_changes` is
/// conservative: a changed path the transitioning session never claimed
/// lands here whether it is the user's edit or an unclaiming worker's own —
/// Glasshouse cannot tell those two apart and does not try, because both are
/// simply *not the experiment's*, which is the only distinction the line
/// needs. A path claimed only by another session can therefore appear in
/// both. `unclaimed_changes` is `None` exactly when `changed` is `None` — an
/// unreadable tree stays *unknown*, never an empty list that reads as
/// nothing to preserve.
pub fn preserve_set(
    claims: &[FileClaim],
    changed: Option<&[String]>,
    session: &SessionId,
) -> PreserveSet {
    let mut claimed_elsewhere: Vec<String> = claims
        .iter()
        .filter(|claim| &claim.session_id != session)
        .map(|claim| claim.path.clone())
        .collect();
    claimed_elsewhere.sort();
    claimed_elsewhere.dedup();

    let unclaimed_changes = changed.map(|changed| {
        let own: std::collections::BTreeSet<&str> = claims
            .iter()
            .filter(|claim| &claim.session_id == session)
            .map(|claim| claim.path.as_str())
            .collect();
        let mut unclaimed: Vec<String> = changed
            .iter()
            .filter(|path| !own.contains(path.as_str()))
            .cloned()
            .collect();
        unclaimed.sort();
        unclaimed.dedup();
        unclaimed
    });

    PreserveSet {
        claimed_elsewhere,
        unclaimed_changes,
    }
}

/// The reply's own reading of what a rollback or isolation must not touch —
/// [`preserve_set`]'s result, and the type the door's transition reply
/// carries as `preserve` when [`transition_wants_preserve`] answers `true`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreserveSet {
    /// Repo-relative paths under another live session's active claim,
    /// deduplicated and sorted. Never the transitioning session's own.
    pub claimed_elsewhere: Vec<String>,
    /// Repo-relative paths the working tree reports changed that the
    /// transitioning session never claimed. `None` only when the tree could
    /// not be read at all.
    pub unclaimed_changes: Option<Vec<String>>,
}

/// One axis of a budget review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BudgetLine {
    pub axis: BudgetAxis,
    pub budget: u32,
    pub spent: u32,
    pub exceeded: bool,
}

/// The stated budget against what was spent — line 1050's *"materially
/// exceeded"*, decided as *any axis over its bound*. Present only when the
/// change states both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BudgetReview {
    pub exceeded: bool,
    pub axes: Vec<BudgetLine>,
}

impl BudgetReview {
    /// The first axis over its bound, for the ledger's `subject`.
    pub fn exceeded_axis(&self) -> Option<BudgetAxis> {
        self.axes
            .iter()
            .find(|line| line.exceeded)
            .map(|line| line.axis)
    }
}

/// Compare `spent` against `budget` on every axis both state.
pub fn review_budget(change: &ChangeFactors) -> Option<BudgetReview> {
    let (budget, spent) = (change.budget?, change.spent?);
    let axes: Vec<BudgetLine> = BudgetAxis::ALL
        .iter()
        .filter_map(|&axis| {
            let bound = budget.axis(axis)?;
            let used = spent.axis(axis)?;
            Some(BudgetLine {
                axis,
                budget: bound,
                spent: used,
                exceeded: used > bound,
            })
        })
        .collect();
    Some(BudgetReview {
        exceeded: axes.iter().any(|line| line.exceeded),
        axes,
    })
}

// ---------------------------------------------------------------------------
// The whole preflight answer
// ---------------------------------------------------------------------------

/// Line 1053's attribution block: what decided the verdict, and how a person
/// lifts it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GateOrigin {
    /// Whether the verdict is a gate. `false` for `proceed` and `advisory`.
    pub triggered: bool,
    pub decided_by: String,
    pub mode: GuardrailMode,
    pub mode_source: &'static str,
    pub blocking: Vec<BlockingCategory>,
    #[serde(rename = "override")]
    pub override_: Option<AppliedOverride>,
    pub lifts: Option<String>,
}

/// Everything a preflight answers that does not depend on a session. The
/// door adds the session-specific parts — the recorded gate row, a
/// checkpoint, the assumptions to re-evaluate — beside these.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreflightAnswer {
    pub risk: RiskClass,
    /// Line 1049: which factor triggered the class. `None` for trivial.
    pub factor: Option<RiskFactor>,
    pub category: Option<BlockingCategory>,
    pub verdict: Verdict,
    pub gate: GateOrigin,
    pub prompts: Vec<Prompt>,
    pub guidance: Vec<Guidance>,
    pub responses: Vec<ResponseOption>,
    pub budget: Option<BudgetReview>,
}

/// The deterministic half of `Request::Preflight`: classify, decide, and
/// fill the template. No store, no session, no I/O.
pub fn preflight(change: &ChangeFactors, policy: &Policy) -> PreflightAnswer {
    let classification = classify(change);
    let decision = decide(&classification, policy);
    let responses = if decision.verdict == Verdict::Proceed {
        Vec::new()
    } else {
        RESPONSES.to_vec()
    };
    let (prompts, guidance) = if decision.verdict == Verdict::Proceed
        && matches!(
            decision.decided_by,
            DecidedBy::Override(GuardrailOverride::Skip)
        ) {
        // A waived gate asks nothing: the person said so.
        (Vec::new(), Vec::new())
    } else {
        (
            prompts(&classification, change),
            guidance(classification.class),
        )
    };

    PreflightAnswer {
        risk: classification.class,
        factor: classification.factor,
        category: classification.factor.and_then(RiskFactor::category),
        verdict: decision.verdict,
        gate: GateOrigin {
            triggered: decision.verdict == Verdict::Gated,
            decided_by: decision.decided_by.describe(),
            mode: policy.mode,
            mode_source: policy.mode_source,
            blocking: policy.blocking.clone(),
            override_: policy.override_,
            lifts: decision.lifts,
        },
        prompts,
        guidance,
        responses,
        budget: review_budget(change),
    }
}

/// Record a per-task override for a session — the one write `launch`,
/// `run` and `SpawnSession` make on the ledger (line 1008).
///
/// A `skip` is recorded with the state `waived_by_user`, so the ledger
/// carries the waiver as the fact it is and every later preflight for the
/// session answers `proceed` with this row as its attribution. Opens and
/// drops its own handle; nothing is held afterwards.
pub fn record_override(
    runtime: &crate::Runtime,
    session: &str,
    kind: GuardrailOverride,
    origin: Origin,
) -> anyhow::Result<Transition> {
    let mut ledger = AssumptionStore::open(runtime)?;
    let state = (kind == GuardrailOverride::Skip).then_some(AssumptionState::WaivedByUser);
    Ok(ledger.record_session_event(
        session,
        TransitionKind::Override,
        state,
        origin,
        Some(kind.as_str()),
        None,
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change() -> ChangeFactors {
        ChangeFactors::default()
    }

    fn policy(mode: GuardrailMode) -> Policy {
        Policy {
            mode,
            ..Policy::default_policy()
        }
    }

    #[test]
    fn a_one_file_reversible_edit_is_trivial_and_names_no_factor() {
        let classification = classify(&change());
        assert_eq!(classification.class, RiskClass::Trivial);
        assert_eq!(classification.factor, None);
        assert!(prompts(&classification, &change()).is_empty());
        assert!(guidance(RiskClass::Trivial).is_empty());
    }

    #[test]
    fn the_ladder_names_the_first_rung_that_fires() {
        let mut both = change();
        both.migration = true;
        both.security = true;
        assert_eq!(classify(&both).factor, Some(RiskFactor::Migration));

        let mut security = change();
        security.security = true;
        assert_eq!(classify(&security).factor, Some(RiskFactor::Security));
        assert_eq!(classify(&security).class, RiskClass::Substantial);

        let mut wide = change();
        wide.footprint = BROAD_MIN_FILES;
        assert_eq!(classify(&wide).factor, Some(RiskFactor::BroadRefactor));

        let mut reach = change();
        reach.blast_radius = BlastRadius::CrossSubsystem;
        assert_eq!(classify(&reach).factor, Some(RiskFactor::BlastRadius));

        let mut inference = change();
        inference.premise_evidence = Some(EvidenceSource::Inference);
        inference.footprint = 3;
        assert_eq!(classify(&inference).factor, Some(RiskFactor::WeakPremise));
        assert_eq!(classify(&inference).class, RiskClass::Substantial);
    }

    #[test]
    fn a_weakly_evidenced_local_edit_is_ordinary_not_trivial() {
        let mut inference = change();
        inference.premise_evidence = Some(EvidenceSource::Inference);
        let classification = classify(&inference);
        assert_eq!(classification.class, RiskClass::Ordinary);
        assert_eq!(classification.factor, Some(RiskFactor::WeakPremise));
    }

    #[test]
    fn a_three_file_reversible_local_edit_is_ordinary_by_footprint() {
        let mut three = change();
        three.footprint = 3;
        let classification = classify(&three);
        assert_eq!(classification.class, RiskClass::Ordinary);
        assert_eq!(classification.factor, Some(RiskFactor::Footprint));
    }

    #[test]
    fn off_always_proceeds_and_advisory_never_gates() {
        let mut migration = change();
        migration.migration = true;
        let classification = classify(&migration);

        let off = decide(&classification, &policy(GuardrailMode::Off));
        assert_eq!(off.verdict, Verdict::Proceed);
        assert_eq!(off.decided_by, DecidedBy::Mode(GuardrailMode::Off));

        let advisory = decide(&classification, &policy(GuardrailMode::Advisory));
        assert_eq!(advisory.verdict, Verdict::Advisory);
        assert_eq!(advisory.lifts, None);
    }

    #[test]
    fn risk_gated_gates_only_a_blocking_category() {
        let mut migration = change();
        migration.migration = true;
        let gated = decide(&classify(&migration), &policy(GuardrailMode::RiskGated));
        assert_eq!(gated.verdict, Verdict::Gated);
        assert!(gated.lifts.is_some(), "a gate must say what lifts it");

        let mut architecture = change();
        architecture.architecture = true;
        let advisory = decide(&classify(&architecture), &policy(GuardrailMode::RiskGated));
        assert_eq!(
            advisory.verdict,
            Verdict::Advisory,
            "a factor with no blocking category is advisory by construction"
        );

        let mut narrowed = policy(GuardrailMode::RiskGated);
        narrowed.blocking = vec![BlockingCategory::Security];
        let not_listed = decide(&classify(&migration), &narrowed);
        assert_eq!(not_listed.verdict, Verdict::Advisory);
    }

    #[test]
    fn the_per_task_override_outranks_the_mode_and_trivial_still_never_gates() {
        let mut migration = change();
        migration.migration = true;
        let substantial = classify(&migration);

        let with = |kind, mode| {
            let mut policy = policy(mode);
            policy.override_ = Some(AppliedOverride {
                kind,
                origin: Origin::User,
                seq: 1,
            });
            policy
        };

        let skipped = decide(
            &substantial,
            &with(GuardrailOverride::Skip, GuardrailMode::RiskGated),
        );
        assert_eq!(skipped.verdict, Verdict::Proceed);
        assert_eq!(
            skipped.decided_by,
            DecidedBy::Override(GuardrailOverride::Skip)
        );

        let forced = decide(
            &substantial,
            &with(GuardrailOverride::Force, GuardrailMode::Off),
        );
        assert_eq!(forced.verdict, Verdict::Gated);

        let lowered = decide(
            &substantial,
            &with(GuardrailOverride::Lower, GuardrailMode::RiskGated),
        );
        assert_eq!(lowered.verdict, Verdict::Advisory);

        let trivial = classify(&change());
        let forced_trivial = decide(
            &trivial,
            &with(GuardrailOverride::Force, GuardrailMode::RiskGated),
        );
        assert_eq!(forced_trivial.verdict, Verdict::Proceed);
        assert_eq!(forced_trivial.decided_by, DecidedBy::TrivialNeverGates);
    }

    #[test]
    fn at_most_three_prompts_and_the_factor_that_fired_comes_first() {
        let mut everything = change();
        everything.migration = true;
        everything.destructive = true;
        everything.security = true;
        everything.data_integrity = true;
        everything.unfamiliar_integration = true;
        everything.reversible = false;
        everything.footprint = 20;
        let chosen = prompts(&classify(&everything), &everything);
        // The literal, not `MAX_PROMPTS`: a test that derived its expectation
        // from the constant survived the constant being raised (practice
        // §80, case 6). Line 1013 says three.
        assert_eq!(chosen.len(), 3);
        assert_eq!(MAX_PROMPTS, 3, "line 1013's small set is three");
        assert_eq!(chosen[0].key, PROMPT_MIGRATION.key);
        assert_eq!(chosen[1].key, PROMPT_DESTRUCTIVE.key);
        assert_eq!(chosen[2].key, PROMPT_SECURITY.key);

        let mut ordinary = change();
        ordinary.footprint = 3;
        let few = prompts(&classify(&ordinary), &ordinary);
        assert_eq!(
            few.iter().map(|p| p.key).collect::<Vec<_>>(),
            vec![PROMPT_PREMISE.key, PROMPT_BASELINE.key]
        );
    }

    #[test]
    fn substantial_guidance_carries_every_line_the_map_names() {
        let lines: Vec<u32> = guidance(RiskClass::Substantial)
            .iter()
            .map(|g| g.line)
            .collect();
        for line in [
            997, 1009, 1024, 1025, 1026, 1027, 1028, 1029, 1030, 1031, 1032, 1038, 1039, 1040,
            1041, 1042, 1043, 1044,
        ] {
            assert!(lines.contains(&line), "guidance is missing line {line}");
        }
        let core: Vec<u32> = guidance(RiskClass::Ordinary)
            .iter()
            .map(|g| g.line)
            .collect();
        assert!(core.contains(&997) && !core.contains(&1041));
    }

    #[test]
    fn a_budget_is_exceeded_when_any_stated_axis_is_over() {
        let mut change = change();
        change.budget = Some(Budget {
            footprint: Some(4),
            tool_rounds: Some(30),
            elapsed_minutes: None,
        });
        change.spent = Some(Budget {
            footprint: Some(4),
            tool_rounds: Some(31),
            elapsed_minutes: Some(999),
        });
        let review = review_budget(&change).expect("both stated");
        assert!(review.exceeded);
        assert_eq!(review.exceeded_axis(), Some(BudgetAxis::ToolRounds));
        assert_eq!(
            review.axes.len(),
            2,
            "an axis only one side states is not compared"
        );

        change.spent = None;
        assert_eq!(review_budget(&change), None);
    }

    #[test]
    fn sanitize_strips_what_could_act_on_a_terminal_and_quote_also_rewrites_brackets() {
        let raw = "a\r\nclaim\u{1b}[31m with [brackets]  and\u{202e}bidi";
        let stored = sanitize(raw, 100);
        assert_eq!(stored.text, "a claim [31m with [brackets] and bidi");
        assert!(!stored.truncated);
        assert!(!stored.text.chars().any(char::is_control));
        assert_eq!(quote(raw, 100), "a claim (31m with (brackets) and bidi");

        let cut = sanitize("abcdef", 3);
        assert_eq!(cut.text, "abc…");
        assert!(cut.truncated);
    }

    #[test]
    fn every_vocabulary_round_trips_and_refuses_a_stranger() {
        for state in AssumptionState::ALL {
            assert_eq!(AssumptionState::from_stored(state.as_str()), Some(*state));
        }
        assert_eq!(AssumptionState::ALL.len(), 6, "line 1018 names six");
        assert_eq!(AssumptionState::from_stored("waived"), None);
        assert_eq!(GuardrailResponse::RePlan.as_str(), "re-plan");
        assert_eq!(GuardrailResponse::ALL.len(), 7, "line 1051 names seven");
        assert_eq!(EvidenceSource::ALL.len(), 6, "line 1015 names six");
        let parsed: GuardrailMode = serde_json::from_str("\"risk_gated\"").unwrap();
        assert_eq!(parsed, GuardrailMode::RiskGated);
        assert!(serde_json::from_str::<GuardrailMode>("\"strict\"").is_err());
    }

    #[test]
    fn a_change_carrying_a_reasoning_field_is_refused_not_ignored() {
        let err = serde_json::from_str::<ChangeFactors>(r#"{"footprint": 1, "reasoning": "..."}"#)
            .unwrap_err();
        assert!(err.to_string().contains("reasoning"), "{err}");
    }

    // -----------------------------------------------------------------
    // Line 1044 — the preserve set.
    // -----------------------------------------------------------------

    fn claim(session: &str, path: &str) -> FileClaim {
        FileClaim {
            session_id: SessionId::new(session),
            path: path.to_owned(),
            claimed_at: 0,
            renewed_at: 0,
            expires_at: 0,
        }
    }

    #[test]
    fn another_sessions_claim_is_preserved_and_the_transitioning_sessions_own_is_not() {
        let a = SessionId::new("session-a");
        let claims = [
            claim("session-a", "src/mine.rs"),
            claim("session-b", "src/b.rs"),
        ];
        let set = preserve_set(&claims, None, &a);
        assert_eq!(set.claimed_elsewhere, vec!["src/b.rs".to_owned()]);
        assert_eq!(set.unclaimed_changes, None);
    }

    #[test]
    fn claimed_elsewhere_is_deduplicated_and_sorted() {
        let a = SessionId::new("session-a");
        let claims = [
            claim("session-c", "z.rs"),
            claim("session-b", "a.rs"),
            claim("session-b", "a.rs"),
        ];
        let set = preserve_set(&claims, None, &a);
        assert_eq!(
            set.claimed_elsewhere,
            vec!["a.rs".to_owned(), "z.rs".to_owned()]
        );
    }

    #[test]
    fn unclaimed_changes_excludes_the_transitioning_sessions_own_claim() {
        let a = SessionId::new("session-a");
        let claims = [claim("session-a", "src/mine.rs")];
        let changed = vec!["src/mine.rs".to_owned(), "notes.md".to_owned()];
        let set = preserve_set(&claims, Some(&changed), &a);
        assert_eq!(set.unclaimed_changes, Some(vec!["notes.md".to_owned()]));
    }

    /// A path claimed only by another session is conservative in both
    /// halves at once: it is that session's own claim, and it is also a
    /// change the transitioning session never claimed.
    #[test]
    fn a_path_claimed_only_by_another_session_can_appear_in_both() {
        let a = SessionId::new("session-a");
        let claims = [claim("session-b", "src/b.rs")];
        let changed = vec!["src/b.rs".to_owned()];
        let set = preserve_set(&claims, Some(&changed), &a);
        assert_eq!(set.claimed_elsewhere, vec!["src/b.rs".to_owned()]);
        assert_eq!(set.unclaimed_changes, Some(vec!["src/b.rs".to_owned()]));
    }

    /// An unreadable working tree stays *unknown* — never an empty list that
    /// would read as nothing to preserve.
    #[test]
    fn an_unreadable_tree_is_none_not_empty() {
        let a = SessionId::new("session-a");
        let set = preserve_set(&[], None, &a);
        assert_eq!(set.unclaimed_changes, None);
    }

    #[test]
    fn a_clean_readable_tree_is_some_empty() {
        let a = SessionId::new("session-a");
        let set = preserve_set(&[], Some(&[]), &a);
        assert_eq!(set.unclaimed_changes, Some(Vec::new()));
    }

    #[test]
    fn transition_wants_preserve_on_refuted_regardless_of_response() {
        assert!(transition_wants_preserve(
            Some(AssumptionState::Refuted),
            None
        ));
        assert!(transition_wants_preserve(
            Some(AssumptionState::Refuted),
            Some(GuardrailResponse::Continue)
        ));
    }

    #[test]
    fn transition_wants_preserve_on_the_rollback_and_isolate_responses() {
        assert!(transition_wants_preserve(
            Some(AssumptionState::Supported),
            Some(GuardrailResponse::RePlan)
        ));
        assert!(transition_wants_preserve(
            None,
            Some(GuardrailResponse::Handoff)
        ));
    }

    #[test]
    fn transition_wants_preserve_is_false_for_every_other_combination() {
        assert!(!transition_wants_preserve(
            Some(AssumptionState::Supported),
            Some(GuardrailResponse::Continue)
        ));
        assert!(!transition_wants_preserve(
            Some(AssumptionState::Probing),
            Some(GuardrailResponse::Verify)
        ));
        assert!(!transition_wants_preserve(None, None));
    }
}
