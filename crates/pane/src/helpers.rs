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

use std::time::Instant;

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

/// Check a claim against a diff, and return the evidence with the verdict.
///
/// `read` and `grep` only: it decides whether something holds, and a helper
/// that could also change it would be deciding about its own work.
pub const CHECKER: HelperSpec = HelperSpec {
    name: "check",
    summary: "Check a claim against a diff and answer with a verdict plus the file:line evidence for it.",
    verb: "checking",
    preamble: "Never state a number you did not compute. You have a runtime: count and total in a \
        cell and report what it returned. A computed number is evidence; an estimated one \
        is a conclusion, and conclusions are not yours to draw.\n\
        \n\
        You check whether a claim holds for a diff you are given. You can read and grep, \
        and you can change nothing.\n\
        \n\
        Open with the verdict alone on the first line: `holds`, `does not hold`, or `cannot \
        tell`. `cannot tell` is a real verdict and is the honest one whenever the diff does not \
        contain what the claim is about. Under it, the evidence and nothing else: each point as \
        `path/to/file.rs:120` and the text there that decides it, quoted as it stands. A verdict \
        with no evidence under it is not a verdict.\n\
        \n\
        Never propose a fix, never write the corrected code, and never report anything the diff \
        or the files do not say — you are returning evidence, and a wrong verdict the caller \
        trusts is worse than no verdict. Name what you did not check — the parts of the diff \
        you did not open, and whether you ran out of turns — as your last line.",
    tools: &["read", "grep"],
    max_tokens: 2048,
    max_turns: 3,
    input: InputKind::Diff,
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
    pub elapsed_ms: u64,
}

impl Default for HelperOutcome {
    /// A record read back from an older rollout that predates this field.
    fn default() -> Self {
        Self {
            text: String::new(),
            ok: false,
            elapsed_ms: 0,
        }
    }
}

impl HelperOutcome {
    fn failed(reason: impl Into<String>, started: Instant) -> Self {
        Self {
            text: reason.into(),
            ok: false,
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
    /// Empty for a toolless helper, which reaches for nothing by construction.
    #[serde(default)]
    pub looked: Vec<String>,
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
) -> HelperCall {
    if one_shot(spec) {
        HelperCall {
            outcome: run_once(spec, model, input),
            turns: 1,
            looked: Vec::new(),
        }
    } else {
        run_with_tools(spec, model, input, profile, glasshouse, session)
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
    let started = Instant::now();
    debug_assert!(spec.tools.is_empty() && spec.max_turns == 1);

    let conversation = Conversation {
        system: spec.preamble.to_string(),
        messages: vec![Message::text(Role::User, input)],
    };
    match wire::send_turn_with(&conversation, model, spec.max_tokens, Some(PURPOSE_HEADER)) {
        Ok(message) => {
            let text: String = message
                .content
                .iter()
                .map(crate::contract::Block::text)
                .collect::<Vec<_>>()
                .join("");
            if text.trim().is_empty() {
                return HelperOutcome::failed("the helper returned nothing", started);
            }
            HelperOutcome {
                text,
                ok: true,
                elapsed_ms: started.elapsed().as_millis() as u64,
            }
        }
        Err(err) => HelperOutcome::failed(format!("request failed: {err}"), started),
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
    // **On another thread, always.** `run_narrowed` builds a Runtime, which is
    // a second V8 isolate, and this function is reached from a host callback
    // while the caller's isolate is borrowed -- `agent.rs`'s module doc names
    // that hazard and `bg::serve_once` already solves it the same way. A V8
    // isolate belongs to one thread, so giving the nested loop its own is what
    // makes a tool-holding helper safe to call at all.
    let token = crate::tools::invoke::CancellationToken::new();
    let result = std::thread::scope(|scope| {
        scope
            .spawn(|| {
                crate::agent::run_narrowed(
                    profile,
                    glasshouse,
                    session,
                    input,
                    &options,
                    &token,
                    Some(&narrowed),
                )
            })
            .join()
    });
    let result = match result {
        Ok(result) => result,
        Err(_) => {
            return HelperCall {
                outcome: HelperOutcome::failed(
                    format!("`{}` panicked while running", spec.name),
                    started,
                ),
                turns: 0,
                looked: Vec::new(),
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
            elapsed_ms: started.elapsed().as_millis() as u64,
        }
    };
    HelperCall {
        outcome,
        turns: u32::try_from(result.turns).unwrap_or(u32::MAX),
        looked: result.trajectory,
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
) -> Option<HelperRecord> {
    let spec = HELPERS
        .iter()
        .find(|spec| spec.call_sites.contains(&CallSite::Preflight))?;
    let call = run(spec, model, task, profile, glasshouse, session);
    Some(HelperRecord {
        helper: spec.name.to_string(),
        verb: spec.verb.to_string(),
        asked: bounded_ask(task),
        outcome: call.outcome,
        turns: call.turns,
        looked: call.looked,
    })
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
    let call = run(spec, model, evidence, profile, glasshouse, session);
    Some(HelperRecord {
        helper: spec.name.to_string(),
        verb: spec.verb.to_string(),
        asked: bounded_ask(evidence),
        outcome: call.outcome,
        turns: call.turns,
        looked: call.looked,
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
}
