//! Lightweight task classification — Phase 35.
//!
//! [`classify_heuristically`] is a pure, deterministic function of the
//! request text, and [`classify`]'s model path takes an *already-produced*
//! [`TaskClassification`] as an argument rather than calling anything
//! itself, so the module never makes a network call of its own —
//! [`mod@super`]'s own doc comment states the same discipline for the two
//! routing-policy classes.
//!
//! `crate::config::RoutingModelChoice` decides *whether* a routing model is
//! configured; this module is downstream of that decision, not a duplicate:
//! a caller turns a model reply into a [`TaskClassification`] and hands it to
//! [`classify`], or falls through to [`classify_heuristically`] when no
//! routing model is configured. Neither path has a production caller yet.
//!
//! [`TaskClassification::conservative_workload_tier`] and
//! [`TaskClassification::conservative_safe_for_disposable_model`] never read
//! better than the raw fields and only ever move toward *more* capability or
//! *less* trust — the same fail-closed shape [`super::Cost::Metered`] uses.
// History: design-decisions.md, "Trims: routing module docs", routing/classify.rs module doc.

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
/// ([`WorkloadTier::escalate`]) without a `match` of its own. A task's
/// *requirement* and a model's *ceiling* stay separate types, compared by a
/// router, rather than one merged scale — the same reason
/// [`super::AssignedModel`] keeps "no model" and "a named model" apart.
///
/// [`Self::Deterministic`] (Tier 0) and [`Self::Frontier`] (Tier 4) have no
/// producer yet — this project adds a variant when its producer lands, never
/// in advance (`src/evaluation/mod.rs:89` states the same rule for its own
/// enum) — and every consumer must stay exhaustive over all five so a missed
/// call site is a compile error, not a silent wrong decision, the day one
/// does land.
// History: design-decisions.md, "Trims: routing module docs", routing/classify.rs `WorkloadTier` doc.
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

/// How long the work is expected to run — Phase 34D's "expected duration
/// class" (map line 1457), on three coarse bands like [`Complexity`].
///
/// Ordered, so a conservative reading can move *up* the scale the way
/// [`WorkloadTier::escalate`] does: planning for a longer session than the
/// work needs costs a little warmth-preference; planning for a shorter one
/// costs the person their context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DurationClass {
    /// One exchange and done.
    SingleTurn,
    /// A handful of exchanges with a clear end.
    FewTurns,
    /// Open-ended, multi-turn work that will want to keep its context.
    LongRunning,
}

impl DurationClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SingleTurn => "single turn",
            Self::FewTurns => "a few turns",
            Self::LongRunning => "long-running",
        }
    }

    /// The class the signal fields imply when nothing stated one: the same
    /// fail-closed direction every other derived value in this module takes.
    pub fn derived_from(classification: &TaskClassification) -> Self {
        if !classification.likely_multi_turn() {
            Self::SingleTurn
        } else if classification.complexity() < Complexity::Complex {
            Self::FewTurns
        } else {
            Self::LongRunning
        }
    }
}

impl fmt::Display for DurationClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The execution shape a router answer may recommend — map line 1458's three
/// words, and no others.
///
/// A *recommendation*, carried and rendered: the session router's own ranking
/// (`super::session`) still decides between continuing and starting, because
/// it weighs facts about the candidates — warmth, quota, health — that a
/// classifier of the request text alone cannot see. What this adds is the
/// classifier's view of the *work*: whether it is the kind that wants its
/// context kept, the kind that wants a clean start, or the kind a throwaway
/// model could absorb.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionShape {
    ReuseSession,
    NewSession,
    DisposableJob,
}

