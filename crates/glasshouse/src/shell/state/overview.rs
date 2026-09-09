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

    /// `id`, if a session in this `disposition` may be resumed — and a spoken
    /// refusal naming the session and its actual state if not. The resume
    /// half of Phase 11 line 688.
    ///
    /// Deliberately not `actionable_overview_target`: that helper refuses
    /// every session whose lifecycle is not live, which is backwards for
    /// resume — a live session has nothing to resume *to*, and the whole
    /// point of this key is the session that is *not* running. Gated on
    /// [`SessionRecord::disposition`] instead, so the session this key acts
    /// on is exactly the one the STATE column already labels `resumable`.
    ///
    /// Takes the disposition rather than reading it, so the overview's `R`
    /// and the session bar's `Enter` refuse in exactly the same words — one
    /// sentence per state, in one place. **Every refusal here names why it is
    /// impossible rather than only that it is**: `Closed` is the "no native
    /// session id captured" case, and it is the only shape of stopped session
    /// this still turns away. The other impossible case — a harness with no
    /// verified resume mechanism, `pane` today — is not knowable from a
    /// record, and is refused by name where the adapter is in hand
    /// (`shell::resume_session`).
    fn resume_target(
        &mut self,
        id: SessionId,
        disposition: SessionDisposition,
    ) -> Option<SessionId> {
        match disposition {
            SessionDisposition::Resumable => Some(id),
            SessionDisposition::Active => {
                self.set_status(format!(
                    "cannot resume `{}`: it is still running",
                    short_session_id(&id)
                ));
                None
            }
            SessionDisposition::Failed => {
                self.set_status(format!(
                    "cannot resume `{}`: it failed with no session to reopen",
                    short_session_id(&id)
                ));
                None
            }
            SessionDisposition::Closed => {
                self.set_status(format!(
                    "cannot resume `{}`: no native session id was recorded",
                    short_session_id(&id)
                ));
                None
            }
        }
    }

    /// The session under the overview's cursor, if it may be resumed.
    fn resumable_overview_target(&mut self) -> Option<SessionId> {
        let target = self
            .overview_target()
            .map(|record| (record.id.clone(), record.disposition()));
        match target {
            None => {
                self.set_status("nothing to resume: this project has no sessions");
                None
            }
            Some((id, disposition)) => self.resume_target(id, disposition),
        }
    }

    /// Resume the session under the cursor, leaving the overview open around
    /// it — the user is reading a list, not asking to be inside one row of it.
    ///
    /// `resume_entry` is deliberately left unset, which is the whole
    /// difference between this and `Enter`: see [`Action::ResumeSession`].
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
    ///
    /// **A session whose process is gone is resumed, not refused** — user
    /// ruling 2026-09-09: *"stopped session dont reopen on entering them while
    /// that would be easily doable by codex --resume id, claude --resume id …
    /// so just not entering because you exited kinda dumb"*. Entering used to
    /// be turned away here, on the true observation that a stopped session has
    /// no pseudo-terminal left and a keystroke sent to one vanishes. The
    /// observation stands and the conclusion did not: the answer is to give
    /// the session a process again, through the same
    /// [`Action::ResumeSession`] the overview's `R` produces and the same
    /// `shell::resume_session` that answers it — which is `Enter`'s whole
    /// job, since a user pressing it is asking to be *in* that session.
    /// [`Self::resume_target`] is where the two paths' refusals live, so what
    /// is genuinely unresumable is turned away in one voice.
    pub(super) fn enter_session_mode(&mut self) -> Action {
        let Some(record) = self.active_session() else {
            self.set_status("no session to enter — start one with `n`");
            return Action::Redraw;
        };
        // Read out before the refusals rather than in them: `set_status`
        // borrows `self` mutably and `record` is borrowed from `self`, so the
        // record cannot outlive the first note.
        let id = record.id.clone();
        let presentation = record.presentation;
        // `disposition` and not `lifecycle.is_live()`: they classify the same
        // set — `Active` is exactly the live states — and asking one question
        // means the refusal below cannot disagree with the resume above it.
        let disposition = record.disposition();
        if presentation == SessionPresentation::Headless {
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
        if disposition != SessionDisposition::Active {
            return match self.resume_target(id, disposition) {
                Some(id) => {
                    // The note the run loop replaces the moment the harness is
                    // up. Set here rather than there because starting a
                    // harness is not instant and this is the only frame drawn
                    // in between — without it `Enter` on a stopped session
                    // looks like a key that did nothing.
                    self.set_status(format!("resuming `{}` …", short_session_id(&id)));
                    // What makes this `Enter`'s resume and not the overview's:
                    // the request to be put *inside* what is being reopened.
                    self.resume_entry = Some(id.clone());
                    Action::ResumeSession(id)
                }
                None => Action::Redraw,
            };
        }
        self.overlay = None;
        self.session_view = true;
        self.mode = Mode::Session;
        Action::Redraw
    }

    /// The layout a resume of `id` ends in, asked before the harness starts.
    ///
    /// The run loop has to size a pseudo-terminal for a frame that has not
    /// been drawn yet, and the answer depends on the request the key recorded:
    /// a resume the user asked to be put inside ends in [`Chrome::Header`],
    /// and one from the overview ends wherever the shell already was. Here
    /// rather than as a branch in the run loop, for the reason
    /// [`Action::ResumeSession`] gives.
    pub fn chrome_after_resume(&self, id: &SessionId) -> Chrome {
        if self.resume_entry.as_ref() == Some(id) {
            Chrome::Header
        } else {
            self.chrome()
        }
    }

    /// The run loop's report that a resumed session's harness is running.
    ///
    /// **Answers the request the key made, rather than a decision the run loop
    /// takes.** `Enter` on a stopped session is a request to be *in* it, and
    /// the process only exists once the run loop has started it, so the focus
    /// cannot be taken at the moment the key is answered — it is recorded in
    /// `resume_entry` and honoured here. The overview's `R` records nothing,
    /// so the same call leaves the user in the list they were reading.
    /// Compared by identifier, so a report about one session can never focus
    /// another.
    ///
    /// Selects before entering — `refresh` reconciles onto whatever was
    /// presented before the key, exactly as it does after `StartSession`, so
    /// without this the resumed session could be focused while the bar
    /// presents another.
    pub fn session_resumed(&mut self, id: &SessionId) -> Action {
        if self.resume_entry.take().as_ref() != Some(id) {
            return Action::Redraw;
        }
        self.select_session(id);
        self.enter_session_mode()
    }

    /// Put a newly started embedded session under the keyboard immediately.
    ///
    /// Called only after the process exists. Selecting by identifier first
    /// keeps a failed record refresh from entering whichever older session
    /// happened to remain selected.
    pub fn session_started(&mut self, id: &SessionId) -> Action {
        if !self.select_session(id) {
            return Action::Redraw;
        }
        self.enter_session_mode()
    }

    /// Answer one key while a session owns the keyboard.
    ///
    /// Everything is forwarded to the focused PTY untouched — including `q`,
    /// Tab, and Ctrl-C — except the two reserved chords, which are intercepted
    /// here and never forwarded.
    ///
    /// **They are different acts and both are kept.** The escape chord leaves
    /// the session: the fleet view comes back with all five of its bands, and
    /// that is what it has always done. [`FOCUS_CHORD`] leaves the session's
    /// *keyboard* and nothing else — the header and the viewport stay exactly
    /// where they are and Glasshouse's own bindings answer instead, which is
    /// the *"key to change focus"* of the 2026-09-09 ruling. Neither reaches
    /// the harness: a chord Glasshouse answers is a key the harness never
    /// sees, which is the half of the contract
    /// `state_tests::the_focus_chord_moves_the_keyboard_and_nothing_else`
    /// asserts in both directions.
    pub(super) fn handle_session_key(&mut self, key: KeyEvent) -> Action {
        if is_session_escape(&key) {
            self.mode = Mode::Control;
            self.session_view = false;
            return Action::Redraw;
        }
        if is_focus_chord(&key) {
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
        if self.mode == Mode::Session || self.session_view {
            self.mode = Mode::Control;
            // The layout goes with the keyboard. A header focused over a
            // viewport whose process has ended would leave the user reading a
            // frozen screen with the fleet view — and the `n` that starts
            // another session — off screen.
            self.session_view = false;
            Action::Redraw
        } else {
            Action::None
        }
    }
}

/// `Ctrl-]` or `F12` — the chords that return to control mode from session
/// mode, both of which must always be accepted.
///
/// `Ctrl-]` is what `telnet` has used for decades, and no ordinary key
/// produces it. It has more than one spelling, and all of them must be
/// accepted: the chord is really the byte `0x1D`, and Crossterm's Unix parser
/// decodes the control range `0x1C..=0x1F` arithmetically, so a real
/// terminal's `Ctrl-]` arrives as `Ctrl` + `'5'`, never as `Ctrl` + `']'` —
/// matching too narrowly traps the user in session mode with no way back, and
/// only a real pseudo-terminal caught it (twice, once per platform), since a
/// synthetic `KeyEvent` is not what any terminal sends.
///
/// On Windows, Crossterm asks the keyboard layout which character a
/// control code's virtual key really types, so the same physical keypress
/// decodes differently per layout even though the raw `uChar` already
/// carries `0x1D` exactly (Crossterm discards it). No fixed character set
/// can be correct across layouts, so on Windows the test is the modifier
/// and the *shape* — any non-alphanumeric character with Control, excluding
/// `AltGr` (`CONTROL | ALT`, never carried by the real chord) — with a
/// spurious escape as the acceptable failure direction. History:
/// design-decisions.md, "Trims: the remaining module docs, second packet",
/// `is_session_escape`.
///
/// `F12` exists because every spelling above is the single byte `0x1D`, and a
/// keyboard that cannot type `]` without a modifier cannot produce it: on a
/// German Mac layout `]` is `Right-Option-6`, and `Ctrl` with that is not the
/// chord. `F12` is layout-independent, and no harness Glasshouse embeds binds
/// it. Accepted with any modifiers or none, since nothing else claims it.
/// Deliberately **not** `Ctrl-Q`: that is XON, real harnesses bind it, and
/// design-decisions.md ("Session mode") chose `Ctrl-]` precisely so the escape
/// steals no harness key.
pub(super) fn is_session_escape(key: &KeyEvent) -> bool {
    if key.code == KeyCode::F(12) {
        return true;
    }
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

/// [`FOCUS_CHORD`] — `Ctrl-6`, the chord that moves the keyboard between
/// Glasshouse's own header and the session drawn under it.
///
/// Both spellings of the same byte, for the reason
/// [`is_session_escape`] states at length: the chord is `0x1E`, and
/// Crossterm's Unix parser decodes `0x1C..=0x1F` arithmetically, so a real
/// terminal delivers `Ctrl` + `'6'`. A terminal that names the shifted
/// character instead delivers `Ctrl` + `'^'`, and both are accepted so that
/// matching too narrowly cannot cost the user the key.
///
/// `'6'` is a digit, which is the whole point: `FOCUS_CHORD` documents why a
/// `Ctrl`+letter chord was not available, and every Latin layout — the German
/// Mac one this project is developed on included — puts the digits where a US
/// layout does. On Windows [`is_session_escape`]'s shape test claims every
/// non-alphanumeric character with `Control`, so `Ctrl-^` escapes there
/// rather than moving focus; `Ctrl-6` is alphanumeric and reaches this
/// function on every platform, which is why it is the chord that is
/// advertised.
pub(super) fn is_focus_chord(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('6') | KeyCode::Char('^'))
}

/// Turn one key event into the bytes a PTY expects.
///
/// `None` only for a key no terminal has bytes for at all (a bare modifier,
/// `CapsLock`, a media key) — anything a harness could bind must arrive, and a
/// key silently dropped here is indistinguishable from a harness that ignored
/// it. Every sequence below is the one an ordinary terminal emulator sends, so
/// the harness's own input parser needs no special case for Glasshouse.
pub(super) fn encode(key: KeyEvent) -> Option<Vec<u8>> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char(c) => {
            // A control chord is its control byte, never its literal
            // character: without this `Ctrl-\` would reach the harness as the
            // `'4'` Crossterm named it after.
            let control = if ctrl { control_byte(c) } else { None };
            Some(match control {
                Some(byte) => vec![byte],
                None => {
                    let mut buf = [0u8; 4];
                    c.encode_utf8(&mut buf).as_bytes().to_vec()
                }
            })
        }
        KeyCode::Enter => Some(vec![b'\r']),
        KeyCode::Backspace => Some(vec![0x7f]),
        KeyCode::Tab => Some(vec![b'\t']),
        KeyCode::BackTab => Some(b"\x1b[Z".to_vec()),
        KeyCode::Esc => Some(vec![0x1b]),
        KeyCode::Up => Some(b"\x1b[A".to_vec()),
        KeyCode::Down => Some(b"\x1b[B".to_vec()),
        KeyCode::Right => Some(b"\x1b[C".to_vec()),
        KeyCode::Left => Some(b"\x1b[D".to_vec()),
        KeyCode::Home => Some(b"\x1b[H".to_vec()),
        KeyCode::End => Some(b"\x1b[F".to_vec()),
        KeyCode::Insert => Some(b"\x1b[2~".to_vec()),
        KeyCode::Delete => Some(b"\x1b[3~".to_vec()),
        KeyCode::PageUp => Some(b"\x1b[5~".to_vec()),
        KeyCode::PageDown => Some(b"\x1b[6~".to_vec()),
        KeyCode::F(n) => function_key(n).map(<[u8]>::to_vec),
        _ => None,
    }
}

