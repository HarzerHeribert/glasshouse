//! Phase 34D — the router request schema, the router answer, and the one
//! economy rule (Phase 34E, line 1467) that decides when the answer can be
//! reused without asking again.
//!
//! # What reaches a routing model, and what structurally cannot
//!
//! [`RouterRequest`] is the whole of what a routing model is shown about a
//! decision, and it is built from **values the caller already holds at the
//! moment it decides**: the task a person typed, which harness they named,
//! whether a warm session exists among the candidates, the capacity *band*
//! of each candidate provider, and the constraints they stated. Every field
//! is a typed, bounded value. There is no field of type "file", "transcript",
//! "environment" or "credential", and no constructor takes one — so map lines
//! 1425, 1426, 1455 and 1456 hold by the shape of the type rather than by a
//! filter that could be bypassed. The one free-text field, the task, is
//! bounded by [`TASK_TEXT_CEILING_BYTES`] and is the half
//! [`crate::memory::extract::Prompt::for_request`] scrubs before anything
//! leaves the process.
//!
//! # Bands, never numbers
//!
//! Line 1449: a provider's remaining quota reaches the model as one of five
//! words ([`crate::provider::quota::CapacityBand`]) and never as a remaining
//! count, a limit, a reset time or a spend. The router needs to know whether
//! a provider is tight; it does not need the billing figure that says so.
//!
//! # Purity
//!
//! The same rule as the rest of `routing`: no socket, no file, no clock. The
//! sticky record ([`StickyClassification`]) is a value the caller persists and
//! reloads; [`StickyClassification::reuse_for`] is a pure function of it and
//! of what the caller can see right now.

use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::pairing::{WarmSession, WarmSessionState};
use crate::integrations::IntegrationId;
use crate::provider::quota::CapacityBand;

use super::classify::{
    ClassificationSource, Complexity, Confidence, DurationClass, ExecutionShape, HardCapability,
    TaskClassification, WarmContextValue, WorkloadTier, classify_heuristically,
};
use super::session::{Continuation, Destination, RoutingMoment, TaskRequirements};

// ---------------------------------------------------------------------------
// Bounds. Named, so a test can bracket them (practice §80 case 6) and a reader
// can see that every list and every string in the request has a ceiling.
// ---------------------------------------------------------------------------

/// The most of the person's task text a router is ever shown — map line
/// 1425's *"keep routing-model prompts short"*, as a number. Two kilobytes is
/// several paragraphs; a request longer than that is a document, and a
/// classifier reads the opening of a document the way a person does.
pub const TASK_TEXT_CEILING_BYTES: usize = 2_048;

/// The hard ceiling on the whole rendered request. Everything besides the
/// task text is fixed prose plus bounded lists, so this is a property of the
/// construction; the guard in [`RouterRequest::render`] is the belt to that
/// pair of braces.
pub const REQUEST_CEILING_BYTES: usize = 6_144;

/// What a task that had to be cut says in its place — visible, so a model
/// never mistakes a truncated request for a complete one.
pub const TRUNCATION_MARKER: &str = "[… truncated to fit the router's byte ceiling]";

const MAX_PROVIDERS_NAMED: usize = 16;
const MAX_NAME_BYTES: usize = 64;
const MAX_DESTINATION_ID_BYTES: usize = 128;

/// Cut `text` to at most `ceiling` bytes on a character boundary.
fn clip(text: &str, ceiling: usize) -> &str {
    if text.len() <= ceiling {
        return text;
    }
    let mut end = ceiling;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

// ---------------------------------------------------------------------------
// The request.
// ---------------------------------------------------------------------------

/// The person's own words, bounded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskText {
    text: String,
    truncated_bytes: usize,
}

impl TaskText {
    fn bounded(raw: &str) -> Self {
        let raw = raw.trim();
        let kept = clip(raw, TASK_TEXT_CEILING_BYTES);
        Self {
            text: kept.to_owned(),
            truncated_bytes: raw.len() - kept.len(),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// How many bytes of the original did not fit. Zero for the common case.
    pub fn truncated_bytes(&self) -> usize {
        self.truncated_bytes
    }
}

/// Line 1448: whether a relevant warm session already exists, as the two
/// facts the router can use — that one exists, and how warm it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WarmSessionFact {
    state: WarmSessionState,
    idle_seconds: i64,
}

impl WarmSessionFact {
    pub fn of(warm: WarmSession) -> Self {
        Self {
            state: warm.state,
            idle_seconds: warm.idle_seconds.max(0),
        }
    }

    /// The warmest existing destination among `destinations`, which the
    /// caller orders most recently active first — so the first existing one
    /// is the relevant one.
    pub fn among(destinations: &[Destination]) -> Option<Self> {
        destinations
            .iter()
            .find_map(|destination| match destination.continuation() {
                Continuation::Existing(warm) => Some(Self::of(warm)),
                Continuation::Fresh(_) => None,
            })
    }

    pub fn state(&self) -> WarmSessionState {
        self.state
    }

    pub fn idle_seconds(&self) -> i64 {
        self.idle_seconds
    }
}

/// Line 1449: one candidate provider and the band its remaining capacity
/// falls in. `None` is "nothing has been read", which is a different fact
/// from any band and is rendered as `unknown`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderBand {
    provider: String,
    band: Option<CapacityBand>,
}

impl ProviderBand {
    pub fn new(provider: impl AsRef<str>, band: Option<CapacityBand>) -> Self {
        Self {
            provider: clip(provider.as_ref(), MAX_NAME_BYTES).to_owned(),
            band,
        }
    }

    pub fn provider(&self) -> &str {
        &self.provider
    }

    pub fn band(&self) -> Option<CapacityBand> {
        self.band
    }

    fn band_word(&self) -> &'static str {
        self.band.map(CapacityBand::as_str).unwrap_or("unknown")
    }
}

/// Line 1450: what the person stated about where the work must go.
///
/// `destination` and `fresh` are the two overrides `glasshouse launch` takes;
/// either one makes the decision deterministic (line 1470 —
/// [`Self::is_deterministic`]). `forbidden_providers` is filled from
/// providers the configuration disables, which is the one "forbidden" the
/// configuration can currently express.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UserConstraints {
    pinned_harness: Option<IntegrationId>,
    destination: Option<String>,
    fresh: bool,
    forbidden_providers: Vec<String>,
}

impl UserConstraints {
    pub fn none() -> Self {
        Self::default()
    }

