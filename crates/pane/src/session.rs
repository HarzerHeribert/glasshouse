//! `pane session`: the run that wires the six merged 61C modules together.
//! Each of them is correct and tested in isolation; this module is the only
//! place any of them is called from `main` rather than from its own tests --
//! see the packet's OBJECTIVE for why that gap, not missing code, is what
//! this module exists to close.

use std::fs;
use std::io::{self, BufRead, IsTerminal};
use std::path::PathBuf;
use std::time::SystemTime;

use clap::Parser;
use ratatui::Terminal;
use ratatui::backend::{CrosstermBackend, TestBackend};

use crate::commands::{self, CommandStatus};
use crate::contract::{Block, Conversation, Message, ProjectConfig, Role, ServedBy, SessionId};
use crate::glasshouse::{self, Glasshouse, LifecycleEvent, LocalMemory};
use crate::project;
use crate::prompt::{self, Budget, CellResult, ErrorSection, ExhaustedReason, Extracted};
use crate::rollout::{self, Rollout};
use crate::runtime::handles::HandleTable;
use crate::runtime::isolate::Runtime;
use crate::runtime::outcome::CellOutcome;
use crate::runtime::preview;
use crate::sandbox::profile::Profile;
use crate::tools::invoke::{self, Args, ToolContext, ToolError};
use crate::tools::registry;
use crate::tui::{self, CellError, CellView, Counted, Notebook, TaskTokens};
use crate::wire;

/// `model-contract.md` §6's task defaults. Constants until `pane.toml`
/// supplies them -- 61F owns the setting, and a figure the model is told is
/// worth nothing if it is invented twice, so they are spelled once here. The
/// turn cap is not spelled at all: the budget line reads [`wire::MAX_TOKENS`],
/// the figure actually sent, so the model is told the cap that binds it.
const TASK_TOKEN_CAP: u64 = 400_000;
const CELL_CAP: u64 = 40;

/// How many prose turns in a row end the task (the primary's addendum of
/// 2026-09-06): on this one, the answer carries the exhausted preamble and
/// the loop ends after one more turn whatever the model does. A program or
/// two blocks resets the count; the token budget stays the outer stop.
const PROSE_TURN_CAP: u32 = 3;

/// §5's answer to a message that carried no program.
const NO_PROGRAM: &str = "no program ran; send one pane block";

/// §5's answer to a message that carried two. **Neither runs**, and the
/// sentence is the contract's own: running the first is the silently-wrong
/// reading, because the second is usually the one the model meant.
const TWO_BLOCKS: &str = "two pane blocks in one turn; send one";

/// `pane session`'s whole flag set. A project root and a way to identify the
/// rollout file are the only things every run needs; `--task` is the
/// non-interactive, scriptable entry point this package's own acceptance
/// tests drive (`env!("CARGO_BIN_EXE_pane")` subprocesses can pipe a task in
/// as an argument far more simply than as timed stdin), and the same flag is
/// what the ruler's future `pane` harness row will pass a statement through.
/// Absent `--task`, `session` reads one input per line from stdin until EOF,
/// so a person can drive it as a REPL.
#[derive(Parser, Debug)]
#[command(name = "pane session")]
pub struct SessionArgs {
    /// The project root map line 2448 loads from.
    #[arg(long)]
    pub root: PathBuf,

    /// One scripted user input (a slash command or a task) run once, non-
    /// interactively. Omitted means read stdin, one input per line, until
    /// EOF.
    #[arg(long)]
    pub task: Option<String>,

    /// Where turns are appended and, on a later run, resumed from. Defaults
    /// to `<root>/.pane/rollout.jsonl` so two runs against the same project
    /// root resume each other without any extra flag.
    #[arg(long)]
    pub rollout: Option<PathBuf>,

    /// This session's id: the value every `glasshouse hook --session`
    /// invocation carries. Defaults to a value derived from the process, since
    /// nothing in map lines 2444-2451 requires it to be stable across runs
    /// absent an explicit choice.
    #[arg(long)]
    pub session: Option<String>,

    /// The `glasshouse` executable pane's three seams shell out to. Bare
    /// `"glasshouse"`, absent this flag, resolves through `PATH` exactly as
    /// `glasshouse.rs`'s own doc comment describes; a test overrides it with
    /// its own fake script so no test performs a real `PATH` lookup.
    #[arg(long)]
    pub glasshouse: Option<PathBuf>,
}

