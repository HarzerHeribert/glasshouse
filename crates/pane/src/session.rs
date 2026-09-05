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
use crate::rollout::{self, Rollout};
use crate::sandbox::profile::Profile;
use crate::tools::invoke::{self, Args, ToolContext, ToolError};
use crate::tools::registry;
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

/// Draws the two-region screen where a user or a test can actually see it.
/// A live interactive terminal (raw mode, an alternate screen, a
/// resize-aware redraw loop) is out of scope for this package -- see the
/// report's limits -- so this draws one frame per turn either way, through
/// the same unmodified `tui::render`: to a real `CrosstermBackend` when
/// stdout is a tty, and to stdout as plain lines otherwise, so a pipe never
/// makes the session's output disappear the way it used to.
fn render(conversation: &Conversation, served_by: &ServedBy) {
    if io::stdout().is_terminal() {
        render_to_terminal(conversation, served_by);
    } else {
        render_as_lines(conversation, served_by);
    }
}

fn render_to_terminal(conversation: &Conversation, served_by: &ServedBy) {
    let Ok(mut terminal) = Terminal::new(CrosstermBackend::new(io::stdout())) else {
        return;
    };
    let _ = terminal.draw(|frame| tui::render(frame, conversation, served_by));
}

/// Every acceptance test below, and any real pipe, takes this path. Draws
/// through the identical `tui::render` a live terminal uses, into an
/// in-memory buffer exactly as `tui.rs`'s own tests do, then prints each
/// non-blank row as a line of text -- so the conversation column and the
/// sidebar's content (including its honest "not connected" collapse) reach
/// stdout rather than a dropped `TestBackend`.
fn render_as_lines(conversation: &Conversation, served_by: &ServedBy) {
    let backend = TestBackend::new(100, 40);
    let mut terminal = Terminal::new(backend).expect("an in-memory backend never fails to init");
    let _ = terminal.draw(|frame| tui::render(frame, conversation, served_by));
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

    let session = Session {
        project: &project,
        profile: &profile,
        glasshouse: &glasshouse,
        id: &session_id,
        memory: &memory,
    };
    let outcome = drive(&args, &session, &mut conversation, &mut rollout);

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
    conversation: &mut Conversation,
    rollout: &mut Rollout,
) -> Result<(), String> {
    if let Some(task) = &args.task {
        return process_input(task, session, conversation, rollout);
    }

    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let line = line.map_err(|e| e.to_string())?;
        process_input(&line, session, conversation, rollout)?;
    }
    Ok(())
}

/// One input: a slash command answered locally, or a turn sent to the model.
/// A slash command -- resolved or not -- never reaches [`wire::send_turn`];
/// only text that is not a slash command does.
fn process_input(
    input: &str,
    session: &Session<'_>,
    conversation: &mut Conversation,
    rollout: &mut Rollout,
) -> Result<(), String> {
    if let Some(rest) = input.strip_prefix('/') {
        let (name, argument) = split_command(rest);
        answer_command(rest, name, argument, session);
        render(conversation, &ServedBy::default());
        return Ok(());
    }

    glasshouse::emit_lifecycle(
        session.glasshouse,
        session.id,
        LifecycleEvent::UserPromptSubmit,
    );

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

    let served = glasshouse::served_by(session.glasshouse, since);
    render(conversation, &served);
    Ok(())
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