    /// The harness the person named on the command line, when they named
    /// one rather than letting the single enabled harness be selected.
    pub fn with_pinned_harness(mut self, harness: Option<IntegrationId>) -> Self {
        self.pinned_harness = harness;
        self
    }

    /// `--to <id>`.
    pub fn with_destination(mut self, destination: Option<&str>) -> Self {
        self.destination = destination.map(|id| clip(id, MAX_DESTINATION_ID_BYTES).to_owned());
        self
    }

    /// `--fresh`.
    pub fn with_fresh(mut self, fresh: bool) -> Self {
        self.fresh = fresh;
        self
    }

    pub fn with_forbidden_providers(mut self, providers: Vec<String>) -> Self {
        self.forbidden_providers = providers
            .into_iter()
            .take(MAX_PROVIDERS_NAMED)
            .map(|name| clip(&name, MAX_NAME_BYTES).to_owned())
            .collect();
        self
    }

    pub fn pinned_harness(&self) -> Option<IntegrationId> {
        self.pinned_harness
    }

    pub fn destination(&self) -> Option<&str> {
        self.destination.as_deref()
    }

    pub fn fresh(&self) -> bool {
        self.fresh
    }

    pub fn forbidden_providers(&self) -> &[String] {
        &self.forbidden_providers
    }

    /// Line 1470: an explicitly named destination, or an explicit fresh
    /// start, is an obvious command. The override decides; a classifier can
    /// only add an explanation, so no routing model is asked.
    pub fn is_deterministic(&self) -> bool {
        self.destination.is_some() || self.fresh
    }
}

/// Line 1447: the small structured input a routing model is given.
///
/// Built by [`Self::new`] from the task alone and then filled by the three
/// builders with what the caller can see; see the module header for what
/// the type cannot carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouterRequest {
    task: TaskText,
    moment: RoutingMoment,
    warm_session: Option<WarmSessionFact>,
    capacity: Vec<ProviderBand>,
    constraints: UserConstraints,
    /// Line 1451, from the heuristic classifier's own signal field.
    expects_code_modification: bool,
    /// Line 1454, likewise.
    expects_long_running: bool,
}

impl RouterRequest {
    /// A request for `task` at `moment`, with no session facts attached yet.
    ///
    /// Lines 1451 and 1454 are answered here from
    /// [`classify_heuristically`]'s own signal fields, which is a pure
    /// function of the text — the request carries the tool's *expectation*,
    /// stated as such, and the model may disagree in its answer.
    pub fn new(task: &str, moment: RoutingMoment) -> Self {
        let expectation = classify_heuristically(task);
        Self {
            task: TaskText::bounded(task),
            moment,
            warm_session: None,
            capacity: Vec::new(),
            constraints: UserConstraints::none(),
            expects_code_modification: expectation.needs_code_modification(),
            expects_long_running: expectation.likely_multi_turn(),
        }
    }

    /// The request `glasshouse classify <text>` sends: the text and nothing
    /// about any session, because that command decides nothing.
    pub fn for_text(task: &str) -> Self {
        Self::new(task, RoutingMoment::SessionStart)
    }

    pub fn with_warm_session(mut self, warm_session: Option<WarmSessionFact>) -> Self {
        self.warm_session = warm_session;
        self
    }

    /// One entry per candidate provider. Bounded to the first
    /// `MAX_PROVIDERS_NAMED`; a configuration with more providers than that
    /// has more providers than a routing model can usefully weigh in one
    /// glance.
    pub fn with_capacity(mut self, capacity: Vec<ProviderBand>) -> Self {
        self.capacity = capacity.into_iter().take(MAX_PROVIDERS_NAMED).collect();
        self
    }

    pub fn with_constraints(mut self, constraints: UserConstraints) -> Self {
        self.constraints = constraints;
        self
    }

    /// The bounded task text, for the heuristic classifier and for a report.
    pub fn task_text(&self) -> &str {
        self.task.as_str()
    }

    pub fn task(&self) -> &TaskText {
        &self.task
    }

    pub fn moment(&self) -> RoutingMoment {
        self.moment
    }

    pub fn warm_session(&self) -> Option<WarmSessionFact> {
        self.warm_session
    }

    pub fn capacity(&self) -> &[ProviderBand] {
        &self.capacity
    }

    pub fn constraints(&self) -> &UserConstraints {
        &self.constraints
    }

    pub fn expects_code_modification(&self) -> bool {
        self.expects_code_modification
    }

    pub fn expects_long_running(&self) -> bool {
        self.expects_long_running
    }

    /// The text a routing model is shown, after the contract and the schema.
    ///
    /// Bounded by [`REQUEST_CEILING_BYTES`]: the task is already clipped, the
    /// lists are already capped, and the guard at the end cuts anything that
    /// still overflows — which nothing can, and a test says so.
    pub fn render(&self) -> String {
        use std::fmt::Write as _;

        let mut out = String::with_capacity(self.task.as_str().len() + 1024);
        out.push_str("task\n");
        out.push_str(self.task.as_str());
        out.push('\n');
        if self.task.truncated_bytes() > 0 {
            let _ = writeln!(out, "{TRUNCATION_MARKER}");
        }

        out.push_str("\nsession\n");
        let _ = writeln!(out, "  moment            {}", self.moment);
        match self.constraints.pinned_harness {
            Some(harness) => {
                let _ = writeln!(
                    out,
                    "  harness           {}, named by the user",
                    harness.slug()
                );
            }
            None => {
                let _ = writeln!(out, "  harness           not named; selected by the tool");
            }
        }
        match self.warm_session {
            Some(fact) => {
                let _ = writeln!(
                    out,
                    "  warm session      yes — a {} session, idle {}s",
                    fact.state, fact.idle_seconds
                );
            }
            None => {
                let _ = writeln!(out, "  warm session      none");
            }
        }
        let _ = writeln!(
            out,
            "  expects           code modification: {}; long-running multi-turn: {}",
            yes_no(self.expects_code_modification),
            yes_no(self.expects_long_running)
        );

        out.push_str("\ncapacity bands\n");
        if self.capacity.is_empty() {
            out.push_str("  no candidate provider named\n");
        }
        for entry in &self.capacity {
            let _ = writeln!(out, "  {:<18}{}", entry.provider, entry.band_word());
        }

        out.push_str("\nconstraints\n");
        match (self.constraints.destination(), self.constraints.fresh) {
            (Some(id), _) => {
                let _ = writeln!(out, "  destination       {id}, stated by the user");
            }
            (None, true) => {
                let _ = writeln!(
                    out,
                    "  destination       a fresh session, stated by the user"
                );
            }
            (None, false) => {
                let _ = writeln!(out, "  destination       none stated");
            }
        }
        if self.constraints.forbidden_providers.is_empty() {
            out.push_str("  forbidden         none\n");
        } else {
            let _ = writeln!(
                out,
                "  forbidden         {}",
                self.constraints.forbidden_providers.join(", ")
            );
        }

        if out.len() > REQUEST_CEILING_BYTES {
            let keep = REQUEST_CEILING_BYTES.saturating_sub(TRUNCATION_MARKER.len() + 1);
            let mut cut = clip(&out, keep).to_owned();
            cut.push('\n');
            cut.push_str(TRUNCATION_MARKER);
            return cut;
        }
        out
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

// ---------------------------------------------------------------------------
// The answer.
// ---------------------------------------------------------------------------

/// Line 1457's *task class*, derived from the classification's own signal
/// fields the way [`TaskClassification::hard_capabilities`] is — one place,
/// never a second field that could disagree with the signals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskClass {
    /// A question needing no repository.
    Question,
    /// Reading this repository without changing it.
    Investigation,
    /// Writing or changing code.
    CodeModification,
    /// Running something.
    ShellWork,
    /// Driving a browser.
    BrowserWork,
}

impl TaskClass {
    /// Most demanding signal first, so a task that both edits and runs is
    /// classed by the thing a harness must be wired for.
    pub fn derived_from(classification: &TaskClassification) -> Self {
        if classification.needs_browser_interaction() {
            Self::BrowserWork
        } else if classification.needs_shell_execution() {
            Self::ShellWork
        } else if classification.needs_code_modification() {
            Self::CodeModification
        } else if classification.needs_repo_context() {
            Self::Investigation
        } else {
            Self::Question
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Question => "question",
            Self::Investigation => "investigation",
            Self::CodeModification => "code modification",
            Self::ShellWork => "shell work",
            Self::BrowserWork => "browser work",
        }
    }