impl ExecutionShape {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReuseSession => "reuse session",
            Self::NewSession => "new session",
            Self::DisposableJob => "disposable job",
        }
    }

    /// The shape the signal fields imply when nothing stated one. Reads the
    /// **conservative** disposable-safety accessor, so a low-confidence
    /// classification can never derive its way to a throwaway model.
    pub fn derived_from(classification: &TaskClassification) -> Self {
        if classification.conservative_safe_for_disposable_model() {
            Self::DisposableJob
        } else if classification.likely_multi_turn()
            || classification.warm_context() == WarmContextValue::PreferWarm
        {
            Self::ReuseSession
        } else {
            Self::NewSession
        }
    }
}

impl fmt::Display for ExecutionShape {
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
    /// Map line 1457's duration class, **when the producer stated one**.
    /// `None` is not a default: it means the classifier said nothing, and
    /// [`DurationClass::derived_from`] is what a reader falls back to — a
    /// deterministic function of fields the producer *did* state, never an
    /// invented value wearing the producer's source.
    duration: Option<DurationClass>,
    /// Map line 1458's execution shape, on the same terms as `duration`.
    execution_shape: Option<ExecutionShape>,
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
            duration: None,
            execution_shape: None,
        }
    }

    /// Attach the duration class a producer stated. Builder rather than a
    /// twelfth constructor argument, so every existing caller of
    /// [`Self::new`] — and every reply that predates the field — is
    /// unchanged and reads as "not stated".
    pub fn with_duration(mut self, duration: Option<DurationClass>) -> Self {
        self.duration = duration;
        self
    }

    /// Attach the execution shape a producer stated — see [`Self::with_duration`].
    pub fn with_execution_shape(mut self, shape: Option<ExecutionShape>) -> Self {
        self.execution_shape = shape;
        self
    }

    /// The duration class the producer stated, if it stated one. Prefer
    /// [`Self::expected_duration`] for a decision.
    pub fn stated_duration(&self) -> Option<DurationClass> {
        self.duration
    }

    /// The execution shape the producer stated, if it stated one. Prefer
    /// [`Self::expected_execution_shape`] for a decision.
    pub fn stated_execution_shape(&self) -> Option<ExecutionShape> {
        self.execution_shape
    }

    /// Line 1457's duration class as a router should read it: what the
    /// producer stated, or what its signal fields imply — and
    /// [`DurationClass::LongRunning`] whenever confidence is
    /// [`Confidence::Low`], for the reason [`Self::conservative_workload_tier`]
    /// escalates: an uncertain answer is planned for as the more demanding
    /// case, never the cheaper one.
    pub fn expected_duration(&self) -> DurationClass {
        let stated_or_derived = self
            .duration
            .unwrap_or_else(|| DurationClass::derived_from(self));
        match self.confidence {
            Confidence::Low => DurationClass::LongRunning,
            Confidence::Medium | Confidence::High => stated_or_derived,
        }
    }

    /// Line 1458's execution shape as a router should read it: what the
    /// producer stated, or what its signal fields imply — except that a
    /// stated [`ExecutionShape::DisposableJob`] is withdrawn whenever
    /// [`Self::conservative_safe_for_disposable_model`] is false, which is
    /// the same fail-closed rule that accessor states for the raw flag.
    pub fn expected_execution_shape(&self) -> ExecutionShape {
        match self.execution_shape {
            Some(ExecutionShape::DisposableJob) | None => ExecutionShape::derived_from(self),
            Some(stated) => stated,
        }
    }

    /// Whether this is the kind of work a sticky session may keep absorbing
    /// without asking the routing model again — map line 1467's *"repeated
    /// low-risk turns"*, defined from the fields this type already carries
    /// and not from a second reading of the request text.
    ///
    /// Low-risk means: nothing is modified, nothing is executed, no browser
    /// is driven, the tier is at most [`WorkloadTier::Standard`], and the
    /// producer was at least [`Confidence::Medium`] — a low-confidence
    /// classification is exactly the one that should be asked about again.
    pub fn is_low_risk(&self) -> bool {
        !self.needs_code_modification
            && !self.needs_shell_execution
            && !self.needs_browser_interaction
            && self.conservative_workload_tier() <= WorkloadTier::Standard
            && self.confidence != Confidence::Low
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

/// What a classification model is told before it is shown the request.
///
/// A `&'static str` and therefore a literal in the binary, which is what
/// lets [`crate::memory::extract::Prompt::for_request`] accept it without scrubbing
/// it: the only unscrubbed half of a classification prompt is text this
/// repository wrote. See that constructor for why the type says so.
///
/// Deliberately **not** [`crate::memory::extract::schema::PROMPT_CONTRACT`].
/// That one asks for a document of durable memories out of a session
/// transcript; this one asks for ten fields about one request. Sharing them
/// would mean one prompt trying to be two, and the reply schemas have no
/// field in common.
pub const CLASSIFICATION_PROMPT_CONTRACT: &str = "\
You are a routing classifier inside a developer tool. You are given one \
request a person has made to a coding agent. Your whole job is to describe \
that request so a router can decide which model should answer it.

Do not answer the request. Do not explain your reasoning. Do not apologise \
for anything. Reply with one JSON object and nothing else.

Every field below is required. If you are unsure of a field, still emit it, \
and set \"confidence\" to \"low\" so the router can escalate — a missing \
field is a failed classification and the tool falls back to its own \
heuristics.
";

/// The reply shape a classification model must produce.
///
/// One flat object with ten keys and no nesting: this is the smallest thing
/// that can carry a [`TaskClassification`], and every key here is a field
/// [`TaskClassification::new`] takes.
///
/// **`hard_capabilities` is deliberately absent.**
/// [`TaskClassification::hard_capabilities`] derives it from the four signal
/// booleans, and `tests::hard_capabilities_are_derived_not_stored_separately`
/// pins that. Asking a model for it would create a second place the same
/// fact is recorded, which is the one thing that type's doc comment refuses.
/// `source` is absent for the same reason — it is a fact about who answered,
/// which the caller knows and the model does not.
pub const CLASSIFICATION_RESPONSE_SCHEMA: &str = r#"
## Reply with exactly this shape

{
  "needs_repo_context": true | false,
  "needs_code_modification": true | false,
  "needs_shell_execution": true | false,
  "needs_browser_interaction": true | false,
  "complexity": "trivial" | "moderate" | "complex",
  "likely_multi_turn": true | false,
  "workload_tier": "deterministic" | "leaf" | "standard" | "heavy" | "frontier",
  "safe_for_disposable_model": true | false,
  "warm_context": "prefer_warm" | "prefer_stronger_cold",
  "confidence": "low" | "medium" | "high",
  "expected_duration": "single_turn" | "few_turns" | "long_running",
  "execution_shape": "reuse_session" | "new_session" | "disposable_job"
}

## What each field means

- needs_repo_context: answering well requires reading this repository.
- needs_code_modification: the request asks for code to be written or changed.
- needs_shell_execution: a command must actually be run.
- needs_browser_interaction: a browser must actually be driven.
- complexity: how hard the work is, on three coarse bands.
- likely_multi_turn: this will take several exchanges rather than one.
- workload_tier: the weakest model that could do this acceptably.
  deterministic = no model needed at all; leaf = a cheap or local model;
  standard = an ordinary interactive model; heavy = strong reasoning or
  long-lived repository context; frontier = the strongest model available.
- safe_for_disposable_model: this could be handed to a throwaway free model
  without harming the session.
- warm_context: whether the session's existing warm backend is worth more
  than a stronger cold one for this request.
- confidence: how much the router should trust the fields above.
- expected_duration: how long the work will run. single_turn = one exchange;
  few_turns = a handful with a clear end; long_running = open-ended work that
  wants to keep its context. Optional — omit it if unsure.
- execution_shape: where the work should go. reuse_session = continue the
  warm session if one exists; new_session = start clean; disposable_job = a
  throwaway model could do this. Optional — omit it if unsure.

## The routing request

Everything below was assembled by the tool from the request and a few facts
about the session — never from repository files or conversation history.

"#;

/// Why a model's reply could not be read as a classification.
///
/// # No provider text, ever
///
/// The same rule `crate::memory::extract::model`'s module header states and
/// [`crate::memory::ModelError`] was given a `&'static str` to enforce: a
/// reply answers a prompt built from the user's own request, and a provider's
/// error body can echo it. So every variant here carries either nothing or a
/// **field name this file wrote** — never a value, never a fragment of the
/// reply, never a `serde` message (which names a type and a position but is
/// still a string this module did not choose).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClassificationParseError {
    /// Nothing in the reply was a JSON object.
    #[error("the routing model's reply was not JSON")]
    NotJson,
    /// A required field was missing, or held the wrong JSON type.
    #[error("the routing model's reply is missing `{field}` or gave it the wrong type")]
    MissingField { field: &'static str },
    /// A field was present and a string, and named nothing this enum has.
    #[error("the routing model's reply gave `{field}` a value this build does not recognise")]
    UnknownValue { field: &'static str },
}

/// The outermost `{…}` in `reply`, by brace balance.
///
/// Models put JSON inside prose and inside ```` ```json ```` fences, and
/// tolerating exactly those two things is the difference between a parser
/// that works against real providers and one that works against a fixture.
/// Nothing else is tolerated: a reply with no balanced object in it is a
/// failure, not something to guess at.
///
/// Brace counting rather than `find('{')` and `rfind('}')`, because a reply
/// whose trailing prose contains a `}` would otherwise capture it. Strings
/// are tracked so a brace inside one does not move the depth.
///
/// This repeats `crate::memory::extract::schema`'s own private helper on
/// purpose. Sharing it would make a routing module depend on memory
/// extraction's reply schema — the one coupling the recon that specified this
/// work spent most of its length arguing against, since the transport is
/// reusable and the schema is not.
fn outermost_json_object(reply: &str) -> Option<&str> {
    let start = reply.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (offset, c) in reply[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&reply[start..start + offset + c.len_utf8()]);
                }
            }
            _ => {}
        }
    }
    None
}