/// Parses `args` (everything after `pane session`) and runs it.
pub fn dispatch(args: &[String]) -> Result<(), String> {
    let parsed = SessionArgs::try_parse_from(
        std::iter::once("pane session".to_string()).chain(args.iter().cloned()),
    )
    .map_err(|e| e.to_string())?;
    run(parsed)
}

fn default_session_id() -> String {
    format!("pane-{}", std::process::id())
}

fn default_rollout_path(root: &std::path::Path) -> PathBuf {
    root.join(".pane").join("rollout.jsonl")
}

/// The system block, and it is [`prompt::render_system`]'s bytes and nothing
/// else -- `model-contract.md` §1: the preamble, one declaration per
/// registered tool, then the project's own instructions.
///
/// **The joining of the instruction documents is all this function decides.**
/// Map line 2448 fixes what is loaded, not how it is joined; everything from
/// the preamble outwards is `prompt`'s, whose own golden test pins it byte for
/// byte, so there is no second spelling of the contract here to drift from it.
fn build_system_prompt(project: &ProjectConfig) -> String {
    let instructions = project
        .instructions
        .iter()
        .map(|(_, text)| text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    prompt::render_system(&instructions, &registry::ALL.iter().collect::<Vec<_>>())
}

fn message_text(message: &Message) -> String {
    message
        .content
        .iter()
        .map(Block::text)
        .collect::<Vec<_>>()
        .join("")
}

/// The conversation and, beside it, everything the notebook column knows
/// about it that the messages themselves do not say: which cell threw, which
/// returned, what its handle table looked like as it ended.
///
/// **They travel together because they are indexed together.** A cell's view
/// is found by the same ordinal the screen numbers the cell with, so a
/// conversation that grew without its notebook -- a resumed session, whose
/// cells came back from the rollout file -- would hang every later view under
/// the wrong cell.
struct Transcript {
    conversation: Conversation,
    notebook: Notebook,
}

/// **Nothing in the notebook is a live object.** The runtime hands out a
/// rendered handle table and a rendered preview and never its table or its
/// value, so `tui` receives strings; the empty [`HandleTable`] below is the
/// argument for a caller that holds one, which the session never does.
fn empty_handles() -> HandleTable {
    HandleTable::new()
}

/// Draws the two-region screen where a user or a test can actually see it.
/// A live interactive terminal (raw mode, an alternate screen, a
/// resize-aware redraw loop) is out of scope for this package -- see the
/// report's limits -- so this draws one frame per cell either way, through
/// the same unmodified `tui::render`: to a real `CrosstermBackend` when
/// stdout is a tty, and to stdout as plain lines otherwise, so a pipe never
/// makes the session's output disappear the way it used to.
fn render(transcript: &Transcript, served_by: &ServedBy) {
    if io::stdout().is_terminal() {
        render_to_terminal(transcript, served_by);
    } else {
        render_as_lines(transcript, served_by);
    }
}

fn render_to_terminal(transcript: &Transcript, served_by: &ServedBy) {
    let Ok(mut terminal) = Terminal::new(CrosstermBackend::new(io::stdout())) else {
        return;
    };
    let _ = terminal.draw(|frame| {
        tui::render(
            frame,
            &transcript.conversation,
            served_by,
            &empty_handles(),
            &transcript.notebook,
        )
    });
}

/// Every acceptance test below, and any real pipe, takes this path. Draws
/// through the identical `tui::render` a live terminal uses, into an
/// in-memory buffer exactly as `tui.rs`'s own tests do, then prints each
/// non-blank row as a line of text -- so the conversation column and the
/// sidebar's content (including its honest "not connected" collapse) reach
/// stdout rather than a dropped `TestBackend`.
///
/// **The buffer is sized to the notebook rather than fixed.** A pipe has no
/// scrollback, so a height chosen once would silently drop the newest cell
/// exactly when a task had run long enough to be worth reading; the doubling
/// is the room a wrapped table line takes.
fn render_as_lines(transcript: &Transcript, served_by: &ServedBy) {
    let handles = empty_handles();
    let rows = tui::notebook_height(&transcript.conversation, &handles, &transcript.notebook);
    let height = (rows * 2 + 8).clamp(40, 2_000) as u16;
    let backend = TestBackend::new(100, height);
    let mut terminal = Terminal::new(backend).expect("an in-memory backend never fails to init");
    let _ = terminal.draw(|frame| {
        tui::render(
            frame,
            &transcript.conversation,
            served_by,
            &handles,
            &transcript.notebook,
        )
    });
    let buffer = terminal.backend().buffer();
    for y in 0..buffer.area.height {
        let mut line = String::new();
        for x in 0..buffer.area.width {
            if let Some(cell) = buffer.cell((x, y)) {
                line.push_str(cell.symbol());
            }
        }
        let line = line.trim_end();
        if !line.is_empty() {
            println!("{line}");
        }
    }
}

/// Runs `session`, in the order the packet's OBJECTIVE fixes: load the
/// project, resume or start the rollout, `SessionStart`, then one input (or
/// stdin's, one per line) at a time until the input source is exhausted.
fn run(args: SessionArgs) -> Result<(), String> {
    let project = project::load(&args.root);

    // `sandbox-grants.md` §1.5: computed once, at session start, immutable
    // for the session's life. `.claude/` lives inside the writable project
    // root, so a profile recomputed mid-session would let a program widen
    // its own sandbox by editing the file it was derived from.
    let profile = compile_profile_once(&project);

    let rollout_path = args
        .rollout
        .clone()
        .unwrap_or_else(|| default_rollout_path(&args.root));
    if let Some(parent) = rollout_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
    }

    let session_id = SessionId::new(args.session.clone().unwrap_or_else(default_session_id));
    let glasshouse = match &args.glasshouse {
        Some(path) => Glasshouse::Command {
            glasshouse: path.clone(),
        },
        None => Glasshouse::Command {
            glasshouse: PathBuf::from("glasshouse"),
        },
    };

    let resuming = rollout_path.exists();
    let conversation = if resuming {
        rollout::resume(&rollout_path)
            .map_err(|e| format!("could not resume {}: {e}", rollout_path.display()))?
    } else {
        Conversation {
            system: build_system_prompt(&project),
            messages: Vec::new(),
        }
    };

    let mut rollout = Rollout::create(&rollout_path, session_id.clone(), &conversation.system)
        .map_err(|e| format!("could not open {}: {e}", rollout_path.display()))?;

    // A resumed conversation's cells are not replayed (`runtime-contract.md`
    // §4), so the notebook starts empty and pads: an earlier cell renders
    // with no view of its own rather than with the next task's.
    let mut transcript = Transcript {
        conversation,
        notebook: Notebook::default(),
    };

    glasshouse::emit_lifecycle(&glasshouse, &session_id, LifecycleEvent::SessionStart);

    // The local store lives beside the rollout, so a project's notes travel
    // with the session that made them. `glasshouse.rs` owns the fallback
    // decision; this only says where the fallback's file goes.
    let memory = LocalMemory::new(
        rollout_path
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| args.root.clone()),
    );

    let session = Session {
        project: &project,
        profile: &profile,
        glasshouse: &glasshouse,
        id: &session_id,
        memory: &memory,
        token: invoke::CancellationToken::new(),
    };
    let outcome = drive(&args, &session, &mut transcript, &mut rollout);

    glasshouse::emit_lifecycle(
        &glasshouse,
        &session_id,
        match &outcome {
            Ok(()) => LifecycleEvent::Stop,
            Err(_) => LifecycleEvent::StopFailure,
        },
    );

    outcome
}

