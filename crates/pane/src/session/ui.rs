//! One thread owns the terminal and keys; the task thread only sends view state.
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::contract::{Conversation, ServedBy};
use crate::tui::{self, Activity, Notebook, ScreenState, SidebarVisibility};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton};
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste, MouseEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::{Terminal, backend::CrosstermBackend};

mod console_mode;
mod links;
mod terminal_input;

static ACTIVE: AtomicBool = AtomicBool::new(false);
static DRAWING: Mutex<()> = Mutex::new(());
thread_local! { static OUTPUT: RefCell<Option<mpsc::Sender<Update>>> = const { RefCell::new(None) }; }
thread_local! { static STARTUP: RefCell<Option<Vec<String>>> = const { RefCell::new(None) }; }

pub(super) fn output(message: String) {
    if super::output::active() {
        eprintln!("{message}");
        return;
    }
    OUTPUT.with(|output| {
        if let Some(sender) = output.borrow().as_ref() {
            let _ = sender.send(Update::Notice(message));
        } else if let Some(message) = STARTUP.with(|held| match held.borrow_mut().as_mut() {
            Some(notes) => {
                notes.push(message);
                None
            }
            None => Some(message),
        }) {
            println!("{message}");
        }
    });
}

/// A note a terminal session shows elsewhere (its footer), and so does not
/// repeat in the conversation; every other session prints it.
pub(super) fn detail(message: String) {
    if STARTUP.with(|held| held.borrow().is_none()) {
        output(message);
    }
}

/// Holds the notes a session says before its terminal UI exists, so they
/// open the conversation instead of flashing on the screen the UI replaces.
/// [`LiveUi::start`] takes them; a session that ends before it prints them
/// when this guard drops, so a refusal still arrives under its notes.
pub(super) struct StartupNotes;
impl StartupNotes {
    pub(super) fn hold() -> Self {
        STARTUP.with(|held| *held.borrow_mut() = Some(Vec::new()));
        Self
    }
}
impl Drop for StartupNotes {
    fn drop(&mut self) {
        for note in STARTUP
            .with(|held| held.borrow_mut().take())
            .unwrap_or_default()
        {
            println!("{note}");
        }
    }
}

/// **Ask for exactly the mouse reports the UI consumes.** `?1000` is
/// press/release, `?1002` is motion **while a button is held**, and `?1006`
/// is the SGR encoding all three are parsed from. `?1002` was deliberately
/// absent until 2026-09-18, when the user ruled that a plain click-and-drag
/// must select: a terminal offers its own selection only behind a modifier
/// while reporting is on, so Pane draws the selection itself and needs to see
/// the drag. Still **not** `?1003` (any motion), which reports every pointer
/// movement over the window whether or not anything is pressed, and each
/// report no arm consumes is another chance for a read boundary to split one
/// into text (`terminal_input`). Crossterm has no command for this set, so
/// the bytes are written directly.
///
/// **On Windows these bytes are necessary and not sufficient**, which is why
/// [`enable_mouse_reporting`] exists. They reach the ConPTY emulator and tell
/// it to accept mouse input from the terminal outside it, but crossterm reads
/// Windows input as console records rather than as bytes, and a record only
/// reaches it once `ENABLE_MOUSE_INPUT` is set on the console handle — which
/// is all `EnableMouseCapture` does there (`is_ansi_code_supported` is `false`
/// on Windows, so it writes no `?1002`/`?1003` either). Without that call
/// pane's wheel did nothing on Windows at all.
const ENABLE_MOUSE_REPORTING: &[u8] = b"\x1b[?1000h\x1b[?1002h\x1b[?1006h";
/// The matching resets, in the same order.
const DISABLE_MOUSE_REPORTING: &[u8] = b"\x1b[?1000l\x1b[?1002l\x1b[?1006l";

/// The opening of the drag-selection notice, which replaces its own
/// predecessor rather than stacking under it.
const COPIED: &str = "Copied ";

/// The longest the screen goes without a frame while input keeps arriving.
/// A drag or a wheel delivers events faster than a full transcript re-render
/// takes, so the loop draws once per *batch* of input; this is the bound
/// that keeps a continuous stream from starving the screen entirely.
const FRAME: Duration = Duration::from_millis(50);

/// Request mouse reporting in both of the spellings a host can need.
fn enable_mouse_reporting() -> io::Result<()> {
    io::stdout().write_all(ENABLE_MOUSE_REPORTING)?;
    io::stdout().flush()?;
    #[cfg(windows)]
    execute!(io::stdout(), crossterm::event::EnableMouseCapture)?;
    Ok(())
}

/// The reverse, and it must run **before** `disable_raw_mode`: crossterm's
/// `DisableMouseCapture` restores the whole console input mode that was
/// captured when capture was enabled, and that snapshot was already raw — so
/// undoing it afterwards would hand the console straight back to raw mode.
fn disable_mouse_reporting() {
    #[cfg(windows)]
    let _ = execute!(io::stdout(), crossterm::event::DisableMouseCapture);
    let _ = io::stdout().write_all(DISABLE_MOUSE_REPORTING);
    let _ = io::stdout().flush();
}

/// Also called by the existing second-SIGINT exit path, which skips Drop.
pub(super) fn restore_terminal() {
    let _guard = super::lock(&DRAWING);
    if ACTIVE.swap(false, Ordering::SeqCst) {
        disable_mouse_reporting();
        console_mode::disable();
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), DisableBracketedPaste);
        let _ = execute!(io::stdout(), LeaveAlternateScreen, crossterm::cursor::Show);
    }
}
struct Restore;
impl Drop for Restore {
    fn drop(&mut self) {
        restore_terminal();
    }
}

/// The next line of the session's stdin, `None` at end of input.
///
/// **One line per lock, rather than `stdin().lock().lines()`.** A lock held
/// for a whole read loop is not reentrant, and `/key` reads the key's own
/// line from inside the input that asked for it -- which would deadlock
/// against a loop still holding the lock it was called from.
pub(super) fn read_line() -> io::Result<Option<String>> {
    let mut line = String::new();
    if io::stdin().read_line(&mut line)? == 0 {
        return Ok(None);
    }
    line.truncate(line.trim_end_matches('\n').trim_end_matches('\r').len());
    Ok(Some(line))
}

pub(super) enum Update {
    Approval(crate::approval::Request),
    /// A question a cell put to the person, waiting on the session thread.
    Ask(crate::ask::Request),
    Snapshot(Box<(Conversation, Notebook, ServedBy, Activity)>),
    /// Open a modal masked prompt with this title. The terminal thread
    /// answers it on the secret channel and on nothing else.
    SecretPrompt(String),
    Model(String),
    Delta(String),
    ToolDelta(String),
    Mode(tui::Mode),
    Effort(crate::wire::Effort),
    Panel(Box<tui::Panel>),
    Notice(String),
    Stop,
}
enum Input {
    Submit(String),
    Exit,
    Failed(String),
}

/// The two channels the terminal thread answers on. A masked prompt's reply
/// has its own, so a secret cannot arrive where a message is expected.
struct Answers<'a> {
    inputs: &'a mpsc::Sender<Input>,
    secrets: &'a mpsc::Sender<Option<String>>,
}

pub(super) struct LiveUi {
    handler_cancellations: Arc<Mutex<Vec<String>>>,
    updates: mpsc::Sender<Update>,
    inputs: mpsc::Receiver<Input>,
    /// Answers to [`Update::SecretPrompt`], on their own channel: a secret
    /// must not be able to arrive as an `Input` and be taken for a message.
    secrets: mpsc::Receiver<Option<String>>,
    thread: Option<JoinHandle<()>>,
}
impl LiveUi {
    /// Forwards suspended exact actions to the terminal owner. Closing the
    /// terminal drops pending requests and denies their waiting callbacks.
    pub(super) fn approval_gate(
        &self,
        ladder: crate::permissions::Ladder,
    ) -> crate::approval::Gate {
        let (gate, receiver) = crate::approval::Gate::channel(ladder);
        let updates = self.updates.clone();
        thread::spawn(move || {
            for request in receiver {
                if updates.send(Update::Approval(request)).is_err() {
                    break;
                }
            }
        });
        gate
    }
    /// Forwards questions a cell asked to the terminal owner. Closing the
    /// terminal drops the pending question, which the session reads as
    /// nobody having answered -- never as a reason to wait.
    pub(super) fn ask_gate(&self) -> crate::ask::Gate {
        let (gate, receiver) = crate::ask::Gate::channel();
        let updates = self.updates.clone();
        thread::spawn(move || {
            for request in receiver {
                if updates.send(Update::Ask(request)).is_err() {
                    break;
                }
            }
        });
        gate
    }