    /// The inverse of [`TaskClass::as_str`], for
    /// `routing_observations.task_class` (`crate::database` migration 23).
    ///
    /// `None` for anything this build does not recognise — and that is a
    /// deliberate difference from
    /// [`crate::routing::evidence::FailureClass::from_stored`], whose caller
    /// turns an unknown word into an error. See migration 23's own doc
    /// comment: a class is a bucketing input to an average, so a row this
    /// build cannot bucket is one more request of no class it counts, never
    /// a reason to fail the row.
    ///
    /// Every variant round-trips, pinned by
    /// `every_task_class_round_trips_through_its_stored_word`.
    pub fn from_stored(text: &str) -> Option<Self> {
        match text {
            "question" => Some(Self::Question),
            "investigation" => Some(Self::Investigation),
            "code modification" => Some(Self::CodeModification),
            "shell work" => Some(Self::ShellWork),
            "browser work" => Some(Self::BrowserWork),
            _ => None,
        }
    }

    /// Every variant, for a reader that must bucket by all of them and for
    /// the round-trip test. Ordered as declared.
    pub const ALL: [Self; 5] = [
        Self::Question,
        Self::Investigation,
        Self::CodeModification,
        Self::ShellWork,
        Self::BrowserWork,
    ];
}

impl fmt::Display for TaskClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why deterministic heuristics answered instead of a routing model. Every
/// sentence here is one this repository wrote — never provider text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeuristicReason {
    /// Line 1471: no routing model is configured, and everything still works.
    NoRoutingModel,
    /// Line 1470: the person stated the destination, so the override decides
    /// and no model was asked.
    DeterministicOverride,
    /// A model was configured and did not answer usably; `why` is the
    /// caller's own sentence about it.
    ModelFailed(String),
}

impl fmt::Display for HeuristicReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoRoutingModel => f.write_str("no routing model is configured"),
            Self::DeterministicOverride => {
                f.write_str("the destination was stated, so no routing model was asked")
            }
            Self::ModelFailed(why) => f.write_str(why),
        }
    }
}

/// Who produced the answer a decision is about to act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnswerProvenance {
    /// A routing model was asked, for this decision. `label` is the
    /// caller's own description of the model, never anything the reply said.
    Model { label: String },
    /// Deterministic heuristics answered, and why.
    Heuristic(HeuristicReason),
    /// Line 1467: the previous low-risk classification for the same sticky
    /// session was reused, and no routing model was asked.
    Reused { session: String, previously: String },
    /// Line 1469: a classification cached for the same normalised task text
    /// was reused, and no routing model was asked. Deliberately distinct
    /// from [`Self::Reused`] — that variant's `session` names a warm session
    /// a person is returning to, and this one has no session at all, only a
    /// hash of the text.
    ReusedFromCache { previously: String },
}

impl AnswerProvenance {
    pub fn asked_a_model(&self) -> bool {
        matches!(self, Self::Model { .. })
    }

    /// The provenance a [`ClassificationSource`] implies when the caller has
    /// no more specific reason to give.
    pub fn of_source(source: &ClassificationSource) -> Self {
        match source {
            ClassificationSource::Model { label } => Self::Model {
                label: label.clone(),
            },
            ClassificationSource::Heuristic => Self::Heuristic(HeuristicReason::NoRoutingModel),
        }
    }
}

impl fmt::Display for AnswerProvenance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Model { label } => write!(f, "the routing model ({label})"),
            Self::Heuristic(reason) => write!(f, "deterministic heuristics ({reason})"),
            Self::Reused {
                session,
                previously,
            } => write!(
                f,
                "the previous low-risk classification for session {session} ({previously}), \
                 reused without asking the routing model"
            ),
            Self::ReusedFromCache { previously } => write!(
                f,
                "the cached classification for the same task text ({previously}), reused \
                 without asking the routing model"
            ),
        }
    }
}

/// Line 1457's structured output: task class, required workload tier, hard
/// capabilities, expected duration class, confidence — and line 1458's
/// execution shape — as a router should read them, which means **after** the
/// conservative rules of line 1459 have been applied.
///
/// Wraps the [`TaskClassification`] rather than copying its fields, so the
/// raw values stay readable for a diagnostic (the pre-escalation tier, the
/// stated shape) beside the values a decision uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouterAnswer {
    classification: TaskClassification,
    provenance: AnswerProvenance,
}

impl RouterAnswer {
    pub fn new(classification: TaskClassification, provenance: AnswerProvenance) -> Self {
        Self {
            classification,
            provenance,
        }
    }

    pub fn classification(&self) -> &TaskClassification {
        &self.classification
    }

