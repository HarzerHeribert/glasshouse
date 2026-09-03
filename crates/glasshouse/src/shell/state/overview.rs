use super::*;

/// The session overview's own data: where its cursor is, and the line being
/// typed at a session, if one is.
///
/// **The cursor is deliberately not the session bar's selection.** The bar's
/// selection is what the viewport presents and what the runtime focuses (see
/// `shell::sync_focus`); the overview's cursor is what the overview *acts
/// on*. Sharing one index would make "send this to a session I am not
/// looking at" impossible to express, which is precisely the capability the
/// overview exists to provide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverviewState {
    /// Index into [`ShellState::sessions`]. Reconciled by identity on every
    /// refresh, exactly like the bar's selection — sessions are ordered by
    /// last activity, so any refresh can reorder them and a held index would
    /// silently move the cursor onto a different session.
    pub(super) cursor: usize,
    /// The line being typed at the session under the cursor, or `None` when
    /// no field is open. `Some("")` is an open, empty field — a different
    /// state from no field at all, which is why this is not a bare `String`.
    entry: Option<String>,
}

impl OverviewState {
    /// Which row the cursor is on.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The line being typed, or `None` when no field is open.
    pub fn entry(&self) -> Option<&str> {
        self.entry.as_deref()
    }
}

/// The project overview's own data: memory the run loop already read from
/// disk, formatted into one line per entry.
///
/// Sessions are not duplicated here — [`ShellState::sessions`] already holds
/// every session record, and the view groups them by role and lifecycle at
/// render time, the same way `render_overview` derives its columns from
/// [`SessionRecord`] rather than from a copy. Memory is different: reading
/// it is file I/O this module deliberately does not hold, exactly like
/// [`ShellState::open_settings`]'s rows, so the run loop reads it and hands
/// back plain strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectOverviewState {
    /// Current binding memory — decisions and constraints, most recently
    /// updated first. See [`crate::memory::store::MemoryStore::binding`].
    decisions: Vec<String>,
    /// Current, unresolved [`crate::memory::store::MemoryKind::Todo`]
    /// entries, most recently updated first.
    todos: Vec<String>,
    /// How many further open todos exist beyond `todos` — Phase 26's
    /// snapshot budget, not a number invented here.
    todos_omitted: usize,
    /// One already-formatted line per configured resource — capability map
    /// lines 1657, 1658, 1659, 1660 and 1663. Pre-formatted the same way
    /// `decisions` and `todos` are: reading `crate::config` and the on-disk
    /// gateway-quota cache is file I/O this module deliberately does not
    /// hold, so `shell::build_project_overview_capacity` builds the text and
    /// this struct only carries it. Empty means no resource is configured
    /// for this project, not that reading failed silently — see
    /// [`Self::memory_note`] for the one honest-failure case this section
    /// shares with the memory sections.
    resources: Vec<String>,
    /// Capability map line 1661: one already-formatted line naming the
    /// currently selected routing model and its recent latency —
    /// pre-formatted the same way `resources` is: reading `crate::config`
    /// and the routing evidence ledger is file I/O this module deliberately
    /// does not hold, so `shell::build_project_overview_routing` builds the
    /// text and this struct only carries it. Always present, unlike
    /// `resources`, which can legitimately be empty — this line always has
    /// something honest to say, even when that is "not applicable" or
    /// "unknown".
    routing: String,
    /// Set when the run loop could not read project memory at all — a
    /// missing or unreadable database, say. The overlay still opens and
    /// still shows sessions; only the memory sections are empty, and this
    /// explains why rather than leaving them silently blank.
    memory_note: Option<String>,
}

impl ProjectOverviewState {
    pub fn decisions(&self) -> &[String] {
        &self.decisions
    }

    pub fn todos(&self) -> &[String] {
        &self.todos
    }

    pub fn todos_omitted(&self) -> usize {
        self.todos_omitted
    }

    pub fn resources(&self) -> &[String] {
        &self.resources
    }

    pub fn routing(&self) -> &str {
        &self.routing
    }

    pub fn memory_note(&self) -> Option<&str> {
        self.memory_note.as_deref()
    }
}

