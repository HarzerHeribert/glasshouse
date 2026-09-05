//! `pane session`: the run that wires the six merged 61C modules together.
//! Each of them is correct and tested in isolation; this module is the only
//! place any of them is called from `main` rather than from its own tests --
//! see the packet's OBJECTIVE for why that gap, not missing code, is what
//! this module exists to close.

use std::fs;
use std::io::{self, BufRead};
use std::path::PathBuf;
use std::time::SystemTime;

use clap::Parser;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::commands::{self, CommandStatus};
use crate::contract::{Block, Conversation, Message, ProjectConfig, Role, ServedBy, SessionId};
use crate::glasshouse::{self, Glasshouse, LifecycleEvent, LocalMemory};
use crate::project;
use crate::rollout::{self, Rollout};
use crate::tui;
use crate::wire;

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

/// Every project instruction document, concatenated in the order
/// `project::load` found them. The format is this package's own choice --
/// map line 2448 fixes what is loaded, not how it is joined into one prompt.
fn build_system_prompt(project: &ProjectConfig) -> String {
    project
        .instructions
        .iter()
        .map(|(_, text)| text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn message_text(message: &Message) -> String {
    message
        .content
        .iter()
        .map(Block::text)
        .collect::<Vec<_>>()
        .join("")
}

/// Draws the two-region screen into an in-memory buffer. A live interactive
/// terminal (raw mode, an alternate screen, a resize-aware redraw loop) is
/// out of scope for this package -- see the report's limits -- so every call
/// here, scripted or interactive, renders the same way `tui.rs`'s own tests
/// do: nothing in `pane session`'s ordinary path requires a real tty, which
/// is exactly what lets every acceptance test below drive it as a
/// subprocess with piped stdio.
fn render(conversation: &Conversation, served_by: &ServedBy) {
    let backend = TestBackend::new(100, 40);
    let mut terminal = Terminal::new(backend).expect("an in-memory backend never fails to init");
    let _ = terminal.draw(|frame| tui::render(frame, conversation, served_by));
}

/// Runs `session`, in the order the packet's OBJECTIVE fixes: load the
/// project, resume or start the rollout, `SessionStart`, then one input (or
/// stdin's, one per line) at a time until the input source is exhausted.
fn run(args: SessionArgs) -> Result<(), String> {
    let project = project::load(&args.root);

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
    let mut conversation = if resuming {
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

    let outcome = drive(
        &args,
        &project,
        &glasshouse,
        &session_id,
        &mut conversation,
        &mut rollout,
        &memory,
    );

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

/// Runs `args.task` once if given, else reads stdin one line at a time until
/// EOF -- the REPL half of the same input source, so a slash command or a
/// task is handled identically regardless of where it came from.
fn drive(
    args: &SessionArgs,
    project: &ProjectConfig,
    glasshouse: &Glasshouse,
    session_id: &SessionId,
    conversation: &mut Conversation,
    rollout: &mut Rollout,
    memory: &LocalMemory,
) -> Result<(), String> {
    if let Some(task) = &args.task {
        return process_input(
            task,
            project,
            glasshouse,
            session_id,
            conversation,
            rollout,
            memory,
        );
    }

    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let line = line.map_err(|e| e.to_string())?;
        process_input(
            &line,
            project,
            glasshouse,
            session_id,
            conversation,
            rollout,
            memory,
        )?;
    }
    Ok(())
}

/// One input: a slash command answered locally, or a turn sent to the model.
/// A slash command -- resolved or not -- never reaches [`wire::send_turn`];
/// only text that is not a slash command does.
fn process_input(
    input: &str,
    project: &ProjectConfig,
    glasshouse: &Glasshouse,
    session_id: &SessionId,
    conversation: &mut Conversation,
    rollout: &mut Rollout,
    memory: &LocalMemory,
) -> Result<(), String> {
    if let Some(name) = input.strip_prefix('/') {
        answer_command(name, project, glasshouse, memory);
        render(conversation, &ServedBy::default());
        return Ok(());
    }

    glasshouse::emit_lifecycle(glasshouse, session_id, LifecycleEvent::UserPromptSubmit);

    conversation.messages.push(Message::text(Role::User, input));
    rollout
        .record_turn(Role::User, input)
        .map_err(|e| format!("could not record the user turn: {e}"))?;

    let since = SystemTime::now();
    let assistant = wire::send_turn(conversation).map_err(|e| format!("request failed: {e}"))?;
    let assistant_text = message_text(&assistant);
    conversation.messages.push(assistant);
    rollout
        .record_turn(Role::Assistant, &assistant_text)
        .map_err(|e| format!("could not record the assistant turn: {e}"))?;

    let served = glasshouse::served_by(glasshouse, since);
    render(conversation, &served);
    Ok(())
}

/// Answers a slash command. `commands::resolve` only ever *decides* what a
/// command is; acting on one is this function's job, and `/memory` is the
/// only built-in with an action so far.
///
/// **`/memory` is where map line 2446 reaches the binary.** The seam and its
/// local fallback were built and tested by `GH-PANE-61C-SEAMS`, and then
/// nothing called them: `commands` was scoped to decide and never to act, and
/// no package was given the acting half. A capability nothing invokes is not
/// a capability, whatever its tests say.
fn answer_command(
    name: &str,
    project: &ProjectConfig,
    glasshouse: &Glasshouse,
    memory: &LocalMemory,
) {
    match commands::resolve(project, name) {
        Some(resolved) => match resolved.status {
            CommandStatus::Available => {
                if name == "memory" {
                    answer_memory(glasshouse, memory);
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

/// Reads memory and the latest checkpoint through Glasshouse's MCP surface,
/// falling back to the local store when nothing answers — map line 2446.
///
/// Both readers degrade rather than fail, so this prints what it found and
/// says plainly when that was nothing; it never reports an error and never
/// distinguishes "Glasshouse is absent" from "Glasshouse had nothing", which
/// is `glasshouse.rs`'s own contract and not this function's to re-decide.
fn answer_memory(glasshouse: &Glasshouse, memory: &LocalMemory) {
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