    pub fn provenance(&self) -> &AnswerProvenance {
        &self.provenance
    }

    pub fn task_class(&self) -> TaskClass {
        TaskClass::derived_from(&self.classification)
    }

    /// The tier a decision uses — [`TaskClassification::conservative_workload_tier`].
    pub fn required_tier(&self) -> WorkloadTier {
        self.classification.conservative_workload_tier()
    }

    /// The tier the producer stated, before line 1459's escalation.
    pub fn stated_tier(&self) -> WorkloadTier {
        self.classification.workload_tier()
    }

    pub fn hard_capabilities(&self) -> Vec<HardCapability> {
        self.classification.hard_capabilities()
    }

    pub fn expected_duration(&self) -> DurationClass {
        self.classification.expected_duration()
    }

    pub fn confidence(&self) -> Confidence {
        self.classification.confidence()
    }

    pub fn execution_shape(&self) -> ExecutionShape {
        self.classification.expected_execution_shape()
    }

    /// Line 1459: whether the conservative rules changed what this answer
    /// says — true exactly when confidence is [`Confidence::Low`].
    pub fn is_conservative(&self) -> bool {
        self.classification.confidence() == Confidence::Low
    }

    /// The requirements the session router is handed for this answer.
    pub fn requirements(&self) -> TaskRequirements {
        let hard_capabilities = self.hard_capabilities();
        TaskRequirements {
            needs_tool_calls: !hard_capabilities.is_empty(),
            hard_capabilities,
            minimum_tier: Some(self.required_tier()),
            classification: Some(self.clone()),
        }
    }

    /// One line a person reads: who answered, what the work is, and every
    /// value the decision used — with the conservative escalation named as
    /// such when it happened.
    pub fn explain(&self) -> String {
        let caps = self.hard_capabilities();
        let caps = if caps.is_empty() {
            "nothing a stronger text model could not supply".to_owned()
        } else {
            caps.iter()
                .copied()
                .map(HardCapability::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        };
        let tier = if self.is_conservative() {
            format!(
                "tier {} (conservative: confidence was low, escalated from {})",
                self.required_tier(),
                self.stated_tier()
            )
        } else {
            format!(
                "tier {} (confidence {})",
                self.required_tier(),
                self.confidence()
            )
        };
        format!(
            "classified by {}: {}; {}; needs {}; {}; shape {}",
            self.provenance,
            self.task_class(),
            tier,
            caps,
            self.expected_duration(),
            self.execution_shape()
        )
    }
}

// ---------------------------------------------------------------------------
// Line 1467 / 1468 — when the previous answer may stand.
// ---------------------------------------------------------------------------

/// How long after its last activity a session still counts as the one the
/// person is "in" for line 1467's purposes. A resumable session idle for
/// longer than this is a session being *returned to*, and returning to work
/// is line 1468's "starts a new task".
pub const STICKY_TURN_WINDOW_SECONDS: i64 = 30 * 60;

/// Bumped when `StoredClassification`'s shape changes; a record with
/// another version reads as absent.
pub const STICKY_CLASSIFICATION_FORMAT_VERSION: u32 = 1;

/// Line 1468's "resource conditions": the facts a classifier's answer was
/// conditioned on, in a shape two decisions can compare for equality.
///
/// Bands, not readings — the same coarseness the request itself carries, so
/// a quota that ticked down by one request does not count as a material
/// change, and a band boundary crossed does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingFingerprint {
    harness: String,
    capacity: Vec<(String, String)>,
    health_observed: Vec<String>,
}

impl RoutingFingerprint {
    /// `harness` is `None` for a decision that ranks across every harness
    /// (`glasshouse route`); a launch always names one, and a record made
    /// for one harness never stands for another.
    pub fn new(
        harness: Option<IntegrationId>,
        capacity: &[ProviderBand],
        health_observed: impl IntoIterator<Item = String>,
    ) -> Self {
        let mut capacity: Vec<(String, String)> = capacity
            .iter()
            .map(|entry| (entry.provider.clone(), entry.band_word().to_owned()))
            .collect();
        capacity.sort();
        capacity.dedup();
        let mut health_observed: Vec<String> = health_observed.into_iter().collect();
        health_observed.sort();
        health_observed.dedup();
        Self {
            harness: harness.map(IntegrationId::slug).unwrap_or("any").to_owned(),
            capacity,
            health_observed,
        }
    }
}

/// A [`TaskClassification`] in a shape `serde` can carry: every enum as the
/// word its `as_str` prints, and the two recommendations as the words the
/// reply schema uses. Private on purpose — the only readers are
/// [`StickyClassification::new`] and its inverse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StoredClassification {
    needs_repo_context: bool,
    needs_code_modification: bool,
    needs_shell_execution: bool,
    needs_browser_interaction: bool,
    complexity: String,
    likely_multi_turn: bool,
    workload_tier: String,
    safe_for_disposable_model: bool,
    warm_context: String,
    confidence: String,
    /// `Some` for a model answer (its label), `None` for the heuristic.
    model_label: Option<String>,
    duration: Option<String>,
    execution_shape: Option<String>,
}

impl StoredClassification {
    fn of(classification: &TaskClassification) -> Self {
        Self {
            needs_repo_context: classification.needs_repo_context(),
            needs_code_modification: classification.needs_code_modification(),
            needs_shell_execution: classification.needs_shell_execution(),
            needs_browser_interaction: classification.needs_browser_interaction(),
            complexity: classification.complexity().as_str().to_owned(),
            likely_multi_turn: classification.likely_multi_turn(),
            workload_tier: classification.workload_tier().as_str().to_owned(),
            safe_for_disposable_model: classification.safe_for_disposable_model(),
            warm_context: classification.warm_context().as_str().to_owned(),
            confidence: classification.confidence().as_str().to_owned(),
            model_label: match classification.source() {
                ClassificationSource::Model { label } => Some(label.clone()),
                ClassificationSource::Heuristic => None,
            },
            duration: classification
                .stated_duration()
                .map(|d| d.as_str().to_owned()),
            execution_shape: classification
                .stated_execution_shape()
                .map(|s| s.as_str().to_owned()),
        }
    }

