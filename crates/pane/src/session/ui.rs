//! One thread owns the terminal and keys; the task thread only sends view state.
use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::contract::{Conversation, ServedBy};
use crate::tui::{self, Activity, Notebook, ScreenState, SidebarVisibility};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::{Terminal, backend::CrosstermBackend};

mod terminal_input;

static ACTIVE: AtomicBool = AtomicBool::new(false);
static DRAWING: Mutex<()> = Mutex::new(());
thread_local! { static OUTPUT: RefCell<Option<mpsc::Sender<Update>>> = const { RefCell::new(None) }; }

pub(super) fn output(message: String) {
    OUTPUT.with(|output| {
        if let Some(sender) = output.borrow().as_ref() {
            let _ = sender.send(Update::Notice(message));
        } else {
            println!("{message}");
        }
    });
}

/// Also called by the existing second-SIGINT exit path, which skips Drop.
pub(super) fn restore_terminal() {
    let _guard = super::lock(&DRAWING);
    if ACTIVE.swap(false, Ordering::SeqCst) {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            DisableBracketedPaste,
            DisableMouseCapture,
            LeaveAlternateScreen,
            crossterm::cursor::Show
        );
    }
}
struct Restore;
impl Drop for Restore {
    fn drop(&mut self) {
        restore_terminal();
    }
}

pub(super) enum Update {
    Snapshot(Box<(Conversation, Notebook, ServedBy, Activity)>),
    Model(String),
    Delta(String),
    ToolDelta(String),
    Mode(tui::Mode),
    Effort(crate::wire::Effort),
    Panel(tui::Panel),
    Notice(String),
    Stop,
}
enum Input {
    Submit(String),
    Exit,
    Failed(String),
}

