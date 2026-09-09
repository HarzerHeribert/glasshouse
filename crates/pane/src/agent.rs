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
use crate::runtime::bindings::HostGlobals;
use crate::runtime::isolate::Runtime;
use crate::runtime::outcome::CellOutcome;
use crate::sandbox::profile::Profile;
use crate::tools::invoke::CancellationToken;
use crate::tools::registry;
use crate::wire::{self, Effort};

/// The most turns a subagent may take whatever it was asked for.
///
/// `agent.run({turns})` is written by the model, so a local turn cap keeps one
/// background worker finite even though the parent task's token spend is
/// uncapped.
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
    /// Turns actually taken, reported with the parent's cumulative spend.
    pub turns: u64,
    /// Provider-reported tokens, summed over the turns that reported any.
    pub tokens: u64,
    /// What the loop actually did, one entry per tool call in order.
    ///
    /// A helper reports numbers it claims to have computed. Without this the
    /// claim is unfalsifiable: the caller sees an answer and no trace of the
    /// work. Tool names only -- never an argument, never a payload, so this
    /// stays the same shape §9.4's trajectory already is.
    pub trajectory: Vec<String>,
}

/// How a subagent is asked for.
#[derive(Debug, Clone)]
pub struct AgentOptions {
    pub turns: u64,
    pub model: String,
    pub effort: Effort,
}

/// A nested loop narrowed from a subagent to a helper.
///
/// The invariant: **a helper holds only what its spec names, and is a leaf.**
/// `little-helpers.md` puts both in the runtime rather than in a helper's
/// prose, so this is what [`run_narrowed`] reads in place of `registry::ALL`
/// and in place of granting the project's `[helpers]` onward.
#[derive(Debug, Clone, Copy)]
pub struct Narrowed {
    /// Tool names, resolved against `registry::ALL`. `helpers::check_spec`
    /// has already refused an unregistered or mutating name at startup.
    pub tools: &'static [&'static str],
    /// The instructions this loop opens with, in place of
    /// [`SUBAGENT_INSTRUCTIONS`] — a helper's own `preamble`.
    pub instructions: &'static str,
}

/// Runs one subagent to its end, holding every registered tool. Blocking, and
/// called on `bg`'s own worker thread — never on the thread that holds the
/// parent isolate.
pub fn run(
    profile: &Profile,
    glasshouse: &Glasshouse,
    session: &SessionId,
    task: &str,
    options: &AgentOptions,
    token: &CancellationToken,
) -> AgentResult {
    run_narrowed(profile, glasshouse, session, task, options, token, None)
}

