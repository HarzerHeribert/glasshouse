//! Lightweight task classification — Phase 35.
//!
//! # What "lightweight" rules out
//!
//! The map's own preamble frames this as the thing Glasshouse asks *before*
//! spending premium agent capacity — [`super::disposable::JobKind::Classification`]
//! is already the name for that job in the disposable-routing policy class.
//! A classifier that had to make a network call for every request would not
//! be lightweight and could not "run on a cheap, free, or local model" in any
//! meaningful sense, so this module makes none: [`classify_heuristically`] is
//! a pure, deterministic function of the request text, and [`classify`]'s
//! model path takes an *already-produced* [`TaskClassification`] as an
//! argument rather than calling anything itself — the same discipline
//! [`mod@super`]'s own doc comment states for the two routing-policy classes,
//! extended to this one.
//!
//! # Nothing here decides which model does the classifying
//!
//! `crate::config::RoutingModelChoice` and `RoutingModelResolution` (Phase
//! 2C) already record *whether* a routing model is configured and resolve it
//! against the providers that exist. This module is downstream of that
//! decision, not a duplicate of it: whatever calls a routing model is
//! expected to turn its reply into a [`TaskClassification`] and hand it to
//! [`classify`], and whatever finds no routing model configured falls
//! through to [`classify_heuristically`] instead. Neither path is wired to a
//! caller yet — see the module-level "no production caller" note in this
//! phase's evidence entry.
//!
//! # Confidence is an escalation lever, not a report card
//!
//! Phase 35's line about escalating "uncertain tier assignments... conservatively"
//! is answered by [`TaskClassification::conservative_workload_tier`] and
//! [`TaskClassification::conservative_safe_for_disposable_model`], which never
//! read better than the raw fields and only ever move in the direction of
//! *more* capability or *less* trust — the same fail-closed shape
//! [`super::Cost::Metered`] already uses as its default.

use std::fmt;

/// Coarse complexity estimate — Phase 35's "estimate task complexity on a
/// coarse scale". Three bands, ordered, and nothing finer: a policy that
/// wants more resolution than this belongs to a later phase, not this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Complexity {
    Trivial,
    Moderate,
    Complex,
}

impl Complexity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Trivial => "trivial",
            Self::Moderate => "moderate",
            Self::Complex => "complex",
        }
    }
}

impl fmt::Display for Complexity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The coarse workload tier a task requires — Phase 35's "assign a required
/// workload tier to the task", widened to the map's five-tier system
/// (capability map lines 1395-1400 and 1404).
///
/// Ordered, so a policy may escalate by moving one step up
/// ([`WorkloadTier::escalate`]) without a `match` of its own. This is
/// deliberately not the same type as any future Phase 34F model-capability
/// ceiling: a task's *requirement* and a model's *ceiling* are compared by a
/// router, not merged into one enum, for the reason
/// [`super::AssignedModel`]'s doc comment gives for keeping "no model" and "a
/// named model" apart — collapsing a requirement and a capability into one
/// scale would let a router compare a task's tier against its own tier and
/// believe that proved something.
///
/// [`Self::Deterministic`] (Tier 0) and [`Self::Frontier`] (Tier 4) have no
/// producer yet: nothing in this module or its callers currently classifies
/// a task into either. That is deliberate — this project adds a variant when
/// its producer lands, never in advance (`src/evaluation/mod.rs:89` states
/// the same rule for its own enum) — and every consumer of this type must
/// stay exhaustive over all five so that the day a producer does exist, a
/// missed call site is a compile error rather than a silent wrong decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WorkloadTier {
    /// Tier 0: deterministic or trivial work that should not require an LLM
    /// when simple rules are sufficient (line 1396).
    Deterministic,
    /// Tier 1: lightweight classification, extraction, reranking,
    /// formatting, and simple factual codebase lookup (line 1397). A
    /// disposable, free, or local model is expected to be sufficient.
    Leaf,
    /// Tier 2: routine coding, bounded debugging, focused review, and small
    /// multi-file changes (line 1398). An ordinary interactive model.
    Standard,
    /// Tier 3: difficult debugging, architecture-sensitive changes, broad
    /// refactors, and work requiring strong reasoning or long-lived
    /// repository context (line 1399). The strongest configured model the
    /// session has, short of a Tier 4 need.
    Heavy,
    /// Tier 4: frontier work where failure cost or reasoning difficulty
    /// justifies the strongest available model or a warm premium session
    /// (line 1400).
    Frontier,
}