impl ShellState {
    /// Open the session overview.
    ///
    /// The cursor starts on whichever session the bar is presenting, so the
    /// overview opens looking at the same place the user already was; moving
    /// it is how they reach a session they are *not* looking at.
    pub fn open_overview(&mut self) -> Action {
        if self.overlay == Some(Overlay::Overview) {
            return Action::None;
        }
        self.overlay = Some(Overlay::Overview);
        self.overview = Some(OverviewState {
            cursor: self.selected,
            entry: None,
        });
        Action::Redraw
    }

    /// The overview's own data, or `None` when it is not open.
    pub fn overview(&self) -> Option<&OverviewState> {
        self.overview.as_ref()
    }

    /// Open the project overview with memory the run loop already read from
    /// disk. Reading `crate::memory` is file I/O this module deliberately
    /// does not hold — see [`ShellState::open_settings`] for the same split.
    ///
    /// Opens even when `memory_note` is `Some`: a project whose memory
    /// database could not be read still has sessions to show, and closing
    /// the whole overlay over one failed section would hide the part that
    /// worked. See `shell::build_project_overview_memory`'s doc comment for
    /// why the two failure paths both reach this.
    pub fn open_project_overview(
        &mut self,
        decisions: Vec<String>,
        todos: Vec<String>,
        todos_omitted: usize,
        resources: Vec<String>,
        routing: String,
        memory_note: Option<String>,
    ) -> Action {
        self.overlay = Some(Overlay::ProjectOverview);
        self.project_overview = Some(ProjectOverviewState {
            decisions,
            todos,
            todos_omitted,
            resources,
            routing,
            memory_note,
        });
        Action::Redraw
    }

    /// The project overview's own data, or `None` when it is not open.
    pub fn project_overview(&self) -> Option<&ProjectOverviewState> {
        self.project_overview.as_ref()
    }
}

impl ShellState {
    /// Open the presented session's recent-lifecycle-events overlay — map
    /// line 1758.
    ///
    /// Unlike [`Self::open_project_overview`], nothing here needs the run
    /// loop's file I/O: the events this overlay shows are the same
    /// `activity` buffer [`Self::note_events`] already keeps up to date in
    /// production, whether or not this overlay is ever opened, so there is
    /// no data to hand in — only the marker, exactly like
    /// [`Self::open_overview`] before Phase 11 gave it a cursor to track.
    pub fn open_session_events(&mut self) -> Action {
        if self.overlay == Some(Overlay::SessionEvents) {
            return Action::None;
        }
        self.overlay = Some(Overlay::SessionEvents);
        Action::Redraw
    }

    /// The session the overview's cursor is on — the one an interrupt or a
    /// sent line acts on. `None` when the overview is closed or the project
    /// has no sessions.
    pub fn overview_target(&self) -> Option<&SessionRecord> {
        self.sessions.get(self.overview.as_ref()?.cursor)
    }
}

impl ShellState {
    /// Answer one key while the session overview is open.
    ///
    /// Unlike Settings, which owns every key, the Overview claims only the
    /// keys it has a meaning for and passes the rest down: the popup is drawn
    /// over a live shell, and Tab still moving between sessions underneath it
    /// is a property worth keeping.
    pub(super) fn handle_overview_key(&mut self, key: KeyEvent, had_status: bool) -> Action {
        // While a line is being typed every key belongs to it — the letters
        // of a message must not also fire Glasshouse bindings, or typing
        // "not now" would quit.
        if self.overview.as_ref().is_some_and(|o| o.entry.is_some()) {
            return self.handle_overview_entry_key(key);
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc | KeyCode::Char('o') => self.close_overlay(),
            KeyCode::Up => self.move_overview_cursor(-1),
            KeyCode::Down => self.move_overview_cursor(1),
            // Not `Ctrl-C`, which still quits: this is the overview's own
            // key for "interrupt the session on this row", and it acts on the
            // cursor, never on the session in the viewport.
            KeyCode::Char('c') if !ctrl => self.interrupt_overview_target(),
            KeyCode::Char('m') if !ctrl => self.begin_overview_send(),
            // Phase 11 line 687: bring the cursor's session into the
            // viewport. Bound to Enter specifically because it was unclaimed
            // here — `handle_overview_entry_key` above claims Enter too, but
            // only while the send field is open, which the guard at the top
            // of this function already routes around before this match is
            // ever reached.
            KeyCode::Enter => self.focus_overview_target(),
            // Phase 11 line 688: reopen the cursor's session where it left
            // off. Not `c`/`m`'s neighbour by coincidence — `r` for resume,
            // unclaimed here and unclaimed by Settings' own `r` binding,
            // which belongs to a different overlay entirely.
            KeyCode::Char('r') if !ctrl => self.resume_overview_target(),
            _ => self.handle_control_key(key, had_status),
        }
    }