/// Everything one session holds for its whole life, gathered so a per-input
/// function takes the session rather than five of its parts.
///
/// **`profile` is a borrow, and that is `sandbox-grants.md` §1.5.** The one
/// `Profile` `run` compiled is the only one any input can be answered
/// against; there is no owned field here that a later call could replace.
struct Session<'a> {
    project: &'a ProjectConfig,
    /// The token every tool call in this session is cancellable through
    /// (`Runtime::with_token`). Nothing sets it yet: the `SIGINT` handler that
    /// does is the follow-up after the isolate fix, so today it is plumbing
    /// that makes a cancelled call a §5 `Cancelled` throw the moment it exists.
    token: invoke::CancellationToken,
    profile: &'a Profile,
    glasshouse: &'a Glasshouse,
    id: &'a SessionId,
    memory: &'a LocalMemory,
}

/// Runs `args.task` once if given, else reads stdin one line at a time until
/// EOF -- the REPL half of the same input source, so a slash command or a
/// task is handled identically regardless of where it came from.
fn drive(
    args: &SessionArgs,
    session: &Session<'_>,
    transcript: &mut Transcript,
    rollout: &mut Rollout,
) -> Result<(), String> {
    if let Some(task) = &args.task {
        return process_input(task, session, transcript, rollout);
    }

    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let line = line.map_err(|e| e.to_string())?;
        process_input(&line, session, transcript, rollout)?;
    }
    Ok(())
}

