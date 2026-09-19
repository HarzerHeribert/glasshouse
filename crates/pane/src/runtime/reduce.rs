//! Reducing an oversized command result, and saying what that cost.
//!
//! `little-helpers.md`'s `CallSite::PostResult` in one place: the decision
//! whether a reduction is worth making, the deterministic rung that often
//! makes one unnecessary ([`super::reduce_rules`]), and the line a reduction
//! carries about its own lossiness.
//!
//! Split out of `bindings.rs` when it crossed the Phase 59 ratchet. The
//! binding itself stays there; what a reduction *is* lives here.

use std::rc::Rc;

use super::bindings::helper::asked_summary;
use super::preview::{estimate_tokens, thousands};
use super::state::RuntimeState;
use crate::tools::invoke::ToolResult;

/// What became of a pushed reduction, for a caller that must tell "not needed"
/// from "attempted and failed".
///
/// The distinction is the whole point (user ruling, 2026-09-10): a reducer is
/// never handed a truncated log, so an output too large for the helper model
/// fails the request — and a parent that cannot see that failure cannot do the
/// one thing that fixes it, which is narrow the command and run it again. An
/// absent key used to mean both "small enough to read yourself" and "we tried
/// and could not", which are opposite instructions.
pub(super) enum Reduction {
    /// Under the cap, or no helper configured: nothing was attempted.
    NotAttempted,
    Made(String),
    /// Attempted and failed, with the one sentence the parent needs.
    Failed(String),
}

/// The share of its allowance a reduction may use before it is reported as
/// possibly partial.
///
/// A hard truncation is already a failure, never an answer: the provider
/// sets `stop_reason: "max_tokens"`, [`crate::wire`] turns that into
/// `IncompleteResponse`, the helper's `ok` is false and the result carries
/// `reduction_error` instead of `reduced`. This is the *soft* case -- an
/// answer that stopped on its own with almost no room left, which reads
/// exactly like a complete one and is the shape a reader cannot otherwise
/// detect.
const CROWDED: f64 = 0.9;

/// The one line a reduction carries about itself.
///
/// **A summary that looks complete is never re-checked.** The argument that
/// made a reduction safe -- `stdout` and `stderr` stay whole, so the parent
/// can always read the original -- only holds if the parent knows it has
/// reason to, and a reduction with no provenance gives it none: three
/// failures summarised out of two hundred read the same as three out of
/// three.
///
/// One line, because this text is spent on every reduced result. It leads
/// rather than trails so a reader frames the content before trusting it, and
/// it opens with a marker no build log produces, so a reduction *of* a log
/// that discusses reductions cannot be mistaken for it.
fn lossiness_line(input: &str, output: &str, note: Option<&str>) -> String {
    let lines = |text: &str| {
        let n = text.lines().count();
        format!(
            "{} line{}",
            thousands(n as u64),
            if n == 1 { "" } else { "s" }
        )
    };
    let bytes = |text: &str| thousands(text.len() as u64);
    let note = note.map(|note| format!(" · {note}")).unwrap_or_default();
    format!(
        "[pane:reduction {} / {} bytes → {} / {} bytes{note} · \
         `stdout` and `stderr` on this result are complete and unchanged]",
        lines(input),
        bytes(input),
        lines(output),
        bytes(output),
    )
}

/// [`lossiness_line`] joined to the reduction it describes.
fn with_lossiness(input: &str, output: &str, note: Option<&str>) -> String {
    format!("{}\n{output}", lossiness_line(input, output, note))
}