    /// Answer one key while the project overview is open.
    ///
    /// Unlike the session [`Overlay::Overview`], this popup has no cursor and
    /// nothing to act on — the map's boxes ask it to *show* things, never to
    /// act on them from here — so every key but its own close key passes
    /// through to ordinary navigation underneath, exactly like the Overview
    /// does for the keys it does not claim.
    pub(super) fn handle_project_overview_key(
        &mut self,
        key: KeyEvent,
        had_status: bool,
    ) -> Action {
        match key.code {
            KeyCode::Esc | KeyCode::Char('p') => self.close_overlay(),
            _ => self.handle_control_key(key, had_status),
        }
    }

    /// Answer one key while the session-events overlay is open — the same
    /// shape as [`Self::handle_project_overview_key`], for the same reason:
    /// nothing here is acted on, only shown.
    pub(super) fn handle_session_events_key(&mut self, key: KeyEvent, had_status: bool) -> Action {
        match key.code {
            KeyCode::Esc | KeyCode::Char('e') => self.close_overlay(),
            _ => self.handle_control_key(key, had_status),
        }
    }
}

impl ShellState {
    /// Move the overview's cursor, wrapping — the same ring the session bar
    /// is, for the same reason: stopping dead at the last row reads as a
    /// broken key.
    fn move_overview_cursor(&mut self, delta: isize) -> Action {
        if self.sessions.is_empty() {
            self.set_status("this project has no sessions to move between");
            return Action::Redraw;
        }
        let len = self.sessions.len() as isize;
        if let Some(overview) = self.overview.as_mut() {
            overview.cursor = (overview.cursor as isize + delta).rem_euclid(len) as usize;
        }
        Action::Redraw
    }

    /// The session under the cursor, if something may be sent to it — and a
    /// spoken refusal naming the session and the state it is actually in if
    /// not.
    ///
    /// **Never a silent no-op.** A key that quietly does nothing is
    /// indistinguishable from a frozen screen, and a user who has just asked
    /// a background session to stop needs to know whether it was asked. The
    /// same rule the provider probes are already held to.
    ///
    /// The state is read from the session record rather than from a process,
    /// because this module holds no processes — the run loop reports the
    /// runtime's own refusal on top of this one, for the narrower case of a
    /// session whose record still says live because its exit has not been
    /// polled yet.
    fn actionable_overview_target(&mut self, verb: &str) -> Option<SessionId> {
        let target = self
            .overview_target()
            .map(|record| (record.id.clone(), record.lifecycle));
        match target {
            None => {
                self.set_status(format!("nothing to {verb}: this project has no sessions"));
                None
            }
            Some((id, lifecycle)) if !lifecycle.is_live() => {
                self.set_status(format!(
                    "cannot {verb} session `{}`: it is {lifecycle}, not running",
                    short_session_id(&id)
                ));
                None
            }
            Some((id, _)) => Some(id),
        }
    }

    /// Interrupt the session under the cursor.
    ///
    /// Nothing about the shell changes: not the presented session, not focus,
    /// not the session's recorded state. An interrupt is a byte delivered to
    /// a terminal, and what the harness does about it is the harness's
    /// business — a session that handles it is still running afterwards, and
    /// one that exits is noticed by the ordinary exit detection.
    fn interrupt_overview_target(&mut self) -> Action {
        match self.actionable_overview_target("interrupt") {
            Some(id) => Action::InterruptSession(id),
            None => Action::Redraw,
        }
    }

    /// Open the one-line field for sending text to the session under the
    /// cursor, refusing up front if that session is not running.
    fn begin_overview_send(&mut self) -> Action {
        if self.actionable_overview_target("send text to").is_none() {
            return Action::Redraw;
        }
        if let Some(overview) = self.overview.as_mut() {
            overview.entry = Some(String::new());
        }
        Action::Redraw
    }