pub(super) struct LiveUi {
    handler_cancellations: Arc<Mutex<Vec<String>>>,
    updates: mpsc::Sender<Update>,
    inputs: mpsc::Receiver<Input>,
    thread: Option<JoinHandle<()>>,
}
impl LiveUi {
    pub(super) fn start(
        state: ScreenState,
        conversation: Conversation,
        notebook: Notebook,
    ) -> Result<Self, String> {
        let (updates, receiver) = mpsc::channel();
        let (input_sender, inputs) = mpsc::channel();
        let (ready_sender, ready) = mpsc::sync_channel(1);
        let handler_cancellations = Arc::new(Mutex::new(Vec::new()));
        let commands = handler_cancellations.clone();
        let thread = thread::spawn(move || {
            let result = run(
                state,
                conversation,
                notebook,
                receiver,
                &input_sender,
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
            thread: Some(thread),
        })
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
        let _ = self.updates.send(Update::Panel(panel));
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

fn run(
    mut state: ScreenState,
    mut conversation: Conversation,
    mut notebook: Notebook,
    updates: mpsc::Receiver<Update>,
    inputs: &mpsc::Sender<Input>,
    ready: mpsc::SyncSender<Result<(), String>>,
    handler_cancellations: Arc<Mutex<Vec<String>>>,
) -> io::Result<()> {
    let mut pending_events = VecDeque::new();
    let setup = (|| {
        let _guard = super::lock(&DRAWING);
        enable_raw_mode()?;
        ACTIVE.store(true, Ordering::SeqCst);
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableBracketedPaste,
            EnableMouseCapture
        )?;
        Terminal::new(CrosstermBackend::new(io::stdout()))
    })();
    let _restore = Restore;
    let mut terminal = match setup {
        Ok(terminal) => {
            let _ = ready.send(Ok(()));
            terminal
        }
        Err(error) => {
            let _ = ready.send(Err(error.to_string()));
            return Err(error);
        }
    };
    let mut editor = Editor::default();
    let mut served = ServedBy::default();
    let mut busy = false;
    let started = Instant::now();
    state.activity = Activity::Starting;
    let mut dirty = true;
    let mut last_tick = Instant::now();
    let mut task_started: Option<Instant> = None;
    let mut previous_rows = 0usize;
    let mut viewport_height = 10usize;
    loop {
        if !ACTIVE.load(Ordering::SeqCst) {
            break;
        }
        for update in updates.try_iter() {
            dirty = true;
            match update {
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
                Update::Panel(panel) => state.panel = Some(panel),
                Update::Notice(message) => state.notice = Some(message),
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
        if dirty {
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
                tui::render_screen(
                    frame,
                    &conversation,
                    &served,
                    &super::empty_handles(),
                    &notebook,
                    &state,
                )
            })?;
            io::stdout().flush()?;
            dirty = false;
        }
        if pending_events.is_empty()
            && !event::poll(Duration::from_millis(if moving { 40 } else { 100 }))?
        {
            continue;
        }
        let Some(input_event) = terminal_input::read(&mut pending_events)? else {
            continue;
        };
        match input_event {
            Event::Resize(_, _) => {
                dirty = true;
            }
            Event::Mouse(mouse) => {
                let up = mouse.kind == MouseEventKind::ScrollUp;
                if up || mouse.kind == MouseEventKind::ScrollDown {
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
                if !state
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
                            state.panel = None;
                        }
                        KeyCode::Up => panel.move_selection(false, 1),
                        KeyCode::Down => panel.move_selection(true, 1),
                        KeyCode::PageUp => panel.move_selection(false, 10),
                        KeyCode::PageDown => panel.move_selection(true, 10),
                        KeyCode::Enter => {
                            if let Some(command) = panel
                                .rows
                                .get(panel.selected)
                                .and_then(|r| r.command.clone())
                            {
                                if let Some(theme) =
                                    command.strip_prefix("/theme ").and_then(tui::Theme::parse)
                                {
                                    state.theme = theme;
                                    state.panel = None;
                                    state.notice = Some(format!("Theme: {}", theme.name()));
                                } else if let Some(name) = command.strip_prefix("/handlers off ") {
                                    super::lock(&handler_cancellations).push(name.to_string());
                                    state.panel = None;
                                    state.notice = Some(format!(
                                        "handler {name}: cancellation queued for the next cell boundary"
                                    ));
                                } else if !busy {
                                    state.panel = None;
                                    busy = true;
                                    let _ = inputs.send(Input::Submit(command));
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
                    if !busy {
                        busy = true;
                        let _ = inputs
                            .send(Input::Submit(format!("/mode {}", state.mode.next().name())));
                    } else {
                        state.notice = Some("Change mode after the current task finishes.".into());
                    }
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
                        KeyCode::Home => {
                            state.scrollback = previous_rows.saturating_sub(viewport_height);
                            continue;
                        }
                        KeyCode::End => {
                            state.scrollback = 0;
                            continue;
                        }
                        KeyCode::Char('d') if !busy && editor.text.is_empty() => {
                            let _ = inputs.send(Input::Exit);
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
                            state.inspection = tui::Inspection::open(cell, &notebook);
                            state.telemetry_open = false;
                            state.panel = None;
                            state.notice = if state.inspection.is_none() {
                                Some("No recorded cell at that number yet. Use /cells after an action.".into())
                            } else {
                                None
                            };
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
                    if editor.text.split_whitespace().next() == Some("/motion") {
                        let text = editor.take();
                        match text.split_whitespace().nth(1) {
                            Some("off" | "reduce") => state.reduced_motion = true,
                            Some("on") => state.reduced_motion = false,
                            _ => {
                                state.notice = Some("Usage: /motion on | off".into());
                                continue;
                            }
                        }
                        state.completion_tick = None;
                        state.notice = Some(
                            if state.reduced_motion {
                                "Motion reduced. /motion on restores animation."
                            } else {
                                "Motion on. /motion off reduces animation."
                            }
                            .into(),
                        );
                        continue;
                    }
                    if editor.text.split_whitespace().next() == Some("/theme") {
                        let text = editor.take();
                        match text.split_whitespace().nth(1) {
                            Some(name) => {
                                if let Some(theme) = tui::Theme::parse(name) {
                                    state.theme = theme;
                                    state.notice = Some(format!(
                                        "Theme: {} · /theme opens the palette",
                                        theme.name()
                                    ));
                                } else {
                                    state.notice =
                                        Some("Unknown theme. /theme opens the palette.".into());
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
                                    state.notice = Some(format!(
                                        "handler {name}: cancellation queued for the next cell boundary"
                                    ));
                                } else {
                                    state.notice = Some(format!(
                                        "handler {name}: no active handler with that name"
                                    ));
                                }
                            }
                            _ => {
                                state.notice = Some("Use /handlers or /handlers off <name>".into())
                            }
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
                        let _ = inputs.send(Input::Exit);
                        return Ok(());
                    }
                    if text.split_whitespace().next() == Some("/statusline") {
                        state.status_line = match text.split_whitespace().nth(1) {
                            Some("compact") => tui::StatusLine::Compact,
                            Some("hide") | Some("hidden") => tui::StatusLine::Hidden,
                            Some("full") => tui::StatusLine::Full,
                            _ => {
                                state.notice = Some("Use /statusline full|compact|hide".into());
                                continue;
                            }
                        };
                        continue;
                    }
                    if text.split_whitespace().next() == Some("/sidebar") {
                        state.sidebar = match text.split_whitespace().nth(1) {
                            Some("hide") => SidebarVisibility::Hidden,
                            Some("show") => SidebarVisibility::Shown,
                            _ => SidebarVisibility::Auto,
                        };
                        state.notice =
                            Some("Sidebar: /sidebar auto|show|hide · Ctrl-B toggles".into());
                        continue;
                    }
                    busy = true;
                    task_started = Some(Instant::now());
                    state.pulse = tui::Pulse::default();
                    state.activity = Activity::Thinking;
                    if inputs.send(Input::Submit(text)).is_err() {
                        return Ok(());
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
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
        assert_eq!(editor.text, "/entitlements ");
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
}