    pub(super) fn start(
        mut state: ScreenState,
        conversation: Conversation,
        notebook: Notebook,
    ) -> Result<Self, String> {
        state.messages_seen = conversation.messages.len();
        for note in STARTUP
            .with(|held| held.borrow_mut().take())
            .unwrap_or_default()
        {
            state.note(note);
        }
        let (updates, receiver) = mpsc::channel();
        let (input_sender, inputs) = mpsc::channel();
        let (secret_sender, secrets) = mpsc::channel();
        let (ready_sender, ready) = mpsc::sync_channel(1);
        let handler_cancellations = Arc::new(Mutex::new(Vec::new()));
        let commands = handler_cancellations.clone();
        let thread = thread::spawn(move || {
            let result = run(
                state,
                conversation,
                notebook,
                receiver,
                Answers {
                    inputs: &input_sender,
                    secrets: &secret_sender,
                },
                ready_sender,
                commands,
            );
            if let Err(error) = result {
                let _ = input_sender.send(Input::Failed(error.to_string()));
            }
        });
        ready
            .recv()
            .map_err(|_| "terminal thread exited during setup".to_string())??;
        OUTPUT.with(|slot| *slot.borrow_mut() = Some(updates.clone()));
        Ok(Self {
            handler_cancellations,
            updates,
            inputs,
            secrets,
            thread: Some(thread),
        })
    }
    /// Opens the modal masked prompt and blocks until it is answered:
    /// `Some` is what was entered, `None` an Esc or a terminal that went
    /// away. **Nothing typed into it reaches the editor, the transcript or
    /// the input history** -- it comes back here and nowhere else.
    pub(super) fn secret(&self, title: &str) -> Option<String> {
        // A paste nobody collected (a sign-in that ended first) must never
        // answer a prompt for a key.
        while self.secrets.try_recv().is_ok() {}
        self.updates.send(Update::SecretPrompt(title.into())).ok()?;
        self.secrets.recv().ok().flatten()
    }
    /// What the person entered in a prompt the terminal opened on its own,
    /// such as a sign-in panel's paste row, if anything has arrived.
    pub(super) fn try_secret(&self) -> Option<String> {
        self.secrets.try_recv().ok().flatten()
    }
    pub(super) fn handler_cancellations(&self) -> Vec<String> {
        std::mem::take(&mut *super::lock(&self.handler_cancellations))
    }
    pub(super) fn next(&self) -> Result<Option<String>, String> {
        match self.inputs.recv() {
            Ok(Input::Submit(text)) => Ok(Some(text)),
            Ok(Input::Exit) => Ok(None),
            Ok(Input::Failed(error)) => Err(error),
            Err(_) => Err("terminal input closed".into()),
        }
    }
    pub(super) fn publish(
        &self,
        transcript: &super::Transcript,
        served: &ServedBy,
        activity: Activity,
    ) {
        let _ = self.updates.send(Update::Snapshot(Box::new((
            transcript.conversation.clone(),
            transcript.notebook.clone(),
            served.clone(),
            activity,
        ))));
    }
    /// A publisher onto this terminal's channel that borrows nothing.
    pub(super) fn publisher(&self) -> Publisher {
        Publisher {
            updates: self.updates.clone(),
        }
    }
    pub(super) fn append_delta(&self, text: &str) {
        let _ = self.updates.send(Update::Delta(text.into()));
    }
    pub(super) fn tool_delta(&self, fragment: &str) {
        let _ = self.updates.send(Update::ToolDelta(fragment.into()));
    }
    pub(super) fn effort(&self, effort: crate::wire::Effort) {
        let _ = self.updates.send(Update::Effort(effort));
    }
    pub(super) fn mode(&self, mode: tui::Mode) {
        let _ = self.updates.send(Update::Mode(mode));
    }
    pub(super) fn panel(&self, panel: tui::Panel) {
        let _ = self.updates.send(Update::Panel(Box::new(panel)));
    }
    pub(super) fn model(&self, model: &str) {
        let _ = self.updates.send(Update::Model(model.into()));
    }
}
/// Publishes a snapshot while holding no borrow of the [`LiveUi`] it came from.
///
/// The invariant: a caller deeper in the stack than the session loop can draw
/// the screen. A cell blocks the task thread for as long as it runs, so
/// anything it wants shown while it runs -- a helper call in flight -- must
/// publish through a handle it owns; the channel is already `Send`-free and
/// cheap to clone, so this is that same channel without the borrow.
#[derive(Clone)]
pub(super) struct Publisher {
    updates: mpsc::Sender<Update>,
}
impl Publisher {
    pub(super) fn publish(
        &self,
        conversation: &Conversation,
        notebook: &Notebook,
        served: &ServedBy,
        activity: Activity,
    ) {
        let _ = self.updates.send(Update::Snapshot(Box::new((
            conversation.clone(),
            notebook.clone(),
            served.clone(),
            activity,
        ))));
    }
}

/// A publisher with no terminal thread behind it, paired with the receiving
/// end, so `session`'s tests can read what a publish would have drawn.
#[cfg(test)]
pub(super) fn test_publisher() -> (Publisher, mpsc::Receiver<Update>) {
    let (updates, receiver) = mpsc::channel();
    (Publisher { updates }, receiver)
}

impl Drop for LiveUi {
    fn drop(&mut self) {
        OUTPUT.with(|slot| *slot.borrow_mut() = None);
        let _ = self.updates.send(Update::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Default)]
struct Editor {
    text: String,
    cursor: usize,
    selected: usize,
    history: Vec<String>,
    history_index: Option<usize>,
    draft: String,
}
impl Editor {
    fn previous(&self) -> usize {
        self.text[..self.cursor]
            .char_indices()
            .last()
            .map(|(i, _)| i)
            .unwrap_or(0)
    }
    fn next(&self) -> usize {
        self.text[self.cursor..]
            .chars()
            .next()
            .map(|c| self.cursor + c.len_utf8())
            .unwrap_or(self.cursor)
    }
    fn insert(&mut self, text: &str) {
        // Terminal transports may turn pasted LF into CR. Preserve either
        // newline convention as one LF, while stripping other controls.
        let mut text = text.chars().peekable();
        let mut normalized = String::with_capacity(text.size_hint().0);
        while let Some(c) = text.next() {
            match c {
                '\r' => {
                    if text.peek() == Some(&'\n') {
                        text.next();
                    }
                    normalized.push('\n');
                }
                '\n' | '\t' => normalized.push(c),
                c if !c.is_control() => normalized.push(c),
                _ => {}
            }
        }
        self.text.insert_str(self.cursor, &normalized);
        self.cursor += normalized.len();
        self.selected = 0;
    }
    fn recall(&mut self, older: bool) {
        if self.history.is_empty() {
            return;
        }
        let index = match (self.history_index, older) {
            (None, true) => {
                self.draft = self.text.clone();
                Some(self.history.len() - 1)
            }
            (Some(i), true) => Some(i.saturating_sub(1)),
            (Some(i), false) if i + 1 < self.history.len() => Some(i + 1),
            _ => None,
        };
        self.text = index
            .map(|i| self.history[i].clone())
            .unwrap_or_else(|| self.draft.clone());
        self.history_index = index;
        self.cursor = self.text.len();
        self.selected = 0;
    }
    fn take(&mut self) -> String {
        let text = std::mem::take(&mut self.text);
        if self.history.last() != Some(&text) {
            self.history.push(text.clone());
        }
        self.cursor = 0;
        self.selected = 0;
        self.history_index = None;
        self.draft.clear();
        text
    }
    fn key(&mut self, key: KeyEvent) -> bool {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('p') if control => self.recall(true),
            KeyCode::Char('n') if control => self.recall(false),
            KeyCode::Char('a') if control => self.cursor = 0,
            KeyCode::Char('e') if control => self.cursor = self.text.len(),
            KeyCode::Char('u') if control => {
                self.text.drain(..self.cursor);
                self.cursor = 0;
            }
            KeyCode::Char('k') if control => {
                self.text.truncate(self.cursor);
            }
            KeyCode::Left => self.cursor = self.previous(),
            KeyCode::Right => self.cursor = self.next(),
            KeyCode::Home => {
                self.cursor = self.text[..self.cursor]
                    .rfind('\n')
                    .map(|i| i + 1)
                    .unwrap_or(0)
            }
            KeyCode::End => {
                self.cursor = self.text[self.cursor..]
                    .find('\n')
                    .map(|i| self.cursor + i)
                    .unwrap_or(self.text.len())
            }
            KeyCode::Backspace => {
                let prev = self.previous();
                self.text.drain(prev..self.cursor);
                self.cursor = prev;
                self.selected = 0;
            }
            KeyCode::Delete => {
                self.text.drain(self.cursor..self.next());
                self.selected = 0;
            }
            KeyCode::Up | KeyCode::Down if !tui::slash_matches(&self.text).is_empty() => {
                let count = tui::slash_matches(&self.text).len();
                self.selected = if key.code == KeyCode::Down {
                    (self.selected + 1) % count
                } else {
                    (self.selected + count - 1) % count
                };
            }
            KeyCode::Up => self.recall(true),
            KeyCode::Down => self.recall(false),
            KeyCode::Tab => {
                if let Some((name, _)) = tui::slash_matches(&self.text).get(self.selected) {
                    self.text = format!("{name} ");
                    self.cursor = self.text.len();
                    self.selected = 0;
                }
            }
            KeyCode::Enter
                if key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT) =>
            {
                self.insert("\n")
            }
            KeyCode::Enter => {
                if let Some((name, _)) = tui::slash_matches(&self.text).get(self.selected) {
                    self.text = name.clone();
                    self.cursor = self.text.len();
                }
                return true;
            }
            KeyCode::Char(c) if !control && !key.modifiers.contains(KeyModifiers::ALT) => {
                self.insert(&c.to_string())
            }
            _ => {}
        }
        false
    }
}