    /// Bring the session under the cursor into the viewport and hand it the
    /// keyboard — Phase 11 line 687: "focus any live embedded session from
    /// the overview".
    ///
    /// Deliberately not built on `actionable_overview_target` alone. That
    /// helper's liveness refusal is exactly right here too — a stopped
    /// session has no process to give the keyboard to — but the box names a
    /// second adjective the helper knows nothing about: *embedded*. A
    /// headless session is live and still has no viewport to focus into, so
    /// it needs its own refusal on top, spoken rather than silent for the
    /// same reason every other overview key is.
    fn focus_overview_target(&mut self) -> Action {
        let Some(id) = self.actionable_overview_target("focus") else {
            return Action::Redraw;
        };
        // Re-read after the liveness check rather than trusting the id
        // alone: `actionable_overview_target` looked this session up by the
        // cursor's position, and re-finding it by identity here is what
        // keeps this correct if that ever changes.
        let Some(index) = self.sessions.iter().position(|record| record.id == id) else {
            return Action::Redraw;
        };
        if self.sessions[index].presentation != SessionPresentation::Embedded {
            self.set_status(format!(
                "cannot focus session `{}`: it is {}, not embedded — there is no viewport to focus into",
                short_session_id(&id),
                self.sessions[index].presentation,
            ));
            return Action::Redraw;
        }
        self.selected = index;
        self.overlay = None;
        self.overview = None;
        self.mode = Mode::Session;
        Action::Redraw
    }

    /// The session under the cursor, if it may be resumed — and a spoken
    /// refusal naming the session and its actual state if not. The resume
    /// half of Phase 11 line 688.
    ///
    /// Deliberately not `actionable_overview_target`: that helper refuses
    /// every session whose lifecycle is not live, which is backwards for
    /// resume — a live session has nothing to resume *to*, and the whole
    /// point of this key is the session that is *not* running. Gated on
    /// [`SessionRecord::disposition`] instead, so the session this key acts
    /// on is exactly the one the STATE column already labels `resumable`.
    fn resumable_overview_target(&mut self) -> Option<SessionId> {
        let target = self
            .overview_target()
            .map(|record| (record.id.clone(), record.disposition()));
        match target {
            None => {
                self.set_status("nothing to resume: this project has no sessions");
                None
            }
            Some((id, SessionDisposition::Resumable)) => Some(id),
            Some((id, SessionDisposition::Active)) => {
                self.set_status(format!(
                    "cannot resume session `{}`: it is still running",
                    short_session_id(&id)
                ));
                None
            }
            Some((id, SessionDisposition::Failed)) => {
                self.set_status(format!(
                    "cannot resume session `{}`: it failed, with no session to reopen",
                    short_session_id(&id)
                ));
                None
            }
            Some((id, SessionDisposition::Closed)) => {
                self.set_status(format!(
                    "cannot resume session `{}`: it is closed, with no native session id \
                     recorded to resume to",
                    short_session_id(&id)
                ));
                None
            }
        }
    }

    /// Resume the session under the cursor.
    fn resume_overview_target(&mut self) -> Action {
        match self.resumable_overview_target() {
            Some(id) => Action::ResumeSession(id),
            None => Action::Redraw,
        }
    }