    /// `None` when any word is one this build does not print — a record from
    /// a build with a different vocabulary reads as absent, never as a
    /// guessed classification.
    fn classification(&self) -> Option<TaskClassification> {
        const COMPLEXITIES: [Complexity; 3] = [
            Complexity::Trivial,
            Complexity::Moderate,
            Complexity::Complex,
        ];
        const TIERS: [WorkloadTier; 5] = [
            WorkloadTier::Deterministic,
            WorkloadTier::Leaf,
            WorkloadTier::Standard,
            WorkloadTier::Heavy,
            WorkloadTier::Frontier,
        ];
        const WARMTHS: [WarmContextValue; 2] = [
            WarmContextValue::PreferWarm,
            WarmContextValue::PreferStrongerCold,
        ];
        const CONFIDENCES: [Confidence; 3] =
            [Confidence::Low, Confidence::Medium, Confidence::High];
        const DURATIONS: [DurationClass; 3] = [
            DurationClass::SingleTurn,
            DurationClass::FewTurns,
            DurationClass::LongRunning,
        ];
        const SHAPES: [ExecutionShape; 3] = [
            ExecutionShape::ReuseSession,
            ExecutionShape::NewSession,
            ExecutionShape::DisposableJob,
        ];
        fn word<T: Copy>(all: &[T], name: &str, as_str: fn(T) -> &'static str) -> Option<T> {
            all.iter().copied().find(|v| as_str(*v) == name)
        }

        let complexity = word(&COMPLEXITIES, &self.complexity, Complexity::as_str)?;
        let workload_tier = word(&TIERS, &self.workload_tier, WorkloadTier::as_str)?;
        let warm_context = word(&WARMTHS, &self.warm_context, WarmContextValue::as_str)?;
        let confidence = word(&CONFIDENCES, &self.confidence, Confidence::as_str)?;
        let duration = match &self.duration {
            Some(name) => Some(word(&DURATIONS, name, DurationClass::as_str)?),
            None => None,
        };
        let execution_shape = match &self.execution_shape {
            Some(name) => Some(word(&SHAPES, name, ExecutionShape::as_str)?),
            None => None,
        };
        let source = match &self.model_label {
            Some(label) => ClassificationSource::Model {
                label: label.clone(),
            },
            None => ClassificationSource::Heuristic,
        };
        Some(
            TaskClassification::new(
                self.needs_repo_context,
                self.needs_code_modification,
                self.needs_shell_execution,
                self.needs_browser_interaction,
                complexity,
                self.likely_multi_turn,
                workload_tier,
                self.safe_for_disposable_model,
                warm_context,
                confidence,
                source,
            )
            .with_duration(duration)
            .with_execution_shape(execution_shape),
        )
    }
}

/// Why the previous answer may not stand — one reason per clause of line
/// 1468, so the explanation can say which one fired.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StickyRefusal {
    /// The previous work was not low-risk; every turn of it is re-classified.
    NotLowRisk,
    /// Line 1468's "the current session becomes unsuitable": the sticky
    /// session is no longer among the destinations offered.
    SessionGone,
    /// The sticky session is offered but has been idle past
    /// [`STICKY_TURN_WINDOW_SECONDS`] — a return to work, not a turn.
    SessionIdle { idle_seconds: i64 },
    /// Line 1468's "resource conditions materially change".
    ConditionsChanged,
    /// The stored record could not be read back as a classification.
    Unreadable,
}

impl fmt::Display for StickyRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotLowRisk => f.write_str("the previous classification was not low-risk"),
            Self::SessionGone => {
                f.write_str("the session it was made for is no longer among the destinations")
            }
            Self::SessionIdle { idle_seconds } => write!(
                f,
                "the session it was made for has been idle {idle_seconds}s, past the \
                 {STICKY_TURN_WINDOW_SECONDS}s window"
            ),
            Self::ConditionsChanged => {
                f.write_str("capacity bands or observed health changed since it was made")
            }
            Self::Unreadable => f.write_str("the stored record could not be read"),
        }
    }
}

/// What one decision leaves behind for the next — line 1467's memory.
///
/// A value the caller persists under the project's own state directory and
/// reloads next time; this module never touches the file. Every read
/// failure at the caller — absent, unreadable, wrong version — is the same
/// as no record, and [`Self::reuse_for`] is the only reader.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StickyClassification {
    version: u32,
    session: String,
    fingerprint: RoutingFingerprint,
    classification: StoredClassification,
    recorded_at_unix: i64,
}

impl StickyClassification {
    pub fn new(
        session: impl Into<String>,
        fingerprint: RoutingFingerprint,
        classification: &TaskClassification,
        recorded_at_unix: i64,
    ) -> Self {
        Self {
            version: STICKY_CLASSIFICATION_FORMAT_VERSION,
            session: session.into(),
            fingerprint,
            classification: StoredClassification::of(classification),
            recorded_at_unix,
        }
    }

    /// The session this classification was made for.
    pub fn session(&self) -> &str {
        &self.session
    }

    pub fn recorded_at_unix(&self) -> i64 {
        self.recorded_at_unix
    }

    /// The classification this record stores, when this build can read it —
    /// Phase 36 line 1582's producer, read by `main.rs::routing_destinations`
    /// for the session [`Self::session`] names and attached to that
    /// destination as its last classified task. `None` on a record from a
    /// build with a different vocabulary, exactly as [`Self::reuse_for`]
    /// refuses one.
    pub fn classification(&self) -> Option<TaskClassification> {
        self.classification.classification()
    }

    /// The bytes a caller writes. Pretty, because a person may read the file.
    pub fn to_json(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec_pretty(self)
    }

    /// The inverse of [`Self::to_json`]. `None` for anything this build did
    /// not write — a different version included — rather than an error.
    pub fn from_json(bytes: &[u8]) -> Option<Self> {
        let stored: Self = serde_json::from_slice(bytes).ok()?;
        (stored.version == STICKY_CLASSIFICATION_FORMAT_VERSION).then_some(stored)
    }

    /// Line 1467 in one function: the previous answer stands for this
    /// decision iff it was low-risk, nothing the classifier was conditioned
    /// on has changed (line 1468), and the session it was made for is still
    /// offered and still warm enough to be the one the person is in.
    ///
    /// On success the classification is returned as it was stored — its
    /// source still names the model that originally answered — and the
    /// caller records the provenance as [`AnswerProvenance::Reused`].
    pub fn reuse_for(
        &self,
        fingerprint: &RoutingFingerprint,
        destinations: &[Destination],
    ) -> Result<TaskClassification, StickyRefusal> {
        let classification = self
            .classification
            .classification()
            .ok_or(StickyRefusal::Unreadable)?;
        if !classification.is_low_risk() {
            return Err(StickyRefusal::NotLowRisk);
        }
        if &self.fingerprint != fingerprint {
            return Err(StickyRefusal::ConditionsChanged);
        }
        let warm = destinations
            .iter()
            .find(|destination| destination.id() == self.session)
            .and_then(|destination| match destination.continuation() {
                Continuation::Existing(warm) => Some(warm),
                Continuation::Fresh(_) => None,
            })
            .ok_or(StickyRefusal::SessionGone)?;
        if warm.idle_seconds > STICKY_TURN_WINDOW_SECONDS {
            return Err(StickyRefusal::SessionIdle {
                idle_seconds: warm.idle_seconds,
            });
        }
        Ok(classification)
    }
}