/// Dates every helper call still in flight from the frame it first appeared
/// in, and writes that wall clock into the copy of the notebook this thread
/// is about to draw.
///
/// The invariant: **a running call's elapsed comes from the clock, not from
/// its record.** The cell owns the task thread until the call returns, so
/// the record reaches the screen once, with `elapsed_ms` still zero, and
/// cannot be republished while it runs. `little-helpers.md` makes elapsed
/// text rather than animation precisely so it keeps counting under
/// `/motion off`, where the glyph is frozen and *is it alive* is the only
/// question left. Presentation only: this notebook is the terminal thread's
/// own clone, and a resolved record carries its real duration already.
fn tick_helper_clocks(notebook: &mut Notebook, since: &mut HashMap<(usize, usize), Instant>) {
    const PREFLIGHT: (usize, usize) = (usize::MAX, 0);
    since.retain(|(cell, call), _| {
        if (*cell, *call) == PREFLIGHT {
            return notebook
                .preflight
                .as_ref()
                .is_some_and(tui::helper_in_flight);
        }
        notebook
            .cells
            .get(*cell)
            .and_then(|cell| cell.helpers.get(*call))
            .is_some_and(tui::helper_in_flight)
    });
    if let Some(record) = notebook.preflight.as_mut()
        && tui::helper_in_flight(record)
    {
        let started = since.entry(PREFLIGHT).or_insert_with(Instant::now);
        record.outcome.elapsed_ms = started.elapsed().as_millis() as u64;
    }
    for (index, cell) in notebook.cells.iter_mut().enumerate() {
        for (call, record) in cell.helpers.iter_mut().enumerate() {
            if tui::helper_in_flight(record) {
                let started = since.entry((index, call)).or_insert_with(Instant::now);
                record.outcome.elapsed_ms = started.elapsed().as_millis() as u64;
            }
        }
    }
}

/// Opens a cell's inspection: **the one path `/cell <n>` and a click on that
/// cell's header both take**, so the two routes cannot drift into doing
/// different things (`tui::hit`: every click has a keyboard twin).
fn open_cell(state: &mut ScreenState, notebook: &Notebook, cell: usize) {
    state.inspection = tui::Inspection::open(cell, notebook);
    state.telemetry_open = false;
    state.panel = None;
    if state.inspection.is_none() {
        state.note("No recorded cell at that number yet. Use /cells after an action.");
    }
}

/// Shift-Tab: one rung along the permission ladder, and the line that says
/// where it landed.
///
/// **It takes effect at once, task or no task.** The ladder is an atomic the
/// approval gate reads on the session thread, so a person who moves *down*
/// mid-task is asked about the very next call; moving *up* is their own act
/// and the ladder records it for the rollout.
fn rung_change(ladder: &crate::permissions::Ladder) -> String {
    let moved = ladder.cycle();
    format!(
        "permissions {} — Shift-Tab cycles, /permissions <rung> sets one",
        moved.to.name()
    )
}

/// The input a click on the mode field sends, or the sentence it shows
/// instead while a task is running.
///
/// Shift-Tab no longer sends this — it moves the permission rung — so the
/// request mode's keyboard route is `/mode`, which this input is.
fn mode_change(busy: bool, mode: tui::Mode) -> Result<String, &'static str> {
    if busy {
        return Err("Change mode after the current task finishes.");
    }
    Ok(format!("/mode {}", mode.next().name()))
}

/// Turns mouse reporting on or off, sets the state the status line draws
/// from, and says what just happened.
///
/// **What capture actually takes, and what it leaves.** Pane asks for
/// [`ENABLE_MOUSE_REPORTING`] — `?1000` press/release and the `?1006` SGR
/// encoding — and deliberately **not** `?1002`/`?1003` motion tracking, so a
/// pointer dragged across the window is never reported to Pane. What the
/// terminal does with that drag is then the terminal's own decision: most
/// keep Shift for native selection whatever the application asked for, and
/// several select on an unmodified drag too once no motion is requested. That
/// is a terminal's behaviour, not a promise this program can make, which is
/// the whole reason for the explicit release below.
///
/// **Released, the terminal owns the pointer again and no click reaches
/// Pane** — including a click on the marker that would take it back, which is
/// why both routes are keys: `/mouse` and Ctrl-G.
fn set_mouse_capture(state: &mut tui::ScreenState, on: bool) {
    if on {
        if enable_mouse_reporting().is_err() {
            state.note("This terminal did not accept the mouse-mode change.");
            return;
        }
    } else {
        disable_mouse_reporting();
    }
    state.mouse_off = !on;
    state.note(if on {
        "Mouse on: click to open, drag to select. /mouse or Ctrl-G hands it back."
    } else {
        "Mouse released: the terminal owns the pointer. /mouse or Ctrl-G takes it back."
    });
}

/// Applies one panel hit. The hit test itself now lives in
/// `tui::ScreenGeometry`, which asks the panel first because it is drawn over
/// the transcript.
fn select_panel_at(panel: &mut tui::Panel, hit: tui::PanelHit) -> bool {
    match hit {
        tui::PanelHit::Provider(index) => panel.select_provider(index),
        tui::PanelHit::Model(index) => panel.select_model_row(index),
        // The roster's whole point over Tab: the tier you want is already on
        // screen, so reaching it is one click rather than up to two cycles.
        tui::PanelHit::Tier(tier) => panel.select_tier(tier),
        tui::PanelHit::Order(order) => panel.select_order(order),
        tui::PanelHit::Mode(index) => {
            panel.select_model_row(index);
            panel.stage();
            true
        }
    }
}