    /// Answer one key while the send field is open.
    fn handle_overview_entry_key(&mut self, key: KeyEvent) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => {
                if let Some(overview) = self.overview.as_mut() {
                    overview.entry = None;
                }
                Action::Redraw
            }
            KeyCode::Enter => self.submit_overview_send(),
            KeyCode::Backspace => {
                if let Some(entry) = self.overview.as_mut().and_then(|o| o.entry.as_mut()) {
                    entry.pop();
                }
                Action::Redraw
            }
            KeyCode::Char(c) if !ctrl => {
                if let Some(entry) = self.overview.as_mut().and_then(|o| o.entry.as_mut()) {
                    entry.push(c);
                }
                Action::Redraw
            }
            _ => Action::None,
        }
    }

    /// Send what has been typed to the session under the cursor.
    ///
    /// The field is closed on every path out — a refused line and an empty
    /// one both leave the overview where it was, rather than trapping the
    /// user in a field that will not accept anything.
    fn submit_overview_send(&mut self) -> Action {
        let text = self
            .overview
            .as_ref()
            .and_then(|overview| overview.entry.clone())
            .unwrap_or_default();
        if let Some(overview) = self.overview.as_mut() {
            overview.entry = None;
        }
        if text.is_empty() {
            self.set_status("nothing to send: the line was empty");
            return Action::Redraw;
        }
        // Checked again here, not only when the field opened: a session can
        // end while a line is being typed at it, and sending into a dead
        // session must be refused out loud at the moment of sending.
        match self.actionable_overview_target("send text to") {
            Some(id) => Action::SendSessionText { id, text },
            None => Action::Redraw,
        }
    }

    /// Enter session mode, giving the focused session's PTY the keyboard.
    ///
    /// Refused with nothing active: there would be nowhere to send the keys,
    /// and a mode with no destination is worse than staying put — see
    /// invariant 5 in the design note. Any open overlay is closed first:
    /// session mode and an overlay must never coexist, which is what keeps
    /// "leaving an overlay returns to the mode you were in" simple, since
    /// that mode is by construction always control.
    ///
    /// Refused for a **headless** session for a sharper version of the same
    /// reason. A headless session has no viewport, so the runtime refuses to
    /// focus it (`RuntimeError::Headless`) and keystrokes would go to
    /// whichever session held focus before — the user would be typing into a
    /// session the bar is not showing. Saying no is the only honest answer.
    pub(super) fn enter_session_mode(&mut self) -> Action {
        let Some(record) = self.active_session() else {
            self.set_status("no session to enter — start one with `n`");
            return Action::Redraw;
        };
        if record.presentation == SessionPresentation::Headless {
            let id = record.id.clone();
            // Short on purpose: a status note shares its row with the key
            // bindings, which are written first, so a long refusal is a
            // clipped one. The viewport itself carries the full explanation
            // on every frame — see `view::render_viewport`.
            self.set_status(format!(
                "`{}` is headless — no viewport to enter",
                short_session_id(&id)
            ));
            return Action::Redraw;
        }
        self.overlay = None;
        self.mode = Mode::Session;
        Action::Redraw
    }

    /// Answer one key while a session owns the keyboard.
    ///
    /// Everything is forwarded to the focused PTY untouched — including `q`,
    /// Tab, and Ctrl-C — except the one reserved escape chord, which is
    /// intercepted here and never forwarded.
    pub(super) fn handle_session_key(&mut self, key: KeyEvent) -> Action {
        if is_session_escape(&key) {
            self.mode = Mode::Control;
            return Action::Redraw;
        }
        match encode(key) {
            Some(bytes) => Action::Forward(bytes),
            None => Action::None,
        }
    }

    /// Called when the session currently presented has exited.
    ///
    /// Session mode with nowhere left to send keystrokes would leave every
    /// keypress going nowhere with no visible way out, so an exit always
    /// drops back to control mode — see invariant 6 in the design note.
    pub fn session_exited(&mut self) -> Action {
        if self.mode == Mode::Session {
            self.mode = Mode::Control;
            Action::Redraw
        } else {
            Action::None
        }
    }
}