/// The same loop, optionally narrowed to one helper's toolset and preamble.
///
/// **The profile is the parent's, cloned and not recompiled.** A subagent that
/// compiled its own profile could differ from its parent's by a file edited
/// mid-session, which is a widening no one asked for; `sandbox-grants.md` §1.5
/// computes a profile once per session and this honours that across the nested
/// loop too.
pub fn run_narrowed(
    profile: &Profile,
    glasshouse: &Glasshouse,
    session: &SessionId,
    task: &str,
    options: &AgentOptions,
    token: &CancellationToken,
    narrowed: Option<&Narrowed>,
) -> AgentResult {
    let tools = toolset(narrowed);
    let facts = crate::session::session_facts(profile);
    let instructions = format!(
        "{}\n\n{}",
        narrowed.map_or(SUBAGENT_INSTRUCTIONS, |narrowed| narrowed.instructions),
        crate::project::instructions::root(profile)
    );
    // One value decides both what the context binds and what it is told it
    // binds: a helper never receives `bg`, `send` or `mcp`, because none of
    // the three is a tool and narrowing `spec.tools` therefore left every one
    // of them installed.
    let globals = match narrowed {
        Some(narrowed) => HostGlobals::Helper(narrowed.tools),
        None => HostGlobals::Every,
    };
    let mut system = prompt::render_system_for(&instructions, &tools, &facts, globals);
    system.push_str("\n\n");
    system.push_str(&crate::project::orientation::collect(profile));
    let mut conversation = Conversation {
        system,
        messages: vec![Message::text(Role::User, task)],
    };

    let mut runtime = match globals {
        HostGlobals::Helper(tools) => Runtime::for_helper(profile, glasshouse, session, tools),
        HostGlobals::Every => Runtime::new(profile, glasshouse, session),
    }
    .as_subagent()
    .with_token(token.clone())
    .with_instruction_context();
    if narrowed.is_none() {
        // A subagent completes a goal, so it hits the same walls the task
        // model does and gets the same helpers; a narrowed loop is itself a
        // helper and gets none, which is what makes it a leaf. It is handed a
        // profile and nothing else — `bg` calls this on its own thread — so it
        // reads `[helpers]` from the project itself; a file that will not
        // parse leaves helpers off, which is the same fail-closed answer as an
        // unset model.
        let helpers = crate::config::PaneConfig::load(profile.root())
            .unwrap_or_default()
            .helpers;
        runtime = runtime.with_helpers(helpers);
    }
    let mut tokens = 0u64;
    let mut trajectory: Vec<String> = Vec::new();
    let turns_allowed = options.turns.clamp(1, MAX_TURNS);

    for turn in 1..=turns_allowed {
        if token.is_cancelled() {
            return finish("", "cancelled", turn - 1, tokens, trajectory);
        }
        let mut request = conversation.clone();
        prompt::project_runtime_history(&mut request, 0);
        // A narrowed loop is a helper: its provider request can outlive the
        // caller that stopped waiting, so the wire request keeps a hard bound.
        let deadline = narrowed.map(|_| wire::SIDE_ERRAND_TIMEOUT);
        let purpose = narrowed.map(|_| crate::helpers::PURPOSE_HEADER);
        let sent = match wire::send_turn_bounded_with(
            &request,
            &options.model,
            options.effort,
            deadline,
            purpose,
        ) {
            Ok(sent) => sent,
            Err(error) => return finish(&error.to_string(), "failed", turn, tokens, trajectory),
        };
        // A provider response can race the caller's cancellation. Do not let
        // that late response start one of the helper's read tools.
        if token.is_cancelled() {
            return finish("", "cancelled", turn - 1, tokens, trajectory);
        }
        if let Some(usage) = &sent.usage {
            tokens = tokens.saturating_add(usage.total_tokens());
        }
        let text = message_text(&sent.message);
        let calls: Vec<_> = sent
            .message
            .content
            .iter()
            .filter_map(|block| match block {
                crate::contract::Block::ToolUse { id, name, input } => {
                    Some((id.clone(), name.clone(), input.clone()))
                }
                _ => None,
            })
            .collect();
        if text.trim().is_empty() && calls.is_empty() {
            return finish(
                "the model returned an empty reply",
                "failed",
                turn,
                tokens,
                trajectory,
            );
        }
        conversation.messages.push(sent.message);

        let native = calls.first().cloned();
        if calls.len() > 1 || native.as_ref().is_some_and(|call| call.1 != "execute_cell") {
            let explanation = if calls.len() > 1 {
                "ProtocolError: exactly one execute_cell call is allowed; nothing ran."
            } else {
                "ProtocolError: unknown tool call; nothing ran."
            };
            conversation.messages.push(Message {
                role: Role::User,
                content: calls
                    .iter()
                    .map(|(id, _, _)| crate::contract::Block::ToolResult {
                        tool_use_id: id.clone(),
                        content: explanation.into(),
                        is_error: true,
                    })
                    .collect(),
                historical: None,
            });
            continue;
        }
        let program = if let Some((id, _, input)) = &native {
            match input
                .as_object()
                .filter(|object| object.len() == 1)
                .and_then(|object| object.get("code"))
                .and_then(serde_json::Value::as_str)
            {
                Some(code) => code.to_string(),
                None => {
                    conversation.messages.push(Message::tool_result(
                        id.clone(),
                        "ProtocolError: execute_cell input must be exactly {\"code\": string}; nothing ran.",
                        true,
                    ));
                    continue;
                }
            }
        } else {
            match prompt::extract_program(&text) {
                Extracted::Program(source) => source,
                Extracted::Edit(json) => {
                    match runtime
                        .syntax_failure()
                        .ok_or_else(|| {
                            "No syntax-failed cell is available in this task.".to_string()
                        })
                        .and_then(|failed| failed.apply(&json))
                    {
                        Ok(source) => source,
                        Err(error) => {
                            let hint = runtime
                                .syntax_failure()
                                .map(|failed| failed.hint())
                                .unwrap_or_default();
                            conversation.messages.push(Message::text(
                                Role::User,
                                format!("CellEditError: {error} Nothing ran.\n{hint}"),
                            ));
                            continue;
                        }
                    }
                }
                Extracted::Prose => {
                    if let Some(answer) = prompt::completion_text(&text) {
                        runtime.end_task();
                        return finish(&answer, "returned", turn, tokens, trajectory);
                    }
                    conversation
                        .messages
                        .push(Message::text(Role::User, prompt::CONTINUE_WORK));
                    continue;
                }
                Extracted::Invalid(error) => {
                    conversation.messages.push(Message::text(
                        Role::User,
                        format!("ProtocolError: {error} Nothing ran."),
                    ));
                    continue;
                }
                Extracted::TwoBlocks => {
                    conversation.messages.push(Message::text(Role::User,
                    "Mixed or multiple pane-edit blocks are ambiguous. Send one repair or ordinary Pane code; nothing ran."));
                    continue;
                }
            }
        };

        let outcome = runtime.run_cell(&program);
        // What this turn actually reached for, in order. Tool names only.
        trajectory.extend(
            outcome
                .turn()
                .record
                .calls
                .iter()
                .map(|call| call.tool.clone()),
        );
        let instruction_boundary = runtime.pending_instructions();
        if let Some(pending) = &instruction_boundary {
            conversation.system.push_str("\n\n");
            conversation.system.push_str(&pending.text);
        }
        if let CellOutcome::Returned {
            value, terminal, ..
        } = &outcome
            && outcome.ends_the_task()
        {
            let answer = terminal.render(value);
            if let Some((id, _, _)) = &native {
                let result = result_message(&outcome, turn);
                let mut feedback = prompt::render_result(&result);
                feedback.push_str("\n\n## Return\n");
                feedback.push_str(&answer);
                conversation
                    .messages
                    .push(Message::tool_result(id.clone(), feedback, false));
            }
            runtime.end_task();
            return finish(&answer, "returned", turn, tokens, trajectory);
        }
        let result = result_message(&outcome, turn);
        let mut full = prompt::render_result(&result);
        let mut historical = prompt::render_result_history(&result);
        if let Some(failed) = runtime.syntax_failure() {
            for text in [&mut full, &mut historical] {
                text.push_str("\n\n");
                text.push_str(&failed.hint());
            }
        }
        if let Some((id, _, _)) = &native {
            let message = Message::runtime_tool_result(
                id.clone(),
                full,
                matches!(outcome, CellOutcome::Threw { .. }),
                historical,
            );
            conversation.messages.push(message);
        } else {
            conversation
                .messages
                .push(Message::runtime(full, historical));
        }
        if let Some(pending) = instruction_boundary {
            if pending.fatal {
                runtime.end_task();
                return finish(&pending.text, "failed", turn, tokens, trajectory);
            }
            runtime.acknowledge_instructions();
        }
    }

    runtime.end_task();
    finish(
        "the subagent used every turn it was given without returning",
        "turns",
        turns_allowed,
        tokens,
        trajectory,
    )
}

