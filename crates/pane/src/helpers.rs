//! Little helpers -- `docs/product/pane/little-helpers.md`.
//!
//! A helper is a cheap model given a narrow toolset to answer one question on
//! demand: a secretary, not a delegate. It returns a value and it is gone.
//!
//! **The roster is data.** Adding a helper is a [`HelperSpec`] literal appended
//! to [`HELPERS`]; the guardrails are enforced here rather than by each spec,
//! so a new helper cannot introduce a new failure mode -- it can only choose
//! within a boundary [`validate`] already checks at startup.
//!
//! A helper is not a subagent. A subagent owns a goal and its effects persist;
//! a helper owns a question and leaves nothing behind. That is why
//! [`FORBIDDEN_TOOLS`] exists as a list a spec cannot hold rather than as a
//! sentence a model might ignore.

use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use crate::contract::{Conversation, Message, Role};
use crate::tools::registry;
use crate::wire;

/// Tools no helper may hold, whatever its spec says.
///
/// A helper answers questions; it does not change the world. This is the
/// mutating third of `registry::ALL`, and [`validate`] refuses a spec naming
/// any of them -- so "a helper never writes" is a list it does not have rather
/// than a rule it might disregard.
pub const FORBIDDEN_TOOLS: [&str; 3] = ["write", "edit", "bash"];

/// The most turns any helper may take inside one call, whatever its spec says.
pub const HELPER_MAX_TURNS: u32 = 8;

/// Lets the ledger tell a helper's request from a task turn before the gateway
/// reads the body -- the same seam `supervisor.rs` uses for its look.
pub const PURPOSE_HEADER: (&str, &str) = ("x-glasshouse-purpose", "helper");

/// What a helper accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    /// The user's request, for a helper that orients a task.
    Request,
    /// A blob the caller already holds.
    Text,
    /// A handle the caller already holds.
    Handle,
    /// A diff of the turn.
    Diff,
    /// Caller-supplied evidence about a claim, which may include a diff,
    /// current source, a contract, or verification observations.
    Evidence,
}

/// What a helper returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputKind {
    /// `file:line` spans -- cheap for the caller to verify.
    Spans,
    /// Shorter text, with the full text still in the caller's hands.
    Reduction,
    /// A judgement plus the evidence for it.
    Verdict,
}

/// Where a helper may be invoked from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallSite {
    /// The preflight hook, before the task model's first turn.
    Preflight,
    /// After a tool result.
    PostResult,
    /// Before a completion is accepted.
    CompletionGate,
    /// From inside a cell, by the model itself.
    Cell,
}

/// One helper, entirely as data.
#[derive(Debug, Clone, Copy)]
pub struct HelperSpec {
    /// Called as `helper.<name>(…)`; also the key in the generated declarations.
    pub name: &'static str,
    /// One line, rendered into the declarations the caller sees.
    pub summary: &'static str,
    /// Present participle for the TUI lane: "scanning", "reducing", "checking".
    pub verb: &'static str,
    /// Its fixed instruction preamble -- the only prose a helper carries, and
    /// where the two contracts no toolset can express are stated.
    pub preamble: &'static str,
    /// Tool names, a subset of `registry::ALL`. Empty is legal and is the
    /// safest possible helper.
    pub tools: &'static [&'static str],
    /// Small on purpose: a helper answers, it does not compose.
    pub max_tokens: u32,
    /// Turns inside one call. `1` is one-shot and needs no agent loop.
    pub max_turns: u32,
    pub input: InputKind,
    pub output: OutputKind,
    pub call_sites: &'static [CallSite],
}

/// Find where something lives in this project, as spans the caller can open.
///
/// It holds the four reading tools and nothing else, so the worst it can do
/// is look in the wrong place -- and `max_turns` is what stops it looking
/// forever.
pub const SCOUT: HelperSpec = HelperSpec {
    name: "find",
    summary: "Find where something lives in this project and answer with file:line spans, never a diagnosis.",
    verb: "scanning",
    preamble: "Never state a number you did not compute. You have a runtime: count and total in a \
        cell and report what it returned. A computed number is evidence; an estimated one \
        is a conclusion, and conclusions are not yours to draw.\n\
        \n\
        You find where something lives in this project. You can read, glob, grep and \
        fetch context, and you can change nothing.\n\
        \n\
        Answer with spans only. For each: the path and the line as `path/to/file.rs:120`, a line \
        you actually opened rather than one you inferred, then one short sentence saying what is \
        there in the file's own words. Put the spans that answer the question first. If nothing \
        matches, say that in one line.\n\
        \n\
        Never state a cause. Never propose a fix. Never report anything you did not read \
        — you are returning evidence, and a wrong diagnosis the caller trusts is worse than no \
        diagnosis. Prose is never a substitute for a span: a caller who asked where something is \
        cannot open a paragraph about it. Name what you did not look at — the patterns you did \
        not run, the directories you skipped, and whether you ran out of turns — as your last \
        line.",
    // `rg` and `fd` rather than `grep` and `glob`: same purity, sharper search,
    // and a Scout's whole job is finding things. `grep` stays for the plain
    // regex case the model may already know.
    tools: &["read", "rg", "fd", "grep", "context"],
    max_tokens: 2048,
    max_turns: 8,
    input: InputKind::Request,
    output: OutputKind::Spans,
    call_sites: &[CallSite::Preflight, CallSite::Cell],
};

