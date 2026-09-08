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

/// The roster. **This array is the whole extension point.**
pub const HELPERS: &[HelperSpec] = &[REDUCER];

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

    #[test]
    fn the_reducer_holds_no_tools_and_takes_one_turn() {
        assert!(REDUCER.tools.is_empty(), "the reducer must reach nothing");
        assert_eq!(REDUCER.max_turns, 1);
    }

    #[test]
    fn the_reducer_forbids_conclusions_in_its_own_preamble() {
        // The one contract no toolset can express, so it must be in the prose.
        assert!(REDUCER.preamble.contains("Never state a cause"));
        assert!(REDUCER.preamble.contains("evidence"));
    }
}