/// The tools this loop is declared, by name.
///
/// The invariant: **a narrowed loop is declared only what its spec names.** A
/// name that resolves to nothing is dropped rather than substituted, and
/// `helpers::check_spec` has already refused such a name at startup.
fn toolset(narrowed: Option<&Narrowed>) -> Vec<&'static registry::Tool> {
    match narrowed {
        None => registry::ALL.iter().collect(),
        Some(narrowed) => narrowed
            .tools
            .iter()
            .filter_map(|name| registry::lookup(name))
            .collect(),
    }
}

fn finish(
    answer: &str,
    status: &str,
    turns: u64,
    tokens: u64,
    trajectory: Vec<String>,
) -> AgentResult {
    AgentResult {
        answer: answer.to_string(),
        status: status.to_string(),
        turns,
        tokens,
        trajectory,
    }
}

fn message_text(message: &Message) -> String {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            crate::contract::Block::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

/// The subagent's own result message, which is the parent's renderer with no
/// usage line: a subagent is bounded by its turn count, and a token figure it
/// cannot act on is prompt it pays for.
fn result_message(outcome: &CellOutcome, cell: u64) -> CellResult {
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
    CellResult {
        cell,
        elapsed_ms: turn.elapsed_ms,
        error,
        yield_reason: turn.yield_reason.clone(),
        output: match outcome {
            CellOutcome::Returned {
                value, terminal, ..
            } if !outcome.ends_the_task() => Some(terminal.render(value)),
            _ => None,
        },
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
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `little-helpers.md`'s first build cost: the tool list comes from the
    /// caller instead of `registry::ALL`. The mutating tools are the point --
    /// a narrowed loop is never declared them, so "a helper never writes" is
    /// a list it does not have rather than a rule it might disregard.
    #[test]
    fn a_narrowed_loop_is_declared_only_the_tools_it_named() {
        let every: Vec<&str> = toolset(None).iter().map(|tool| tool.name()).collect();
        assert_eq!(
            every.len(),
            registry::ALL.len(),
            "a subagent still holds every registered tool: {every:?}"
        );

        let scout = Narrowed {
            tools: &["read", "grep"],
            instructions: "",
        };
        let named: Vec<&str> = toolset(Some(&scout))
            .iter()
            .map(|tool| tool.name())
            .collect();
        assert_eq!(named, ["read", "grep"], "in the order the spec named them");
        for forbidden in crate::helpers::FORBIDDEN_TOOLS {
            assert!(
                !named.contains(&forbidden),
                "a narrowed loop must not be declared `{forbidden}`"
            );
        }
    }
}