/// Reduce build output, logs and test results to their distinct failures.
///
/// `tools` is empty, so this helper cannot reach the filesystem at all -- the
/// caller passes the text in and still holds it, which is why a `Reduction`
/// needs no handle of its own.
pub const REDUCER: HelperSpec = HelperSpec {
    name: "reduce",
    summary: "Reduce a log or command output to its distinct failures, with file:line where the text names one.",
    verb: "reducing",
    preamble: "You reduce build output, logs and test results to their distinct failures.\n\
        \n\
        A warning is not a failure: report only entries that report an error.\n\
        \n\
        Answer with the failures only. For each: **the name of what failed** as the text spells it \
        — the test, target, crate or symbol — then the file and line if the text names one, \
        then the error exactly as it appeared, and how many times it repeated. Order them as they \
        first appear. If there are no failures, say so in one line.\n\
        \n\
        Never state a cause. Never propose a fix. Never report anything the text does not say \
        — you are returning evidence, and a wrong diagnosis the caller trusts is worse than no \
        diagnosis. A file and line is never a substitute for a name: a caller who asked which \
        test failed cannot use a line number. If the input was truncated or you could not read part of it, say so as your \
        last line.",
    tools: &[],
    max_tokens: 1024,
    max_turns: 1,
    input: InputKind::Text,
    output: OutputKind::Reduction,
    call_sites: &[CallSite::PostResult, CallSite::Cell],
};

/// Check a claim against supplied evidence and readable files, and return the
/// evidence with the verdict.
///
/// `read` and `grep` only: it decides whether something holds, and a helper
/// that could also change it would be deciding about its own work.
pub const CHECKER: HelperSpec = HelperSpec {
    name: "check",
    summary: "Check a claim against supplied evidence and current files, and answer with a verdict plus the evidence for it.",
    verb: "checking",
    preamble: "Never state a number you did not compute. You have a runtime: count and total in a \
        cell and report what it returned. A computed number is evidence; an estimated one \
        is a conclusion, and conclusions are not yours to draw.\n\
        \n\
        You check whether a claim holds against the supplied evidence and current readable files. \
        The evidence may contain a diff, current source excerpts, a contract, or verification \
        observations. You can read and grep, and you can change nothing.\n\
        \n\
        Open with the verdict alone on the first line: `holds`, `does not hold`, or `cannot \
        tell`. Current source and its contract can establish a current-state claim without a \
        unified diff. An absent diff or baseline prevents only change-history claims such as \
        whether old tests were preserved. For a composite claim, use `cannot tell` if a material \
        part lacks evidence, but still identify which parts the evidence supports or refutes. \
        Under the verdict, give the evidence and nothing else. Cite source as \
        `path/to/file.rs:120` with the deciding text. Cite a verification observation by its check \
        name, exit code, `executed`, `reused`, `observed_at_ms`, and `reuse_scope`; do not turn it \
        into a file citation. A verdict with no evidence under it is not a verdict.\n\
        \n\
        `executed=true` means that observation came from an execution. `reused=true` means a \
        successful observation originally executed earlier in this request was reused because \
        its declared inputs and captured process environment were unchanged; it is valid for \
        that stated reuse scope but is not a fresh run during checker preparation. Do not demand \
        another run merely because an honest reusable observation was supplied. Do not infer \
        coverage beyond the named command, declared inputs, or stated reuse scope.\n\
        \n\
        Never propose a fix, never write the corrected code, and never report anything the \
        supplied evidence or the files do not say — you are returning evidence, and a wrong \
        verdict the caller trusts is worse than no verdict. Name the material evidence gaps, \
        unread files, and whether you ran out of turns as your last line.",
    tools: &["read", "grep"],
    max_tokens: 2048,
    max_turns: 3,
    input: InputKind::Evidence,
    output: OutputKind::Verdict,
    call_sites: &[CallSite::CompletionGate, CallSite::Cell],
};

