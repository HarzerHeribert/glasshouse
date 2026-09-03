use super::*;

/// One [`crate::memory::MemoryKind`]'s entries for the
/// project-knowledge view: already-formatted display lines, most recently
/// updated first, plus how many further matching entries exist beyond what
/// is shown. Built by `shell::build_project_knowledge_memory` — this module
/// never queries `crate::memory` itself, the same split
/// [`ProjectOverviewState`] keeps.
///
/// `details` is index-aligned with `lines` — `details[i]` is
/// [`MemoryDetail`] for the memory `lines[i]` summarizes — map line 1105's
/// drill-down. Kept as a parallel `Vec` rather than folding the two into one
/// per-entry type so every existing reader of `lines` (and every fixture
/// that builds one by hand) stays unaffected by a field it does not use.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KnowledgeSection {
    pub lines: Vec<String>,
    pub details: Vec<MemoryDetail>,
    pub omitted: usize,
}

/// One memory's rationale, source session, source commit and lifecycle
/// state — map line 1105: *"allow the user to open a memory item and
/// inspect its rationale, source session, source commit, and lifecycle
/// state."* Built by `shell::knowledge_detail` from
/// [`crate::memory::MemoryRecord`]'s own fields — this module holds plain
/// strings rather than importing `crate::memory`, the same split
/// [`KnowledgeSection`] itself keeps.
///
/// `None` on `rationale`, `source_session` or `source_commit` means the
/// producer never recorded one — never rendered as an empty field, always
/// as an honest "none recorded" note (see `view::render_knowledge_detail`).
/// `lifecycle` is never absent: every memory has a
/// [`crate::memory::MemoryStatus`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryDetail {
    pub rationale: Option<String>,
    pub source_session: Option<String>,
    pub source_commit: Option<String>,
    pub lifecycle: String,
}

/// The project-knowledge view's own data: every kind of durable project
/// memory the run loop already read from disk, grouped by kind and
/// formatted into display lines. Decisions, constraints and features are
/// filtered to current knowledge
/// ([`crate::memory::MemoryStatus::is_current`]); todos to open work
/// ([`crate::memory::MemoryStatus::is_open_work`], which — unlike
/// `is_current` — keeps one under review or in conflict); failed approaches
/// are shown regardless of status, because the historical record of what was
/// tried is the point of that section (map line 1103). See
/// [`ShellState::open_project_knowledge`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectKnowledgeState {
    decisions: KnowledgeSection,
    constraints: KnowledgeSection,
    features: KnowledgeSection,
    failed_attempts: KnowledgeSection,
    todos: KnowledgeSection,
    /// Set when the run loop could not read project memory at all. The
    /// overlay still opens with honest, empty sections rather than refusing
    /// to show anything — the same contract
    /// [`ProjectOverviewState::memory_note`] keeps.
    memory_note: Option<String>,
    /// Index into the entries of [`Self::sections`], concatenated in the
    /// same order the view renders them (decisions, constraints, features,
    /// failed attempts, todos) — map line 1105's selection, the same cursor
    /// idiom [`OverviewState::cursor`] uses. Meaningless when there are no
    /// entries at all; every accessor guards for that rather than trusting
    /// it, the same rule [`ShellState::selected`]'s own doc comment states.
    cursor: usize,
    /// Whether the detail popup for the entry under [`Self::cursor`] is
    /// currently shown. A separate flag rather than folding into `cursor`
    /// (say, a sentinel value) because "which entry" and "am I looking at
    /// its detail" are independent facts — the cursor keeps moving to the
    /// same place if the detail view is closed and reopened.
    detail_open: bool,
}

impl ProjectKnowledgeState {
    pub fn decisions(&self) -> &KnowledgeSection {
        &self.decisions
    }

    pub fn constraints(&self) -> &KnowledgeSection {
        &self.constraints
    }

    pub fn features(&self) -> &KnowledgeSection {
        &self.features
    }

