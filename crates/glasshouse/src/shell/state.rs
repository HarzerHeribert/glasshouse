//! What the shell knows and how keys change it.
//!
//! Kept apart from rendering for the same reason the wizard is: state that
//! answers keys without drawing anything can be tested exhaustively without a
//! terminal, and a view that only reads state cannot accidentally become the
//! place a decision is made.
//!
//! Nothing here starts, stops, or touches a process. Moving between sessions
//! changes which one is *presented*, and that distinction is the whole point of
//! the capability: a user flipping through sessions must never be terminating
//! them.

use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::session::SessionRecord;

/// A Glasshouse-owned screen drawn over the session viewport.
///
/// Overlays are transient by design: they are somewhere the user visits and
/// leaves, and leaving returns to the session that was already active rather
/// than closing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    /// Every session in the project, with the detail the bar has no room for.
    Overview,
}

/// What the run loop should do after a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Nothing changed; do not spend a frame.
    None,
    Redraw,
    /// Leave Glasshouse. Sessions are not affected — see [`ShellState`]'s note
    /// about presentation versus lifecycle.
    Quit,
}

/// Everything the shell displays.
pub struct ShellState {
    project_name: String,
    project_root: PathBuf,
    version: String,
    sessions: Vec<SessionRecord>,
    /// Index into `sessions`. Meaningless when `sessions` is empty, and every
    /// accessor guards for that rather than trusting it.
    selected: usize,
    overlay: Option<Overlay>,
    /// A one-line note for the status bar, cleared by the next keystroke.
    ///
    /// Its job is to explain a key that appeared to do nothing. Without it a
    /// user pressing Tab in a project with one session sees a dead keyboard
    /// and has no way to tell that from a bug.
    status: Option<String>,
}

impl ShellState {
    pub fn new(
        project_name: impl Into<String>,
        project_root: impl Into<PathBuf>,
        version: impl Into<String>,
        sessions: Vec<SessionRecord>,
    ) -> Self {
        Self {
            project_name: project_name.into(),
            project_root: project_root.into(),
            version: version.into(),
            sessions,
            selected: 0,
            overlay: None,
            status: None,
        }
    }

    pub fn project_name(&self) -> &str {
        &self.project_name
    }