impl WorkloadTier {
    /// One step more capable, or unchanged at the top. Never a step down —
    /// there is no direction in which escalating a workload tier should make
    /// it cheaper.
    pub fn escalate(self) -> Self {
        match self {
            Self::Deterministic => Self::Leaf,
            Self::Leaf => Self::Standard,
            Self::Standard => Self::Heavy,
            Self::Heavy | Self::Frontier => Self::Frontier,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deterministic => "deterministic",
            Self::Leaf => "leaf",
            Self::Standard => "standard",
            Self::Heavy => "heavy",
            Self::Frontier => "frontier",
        }
    }
}

impl fmt::Display for WorkloadTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How much a policy should trust this classification — Phase 35's "return
/// classification confidence so uncertain tier assignments can be escalated
/// conservatively".
///
/// Three states rather than a bare number: a float invites a threshold
/// constant to drift between call sites, the way [`super::Cost`]'s own doc
/// comment warns a guessed boolean would. [`Self::Low`] is the one state a
/// caller must act on — see [`TaskClassification::conservative_workload_tier`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Confidence {
    /// Nothing distinguishing matched. Treat the raw fields as a guess.
    Low,
    /// At least one distinguishing signal matched.
    Medium,
    /// Established by a model whose classification is trusted outright,
    /// never produced by [`classify_heuristically`].
    High,
}

impl Confidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

impl fmt::Display for Confidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Whether the session's existing warm backend is likely worth more than a
/// stronger cold one — Phase 35's "estimate whether existing warm context is
/// likely more valuable than a stronger cold model".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarmContextValue {
    /// Keep the warm backend; switching would cost more than it buys.
    PreferWarm,
    /// A stronger cold model is likely worth the switch.
    PreferStrongerCold,
}

impl WarmContextValue {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PreferWarm => "prefer warm",
            Self::PreferStrongerCold => "prefer stronger cold",
        }
    }
}

impl fmt::Display for WarmContextValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What produced a classification — Phase 35's own preamble ("can run on a
/// cheap, free, or local model") told apart from its stated fallback
/// ("deterministic heuristics when no cheap model is available").
///
/// `label` is a diagnostic only, the same "names, never values" rule
/// [`super::CredentialId::label`] follows — this module never resolves a
/// credential, so there is nothing here that could be one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClassificationSource {
    /// A cheap, free, or local model produced this, named for the log line.
    Model { label: String },
    /// [`classify_heuristically`] produced this; no model was consulted.
    Heuristic,
}

impl ClassificationSource {
    pub fn is_heuristic(&self) -> bool {
        matches!(self, Self::Heuristic)
    }
}

impl fmt::Display for ClassificationSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Model { label } => write!(f, "model ({label})"),
            Self::Heuristic => f.write_str("deterministic heuristics"),
        }
    }
}

/// A capability requirement that a stronger text model cannot supply by
/// itself — Phase 35's "identify hard capability requirements that cannot be
/// satisfied merely by choosing a stronger text model".
///
/// Each variant names something a *harness* must be wired for — repository
/// access, a shell, a browser — rather than something a smarter model makes
/// more likely to succeed. [`TaskClassification::hard_capabilities`] derives
/// this set from the task's own signal fields; it is not a fourth place the
/// same information is recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HardCapability {
    RepositoryAccess,
    ShellExecution,
    BrowserInteraction,
}

