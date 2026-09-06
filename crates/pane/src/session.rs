//! `pane session`: the run that wires the six merged 61C modules together.
//! Each of them is correct and tested in isolation; this module is the only
//! place any of them is called from `main` rather than from its own tests --
//! see the packet's OBJECTIVE for why that gap, not missing code, is what
//! this module exists to close.

mod ui;

use std::cell::{Cell, RefCell};
use std::fs;
use std::io::{self, BufRead, IsTerminal};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use clap::Parser;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::bg;
use crate::commands::{self, CommandSource, CommandStatus};
use crate::config::PaneConfig;
use crate::contract::{Block, Conversation, Message, ProjectConfig, Role, ServedBy, SessionId};
use crate::events::batch::Batch;
use crate::events::window::{Window, WindowConfig};
use crate::glasshouse::{self, Glasshouse, LifecycleEvent, LocalMemory};
use crate::project;
use crate::prompt::{self, Budget, CellResult, ErrorSection, ExhaustedReason, Extracted};
use crate::rollout::{self, Rollout};
use crate::runtime::handles::HandleTable;
use crate::runtime::isolate::{DEFAULT_HEAP_LIMIT_BYTES, Runtime};
use crate::runtime::outcome::{CellOutcome, CellRecord, Ended};
use crate::runtime::preview;
use crate::sandbox::profile::Profile;
use crate::supervisor::Supervisor;
use crate::telemetry::RequestMeasurement;
use crate::tools::invoke::{self, Args, ToolContext, ToolError};
use crate::tools::registry;
use crate::tui::{self, CellError, CellView, Counted, Notebook, SupervisorStatus, TaskTokens};
use crate::wire;

/// How many prose turns in a row end the task (the primary's addendum of
/// 2026-09-06): on this one, the answer carries the exhausted preamble and
/// the loop ends after one more turn whatever the model does. A program or
/// two blocks resets the count; the token budget stays the outer stop.
const PROSE_TURN_CAP: u32 = 3;
const REQUEST_MEASUREMENT_CAP: usize = 64;

macro_rules! session_println {
    ($($arg:tt)*) => { ui::output(format!($($arg)*)) };
}

fn record_request(notebook: &mut Notebook, measurement: RequestMeasurement) {
    if notebook.requests.len() >= REQUEST_MEASUREMENT_CAP {
        let remove = notebook.requests.len() + 1 - REQUEST_MEASUREMENT_CAP;
        notebook.requests.drain(..remove);
    }
    notebook.requests.push(measurement);
}
mod controls;

/// The longest a turn waits for an open event window to close before it is
/// composed without one — `events-contract.md` §2's own 2,000 ms deadline
/// plus room for the drain that follows it.
///
/// It is a ceiling, not a delay: an **empty** window can never close and is
/// answered at once, which is every turn of every session that raised no
/// event. Only a window already holding an event is waited on, and only until
/// §2's deadline closes it.
const EVENT_WAIT: Duration = Duration::from_millis(2_500);

/// How often that wait looks again. Short enough that a window closes within
/// a frame of its deadline, long enough that waiting costs nothing.
const EVENT_POLL: Duration = Duration::from_millis(25);

/// §5's answer to a message that carried no program.
const NO_PROGRAM: &str = "no program ran; send one pane block";

/// §5's answer to a message that carried two. **Neither runs**, and the
/// sentence is the contract's own: running the first is the silently-wrong
/// reading, because the second is usually the one the model meant.
const TWO_BLOCKS: &str = "two pane blocks in one turn; send one";

/// The class a cancelled call throws with (`bindings.rs`'s `Cancelled`), read
/// off the cell's own trajectory so the session knows a Ctrl-C was delivered.
const CANCELLED: &str = "Cancelled";

/// A second Ctrl-C inside this window ends the session; a later one starts a
/// new pair. Two seconds is long enough that a person who meant "again"
/// reaches it and short enough that an interrupt an hour ago is not half of
/// today's.
const DOUBLE_INTERRUPT_WINDOW: Duration = Duration::from_secs(2);

/// How often the watcher asks whether the handler fired -- the same 20 ms
/// `tools::invoke` polls its child with, so a Ctrl-C costs at most two polls.
const INTERRUPT_POLL: Duration = Duration::from_millis(20);

/// The status a shell reports for a process ended by SIGINT.
const INTERRUPTED_EXIT: i32 = 130;

/// How long the second Ctrl-C gives the cancelled call to kill and reap its
/// own child before exiting anyway: twelve of `invoke`'s 20 ms polls, spent
/// holding the rollout's write lock so the task loop cannot start another
/// call inside it. See [`Interrupter::end_the_session`].
const REAP_GRACE: Duration = Duration::from_millis(250);

/// Raised by the signal handler and by nothing else.
///
/// **A handler may do exactly one async-signal-safe thing, and this is it.**
/// Everything the interrupt means -- which token to cancel, whether it is the
/// second of a pair, whether a rollout line is half written -- is decided by
/// [`watch`] on an ordinary thread, where locks and allocation are legal.
static INTERRUPT: AtomicBool = AtomicBool::new(false);

/// Installs the process's SIGINT handler. Unix: `signal(2)`, whose BSD
/// semantics on both platforms pane ships for leave the handler installed
/// across deliveries, so a second Ctrl-C reaches the same function.
///
/// `libc` is not a dependency of this crate on macOS and this is two lines of
/// declaration, so the handler is declared rather than depended on -- the same
/// choice `sandbox::macos` makes for `sandbox_init`.
#[cfg(unix)]
fn install_interrupt_handler() {
    /// `SIGINT` on every unix pane ships for.
    const SIGINT: i32 = 2;

    unsafe extern "C" {
        fn signal(sig: i32, handler: usize) -> usize;
    }

    extern "C" fn on_interrupt(_sig: i32) {
        INTERRUPT.store(true, Ordering::SeqCst);
    }

    unsafe { signal(SIGINT, on_interrupt as *const () as usize) };
}

/// The Windows half: the console's Ctrl-C routine sets the identical flag.
///
/// It runs on a thread of the console's own making rather than on top of the
/// interrupted one, and returning `TRUE` says the event was handled -- which
/// is what stops the default handler ending the process before [`watch`] has
/// decided whether this was the first Ctrl-C or the second.
#[cfg(windows)]
fn install_interrupt_handler() {
    use windows_sys::Win32::Foundation::TRUE;
    use windows_sys::Win32::System::Console::{
        CTRL_BREAK_EVENT, CTRL_C_EVENT, SetConsoleCtrlHandler,
    };
    use windows_sys::core::BOOL;

    unsafe extern "system" fn on_interrupt(event: u32) -> BOOL {
        if event == CTRL_C_EVENT || event == CTRL_BREAK_EVENT {
            INTERRUPT.store(true, Ordering::SeqCst);
        }
        TRUE
    }

    unsafe { SetConsoleCtrlHandler(Some(on_interrupt), TRUE) };
}

