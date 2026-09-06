//! One thread owns the terminal and keys; the task thread only sends view state.
use std::cell::RefCell;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::contract::{Conversation, ServedBy};
use crate::tui::{self, Activity, Notebook, ScreenState, SidebarVisibility};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::{Terminal, backend::CrosstermBackend};

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

enum Update {
    Snapshot(Box<(Conversation, Notebook, ServedBy, Activity)>),
    Model(String),
    Delta(String),
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
        let thread = thread::spawn(move || {
            let result = run(
                state,
                conversation,
                notebook,
                receiver,
                &input_sender,
                ready_sender,
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
            updates,
            inputs,
            thread: Some(thread),
        })
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
    pub(super) fn append_delta(&self, text: &str) {
        let _ = self.updates.send(Update::Delta(text.into()));
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
        // Strip terminal control bytes; newlines and tabs are composition.
        let text: String = text
            .chars()
            .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
            .collect();
        self.text.insert_str(self.cursor, &text);
        self.cursor += text.len();
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
) -> io::Result<()> {
    let setup = (|| {
        let _guard = super::lock(&DRAWING);
        enable_raw_mode()?;
        ACTIVE.store(true, Ordering::SeqCst);
        execute!(io::stdout(), EnterAlternateScreen, EnableBracketedPaste)?;
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
                    conversation = c;
                    notebook = n;
                    if s.is_known() {
                        served = s;
                    }
                    state.activity = activity;
                    state.streaming_text = None;
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
        if !event::poll(Duration::from_millis(if moving { 40 } else { 100 }))? {
            continue;
        }
        match event::read()? {
            Event::Resize(_, _) => {
                dirty = true;
            }
            Event::Paste(text) => {
                editor.insert(&text);
                dirty = true;
            }
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                dirty = true;
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
                    match key.code {
                        KeyCode::Esc => {
                            state.panel = None;
                        }
                        KeyCode::Up => panel.selected = panel.selected.saturating_sub(1),
                        KeyCode::Down => {
                            panel.selected =
                                (panel.selected + 1).min(panel.rows.len().saturating_sub(1))
                        }
                        KeyCode::PageUp => panel.selected = panel.selected.saturating_sub(10),
                        KeyCode::PageDown => {
                            panel.selected =
                                (panel.selected + 10).min(panel.rows.len().saturating_sub(1))
                        }
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
                        KeyCode::Char('d') if !busy && editor.text.is_empty() => {
                            let _ = inputs.send(Input::Exit);
                            return Ok(());
                        }
                        _ => {}
                    }
                }
                match key.code {
                    KeyCode::PageUp => {
                        state.scrollback = state.scrollback.saturating_add(10);
                        continue;
                    }
                    KeyCode::PageDown => {
                        state.scrollback = state.scrollback.saturating_sub(10);
                        continue;
                    }
                    _ => {}
                }
                if editor.key(key) && !editor.text.trim().is_empty() {
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
                                });
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

#[cfg(test)]
mod tests {
    use super::*;
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