impl HardCapability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RepositoryAccess => "repository access",
            Self::ShellExecution => "shell execution",
            Self::BrowserInteraction => "browser interaction",
        }
    }
}

impl fmt::Display for HardCapability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A structured, small classification of one request — Phase 35's "keep
/// classification output structured and small".
///
/// Every field is `Copy` except [`ClassificationSource`]'s optional label, so
/// the type stays cheap to carry through a routing decision and cheap to log
/// whole. `tests::the_classification_stays_small` pins the size bound this
/// doc comment claims.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskClassification {
    needs_repo_context: bool,
    needs_code_modification: bool,
    needs_shell_execution: bool,
    needs_browser_interaction: bool,
    complexity: Complexity,
    likely_multi_turn: bool,
    workload_tier: WorkloadTier,
    safe_for_disposable_model: bool,
    warm_context: WarmContextValue,
    confidence: Confidence,
    source: ClassificationSource,
}

impl TaskClassification {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        needs_repo_context: bool,
        needs_code_modification: bool,
        needs_shell_execution: bool,
        needs_browser_interaction: bool,
        complexity: Complexity,
        likely_multi_turn: bool,
        workload_tier: WorkloadTier,
        safe_for_disposable_model: bool,
        warm_context: WarmContextValue,
        confidence: Confidence,
        source: ClassificationSource,
    ) -> Self {
        Self {
            needs_repo_context,
            needs_code_modification,
            needs_shell_execution,
            needs_browser_interaction,
            complexity,
            likely_multi_turn,
            workload_tier,
            safe_for_disposable_model,
            warm_context,
            confidence,
            source,
        }
    }

    pub fn needs_repo_context(&self) -> bool {
        self.needs_repo_context
    }

    pub fn needs_code_modification(&self) -> bool {
        self.needs_code_modification
    }

    pub fn needs_shell_execution(&self) -> bool {
        self.needs_shell_execution
    }

    pub fn needs_browser_interaction(&self) -> bool {
        self.needs_browser_interaction
    }

    pub fn complexity(&self) -> Complexity {
        self.complexity
    }

    pub fn likely_multi_turn(&self) -> bool {
        self.likely_multi_turn
    }

    /// The raw tier this classification computed. Prefer
    /// [`Self::conservative_workload_tier`] for a routing decision — this
    /// accessor exists so a diagnostic can show the escalation happening
    /// rather than hiding the pre-escalation value.
    pub fn workload_tier(&self) -> WorkloadTier {
        self.workload_tier
    }

    /// The raw safety estimate. Prefer
    /// [`Self::conservative_safe_for_disposable_model`] for a routing
    /// decision, for the same reason as [`Self::workload_tier`].
    pub fn safe_for_disposable_model(&self) -> bool {
        self.safe_for_disposable_model
    }

    pub fn warm_context(&self) -> WarmContextValue {
        self.warm_context
    }

    pub fn confidence(&self) -> Confidence {
        self.confidence
    }

    pub fn source(&self) -> &ClassificationSource {
        &self.source
    }

    /// [`WorkloadTier`] escalated one step when confidence is
    /// [`Confidence::Low`], unchanged otherwise. The one function a router is
    /// expected to call instead of [`Self::workload_tier`] — Phase 35's
    /// "return classification confidence so uncertain tier assignments can
    /// be escalated conservatively".
    pub fn conservative_workload_tier(&self) -> WorkloadTier {
        match self.confidence {
            Confidence::Low => self.workload_tier.escalate(),
            Confidence::Medium | Confidence::High => self.workload_tier,
        }
    }

    /// [`Self::safe_for_disposable_model`], withdrawn when confidence is
    /// [`Confidence::Low`] — an uncertain classification must not be the
    /// reason a request is sent to a disposable model, the same fail-closed
    /// direction [`super::Cost::Metered`] takes when nothing marked a model
    /// free.
    pub fn conservative_safe_for_disposable_model(&self) -> bool {
        self.safe_for_disposable_model && self.confidence != Confidence::Low
    }

    /// The hard capability requirements this task implies — Phase 35's line
    /// on requirements a stronger text model cannot satisfy by itself.
    /// Derived from the signal fields rather than stored, so there is one
    /// place that can disagree with itself.
    pub fn hard_capabilities(&self) -> Vec<HardCapability> {
        let mut caps = Vec::new();
        if self.needs_repo_context {
            caps.push(HardCapability::RepositoryAccess);
        }
        if self.needs_shell_execution {
            caps.push(HardCapability::ShellExecution);
        }
        if self.needs_browser_interaction {
            caps.push(HardCapability::BrowserInteraction);
        }
        caps
    }
}