    /// Kept as history regardless of status — map line 1103's dedicated
    /// section.
    pub fn failed_attempts(&self) -> &KnowledgeSection {
        &self.failed_attempts
    }

    pub fn todos(&self) -> &KnowledgeSection {
        &self.todos
    }

    pub fn memory_note(&self) -> Option<&str> {
        self.memory_note.as_deref()
    }

    /// The five sections, in the exact order the view renders them —
    /// [`Self::cursor`] and [`Self::selected`] both walk this order, and
    /// nothing else, so the two can never disagree about which entry is
    /// "the third one".
    fn sections(&self) -> [&KnowledgeSection; 5] {
        [
            &self.decisions,
            &self.constraints,
            &self.features,
            &self.failed_attempts,
            &self.todos,
        ]
    }

    /// How many selectable entries exist across every section, combined.
    pub fn total_entries(&self) -> usize {
        self.sections().iter().map(|s| s.lines.len()).sum()
    }

    /// Which entry the cursor is on, meaningless when [`Self::total_entries`]
    /// is zero.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Whether the detail popup for the entry under the cursor is open.
    pub fn detail_open(&self) -> bool {
        self.detail_open
    }

    /// The entry under the cursor — its display line and its
    /// [`MemoryDetail`] — or `None` when nothing is selectable.
    pub fn selected(&self) -> Option<(&str, &MemoryDetail)> {
        self.sections()
            .into_iter()
            .flat_map(|section| section.lines.iter().zip(section.details.iter()))
            .map(|(line, detail)| (line.as_str(), detail))
            .nth(self.cursor)
    }
}

/// The project-memory view's own data: every [`crate::memory::MemoryKind`]'s
/// records, at every [`crate::memory::MemoryStatus`], unfiltered and
/// ungrouped into one list — map line 234. [`ProjectKnowledgeState`]'s
/// sibling with the filtering removed: this view is "what does this project
/// remember," not "what has this project learned," so nothing here is
/// dropped for being superseded, resolved, or a kind
/// [`ProjectKnowledgeState`] has no section for. See
/// [`ShellState::open_project_memory`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectMemoryState {
    memory: KnowledgeSection,
    /// Set when the run loop could not read project memory at all. The
    /// overlay still opens with an honest, empty section rather than
    /// refusing to show anything — the same contract
    /// [`ProjectKnowledgeState::memory_note`] keeps.
    memory_note: Option<String>,
    /// Index into [`Self::memory`]'s entries — the same cursor idiom
    /// [`ProjectKnowledgeState::cursor`] uses, over one section instead of
    /// five. Meaningless when there are no entries at all; every accessor
    /// guards for that rather than trusting it.
    cursor: usize,
    /// Whether the detail popup for the entry under [`Self::cursor`] is
    /// currently shown — the same independent flag
    /// [`ProjectKnowledgeState::detail_open`] is, for the same reason.
    detail_open: bool,
}

impl ProjectMemoryState {
    /// Every memory record read for this view, most recently updated first.
    pub fn memory(&self) -> &KnowledgeSection {
        &self.memory
    }

    pub fn memory_note(&self) -> Option<&str> {
        self.memory_note.as_deref()
    }

    /// How many selectable entries exist.
    pub fn total_entries(&self) -> usize {
        self.memory.lines.len()
    }

    /// Which entry the cursor is on, meaningless when [`Self::total_entries`]
    /// is zero.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Whether the detail popup for the entry under the cursor is open.
    pub fn detail_open(&self) -> bool {
        self.detail_open
    }

    /// The entry under the cursor — its display line and its
    /// [`MemoryDetail`] — or `None` when nothing is selectable. The same
    /// shape [`ProjectKnowledgeState::selected`] returns, over the one
    /// section this view has instead of five.
    pub fn selected(&self) -> Option<(&str, &MemoryDetail)> {
        self.memory
            .lines
            .iter()
            .zip(self.memory.details.iter())
            .map(|(line, detail)| (line.as_str(), detail))
            .nth(self.cursor)
    }
}