/// One input: a slash command answered locally, or a **task** run to its end.
/// A slash command -- resolved or not -- never reaches [`wire::send_turn`];
/// only text that is not a slash command does.
///
/// **A slash command is answered between tasks, never inside one.** The one
/// [`Runtime`] a task owns lives inside [`run_task`] and is dropped when the
/// task ends, so there is no code path on which a command could reach it.
fn process_input(
    input: &str,
    session: &Session<'_>,
    transcript: &mut Transcript,
    rollout: &mut Rollout,
) -> Result<(), String> {
    if let Some(rest) = input.strip_prefix('/') {
        let (name, argument) = split_command(rest);
        answer_command(rest, name, argument, session);
        render(transcript, &ServedBy::default());
        return Ok(());
    }
    run_task(input, session, transcript, rollout)
}

/// The task's token total and the cells it has spent, and where each turn's
/// figure came from -- `model-contract.md` §6's budget line.
struct TaskBudget {
    used: u64,
    cells_used: u64,
    reported: bool,
    estimated: bool,
}

impl TaskBudget {
    fn new() -> Self {
        Self {
            used: 0,
            cells_used: 0,
            reported: false,
            estimated: false,
        }
    }

    /// Adds one turn's cost: the gateway's own usage row when it reported
    /// one, else the Messages response's own `usage`, else `estimate`.
    ///
    /// **Which source was used is recorded, not averaged.** §6 reads a
    /// provider's figure "rather than estimated", and a total that quietly
    /// mixed a measurement with a heuristic would be a number the sidebar
    /// could not honestly label. The gateway's row is preferred over the
    /// response's own `usage` when both are present, because it is what
    /// `served_by` was built to make authoritative -- but the sidebar calls
    /// either one `reported`: a reader deciding whether to trust this figure
    /// only needs to know it did not come from `estimate_tokens`.
    fn add(&mut self, served: &ServedBy, usage: Option<&wire::Usage>, estimate: u64) {
        match (served.input_tokens, served.output_tokens) {
            (None, None) => match usage {
                Some(usage) => {
                    self.used = self
                        .used
                        .saturating_add(usage.input_tokens)
                        .saturating_add(usage.output_tokens);
                    self.reported = true;
                }
                None => {
                    self.used = self.used.saturating_add(estimate);
                    self.estimated = true;
                }
            },
            (input, output) => {
                self.used = self
                    .used
                    .saturating_add(input.unwrap_or(0))
                    .saturating_add(output.unwrap_or(0));
                self.reported = true;
            }
        }
    }

    fn counted(&self) -> Option<Counted> {
        match (self.reported, self.estimated) {
            (true, true) => Some(Counted::Mixed),
            (true, false) => Some(Counted::Gateway),
            (false, true) => Some(Counted::Estimated),
            (false, false) => None,
        }
    }

    /// §6's own line, for the result block the model reads next.
    fn line(&self) -> Budget {
        Budget {
            turn_cap: u64::from(wire::MAX_TOKENS),
            task_used: self.used,
            task_cap: TASK_TOKEN_CAP,
            cells_used: self.cells_used,
            cells_cap: CELL_CAP,
        }
    }

    fn tokens(&self) -> Option<TaskTokens> {
        Some(TaskTokens {
            used: self.used,
            cap: TASK_TOKEN_CAP,
            counted: self.counted()?,
        })
    }

    /// Whether this task may still ask for another turn after the one being
    /// answered -- §6's cap on cells and its task budget, either of which
    /// buys exactly one more turn under [`prompt::exhausted_preamble`].
    fn spent(&self) -> bool {
        self.used >= TASK_TOKEN_CAP || self.cells_used >= CELL_CAP
    }
}

/// What one assistant message asked the session to do.
struct Step {
    /// The next user message, or `None` when the task is over: a top-level
    /// `return` is answered with nothing at all, because nothing further is
    /// asked of the model (`runtime-contract.md` §1).
    answer: Option<String>,
    /// The task's terminal response (`runtime-contract.md` §9.2): rendered
    /// and kept as the assistant's own turn, with no request after it.
    response: Option<String>,
    /// Whether the message carried no program (§5's prose), counted by
    /// [`run_task`] against [`PROSE_TURN_CAP`].
    prose: bool,
    view: CellView,
}