/// `Ctrl-]` — the one chord that returns to control mode from session mode.
///
/// See the design note for why this chord: it is what `telnet` has used for
/// decades, no ordinary key produces it, and it is exactly one chord rather
/// than a prefix that would double the latency of escaping a runaway session.
///
/// **It has more than one spelling, and all of them must be accepted.** The
/// chord is really the byte `0x1D` (ASCII group separator), and Crossterm's
/// Unix parser decodes the control range `0x1C..=0x1F` arithmetically, as
/// `Char((c - 0x1C + b'4'))` — so a real terminal's `Ctrl-]` arrives as
/// `Ctrl` + `'5'`, never as `Ctrl` + `']'`.
///
/// Matching too narrowly is not a cosmetic bug: it leaves the user in session
/// mode with no way back, which is precisely the failure the single-chord
/// escape exists to prevent. It survived unit testing because a synthetic
/// `KeyEvent::new(KeyCode::Char(']'), CONTROL)` is not what any terminal
/// sends; only driving the real binary through a real pseudo-terminal caught
/// it — twice, once per platform.
///
/// # Windows delivers a character chosen by the keyboard layout
///
/// The `']'` spelling was written for Windows, where Crossterm reads console
/// records rather than a byte stream. That is right only by accident of
/// whoever wrote it having a US keyboard. Measured on the ARM64 CI machine,
/// whose input locale is `0x04070409` — US English on a **German** physical
/// layout — the byte `0x1D` arrives from ConPTY as this console record:
///
/// ```text
/// vk=0xBB (VK_OEM_PLUS)  scan=0x1B  uChar=0x001D  ctrl=LEFT_CTRL_PRESSED
/// ```
///
/// `uChar` is the right answer and Crossterm throws it away: its Windows
/// parser treats any `uChar` in `0x00..=0x1f` as "some chord produced a
/// control code, ask the layout which character that key really types" and
/// calls `ToUnicodeEx` on the *virtual key*. Scan code `0x1B` is the physical
/// key right of `P`; a US layout types `']'` there and a German one types
/// `'+'`. So Glasshouse is handed `Ctrl` + `'+'` on this machine and
/// `Ctrl` + `']'` on the machine the original spelling came from, for the
/// same keypress.
///
/// **No fixed set of characters can be correct**, because the set is the set
/// of layouts. So on Windows the test is the modifier and the *shape* of the
/// character rather than its identity: any non-alphanumeric character with
/// Control. Its failure mode is a spurious escape — a user typing some other
/// `Ctrl`+punctuation chord lands back in control mode — which is the
/// direction to fail in, because the other direction traps them with no way
/// out. Nothing is lost by it either: [`encode`] has never translated
/// `Ctrl`+punctuation to a control byte, so those chords reached the harness
/// as bare punctuation, which is not what the user pressed.
///
/// **`AltGr` is excluded, and that is measured too.** Windows reports `AltGr`
/// as `CONTROL | ALT`, so on a layout where `']'` itself needs `AltGr` — as
/// it does on the German one — typing a literal `']'` arrived as
/// `Ctrl` + `Alt` + `']'` and matched the old rule. Typing a bracket into a
/// harness would have thrown the user out of the session. The real chord
/// never carries `ALT`: the record above has `LEFT_CTRL_PRESSED` alone.
///
/// The durable fix is upstream of here, in whatever reads the console
/// records: `uChar` already carries `0x1D` exactly. See the report for the
/// shape that would take.
pub(super) fn is_session_escape(key: &KeyEvent) -> bool {
    if !key.modifiers.contains(KeyModifiers::CONTROL) {
        return false;
    }
    // See the doc comment: `AltGr` is `CONTROL | ALT` on Windows, and the
    // chord itself never carries `ALT`, so this only ever excludes a
    // character the user meant to type.
    #[cfg(windows)]
    if key.modifiers.contains(KeyModifiers::ALT) {
        return false;
    }
    #[cfg(windows)]
    if matches!(key.code, KeyCode::Char(c) if !c.is_ascii_alphanumeric()) {
        return true;
    }
    matches!(key.code, KeyCode::Char(']') | KeyCode::Char('5'))
}

/// Turn one key event into the bytes a PTY expects.
///
/// `None` for a key with no sensible byte encoding (a bare modifier, a
/// function key Glasshouse does not translate) — session mode simply has
/// nothing to send for it.
pub(super) fn encode(key: KeyEvent) -> Option<Vec<u8>> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        // Ctrl-A..Ctrl-Z as the control byte 0x01..0x1a. Checked before the
        // plain `Char` arm below so a control chord never encodes as its
        // literal character instead.
        KeyCode::Char(c) if ctrl && c.is_ascii_alphabetic() => {
            Some(vec![c.to_ascii_lowercase() as u8 - b'a' + 1])
        }
        KeyCode::Char(c) => {
            let mut buf = [0u8; 4];
            Some(c.encode_utf8(&mut buf).as_bytes().to_vec())
        }
        KeyCode::Enter => Some(vec![b'\r']),
        KeyCode::Backspace => Some(vec![0x7f]),
        KeyCode::Tab => Some(vec![b'\t']),
        KeyCode::Esc => Some(vec![0x1b]),
        KeyCode::Up => Some(b"\x1b[A".to_vec()),
        KeyCode::Down => Some(b"\x1b[B".to_vec()),
        KeyCode::Right => Some(b"\x1b[C".to_vec()),
        KeyCode::Left => Some(b"\x1b[D".to_vec()),
        _ => None,
    }
}