/// Requests classified as pure questions with no repository-specific
/// reference — the one case [`classify_heuristically`] treats as not needing
/// repository context, code modification, a shell, or a browser.
const QUESTION_KEYWORDS: &[&str] = &[
    "what is",
    "what's",
    "what are",
    "what does",
    "explain",
    "define",
    "how does",
    "why does",
    "why is",
    "difference between",
];

/// Signals that a question is actually about this repository, even though it
/// reads like a question — these override [`QUESTION_KEYWORDS`] back to
/// "needs repository context".
const REPO_REFERENCE_KEYWORDS: &[&str] = &[
    "this repo",
    "this file",
    "this function",
    "this code",
    "this project",
    "the codebase",
    ".rs",
    ".py",
    ".ts",
    ".tsx",
    ".js",
    ".go",
    ".md",
    "readme",
];

const SHELL_KEYWORDS: &[&str] = &[
    "run ", "execute", "install ", "build ", "compile", "npm ", "cargo ", "pip ", "make ",
    "pytest", "terminal", "shell", "deploy", "curl ",
];

const BROWSER_KEYWORDS: &[&str] = &[
    "browser",
    "click ",
    "screenshot",
    "webpage",
    "web page",
    "website",
    "navigate to",
    "chrome",
];

const CODE_MODIFICATION_KEYWORDS: &[&str] = &[
    "fix ",
    "implement",
    "add a",
    "add an",
    "refactor",
    "rewrite",
    "update ",
    "edit ",
    "write a",
    "create a",
    "remove ",
    "delete ",
    "patch ",
    "bug",
];

fn matches_any(lower: &str, keywords: &[&str]) -> bool {
    keywords.iter().any(|keyword| lower.contains(keyword))
}