/// Runs one task to its end: every turn's program goes to this task's own
/// isolate and every outcome comes back as the next user message, with no
/// person in the loop, until a top-level `return`, the cell cap or the task
/// budget ends it.
///
/// **One [`Runtime`] per task, built from the session's one compiled
/// [`Profile`].** `sandbox-grants.md` §1.5 is that the profile is computed
/// once at session start; this borrows it and compiles nothing, so a second
/// task cannot widen the first's grants and a program cannot widen its own.
fn run_task(
    task: &str,
    session: &Session<'_>,
    transcript: &mut Transcript,
    rollout: &mut Rollout,
) -> Result<(), String> {
    glasshouse::emit_lifecycle(
        session.glasshouse,
        session.id,
        LifecycleEvent::UserPromptSubmit,
    );

    transcript
        .conversation
        .messages
        .push(Message::text(Role::User, task));
    rollout
        .record_turn(Role::User, task)
        .map_err(|e| format!("could not record the user turn: {e}"))?;

    let mut runtime = Runtime::new(session.profile, session.glasshouse, session.id)
        .with_token(session.token.clone());
    let mut budget = TaskBudget::new();
    let mut final_turn = false;
    let mut prose_turns = 0u32;

    loop {
        let since = SystemTime::now();
        let turn = wire::send_turn(&transcript.conversation)
            .map_err(|e| format!("request failed: {e}"))?;
        let estimate = estimate_request_tokens(&transcript.conversation);
        let assistant_text = message_text(&turn.message);
        transcript.conversation.messages.push(turn.message);
        rollout
            .record_turn(Role::Assistant, &assistant_text)
            .map_err(|e| format!("could not record the assistant turn: {e}"))?;

        let served = glasshouse::served_by(session.glasshouse, since);
        budget.add(&served, turn.usage.as_ref(), estimate);

        let ordinal = tui::cell_ordinal(&transcript.conversation, &transcript.notebook);
        let mut step = act_on(&assistant_text, &mut runtime, &mut budget, rollout)?;
        prose_turns = if step.prose { prose_turns + 1 } else { 0 };

        // §9.2: the terminal response is the assistant's own turn -- the same
        // line an assistant message has always written, so `resume` rebuilds
        // it with no new reader -- and it is written after the cell line
        // `act_on` already appended, so the rollout ends cell, then reply.
        if let Some(response) = &step.response {
            transcript
                .conversation
                .messages
                .push(Message::text(Role::Assistant, response));
            rollout
                .record_turn(Role::Assistant, response)
                .map_err(|e| format!("could not record the terminal response: {e}"))?;
        }

        // The flag is read before this turn decorates it, so the turn that
        // carries the exhausted preamble is sent, answered and only then
        // ends the task -- §6's "the only permitted action is a top-level
        // `return`" needs that turn to actually happen.
        let stop = step.answer.is_none() || final_turn;
        let exhausted = if budget.spent() {
            Some(ExhaustedReason::TaskBudget)
        } else if prose_turns >= PROSE_TURN_CAP {
            Some(ExhaustedReason::ThreeTurnsWithoutAProgram)
        } else {
            None
        };
        if !stop && let Some(reason) = exhausted {
            step.answer = step
                .answer
                .map(|answer| format!("{}\n\n{answer}", prompt::exhausted_preamble(reason)));
            final_turn = true;
        }

        step.view.answered = step.answer.is_some();
        transcript.notebook.tokens = budget.tokens();
        transcript.notebook.set(ordinal, step.view);

        if let Some(answer) = &step.answer {
            transcript
                .conversation
                .messages
                .push(Message::text(Role::User, answer));
            rollout
                .record_turn(Role::User, answer)
                .map_err(|e| format!("could not record the runtime's answer: {e}"))?;
        }

        render(transcript, &served);

        if stop {
            break;
        }
    }

    runtime.end_task();
    Ok(())
}