fn required_bool(
    document: &serde_json::Value,
    field: &'static str,
) -> Result<bool, ClassificationParseError> {
    document
        .get(field)
        .and_then(serde_json::Value::as_bool)
        .ok_or(ClassificationParseError::MissingField { field })
}

fn required_str<'a>(
    document: &'a serde_json::Value,
    field: &'static str,
) -> Result<&'a str, ClassificationParseError> {
    document
        .get(field)
        .and_then(serde_json::Value::as_str)
        .ok_or(ClassificationParseError::MissingField { field })
}

/// A string field that may be absent — see `parse_classification`'s note on
/// the two recommendation fields. A present non-string reads as absent for
/// the same reason an unknown value does: the producer stated nothing this
/// build can act on.
fn optional_str<'a>(document: &'a serde_json::Value, field: &'static str) -> Option<&'a str> {
    document.get(field).and_then(serde_json::Value::as_str)
}

/// Read one model reply as a [`TaskClassification`] attributed to `label`.
///
/// A model that omits `workload_tier` has not classified the request, and a
/// classification assembled around a default would be a fabrication wearing
/// [`ClassificationSource::Model`] — indistinguishable downstream from a tier
/// the model actually chose — so this returns an error and the caller falls
/// back to [`classify_heuristically`], which is honest about being a
/// heuristic.
///
/// `expected_duration` and `execution_shape` (map lines 1457, 1458) are the
/// exception: read when present, **`None` when absent or unrecognised**,
/// never an error and never stored as if the model had said it. Every
/// reader goes through [`TaskClassification::expected_duration`] and
/// [`TaskClassification::expected_execution_shape`], so a reply predating the
/// two keys parses exactly as it always did, and an invented fourth shape
/// reads as recommending nothing rather than as a failed classification.
///
/// `label` names the resource that answered — the caller's own description
/// of a model it configured, never anything the reply said.
// History: design-decisions.md, "Trims: routing module docs", routing/classify.rs `fn parse_classification`.
pub fn parse_classification(
    reply: &str,
    label: impl Into<String>,
) -> Result<TaskClassification, ClassificationParseError> {
    let body = outermost_json_object(reply).ok_or(ClassificationParseError::NotJson)?;
    let document: serde_json::Value =
        serde_json::from_str(body).map_err(|_| ClassificationParseError::NotJson)?;

    let complexity = match required_str(&document, "complexity")? {
        "trivial" => Complexity::Trivial,
        "moderate" => Complexity::Moderate,
        "complex" => Complexity::Complex,
        _ => {
            return Err(ClassificationParseError::UnknownValue {
                field: "complexity",
            });
        }
    };
    let workload_tier = match required_str(&document, "workload_tier")? {
        "deterministic" => WorkloadTier::Deterministic,
        "leaf" => WorkloadTier::Leaf,
        "standard" => WorkloadTier::Standard,
        "heavy" => WorkloadTier::Heavy,
        "frontier" => WorkloadTier::Frontier,
        _ => {
            return Err(ClassificationParseError::UnknownValue {
                field: "workload_tier",
            });
        }
    };
    let warm_context = match required_str(&document, "warm_context")? {
        "prefer_warm" => WarmContextValue::PreferWarm,
        "prefer_stronger_cold" => WarmContextValue::PreferStrongerCold,
        _ => {
            return Err(ClassificationParseError::UnknownValue {
                field: "warm_context",
            });
        }
    };
    let confidence = match required_str(&document, "confidence")? {
        "low" => Confidence::Low,
        "medium" => Confidence::Medium,
        "high" => Confidence::High,
        _ => {
            return Err(ClassificationParseError::UnknownValue {
                field: "confidence",
            });
        }
    };

    let duration = match optional_str(&document, "expected_duration") {
        Some("single_turn") => Some(DurationClass::SingleTurn),
        Some("few_turns") => Some(DurationClass::FewTurns),
        Some("long_running") => Some(DurationClass::LongRunning),
        Some(_) | None => None,
    };
    let execution_shape = match optional_str(&document, "execution_shape") {
        Some("reuse_session") => Some(ExecutionShape::ReuseSession),
        Some("new_session") => Some(ExecutionShape::NewSession),
        Some("disposable_job") => Some(ExecutionShape::DisposableJob),
        Some(_) | None => None,
    };

    Ok(TaskClassification::new(
        required_bool(&document, "needs_repo_context")?,
        required_bool(&document, "needs_code_modification")?,
        required_bool(&document, "needs_shell_execution")?,
        required_bool(&document, "needs_browser_interaction")?,
        complexity,
        required_bool(&document, "likely_multi_turn")?,
        workload_tier,
        required_bool(&document, "safe_for_disposable_model")?,
        warm_context,
        confidence,
        ClassificationSource::Model {
            label: label.into(),
        },
    )
    .with_duration(duration)
    .with_execution_shape(execution_shape))
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

/// What `glasshouse classify` prints — the production caller of [`classify`],
/// and the function `main.rs`'s `classify` arm calls.
///
/// `model_output` is whatever a configured routing model answered, and
/// [`None`] is Phase 35's "no cheap model is available" — either because the
/// user configured none, or because the one they configured could not be
/// reached or did not answer in the schema. This function does not know
/// which: `main.rs` says so on standard error at the point it finds out, and
/// the report's `source` line says which of the two kinds of answer this is
/// on every run rather than implying a model was consulted.
///
/// It takes an already-produced classification rather than fetching one for
/// the same reason [`classify`] does, and this module's header states: a
/// classifier that made a network call from inside a pure function would not
/// be lightweight, and could not be tested without one.
pub fn report(request_text: &str, model_output: Option<TaskClassification>) -> String {
    use std::fmt::Write as _;

    let result = classify(request_text, model_output);
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
    let _ = writeln!(
        out,
        "  expected duration       {}{}",
        result.expected_duration(),
        stated_or_derived(result.stated_duration().is_some())
    );
    let _ = writeln!(
        out,
        "  execution shape         {}{}",
        result.expected_execution_shape(),
        stated_or_derived(result.stated_execution_shape().is_some())
    );

    out
}

/// The suffix `report` prints beside a recommendation, so a reader can tell a
/// value the producer stated from one derived from its other fields.
fn stated_or_derived(stated: bool) -> &'static str {
    if stated {
        ""
    } else {
        " (derived; the classifier stated none)"
    }
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
        let text = report("run cargo test and fix whatever fails", None);
        assert!(
            text.contains("deterministic heuristics"),
            "no cheap model is wired up in this build, so the report must say so, not imply a \
             model answered:\n{text}"
        );
        assert!(text.contains("shell execution         yes"), "{text}");
        assert!(text.contains("workload tier           heavy"), "{text}");
    }

    /// Line 1467's predicate, bracketed on each clause: a confident question
    /// at a modest tier is low-risk; low confidence, modification, execution
    /// and a heavy tier each disqualify on their own.
    #[test]
    fn low_risk_is_a_confident_question_at_a_modest_tier_and_nothing_else() {
        assert!(classify_heuristically("what is a mutex?").is_low_risk());
        assert!(
            !classify_heuristically("thing").is_low_risk(),
            "a low-confidence classification is exactly the one to ask about again"
        );
        assert!(!classify_heuristically("fix the bug in auth.rs").is_low_risk());
        assert!(!classify_heuristically("run cargo test").is_low_risk());
        assert!(!classify_heuristically("open the browser").is_low_risk());
        let heavy_but_read_only = TaskClassification::new(
            true,
            false,
            false,
            false,
            Complexity::Complex,
            true,
            WorkloadTier::Heavy,
            false,
            WarmContextValue::PreferWarm,
            Confidence::High,
            ClassificationSource::Heuristic,
        );
        assert!(
            !heavy_but_read_only.is_low_risk(),
            "a heavy tier is not low-risk even when nothing is modified"
        );
    }

    /// Lines 1457/1458: the recommendations derive from the stated fields,
    /// and a stated value wins over the derivation except where line 1459
    /// withdraws it.
    #[test]
    fn recommendations_derive_from_the_stated_fields_and_a_stated_value_wins() {
        let question = classify_heuristically("what is a mutex?");
        assert_eq!(question.stated_duration(), None);
        assert_eq!(question.expected_duration(), DurationClass::SingleTurn);
        assert_eq!(
            question.expected_execution_shape(),
            ExecutionShape::DisposableJob,
            "a confident, self-contained question may go to a throwaway model"
        );
        let stated = question
            .clone()
            .with_duration(Some(DurationClass::FewTurns))
            .with_execution_shape(Some(ExecutionShape::NewSession));
        assert_eq!(stated.expected_duration(), DurationClass::FewTurns);
        assert_eq!(
            stated.expected_execution_shape(),
            ExecutionShape::NewSession
        );

        let shell = classify_heuristically("run cargo test and fix whatever fails");
        assert_eq!(shell.expected_duration(), DurationClass::LongRunning);
        assert_eq!(
            shell.expected_execution_shape(),
            ExecutionShape::ReuseSession
        );
        assert_ne!(
            shell
                .with_execution_shape(Some(ExecutionShape::DisposableJob))
                .expected_execution_shape(),
            ExecutionShape::DisposableJob,
            "a stated disposable shape is withdrawn when the work is not safe for one"
        );
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