/// Classify a request from its text alone, with no model call — Phase 35's
/// "allow classification to fall back to deterministic heuristics when no
/// cheap model is available".
///
/// A pure function of `request_text`: same text in, same
/// [`TaskClassification`] out, always. Every signal is fail-closed in the
/// direction of assuming *more* is required — an ambiguous request is
/// treated as needing repository context and produces
/// [`Confidence::Low`] rather than guessing a cheaper answer, the same
/// instinct [`super::Cost::Metered`]'s doc comment states for a model nobody
/// marked free.
pub fn classify_heuristically(request_text: &str) -> TaskClassification {
    let lower = request_text.to_lowercase();

    let is_question = matches_any(&lower, QUESTION_KEYWORDS);
    let references_repo = matches_any(&lower, REPO_REFERENCE_KEYWORDS);
    let needs_shell_execution = matches_any(&lower, SHELL_KEYWORDS);
    let needs_browser_interaction = matches_any(&lower, BROWSER_KEYWORDS);
    let needs_code_modification = if !is_question || references_repo {
        matches_any(&lower, CODE_MODIFICATION_KEYWORDS) || needs_shell_execution
    } else {
        false
    };

    // A pure question with no repository reference is the one case this
    // heuristic treats as not needing repository context — every other
    // request defaults to needing it, which is the conservative direction.
    let needs_repo_context = !is_question || references_repo || needs_code_modification;

    let matched_any_signal = is_question
        || references_repo
        || needs_shell_execution
        || needs_browser_interaction
        || needs_code_modification;

    let complexity = if needs_shell_execution || needs_browser_interaction {
        Complexity::Complex
    } else if needs_code_modification {
        Complexity::Moderate
    } else {
        Complexity::Trivial
    };

    let likely_multi_turn =
        needs_shell_execution || needs_browser_interaction || needs_code_modification;

    let workload_tier = if needs_shell_execution || needs_browser_interaction {
        WorkloadTier::Heavy
    } else if needs_code_modification {
        WorkloadTier::Standard
    } else {
        WorkloadTier::Leaf
    };

    // `== Leaf`, not a threshold: `workload_tier` was just assigned three
    // lines above from a match with exactly three arms (Heavy, Standard,
    // Leaf), so Tier 0 and Tier 4 can never reach here — there is no
    // top-of-scale boundary for this equality to fall on the wrong side of,
    // unlike `quota.rs`'s comparisons against a tier supplied by an
    // arbitrary external caller.
    let safe_for_disposable_model =
        workload_tier == WorkloadTier::Leaf && !needs_repo_context && !likely_multi_turn;

    let warm_context = if likely_multi_turn {
        WarmContextValue::PreferWarm
    } else {
        WarmContextValue::PreferStrongerCold
    };

    let confidence = if matched_any_signal {
        Confidence::Medium
    } else {
        Confidence::Low
    };

    TaskClassification::new(
        needs_repo_context,
        needs_code_modification,
        needs_shell_execution,
        needs_browser_interaction,
        complexity,
        likely_multi_turn,
        workload_tier,
        safe_for_disposable_model,
        warm_context,
        confidence,
        ClassificationSource::Heuristic,
    )
}