/// The keyboard's end of the cancellation facility.
///
/// **A Ctrl-C cancels the call in flight; it never terminates the isolate.**
/// JavaScript is stopped by the wall-clock watchdog (`runtime-contract.md`
/// §7's limits) and by nothing else here: a second terminator racing the
/// watchdog's own is how a runtime that could be stopped becomes one that
/// cannot. So an interrupt raised while a cell is only computing is not lost
/// and not applied either -- [`pending`](Self::pending) stays raised until a
/// call actually ends `Cancelled`, which makes the *next* call the one it
/// cancels.
struct Interrupter {
    /// The session whose background board [`end_the_session`](Self::end_the_session)
    /// has to take with it -- a value `run` already holds when it builds this,
    /// rather than a boxed shutdown callback, whose only effect would be to
    /// hide one call to a module this file already imports.
    session: SessionId,
    /// The token of the cell now running, replaced before every cell so one
    /// Ctrl-C cannot cancel every later call.
    token: Mutex<invoke::CancellationToken>,
    /// Raised when an interrupt has been seen and not yet consumed by a call
    /// that ended `Cancelled`.
    pending: AtomicBool,
    /// Raised once the second Ctrl-C has decided to exit, and never lowered.
    /// It pins [`pending`](Self::pending) raised, so every call started
    /// during the reap grace is cancelled before it spawns a child.
    ending: AtomicBool,
    /// Held for the length of every rollout write. The second-Ctrl-C exit
    /// takes it too, which is the whole of "the rollout's current line is
    /// complete": `Rollout` writes one whole line per call, so waiting for
    /// this lock is waiting for that call to return.
    writing: Mutex<()>,
}

/// A lock that a panic elsewhere cannot turn into a second failure: the data
/// behind both mutexes is a token and a unit, neither of which a panic can
/// leave inconsistent.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Interrupter {
    fn new(session: SessionId) -> Self {
        Self {
            session,
            token: Mutex::new(invoke::CancellationToken::new()),
            pending: AtomicBool::new(false),
            ending: AtomicBool::new(false),
            writing: Mutex::new(()),
        }
    }

    /// Publishes the token the cell about to run will make its calls through,
    /// cancelling it on the spot if an interrupt is still pending.
    ///
    /// The check and the store happen under one lock, so there is no window
    /// in which [`raise`](Self::raise) cancels the token being replaced and
    /// the replacement escapes uncancelled.
    fn arm(&self, token: invoke::CancellationToken) {
        let mut slot = lock(&self.token);
        if self.pending.load(Ordering::SeqCst) {
            token.cancel();
        }
        *slot = token;
    }

    /// One interrupt: cancel the call in flight and stay raised.
    fn raise(&self) {
        let slot = lock(&self.token);
        self.pending.store(true, Ordering::SeqCst);
        slot.cancel();
    }

    /// A call ended `Cancelled`, so the interrupt that asked for it has been
    /// delivered and later cells start clean -- **unless the session is
    /// already ending**, in which case there are no later cells and lowering
    /// the flag would let one start a child the exit then orphans.
    fn consumed(&self) {
        if !self.ending.load(Ordering::SeqCst) {
            self.pending.store(false, Ordering::SeqCst);
        }
    }

    fn writing(&self) -> MutexGuard<'_, ()> {
        lock(&self.writing)
    }

    /// The second Ctrl-C, and the only place in `pane` that exits from a
    /// thread other than the main one.
    ///
    /// **It cancels before it exits, and that is not decoration.**
    /// `std::process::exit` does not touch this process's children, so an
    /// exit taken with a call in flight reparents the confined child to
    /// `init` and leaves it there. Measured, before this function did
    /// anything but exit: one `bash` spinning at 87% of a core, for ever.
    /// Cancelling hands that child to `invoke::kill_and_reap`, which kills
    /// *and* reaps it.
    ///
    /// **Then it takes [`writing`](Self::writing) and holds it across the
    /// grace, and that ordering is the rest of the fix.** Taking the lock
    /// waits for the rollout line in flight to finish, which is the
    /// whole-line guarantee. *Holding* it stops the task loop at its next
    /// write -- `act_on`'s cell line is the very next thing after the
    /// cancelled call returns -- so the loop cannot answer the cell, ask for
    /// another turn and start another cell inside the grace. It did exactly
    /// that when the grace was an unguarded sleep, spawning a *fresh*
    /// spinning child for the same exit to orphan.
    ///
    /// **Then it takes the background board with it, which is the same
    /// defect a second time**: `raise` cancels the foreground call's token
    /// and nothing else, and a job runs on a thread of its own under a token
    /// of its own. Measured before this call existed: a job's `bash` on
    /// `ppid 1` at 99% of a core, twenty seconds after `pane` exited 130.
    /// It goes *after* the lock, because holding it is what stops the loop
    /// starting a fresh `bg.run` for the exit to orphan, and *before* the
    /// sleep, because the grace is what the cancelled children are reaped
    /// in. The grace it passes is [`REAP_GRACE`] rather than `bg`'s own ten
    /// seconds, and `shutdown_within` detaches what has not stopped by then:
    /// a Ctrl-C that waits for an unkillable job would be a worse defect
    /// than the orphan this closes.
    ///
    /// [`REAP_GRACE`] is bounded because a Ctrl-C that hangs is not a Ctrl-C:
    /// after it, the exit proceeds whatever the child is doing.
    fn end_the_session(&self) -> ! {
        self.ending.store(true, Ordering::SeqCst);
        self.raise();
        let _line = self.writing();
        bg::shutdown_within(&self.session, REAP_GRACE);
        std::thread::sleep(REAP_GRACE);
        ui::restore_terminal();
        eprintln!("pane: interrupted twice; ending the session");
        std::process::exit(INTERRUPTED_EXIT);
    }
}

/// Turns the handler's flag into the session's decision, forever.
///
/// It is a thread because there is nowhere else to poll from: a task spends
/// its whole life inside `send_turn` or inside `run_cell`, and neither
/// returns to the loop while the call a Ctrl-C is meant to stop is running.
fn watch(state: &Interrupter) -> ! {
    let mut first: Option<Instant> = None;
    loop {
        std::thread::sleep(INTERRUPT_POLL);
        if !INTERRUPT.swap(false, Ordering::SeqCst) {
            continue;
        }
        let now = Instant::now();
        if first.is_some_and(|earlier| now.duration_since(earlier) <= DOUBLE_INTERRUPT_WINDOW) {
            state.end_the_session();
        }
        first = Some(now);
        state.raise();
    }
}