fn run(
    mut state: ScreenState,
    mut conversation: Conversation,
    mut notebook: Notebook,
    updates: mpsc::Receiver<Update>,
    answers: Answers<'_>,
    ready: mpsc::SyncSender<Result<(), String>>,
    handler_cancellations: Arc<Mutex<Vec<String>>>,
) -> io::Result<()> {
    let setup = (|| {
        let _guard = super::lock(&DRAWING);
        enable_raw_mode()?;
        let console = console_mode::select();
        ACTIVE.store(true, Ordering::SeqCst);
        execute!(io::stdout(), EnterAlternateScreen, EnableBracketedPaste)?;
        enable_mouse_reporting()?;
        Terminal::new(CrosstermBackend::new(io::stdout())).map(|terminal| (terminal, console))
    })();
    let _restore = Restore;
    let (mut terminal, console) = match setup {
        Ok(ready_terminal) => {
            let _ = ready.send(Ok(()));
            ready_terminal
        }
        Err(error) => {
            let _ = ready.send(Err(error.to_string()));
            return Err(error);
        }
    };
    let mut input = terminal_input::TerminalInput::new(console);
    let mut editor = Editor::default();
    let mut served = ServedBy::default();
    let mut busy = false;
    let started = Instant::now();
    state.activity = Activity::Starting;
    let mut dirty = true;
    let mut last_drawn = Instant::now();
    let mut last_tick = Instant::now();
    let mut task_started: Option<Instant> = None;
    let mut helper_clocks: HashMap<(usize, usize), Instant> = HashMap::new();
    let mut previous_rows = 0usize;
    let mut viewport_height = 10usize;
    // Where the left button went down, so a release knows whether the
    // gesture was a click or a drag.
    let mut pressed_at: Option<(u16, u16)> = None;
    let mut geometry = tui::ScreenGeometry::default();
    // When the position indicator came up, so it can go down again after
    // `tui::SCROLL_INDICATOR_LINGER` without a timer of its own.
    let mut last_scroll: Option<Instant> = None;
    let mut approvals: std::collections::VecDeque<crate::approval::Request> =
        std::collections::VecDeque::new();
    let mut approval_scroll = 0u16;
    // At most one question is ever outstanding: `ask` ends the cell that
    // asked, and the session waits for the answer before the next turn.
    let mut asking: Option<crate::ask::Request> = None;
    let mut ask_selected = 0usize;
    let mut settings_editor: Option<crate::settings_session::Editor> = None;
    loop {
        if !ACTIVE.load(Ordering::SeqCst) {
            break;
        }
        let queued = approvals.len();
        approvals.retain(crate::approval::Request::is_pending);
        if approvals.len() != queued {
            approval_scroll = 0;
            dirty = true;
        }
        for update in updates.try_iter() {
            dirty = true;
            match update {
                Update::Approval(request) => {
                    approvals.push_back(request);
                    state.panel = None;
                    state.inspection = None;
                    approval_scroll = 0;
                }
                Update::Ask(request) => {
                    // The decision model's own pick starts selected, so the
                    // common answer is Enter and the person reads rather
                    // than navigates.
                    ask_selected = request
                        .weights()
                        .and_then(|weights| {
                            request
                                .question()
                                .choices
                                .iter()
                                .position(|choice| choice == &weights.choice)
                        })
                        .unwrap_or(0);
                    asking = Some(request);
                    state.panel = None;
                    state.inspection = None;
                }
                Update::Snapshot(snapshot) => {
                    let (c, n, s, activity) = *snapshot;
                    let completed = n
                        .cells
                        .iter()
                        .filter(|cell| cell.execution.is_some())
                        .count();
                    let previous = notebook
                        .cells
                        .iter()
                        .filter(|cell| cell.execution.is_some())
                        .count();
                    if completed > previous
                        && !state.reduced_motion
                        && n.cells.last().is_some_and(|cell| {
                            cell.error.is_none()
                                && cell.execution.as_deref().is_some_and(|calls| {
                                    !calls.contains(" · failed") && !calls.contains(" · denied")
                                })
                        })
                    {
                        state.completion_tick = Some(0);
                    }
                    refresh_handler_panel(&mut state.panel, &notebook.handlers, &n.handlers);
                    conversation = c;
                    state.messages_seen = conversation.messages.len();
                    notebook = n;
                    if s.is_known() {
                        served = s;
                    }
                    state.activity = activity;
                    state.streaming_text = None;
                    state.streaming_tool_input = None;
                    if served.is_known() {
                        state.connected = Some(true);
                    }
                    if matches!(
                        activity,
                        Activity::Idle | Activity::Complete | Activity::Failed
                    ) {
                        // Cancellation may finish the waiting callback before
                        // the user answers. Remove stale confirmations then.
                        approvals.clear();
                        approval_scroll = 0;
                        if let Some(start) = task_started.take() {
                            state.pulse.elapsed_ms = start.elapsed().as_millis() as u64;
                        }
                    } else if task_started.is_none() {
                        task_started = Some(Instant::now());
                    }
                    busy = matches!(
                        activity,
                        Activity::Thinking
                            | Activity::Streaming
                            | Activity::Executing
                            | Activity::Searching
                            | Activity::Waiting
                            | Activity::Compacting
                    );
                }
                Update::Delta(text) => {
                    state.pulse.receive(text.len());
                    state
                        .streaming_text
                        .get_or_insert_with(String::new)
                        .push_str(&text);
                    state.activity = Activity::Streaming;
                    busy = true;
                }
                Update::ToolDelta(fragment) => {
                    state.pulse.receive(fragment.len());
                    state
                        .streaming_tool_input
                        .get_or_insert_with(String::new)
                        .push_str(&fragment);
                    state.activity = Activity::Streaming;
                    busy = true;
                }
                Update::Model(model) => state.model = Some(model),
                Update::Mode(mode) => state.mode = mode,
                Update::Effort(effort) => state.effort = effort,
                Update::Panel(panel) => {
                    state.panel = Some(replace_panel(state.panel.as_ref(), *panel))
                }
                Update::Notice(message) => state.note(message),
                Update::SecretPrompt(title) => {
                    state.secret_prompt = Some(tui::SecretPrompt::new(title));
                    // A panel over a modal prompt would take the Enter that
                    // submits it.
                    state.panel = None;
                    state.inspection = None;
                }
                Update::Stop => return Ok(()),
            }
        }
        if state.activity == Activity::Starting && started.elapsed() >= Duration::from_millis(350) {
            state.activity = Activity::Idle;
            dirty = true;
        }
        let moving =
            busy || state.activity == Activity::Starting || state.completion_tick.is_some();
        if moving
            && last_tick.elapsed()
                >= Duration::from_millis(if state.reduced_motion { 1000 } else { 120 })
        {
            if !state.reduced_motion {
                state.animation_frame = state.animation_frame.wrapping_add(1);
            }
            if let Some(start) = task_started {
                state.pulse.elapsed_ms = start.elapsed().as_millis() as u64;
            }
            state.completion_tick = state
                .completion_tick
                .and_then(|tick| (tick < 5).then_some(tick + 1));
            last_tick = Instant::now();
            dirty = true;
        }
        if state.scrolling
            && last_scroll.is_some_and(|at| at.elapsed() >= tui::SCROLL_INDICATOR_LINGER)
        {
            state.scrolling = false;
            last_scroll = None;
            dirty = true;
        }
        // **One frame per batch of input, not one per event.** Rendering
        // the transcript costs more than the gap between two events of a
        // drag or a wheel flick, so drawing on each one puts the screen
        // behind the hand and leaves a flicked wheel scrolling after it
        // stopped -- the motion a person sees is the queue draining. While
        // more input is already waiting, consume it and draw once.
        let waiting = input.queued() || event::poll(Duration::ZERO)?;
        if dirty && (!waiting || last_drawn.elapsed() >= FRAME) {
            tick_helper_clocks(&mut notebook, &mut helper_clocks);
            state.input = editor.text.clone();
            state.cursor = Some(editor.cursor);
            state.completion_selected = editor.selected;
            let _guard = super::lock(&DRAWING);
            if !ACTIVE.load(Ordering::SeqCst) {
                break;
            }
            let size = terminal.size()?;
            let area = ratatui::layout::Rect::new(0, 0, size.width, size.height);
            let regions = tui::screen_regions(area, &state);
            viewport_height = usize::from(regions.transcript.height).max(1);
            let rows = tui::conversation_rows(
                &conversation,
                &super::empty_handles(),
                &notebook,
                &state,
                regions.transcript.width,
            );
            if previous_rows > 0 {
                state.scrollback =
                    tui::anchor_scrollback(state.scrollback, previous_rows, rows, viewport_height);
            }
            previous_rows = rows;
            if let Some(inspection) = state.inspection.as_mut() {
                inspection.clamp(
                    &conversation,
                    &notebook,
                    regions.transcript.width,
                    regions.transcript.height,
                );
            }
            terminal.draw(|frame| {
                geometry = tui::render_screen_with_geometry(
                    frame,
                    &conversation,
                    &served,
                    &super::empty_handles(),
                    &notebook,
                    &state,
                );
                if let Some(request) = approvals.front() {
                    tui::render_approval(
                        frame,
                        &request.action().confirmation(),
                        approval_scroll,
                        request.hint_line(),
                    );
                } else if let Some(request) = asking.as_ref() {
                    tui::render_ask(frame, request, ask_selected, state.theme);
                } else if let Some(settings) = settings_editor.as_ref() {
                    settings.panel.render(frame, state.theme);
                }
            })?;
            io::stdout().flush()?;
            dirty = false;
            last_drawn = Instant::now();
        }
        if !input.queued() && !event::poll(Duration::from_millis(if moving { 40 } else { 100 }))? {
            continue;
        }
        let Some(input_event) = input.read()? else {
            continue;
        };
        match input_event {
            Event::Resize(_, _) => {
                dirty = true;
            }
            Event::Mouse(mouse) => {
                if settings_editor.is_some() {
                    continue;
                }
                if !approvals.is_empty() {
                    approval_scroll = match mouse.kind {
                        MouseEventKind::ScrollUp => approval_scroll.saturating_sub(3),
                        MouseEventKind::ScrollDown => approval_scroll.saturating_add(3),
                        _ => approval_scroll,
                    };
                    dirty = true;
                    continue;
                }
                // **Every press anchors a possible drag, whatever else it
                // reaches.** A person who starts selecting on a cell header
                // still means to select; the widget that fires below has
                // already done its one thing and the drag is a second, later
                // gesture that cannot be confused with it.
                if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                    pressed_at = Some((mouse.column, mouse.row));
                    if state.selection.take().is_some() {
                        dirty = true;
                    }
                }
                if mouse.kind == MouseEventKind::Drag(MouseButton::Left) {
                    if let Some(anchor) = pressed_at {
                        state.selection = Some(tui::Selection {
                            anchor,
                            head: (mouse.column, mouse.row),
                        });
                        dirty = true;
                    }
                    continue;
                }
                if mouse.kind == MouseEventKind::Up(MouseButton::Left) {
                    let anchor = pressed_at.take();
                    // A drag that covered something is a selection, and it
                    // goes to the clipboard through the terminal -- the same
                    // OSC 52 route `/copy` uses, so it works over SSH too.
                    if state.selection.is_some_and(|span| !span.is_empty()) {
                        match geometry.selected() {
                            Some(text) => {
                                links::copy(text);
                                let lines = text.lines().count();
                                // One standing answer to "what did I just
                                // copy", not a new line per drag.
                                state.note_replacing(
                                    COPIED,
                                    format!(
                                        "{COPIED}{lines} line{} to the clipboard.",
                                        if lines == 1 { "" } else { "s" }
                                    ),
                                );
                            }
                            None => state.selection = None,
                        }
                        dirty = true;
                        continue;
                    }
                    // A press and a release in the same place is a click.
                    // The path is answered here rather than on the press so
                    // that a drag which begins on one selects instead.
                    if let Some(tui::Hit::Path(index)) =
                        anchor.and_then(|(column, row)| geometry.hit(column, row))
                        && let Some(path) = geometry.path(index)
                    {
                        let shown = path.to_string();
                        let resolved = std::path::Path::new(&shown).to_path_buf();
                        let resolved = if resolved.is_absolute() {
                            resolved
                        } else {
                            std::env::current_dir().unwrap_or_default().join(resolved)
                        };
                        state.note(if links::show(&resolved) {
                            format!("Opened {shown}.")
                        } else {
                            format!("Nothing here can open {shown}.")
                        });
                        dirty = true;
                    }
                    continue;
                }
                if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                    match geometry.hit(mouse.column, mouse.row) {
                        // The picker's own rows, exactly as they behaved
                        // before the rest of the screen became clickable.
                        Some(tui::Hit::Panel(hit)) => {
                            if state
                                .panel
                                .as_mut()
                                .is_some_and(|panel| select_panel_at(panel, hit))
                            {
                                dirty = true;
                                continue;
                            }
                        }
                        // A file path drawn in the transcript. The one
                        // surface with no keyboard twin, by the user's
                        // ruling of 2026-09-18: naming the path to a slash
                        // command is slower than the click is worth. It is
                        // opened on the release above, not here, so that a
                        // drag beginning on a path selects instead.
                        Some(tui::Hit::Path(_)) => continue,
                        // The same thing `/cell <n>` does, on the cell whose
                        // header was clicked.
                        Some(tui::Hit::Cell(cell)) => {
                            open_cell(&mut state, &notebook, cell);
                            dirty = true;
                            continue;
                        }
                        // `/model`, submitted for the person: one route, so
                        // the picker cannot open two different ways.
                        Some(tui::Hit::Status(tui::StatusField::Model)) => {
                            let _ = answers.inputs.send(Input::Submit("/model".into()));
                            dirty = true;
                            continue;
                        }
                        // Shift-Tab's own path, refusal included.
                        Some(tui::Hit::Status(tui::StatusField::Mode)) => {
                            match mode_change(busy, state.mode) {
                                Ok(input) => {
                                    busy = true;
                                    let _ = answers.inputs.send(Input::Submit(input));
                                }
                                Err(refusal) => state.notice = Some(refusal.into()),
                            }
                            dirty = true;
                            continue;
                        }
                        // Where the arrow keys would have walked to.
                        Some(tui::Hit::Composer { row, column }) => {
                            let width = terminal.size()?.width;
                            editor.cursor = tui::composer_offset(
                                &editor.text,
                                row,
                                column,
                                width.saturating_sub(2),
                            );
                            dirty = true;
                            continue;
                        }
                        None => {}
                    }
                }
                let up = mouse.kind == MouseEventKind::ScrollUp;
                if up || mouse.kind == MouseEventKind::ScrollDown {
                    state.scrolling = true;
                    last_scroll = Some(Instant::now());
                    if let Some(inspection) = state.inspection.as_mut() {
                        inspection.scroll = if up {
                            inspection.scroll.saturating_sub(3)
                        } else {
                            inspection.scroll.saturating_add(3)
                        };
                    } else if state.panel.is_none() && !state.telemetry_open {
                        state.scrollback = if up {
                            state
                                .scrollback
                                .saturating_add(3)
                                .min(previous_rows.saturating_sub(viewport_height))
                        } else {
                            state.scrollback.saturating_sub(3)
                        };
                    }
                    dirty = true;
                }
            }
            Event::Paste(text) => {
                if settings_editor.is_some() {
                    continue;
                }
                if !approvals.is_empty() {
                    continue;
                }
                // Pasting is how most keys are entered, so the masked prompt
                // takes a paste before anything else can.
                if let Some(prompt) = state.secret_prompt.as_mut() {
                    prompt.push(&text);
                } else if !state
                    .panel
                    .as_mut()
                    .is_some_and(|panel| panel.search_insert(&text))
                {
                    editor.insert(&text);
                }
                dirty = true;
            }
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                dirty = true;
                if let Some(request) = approvals.front() {
                    let complete = request.action().confirmation().complete;
                    let decision = match key.code {
                        KeyCode::Char('o' | 'O') if complete && key.modifiers.is_empty() => {
                            Some(crate::approval::Decision::AllowOnce)
                        }
                        KeyCode::Char('s' | 'S') if complete && key.modifiers.is_empty() => {
                            Some(crate::approval::Decision::AllowForSession)
                        }
                        KeyCode::Char('d' | 'D') | KeyCode::Esc => {
                            Some(crate::approval::Decision::Deny)
                        }
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            super::INTERRUPT.store(true, Ordering::SeqCst);
                            Some(crate::approval::Decision::Deny)
                        }
                        KeyCode::Up => {
                            approval_scroll = approval_scroll.saturating_sub(1);
                            None
                        }
                        KeyCode::Down => {
                            approval_scroll = approval_scroll.saturating_add(1);
                            None
                        }
                        KeyCode::PageUp => {
                            approval_scroll = approval_scroll.saturating_sub(10);
                            None
                        }
                        KeyCode::PageDown => {
                            approval_scroll = approval_scroll.saturating_add(10);
                            None
                        }
                        KeyCode::Home => {
                            approval_scroll = 0;
                            None
                        }
                        _ => None,
                    };
                    if let Some(decision) = decision {
                        if let Some(request) = approvals.pop_front() {
                            request.respond(decision);
                        }
                        approval_scroll = 0;
                    }
                    continue;
                }
                // **Modal, and answered either way.** Escape is a choice
                // ("decide yourself"), not a cancellation: a model waiting on
                // a question nobody answered would only ask it again.
                if let Some(request) = asking.as_ref() {
                    let choices = request.question().choices.len();
                    let answer = match tui::ask_key(key.code, ask_selected, choices) {
                        tui::AskKey::Move(index) => {
                            ask_selected = index;
                            None
                        }
                        tui::AskKey::Confirm => Some(crate::ask::Answer {
                            choice: request.question().choices.get(ask_selected).cloned(),
                            by: crate::ask::AnsweredBy::Person,
                        }),
                        tui::AskKey::Dismiss => Some(crate::ask::Answer::dismissed()),
                        tui::AskKey::Ignored => None,
                    };
                    if let Some(answer) = answer
                        && let Some(request) = asking.take()
                    {
                        request.respond(answer);
                        ask_selected = 0;
                    }
                    continue;
                }
                if let Some(settings) = settings_editor.as_mut() {
                    if settings.key(key, &mut state) {
                        settings_editor = None;
                    }
                    continue;
                }
                // **Modal, and first.** While a masked prompt is open every
                // key belongs to it: none reaches the editor, the panel, the
                // inspector or the input history.
                if let Some(prompt) = state.secret_prompt.as_mut() {
                    match key.code {
                        KeyCode::Enter => {
                            let entered = state.secret_prompt.take().map(tui::SecretPrompt::take);
                            let _ = answers.secrets.send(entered);
                        }
                        KeyCode::Esc => {
                            state.secret_prompt = None;
                            let _ = answers.secrets.send(None);
                        }
                        KeyCode::Char('c' | 'u')
                            if key.modifiers.contains(KeyModifiers::CONTROL) =>
                        {
                            prompt.clear();
                        }
                        KeyCode::Backspace => prompt.backspace(),
                        KeyCode::Char(c)
                            if !key
                                .modifiers
                                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                        {
                            prompt.push(&c.to_string());
                        }
                        _ => {}
                    }
                    continue;
                }
                if let Some(inspection) = state.inspection.as_mut() {
                    let handled = match key.code {
                        KeyCode::Esc => {
                            state.inspection = None;
                            true
                        }
                        KeyCode::Left => {
                            inspection.adjacent(false, &notebook);
                            true
                        }
                        KeyCode::Right => {
                            inspection.adjacent(true, &notebook);
                            true
                        }
                        KeyCode::Up => {
                            inspection.scroll = inspection.scroll.saturating_sub(1);
                            true
                        }
                        KeyCode::Down => {
                            inspection.scroll = inspection.scroll.saturating_add(1);
                            true
                        }
                        KeyCode::PageUp => {
                            inspection.scroll = inspection
                                .scroll
                                .saturating_sub(viewport_height.saturating_sub(3));
                            true
                        }
                        KeyCode::PageDown => {
                            inspection.scroll = inspection
                                .scroll
                                .saturating_add(viewport_height.saturating_sub(3));
                            true
                        }
                        KeyCode::Home => {
                            inspection.scroll = 0;
                            true
                        }
                        KeyCode::End => {
                            inspection.scroll = usize::MAX;
                            true
                        }
                        _ => false,
                    };
                    if handled {
                        continue;
                    }
                }
                if state.telemetry_open && state.panel.is_none() {
                    match key.code {
                        KeyCode::Esc => {
                            state.telemetry_open = false;
                            continue;
                        }
                        KeyCode::Up => {
                            state.telemetry_selected = Some(
                                state
                                    .telemetry_selected
                                    .unwrap_or(notebook.requests.len().saturating_sub(1))
                                    .saturating_sub(1),
                            );
                            continue;
                        }
                        KeyCode::Down => {
                            state.telemetry_selected = state
                                .telemetry_selected
                                .and_then(|i| (i + 1 < notebook.requests.len()).then_some(i + 1));
                            continue;
                        }
                        _ => {}
                    }
                }
                if let Some(panel) = state.panel.as_mut() {
                    if panel.search.is_some() {
                        match key.code {
                            KeyCode::Left => {
                                panel.move_provider(false);
                                continue;
                            }
                            KeyCode::Right => {
                                panel.move_provider(true);
                                continue;
                            }
                            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                panel.search_clear();
                                continue;
                            }
                            // Ctrl-O, not a letter: this panel's plain keys
                            // are its search box. Not Ctrl-S either, which a
                            // terminal takes for flow control.
                            KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                panel.cycle_order();
                                continue;
                            }
                            // Space stages rather than filters. The cost is
                            // stated because it is real: the filter's terms
                            // are whitespace-separated and AND-ed, so it is
                            // now reachable one term at a time. Staging is
                            // what a person does here repeatedly; a two-term
                            // filter is not.
                            KeyCode::Char(' ')
                                if !key
                                    .modifiers
                                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                            {
                                if let Some(said) = panel.stage() {
                                    state.notice = Some(said);
                                }
                                continue;
                            }
                            KeyCode::Char(c)
                                if !key
                                    .modifiers
                                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                            {
                                panel.search_insert(&c.to_string());
                                continue;
                            }
                            KeyCode::Backspace => {
                                panel.search_backspace();
                                continue;
                            }
                            _ => {}
                        }
                    }
                    match key.code {
                        KeyCode::Esc => {
                            // The staged map dies with the panel, which is
                            // the whole of "Esc throws the changes away".
                            let discarded = panel.staged_commands().len();
                            state.panel = None;
                            if discarded > 0 {
                                state.note(format!("discarded {discarded} staged change(s)"));
                            }
                        }
                        KeyCode::Up => panel.move_selection(false, 1),
                        KeyCode::Down => panel.move_selection(true, 1),
                        // Which tier a chosen model is assigned to. Tab rather
                        // than a letter because the model panel's plain keys
                        // are its search box.
                        KeyCode::Tab => {
                            panel.cycle_tier();
                        }
                        KeyCode::PageUp => panel.move_selection(false, 10),
                        KeyCode::PageDown => panel.move_selection(true, 10),
                        KeyCode::Enter => {
                            // Everything staged, in tier order. `Enter` on a
                            // panel with nothing staged still applies the
                            // highlighted row, which is what every non-model
                            // panel -- themes, handlers, login -- relies on.
                            let staged = panel.staged_commands();
                            if !staged.is_empty() {
                                if !busy {
                                    state.panel = None;
                                    busy = true;
                                    for command in staged {
                                        let _ = answers.inputs.send(Input::Submit(command));
                                    }
                                }
                            } else if let Some(command) = panel
                                .rows
                                .get(panel.selected)
                                .and_then(|r| r.command.clone())
                            {
                                if let Some(theme) =
                                    command.strip_prefix("/theme ").and_then(tui::Theme::parse)
                                {
                                    state.theme = theme;
                                    state.panel = None;
                                    state.note(format!("Theme: {}", theme.name()));
                                } else if let Some(link) = command.strip_prefix("/open-link ") {
                                    state.note(if links::open(link) {
                                        "Opened the link in your default browser."
                                    } else {
                                        "No browser can be opened here: copy the link instead."
                                    });
                                } else if let Some(text) = command.strip_prefix("/copy ") {
                                    links::copy(text);
                                    state.note("Copied to the clipboard through the terminal.");
                                } else if command == "/paste-callback" {
                                    state.secret_prompt = Some(tui::SecretPrompt::new(
                                        "Paste the address your browser ended on after signing in, then Enter",
                                    ));
                                } else if let Some(name) = command.strip_prefix("/handlers off ") {
                                    super::lock(&handler_cancellations).push(name.to_string());
                                    state.panel = None;
                                    state.note(format!(
                                        "handler {name}: cancellation queued for the next cell boundary"
                                    ));
                                } else if !busy {
                                    state.panel = None;
                                    busy = true;
                                    let _ = answers.inputs.send(Input::Submit(command));
                                }
                            }
                        }
                        _ => {}
                    }
                    if !key.modifiers.contains(KeyModifiers::CONTROL) {
                        continue;
                    }
                }
                if key.code == KeyCode::BackTab {
                    // The permission rung, not the request mode: this is the
                    // one a person reaches for constantly, and — unlike a
                    // request mode, which is a new request — it must move
                    // *while* a task runs, because that is when someone
                    // notices they are on the wrong rung. The request mode
                    // keeps `/mode` and its own sidebar field.
                    state.notice = Some(rung_change(&state.permissions));
                    continue;
                }
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    match key.code {
                        KeyCode::Char('c') => {
                            if !busy && !editor.text.is_empty() {
                                editor.text.clear();
                                editor.cursor = 0;
                            } else {
                                super::INTERRUPT.store(true, Ordering::SeqCst);
                            }
                            continue;
                        }
                        KeyCode::Char('t') => {
                            state.telemetry_open = !state.telemetry_open;
                            state.inspection = None;
                            state.panel = None;
                            continue;
                        }
                        KeyCode::Char('o') => {
                            state.compact = !state.compact;
                            continue;
                        }
                        KeyCode::Char('b') => {
                            state.sidebar = match state.sidebar {
                                SidebarVisibility::Hidden => SidebarVisibility::Shown,
                                _ => SidebarVisibility::Hidden,
                            };
                            continue;
                        }
                        KeyCode::Char('f') => {
                            state.fullscreen = !state.fullscreen;
                            continue;
                        }
                        // Give the pointer back to the terminal, and take it
                        // again. Ctrl-G because Ctrl-E, Ctrl-A, Ctrl-U and
                        // Ctrl-K are the composer's, and Ctrl-S and Ctrl-Q
                        // are the terminal's own flow control.
                        KeyCode::Char('g') => {
                            let off = !state.mouse_off;
                            set_mouse_capture(&mut state, !off);
                            continue;
                        }
                        KeyCode::Home => {
                            state.scrollback = previous_rows.saturating_sub(viewport_height);
                            continue;
                        }
                        KeyCode::End => {
                            state.scrollback = 0;
                            continue;
                        }
                        KeyCode::Char('d') if !busy && editor.text.is_empty() => {
                            let _ = answers.inputs.send(Input::Exit);
                            return Ok(());
                        }
                        _ => {}
                    }
                }
                match key.code {
                    KeyCode::PageUp => {
                        state.scrollback = state
                            .scrollback
                            .saturating_add(viewport_height.saturating_sub(2))
                            .min(previous_rows.saturating_sub(viewport_height));
                        continue;
                    }
                    KeyCode::PageDown => {
                        state.scrollback = state
                            .scrollback
                            .saturating_sub(viewport_height.saturating_sub(2));
                        continue;
                    }
                    _ => {}
                }
                if editor.key(key) && !editor.text.trim().is_empty() {
                    if matches!(
                        editor.text.split_whitespace().next(),
                        Some("/cell" | "/cells" | "/chat")
                    ) {
                        let text = editor.take();
                        let mut words = text.split_whitespace();
                        let command = words.next().unwrap_or_default();
                        if command == "/chat" {
                            state.inspection = None;
                            state.telemetry_open = false;
                        } else {
                            let cell = match words.next() {
                                Some(value) => value.parse::<usize>().unwrap_or(0),
                                None => tui::Inspection::latest(&notebook).unwrap_or(0),
                            };
                            open_cell(&mut state, &notebook, cell);
                            if state.inspection.is_some()
                                && let Some(line) = &notebook.decision
                            {
                                state.note(line.clone());
                            }
                        }
                        continue;
                    }
                    if editor.text.trim() == "/telemetry" {
                        editor.take();
                        state.telemetry_open = !state.telemetry_open;
                        state.panel = None;
                        state.notice = None;
                        continue;
                    }
                    // Sits with the other argument-less presentation toggles
                    // rather than with `/sidebar`, so it still works while a
                    // task runs -- watching a long stream fill the screen is
                    // the case this command exists for.
                    if editor.text.trim() == "/mouse" {
                        editor.take();
                        let off = !state.mouse_off;
                        set_mouse_capture(&mut state, !off);
                        continue;
                    }
                    if editor.text.trim() == "/fullscreen" {
                        editor.take();
                        state.fullscreen = !state.fullscreen;
                        state.notice = None;
                        continue;
                    }
                    if editor.text.split_whitespace().next() == Some("/motion") {
                        let text = editor.take();
                        match text.split_whitespace().nth(1) {
                            Some("off" | "reduce") => state.reduced_motion = true,
                            Some("on") => state.reduced_motion = false,
                            _ => {
                                state.note("Usage: /motion on | off");
                                continue;
                            }
                        }
                        state.completion_tick = None;
                        state.note(if state.reduced_motion {
                            "Motion reduced. /motion on restores animation."
                        } else {
                            "Motion on. /motion off reduces animation."
                        });
                        continue;
                    }
                    if !busy && matches!(editor.text.trim(), "/settings" | "/statusline") {
                        let status_only = editor.text.trim() == "/statusline";
                        editor.take();
                        match crate::settings_session::Editor::open(&state, status_only) {
                            Ok(settings) => settings_editor = Some(settings),
                            Err(error) => state.note(error),
                        }
                        continue;
                    }
                    if editor.text.split_whitespace().next() == Some("/theme") {
                        let text = editor.take();
                        match text.split_whitespace().nth(1) {
                            Some(name) => {
                                if let Some(theme) = tui::Theme::parse(name) {
                                    state.theme = theme;
                                    state.note(format!(
                                        "Theme: {} · /theme opens the palette",
                                        theme.name()
                                    ));
                                } else {
                                    state.note("Unknown theme. /theme opens the palette.");
                                }
                            }
                            None => {
                                state.notice = None;
                                state.panel = Some(tui::Panel {
                                    title: "Themes".into(),
                                    selected: tui::Theme::ALL
                                        .iter()
                                        .position(|theme| *theme == state.theme)
                                        .unwrap_or(0),
                                    rows: tui::Theme::ALL
                                        .iter()
                                        .map(|theme| tui::PanelRow {
                                            text: format!("██  {}", theme.name()),
                                            command: Some(format!("/theme {}", theme.name())),
                                        })
                                        .collect(),
                                    ..tui::Panel::default()
                                });
                            }
                        }
                        continue;
                    }
                    if editor.text.split_whitespace().next() == Some("/handlers") {
                        let text = editor.take();
                        let parts: Vec<_> = text.split_whitespace().collect();
                        match parts.as_slice() {
                            ["/handlers"] => {
                                state.panel = Some(tui::handlers_panel(&notebook.handlers))
                            }
                            ["/handlers", "off", name] => {
                                if busy
                                    && notebook
                                        .handlers
                                        .iter()
                                        .any(|h| h.name == *name && h.active)
                                {
                                    super::lock(&handler_cancellations).push((*name).to_string());
                                    state.note(format!(
                                        "handler {name}: cancellation queued for the next cell boundary"
                                    ));
                                } else {
                                    state.note(format!(
                                        "handler {name}: no active handler with that name"
                                    ));
                                }
                            }
                            _ => state.note("Use /handlers or /handlers off <name>"),
                        }
                        continue;
                    }
                    if busy {
                        state.notice = Some(
                            "Working. Your draft is kept; Ctrl-C interrupts tools; twice exits."
                                .into(),
                        );
                        continue;
                    }
                    let text = editor.take();
                    state.scrollback = 0;
                    state.notice = None;
                    if text.trim() == "/exit" {
                        let _ = answers.inputs.send(Input::Exit);
                        return Ok(());
                    }
                    if text.split_whitespace().next() == Some("/statusline") {
                        let word = text.split_whitespace().nth(1).unwrap_or("");
                        let said = match crate::settings_session::save_status(&mut state,word) {
                            Ok(())=>"Status line saved for this project. Selected profile overrides still apply.".into(),
                            Err(error)=>error,
                        };
                        state.note(said);
                        continue;
                    }
                    if text.split_whitespace().next() == Some("/sidebar") {
                        state.sidebar = match text.split_whitespace().nth(1) {
                            Some("hide") => SidebarVisibility::Hidden,
                            Some("show") => SidebarVisibility::Shown,
                            _ => SidebarVisibility::Auto,
                        };
                        state.note("Sidebar: /sidebar auto|show|hide · Ctrl-B toggles");
                        continue;
                    }
                    busy = true;
                    task_started = Some(Instant::now());
                    state.pulse = tui::Pulse::default();
                    state.activity = Activity::Thinking;
                    if answers.inputs.send(Input::Submit(text)).is_err() {
                        return Ok(());
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// A panel shown again under the same title keeps the row a person selected:
/// a sign-in panel redraws while it waits, and a reset would move the cursor
/// off the row about to be chosen.
fn replace_panel(current: Option<&tui::Panel>, mut next: tui::Panel) -> tui::Panel {
    // Only a row that does something is a choice worth keeping: a panel
    // that first said only "waiting" must not hold its cursor on text.
    if let Some(current) = current.filter(|current| {
        current.title == next.title
            && current
                .rows
                .get(current.selected)
                .is_some_and(|row| row.command.is_some())
    }) {
        next.selected = current.selected.min(next.rows.len().saturating_sub(1));
    }
    next
}

/// The panel is an open view of task state, not a copy frozen at `/handlers`.
fn refresh_handler_panel(
    panel: &mut Option<tui::Panel>,
    before: &[crate::runtime::handlers::HandlerInfo],
    after: &[crate::runtime::handlers::HandlerInfo],
) {
    let Some(held) = panel.as_mut() else { return };
    let mut refreshed = tui::handlers_panel(after);
    if held.title != refreshed.title {
        return;
    }
    // Row zero is explanatory text. Keep the same handler selected through
    // counter/status changes; a missing row returns selection to that header.
    if let Some(index) = held.selected.checked_sub(1)
        && let Some(selected) = before.get(index)
    {
        refreshed.selected = if after.get(index).is_some_and(|h| h.name == selected.name) {
            index + 1
        } else {
            after
                .iter()
                .position(|h| h.name == selected.name)
                .map_or(0, |index| index + 1)
        };
    }
    *held = refreshed;
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    /// A panel redrawn under its title keeps the selected row; a different
    /// panel starts where it chose to.
    #[test]
    fn a_redrawn_panel_keeps_the_selected_row() {
        let rows = |n: usize| {
            (0..n)
                .map(|i| tui::PanelRow {
                    text: format!("row {i}"),
                    command: Some(format!("/row {i}")),
                })
                .collect::<Vec<_>>()
        };
        let mut shown = tui::Panel::rows("Connecting claude-max", rows(4));
        shown.selected = 2;
        let again = replace_panel(
            Some(&shown),
            tui::Panel::rows("Connecting claude-max", rows(4)),
        );
        assert_eq!(again.selected, 2);
        let shorter = replace_panel(
            Some(&shown),
            tui::Panel::rows("Connecting claude-max", rows(2)),
        );
        assert_eq!(shorter.selected, 1);
        let other = replace_panel(Some(&shown), tui::Panel::rows("Themes", rows(4)));
        assert_eq!(other.selected, 0);
        let waiting = tui::Panel::rows(
            "Connecting claude-max",
            vec![tui::PanelRow {
                text: "waiting for the sign-in…".into(),
                command: None,
            }],
        );
        let mut offered = tui::Panel::rows("Connecting claude-max", rows(4));
        offered.selected = 3;
        assert_eq!(
            replace_panel(Some(&waiting), offered).selected,
            3,
            "a cursor on text is not kept over the new panel's choice"
        );
    }

    /// A call in flight is dated from the frame it first appeared in, so its
    /// lane's seconds keep counting while the cell that made it blocks the
    /// task thread. A call that has resolved keeps what it actually took.
    #[test]
    fn a_running_helper_is_timed_by_the_clock_and_a_resolved_one_by_its_record() {
        use crate::helpers::{HelperOutcome, HelperRecord};
        let running = HelperRecord {
            helper: "reduce".into(),
            verb: "reducing".into(),
            asked: "cargo build log".into(),
            ..HelperRecord::default()
        };
        let resolved = HelperRecord {
            outcome: HelperOutcome {
                text: "3 distinct root failures".into(),
                ok: true,
                cancelled: false,
                elapsed_ms: 120,
            },
            ..running.clone()
        };
        let mut notebook = Notebook::default();
        // `set` numbers cells from one; this is the notebook's first cell.
        notebook.set(
            1,
            tui::CellView {
                helpers: vec![running, resolved],
                ..tui::CellView::default()
            },
        );
        let mut clocks = HashMap::new();
        clocks.insert((0, 0), Instant::now() - Duration::from_millis(1500));
        clocks.insert((0, 1), Instant::now());

        tick_helper_clocks(&mut notebook, &mut clocks);

        let helpers = &notebook.cells[0].helpers;
        assert!(
            helpers[0].outcome.elapsed_ms >= 1500,
            "a running call's elapsed comes from the clock: {}",
            helpers[0].outcome.elapsed_ms
        );
        assert_eq!(
            helpers[1].outcome.elapsed_ms, 120,
            "a resolved call keeps the duration it actually took"
        );
        assert!(
            !clocks.contains_key(&(0, 1)),
            "a call that resolved no longer holds a clock"
        );
    }

    #[test]
    fn an_open_handler_panel_tracks_runs_disable_cancel_and_clear_with_selection() {
        use crate::runtime::handlers::HandlerInfo;
        let mut before = vec![
            HandlerInfo {
                name: "first".into(),
                runs: 0,
                drained: 0,
                error: None,
                active: true,
            },
            HandlerInfo {
                name: "second".into(),
                runs: 0,
                drained: 0,
                error: None,
                active: true,
            },
        ];
        let mut panel = Some(tui::handlers_panel(&before));
        panel.as_mut().unwrap().selected = 2;
        for phase in 0..4 {
            let mut after = before.clone();
            match phase {
                0 => {
                    after[1].runs = 1;
                    after[1].drained = 1;
                }
                1 => {
                    after[1].active = false;
                    after[1].error = Some("RuntimeTimeout".into());
                }
                2 => {
                    after[0].active = false;
                }
                _ => after.clear(),
            }
            refresh_handler_panel(&mut panel, &before, &after);
            let held = panel.as_ref().unwrap();
            assert_eq!(held.selected, if after.is_empty() { 0 } else { 2 });
            if phase == 0 {
                assert!(held.rows[2].text.contains("1 runs · 1 drained"));
            }
            if phase == 1 {
                assert!(held.rows[2].text.contains("stale"));
                assert!(held.rows[2].text.contains("RuntimeTimeout"));
                assert!(held.rows[2].command.is_none());
            }
            if phase == 2 {
                assert!(held.rows[1].text.contains("stale"));
                assert!(held.rows[1].command.is_none());
            }
            if phase == 3 {
                assert_eq!(held.rows.len(), 1);
                assert!(held.rows[0].text.contains("No handlers in this task"));
            }
            before = after;
        }
    }

    #[test]
    fn handler_panel_selection_follows_a_surviving_row_and_leaves_other_panels_alone() {
        use crate::runtime::handlers::HandlerInfo;
        let before: Vec<_> = ["first", "second"]
            .into_iter()
            .map(|name| HandlerInfo {
                name: name.into(),
                runs: 0,
                drained: 0,
                error: None,
                active: true,
            })
            .collect();
        let mut panel = Some(tui::handlers_panel(&before));
        panel.as_mut().unwrap().selected = 2;
        refresh_handler_panel(&mut panel, &before, &before[1..]);
        assert_eq!(panel.as_ref().unwrap().selected, 1);
        assert!(panel.as_ref().unwrap().rows[1].text.starts_with("second"));
        let mut other = Some(tui::Panel::text("Other", "keep"));
        refresh_handler_panel(&mut other, &before, &[]);
        assert_eq!(other.unwrap().rows[0].text, "keep");
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// Clicks the rendered point that answers `hit`, through the screen's own
    /// hit test, so the test exercises the same route a real click takes.
    fn click_panel(state: &mut ScreenState, hit: tui::PanelHit) -> bool {
        let geometry = render_panel_geometry(state);
        let (column, row) = point_for(&geometry, hit);
        match geometry.hit(column, row) {
            Some(tui::Hit::Panel(hit)) => select_panel_at(state.panel.as_mut().unwrap(), hit),
            other => panic!("expected a panel hit at {column},{row}, got {other:?}"),
        }
    }

    fn point_for(geometry: &tui::ScreenGeometry, hit: tui::PanelHit) -> (u16, u16) {
        for row in 0..24 {
            for column in 0..80 {
                if geometry.hit(column, row) == Some(tui::Hit::Panel(hit)) {
                    return (column, row);
                }
            }
        }
        panic!("no rendered point for {hit:?}");
    }

    fn render_panel_geometry(state: &ScreenState) -> tui::ScreenGeometry {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut geometry = tui::ScreenGeometry::default();
        terminal
            .draw(|frame| {
                geometry = tui::render_screen_with_geometry(
                    frame,
                    &Conversation::default(),
                    &ServedBy::default(),
                    &crate::runtime::handles::HandleTable::new(),
                    &Notebook::default(),
                    state,
                );
            })
            .unwrap();
        geometry
    }

    #[test]
    fn model_picker_clicks_select_from_the_rendered_geometry_without_applying() {
        let mut state = ScreenState {
            panel: Some(tui::Panel::models(
                "Models",
                vec![
                    tui::ModelGroup {
                        provider: "anthropic".into(),
                        account: "one".into(),
                        scope: "declared".into(),
                        models: vec!["a-model".into()],
                        selectable: Some(true),
                        unavailable_reason: None,
                        connect: None,
                    },
                    tui::ModelGroup {
                        provider: "openrouter".into(),
                        account: "two".into(),
                        scope: "declared".into(),
                        models: vec!["o-one".into(), "o-two".into()],
                        selectable: Some(true),
                        unavailable_reason: None,
                        connect: None,
                    },
                ],
                tui::TierModels::default(),
            )),
            ..ScreenState::default()
        };

        assert!(click_panel(&mut state, tui::PanelHit::Provider(1)));
        let panel = state.panel.as_ref().unwrap();
        assert_eq!(
            panel.rows[panel.selected].command.as_deref(),
            Some("/model o-one"),
            "a provider click changes the tab and highlights its first model"
        );

        assert!(click_panel(&mut state, tui::PanelHit::Model(2)));
        let panel = state.panel.as_ref().unwrap();
        assert_eq!(panel.selected, 2);
        assert_eq!(panel.rows[2].command.as_deref(), Some("/model o-two"));
        assert!(
            state.panel.is_some(),
            "selection alone must not apply or close"
        );

        assert_eq!(
            tui::ScreenGeometry::default().hit(4, 4),
            None,
            "a geometry nobody drew into hits nothing"
        );
        assert_eq!(state.panel.as_ref().unwrap().selected, 2);
        let click = |state: &mut ScreenState, hit| {
            assert!(click_panel(state, hit));
        };
        click(
            &mut state,
            tui::PanelHit::Tier(crate::spend::Tier::Subagents),
        );
        click(&mut state, tui::PanelHit::Mode(1));
        assert_eq!(
            state.panel.as_ref().unwrap().staged_commands(),
            ["/model subagent off"]
        );
        click(&mut state, tui::PanelHit::Mode(1));
        assert!(state.panel.as_ref().unwrap().staged_commands().is_empty());
    }

    #[test]
    fn editing_preserves_unicode_boundaries_and_multiline_paste() {
        let mut editor = Editor::default();
        editor.insert("a界\nb");
        editor.key(key(KeyCode::Left));
        editor.key(key(KeyCode::Backspace));
        assert_eq!(editor.text, "a界b");
        editor.key(key(KeyCode::Backspace));
        assert_eq!(editor.text, "ab");
        editor.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
        assert_eq!(editor.text, "a\nb");
    }
    #[test]
    fn paste_normalizes_terminal_newlines_without_admitting_controls() {
        let mut editor = Editor::default();
        editor.insert("first\rsecond\r\nthird\nfourth\tcolumn\x00\x1bfinal");
        assert_eq!(editor.text, "first\nsecond\nthird\nfourth\tcolumnfinal");
        assert_eq!(editor.cursor, editor.text.len());
    }
    #[test]
    fn selection_changes_what_tab_completes() {
        let mut editor = Editor::default();
        editor.insert("/");
        editor.key(key(KeyCode::Down));
        editor.key(key(KeyCode::Tab));
        assert_eq!(editor.text, "/models ");
        assert_eq!(editor.cursor, editor.text.len());
    }
    #[test]
    fn history_restores_an_unsent_draft() {
        let mut editor = Editor::default();
        editor.insert("sent");
        editor.take();
        editor.insert("draft");
        editor.recall(true);
        assert_eq!(editor.text, "sent");
        editor.recall(false);
        assert_eq!(editor.text, "draft");
    }

    /// A transcript with one executed cell, so the renderer draws a cell
    /// header the hit map can point at.
    fn cell_conversation() -> (Conversation, Notebook) {
        let conversation = Conversation {
            system: String::new(),
            messages: vec![
                crate::contract::Message::text(crate::contract::Role::User, "do the thing"),
                crate::contract::Message::text(
                    crate::contract::Role::Assistant,
                    "```pane\nawait read({path: \"a\"});\n```",
                ),
            ],
        };
        let mut notebook = Notebook::default();
        notebook.set(
            1,
            tui::CellView {
                execution: Some("read a".into()),
                ..tui::CellView::default()
            },
        );
        (conversation, notebook)
    }

    /// Renders once and hands back both halves: the hit map, and the buffer
    /// it was built from. **The pair is the point** -- a test that only reads
    /// the map cannot tell a correct rectangle from one a row out of place.
    fn render_geometry(
        state: &ScreenState,
        conversation: &Conversation,
        notebook: &Notebook,
    ) -> (tui::ScreenGeometry, ratatui::buffer::Buffer) {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let mut geometry = tui::ScreenGeometry::default();
        terminal
            .draw(|frame| {
                geometry = tui::render_screen_with_geometry(
                    frame,
                    conversation,
                    &ServedBy::default(),
                    &crate::runtime::handles::HandleTable::new(),
                    notebook,
                    state,
                );
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        (geometry, buffer)
    }

    fn row_text(buffer: &ratatui::buffer::Buffer, row: u16) -> String {
        (buffer.area.x..buffer.area.right())
            .map(|x| buffer[(x, row)].symbol())
            .collect()
    }

    #[test]
    fn clicking_a_cell_header_opens_what_slash_cell_opens() {
        let (conversation, notebook) = cell_conversation();
        let state = ScreenState::default();
        let (geometry, buffer) = render_geometry(&state, &conversation, &notebook);
        let point = (0..30)
            .flat_map(|row| (0..100).map(move |column| (column, row)))
            .find(|(column, row)| geometry.hit(*column, *row) == Some(tui::Hit::Cell(1)));
        let (column, row) = point.expect("the drawn cell header is in the hit map");
        // The drift guard: the clickable row must be the row the header was
        // drawn on, not a neighbour of it.
        let drawn = row_text(&buffer, row);
        assert!(
            drawn.contains("[1]") || drawn.contains("· 1"),
            "the hit row must carry cell 1's own header as it was drawn; \
             row {row} is {drawn:?}"
        );

        let mut clicked = ScreenState::default();
        match geometry.hit(column, row) {
            Some(tui::Hit::Cell(cell)) => open_cell(&mut clicked, &notebook, cell),
            other => panic!("expected a cell hit, got {other:?}"),
        }
        let mut typed = ScreenState::default();
        open_cell(&mut typed, &notebook, 1);

        assert!(clicked.inspection.is_some(), "the click opened the cell");
        assert_eq!(
            clicked.inspection.is_some(),
            typed.inspection.is_some(),
            "the click and `/cell 1` reach the same state"
        );
    }

    /// The request mode's click and its keyboard route.
    ///
    /// **Shift-Tab is no longer that route.** It moves the permission rung
    /// (`rung_change`), which is the control a person reaches for
    /// constantly, while the request mode is set once at the start of a
    /// piece of work. The mode keeps `/mode`, and the click on its field
    /// submits exactly that — so the pair this test exists to pin is intact,
    /// with the key on the other side of it changed.
    #[test]
    fn clicking_the_mode_field_sends_what_slash_mode_sends() {
        let state = ScreenState::default();
        let (geometry, _) = render_geometry(&state, &Conversation::default(), &Notebook::default());
        let hit = (0..30)
            .flat_map(|row| (0..100).map(move |column| (column, row)))
            .find_map(|(column, row)| geometry.hit(column, row))
            .is_some();
        assert!(hit, "something on the drawn screen is clickable");
        assert_eq!(
            mode_change(false, state.mode),
            Ok(format!("/mode {}", state.mode.next().name())),
            "the click submits the same input `/mode` does"
        );
        assert_eq!(
            mode_change(true, state.mode),
            Err("Change mode after the current task finishes."),
            "and both refuse the same way while a task runs"
        );
    }

    /// Shift-Tab walks the permission ladder and wraps, and it does not
    /// touch the request mode.
    #[test]
    fn shift_tab_moves_the_rung_and_leaves_the_request_mode_alone() {
        let state = ScreenState {
            permissions: crate::permissions::Ladder::new(crate::permissions::Rung::Manual),
            ..ScreenState::default()
        };
        let mode_before = state.mode;
        let mut seen = Vec::new();
        for _ in 0..4 {
            let line = rung_change(&state.permissions);
            assert!(
                line.starts_with("permissions "),
                "it says where it landed: {line}"
            );
            seen.push(state.permissions.rung());
        }
        assert_eq!(
            seen,
            vec![
                crate::permissions::Rung::AcceptEdits,
                crate::permissions::Rung::Auto,
                crate::permissions::Rung::Full,
                crate::permissions::Rung::Manual,
            ],
            "one rung per press, wrapping to where it started"
        );
        assert_eq!(state.mode, mode_before, "the request mode is untouched");
        assert_eq!(
            state.permissions.drain_moves().len(),
            4,
            "every move is recorded for the rollout"
        );
    }

    #[test]
    fn the_status_fields_are_where_the_status_line_drew_them() {
        let state = ScreenState {
            model: Some("a-model".into()),
            project: Some("a-project".into()),
            ..ScreenState::default()
        };
        let regions = tui::screen_regions(ratatui::layout::Rect::new(0, 0, 100, 30), &state);
        let (geometry, _) = render_geometry(&state, &Conversation::default(), &Notebook::default());
        let model = (0..100)
            .find(|column| {
                geometry.hit(*column, regions.status.y)
                    == Some(tui::Hit::Status(tui::StatusField::Model))
            })
            .expect("the model field is clickable on the status line it is drawn on");
        assert!(
            regions.status.height > 0 && model < regions.status.right(),
            "the field lies inside the status region the renderer used"
        );
        assert_eq!(
            geometry.hit(model, regions.status.y.saturating_sub(4)),
            None,
            "and nowhere else"
        );
    }

    #[test]
    fn a_click_on_empty_transcript_space_does_nothing() {
        let (conversation, notebook) = cell_conversation();
        let (geometry, _) = render_geometry(&ScreenState::default(), &conversation, &notebook);
        let regions = tui::screen_regions(
            ratatui::layout::Rect::new(0, 0, 100, 30),
            &ScreenState::default(),
        );
        // The row above the first drawn line of the transcript body.
        assert_eq!(geometry.hit(50, regions.transcript.y), None);
    }
}
