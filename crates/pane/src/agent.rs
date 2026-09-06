//! A subagent: one nested turn loop, run out of band, whose answer comes back
//! as a handle — capability map Phase 64.
//!
//! **A subagent is a background job whose work is a turn loop rather than a
//! spawned command.** That is the whole design decision, and it is what makes
//! this module small. A cell cannot call the session loop directly: the
//! isolate is borrowed while the cell runs, so re-entering the loop from a
//! host callback would re-enter V8. `bg` already solved exactly that shape —
//! return a handle at once, do the work on another thread, deliver completion
//! as an event whose result is a handle — so a subagent rides it as a second
//! producer instead of inventing a second delivery path. Cancellation, the
//! deadline, the payload store, batching and dedup are all `bg`'s and are not
//! reimplemented here.
//!
//! **What a subagent deliberately is not**: it has no rollout of its own, no
//! supervisor, no TUI, no inbox, no row in Glasshouse's session list, and it
//! cannot start a subagent of its own. It is a task with a budget, not a
//! session.

use crate::contract::{Conversation, Message, Role, SessionId};
use crate::glasshouse::Glasshouse;
use crate::prompt::{self, Budget, CellResult, ErrorSection, Extracted};
use crate::runtime::isolate::Runtime;
use crate::runtime::outcome::CellOutcome;
use crate::sandbox::profile::Profile;
use crate::tools::invoke::CancellationToken;
use crate::tools::registry;
use crate::wire::{self, Effort};

/// The most turns a subagent may take whatever it was asked for.
///
/// A cap here and not only at the call: `agent.run({turns})` is written by the
/// model, and a subagent that could ask for a thousand turns would be a way to
/// spend the parent's whole budget in one call it does not watch.
pub const MAX_TURNS: u64 = 24;

/// The turns a subagent takes when the cell named none.
pub const DEFAULT_TURNS: u64 = 8;

/// What a subagent produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentResult {
    /// The subagent's own answer — its top-level `return`, rendered.
    pub answer: String,
    /// Why it stopped, in one word: `returned`, `turns`, `cancelled`,
    /// `failed`.
    pub status: String,
    /// Turns actually taken, which is what the parent's budget is charged.
    pub turns: u64,
    /// Provider-reported tokens, summed over the turns that reported any.
    pub tokens: u64,
}

/// How a subagent is asked for.
#[derive(Debug, Clone)]
pub struct AgentOptions {
    pub turns: u64,
    pub model: String,
    pub effort: Effort,
}

/// Runs one subagent to its end. Blocking, and called on `bg`'s own worker
/// thread — never on the thread that holds the parent isolate.
///
/// **The profile is the parent's, cloned and not recompiled.** A subagent that
/// compiled its own profile could differ from its parent's by a file edited
/// mid-session, which is a widening no one asked for; `sandbox-grants.md` §1.5
/// computes a profile once per session and this honours that across the nested
/// loop too.
pub fn run(
    profile: &Profile,
    glasshouse: &Glasshouse,
    session: &SessionId,
    task: &str,
    options: &AgentOptions,
    token: &CancellationToken,
) -> AgentResult {
    let tools: Vec<&registry::Tool> = registry::ALL.iter().collect();
    let facts = crate::session::session_facts(profile);
    let system = prompt::render_system(SUBAGENT_INSTRUCTIONS, &tools, &facts);
    let mut conversation = Conversation {
        system,
        messages: vec![Message::text(Role::User, task)],
    };

    let mut runtime = Runtime::new(profile, glasshouse, session).as_subagent();
    let mut tokens = 0u64;
    let turns_allowed = options.turns.clamp(1, MAX_TURNS);

    for turn in 1..=turns_allowed {
        if token.is_cancelled() {
            return finish("", "cancelled", turn - 1, tokens);
        }
        let sent = match wire::send_turn_configured(&conversation, &options.model, options.effort) {
            Ok(sent) => sent,
            Err(error) => return finish(&error.to_string(), "failed", turn, tokens),
        };
        if let Some(usage) = &sent.usage {
            tokens += usage.input_tokens + usage.output_tokens;
        }
        let text = message_text(&sent.message);
        if text.trim().is_empty() {
            return finish("the model returned an empty reply", "failed", turn, tokens);
        }
        conversation.messages.push(sent.message);

        let program = match prompt::extract_program(&text) {
            Extracted::Program(source) => source,
            // Prose from a subagent is its answer: it has no person to talk
            // to and no next instruction coming, so waiting for a program it
            // has already decided not to write would spend the budget on
            // silence.
            Extracted::Prose => return finish(&text, "returned", turn, tokens),
            Extracted::TwoBlocks => {
                conversation.messages.push(Message::text(
                    Role::User,
                    "two `pane` blocks arrived and neither ran; send exactly one",
                ));
                continue;
            }
        };

        let outcome = runtime.run_cell(&program);
        if let CellOutcome::Returned {
            value, terminal, ..
        } = &outcome
        {
            let answer = terminal.render(value);
            runtime.end_task();
            return finish(&answer, "returned", turn, tokens);
        }
        conversation
            .messages
            .push(Message::text(Role::User, &result_message(&outcome, turn)));
    }

    runtime.end_task();
    finish(
        "the subagent used every turn it was given without returning",
        "turns",
        turns_allowed,
        tokens,
    )
}

fn finish(answer: &str, status: &str, turns: u64, tokens: u64) -> AgentResult {
    AgentResult {
        answer: answer.to_string(),
        status: status.to_string(),
        turns,
        tokens,
    }
}

fn message_text(message: &Message) -> String {
    message
        .content
        .iter()
        .map(|block| match block {
            crate::contract::Block::Text(text) => text.as_str(),
        })
        .collect::<Vec<_>>()
        .join("")
}

/// The subagent's own result message, which is the parent's renderer with no
/// budget line: a subagent is bounded by its turn count, and a token figure it
/// cannot act on is prompt it pays for.
fn result_message(outcome: &CellOutcome, cell: u64) -> String {
    let turn = outcome.turn();
    let error = match outcome {
        CellOutcome::Threw { error, .. } => Some(ErrorSection {
            class: error.class.clone(),
            message: error.message.clone(),
            position: error
                .line
                .zip(error.column)
                .map(|(line, column)| (u64::from(line), u64::from(column))),
            frames: Vec::new(),
        }),
        _ => None,
    };
    prompt::render_result(&CellResult {
        cell,
        elapsed_ms: turn.elapsed_ms,
        error,
        yield_reason: turn.yield_reason.clone(),
        handle_table: turn.table.clone(),
        stdout_tail: (!turn.stdout_tail.is_empty()).then(|| turn.stdout_tail.clone()),
        budget: Budget {
            turn_cap: 0,
            task_used: 0,
            task_cap: 0,
            cells_used: cell,
            cells_cap: 0,
        },
        plan: turn.plan.clone(),
    })
}

/// What a subagent is told about itself, appended to the ordinary system
/// block. It is short on purpose: everything else it needs is the same
/// contract its parent works under.
const SUBAGENT_INSTRUCTIONS: &str = "You are a subagent. Another session asked you one question and is waiting \
for the answer; there is no person here to ask for more.\n\n\
Return the answer as a string with a top-level `return`, and return as soon as \
you have it — your turns are counted against the session that started you. You \
have no inbox, no messages, and you cannot start a subagent of your own. If the \
question cannot be answered with the grant you have, return that plainly \
instead of working around it.";