/// The roster. **This array is the whole extension point.**
pub const HELPERS: &[HelperSpec] = &[SCOUT, REDUCER, CHECKER];

/// Find a helper by the name the model calls it with.
pub fn lookup(name: &str) -> Option<&'static HelperSpec> {
    HELPERS.iter().find(|spec| spec.name == name)
}

/// Every guardrail a spec could violate, checked once at startup.
///
/// This runs before any helper does, so a malformed roster is a refusal with
/// one sentence rather than a helper that quietly holds `bash`.
pub fn validate() -> Result<(), String> {
    for spec in HELPERS {
        if HELPERS
            .iter()
            .filter(|other| other.name == spec.name)
            .count()
            > 1
        {
            return Err(format!("helper `{}` is declared twice", spec.name));
        }
        check_spec(spec)?;
    }
    Ok(())
}

/// Every guardrail one spec must satisfy.
///
/// Separate from [`validate`] so a test can drive **this** predicate against a
/// rogue spec: `HELPERS` is a `const`, so a bad entry cannot be pushed into it,
/// and a test that re-implemented the check would pass with the check deleted.
pub fn check_spec(spec: &HelperSpec) -> Result<(), String> {
    if spec.name.is_empty() {
        return Err("a helper has no name".to_string());
    }
    if spec.preamble.trim().is_empty() {
        return Err(format!("helper `{}` has no preamble", spec.name));
    }
    if spec.call_sites.is_empty() {
        return Err(format!("helper `{}` can never be invoked", spec.name));
    }
    if spec.max_turns == 0 || spec.max_turns > HELPER_MAX_TURNS {
        return Err(format!(
            "helper `{}` asks for {} turns; the range is 1..={HELPER_MAX_TURNS}",
            spec.name, spec.max_turns
        ));
    }
    for tool in spec.tools {
        if FORBIDDEN_TOOLS.contains(tool) {
            return Err(format!(
                "helper `{}` names the mutating tool `{tool}`; a helper answers questions \
                 and never changes the world",
                spec.name
            ));
        }
        if registry::lookup(tool).is_none() {
            return Err(format!(
                "helper `{}` names `{tool}`, which is not a registered tool",
                spec.name
            ));
        }
    }
    Ok(())
}

/// What one helper call produced.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct HelperOutcome {
    /// The helper's answer, or the failure sentence when `ok` is false.
    pub text: String,
    /// Whether the call produced an answer at all.
    ///
    /// A transport error and a refused status are **not** silently an empty
    /// answer: `supervisor.rs` shipped for weeks rendering a permanently
    /// failing look as a healthy one, and this field is why that cannot repeat
    /// here.
    pub ok: bool,
    /// The caller's cancellation token ended this call. Kept distinct from a
    /// provider or protocol failure so the session can consume exactly the
    /// interrupt this helper observed instead of leaking it into a later tool.
    #[serde(default)]
    pub cancelled: bool,
    pub elapsed_ms: u64,
}

/// Provider usage observed for one helper call. Counts and coverage travel
/// with the helper record so task totals can include helpers exactly once and
/// a missing usage row never masquerades as zero tokens.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct HelperUsage {
    /// False only when reading a record written before helper metering
    /// existed. Zero requests in such a record is unknown, not measured zero.
    pub coverage_known: bool,
    pub model: String,
    pub requests: u32,
    pub reported_requests: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_reported_requests: u32,
    pub cache_creation_reported_requests: u32,
}

impl HelperUsage {
    pub fn known_tokens(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.cache_read_input_tokens)
            .saturating_add(self.cache_creation_input_tokens)
    }

    pub fn complete(&self) -> bool {
        self.coverage_known
            && self.reported_requests == self.requests
            && self.cache_read_reported_requests == self.reported_requests
            && self.cache_creation_reported_requests == self.reported_requests
    }

    fn begin_request(&mut self) {
        self.requests = self.requests.saturating_add(1);
    }

    fn record_response(&mut self, usage: Option<crate::wire::Usage>) {
        let Some(usage) = usage else {
            return;
        };
        self.reported_requests = self.reported_requests.saturating_add(1);
        self.input_tokens = self.input_tokens.saturating_add(usage.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(usage.output_tokens);
        if let Some(tokens) = usage.cache_read_input_tokens {
            self.cache_read_input_tokens = self.cache_read_input_tokens.saturating_add(tokens);
            self.cache_read_reported_requests = self.cache_read_reported_requests.saturating_add(1);
        }
        if let Some(tokens) = usage.cache_creation_input_tokens {
            self.cache_creation_input_tokens =
                self.cache_creation_input_tokens.saturating_add(tokens);
            self.cache_creation_reported_requests =
                self.cache_creation_reported_requests.saturating_add(1);
        }
    }
}

