//! Input reduction for the new surface: click activates only on release.
use super::{Action, CellTab, Workbench};
use crate::tui::{Notebook, ScreenState, Selection};
use crossterm::event::{Event, KeyCode, KeyModifiers, MouseButton, MouseEventKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Insert(String),
    OpenPath(String),
    Pass,
    Consumed,
    Command(String),
    Copy(String),
    Cursor(usize),
}
impl Workbench {
    pub fn local_command(&mut self, text: &str, s: &mut ScreenState, n: &Notebook) -> bool {
        let parts: Vec<_> = text.split_whitespace().collect();
        match parts.as_slice() {
            ["/settings" | "/statusline"] => {
                self.open_settings(s);
                true
            }
            ["/config"] => {
                self.open_settings(s);
                if let Some(p) = &mut self.preferences {
                    p.category = 5;
                }
                true
            }
            ["/diff"] => {
                if !n.cells.is_empty() {
                    let cell = n.cells.len();
                    self.expanded.insert(cell);
                    self.collapsed.remove(&cell);
                    self.tabs.insert(cell, CellTab::Diff);
                    self.selected_cell = Some(cell);
                    s.scrollback = 0;
                } else {
                    self.notice = "No recorded cell diff yet.".into();
                }
                true
            }
            ["/cell", number] => {
                if let Ok(cell) = number.parse::<usize>() {
                    if cell > 0 && cell <= n.cells.len() {
                        self.expanded.insert(cell);
                        self.collapsed.remove(&cell);
                        self.selected_cell = Some(cell);
                        self.jump_cell = Some(cell);
                        self.notice = format!("Cell {cell} expanded · F4 opens its diff");
                    } else {
                        self.notice = "No recorded cell at that number.".into();
                    }
                }
                true
            }
            ["/cells"] => {
                self.expanded.extend(1..=n.cells.len());
                self.collapsed.clear();
                s.scrollback = 0;
                true
            }
            ["/chat"] => {
                self.close();
                s.inspection = None;
                s.telemetry_open = false;
                true
            }
            ["/activity" | "/telemetry"] => {
                self.close();
                self.activity = true;
                true
            }
            ["/mode"] => {
                self.close();
                self.work = true;
                true
            }
            ["/permissions"] => {
                self.close();
                self.approvals = true;
                true
            }
            _ => false,
        }
    }
    pub fn event(&mut self, e: &Event, s: &mut ScreenState, n: &Notebook, busy: bool) -> Effect {
        match e {
            Event::Mouse(m) => {
                if s.mouse_off {
                    return Effect::Consumed;
                }
                match m.kind {
                    MouseEventKind::Down(MouseButton::Left) => {
                        self.press = Some((m.column, m.row));
                        self.dragged = false;
                        s.selection = None;
                        Effect::Consumed
                    }
                    MouseEventKind::Drag(MouseButton::Left) => {
                        if let Some(anchor) = self.press {
                            self.dragged = true;
                            s.selection = Some(Selection {
                                anchor,
                                head: (m.column, m.row),
                            });
                        }
                        Effect::Consumed
                    }
                    MouseEventKind::Up(MouseButton::Left) => {
                        let anchor = self.press.take();
                        if self.dragged {
                            self.dragged = false;
                            let copied = match (self.geometry.screen.as_ref(), s.selection) {
                                (Some(screen), Some(selection)) => {
                                    let mut screen = screen.clone();
                                    let area = screen.area;
                                    crate::tui::draw_selection(&mut screen, area, selection)
                                }
                                _ => String::new(),
                            };
                            return if copied.is_empty() {
                                Effect::Consumed
                            } else {
                                Effect::Copy(copied)
                            };
                        }
                        if anchor != Some((m.column, m.row)) {
                            return Effect::Consumed;
                        }
                        if let Some(action) = self.geometry.hit(m.column, m.row) {
                            if action == Action::Composer {
                                let width = self.geometry.composer.width as usize;
                                let lines = super::view::wrap_input(&s.input, width);
                                let cursor = s.cursor.unwrap_or(s.input.len()).min(s.input.len());
                                let before = &s.input[..s.input.floor_char_boundary(cursor)];
                                let cr = super::view::wrap_input(before, width)
                                    .len()
                                    .saturating_sub(1);
                                let skip = cr.saturating_sub(
                                    self.geometry.composer.height.saturating_sub(1) as usize,
                                );
                                let wanted =
                                    m.row.saturating_sub(self.geometry.composer.y) as usize + skip;
                                let col =
                                    m.column.saturating_sub(self.geometry.composer.x) as usize;
                                let mut offset = 0;
                                for (i, line) in lines.iter().enumerate() {
                                    if i == wanted {
                                        let mut x = 0;
                                        for (byte, ch) in line.char_indices() {
                                            let w =
                                                ratatui::text::Span::raw(ch.to_string()).width();
                                            if x + w > col {
                                                return Effect::Cursor(offset + byte);
                                            }
                                            x += w;
                                        }
                                        return Effect::Cursor(
                                            (offset + line.len()).min(s.input.len()),
                                        );
                                    }
                                    offset += line.len();
                                    if s.input.as_bytes().get(offset) == Some(&b'\n') {
                                        offset += 1;
                                    }
                                }
                                return Effect::Cursor(s.input.len());
                            }
                            return self.activate(action, s, n, busy);
                        }
                        Effect::Consumed
                    }
                    MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                        let up = m.kind == MouseEventKind::ScrollUp;
                        if let Some(p) = &mut self.preferences {
                            p.selected = move_index(p.selected, up, 3, p.rows().len());
                        } else if let Some(m) = &mut self.models {
                            m.selected = move_index(m.selected, up, 3, m.candidates().len());
                        } else if let Some(p) = &mut s.panel {
                            p.selected = move_index(p.selected, up, 3, p.rows.len());
                        } else if self.is_local() {
                            self.local_scroll = if up {
                                self.local_scroll.saturating_sub(3)
                            } else {
                                self.local_scroll.saturating_add(3)
                            };
                        } else {
                            s.scrollback = if up {
                                s.scrollback.saturating_add(3).min(
                                    self.geometry
                                        .rows
                                        .saturating_sub(self.geometry.transcript.height as usize),
                                )
                            } else {
                                s.scrollback.saturating_sub(3)
                            };
                            s.scrolling = true;
                        }
                        s.selection = None;
                        Effect::Consumed
                    }
                    _ => Effect::Consumed,
                }
            }
            Event::Paste(text) => {
                if let Some(p) = &mut self.preferences {
                    if let Some((_, v)) = &mut p.editing {
                        v.push_str(&text.chars().filter(|c| !c.is_control()).collect::<String>());
                    } else {
                        p.query.push_str(text);
                        p.selected = 0;
                    }
                    return Effect::Consumed;
                }
                if let Some(m) = &mut self.models {
                    m.query.push_str(text);
                    m.selected = 0;
                    return Effect::Consumed;
                }
                if self.is_local() || s.panel.is_some() {
                    Effect::Consumed
                } else {
                    Effect::Pass
                }
            }
            Event::Key(k) => {
                if k.kind == crossterm::event::KeyEventKind::Release {
                    return Effect::Consumed;
                }
                s.selection = None;
                self.notice.clear();
                let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
                if ctrl && k.code == KeyCode::Char('c') {
                    self.close();
                    return Effect::Pass;
                }
                if k.code == KeyCode::Esc && (self.is_local() || s.panel.is_some()) {
                    if let Some(p) = &mut self.preferences {
                        if p.editing.take().is_some() {
                            return Effect::Consumed;
                        }
                        if !p.query.is_empty() {
                            p.query.clear();
                            p.selected = 0;
                            return Effect::Consumed;
                        }
                    }
                    self.close();
                    if let Some((preferences, _)) = self.model_preference.take() {
                        self.preferences = Some(preferences);
                    }
                    s.panel = None;
                    s.selection = None;
                    return Effect::Consumed;
                }
                if let Some(p) = &mut self.preferences {
                    if let Some((key, buffer)) = &mut p.editing {
                        match k.code {
                            KeyCode::Enter => {
                                let (key, value) = (key.clone(), buffer.clone());
                                match p.save(&key, Some(value), s) {
                                    Ok(()) => p.editing = None,
                                    Err(e) => p.notice = e,
                                }
                            }
                            KeyCode::Backspace => {
                                buffer.pop();
                            }
                            KeyCode::Char('u') if ctrl => buffer.clear(),
                            KeyCode::Char(c) if !ctrl => buffer.push(c),
                            _ => {}
                        }
                        return Effect::Consumed;
                    }
                    let spec = p.rows().get(p.selected).copied();
                    let result = match k.code {
                        KeyCode::Up => {
                            p.selected = p.selected.saturating_sub(1);
                            Ok(())
                        }
                        KeyCode::Down => {
                            p.selected = (p.selected + 1).min(p.rows().len().saturating_sub(1));
                            Ok(())
                        }
                        KeyCode::PageUp => {
                            p.selected = p.selected.saturating_sub(8);
                            Ok(())
                        }
                        KeyCode::PageDown => {
                            p.selected = (p.selected + 8).min(p.rows().len().saturating_sub(1));
                            Ok(())
                        }
                        KeyCode::Tab => {
                            p.category = (p.category + 1) % 6;
                            p.query.clear();
                            p.selected = 0;
                            Ok(())
                        }
                        KeyCode::BackTab => {
                            p.category = (p.category + 5) % 6;
                            p.query.clear();
                            p.selected = 0;
                            Ok(())
                        }
                        KeyCode::F(6) => p.switch_scope(),
                        KeyCode::Char('z') if ctrl => p.undo(s),
                        KeyCode::Backspace => {
                            if !p.query.is_empty() {
                                p.query.pop();
                                p.selected = 0;
                                Ok(())
                            } else if let Some(spec) = spec {
                                p.save(spec.key, None, s)
                            } else {
                                Ok(())
                            }
                        }
                        KeyCode::Left | KeyCode::Right => {
                            if spec.is_some_and(|s| s.kind == crate::settings::Kind::Model) {
                                if !busy {
                                    self.browse_preference(spec.expect("model row").key);
                                    return Effect::Command("/models".into());
                                }
                                Ok(())
                            } else {
                                p.cycle(k.code == KeyCode::Right, s)
                            }
                        }
                        KeyCode::Enter => {
                            if let Some(spec) = spec {
                                if spec.kind == crate::settings::Kind::Model {
                                    if busy {
                                        p.notice =
                                            "Open model selection after the current turn.".into();
                                        return Effect::Consumed;
                                    }
                                    self.browse_preference(spec.key);
                                    return Effect::Command("/models".into());
                                } else if super::Preferences::choices(spec).is_empty() {
                                    p.editing = Some((spec.key.into(), p.effective(spec.key)));
                                    Ok(())
                                } else {
                                    p.cycle(true, s)
                                }
                            } else {
                                Ok(())
                            }
                        }
                        KeyCode::Char(c) if !ctrl => {
                            p.query.push(c);
                            p.selected = 0;
                            Ok(())
                        }
                        _ => Ok(()),
                    };
                    if let Err(e) = result {
                        p.notice = e;
                    }
                    return Effect::Consumed;
                }
                if let Some(m) = &mut self.models {
                    match k.code {
                        KeyCode::Up => m.selected = m.selected.saturating_sub(1),
                        KeyCode::Down => {
                            m.selected =
                                (m.selected + 1).min(m.candidates().len().saturating_sub(1))
                        }
                        KeyCode::PageUp => m.selected = m.selected.saturating_sub(10),
                        KeyCode::PageDown => {
                            m.selected =
                                (m.selected + 10).min(m.candidates().len().saturating_sub(1))
                        }
                        KeyCode::Tab if m.target_key.is_none() => {
                            m.role = (m.role + 1) % 3;
                            m.selected = 0;
                        }
                        KeyCode::Left => {
                            m.provider = m.provider.saturating_sub(1);
                            m.selected = 0;
                        }
                        KeyCode::Right => {
                            m.provider =
                                (m.provider + 1).min(m.providers().len().saturating_sub(1));
                            m.selected = 0;
                        }
                        KeyCode::Char('o') if ctrl => m.measured_order = !m.measured_order,
                        KeyCode::Char('u') if ctrl => {
                            m.query.clear();
                            m.selected = 0;
                        }
                        KeyCode::F(7) if m.role == 2 && m.target_key.is_none() => {
                            let names = crate::config::SLOT_NAMES;
                            let next = m
                                .slot
                                .as_deref()
                                .and_then(|s| names.iter().position(|n| *n == s))
                                .map(|i| i + 1)
                                .unwrap_or(0);
                            m.slot = names.get(next).map(|s| s.to_string());
                            m.select_current();
                        }
                        KeyCode::F(6) => {
                            m.all_sources = !m.all_sources;
                            m.provider = 0;
                            m.selected = 0;
                        }
                        KeyCode::Enter => return self.activate(Action::ChooseModel, s, n, busy),
                        KeyCode::Backspace => {
                            m.query.pop();
                            m.selected = 0;
                        }
                        KeyCode::Char(c) if !ctrl => {
                            m.query.push(c);
                            m.selected = 0;
                        }
                        _ => {}
                    }
                    return Effect::Consumed;
                }
                if self.confirm.is_some() {
                    if k.code == KeyCode::Enter {
                        let rung = self.confirm.clone().unwrap_or_default();
                        return self.activate(Action::Rung(rung), s, n, busy);
                    }
                    return Effect::Consumed;
                }
                if self.work || self.approvals {
                    let count = if self.work { 3 } else { 4 };
                    match k.code {
                        KeyCode::Up => self.local_scroll = self.local_scroll.saturating_sub(1),
                        KeyCode::Down => self.local_scroll = (self.local_scroll + 1).min(count - 1),
                        KeyCode::Enter => {
                            let action = if self.work {
                                Action::Command(format!(
                                    "/mode {}",
                                    ["execute", "explore", "plan"][self.local_scroll.min(2)]
                                ))
                            } else {
                                Action::Rung(
                                    ["manual", "accept-edits", "auto", "full"]
                                        [self.local_scroll.min(3)]
                                    .into(),
                                )
                            };
                            return self.activate(action, s, n, busy);
                        }
                        _ => {}
                    }
                    return Effect::Consumed;
                }
                if self.activity || self.access {
                    match k.code {
                        KeyCode::Up | KeyCode::PageUp => {
                            self.local_scroll = self.local_scroll.saturating_sub(3)
                        }
                        KeyCode::Down | KeyCode::PageDown => self.local_scroll += 3,
                        _ => {}
                    }
                    return Effect::Consumed;
                }
                if let Some(p) = &mut s.panel {
                    match k.code {
                        KeyCode::Up => p.selected = p.selected.saturating_sub(1),
                        KeyCode::Down => {
                            p.selected = (p.selected + 1).min(p.rows.len().saturating_sub(1))
                        }
                        KeyCode::PageUp => p.selected = p.selected.saturating_sub(10),
                        KeyCode::PageDown => {
                            p.selected = (p.selected + 10).min(p.rows.len().saturating_sub(1))
                        }
                        KeyCode::Enter => {
                            if let Some(cmd) = Self::panel_command(p) {
                                return Effect::Command(cmd);
                            }
                        }
                        _ => {}
                    }
                    return Effect::Consumed;
                }
                match k.code {
                    KeyCode::Char('t') if ctrl => {
                        self.close();
                        self.activity = true;
                        Effect::Consumed
                    }
                    KeyCode::F(2) => {
                        self.open_settings(s);
                        Effect::Consumed
                    }
                    KeyCode::F(3) => self.activate(Action::Models, s, n, busy),
                    KeyCode::F(4) => {
                        let cell = self.selected_cell.unwrap_or(n.cells.len());
                        self.activate(Action::Tab(cell, CellTab::Diff), s, n, busy)
                    }
                    KeyCode::F(5) => {
                        let cell = self.selected_cell.unwrap_or(n.cells.len());
                        self.activate(Action::Tab(cell, CellTab::Helpers), s, n, busy)
                    }
                    KeyCode::BackTab => {
                        self.close();
                        self.approvals = true;
                        Effect::Consumed
                    }
                    KeyCode::Char('o') if ctrl => self.activate(
                        Action::Cell(self.selected_cell.unwrap_or(n.cells.len())),
                        s,
                        n,
                        busy,
                    ),
                    _ => Effect::Pass,
                }
            }
            _ => Effect::Pass,
        }
    }
    fn activate(
        &mut self,
        action: Action,
        s: &mut ScreenState,
        n: &Notebook,
        busy: bool,
    ) -> Effect {
        match action {
            Action::Insert(command) => return Effect::Insert(command),
            Action::Path(path) => return Effect::OpenPath(path),
            Action::Cell(cell) => {
                if cell > 0 {
                    self.selected_cell = Some(cell);
                    let open = self.expanded.contains(&cell)
                        || (cell >= n.cells.len() && !self.collapsed.contains(&cell));
                    if open {
                        self.expanded.remove(&cell);
                        self.collapsed.insert(cell);
                    } else {
                        self.expanded.insert(cell);
                        self.collapsed.remove(&cell);
                    }
                }
            }
            Action::Tab(cell, tab) => {
                if cell > 0 {
                    self.expanded.insert(cell);
                    self.collapsed.remove(&cell);
                    self.tabs.insert(cell, tab);
                    self.selected_cell = Some(cell);
                }
            }
            Action::Helper(cell, h) => {
                self.helper = if self.helper == Some((cell, h)) {
                    None
                } else {
                    Some((cell, h))
                };
            }
            Action::Latest => s.scrollback = 0,
            Action::Settings => self.open_settings(s),
            Action::Models => {
                if busy {
                    self.notice =
                        "Change models after the current turn; in-flight calls retain their model."
                            .into();
                } else {
                    self.close();
                    return Effect::Command("/models".into());
                }
            }
            Action::Work => {
                self.close();
                self.work = true;
            }
            Action::Approvals => {
                self.close();
                self.approvals = true;
            }
            Action::Access => {
                self.close();
                self.access = true;
            }
            Action::Activity => {
                self.close();
                self.activity = true;
            }
            Action::Close => {
                self.close();
                if let Some((p, _)) = self.model_preference.take() {
                    self.preferences = Some(p);
                }
                s.panel = None;
                s.selection = None;
            }
            Action::Scope => {
                if let Some(p) = &mut self.preferences
                    && let Err(e) = p.switch_scope()
                {
                    p.notice = e;
                }
            }
            Action::Undo => {
                if let Some(p) = &mut self.preferences
                    && let Err(e) = p.undo(s)
                {
                    p.notice = e;
                }
            }
            Action::Category(c) => {
                if let Some(p) = &mut self.preferences {
                    p.category = c.min(5);
                    p.selected = 0;
                    p.query.clear();
                }
            }
            Action::Setting(i, value) => {
                if let Some(p) = &mut self.preferences {
                    p.selected = i;
                    let spec = p.rows().get(i).copied();
                    if let Some(spec) = spec {
                        if let Some(value) = value {
                            if spec.key.starts_with("permissions.") || spec.key == "agents.mode" {
                                p.editing = Some((spec.key.into(), value));
                            } else if let Err(e) = p.save(spec.key, Some(value), s) {
                                p.notice = e;
                            }
                        } else if spec.kind == crate::settings::Kind::Model {
                            if !busy {
                                self.browse_preference(spec.key);
                                return Effect::Command("/models".into());
                            } else {
                                p.notice = "Change models after the current turn.".into();
                            }
                        } else if super::Preferences::choices(spec).is_empty() {
                            p.editing = Some((spec.key.into(), p.effective(spec.key)));
                        }
                    }
                }
            }
            Action::Slot(slot) => {
                if let Some(m) = &mut self.models
                    && m.target_key.is_none()
                {
                    m.slot = slot;
                    m.select_current();
                }
            }
            Action::ModelRole(r) => {
                if let Some(m) = &mut self.models
                    && m.target_key.is_none()
                {
                    m.role = r.min(2);
                    m.selected = 0;
                }
            }
            Action::Provider(i) => {
                if let Some(m) = &mut self.models {
                    m.provider = i.min(m.providers().len().saturating_sub(1));
                    m.selected = 0;
                }
            }
            Action::Model(i) => {
                if let Some(m) = &mut self.models {
                    m.selected = i;
                }
            }
            Action::UnsetModel => {
                if let Some((mut p, key)) = self.model_preference.take() {
                    if let Err(e) = p.save(&key, None, s) {
                        p.notice = e;
                    }
                    self.close();
                    self.preferences = Some(p);
                }
            }
            Action::ChooseModel => {
                if let Some(m) = &mut self.models {
                    if busy {
                        m.notice = "The current turn must finish before changing models.".into();
                    } else {
                        match m.choose() {
                            Ok(cmd) => {
                                if let Some((mut preferences, key)) = self.model_preference.take() {
                                    let selected =
                                        m.candidates().get(m.selected).map(|c| c.model.clone());
                                    if let Some(selected) = selected
                                        && let Err(error) =
                                            preferences.save(&key, Some(selected), s)
                                    {
                                        preferences.notice = error;
                                    }
                                    self.close();
                                    self.preferences = Some(preferences);
                                } else {
                                    self.close();
                                    return Effect::Command(cmd);
                                }
                            }
                            Err(e) => m.notice = e,
                        }
                    }
                }
            }
            Action::Sources => {
                if let Some(m) = &mut self.models {
                    m.all_sources = !m.all_sources;
                    m.provider = 0;
                    m.selected = 0;
                }
            }
            Action::Scores => {
                if let Some(m) = &mut self.models {
                    m.measured_order = !m.measured_order;
                    m.selected = 0;
                }
            }
            Action::Command(cmd) => {
                if busy {
                    self.notice="This runtime change applies between turns; finish or stop the current turn first.".into();
                } else {
                    self.close();
                    return Effect::Command(cmd);
                }
            }
            Action::Rung(rung) => {
                if rung == "full" && self.confirm.as_deref() != Some("full") {
                    self.close();
                    self.confirm = Some(rung);
                } else if let Some(r) = crate::permissions::Rung::parse(&rung) {
                    s.permissions.set(r);
                    self.close();
                    self.notice = format!("Ask: {rung} · sandbox and denials unchanged");
                }
            }
            Action::PanelRow(i) => {
                if let Some(p) = &mut s.panel {
                    p.selected = i;
                    if let Some(cmd) = Self::panel_command(p) {
                        return Effect::Command(cmd);
                    }
                }
            }
            Action::Composer => {}
        }
        Effect::Consumed
    }
}
fn move_index(i: usize, up: bool, step: usize, len: usize) -> usize {
    if up {
        i.saturating_sub(step)
    } else {
        i.saturating_add(step).min(len.saturating_sub(1))
    }
}