/// Whether this cell delivered the pending interrupt: any call of its
/// trajectory that ended as a `Cancelled` throw did.
///
/// **The trajectory rather than the cell's own ending**, because a program
/// may catch the throw (`runtime-contract.md` §9.1's stated limit) and a
/// Ctrl-C the program swallowed was still delivered -- reading the cell's
/// outcome instead would leave the flag raised and cancel the next cell too.
fn delivered_the_interrupt(record: &CellRecord) -> bool {
    record
        .calls
        .iter()
        .any(|call| matches!(&call.ended, Ended::Threw { class } if class == CANCELLED))
}

/// The two rollout writes in this module, and every one of them goes through
/// one of these -- which is what makes [`Interrupter::end_the_session`]'s
/// "never a half line" a property of the code rather than of the timing.
fn write_turn(
    interrupt: &Interrupter,
    rollout: &mut Rollout,
    role: Role,
    text: &str,
) -> io::Result<()> {
    let _line = interrupt.writing();
    rollout.record_turn(role, text)
}

fn write_cell(
    interrupt: &Interrupter,
    rollout: &mut Rollout,
    record: &CellRecord,
) -> io::Result<()> {
    let _line = interrupt.writing();
    rollout.record_cell(record)
}

/// `pane session`'s whole flag set. A project root and a way to identify the
/// rollout file are the only things every run needs; `--task` is the
/// non-interactive, scriptable entry point this package's own acceptance
/// tests drive (`env!("CARGO_BIN_EXE_pane")` subprocesses can pipe a task in
/// as an argument far more simply than as timed stdin), and the same flag is
/// what the ruler's future `pane` harness row will pass a statement through.
/// Absent `--task`, terminals use the live composer; piped input is read
/// one input per line until EOF.
#[derive(Parser, Debug)]
#[command(name = "pane session")]
pub struct SessionArgs {
    /// The project root map line 2448 loads from.
    #[arg(long)]
    pub root: PathBuf,

    /// One scripted user input (a slash command or a task) run once, non-
    /// interactively. Omitted opens the live composer on a terminal, or reads
    /// piped inputs one per line until EOF.
    #[arg(long)]
    pub task: Option<String>,

    /// Initial request model; can also be changed with /model.
    #[arg(long)]
    pub model: Option<String>,

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