/// `model-contract.md` §5 applied to one assistant message: one `pane` block
/// is a program and runs, two run neither, and anything else is prose.
///
/// **Nothing in `assistant_text` reaches a shell.** The one thing extracted
/// from it is a program, and the only thing that ever receives a program is
/// [`Runtime::run_cell`]; every tool that program calls goes through
/// `tools::invoke` and the session's sandbox from inside the isolate.
fn act_on(
    assistant_text: &str,
    runtime: &mut Runtime,
    budget: &mut TaskBudget,
    rollout: &mut Rollout,
) -> Result<Step, String> {
    let source = match prompt::extract_program(assistant_text) {
        Extracted::Program(source) => source,
        // §5: the task does not advance and the cell counter does not move.
        // The screen still shows the table, because it is still what the
        // isolate holds -- an output region saying `(no outputs)` beside live
        // handles would be the screen disagreeing with the message sent in
        // the same breath.
        Extracted::Prose => {
            let table = runtime.render_handles();
            return Ok(Step {
                answer: Some(unchanged_table(&table)),
                response: None,
                prose: true,
                view: CellView {
                    table: Some(table),
                    ..CellView::default()
                },
            });
        }
        Extracted::TwoBlocks => {
            return Ok(Step {
                answer: Some(TWO_BLOCKS.to_string()),
                response: None,
                prose: false,
                view: CellView {
                    table: Some(runtime.render_handles()),
                    ..CellView::default()
                },
            });
        }
    };

    let outcome = runtime.run_cell(&source);
    budget.cells_used = budget.cells_used.saturating_add(1);
    let turn = outcome.turn();
    rollout
        .record_cell(&turn.record)
        .map_err(|e| format!("could not record the cell: {e}"))?;

    let mut view = CellView {
        table: Some(turn.table.clone()),
        ..CellView::default()
    };
    let mut result = CellResult {
        cell: turn.record.cell,
        elapsed_ms: turn.elapsed_ms,
        error: None,
        yield_reason: None,
        handle_table: turn.table.clone(),
        stdout_tail: (!turn.stdout_tail.is_empty()).then(|| turn.stdout_tail.clone()),
        budget: budget.line(),
    };

    let mut response = None;
    match &outcome {
        // Nothing that threw, was refused or was cancelled reaches this arm
        // -- each of those is a `Threw`, and a throw is answered. §9.2: what
        // is rendered is the terminal response -- a string verbatim, any
        // other value as its JSON -- never `marshal`'s sample.
        CellOutcome::Returned {
            value, terminal, ..
        } => {
            let text = terminal.render(value);
            view.returned = Some(text.clone());
            response = Some(text);
        }
        CellOutcome::Threw { error, .. } => {
            view.error = Some(CellError {
                class: error.class.clone(),
                message: error.message.clone(),
                line: error.line,
                column: error.column,
            });
            result.error = Some(ErrorSection {
                class: error.class.clone(),
                message: error.message.clone(),
                position: error
                    .line
                    .zip(error.column)
                    .map(|(line, column)| (u64::from(line), u64::from(column))),
                frames: error
                    .stack
                    .iter()
                    .map(|frame| frame.description.clone())
                    .collect(),
            });
        }
        // §9.3: a yield on purpose says why, under the cell line and beside
        // the table on the screen -- never in the error region.
        CellOutcome::Yielded { turn } => {
            view.yield_reason = turn.yield_reason.clone();
            result.yield_reason = turn.yield_reason.clone();
        }
    }
    // §1: the task ends with a `return` and nothing further is asked of the
    // model; a yield and a throw are answered. The outcome's own predicate
    // decides, so there is no second reading of §1 here to drift from it.
    let answer = (!outcome.ends_the_task()).then(|| prompt::render_result(&result));

    Ok(Step {
        answer,
        response,
        prose: false,
        view,
    })
}

/// §5's answer to a message that carried no program: the handle table
/// unchanged, and one line saying so. `(none)` rather than an empty section,
/// the same rule [`prompt::render_result`] keeps for the same table.
fn unchanged_table(table: &str) -> String {
    let shown = if table.is_empty() { "(none)" } else { table };
    format!("## Handles\n{shown}\n\n{NO_PROGRAM}")
}

/// The turn's cost when the gateway reported none: `estimate_tokens` over the
/// bytes actually sent, which is what makes it comparable turn to turn.
///
/// It counts the request and not the reply, so it is a floor rather than a
/// total -- the sidebar says `estimated` for exactly this reason.
fn estimate_request_tokens(conversation: &Conversation) -> u64 {
    let body = wire::request_body(conversation);
    preview::estimate_tokens(&String::from_utf8_lossy(&body)) as u64
}