    /// The active canonical project root, which the top bar shows on every
    /// frame. This is the value the whole isolation model is built on, so it
    /// is never abbreviated away entirely — the view truncates from the left
    /// when it must, keeping the tail, which is the part that identifies the
    /// project.
    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn sessions(&self) -> &[SessionRecord] {
        &self.sessions
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    /// The session currently presented, or `None` when the project has none
    /// yet.
    pub fn active_session(&self) -> Option<&SessionRecord> {
        self.sessions.get(self.selected)
    }

    pub fn overlay(&self) -> Option<Overlay> {
        self.overlay
    }

    /// The current status note, if any.
    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    /// Show a note in the status bar until the next keystroke.
    pub fn set_status(&mut self, message: impl Into<String>) {
        self.status = Some(message.into());
    }

    /// Present the next session, wrapping at the end.
    ///
    /// Wrapping rather than stopping: a session bar is a ring the user tabs
    /// through, and stopping dead at the last entry reads as a broken key.
    pub fn next_session(&mut self) -> Action {
        self.step(1)
    }

    /// Present the previous session, wrapping at the start.
    pub fn previous_session(&mut self) -> Action {
        self.step(-1)
    }

    fn step(&mut self, delta: isize) -> Action {
        if self.sessions.len() < 2 {
            // Nothing to move between. Say so rather than letting the key look
            // broken — but the selection genuinely did not change, so this is
            // the only reason there is to redraw.
            self.set_status(match self.sessions.len() {
                0 => "no sessions yet — start one with `glasshouse launch`",
                _ => "this project has only one session",
            });
            return Action::Redraw;
        }
        let len = self.sessions.len() as isize;
        let next = (self.selected as isize + delta).rem_euclid(len);
        self.selected = next as usize;
        Action::Redraw
    }

    /// Open the session overview.
    pub fn open_overview(&mut self) -> Action {
        if self.overlay == Some(Overlay::Overview) {
            return Action::None;
        }
        self.overlay = Some(Overlay::Overview);
        Action::Redraw
    }

    /// Leave whatever overlay is open and go back to the active session.
    ///
    /// Returns [`Action::None`] when no overlay is open, which is what lets the
    /// caller distinguish "the user dismissed an overlay" from "the user asked
    /// to leave Glasshouse" using the same key.
    pub fn close_overlay(&mut self) -> Action {
        if self.overlay.is_none() {
            return Action::None;
        }
        self.overlay = None;
        Action::Redraw
    }

    /// Replace the session list, keeping the same session presented if it is
    /// still there.
    ///
    /// Reconciling by identity rather than by index: sessions are ordered by
    /// last activity, so any refresh can reorder them, and holding an index
    /// would silently move the user to a different session.
    pub fn refresh(&mut self, sessions: Vec<SessionRecord>) -> Action {
        let active = self.active_session().map(|record| record.id.clone());
        let unchanged = sessions == self.sessions;
        self.sessions = sessions;
        self.selected = active
            .and_then(|id| self.sessions.iter().position(|record| record.id == id))
            .unwrap_or(0);
        if unchanged {
            Action::None
        } else {
            Action::Redraw
        }
    }

    /// Answer one key.
    ///
    /// Bindings are plain single keys because no native session owns the
    /// keyboard yet. Once one does (Phase 5), Glasshouse's own keys will have
    /// to move behind a prefix or a mode, or they will steal keystrokes the
    /// harness needs — this is deliberately the only place that has to change.
    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        // A note explains the key that was just pressed, so the next key
        // clears it rather than leaving stale text under a new action.
        let had_status = self.status.take().is_some();

        // An overlay takes the keys it understands before anything else, so
        // Escape means "leave this overlay" while one is open and "leave
        // Glasshouse" only when none is.
        if self.overlay.is_some() && matches!(key.code, KeyCode::Esc | KeyCode::Char('o')) {
            return self.close_overlay();
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('c' | 'C') if ctrl => Action::Quit,
            KeyCode::Char('q') => Action::Quit,
            KeyCode::Esc => Action::Quit,
            KeyCode::Tab | KeyCode::Right => self.next_session(),
            KeyCode::BackTab | KeyCode::Left => self.previous_session(),
            KeyCode::Char('o') => self.open_overview(),
            // Clearing a note is itself a visible change.
            _ if had_status => Action::Redraw,
            _ => Action::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{SessionId, SessionLifecycle, SessionPresentation, SessionRole};

    fn record(id: &str, harness: &str) -> SessionRecord {
        SessionRecord {
            id: SessionId::new(id),
            project_id: "project".to_owned(),
            harness: harness.to_owned(),
            native_session_id: None,
            role: SessionRole::Normal,
            lifecycle: SessionLifecycle::Running,
            presentation: SessionPresentation::Embedded,
            created_at: 1_000,
            last_activity_at: 1_000,
        }
    }

    fn state_with(count: usize) -> ShellState {
        let sessions = (0..count)
            .map(|i| record(&format!("id-{i}"), "claude-code"))
            .collect();
        ShellState::new("glasshouse", "/projects/glasshouse", "0.1.0", sessions)
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn the_active_project_root_is_always_available_to_the_view() {
        let state = state_with(1);
        assert_eq!(state.project_root(), Path::new("/projects/glasshouse"));
        assert_eq!(state.project_name(), "glasshouse");
    }

    #[test]
    fn tab_moves_to_the_next_session_and_wraps() {
        let mut state = state_with(3);
        assert_eq!(state.selected_index(), 0);
        assert_eq!(state.handle_key(press(KeyCode::Tab)), Action::Redraw);
        assert_eq!(state.selected_index(), 1);
        state.handle_key(press(KeyCode::Tab));
        assert_eq!(state.selected_index(), 2);
        state.handle_key(press(KeyCode::Tab));
        assert_eq!(
            state.selected_index(),
            0,
            "the last entry wraps to the first"
        );
    }

    #[test]
    fn shift_tab_moves_to_the_previous_session_and_wraps() {
        let mut state = state_with(3);
        assert_eq!(state.handle_key(press(KeyCode::BackTab)), Action::Redraw);
        assert_eq!(
            state.selected_index(),
            2,
            "the first entry wraps to the last"
        );
        state.handle_key(press(KeyCode::BackTab));
        assert_eq!(state.selected_index(), 1);
    }

    #[test]
    fn arrow_keys_navigate_the_same_way_as_tab() {
        let mut state = state_with(3);
        state.handle_key(press(KeyCode::Right));
        assert_eq!(state.selected_index(), 1);
        state.handle_key(press(KeyCode::Left));
        assert_eq!(state.selected_index(), 0);
    }

    /// Navigation presents a different session; it must never look like
    /// anything happened to the sessions themselves.
    #[test]
    fn navigating_changes_only_which_session_is_presented() {
        let mut state = state_with(3);
        let before: Vec<_> = state.sessions().to_vec();

        state.handle_key(press(KeyCode::Tab));
        state.handle_key(press(KeyCode::Tab));
        state.handle_key(press(KeyCode::BackTab));

        assert_eq!(
            state.sessions(),
            before.as_slice(),
            "no session was altered"
        );
        assert_eq!(state.selected_index(), 1);
    }

    /// With nothing to move between, the key must explain itself rather than
    /// appear broken — but it must not move the selection.
    #[test]
    fn navigating_with_fewer_than_two_sessions_explains_itself() {
        for count in [0, 1] {
            let mut state = state_with(count);
            assert_eq!(state.handle_key(press(KeyCode::Tab)), Action::Redraw);
            assert_eq!(state.selected_index(), 0, "{count} sessions: nothing moved");
            let status = state.status().unwrap_or_else(|| {
                panic!("{count} sessions: the key must leave a note explaining itself")
            });
            assert!(!status.is_empty());
        }
        assert!(
            state_with(0).sessions().is_empty(),
            "the empty case must not invent a session"
        );
    }

    /// A note explains the key just pressed, so the next key clears it instead
    /// of leaving stale text sitting under a new action.
    #[test]
    fn a_status_note_is_cleared_by_the_next_keystroke() {
        let mut state = state_with(1);
        state.handle_key(press(KeyCode::Tab));
        assert!(state.status().is_some());

        assert_eq!(
            state.handle_key(press(KeyCode::Char('z'))),
            Action::Redraw,
            "clearing a note is a visible change"
        );
        assert!(state.status().is_none(), "the note must not linger");

        assert_eq!(
            state.handle_key(press(KeyCode::Char('z'))),
            Action::None,
            "with no note to clear, an unknown key costs nothing"
        );
    }

    #[test]
    fn an_empty_project_has_no_active_session_and_does_not_panic() {
        let mut state = state_with(0);
        assert!(state.active_session().is_none());
        state.handle_key(press(KeyCode::Tab));
        state.handle_key(press(KeyCode::BackTab));
        assert!(state.active_session().is_none());
    }

    #[test]
    fn o_opens_the_session_overview_from_the_keyboard() {
        let mut state = state_with(2);
        assert_eq!(state.overlay(), None);
        assert_eq!(state.handle_key(press(KeyCode::Char('o'))), Action::Redraw);
        assert_eq!(state.overlay(), Some(Overlay::Overview));
        assert_eq!(
            state.handle_key(press(KeyCode::Char('o'))),
            Action::Redraw,
            "the same key closes it again"
        );
        assert_eq!(state.overlay(), None);
    }

    /// The capability that separates an overlay from a modal takeover: leaving
    /// it returns to the session that was already active, and the session is
    /// untouched.
    #[test]
    fn leaving_an_overlay_returns_to_the_active_session_without_ending_it() {
        let mut state = state_with(3);
        state.handle_key(press(KeyCode::Tab));
        let active = state.active_session().expect("a session").clone();

        state.handle_key(press(KeyCode::Char('o')));
        assert_eq!(state.overlay(), Some(Overlay::Overview));
        assert_eq!(
            state.active_session(),
            Some(&active),
            "opening an overlay must not change which session is active"
        );

        assert_eq!(state.handle_key(press(KeyCode::Esc)), Action::Redraw);
        assert_eq!(state.overlay(), None, "Escape leaves the overlay");
        assert_eq!(
            state.active_session(),
            Some(&active),
            "the same session is still presented, unchanged"
        );
        assert_eq!(state.sessions().len(), 3, "no session was closed");
    }

    /// Escape has to mean two things, and which one depends on whether an
    /// overlay is open. Getting this backwards would make Escape close
    /// Glasshouse from inside the overview.
    #[test]
    fn escape_leaves_an_overlay_first_and_only_then_leaves_glasshouse() {
        let mut state = state_with(2);
        state.handle_key(press(KeyCode::Char('o')));
        assert_eq!(state.handle_key(press(KeyCode::Esc)), Action::Redraw);
        assert_eq!(state.handle_key(press(KeyCode::Esc)), Action::Quit);
    }

    #[test]
    fn quit_keys_are_q_escape_and_ctrl_c() {
        for key in [
            press(KeyCode::Char('q')),
            press(KeyCode::Esc),
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        ] {
            let mut state = state_with(2);
            assert_eq!(state.handle_key(key), Action::Quit, "{key:?} should quit");
        }
    }

    #[test]
    fn an_unrecognized_key_costs_nothing() {
        let mut state = state_with(2);
        assert_eq!(state.handle_key(press(KeyCode::Char('z'))), Action::None);
        assert_eq!(state.handle_key(press(KeyCode::Enter)), Action::None);
    }

    /// A refresh reorders the list, because sessions sort by last activity.
    /// Following the identifier rather than the index is what stops the user
    /// being moved to a different session by a background update.
    #[test]
    fn a_refresh_keeps_the_same_session_presented_even_when_the_order_changes() {
        let mut state = ShellState::new(
            "p",
            "/p",
            "0.1.0",
            vec![record("a", "claude-code"), record("b", "codex")],
        );
        state.handle_key(press(KeyCode::Tab));
        assert_eq!(state.active_session().unwrap().id.as_str(), "b");

        // "b" is now first, as a fresh `list()` ordered by activity would give.
        state.refresh(vec![record("b", "codex"), record("a", "claude-code")]);
        assert_eq!(
            state.active_session().unwrap().id.as_str(),
            "b",
            "the presented session must follow its identifier, not its position"
        );
        assert_eq!(state.selected_index(), 0);
    }

    #[test]
    fn a_refresh_that_removes_the_active_session_falls_back_to_the_first() {
        let mut state = ShellState::new(
            "p",
            "/p",
            "0.1.0",
            vec![record("a", "claude-code"), record("b", "codex")],
        );
        state.handle_key(press(KeyCode::Tab));
        state.refresh(vec![record("a", "claude-code")]);
        assert_eq!(state.active_session().unwrap().id.as_str(), "a");
    }

    /// Redrawing on every poll would make the interface flicker and burn CPU
    /// on an idle project, so an unchanged list must report no change.
    #[test]
    fn a_refresh_with_identical_sessions_does_not_ask_for_a_redraw() {
        let mut state = state_with(2);
        let same: Vec<_> = state.sessions().to_vec();
        assert_eq!(state.refresh(same), Action::None);
    }
}