    /// Grant the whole project root and every command line, ignoring
    /// `.claude/settings.json`.
    ///
    /// **This is the person widening their own grant at session start, which
    /// is the only widening `sandbox-grants.md` §1.1 permits** — it is a flag
    /// on the command that starts the session, never something a cell can
    /// reach, ask for or set. It compiles a synthesised settings document
    /// rather than adding a second way to build a profile, so §4's
    /// never-grantable set still applies: a debugger is refused under
    /// `--yolo` exactly as it is without it.
    #[arg(long)]
    pub yolo: bool,
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
fn build_system_prompt(project: &ProjectConfig, profile: &Profile) -> String {
    let instructions = project
        .instructions
        .iter()
        .map(|(_, text)| text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    prompt::render_system(
        &instructions,
        &registry::ALL.iter().collect::<Vec<_>>(),
        &session_facts(profile),
    )
}

/// The compiled profile, as the model needs to read it.
///
/// **The invariant: this reports the profile that is actually in force, never
/// the one the configuration asked for.** It is built from `Profile`'s own
/// accessors for that reason — a settings document that failed to parse
/// grants nothing, and a model told otherwise would plan against grants it
/// does not have.
///
/// `pub` so `tests/session.rs`'s byte-equality test can build the same facts
/// the binary did rather than spelling them a second time — the same reason
/// that test calls [`prompt::render_system`] instead of quoting its output.
pub fn session_facts(profile: &Profile) -> prompt::SessionFacts {
    let mut writable: Vec<String> = profile
        .rules()
        .filter(|rule| rule.write() && rule.effect() == crate::sandbox::profile::Effect::Allow)
        .map(|rule| rule.written().to_string())
        .collect();
    writable.sort();
    writable.dedup();
    prompt::SessionFacts {
        root: profile.root().display().to_string(),
        writable,
        command_patterns: profile.command_pattern_count(),
        // Not `args.yolo`: the flag is a request, the profile is the grant.
        // A mutation that stopped `--yolo` reaching the compiler survived
        // while this read the flag, because the model was still told the
        // grant was open (2026-09-06).
        all_commands: profile.admits_every_command(),
        network: profile.grants_network(),
    }
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

/// Pipe output is static; the interactive terminal is owned by `ui::LiveUi`.
fn render(
    transcript: &Transcript,
    served_by: &ServedBy,
    session: &Session<'_>,
    activity: tui::Activity,
) {
    if let Some(ui) = session.ui {
        ui.publish(transcript, served_by, activity);
    } else {
        render_as_lines(transcript, served_by);
    }
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
    let config = PaneConfig::load(&args.root)?;
    if config.supervisor.model.is_none() {
        session_println!("supervisor: off (no model)");
    }

    // `sandbox-grants.md` §1.5: computed once, at session start, immutable
    // for the session's life. `.claude/` lives inside the writable project
    // root, so a profile recomputed mid-session would let a program widen
    // its own sandbox by editing the file it was derived from.
    let profile = compile_profile_once(&project, args.yolo);

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
            system: build_system_prompt(&project, &profile),
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

    // Installed before the first task and never again: from here on a Ctrl-C
    // cancels the call in flight rather than killing the process mid-line.
    let interrupt = Arc::new(Interrupter::new(session_id.clone()));
    install_interrupt_handler();
    let watched = Arc::clone(&interrupt);
    std::thread::spawn(move || watch(&watched));

    let interactive =
        if args.task.is_none() && io::stdin().is_terminal() && io::stdout().is_terminal() {
            Some(ui::LiveUi::start(
                tui::ScreenState {
                    model: Some(args.model.clone().unwrap_or_else(|| wire::MODEL.into())),
                    compact: true,
                    pretty: true,
                    project: Some(
                        args.root
                            .file_name()
                            .unwrap_or(args.root.as_os_str())
                            .to_string_lossy()
                            .into_owned(),
                    ),
                    sandbox: Some(format!(
                        "{}p/{}c{}",
                        profile.rule_count(),
                        profile.command_pattern_count(),
                        if args.yolo { " YOLO" } else { "" }
                    )),
                    network: Some(
                        if profile.grants_network() {
                            "on"
                        } else {
                            "off"
                        }
                        .into(),
                    ),
                    ..tui::ScreenState::default()
                },
                transcript.conversation.clone(),
                transcript.notebook.clone(),
            )?)
        } else {
            None
        };

    let session = Session {
        project: &project,
        config: &config,
        profile: &profile,
        glasshouse: &glasshouse,
        id: &session_id,
        memory: &memory,
        interrupt: &interrupt,
        ui: interactive.as_ref(),
        model: RefCell::new(args.model.clone().unwrap_or_else(|| wire::MODEL.into())),
        mode: Cell::new(tui::Mode::Execute),
        effort: Cell::new(wire::Effort::Auto),
    };
    let outcome = drive(&args, &session, &mut transcript, &mut rollout);
    // §5 again, and this one is the promise `session::run` itself makes: an
    // input that failed mid-task left `run_task` by `?` without reaching its
    // own shutdown, and a job of that task must not outlive the session
    // either.
    bg::shutdown(&session_id);

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
    ui: Option<&'a ui::LiveUi>,
    model: RefCell<String>,
    mode: Cell<tui::Mode>,
    effort: Cell<wire::Effort>,
    project: &'a ProjectConfig,
    config: &'a PaneConfig,
    /// The keyboard's end of the cancellation facility: the SIGINT handler's
    /// flag, the token of the cell now running, and the lock every rollout
    /// write is taken under. A task publishes each cell's fresh token to it
    /// and asks it to forget the interrupt a cancelled call has delivered.
    interrupt: &'a Interrupter,
    profile: &'a Profile,
    glasshouse: &'a Glasshouse,
    id: &'a SessionId,
    memory: &'a LocalMemory,
}

/// Handles scripted, live-composer, and piped input through the same task
/// and command dispatch.
fn drive(
    args: &SessionArgs,
    session: &Session<'_>,
    transcript: &mut Transcript,
    rollout: &mut Rollout,
) -> Result<(), String> {
    if let Some(task) = &args.task {
        return process_input(task, session, transcript, rollout);
    }

    if let Some(ui) = session.ui {
        while let Some(input) = ui.next()? {
            let result = process_input(&input, session, transcript, rollout);
            if let Err(message) = &result {
                session_println!("ERROR: {message}");
            }
            ui.publish(
                transcript,
                &ServedBy::default(),
                if result.is_err() {
                    tui::Activity::Failed
                } else {
                    tui::Activity::Complete
                },
            );
        }
        return Ok(());
    }

    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let line = line.map_err(|e| e.to_string())?;
        // **A failed input ends that input, not the session.** A REPL that
        // exits on the first upstream error loses the whole conversation to
        // one 400 or one dropped connection, which is what a person watching
        // reads as "it crashed"; `--task` above still propagates, because a
        // scripted one-shot has nobody to report to but its exit code.
        // Observed 2026-09-06: one empty message made a gateway answer 400
        // and the session ended mid-task.
        if let Err(message) = process_input(&line, session, transcript, rollout) {
            session_println!("{message}");
        }
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
    // **Blank input is not a turn.** A message with no content is not a
    // message: the Anthropic shape requires content, tool calls or reasoning
    // blocks, and a gateway that enforces it answers 400 and the task dies.
    // A bare Enter is the commonest keystroke in a REPL, so this guard is
    // what stops it ending the session. Observed 2026-09-06 at `messages.0`
    // and again at `messages.13`.
    if input.trim().is_empty() {
        return Ok(());
    }
    if let Some(rest) = input.strip_prefix('/') {
        let (name, argument) = split_command(rest);
        answer_command(rest, name, argument, session, transcript);
        render(
            transcript,
            &ServedBy::default(),
            session,
            tui::Activity::Idle,
        );
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
    /// `pane.toml`'s `[limits]` -- `task_tokens` and `cells`, 61F's own
    /// replacement for what were the constants `TASK_TOKEN_CAP` and
    /// `CELL_CAP`. Defaults match those constants exactly.
    task_cap: u64,
    cells_cap: u64,
}

impl TaskBudget {
    fn new(task_cap: u64, cells_cap: u64) -> Self {
        Self {
            used: 0,
            cells_used: 0,
            reported: false,
            estimated: false,
            task_cap,
            cells_cap,
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
            task_cap: self.task_cap,
            cells_used: self.cells_used,
            cells_cap: self.cells_cap,
        }
    }

    fn tokens(&self) -> Option<TaskTokens> {
        Some(TaskTokens {
            used: self.used,
            cap: self.task_cap,
            counted: self.counted()?,
        })
    }

    /// Whether this task may still ask for another turn after the one being
    /// answered -- §6's cap on cells and its task budget, either of which
    /// buys exactly one more turn under [`prompt::exhausted_preamble`].
    fn spent(&self) -> bool {
        self.used >= self.task_cap || self.cells_used >= self.cells_cap
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
    /// The cell this turn ran, for the supervisor's own buffer
    /// (`supervisor.md` §2) -- `None` for prose and for two blocks, neither
    /// of which ran a cell at all, so neither counts toward the cadence.
    record: Option<CellRecord>,
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
    write_turn(session.interrupt, rollout, Role::User, task)
        .map_err(|e| format!("could not record the user turn: {e}"))?;

    if session.mode.get() == tui::Mode::Plan {
        let mut request = transcript.conversation.clone();
        request.system.push_str("\nPlanning mode: respond naturally with a plan. No code or tool call will execute in this mode.");
        if let Some(ui) = session.ui {
            ui.publish(transcript, &ServedBy::default(), tui::Activity::Thinking);
        }
        let since = SystemTime::now();
        let requested_model = session.model.borrow().clone();
        let request_cell = tui::cell_ordinal(&transcript.conversation, &transcript.notebook) + 1;
        let estimated = estimate_task_request_tokens(&request, &session.model.borrow(), task);
        let (turn, elapsed_ms) = timed_send_task_turn(&request, session, task)
            .map_err(|e| format!("request failed: {e}"))?;
        let served = glasshouse::served_by(session.glasshouse, since);
        record_request(
            &mut transcript.notebook,
            RequestMeasurement::from_response(
                request_cell,
                requested_model,
                elapsed_ms,
                // Project routing rows are not correlated to this request.
                ServedBy::default(),
                turn.usage.as_ref(),
            ),
        );
        let text = message_text(&turn.message);
        if text.trim().is_empty() {
            return Err("the model returned an empty reply".into());
        }
        write_turn(session.interrupt, rollout, Role::Assistant, &text)
            .map_err(|e| e.to_string())?;
        let mut budget = TaskBudget::new(
            session.config.limits.task_tokens,
            session.config.limits.cells,
        );
        budget.add(&served, turn.usage.as_ref(), estimated);
        transcript.notebook.tokens = budget.tokens();
        transcript.conversation.messages.push(turn.message);
        transcript.notebook.cells.push(CellView {
            table: Some("Planning mode · code was not executed".into()),
            ..CellView::default()
        });
        render(transcript, &served, session, tui::Activity::Complete);
        return Ok(());
    }
    let mut runtime = Runtime::with_limits(
        session.profile,
        session.glasshouse,
        session.id,
        DEFAULT_HEAP_LIMIT_BYTES,
        Duration::from_secs(session.config.limits.cell_wall_clock_s),
    )
    .with_response_byte_cap(session.config.limits.response_bytes);
    let mut budget = TaskBudget::new(
        session.config.limits.task_tokens,
        session.config.limits.cells,
    );
    // `events-contract.md` §2: one window is always open, from session start
    // or from the moment the previous batch was delivered. It is per task
    // because the isolate the batch is bound in is, and §5's jobs are
    // cancelled with it below.
    let mut window = Window::new(WindowConfig::default());
    let mut final_turn = false;
    let mut prose_turns = 0u32;
    let supervisor = Supervisor::new();
    let supervisor_active =
        session.config.supervisor.enabled && session.config.supervisor.model.is_some();
    let mut cells_since_look: Vec<CellRecord> = Vec::new();

    loop {
        let since = SystemTime::now();
        let requested_model = session.model.borrow().clone();
        if let Some(ui) = session.ui {
            ui.publish(transcript, &ServedBy::default(), tui::Activity::Thinking);
        }
        let (turn, elapsed_ms) =
            send_task_turn_recovering(transcript, session, &runtime, task, rollout)?;
        let request_cell = tui::cell_ordinal(&transcript.conversation, &transcript.notebook) + 1;
        let estimate =
            estimate_task_request_tokens(&transcript.conversation, &session.model.borrow(), task);
        let served = glasshouse::served_by(session.glasshouse, since);
        record_request(
            &mut transcript.notebook,
            RequestMeasurement::from_response(
                request_cell,
                requested_model,
                elapsed_ms,
                // Project routing rows are not correlated to this request.
                ServedBy::default(),
                turn.usage.as_ref(),
            ),
        );
        let assistant_text = message_text(&turn.message);
        // **An empty reply is never appended.** A message with no content is
        // not a message, and appending one poisons the conversation for the
        // whole task: every later request replays it, and a gateway that
        // enforces the shape answers 400 to all of them, so one empty reply
        // becomes a task that can no longer make any request at all. Ending
        // here loses this turn; appending loses the session. Observed
        // 2026-09-06: `messages.13` empty, then 400 on every retry.
        if assistant_text.trim().is_empty() {
            return Err(
                "the model returned an empty reply; the task ends here rather than repeating it \
                 on every later request"
                    .to_string(),
            );
        }
        transcript.conversation.messages.push(turn.message);
        write_turn(session.interrupt, rollout, Role::Assistant, &assistant_text)
            .map_err(|e| format!("could not record the assistant turn: {e}"))?;

        budget.add(&served, turn.usage.as_ref(), estimate);

        // A fresh token for this cell, published to the watcher before the
        // cell can make a call: one Ctrl-C is one cell's cancellation, and
        // `arm` cancels this one on the spot if an earlier interrupt is still
        // pending, so an interrupt raised while nothing was in flight is
        // spent on the next call rather than lost.
        let cell_token = invoke::CancellationToken::new();
        session.interrupt.arm(cell_token.clone());
        runtime.set_token(cell_token);
        // What a subagent inherits and is measured against — Phase 64. Set
        // per turn rather than once, because both change during a task.
        runtime.set_task_context(
            budget.task_cap.saturating_sub(budget.used),
            &session.model.borrow(),
        );

        // `events-contract.md` §4: the window that was open while this turn
        // was being answered closes here and its batch is bound into the
        // model's scope -- before the cell runs, so the handle table this
        // cell's own result carries has the `batch` row last and the model
        // sees the events on the very next turn. **No event ever gets a turn
        // of its own** (line 2481): a turn is composed for a user message,
        // and a batch rides the one that was already going to happen.
        if let Some(batch) = next_batch(&mut window, session.id, EVENT_WAIT)
            && let Some(previous) = runtime.deliver_batch(batch)
        {
            // §3: the previous batch's unacked events roll into the window
            // that just opened. The runtime hands the batch back rather than
            // rolling it itself -- it owns no window.
            window.carry_forward(previous.roll());
        }

        let ordinal = tui::cell_ordinal(&transcript.conversation, &transcript.notebook);
        if let Some(ui) = session.ui {
            ui.publish(transcript, &served, tui::Activity::Executing);
        }
        let mut step = act_on(
            &assistant_text,
            &mut runtime,
            &mut budget,
            rollout,
            session.interrupt,
            session.profile,
        )?;
        prose_turns = if step.prose { prose_turns + 1 } else { 0 };
        if let Some(record) = step.record.take() {
            if delivered_the_interrupt(&record) {
                session.interrupt.consumed();
            }
            cells_since_look.push(record);
        }

        // `supervisor.md` §3: one look every `every` cells, and only when
        // there is a next user message left to head -- a task that just
        // ended has nothing for a nudge to attach to, so no look is spent on
        // one. §2: prose and two-blocks turns never reach `cells_since_look`
        // at all (they push no record above), so they never count.
        let mut nudge_reason: Option<String> = None;
        if step.answer.is_some() {
            if !supervisor_active {
                transcript.notebook.supervisor = Some(SupervisorStatus::Off);
            } else if cells_since_look.len() as u32 >= session.config.supervisor.every {
                let trajectory = crate::supervisor::compress(&cells_since_look);
                cells_since_look.clear();
                // `supervisor_active` already established `model.is_some()`.
                let model = session
                    .config
                    .supervisor
                    .model
                    .as_deref()
                    .expect("supervisor_active implies a configured model");
                let decision = supervisor.look(model, &trajectory);
                if decision.intervene {
                    nudge_reason = Some(decision.reason.clone());
                    transcript.notebook.supervisor =
                        Some(SupervisorStatus::Nudged(decision.reason));
                } else {
                    transcript.notebook.supervisor = Some(SupervisorStatus::LookedNoNudge);
                }
            }
        }

        // §9.2: the terminal response is the assistant's own turn -- the same
        // line an assistant message has always written, so `resume` rebuilds
        // it with no new reader -- and it is written after the cell line
        // `act_on` already appended, so the rollout ends cell, then reply.
        if let Some(response) = &step.response {
            transcript
                .conversation
                .messages
                .push(Message::text(Role::Assistant, response));
            write_turn(session.interrupt, rollout, Role::Assistant, response)
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

        // `supervisor.md` §4: the nudge is the very head of the next user
        // message -- applied last, so a look that coincides with the
        // exhausted preamble puts the nudge first, ahead of it.
        if let Some(reason) = nudge_reason
            && let Some(answer) = step.answer.take()
        {
            step.answer = Some(format!("supervisor: {reason}\n{answer}"));
        }

        step.view.answered = step.answer.is_some();
        transcript.notebook.tokens = budget.tokens();
        transcript.notebook.set(ordinal, step.view);

        if let Some(answer) = &step.answer {
            transcript
                .conversation
                .messages
                .push(Message::text(Role::User, answer));
            write_turn(session.interrupt, rollout, Role::User, answer)
                .map_err(|e| format!("could not record the runtime's answer: {e}"))?;
        }

        render(
            transcript,
            &served,
            session,
            if stop {
                tui::Activity::Complete
            } else {
                tui::Activity::Thinking
            },
        );

        if stop {
            break;
        }
    }

    runtime.end_task();
    // §5: a background job outlives no task. Every live job is cancelled
    // through `bg::cancel`'s ladder -- `invoke`'s own group kill -- and its
    // thread is joined, so nothing this task started is still running when
    // the isolate that could have read its result is gone.
    bg::shutdown(session.id);
    Ok(())
}

/// §4's delivery decision for one turn: the batch this turn carries, or
/// `None`.
///
/// Three states, and the third is the whole of §4's *"a turn with an empty
/// batch and no user input does not happen: the runtime waits"*:
///
/// - the window is **closed** (an interrupt, or its deadline has passed):
///   its batch is delivered now;
/// - the window is **open with events in it**: this waits, because
///   delivering now would give the events that arrived first a turn of their
///   own and leave the rest for the next one — which is exactly what line
///   2481 forbids. It waits only until §2's deadline closes the window, and
///   never past `budget`;
/// - the window is **empty**: there is nothing to deliver and nothing to wait
///   for — an empty window has no deadline and can never close — so this
///   answers `None` at once and the turn happens because a user message asked
///   for it. **A batch never composes a turn**; that is why an empty one
///   cannot.
///
/// `budget` is a ceiling on the second state, so a clock that jumps backwards
/// or a window whose deadline is misconfigured costs a bounded wait rather
/// than a session that never sends another turn.
fn next_batch(window: &mut Window, session: &SessionId, budget: Duration) -> Option<Batch> {
    let started = Instant::now();
    loop {
        for event in bg::drain(session) {
            window.accept(event, crate::events::now());
        }
        if let Some(batch) = window.close_if_due(crate::events::now()) {
            return Some(batch);
        }
        if window.is_empty() || started.elapsed() >= budget {
            return None;
        }
        std::thread::sleep(EVENT_POLL);
    }
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
    interrupt: &Interrupter,
    profile: &Profile,
) -> Result<Step, String> {
    let (source, repaired_from) = match prompt::extract_program(assistant_text) {
        Extracted::Program(source) => (source, None),
        Extracted::Edit(json) => {
            let patched = runtime
                .syntax_failure()
                .ok_or_else(|| "No syntax-failed cell is available in this task.".to_string())
                .and_then(|failed| {
                    failed
                        .apply(&json)
                        .map(|source| (source, Some(failed.cell)))
                });
            match patched {
                Ok(patched) => patched,
                Err(error) => {
                    let hint = runtime
                        .syntax_failure()
                        .map(|failed| failed.hint())
                        .unwrap_or_default();
                    return Ok(Step {
                        answer: Some(format!("CellEditError: {error} Nothing ran.\n{hint}")),
                        response: None,
                        prose: true,
                        record: None,
                        view: CellView {
                            error: Some(CellError {
                                class: "CellEditError".into(),
                                message: error,
                                line: None,
                                column: None,
                            }),
                            ..CellView::default()
                        },
                    });
                }
            }
        }
        // §5: the task does not advance and the cell counter does not move.
        // The screen still shows the table, because it is still what the
        // isolate holds -- an output region saying `(no outputs)` beside live
        // handles would be the screen disagreeing with the message sent in
        // the same breath.
        Extracted::Prose
            if !assistant_text.contains("<php-pane>") && !assistant_text.contains("```pane") =>
        {
            return Ok(Step {
                answer: None,
                response: None,
                prose: false,
                record: None,
                view: CellView::default(),
            });
        }
        Extracted::Prose => {
            let table = runtime.render_handles();
            return Ok(Step {
                answer: Some(unchanged_table(&table)),
                response: None,
                prose: true,
                record: None,
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
                prose: assistant_text.contains("```pane-edit"),
                record: None,
                view: CellView {
                    table: Some(runtime.render_handles()),
                    ..CellView::default()
                },
            });
        }
    };

    let before = crate::changes::Snapshot::capture(profile);
    let outcome = runtime.run_cell(&source);
    let changes = before.diff(&crate::changes::Snapshot::capture(profile));
    budget.cells_used = budget.cells_used.saturating_add(1);
    let turn = outcome.turn();
    let record = turn.record.clone();
    write_cell(interrupt, rollout, &turn.record)
        .map_err(|e| format!("could not record the cell: {e}"))?;

    let mut view = CellView {
        executed_source: repaired_from.map(|_| source.clone()),
        repaired_from,
        changes,
        execution: Some(if record.calls.is_empty() {
            "No tool calls ran in this cell.".into()
        } else {
            record
                .calls
                .iter()
                .enumerate()
                .map(|(i, call)| {
                    let status = match &call.ended {
                        crate::runtime::outcome::Ended::Ok if call.tool == "agent.run" => {
                            "started".to_string()
                        }
                        crate::runtime::outcome::Ended::Ok => "returned".to_string(),
                        crate::runtime::outcome::Ended::Threw { class } => {
                            format!("failed · {class}")
                        }
                        crate::runtime::outcome::Ended::Denied { rule } => {
                            format!("denied · {rule}")
                        }
                    };
                    format!(
                        "{} {}{} · {status}",
                        if i + 1 == record.calls.len() {
                            "└─"
                        } else {
                            "├─"
                        },
                        call.tool,
                        call.args
                            .get("path")
                            .map(|path| format!(
                                " {}",
                                std::path::Path::new(path)
                                    .file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                            ))
                            .or_else(|| call
                                .args
                                .get("command")
                                .map(|command| format!(" {command}")))
                            .or_else(|| call.args.get("source").map(|source| format!(" {source}")))
                            .unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        }),
        table: Some(turn.table.clone()),
        stdout: (!turn.stdout_tail.is_empty()).then(|| turn.stdout_tail.clone()),
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
        plan: turn.plan.clone(),
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
                position: ErrorSection::position_of(error.line, error.column),
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
    let answer = (!outcome.ends_the_task()).then(|| {
        let mut answer = prompt::render_result(&result);
        if let Some(failed) = runtime.syntax_failure() {
            answer.push_str("\n\n");
            answer.push_str(&failed.hint());
        }
        answer
    });

    Ok(Step {
        answer,
        response,
        prose: false,
        record: Some(record),
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
/// [`send_task_turn`], recovering from a conversation that no longer fits.
///
/// **Two rungs, and only an overflow reaches either.** The first is lossless:
/// drop from every older result the sections the newest one restates in full.
/// The second is not: throw the conversation away and start from a
/// checkpoint. Any other error is returned unchanged, so a malformed request
/// is never retried as though it were too long.
///
/// The second rung is the one a text harness cannot take. Its tool results
/// *are* its transcript, so dropping the transcript drops the work; pane's
/// objects live in the isolate, so the handle table after the checkpoint is
/// complete and every name in it still resolves. What is lost is the
/// narration.
fn send_task_turn_recovering(
    transcript: &mut Transcript,
    session: &Session<'_>,
    runtime: &Runtime,
    task: &str,
    rollout: &mut Rollout,
) -> Result<(wire::Turn, u64), String> {
    let first = match timed_send_task_turn(&transcript.conversation, session, task) {
        Ok(turn) => return Ok(turn),
        Err(error) if error.is_context_overflow() => error,
        Err(error) => return Err(format!("request failed: {error}")),
    };

    let report = prompt::compact_conversation(&mut transcript.conversation);
    if !report.is_empty() {
        session_println!(
            "context: the conversation did not fit; dropped {} redundant byte(s) from {} earlier              result(s) -- nothing was lost, each was restated in full by a later message",
            report.bytes,
            report.messages
        );
        match timed_send_task_turn(&transcript.conversation, session, task) {
            Ok(turn) => return Ok(turn),
            Err(error) if error.is_context_overflow() => {}
            Err(error) => return Err(format!("request failed: {error}")),
        }
    }

    // Second rung. The notebook is indexed by the conversation, so the two
    // are replaced together -- a notebook left behind would hang every
    // earlier cell's view under the wrong cell.
    let checkpoint = prompt::checkpoint(
        task,
        &runtime.plan(),
        &runtime.handle_names(),
        Some(&first.to_string()),
    );
    transcript.conversation.messages = vec![Message::text(Role::User, &checkpoint)];
    transcript.notebook = Notebook::default();
    write_turn(session.interrupt, rollout, Role::User, &checkpoint)
        .map_err(|e| format!("could not record the checkpoint: {e}"))?;
    session_println!(
        "context: still did not fit, so the conversation was replaced by a checkpoint; {} handle(s) \
         are still live and nothing was re-run",
        runtime.handle_names().len()
    );
    timed_send_task_turn(&transcript.conversation, session, task)
        .map_err(|error| format!("request failed after a checkpoint: {error}"))
}

// Measure only the successful request, excluding context recovery and UI work.
fn timed_send_task_turn(
    conversation: &Conversation,
    session: &Session<'_>,
    task: &str,
) -> Result<(wire::Turn, u64), wire::WireError> {
    let start = Instant::now();
    let turn = send_task_turn(conversation, session, task)?;
    let elapsed = start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    Ok((turn, elapsed))
}

fn send_task_turn(
    conversation: &Conversation,
    session: &Session<'_>,
    task: &str,
) -> Result<wire::Turn, wire::WireError> {
    let model = session.model.borrow();
    let request = prompt::with_task_context(conversation, &model, task);
    let conversation = &request;
    if let Some(ui) = session.ui {
        wire::send_turn_streaming_configured(
            conversation,
            &model,
            session.effort.get(),
            &mut |text| ui.append_delta(text),
        )
    } else {
        wire::send_turn_configured(conversation, &model, session.effort.get())
    }
}

fn estimate_request_tokens(conversation: &Conversation, model: &str) -> u64 {
    estimate_task_request_tokens(conversation, model, "")
}

fn estimate_task_request_tokens(conversation: &Conversation, model: &str, task: &str) -> u64 {
    let request = prompt::with_task_context(conversation, model, task);
    let body = wire::request_body_on_model(&request, model);
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
fn answer_command(
    rest: &str,
    name: &str,
    argument: Option<&str>,
    session: &Session<'_>,
    transcript: &Transcript,
) {
    if controls::command(name, argument, session, transcript) {
        return;
    }
    // A bare `/` or `/help` lists rather than resolves, so `commands::all`
    // has a caller and the binary actually *offers* what 2450 names.
    if name == "model" {
        if let Some(model) = argument.filter(|value| !value.is_empty()) {
            if model.chars().any(char::is_whitespace) {
                session_println!("/model expects one model name");
                return;
            }
            *session.model.borrow_mut() = model.into();
            if !model.contains("claude")
                && matches!(
                    session.effort.get(),
                    wire::Effort::Xhigh | wire::Effort::Max
                )
            {
                session.effort.set(wire::Effort::Auto);
                if let Some(ui) = session.ui {
                    ui.effort(wire::Effort::Auto);
                }
            }
            if let Some(ui) = session.ui {
                ui.model(model);
            }
            session_println!("model changed to {model}");
        } else {
            controls::models(session);
        }
        return;
    }
    if name.is_empty() || name == "help" {
        offer_commands(session);
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
                    let description = match resolved.source {
                        CommandSource::ProjectSkill => "project skill",
                        CommandSource::ProjectCommand => "project command",
                        CommandSource::BuiltIn(_) => "command",
                    };
                    session_println!(
                        "/{name}: {description} found; running it is not supported yet"
                    );
                }
            }
            CommandStatus::NotBuilt { subphase } => {
                session_println!("/{name} is not built yet -- that is sub-phase {subphase}");
            }
        },
        None => session_println!("/{name}: unknown command"),
    }
}

/// Prints every command `commands::all` names, in its own order -- the
/// built-ins, then the project's own commands and skills. `all` had a
/// production caller nowhere before this package; this is that caller.
fn offer_commands(session: &Session<'_>) {
    let mut lines: Vec<String> = tui::slash_matches("/")
        .into_iter()
        .map(|(name, help)| format!("{name:<16} {help}"))
        .collect();
    for command in commands::all(session.project) {
        if !lines
            .iter()
            .any(|line| line.starts_with(&format!("/{} ", command.name)))
        {
            lines.push(format!("/{}", command.name));
        }
    }
    controls::show(session, tui::Panel::text("Commands", lines.join("\n")));
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
            Ok(()) => session_println!("/memory: saved"),
            Err(e) => session_println!("/memory: could not save: {e}"),
        }
        return;
    }

    let notes = glasshouse::search_memory(glasshouse, memory, "");
    if notes.is_empty() {
        session_println!("/memory: no notes");
    } else {
        for note in &notes {
            session_println!("/memory: {note}");
        }
    }
    match glasshouse::checkpoint(glasshouse, memory) {
        Some(checkpoint) => session_println!("/memory checkpoint: {checkpoint}"),
        None => session_println!("/memory checkpoint: none"),
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
fn compile_profile_once(project: &ProjectConfig, yolo: bool) -> Profile {
    let profile = if yolo {
        session_println!(
            "sandbox: --yolo — the project root and every command line are granted; \
             .claude/settings.json is ignored and the never-grantable set still applies"
        );
        Profile::compile(&project.root, Some(&yolo_settings(&project.root)))
    } else {
        Profile::from_project(project)
    };
    session_println!(
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
        session_println!("sandbox: {diagnostic}");
    }
    profile
}

/// The settings document `--yolo` compiles instead of the project's own.
///
/// **The invariant: this is an ordinary settings document and nothing else.**
/// `--yolo` adds no grant kind and no bypass inside `Profile`, so every rule
/// the compiler already enforces — §4's never-grantable set above all — is
/// enforced here identically. `Bash` bare is the spec's own "every command
/// line admitted"; the three path patterns are the project root's closure.
fn yolo_settings(root: &std::path::Path) -> String {
    let root = root.display().to_string().replace('\\', "\\\\");
    format!(
        r#"{{"permissions":{{"allow":["Read({root}/**)","Write({root}/**)","Edit({root}/**)","Bash"]}}}}"#
    )
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
    if session.mode.get() == tui::Mode::Plan {
        session_println!("Planning mode does not execute tools. Use /mode execute first.");
        return;
    }
    let Some((tool, args)) = parse_tool_line(rest) else {
        session_println!(
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
            session_println!("{}{}", result.stdout, result.stderr);
            session_println!(
                "/tool {tool}: exit {} under {}",
                result
                    .exit_code
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "signal".to_string()),
                result.confinement.as_str()
            );
        }
        Err(ToolError::Denied(denied)) => session_println!("{denied}"),
        Err(error) => session_println!("/tool {tool}: {error}"),
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

    /// One `bg.done`, the only kind §5 produces.
    fn bg_done(source: &str) -> crate::events::Event {
        use crate::events::{Event, Kind, PayloadRef, Priority};
        Event::pending(
            Kind::BgDone {
                emission: source.to_string(),
            },
            source,
            crate::events::now(),
            PayloadRef::new(format!("{source}#exit")),
            Priority::Batch,
            "a job finished",
        )
    }

    /// `events-contract.md` §4: **"a turn with an empty batch and no user
    /// input does not happen: the runtime waits."**
    ///
    /// An empty window is the case where there is nothing to wait *for*: it
    /// has no deadline and can never close, so no batch is ever delivered and
    /// the turn that follows happens because a user message asked for it. A
    /// batch never composes a turn, which is why an empty one cannot.
    ///
    /// **This is also what keeps the test from hanging**, and it is the
    /// property being asserted rather than a convenience: the budget is
    /// thirty seconds and is never reached, because an empty window is
    /// answered on the first pass. A `next_batch` that waited out its budget
    /// here would fail this test rather than slow it down.
    #[test]
    fn an_empty_window_delivers_nothing_and_waits_for_nothing() {
        let session = SessionId::new("next-batch-empty");
        let mut window = Window::new(WindowConfig::default());
        let started = Instant::now();
        assert!(
            next_batch(&mut window, &session, Duration::from_secs(30)).is_none(),
            "an empty window produced a batch"
        );
        assert!(
            started.elapsed() < Duration::from_millis(200),
            "an empty window was waited on for {:?}; it can never close",
            started.elapsed()
        );
        bg::shutdown(&session);
    }

    /// Line 2481's first clause: **an event does not get a turn of its own
    /// while a batch window is open.** A window holding an event that has not
    /// reached §2's deadline is waited on, not delivered — and the wait is
    /// bounded by the budget, which is the only reason this test terminates
    /// if the deadline logic is ever wrong.
    #[test]
    fn an_open_window_is_waited_on_rather_than_delivered_in_pieces() {
        let session = SessionId::new("next-batch-open");
        let mut window = Window::new(WindowConfig::default());
        window.accept(bg_done("bg/job1"), crate::events::now());
        let started = Instant::now();
        assert!(
            next_batch(&mut window, &session, Duration::from_millis(200)).is_none(),
            "an open window was delivered before §2's deadline closed it"
        );
        assert!(
            started.elapsed() >= Duration::from_millis(200),
            "the open window was not waited on at all: {:?}",
            started.elapsed()
        );
        bg::shutdown(&session);
    }

    /// And the other side of it: once the window is due, **both** events
    /// arrive as one batch — the second never became a turn of its own.
    #[test]
    fn a_due_window_delivers_every_event_it_holds_as_one_batch() {
        use crate::events::Stamp;
        let session = SessionId::new("next-batch-due");
        let mut window = Window::new(WindowConfig::default());
        let long_ago = Stamp::from_millis(crate::events::now().as_millis() - 3_000);
        window.accept(bg_done("bg/job1"), long_ago);
        window.accept(bg_done("bg/job2"), long_ago);
        let batch = next_batch(&mut window, &session, Duration::from_millis(200))
            .expect("a window past its deadline closes");
        assert_eq!(batch.n, 2, "the events were split across two deliveries");
        bg::shutdown(&session);
    }

    #[test]
    fn two_arguments_are_kept_apart() {
        let (tool, args) = parse_tool_line("grep pattern=fn path=/tmp").unwrap();
        assert_eq!(tool, "grep");
        assert_eq!(args.get("pattern"), Some("fn"));
        assert_eq!(args.get("path"), Some("/tmp"));
    }
}