/// Classify a request, preferring an already-produced model classification
/// and falling back to [`classify_heuristically`] when there is none.
///
/// `model_output` is never called for here — the caller is expected to have
/// already asked a cheap, free, or local model (if one is configured; see
/// `crate::config::RoutingModelResolution`) and supply the result. Passing
/// `None` is exactly Phase 35's "no cheap model available" case.
pub fn classify(
    request_text: &str,
    model_output: Option<TaskClassification>,
) -> TaskClassification {
    model_output.unwrap_or_else(|| classify_heuristically(request_text))
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

/// What `glasshouse classify` prints — the production caller of [`classify`],
/// and the function `main.rs`'s `classify` arm calls.
///
/// No cheap model is wired up in this build, so this always calls
/// `classify(request_text, None)`: Phase 35's "fall back to deterministic
/// heuristics when no cheap model is available" is not a fallback for this
/// caller, it is the only path available, and the report's `source` line
/// says so on every run rather than implying a model was consulted.
pub fn report(request_text: &str) -> String {
    use std::fmt::Write as _;

    let result = classify(request_text, None);
    let mut out = String::new();

    let _ = writeln!(out, "Glasshouse task classification");
    let _ = writeln!(out, "===============================");
    let _ = writeln!(out);
    let _ = writeln!(out, "request                 {request_text:?}");
    let _ = writeln!(out, "source                  {}", result.source());
    let _ = writeln!(out);

    let _ = writeln!(out, "Signals");
    let _ = writeln!(
        out,
        "  repository context      {}",
        yes_no(result.needs_repo_context())
    );
    let _ = writeln!(
        out,
        "  code modification       {}",
        yes_no(result.needs_code_modification())
    );
    let _ = writeln!(
        out,
        "  shell execution         {}",
        yes_no(result.needs_shell_execution())
    );
    let _ = writeln!(
        out,
        "  browser interaction     {}",
        yes_no(result.needs_browser_interaction())
    );
    let _ = writeln!(out);

    let _ = writeln!(out, "Estimates");
    let _ = writeln!(out, "  complexity              {}", result.complexity());
    let _ = writeln!(
        out,
        "  likely multi-turn       {}",
        yes_no(result.likely_multi_turn())
    );
    let _ = writeln!(out, "  warm context            {}", result.warm_context());
    let _ = writeln!(out);

    let _ = writeln!(out, "Routing");
    let _ = writeln!(out, "  confidence              {}", result.confidence());
    let _ = writeln!(
        out,
        "  workload tier           {} (conservative: {})",
        result.workload_tier(),
        result.conservative_workload_tier()
    );
    let _ = writeln!(
        out,
        "  safe for disposable     {} (conservative: {})",
        yes_no(result.safe_for_disposable_model()),
        yes_no(result.conservative_safe_for_disposable_model())
    );
    let caps = result.hard_capabilities();
    let _ = writeln!(
        out,
        "  hard capabilities       {}",
        if caps.is_empty() {
            "none".to_owned()
        } else {
            caps.iter()
                .copied()
                .map(HardCapability::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        }
    );

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_classification_stays_small() {
        // Pins Phase 35's "keep classification output structured and
        // small": no accidental `Vec`, `HashMap`, or unbounded `String`
        // sitting directly on the struct. `ClassificationSource::Model`'s
        // label is the only heap allocation the type can carry, and it is
        // optional (the heuristic path never allocates one).
        assert!(
            std::mem::size_of::<TaskClassification>() <= 64,
            "TaskClassification grew past a small, structured shape: {} bytes",
            std::mem::size_of::<TaskClassification>()
        );
    }

    #[test]
    fn a_shell_command_request_is_heavy_and_multi_turn() {
        let c = classify_heuristically("run cargo test and fix whatever fails");
        assert!(c.needs_shell_execution());
        assert!(c.needs_code_modification());
        assert!(c.needs_repo_context());
        assert!(!c.needs_browser_interaction());
        assert_eq!(c.complexity(), Complexity::Complex);
        assert!(c.likely_multi_turn());
        assert_eq!(c.workload_tier(), WorkloadTier::Heavy);
        assert!(!c.safe_for_disposable_model());
        assert_eq!(c.warm_context(), WarmContextValue::PreferWarm);
        assert_eq!(c.confidence(), Confidence::Medium);
    }

    #[test]
    fn a_browser_task_needs_browser_interaction_and_no_repo_context_is_assumed_needed() {
        let c = classify_heuristically("open the browser and take a screenshot of the homepage");
        assert!(c.needs_browser_interaction());
        assert_eq!(c.workload_tier(), WorkloadTier::Heavy);
        assert!(
            c.hard_capabilities()
                .contains(&HardCapability::BrowserInteraction)
        );
    }

    #[test]
    fn a_generic_question_needs_no_repo_context_and_is_leaf_tier() {
        let c = classify_heuristically("what is a mutex?");
        assert!(!c.needs_repo_context());
        assert!(!c.needs_code_modification());
        assert!(!c.needs_shell_execution());
        assert!(!c.needs_browser_interaction());
        assert_eq!(c.complexity(), Complexity::Trivial);
        assert!(!c.likely_multi_turn());
        assert_eq!(c.workload_tier(), WorkloadTier::Leaf);
        assert!(c.safe_for_disposable_model());
        assert_eq!(c.warm_context(), WarmContextValue::PreferStrongerCold);
        assert!(c.hard_capabilities().is_empty());
    }

    #[test]
    fn a_question_about_a_named_file_still_needs_repo_context() {
        let c = classify_heuristically("what does auth.rs do?");
        assert!(
            c.needs_repo_context(),
            "a question naming a repository file must not be treated as a generic question"
        );
    }

    #[test]
    fn an_ambiguous_request_gets_low_confidence_and_escalates() {
        let c = classify_heuristically("thing");
        assert_eq!(c.confidence(), Confidence::Low);
        assert_eq!(c.workload_tier(), WorkloadTier::Leaf);
        assert_eq!(
            c.conservative_workload_tier(),
            WorkloadTier::Standard,
            "a low-confidence classification must escalate one tier, not stay at leaf"
        );
    }

    #[test]
    fn low_confidence_withdraws_disposable_safety_even_when_the_raw_fields_say_safe() {
        // Constructed directly rather than through `classify_heuristically`:
        // that function never pairs a raw-safe classification with
        // `Confidence::Low` (a Low reading has no matched signal at all,
        // which also defaults `needs_repo_context` to true and makes the
        // raw fields un-safe on their own). The accessor's job is to hold
        // even for a source that could produce this combination — a future
        // model-backed classifier is exactly such a source.
        let c = TaskClassification::new(
            false,
            false,
            false,
            false,
            Complexity::Trivial,
            false,
            WorkloadTier::Leaf,
            true,
            WarmContextValue::PreferStrongerCold,
            Confidence::Low,
            ClassificationSource::Heuristic,
        );
        assert!(c.safe_for_disposable_model());
        assert!(
            !c.conservative_safe_for_disposable_model(),
            "an uncertain classification must not be trusted to route to a disposable model"
        );
    }

    #[test]
    fn medium_confidence_does_not_escalate() {
        let c = classify_heuristically("what is a mutex?");
        assert_eq!(c.confidence(), Confidence::Medium);
        assert_eq!(c.conservative_workload_tier(), c.workload_tier());
        assert_eq!(
            c.conservative_safe_for_disposable_model(),
            c.safe_for_disposable_model()
        );
    }

    #[test]
    fn workload_tier_escalation_never_goes_past_frontier() {
        // Heavy was the top before this batch and saturated at itself; now
        // Frontier is the top and Heavy takes the one-step-up path like
        // every other non-top variant.
        assert_eq!(WorkloadTier::Frontier.escalate(), WorkloadTier::Frontier);
        assert_eq!(WorkloadTier::Heavy.escalate(), WorkloadTier::Frontier);
        assert_eq!(WorkloadTier::Standard.escalate(), WorkloadTier::Heavy);
        assert_eq!(WorkloadTier::Leaf.escalate(), WorkloadTier::Standard);
        assert_eq!(WorkloadTier::Deterministic.escalate(), WorkloadTier::Leaf);
    }

    #[test]
    fn classify_prefers_a_supplied_model_output_over_the_heuristic() {
        let model_answer = TaskClassification::new(
            false,
            false,
            false,
            false,
            Complexity::Trivial,
            false,
            WorkloadTier::Leaf,
            true,
            WarmContextValue::PreferStrongerCold,
            Confidence::High,
            ClassificationSource::Model {
                label: "test-cheap-model".to_owned(),
            },
        );
        let result = classify("run cargo test", Some(model_answer.clone()));
        assert_eq!(result, model_answer);
    }

    #[test]
    fn classify_falls_back_to_heuristics_when_no_model_output_is_supplied() {
        let result = classify("run cargo test", None);
        assert!(result.source().is_heuristic());
        assert_eq!(result, classify_heuristically("run cargo test"));
    }

    #[test]
    fn report_says_no_model_was_consulted_and_shows_the_signals() {
        let text = report("run cargo test and fix whatever fails");
        assert!(
            text.contains("deterministic heuristics"),
            "no cheap model is wired up in this build, so the report must say so, not imply a \
             model answered:\n{text}"
        );
        assert!(text.contains("shell execution         yes"), "{text}");
        assert!(text.contains("workload tier           heavy"), "{text}");
    }

    #[test]
    fn hard_capabilities_are_derived_not_stored_separately() {
        let shell_only = classify_heuristically("run the build");
        assert_eq!(
            shell_only.hard_capabilities(),
            vec![
                HardCapability::RepositoryAccess,
                HardCapability::ShellExecution
            ]
        );
    }
}