impl ShellState {
    /// Open the project-knowledge view with memory the run loop already read
    /// from disk, grouped by kind. Reading `crate::memory` is file I/O this
    /// module deliberately does not hold — see [`Self::open_project_overview`]
    /// for the same split.
    ///
    /// Opens even when `memory_note` is `Some`: a project whose memory
    /// database could not be read still gets an honest, empty view rather
    /// than no view at all — see `shell::build_project_knowledge_memory`'s
    /// doc comment for why both failure paths reach this.
    pub fn open_project_knowledge(
        &mut self,
        decisions: KnowledgeSection,
        constraints: KnowledgeSection,
        features: KnowledgeSection,
        failed_attempts: KnowledgeSection,
        todos: KnowledgeSection,
        memory_note: Option<String>,
    ) -> Action {
        self.overlay = Some(Overlay::ProjectKnowledge);
        self.project_knowledge = Some(ProjectKnowledgeState {
            decisions,
            constraints,
            features,
            failed_attempts,
            todos,
            memory_note,
            cursor: 0,
            detail_open: false,
        });
        Action::Redraw
    }

    /// The project-knowledge view's own data, or `None` when it is not open.
    pub fn project_knowledge(&self) -> Option<&ProjectKnowledgeState> {
        self.project_knowledge.as_ref()
    }
}

impl ShellState {
    /// Open the project-memory view with memory the run loop already read
    /// from disk — every kind, at every status, unfiltered. Reading
    /// `crate::memory` is file I/O this module deliberately does not hold —
    /// see [`Self::open_project_overview`] for the same split. Map line 234.
    ///
    /// Opens even when `memory_note` is `Some`: a project whose memory
    /// database could not be read still gets an honest, empty view rather
    /// than no view at all — the same contract
    /// [`Self::open_project_knowledge`] keeps.
    pub fn open_project_memory(
        &mut self,
        memory: KnowledgeSection,
        memory_note: Option<String>,
    ) -> Action {
        self.overlay = Some(Overlay::ProjectMemory);
        self.project_memory = Some(ProjectMemoryState {
            memory,
            memory_note,
            cursor: 0,
            detail_open: false,
        });
        Action::Redraw
    }

    /// The project-memory view's own data, or `None` when it is not open.
    pub fn project_memory(&self) -> Option<&ProjectMemoryState> {
        self.project_memory.as_ref()
    }
}

impl ShellState {
    /// Answer one key while the project-knowledge view is open.
    ///
    /// Unlike [`Self::handle_project_overview_key`], this overlay now has a
    /// cursor and something to act on — map line 1105's drill-down — so it
    /// claims Up/Down and Enter the same way [`Self::handle_overview_key`]
    /// does, and passes everything else through unchanged. While the detail
    /// popup is open every key but its own close key is swallowed rather
    /// than passed through: a live shell moving underneath a popup that is
    /// itself showing detail on a *specific* entry would let the cursor
    /// wander before the detail closes, silently showing the wrong memory's
    /// detail next time.
    pub(super) fn handle_project_knowledge_key(
        &mut self,
        key: KeyEvent,
        had_status: bool,
    ) -> Action {
        if self
            .project_knowledge
            .as_ref()
            .is_some_and(ProjectKnowledgeState::detail_open)
        {
            return match key.code {
                KeyCode::Esc => self.close_knowledge_detail(),
                _ => Action::None,
            };
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('k') => self.close_overlay(),
            KeyCode::Up => self.move_knowledge_cursor(-1),
            KeyCode::Down => self.move_knowledge_cursor(1),
            KeyCode::Enter => self.open_knowledge_detail(),
            _ => self.handle_control_key(key, had_status),
        }
    }