// ---------------------------------------------------------------------------
// Line 1469 — a text-keyed cache beside the session-keyed one above: the same
// low-confidence gate, a fingerprint, and a routing-model identity, but keyed
// by the normalised task text itself rather than by which session is warm.
// ---------------------------------------------------------------------------

/// How long a text-keyed cache entry may still answer for its key before
/// this build asks again — map line 1469's "recent". Named beside
/// [`STICKY_TURN_WINDOW_SECONDS`] rather than sharing it: that window
/// measures session warmth, a signal this cache does not have, so it earns
/// its own name instead of borrowing a number reasoned about a different
/// question. Set to the same span anyway — nothing in current practice
/// suggests a classification goes stale on a faster clock than a session
/// does.
pub const CLASSIFICATION_CACHE_WINDOW_SECONDS: i64 = STICKY_TURN_WINDOW_SECONDS;

/// Map line 1469's "semantically identical, honestly": no embeddings exist
/// in this build (Phase 52 is Cluster Q for exactly that reason), so
/// identity is a normalised literal text match — trim, collapse every run of
/// internal whitespace to one space, lowercase — hashed with this crate's
/// existing choice ([`Sha256`], the same digest `crate::firewall::store` and
/// `crate::project` already key their own content by), so a cache keyed by
/// this value never carries the task text itself.
pub fn normalised_task_key(text: &str) -> String {
    let mut normalised = String::with_capacity(text.len());
    let mut last_was_space = false;
    for ch in text.trim().chars() {
        if ch.is_whitespace() {
            if !last_was_space {
                normalised.push(' ');
                last_was_space = true;
            }
        } else {
            normalised.extend(ch.to_lowercase());
            last_was_space = false;
        }
    }
    let digest = Sha256::digest(normalised.as_bytes());
    hex::encode(&digest[..16])
}

/// One remembered answer for a normalised task text — map line 1469's
/// memory, beside [`StickyClassification`]'s session-keyed one and in the
/// same shape: a fingerprint, a routing-model identity, the classification
/// itself in the shape `StoredClassification` already carries, and when it
/// was recorded. The caller persists and reloads this; this module never
/// touches a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedClassification {
    key: String,
    fingerprint: RoutingFingerprint,
    resolution: String,
    stored: StoredClassification,
    recorded_at_unix: i64,
}

impl CachedClassification {
    pub fn new(
        key: impl Into<String>,
        fingerprint: RoutingFingerprint,
        resolution: impl Into<String>,
        classification: &TaskClassification,
        recorded_at_unix: i64,
    ) -> Self {
        Self {
            key: key.into(),
            fingerprint,
            resolution: resolution.into(),
            stored: StoredClassification::of(classification),
            recorded_at_unix,
        }
    }

    /// The key this record answers for — [`normalised_task_key`]'s output,
    /// never the task text.
    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn recorded_at_unix(&self) -> i64 {
        self.recorded_at_unix
    }

    /// The classification this record stores, when this build can read it —
    /// `None` for a record written by a build with a different vocabulary,
    /// exactly as `StoredClassification::classification` refuses one.
    pub fn classification(&self) -> Option<TaskClassification> {
        self.stored.classification()
    }