/// Splits a slash command's name from whatever follows it -- `/memory a
/// note` is a name and an argument, `/model` is a name and nothing. Empty
/// input (a bare `/`) yields an empty name, which [`answer_command`] treats
/// the same as `/help`.
fn split_command(rest: &str) -> (&str, Option<&str>) {
    match rest.split_once(char::is_whitespace) {
        Some((name, argument)) => (name, Some(argument.trim())),
        None => (rest, None),
    }
}

/// Answers a slash command. `commands::resolve` only ever *decides* what a
/// command is; acting on one is this function's job. `/memory` is the only
/// built-in with an action beyond naming itself, and a bare `/` or `/help`
/// is this package's chosen way to reach map line 2450's other half: the
/// full list `commands::all` has decided since it was written, and that
/// nothing before this package ever printed.
///
/// **`/memory` is where map line 2446 reaches the binary.** The seam and its
/// local fallback were built and tested by `GH-PANE-61C-SEAMS`, and then
/// nothing called them: `commands` was scoped to decide and never to act, and
/// no package was given the acting half. A capability nothing invokes is not
/// a capability, whatever its tests say.
fn answer_command(rest: &str, name: &str, argument: Option<&str>, session: &Session<'_>) {
    // A bare `/` or `/help` lists rather than resolves, so `commands::all`
    // has a caller and the binary actually *offers* what 2450 names.
    if name.is_empty() || name == "help" {
        offer_commands(session.project);
        return;
    }

    // `/tool` is answered before `commands::resolve` is consulted, because
    // it is not a project command: it carries its own arguments on the same
    // line, and a resolver keyed on a bare name would look up
    // `tool read path=…` and answer "unknown".
    // `tool_invocation` reads the **whole** line, not the split-off name:
    // `/tool read path=…` carries its arguments after the command word, and
    // a resolver keyed on the bare name would look up `tool` and lose them.
    if let Some(call) = tool_invocation(rest) {
        answer_tool(call, session);
        return;
    }
    match commands::resolve(session.project, name) {
        Some(resolved) => match resolved.status {
            CommandStatus::Available => {
                if name == "memory" {
                    answer_memory(session.glasshouse, session.memory, argument);
                } else {
                    println!("/{name} ({:?})", resolved.source);
                }
            }
            CommandStatus::NotBuilt { subphase } => {
                println!("/{name} is not built yet -- that is sub-phase {subphase}");
            }
        },
        None => println!("/{name}: unknown command"),
    }
}

/// Prints every command `commands::all` names, in its own order -- the
/// built-ins, then the project's own commands and skills. `all` had a
/// production caller nowhere before this package; this is that caller.
fn offer_commands(project: &ProjectConfig) {
    for resolved in commands::all(project) {
        println!("/{} ({:?})", resolved.name, resolved.source);
    }
}

/// Reads memory and the latest checkpoint through Glasshouse's MCP surface,
/// falling back to the local store when nothing answers — map line 2446. A
/// non-empty `argument` is a note to save instead: `/memory <text>` is this
/// package's chosen writer, and it always lands in the local store, the only
/// store `pane` itself owns -- Glasshouse's own memory tool is written to by
/// Glasshouse's own harness, not by a second writer invented here.
///
/// The readers degrade rather than fail, so a read prints what it found and
/// says plainly when that was nothing; it never reports an error and never
/// distinguishes "Glasshouse is absent" from "Glasshouse had nothing", which
/// is `glasshouse.rs`'s own contract and not this function's to re-decide.
fn answer_memory(glasshouse: &Glasshouse, memory: &LocalMemory, argument: Option<&str>) {
    if let Some(text) = argument.filter(|text| !text.is_empty()) {
        match memory.add(text) {
            Ok(()) => println!("/memory: saved"),
            Err(e) => println!("/memory: could not save: {e}"),
        }
        return;
    }

    let notes = glasshouse::search_memory(glasshouse, memory, "");
    if notes.is_empty() {
        println!("/memory: no notes");
    } else {
        for note in &notes {
            println!("/memory: {note}");
        }
    }
    match glasshouse::checkpoint(glasshouse, memory) {
        Some(checkpoint) => println!("/memory checkpoint: {checkpoint}"),
        None => println!("/memory checkpoint: none"),
    }
}