    /// Move the project-knowledge cursor, wrapping — the same ring
    /// [`Self::move_overview_cursor`] is, for the same reason.
    fn move_knowledge_cursor(&mut self, delta: isize) -> Action {
        let total = self
            .project_knowledge
            .as_ref()
            .map(ProjectKnowledgeState::total_entries)
            .unwrap_or(0);
        if total == 0 {
            self.set_status("nothing to select in the project-knowledge view");
            return Action::Redraw;
        }
        if let Some(knowledge) = self.project_knowledge.as_mut() {
            knowledge.cursor =
                (knowledge.cursor as isize + delta).rem_euclid(total as isize) as usize;
        }
        Action::Redraw
    }

    /// Open the detail popup for the entry under the cursor — map line
    /// 1105. A project with nothing recorded yet has nothing to select, so
    /// this refuses rather than opening a detail popup with nothing in it.
    fn open_knowledge_detail(&mut self) -> Action {
        let has_selection = self
            .project_knowledge
            .as_ref()
            .is_some_and(|knowledge| knowledge.total_entries() > 0);
        if !has_selection {
            self.set_status("nothing selected to inspect");
            return Action::Redraw;
        }
        if let Some(knowledge) = self.project_knowledge.as_mut() {
            knowledge.detail_open = true;
        }
        Action::Redraw
    }

    /// Close the detail popup, returning to the entry list — the cursor is
    /// left exactly where it was, so reopening the same key shows the same
    /// memory.
    fn close_knowledge_detail(&mut self) -> Action {
        if let Some(knowledge) = self.project_knowledge.as_mut() {
            knowledge.detail_open = false;
        }
        Action::Redraw
    }

    /// Answer one key while the project-memory view is open — the same
    /// shape as [`Self::handle_project_knowledge_key`], over one unfiltered
    /// list instead of five curated sections.
    pub(super) fn handle_project_memory_key(&mut self, key: KeyEvent, had_status: bool) -> Action {
        if self
            .project_memory
            .as_ref()
            .is_some_and(ProjectMemoryState::detail_open)
        {
            return match key.code {
                KeyCode::Esc => self.close_memory_detail(),
                _ => Action::None,
            };
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('M') => self.close_overlay(),
            KeyCode::Up => self.move_memory_cursor(-1),
            KeyCode::Down => self.move_memory_cursor(1),
            KeyCode::Enter => self.open_memory_detail(),
            _ => self.handle_control_key(key, had_status),
        }
    }

    /// Move the project-memory cursor, wrapping — the same ring
    /// [`Self::move_knowledge_cursor`] is, for the same reason.
    fn move_memory_cursor(&mut self, delta: isize) -> Action {
        let total = self
            .project_memory
            .as_ref()
            .map(ProjectMemoryState::total_entries)
            .unwrap_or(0);
        if total == 0 {
            self.set_status("nothing to select in the project-memory view");
            return Action::Redraw;
        }
        if let Some(memory) = self.project_memory.as_mut() {
            memory.cursor = (memory.cursor as isize + delta).rem_euclid(total as isize) as usize;
        }
        Action::Redraw
    }

    /// Open the detail popup for the entry under the cursor. A project with
    /// nothing recorded yet has nothing to select, so this refuses rather
    /// than opening a detail popup with nothing in it — the same rule
    /// [`Self::open_knowledge_detail`] follows.
    fn open_memory_detail(&mut self) -> Action {
        let has_selection = self
            .project_memory
            .as_ref()
            .is_some_and(|memory| memory.total_entries() > 0);
        if !has_selection {
            self.set_status("nothing selected to inspect");
            return Action::Redraw;
        }
        if let Some(memory) = self.project_memory.as_mut() {
            memory.detail_open = true;
        }
        Action::Redraw
    }

    /// Close the detail popup, returning to the entry list — the cursor is
    /// left exactly where it was, the same rule
    /// [`Self::close_knowledge_detail`] follows.
    fn close_memory_detail(&mut self) -> Action {
        if let Some(memory) = self.project_memory.as_mut() {
            memory.detail_open = false;
        }
        Action::Redraw
    }
}