/// Shared only with the owned provider worker so a cancellation can retain
/// every earlier completed turn and name the currently unreported request.
#[derive(Debug, Clone)]
pub struct HelperUsageTracker(Arc<Mutex<HelperUsage>>);

impl HelperUsageTracker {
    fn new(model: &str) -> Self {
        Self(Arc::new(Mutex::new(HelperUsage {
            coverage_known: true,
            model: model.to_string(),
            ..HelperUsage::default()
        })))
    }

    pub(crate) fn begin_request(&self) {
        self.0.lock().expect("helper usage lock").begin_request();
    }

    pub(crate) fn record_response(&self, usage: Option<crate::wire::Usage>) {
        self.0
            .lock()
            .expect("helper usage lock")
            .record_response(usage);
    }

    fn snapshot(&self) -> HelperUsage {
        self.0.lock().expect("helper usage lock").clone()
    }
}

impl Default for HelperOutcome {
    /// A record read back from an older rollout that predates this field.
    fn default() -> Self {
        Self {
            text: String::new(),
            ok: false,
            cancelled: false,
            elapsed_ms: 0,
        }
    }
}

impl HelperOutcome {
    fn failed(reason: impl Into<String>, started: Instant) -> Self {
        Self {
            text: reason.into(),
            ok: false,
            cancelled: false,
            elapsed_ms: started.elapsed().as_millis() as u64,
        }
    }

    fn cancelled(started: Instant) -> Self {
        Self {
            text: "the helper was cancelled".into(),
            ok: false,
            cancelled: true,
            elapsed_ms: started.elapsed().as_millis() as u64,
        }
    }
}

/// One completed helper call, as the cell ledger and the TUI both read it.
///
/// Built by whatever invoked the helper, so the lane, the `HELPERS` inspector
/// section and the trajectory all render the same facts rather than three
/// nearly-equal shapes.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct HelperRecord {
    /// The spec's `name`, e.g. `reduce`. Owned because a `CellView` is
    /// persisted to the rollout and read back on `--resume`.
    pub helper: String,
    /// The spec's `verb`, for the lane while it runs.
    pub verb: String,
    /// A short, already-bounded description of what it was asked, for the
    /// lane and the inspector. Never the payload itself.
    pub asked: String,
    /// What came back.
    pub outcome: HelperOutcome,
    /// Turns actually taken; `1` for a one-shot helper.
    pub turns: u32,
    /// What the helper actually reached for, tool names in order.
    ///
    /// A helper reports numbers it says it computed. Without this the claim
    /// cannot be checked: the caller sees an answer and no trace of the work.
    /// Host preparation operations and omissions are prefixed `prepare`;
    /// a toolless helper has no subsequent model-driven tool operations.
    #[serde(default)]
    pub looked: Vec<String>,
    /// The helper model and every provider-reported token class, including
    /// per-class request coverage when a provider omitted usage.
    #[serde(default)]
    pub usage: HelperUsage,
}

impl HelperRecord {
    /// The lane's one-line summary once the call has resolved.
    pub fn lane_result(&self) -> String {
        if self.outcome.ok {
            format!("{} · {}ms", self.asked, self.outcome.elapsed_ms)
        } else {
            format!("{} · {}", self.outcome.text, self.asked)
        }
    }
}

/// What one helper call produced, and what it actually cost in turns.
///
/// The invariant: **`turns` is what the call took, never what it was
/// allowed.** A [`HelperRecord`] built from `spec.max_turns` renders a scout
/// that answered on its first turn as one that burned eight, and
/// `little-helpers.md`'s inspector section exists precisely so a bad call is
/// visible rather than hidden behind an `OK`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperCall {
    pub outcome: HelperOutcome,
    pub turns: u32,
    /// Tool names the loop reached for, in order; empty for `run_once`.
    pub looked: Vec<String>,
    pub usage: HelperUsage,
}