/// The one place a session compiles its [`Profile`], and the one place that
/// prints the sandbox notice.
///
/// **The notice is the observation, not a courtesy.** `sandbox-grants.md`
/// §1.5's "computed once, at session start" is otherwise a property of the
/// call graph that no test can see; because this is the only expression in
/// `pane session` that produces a `Profile` and it prints as it does so, a
/// second compilation would print a second line, and
/// `tests/tools.rs::the_profile_is_built_once_per_session` counts them.
fn compile_profile_once(project: &ProjectConfig) -> Profile {
    let profile = Profile::from_project(project);
    println!(
        "sandbox: profile compiled once for this session -- {} path rule(s), {} command \
         pattern(s), network: {}",
        profile.rule_count(),
        profile.command_pattern_count(),
        if profile.grants_network() {
            "yes"
        } else {
            "no"
        },
    );
    for diagnostic in profile.diagnostics() {
        println!("sandbox: {diagnostic}");
    }
    profile
}

/// The rest of a `/tool …` line, or `None` for any other slash command.
fn tool_invocation(name: &str) -> Option<&str> {
    match name.strip_prefix("tool") {
        Some("") => Some(""),
        Some(rest) if rest.starts_with(char::is_whitespace) => Some(rest.trim()),
        _ => None,
    }
}

/// Parses `<tool> [name=value …]` into a call.
///
/// A value runs to the next `name=` token or to the end of the line, so
/// `command=echo hello` is one command line rather than two arguments. That
/// is the whole grammar: this is a person's entry point, and the model has
/// none — nothing an assistant returns reaches [`invoke::run`] in this
/// package (map line 2457).
fn parse_tool_line(rest: &str) -> Option<(String, Args)> {
    let mut tokens = rest.split_whitespace();
    let tool = tokens.next()?.to_string();
    let mut args = Args::new();
    let mut current: Option<(String, String)> = None;
    for token in tokens {
        match token.split_once('=') {
            Some((name, value)) if !name.is_empty() => {
                if let Some((name, value)) = current.take() {
                    args = args.with(name, value);
                }
                current = Some((name.to_string(), value.to_string()));
            }
            _ => match current.as_mut() {
                Some((_, value)) => {
                    value.push(' ');
                    value.push_str(token);
                }
                None => return None,
            },
        }
    }
    if let Some((name, value)) = current {
        args = args.with(name, value);
    }
    Some((tool, args))
}

/// Answers `/tool …` — **the sandbox's first production caller**, and map
/// line 2455's "every tool runs confined" reaching the binary.
///
/// A refusal is printed and the session continues: `sandbox-grants.md` §1.4
/// is that a refusal is a value, so nothing here prompts, escalates, retries
/// or returns an error to the caller.
fn answer_tool(rest: &str, session: &Session<'_>) {
    let Some((tool, args)) = parse_tool_line(rest) else {
        println!(
            "/tool <name> [arg=value ...]; registered: {}",
            registry::names().join(", ")
        );
        return;
    };
    let ctx = ToolContext {
        profile: session.profile,
        glasshouse: session.glasshouse,
        session: session.id,
    };
    match invoke::run(&ctx, &tool, &args) {
        Ok(result) => {
            print!("{}", result.stdout);
            if !result.stderr.is_empty() {
                eprint!("{}", result.stderr);
            }
            println!(
                "/tool {tool}: exit {} under {}",
                result
                    .exit_code
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "signal".to_string()),
                result.confinement.as_str()
            );
        }
        Err(ToolError::Denied(denied)) => println!("{denied}"),
        Err(error) => println!("/tool {tool}: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_tool_line_is_a_tool_invocation() {
        assert_eq!(tool_invocation("tool read path=x"), Some("read path=x"));
        assert_eq!(tool_invocation("tool"), Some(""));
        assert_eq!(tool_invocation("tooling"), None);
        assert_eq!(tool_invocation("memory"), None);
    }

    #[test]
    fn a_value_runs_to_the_next_name_equals_token() {
        let (tool, args) = parse_tool_line("bash command=echo hello world").unwrap();
        assert_eq!(tool, "bash");
        assert_eq!(args.get("command"), Some("echo hello world"));
    }

    #[test]
    fn two_arguments_are_kept_apart() {
        let (tool, args) = parse_tool_line("grep pattern=fn path=/tmp").unwrap();
        assert_eq!(tool, "grep");
        assert_eq!(args.get("pattern"), Some("fn"));
        assert_eq!(args.get("path"), Some("/tmp"));
    }
}