/// The control byte `Ctrl` with `c` sends, or `None` when the chord has no
/// control byte and the character itself is what the harness should receive.
///
/// Two families. `Ctrl-A..Ctrl-Z` are `0x01..=0x1a`, derived from the letter.
/// The C0 codes with no letter to derive them from are spelled by whichever
/// character a parser named them after: Crossterm's Unix parser maps
/// `0x1C..=0x1F` to `'4'..='7'` arithmetically, while a keyboard that can type
/// `\` or `_` reports those literally — both spellings mean the same byte, so
/// both are listed. `'5'` and `']'` (`0x1D`) are deliberately missing:
/// [`is_session_escape`] claims that byte in
/// [`ShellState::handle_session_key`] before this is ever called.
fn control_byte(c: char) -> Option<u8> {
    match c {
        c if c.is_ascii_alphabetic() => Some(c.to_ascii_lowercase() as u8 - b'a' + 1),
        ' ' => Some(0x00),
        '4' | '\\' => Some(0x1c),
        '6' => Some(0x1e),
        '7' | '_' => Some(0x1f),
        _ => None,
    }
}

/// The escape sequence a function key sends, or `None` for the one Glasshouse
/// keeps for itself.
///
/// `F1..=F4` are the VT100 `SS3` forms and `F5..=F11` the xterm `CSI ~` forms,
/// which is what a harness's own input parser reads a function-key binding
/// from — before this existed every one of them was dropped. `F12` is
/// absent on purpose: it is the layout-independent escape, and
/// [`ShellState::handle_session_key`] tests [`is_session_escape`] first, so it
/// cannot reach here. `F13` and up have no agreed encoding and are dropped.
fn function_key(n: u8) -> Option<&'static [u8]> {
    Some(match n {
        1 => b"\x1bOP",
        2 => b"\x1bOQ",
        3 => b"\x1bOR",
        4 => b"\x1bOS",
        5 => b"\x1b[15~",
        6 => b"\x1b[17~",
        7 => b"\x1b[18~",
        8 => b"\x1b[19~",
        9 => b"\x1b[20~",
        10 => b"\x1b[21~",
        11 => b"\x1b[23~",
        _ => return None,
    })
}