    /// Map line 1469's "when safe", in one function: never below
    /// [`Confidence::Low`] — the same rule
    /// [`super::classify::TaskClassification::is_low_risk`] states this
    /// reasoning for, reused here rather than restated a second time — the
    /// same fingerprint, the same routing-model identity, and recorded
    /// within [`CLASSIFICATION_CACHE_WINDOW_SECONDS`] of `now_unix`.
    pub fn is_reusable_for(
        &self,
        now_unix: i64,
        fingerprint: &RoutingFingerprint,
        resolution: &str,
    ) -> bool {
        let Some(classification) = self.stored.classification() else {
            return false;
        };
        if classification.confidence() == Confidence::Low {
            return false;
        }
        if &self.fingerprint != fingerprint {
            return false;
        }
        if self.resolution != resolution {
            return false;
        }
        let age = now_unix.saturating_sub(self.recorded_at_unix);
        (0..=CLASSIFICATION_CACHE_WINDOW_SECONDS).contains(&age)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::session::Destination;
    use crate::routing::{AssignedModel, Backend, Cost, CredentialId, ToolSemantics};
    use crate::secret::SecretRef;

    fn backend() -> Backend {
        Backend::new(
            "route-probe",
            "openai-chat",
            AssignedModel::HarnessDefault,
            CredentialId::new(
                "route-probe",
                SecretRef::Environment {
                    var: "GLASSHOUSE_REQUEST_TEST_KEY".to_owned(),
                },
            ),
            Cost::Metered,
            ToolSemantics::Unverified,
        )
    }

    fn existing(id: &str, idle_seconds: i64) -> Destination {
        Destination::existing(
            id,
            IntegrationId::ClaudeCode,
            "native",
            backend(),
            WarmSession {
                state: WarmSessionState::Resumable,
                idle_seconds,
            },
        )
    }

    fn low_risk() -> TaskClassification {
        classify_heuristically("what is a mutex?")
    }

    fn fingerprint() -> RoutingFingerprint {
        RoutingFingerprint::new(
            Some(IntegrationId::ClaudeCode),
            &[ProviderBand::new(
                "route-probe",
                Some(CapacityBand::Healthy),
            )],
            Vec::<String>::new(),
        )
    }

    // --- 1425 / 1447: the rendered request is bounded ----------------------

    #[test]
    fn a_task_longer_than_the_ceiling_is_truncated_with_a_visible_marker() {
        let long = "fn handle(req: &Request) -> Response { todo!() }\n".repeat(5_000);
        assert!(long.len() > 100 * TASK_TEXT_CEILING_BYTES);
        let request = RouterRequest::for_text(&long);
        let rendered = request.render();
        assert!(
            rendered.len() <= REQUEST_CEILING_BYTES,
            "rendered {} bytes, over the {REQUEST_CEILING_BYTES}-byte ceiling",
            rendered.len()
        );
        assert!(rendered.contains(TRUNCATION_MARKER), "{rendered}");
        assert_eq!(
            request.task().truncated_bytes(),
            long.trim().len() - request.task_text().len()
        );
    }

    /// Bracketing, not derivation (practice §80 case 6): a task of exactly
    /// the ceiling is kept whole, and one byte more is not.
    #[test]
    fn the_task_ceiling_is_exactly_where_the_constant_says() {
        let exact = "x".repeat(TASK_TEXT_CEILING_BYTES);
        assert_eq!(RouterRequest::for_text(&exact).task().truncated_bytes(), 0);
        let over = "x".repeat(TASK_TEXT_CEILING_BYTES + 1);
        assert_eq!(RouterRequest::for_text(&over).task().truncated_bytes(), 1);
    }

    /// A literal, not the constant: a four-kilobyte task does not reach a
    /// model whole. A mutation that raises [`TASK_TEXT_CEILING_BYTES`] past
    /// this cannot rescale the test along with it.
    #[test]
    fn a_four_kilobyte_task_does_not_reach_the_model_whole() {
        let request = RouterRequest::for_text(&"z".repeat(4_096));
        assert!(request.task().truncated_bytes() > 0);
        assert!(request.task_text().len() < 4_096);
        assert!(request.render().contains(TRUNCATION_MARKER));
    }

    #[test]
    fn a_maximal_request_still_fits_the_ceiling_without_the_guard() {
        let request = RouterRequest::for_text(&"y".repeat(TASK_TEXT_CEILING_BYTES))
            .with_warm_session(Some(WarmSessionFact::of(WarmSession {
                state: WarmSessionState::Live,
                idle_seconds: i64::MAX,
            })))
            .with_capacity(
                (0..MAX_PROVIDERS_NAMED * 2)
                    .map(|i| {
                        ProviderBand::new(
                            format!("{i}-{}", "p".repeat(MAX_NAME_BYTES * 2)),
                            Some(CapacityBand::Exhausted),
                        )
                    })
                    .collect(),
            )
            .with_constraints(
                UserConstraints::none()
                    .with_pinned_harness(Some(IntegrationId::ClaudeCode))
                    .with_destination(Some(&"d".repeat(MAX_DESTINATION_ID_BYTES * 2)))
                    .with_forbidden_providers(
                        (0..MAX_PROVIDERS_NAMED * 2)
                            .map(|i| format!("{i}-{}", "f".repeat(MAX_NAME_BYTES * 2)))
                            .collect(),
                    ),
            );
        let rendered = request.render();
        assert!(
            !rendered.ends_with(TRUNCATION_MARKER),
            "the lists and strings are capped so the final guard never has to fire: {} bytes",
            rendered.len()
        );
        assert!(rendered.len() <= REQUEST_CEILING_BYTES);
        assert_eq!(request.capacity().len(), MAX_PROVIDERS_NAMED);
    }

    // --- 1448 / 1449 / 1450 / 1451 / 1454: what the request says -----------

    #[test]
    fn the_request_names_bands_the_warm_session_and_the_expectations() {
        let request =
            RouterRequest::new("fix the flaky test in auth.rs", RoutingMoment::SessionStart)
                .with_warm_session(Some(WarmSessionFact::of(WarmSession {
                    state: WarmSessionState::Resumable,
                    idle_seconds: 42,
                })))
                .with_capacity(vec![
                    ProviderBand::new("alpha", Some(CapacityBand::Tight)),
                    ProviderBand::new("beta", None),
                ])
                .with_constraints(
                    UserConstraints::none()
                        .with_pinned_harness(Some(IntegrationId::ClaudeCode))
                        .with_forbidden_providers(vec!["gamma".to_owned()]),
                );
        let rendered = request.render();
        assert!(rendered.contains("warm session      yes — a resumable session, idle 42s"));
        assert!(rendered.contains("alpha             tight"), "{rendered}");
        assert!(rendered.contains("beta              unknown"), "{rendered}");
        assert!(rendered.contains("harness           claude-code, named by the user"));
        assert!(rendered.contains("forbidden         gamma"));
        assert!(
            rendered.contains("code modification: yes; long-running multi-turn: yes"),
            "{rendered}"
        );
        assert!(request.expects_code_modification());
        assert!(request.expects_long_running());
    }

    #[test]
    fn a_question_expects_neither_modification_nor_a_long_session() {
        let request = RouterRequest::for_text("what is a mutex?");
        assert!(!request.expects_code_modification());
        assert!(!request.expects_long_running());
        assert!(request.render().contains("warm session      none"));
    }

    // --- 1470: an explicit destination is deterministic --------------------

    #[test]
    fn a_stated_destination_or_a_fresh_start_is_deterministic() {
        assert!(!UserConstraints::none().is_deterministic());
        assert!(UserConstraints::none().with_fresh(true).is_deterministic());
        assert!(
            UserConstraints::none()
                .with_destination(Some("abc"))
                .is_deterministic()
        );
    }

    // --- 1457 / 1458 / 1459: the answer -----------------------------------

    #[test]
    fn a_low_confidence_answer_reads_conservatively_and_says_so() {
        let low = classify_heuristically("thing");
        assert_eq!(low.confidence(), Confidence::Low);
        let answer = RouterAnswer::new(
            low,
            AnswerProvenance::Model {
                label: "alpha/alpha-model".to_owned(),
            },
        );
        assert!(answer.is_conservative());
        assert_eq!(answer.required_tier(), WorkloadTier::Standard);
        assert_eq!(answer.stated_tier(), WorkloadTier::Leaf);
        assert_eq!(answer.expected_duration(), DurationClass::LongRunning);
        assert_ne!(answer.execution_shape(), ExecutionShape::DisposableJob);
        let line = answer.explain();
        assert!(line.contains("conservative: confidence was low"), "{line}");
        assert!(line.contains("escalated from leaf"), "{line}");
        assert_eq!(
            answer.requirements().minimum_tier,
            Some(WorkloadTier::Standard),
            "the requirements carry the conservative tier, not the stated one"
        );
    }

    #[test]
    fn task_class_follows_the_most_demanding_signal() {
        assert_eq!(
            RouterAnswer::new(
                classify_heuristically("run cargo test and fix whatever fails"),
                AnswerProvenance::Heuristic(HeuristicReason::NoRoutingModel)
            )
            .task_class(),
            TaskClass::ShellWork
        );
        assert_eq!(
            RouterAnswer::new(
                classify_heuristically("what is a mutex?"),
                AnswerProvenance::Heuristic(HeuristicReason::NoRoutingModel)
            )
            .task_class(),
            TaskClass::Question
        );
    }

    // --- 1467 / 1468: the sticky bypass -----------------------------------

    #[test]
    fn a_low_risk_classification_for_a_warm_offered_session_is_reused() {
        let sticky = StickyClassification::new("s1", fingerprint(), &low_risk(), 1_000);
        let reused = sticky
            .reuse_for(&fingerprint(), &[existing("s1", 5)])
            .expect("a low-risk answer for the same warm session stands");
        assert_eq!(reused, low_risk());
    }

    #[test]
    fn a_classification_that_is_not_low_risk_is_never_reused() {
        let risky = classify_heuristically("run cargo test and fix whatever fails");
        assert!(!risky.is_low_risk());
        let sticky = StickyClassification::new("s1", fingerprint(), &risky, 1_000);
        assert_eq!(
            sticky.reuse_for(&fingerprint(), &[existing("s1", 5)]),
            Err(StickyRefusal::NotLowRisk)
        );
    }

    #[test]
    fn changed_capacity_bands_force_a_fresh_classification() {
        let sticky = StickyClassification::new("s1", fingerprint(), &low_risk(), 1_000);
        let changed = RoutingFingerprint::new(
            Some(IntegrationId::ClaudeCode),
            &[ProviderBand::new(
                "route-probe",
                Some(CapacityBand::Reserve),
            )],
            Vec::<String>::new(),
        );
        assert_eq!(
            sticky.reuse_for(&changed, &[existing("s1", 5)]),
            Err(StickyRefusal::ConditionsChanged)
        );
    }

    #[test]
    fn a_session_no_longer_offered_or_idle_past_the_window_is_not_sticky() {
        let sticky = StickyClassification::new("s1", fingerprint(), &low_risk(), 1_000);
        assert_eq!(
            sticky.reuse_for(&fingerprint(), &[existing("other", 5)]),
            Err(StickyRefusal::SessionGone)
        );
        assert_eq!(
            sticky.reuse_for(
                &fingerprint(),
                &[existing("s1", STICKY_TURN_WINDOW_SECONDS + 1)]
            ),
            Err(StickyRefusal::SessionIdle {
                idle_seconds: STICKY_TURN_WINDOW_SECONDS + 1
            })
        );
        assert!(
            sticky
                .reuse_for(
                    &fingerprint(),
                    &[existing("s1", STICKY_TURN_WINDOW_SECONDS)]
                )
                .is_ok(),
            "exactly at the window is still a turn"
        );
    }

    #[test]
    fn a_sticky_record_round_trips_through_json_and_refuses_another_version() {
        let sticky = StickyClassification::new(
            "s1",
            fingerprint(),
            &low_risk().with_duration(Some(DurationClass::FewTurns)),
            1_000,
        );
        let bytes = sticky.to_json().unwrap();
        assert_eq!(
            StickyClassification::from_json(&bytes),
            Some(sticky.clone())
        );
        let other_version = String::from_utf8(bytes)
            .unwrap()
            .replace("\"version\": 1", "\"version\": 99");
        assert_eq!(
            StickyClassification::from_json(other_version.as_bytes()),
            None
        );
        assert_eq!(StickyClassification::from_json(b"not json"), None);
    }

    // --- 1469: the text-keyed cache ----------------------------------------

    #[test]
    fn normalisation_collapses_whitespace_and_case_to_the_same_key() {
        let a = normalised_task_key("Fix   the Bug");
        let b = normalised_task_key("  fix the bug  ");
        let c = normalised_task_key("fix\tthe\nbug");
        assert_eq!(a, b);
        assert_eq!(a, c);
        assert_ne!(a, normalised_task_key("fix the bugs"));
    }

    #[test]
    fn normalisation_never_stores_the_task_text() {
        let key = normalised_task_key("a very specific secret task string");
        assert!(!key.contains("secret"));
        assert!(key.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn a_reusable_entry_passes_all_four_gates() {
        let cached = CachedClassification::new(
            normalised_task_key("fix the bug"),
            fingerprint(),
            "pinned:route-probe/router-model",
            &low_risk(),
            1_000,
        );
        assert!(cached.is_reusable_for(1_500, &fingerprint(), "pinned:route-probe/router-model"));
    }

    #[test]
    fn a_low_confidence_entry_is_never_reusable() {
        let low_confidence = classify_heuristically("thing");
        assert_eq!(low_confidence.confidence(), Confidence::Low);
        let cached = CachedClassification::new(
            normalised_task_key("thing"),
            fingerprint(),
            "pinned:route-probe/router-model",
            &low_confidence,
            1_000,
        );
        assert!(!cached.is_reusable_for(1_500, &fingerprint(), "pinned:route-probe/router-model"));
    }

    #[test]
    fn a_different_fingerprint_is_never_reusable() {
        let cached = CachedClassification::new(
            normalised_task_key("fix the bug"),
            fingerprint(),
            "pinned:route-probe/router-model",
            &low_risk(),
            1_000,
        );
        let changed = RoutingFingerprint::new(
            Some(IntegrationId::ClaudeCode),
            &[ProviderBand::new(
                "route-probe",
                Some(CapacityBand::Exhausted),
            )],
            Vec::<String>::new(),
        );
        assert!(!cached.is_reusable_for(1_500, &changed, "pinned:route-probe/router-model"));
    }

    #[test]
    fn a_different_resolution_tag_is_never_reusable() {
        let cached = CachedClassification::new(
            normalised_task_key("fix the bug"),
            fingerprint(),
            "pinned:route-probe/router-model",
            &low_risk(),
            1_000,
        );
        assert!(!cached.is_reusable_for(1_500, &fingerprint(), "pinned:route-probe/other-model"));
    }

    #[test]
    fn an_entry_older_than_the_window_is_never_reusable() {
        let cached = CachedClassification::new(
            normalised_task_key("fix the bug"),
            fingerprint(),
            "pinned:route-probe/router-model",
            &low_risk(),
            1_000,
        );
        assert!(cached.is_reusable_for(
            1_000 + CLASSIFICATION_CACHE_WINDOW_SECONDS,
            &fingerprint(),
            "pinned:route-probe/router-model"
        ));
        assert!(!cached.is_reusable_for(
            1_000 + CLASSIFICATION_CACHE_WINDOW_SECONDS + 1,
            &fingerprint(),
            "pinned:route-probe/router-model"
        ));
    }
}