/// Run any helper in the roster: the one entry point a caller uses.
///
/// Dispatches on the spec rather than on its name, so a new `HelperSpec`
/// needs no call-site edit. A toolless one-turn spec is [`run_once`]; a spec
/// holding tools or asking for more than one turn needs the agent loop and
/// goes to [`run_with_tools`].
pub fn run(
    spec: &HelperSpec,
    model: &str,
    input: &str,
    profile: &crate::sandbox::profile::Profile,
    glasshouse: &crate::glasshouse::Glasshouse,
    session: &crate::contract::SessionId,
    token: &crate::tools::invoke::CancellationToken,
) -> HelperCall {
    let started = Instant::now();
    let prepared = crate::helper_context::HelperRole::from_helper_name(spec.name)
        .map(|role| crate::helper_context::prepare(role, input, profile, token));
    // The appended original is bounded by the same limit the preparation
    // uses. Sending it whole would contradict the omission the packet already
    // recorded, and on a log large enough to be worth reducing it would
    // overflow the helper model's own context window -- so the reduction
    // would stop working exactly when it matters most.
    let request = prepared.as_ref().map(|packet| {
        format!(
            "{}\n\nOriginal helper request:\n{}",
            packet.rendered,
            crate::helper_context::bounded_string(input, crate::helper_context::MAX_INPUT_BYTES)
        )
    });
    let mut call = run_unprepared(
        spec,
        model,
        request.as_deref().unwrap_or(input),
        profile,
        glasshouse,
        session,
        token,
    );
    if let Some(packet) = prepared {
        let mut operations: Vec<String> = packet
            .operations
            .into_iter()
            .map(|operation| format!("prepare: {} {}", operation.action, operation.subject))
            .collect();
        operations.extend(packet.omissions.into_iter().map(|omission| {
            format!(
                "prepare omitted: {} ({})",
                omission.subject, omission.reason
            )
        }));
        operations.append(&mut call.looked);
        call.looked = operations;
    }
    call.outcome.elapsed_ms = started.elapsed().as_millis() as u64;
    call
}

fn run_unprepared(
    spec: &HelperSpec,
    model: &str,
    input: &str,
    profile: &crate::sandbox::profile::Profile,
    glasshouse: &crate::glasshouse::Glasshouse,
    session: &crate::contract::SessionId,
    token: &crate::tools::invoke::CancellationToken,
) -> HelperCall {
    let started = Instant::now();
    if token.is_cancelled() {
        return HelperCall {
            outcome: HelperOutcome::cancelled(started),
            turns: 0,
            looked: Vec::new(),
            usage: HelperUsage {
                coverage_known: true,
                model: model.to_string(),
                ..HelperUsage::default()
            },
        };
    }
    if one_shot(spec) {
        let spec = *spec;
        let model = model.to_string();
        let input = input.to_string();
        let usage = HelperUsageTracker::new(&model);
        usage.begin_request();
        let worker_usage = usage.clone();
        match wait_for_helper(token, move || {
            run_once_metered(&spec, &model, &input, &worker_usage)
        }) {
            HelperWait::Returned(call) => call,
            HelperWait::Cancelled => HelperCall {
                outcome: HelperOutcome::cancelled(started),
                turns: 0,
                looked: Vec::new(),
                usage: usage.snapshot(),
            },
            HelperWait::Panicked => HelperCall {
                outcome: HelperOutcome::failed(
                    format!("`{}` panicked while running", spec.name),
                    started,
                ),
                turns: 0,
                looked: Vec::new(),
                usage: usage.snapshot(),
            },
        }
    } else {
        run_with_tools(spec, model, input, profile, glasshouse, session, token)
    }
}

/// How often a blocked helper checks whether its caller cancelled it.
const HELPER_CANCEL_POLL: Duration = Duration::from_millis(20);

enum HelperWait<T> {
    Returned(T),
    Cancelled,
    Panicked,
}