/// `little-helpers.md`'s `CallSite::PostResult`: a command result the model
/// would otherwise have to page is reduced by REDUCER, without the model
/// spending a turn to ask.
///
/// **Cheap-model tokens are spent only to remove parent tokens, never to add
/// a second view of a small output.** The trigger is `[helpers]
/// reduce_above_tokens`, and above it a request is made only when the
/// expected saving is positive: the output's estimated tokens minus the
/// reducer's own `max_tokens` must exceed half the threshold. Below either
/// line the parent reads the output itself.
///
/// Only [`typed_result`]'s command-output arm reaches here, so `read`,
/// `context`, `edit`, `grep` and `glob` — every shape a model edits or quotes
/// from — are excluded structurally. The exact `stdout` and `stderr` stay
/// complete on the result; the reduction is one more property beside them,
/// and a helper failing here is never fatal.
pub(super) fn reduce_oversized(result: &ToolResult, state: &Rc<RuntimeState>) -> Reduction {
    let threshold = state.reduce_above_tokens();
    let tokens = estimate_tokens(&result.stdout) + estimate_tokens(&result.stderr);
    if tokens <= threshold {
        return Reduction::NotAttempted;
    }
    let Some(spec) = crate::helpers::HELPERS.iter().find(|spec| {
        spec.call_sites
            .contains(&crate::helpers::CallSite::PostResult)
    }) else {
        return Reduction::NotAttempted;
    };
    let Ok((model, effort)) = state.helper_route(spec.name) else {
        return Reduction::NotAttempted;
    };

    // Both streams, because a build writes its failures to whichever it
    // likes and the reduction is of the output, not of one pipe.
    let mut text = result.stdout.clone();
    text.push_str(&result.stderr);

    // **The deterministic rung, before a model is considered at all.** Most
    // of what trips the threshold -- a thousand passing test lines, a page of
    // `Compiling`, a warning repeated forty times -- is removable by a
    // predicate, and a request is worth making for what is left rather than
    // for what a rule could have deleted for free. An input no rule
    // recognises comes back byte-identical, so the path below is unchanged
    // where this does not help.
    let original = text;
    let ruled = crate::runtime::reduce_rules::apply(&original);
    let text = ruled.text;
    let tokens = estimate_tokens(&text);
    if tokens <= threshold {
        state.count_reduction(|stats| {
            stats.ruled += 1;
            stats.bytes_out += text.len() as u64;
        });
        let note = format!("rules only, no model: {}", ruled.applied.join(", "));
        return Reduction::Made(with_lossiness(&original, &text, Some(&note)));
    }

    // The allowance the wire will actually carry, not the one the spec
    // declares. They were seventeen times apart: `REDUCER` names 1,024 and
    // `configure_effort` sent 16,384 of thinking beside it, so this test --
    // written to stop a reduction costing more than it saves -- was computed
    // against a number that made every reduction look free.
    //
    // The cap scales with the work (`helpers::reduction_cap`): a dense
    // failure log needs about a quarter of its own size to name every
    // distinct failure, and a constant 1,024 turned that into a refusal the
    // caller had already paid a request for.
    let cap = crate::helpers::reduction_cap(tokens);
    let allowance_tokens =
        crate::wire::wire_max_tokens(&model, crate::wire::Allowance::Capped(cap), effort);
    let allowance = allowance_tokens as usize;
    if tokens.saturating_sub(allowance) <= threshold / 2 {
        return Reduction::NotAttempted;
    }

    use sha2::{Digest, Sha256};
    let digest = format!("{:x}", Sha256::digest(text.as_bytes()));
    if let Some(reduction) = state.reduction_of(&digest) {
        state.count_reduction(|stats| {
            stats.cached += 1;
            stats.bytes_out += reduction.len() as u64;
        });
        return Reduction::Made(reduction);
    }
    // After the cache and before the call: a served reduction spends neither
    // a request nor a slot, and a claimed slot is always a request made.
    if state.claim_pushed_helper_call().is_err() {
        return Reduction::NotAttempted;
    }
    state.count_reduction(|stats| {
        stats.attempted += 1;
        stats.bytes_in += text.len() as u64;
    });

    let asked = asked_summary(&text);
    let slot = state.begin_helper(crate::helpers::HelperRecord {
        helper: spec.name.to_string(),
        verb: spec.verb.to_string(),
        asked: asked.clone(),
        ..crate::helpers::HelperRecord::default()
    });
    let token = state.token.borrow().clone();
    // A helper thinking is the cell waiting (`RuntimeState::away_from_js`);
    // the call outlasts the whole cell limit by design — a one-shot errand
    // ends on silence (`wire::SIDE_ERRAND_SILENCE`) and never on duration —
    // so without this one helper could spend the cell's clock.
    let _away = state.away_from_js();
    let call = crate::helpers::run(
        spec,
        crate::helpers::HelperRoute {
            model: &model,
            effort,
            cap: Some(cap),
        },
        &text,
        &state.profile,
        &state.glasshouse,
        &state.session,
        &token,
    );
    let ok = call.outcome.ok;
    let cancelled = call.outcome.cancelled;
    let reduction = call.outcome.text.clone();
    let spent = call.usage.output_tokens;
    state.finish_helper(slot, call);
    if cancelled {
        return Reduction::NotAttempted;
    }
    if !ok {
        state.count_reduction(|stats| stats.failed += 1);
        // The exact output is untouched and still on the result; what the
        // parent is being told is that it will not get a summary of it unless
        // it narrows what the command prints.
        return Reduction::Failed(format!(
            "This output was too large to summarise and no summary was made \
             ({reduction}). `stdout` and `stderr` are complete and unchanged. \
             Narrow what the command prints, or select the part you need, \
             before relying on a summary."
        ));
    }
    // A reduction that stopped with almost none of its allowance left is a
    // partial answer wearing a complete one's clothes. The provider did not
    // truncate it -- that path is `ok: false` above -- so nothing but the
    // arithmetic can say so.
    let crowded = spent > 0 && (spent as f64) >= f64::from(allowance_tokens) * CROWDED;
    let note = crowded.then(|| {
        format!(
            "the reducer used {} of its {}-token allowance, so read it as partial and check \
             `stdout` before relying on it",
            thousands(spent),
            thousands(u64::from(allowance_tokens)),
        )
    });
    let answer = with_lossiness(&text, &reduction, note.as_deref());
    // Counted on the answer that reaches the program, not on the model's
    // bare text: the served copy below counts the same value, and two
    // spellings of `bytes_out` would make the pair disagree about one cache.
    state.count_reduction(|stats| {
        stats.made += 1;
        stats.bytes_out += answer.len() as u64;
    });
    state.remember_reduction(digest, answer.clone());
    Reduction::Made(answer)
}