/// Run an owned helper operation away from the caller's V8 isolate while the
/// caller remains able to observe cancellation. A cancelled provider request
/// may finish on this worker, but it owns every value it retained and remains
/// bounded by the wire timeout; its result is discarded.
fn wait_for_helper<T, F>(
    token: &crate::tools::invoke::CancellationToken,
    operation: F,
) -> HelperWait<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation));
        let _ = sender.send(result);
    });
    loop {
        if token.is_cancelled() {
            return HelperWait::Cancelled;
        }
        match receiver.recv_timeout(HELPER_CANCEL_POLL) {
            Ok(Ok(value)) => return HelperWait::Returned(value),
            Ok(Err(_)) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                return HelperWait::Panicked;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

/// Whether one wire call serves this spec: nothing to call a tool with, and
/// one turn to do it in.
fn one_shot(spec: &HelperSpec) -> bool {
    spec.tools.is_empty() && spec.max_turns == 1
}

/// Run a one-turn, toolless helper: the spec's preamble as the system block,
/// the caller's input as the one user message, the purpose header set.
///
/// A helper holding tools or asking for more than one turn needs the agent
/// loop and is not this function's job; [`validate`] permits such a spec and
/// callers dispatch on `max_turns`.
pub fn run_once(spec: &HelperSpec, model: &str, input: &str) -> HelperOutcome {
    let usage = HelperUsageTracker::new(model);
    usage.begin_request();
    run_once_metered(spec, model, input, &usage).outcome
}

fn run_once_metered(
    spec: &HelperSpec,
    model: &str,
    input: &str,
    usage: &HelperUsageTracker,
) -> HelperCall {
    let started = Instant::now();
    debug_assert!(spec.tools.is_empty() && spec.max_turns == 1);

    let conversation = Conversation {
        system: spec.preamble.to_string(),
        messages: vec![Message::text(Role::User, input)],
    };
    let outcome = match wire::send_turn_with_usage(
        &conversation,
        model,
        spec.max_tokens,
        Some(PURPOSE_HEADER),
    ) {
        Ok(turn) => {
            usage.record_response(turn.usage);
            let text: String = turn
                .message
                .content
                .iter()
                .map(crate::contract::Block::text)
                .collect::<Vec<_>>()
                .join("");
            if text.trim().is_empty() {
                HelperOutcome::failed("the helper returned nothing", started)
            } else {
                HelperOutcome {
                    text,
                    ok: true,
                    cancelled: false,
                    elapsed_ms: started.elapsed().as_millis() as u64,
                }
            }
        }
        Err(err) => HelperOutcome::failed(format!("request failed: {err}"), started),
    };
    HelperCall {
        outcome,
        turns: 1,
        looked: Vec::new(),
        usage: usage.snapshot(),
    }
}

/// Run a helper that holds tools, through the subagent loop with its toolset
/// narrowed to `spec.tools`, its instructions replaced by the preamble, and
/// its model set to the tier.
///
/// The invariant: **only a returned answer is an answer.** The loop can also
/// end out of turns, cancelled or failed, and every one of those is `ok:
/// false` with the reason in the text -- a helper that ran out of turns
/// having said nothing must not read as a healthy short answer.
pub fn run_with_tools(
    spec: &HelperSpec,
    model: &str,
    input: &str,
    profile: &crate::sandbox::profile::Profile,
    glasshouse: &crate::glasshouse::Glasshouse,
    session: &crate::contract::SessionId,
    token: &crate::tools::invoke::CancellationToken,
) -> HelperCall {
    let started = Instant::now();
    let options = crate::agent::AgentOptions {
        turns: u64::from(spec.max_turns.min(HELPER_MAX_TURNS)),
        model: model.to_string(),
        effort: crate::wire::Effort::default(),
    };
    let narrowed = crate::agent::Narrowed {
        tools: spec.tools,
        instructions: spec.preamble,
    };
    let usage = HelperUsageTracker::new(model);
    let worker_usage = usage.clone();
    // **On an owned thread, always.** `run_narrowed` builds a Runtime, which is
    // a second V8 isolate, and this function is reached from a host callback
    // while the caller's isolate is borrowed. Owning every input also lets the
    // caller stop waiting when cancelled; the provider request may finish on
    // this thread under its hard ceiling, but it retains no session borrow and
    // the post-response token check starts no late tool.
    let profile = profile.clone();
    let glasshouse = glasshouse.clone();
    let session = session.clone();
    let input = input.to_string();
    let worker_token = token.clone();
    let result = match wait_for_helper(token, move || {
        crate::agent::run_narrowed_metered(
            &profile,
            &glasshouse,
            &session,
            &input,
            &options,
            &worker_token,
            crate::agent::NarrowedRun::helper(&narrowed, &worker_usage),
        )
    }) {
        HelperWait::Returned(result) => result,
        HelperWait::Cancelled => {
            return HelperCall {
                outcome: HelperOutcome::cancelled(started),
                turns: 0,
                looked: Vec::new(),
                usage: usage.snapshot(),
            };
        }
        HelperWait::Panicked => {
            return HelperCall {
                outcome: HelperOutcome::failed(
                    format!("`{}` panicked while running", spec.name),
                    started,
                ),
                turns: 0,
                looked: Vec::new(),
                usage: usage.snapshot(),
            };
        }
    };

    let answer = result.answer.trim();
    let outcome = if result.status != "returned" {
        HelperOutcome::failed(
            format!(
                "the call ended `{}`: {}",
                result.status,
                if answer.is_empty() {
                    "no answer"
                } else {
                    answer
                }
            ),
            started,
        )
    } else if answer.is_empty() {
        HelperOutcome::failed("the helper returned nothing", started)
    } else {
        HelperOutcome {
            text: result.answer.clone(),
            ok: true,
            cancelled: false,
            elapsed_ms: started.elapsed().as_millis() as u64,
        }
    };
    HelperCall {
        outcome,
        turns: u32::try_from(result.turns).unwrap_or(u32::MAX),
        looked: result.trajectory,
        usage: usage.snapshot(),
    }
}

// ---------------------------------------------------------------------
// The three call sites that are not a cell.
//
// Each is a thin entry point rather than a second runtime: they all reach
// `run`, so a helper invoked automatically is the same helper the model can
// call, under the same guardrails.
// ---------------------------------------------------------------------

/// SCOUT at `CallSite::Preflight` -- once per task, after the request arrives
/// and before the model's first turn.
///
/// Returns the block to append to the system prompt, or `None` when helpers
/// are off, no spec serves this site, or the call failed. **A failed preflight
/// is never fatal**: the task runs with the static orientation, exactly as it
/// does today.
pub fn preflight(
    task: &str,
    model: &str,
    profile: &crate::sandbox::profile::Profile,
    glasshouse: &crate::glasshouse::Glasshouse,
    session: &crate::contract::SessionId,
    token: &crate::tools::invoke::CancellationToken,
    mut progress: impl FnMut(&HelperRecord),
) -> Option<HelperRecord> {
    let spec = HELPERS
        .iter()
        .find(|spec| spec.call_sites.contains(&CallSite::Preflight))?;
    let mut record = HelperRecord {
        helper: spec.name.to_string(),
        verb: spec.verb.to_string(),
        asked: bounded_ask(task),
        ..HelperRecord::default()
    };
    progress(&record);
    let call = run(spec, model, task, profile, glasshouse, session, token);
    record.outcome = call.outcome;
    record.turns = call.turns;
    record.looked = call.looked;
    record.usage = call.usage;
    progress(&record);
    Some(record)
}

/// CHECKER at `CallSite::CompletionGate` -- before a completion is accepted.
pub fn check_completion(
    evidence: &str,
    model: &str,
    profile: &crate::sandbox::profile::Profile,
    glasshouse: &crate::glasshouse::Glasshouse,
    session: &crate::contract::SessionId,
) -> Option<HelperRecord> {
    let spec = HELPERS
        .iter()
        .find(|spec| spec.call_sites.contains(&CallSite::CompletionGate))?;
    let call = run(
        spec,
        model,
        evidence,
        profile,
        glasshouse,
        session,
        &crate::tools::invoke::CancellationToken::new(),
    );
    Some(HelperRecord {
        helper: spec.name.to_string(),
        verb: spec.verb.to_string(),
        asked: bounded_ask(evidence),
        outcome: call.outcome,
        turns: call.turns,
        looked: call.looked,
        usage: call.usage,
    })
}

/// The recap `[helpers] completion = "recap"` asks for: one or two sentences
/// on what the session did, and one suggested next prompt.
///
/// A one-shot toolless call by construction -- it summarises what already
/// happened and must not go looking for more.
pub const RECAP_PREAMBLE: &str = "You close out a coding session. In at most two sentences, say what was actually done — \
     from the transcript only, never inferred. Then, on a new line beginning `Next:`, suggest \
     one specific next prompt the user could send. Never invent work that did not happen, and \
     never claim something succeeded that the transcript does not show succeeding.";

/// One short description of an input, bounded, never the payload itself.
fn bounded_ask(input: &str) -> String {
    let line = input.lines().next().unwrap_or("").trim();
    if line.chars().count() <= 60 {
        line.to_string()
    } else {
        format!("{}…", line.chars().take(59).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_roster_is_valid() {
        validate().expect("the roster must pass its own guardrails");
    }

    #[test]
    fn a_spec_naming_a_mutating_tool_is_refused() {
        // Driven through the production predicate, so deleting the guard fails
        // this test rather than leaving it green.
        /// One `'static` toolset per forbidden tool. Kept honest by the
        /// assertion below rather than by matching it up by eye.
        const ROGUE: [(&str, &[&str]); 3] = [
            ("write", &["write"]),
            ("edit", &["edit"]),
            ("bash", &["bash"]),
        ];

        assert_eq!(
            ROGUE.map(|(name, _)| name),
            FORBIDDEN_TOOLS,
            "this test must cover every forbidden tool"
        );

        for (tool, toolset) in ROGUE {
            assert!(
                registry::lookup(tool).is_some(),
                "`{tool}` must be a real tool, or the refusal guards nothing"
            );
            let rogue = HelperSpec {
                tools: toolset,
                ..REDUCER
            };
            let refused =
                check_spec(&rogue).expect_err("a helper holding a mutating tool must be refused");
            assert!(refused.contains(tool), "{refused}");
            assert!(refused.contains("never changes the world"), "{refused}");
        }
    }

    #[test]
    fn a_spec_naming_an_unregistered_tool_is_refused() {
        let rogue = HelperSpec {
            tools: &["telepathy"],
            ..REDUCER
        };
        let refused = check_spec(&rogue).expect_err("an unknown tool must be refused");
        assert!(refused.contains("not a registered tool"), "{refused}");
    }

    #[test]
    fn a_spec_with_no_call_site_or_too_many_turns_is_refused() {
        let unreachable = HelperSpec {
            call_sites: &[],
            ..REDUCER
        };
        assert!(
            check_spec(&unreachable)
                .expect_err("a helper nothing can invoke must be refused")
                .contains("never be invoked")
        );

        let greedy = HelperSpec {
            max_turns: HELPER_MAX_TURNS + 1,
            ..REDUCER
        };
        assert!(
            check_spec(&greedy)
                .expect_err("a helper over the turn ceiling must be refused")
                .contains("the range is")
        );
    }

    /// The three `little-helpers.md` starts with, by the names the model
    /// calls them by. `lookup` is the production path a binding uses, so a
    /// spec that is written but not appended to `HELPERS` fails here.
    #[test]
    fn the_roster_holds_the_three_the_spec_starts_with() {
        for name in ["find", "reduce", "check"] {
            assert!(
                lookup(name).is_some(),
                "`{name}` must be in the roster the model is offered"
            );
        }
        assert_eq!(
            HELPERS.len(),
            3,
            "the roster is the whole extension point; nothing else may be in it"
        );
    }

    /// `run` routes on the spec rather than on the name: a toolless one-turn
    /// spec is one wire call, and anything else needs the agent loop. Driven
    /// through the production predicate, because a test that re-derived the
    /// condition would stay green with the branch deleted.
    #[test]
    fn each_spec_is_routed_to_the_runtime_that_can_serve_it() {
        assert!(
            one_shot(&REDUCER),
            "the reducer holds nothing and takes one turn"
        );
        for spec in [&SCOUT, &CHECKER] {
            assert!(
                !one_shot(spec),
                "`{}` holds tools, so it needs the agent loop",
                spec.name
            );
        }
    }

    #[test]
    fn the_reducer_holds_no_tools_and_takes_one_turn() {
        assert!(REDUCER.tools.is_empty(), "the reducer must reach nothing");
        assert_eq!(REDUCER.max_turns, 1);
    }

    /// `little-helpers.md`: the two contracts no toolset can express stay in
    /// **each** helper's preamble. Asserted over the whole roster rather than
    /// per spec, so a helper appended tomorrow cannot ship without them.
    #[test]
    fn every_helper_states_the_two_contracts_in_its_own_preamble() {
        for spec in HELPERS {
            for phrase in [
                "you are returning evidence",
                "Never propose a fix",
                "as your last line",
            ] {
                assert!(
                    spec.preamble.contains(phrase),
                    "`{}` must say `{phrase}`: evidence never conclusions, and say what you \
                     did not look at",
                    spec.name
                );
            }
        }
    }

    /// The toolsets are driven through the production predicate: re-listing
    /// them here would pass with `check_spec` deleted, and `check_spec` is
    /// what refuses a mutating or unregistered name.
    #[test]
    fn the_scout_and_the_checker_hold_only_registered_read_only_tools() {
        for spec in [&SCOUT, &CHECKER] {
            check_spec(spec).unwrap_or_else(|refused| panic!("{refused}"));
            assert!(
                !spec.tools.is_empty(),
                "`{}` reads the project, so an empty toolset would make it a one-shot",
                spec.name
            );
        }
    }

    #[test]
    fn the_reducer_forbids_conclusions_in_its_own_preamble() {
        // The one contract no toolset can express, so it must be in the prose.
        assert!(REDUCER.preamble.contains("Never state a cause"));
        assert!(REDUCER.preamble.contains("evidence"));
    }

    #[test]
    fn a_waiting_helper_returns_promptly_when_its_caller_cancels() {
        let token = crate::tools::invoke::CancellationToken::new();
        let canceller = token.clone();
        let (release, held) = mpsc::channel::<()>();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            canceller.cancel();
        });

        let started = Instant::now();
        let result = wait_for_helper(&token, move || {
            let _ = held.recv_timeout(Duration::from_secs(1));
            7
        });
        drop(release);

        assert!(matches!(result, HelperWait::Cancelled));
        assert!(
            started.elapsed() < Duration::from_millis(250),
            "cancellation waited for the owned helper operation"
        );
    }
}
