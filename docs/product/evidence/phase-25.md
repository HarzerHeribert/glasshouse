# Capability evidence — phase 25

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 25 — the project-knowledge view, built by copying a proven read path

Contract: Given a project whose memory database holds decisions, constraints,
features, failed approaches and todos, when the user opens the project-knowledge
view in the TUI, Glasshouse renders them as plain grouped text sections, each
kind in its own labelled section, naming supersession relationships where they
exist — while reading only the current project's database and never drawing a
decorative graph.

State: **COMPLETE** for all ten of map lines 1098–1107. Nine landed in batch 41;
**1105 landed in batch 42**, deliberately deferred one batch as a second
interaction shape rather than bundled and rushed.

**No new mechanism.** The package copied Phase 41's proven pattern rather than
inventing one: open `ProjectMemory` from the run loop, read through the store,
build a plain data struct, hand it to a `ShellState` overlay, render it in
`view.rs`. Nothing under `memory/**` was edited — the existing public API served
the whole feature, which is the strongest evidence the read path was already the
right one.

Production evidence:

- `shell/mod.rs::build_project_knowledge_memory` — opens `ProjectMemory::open(runtime)`
  and reads through the store, modelled directly on
  `build_project_overview_memory` (Phase 41).
- `shell/state.rs::Overlay::ProjectKnowledge` and `ShellState::open_project_knowledge`,
  bound to `k` in `ShellState::handle_control_key` — line 1098.
- `shell/view.rs::render_project_knowledge` and `push_knowledge_section` — plain
  ratatui `Line`/`Span` text, one labelled section per `MemoryKind`. **Grouped,
  not hierarchical**, which line 1099 permits explicitly; the choice is recorded
  rather than left implicit.
- `knowledge_section(&store, MemoryKind::Decision, |status| status.is_current())`
  and its siblings for `Constraint`, `Feature`, `Todo` — lines 1100, 1101, 1102,
  1104. Failed approaches (line 1103) are shown **regardless of status**, because
  the line asks for a *historical* section and filtering it to current would
  empty it by definition.
- `shell/mod.rs::knowledge_line`'s `superseded_by` branch — line 1106, printed
  only when a successor exists.

Regression evidence:

- `shell::project_knowledge_tests::opening_the_project_knowledge_view_shows_real_memory`
  — key press → build → state, against a real on-disk database.
- `shell::project_knowledge_tests::a_project_with_no_knowledge_yet_reports_empty_sections_not_an_error`
- `shell::project_knowledge_tests::a_superseded_decision_does_not_appear_among_active_decisions`
- `shell::project_knowledge_tests::constraints_and_features_are_filtered_to_current_the_same_way_decisions_are`
- `shell::project_knowledge_tests::failed_approaches_are_shown_regardless_of_status_and_name_their_successor`
- `shell::view::tests::the_project_knowledge_view_shows_active_decisions_in_their_own_section`
  and its four siblings — one per section, at the render layer.
- `shell::view::tests::the_project_knowledge_view_renders_no_decorative_graph_glyphs` — line 1107.
- `shell::view::tests::the_project_knowledge_view_says_so_when_every_section_is_empty`

**Each section has its own failing test.** A single test asserting "the view has
sections" would have claimed five boxes and proven one.

Failure/isolation evidence:

- Mutations: `remove-guard` (drop the `is_current` filter so history leaks into
  active decisions), `skip-state-update` (stop reading `superseded_by`) and
  `invert-condition` (print a supersession note when there is none) — **all
  killed**.
- **One mutation SURVIVED, and it is recorded rather than worked around.**
  Removing `Action::OpenProjectKnowledge`'s `Err` arm — the wiring that opens the
  overlay with an honest note when the memory read fails — killed no test. The
  worker found this itself and reported it.
  **Root cause:** that arm lives inside `shell::run()`, the real interactive
  event loop that owns a live terminal via `Screen::acquire()`, and **nothing in
  this codebase unit-tests that loop.** Phase 41 has the structurally identical
  untested `Action::OpenProjectOverview` `Err` arm, so this is a pre-existing
  shape rather than a regression.
  **What is proven instead:** `ShellState::open_project_knowledge` opens
  unconditionally regardless of the note
  (`a_project_knowledge_read_failure_still_opens_with_an_honest_note`), so the
  *state* layer's behaviour is covered; only the run-loop wiring is not.
  **What would close it:** something that forces `ProjectMemory::open` to `Err`.
  No existing test does, so it is new test infrastructure — and it would close
  Phase 41's identical gap at the same time. **Recorded as debt against the
  run-loop, not against these boxes**, none of which depend on the failure path.
- **Binary run, through a real PTY, with real memories.** The worker built the
  binary, created a scratch git project, ran the onboarding wizard to completion
  in tmux, seeded five memories of five kinds through the **production**
  `glasshouse memory extract --reply-from` pipeline (real admission guard, real
  store — not hand-inserted rows), launched the shell in a 140×45 tmux pty,
  pressed `k`, and captured the pane. It rendered `ACTIVE DECISIONS`, `KNOWN
  CONSTRAINTS`, `FEATURES (IMPLEMENTED OR PLANNED)` and `FAILED APPROACHES
  (HISTORICAL)` with their real contents. This is the standard this project
  holds — every real defect here has been found by running the shipped binary in
  a real terminal.

**§17's viewport rule was applied and its limit reasoned about.** Line 1107's
absence assertion renders at **both** (120,40) and (400,60), so it cannot pass
merely because the glyphs were clipped. The empty-state test renders at one size
only, and the report argues why: it asserts the *presence* of short empty-note
strings rather than the absence of a value that could be truncated, so §17's
criterion does not bite. That reasoning is correct and matches Phase 41's own
precedent.

**A packet error worth recording, because the worker flagged it instead of
silently resolving it.** The acceptance test was written as *"the rendered output
contains no box-drawing/graph glyphs"*, which read literally would fail **every**
overlay in this shell — they all use a bordered `Block`. The worker resolved it
the way this codebase already resolves the analogous line-1771 requirement
(`nothing_draws_with_block_elements_so_the_design_stays_text_first`): the popup's
own single rectangular border is allowed; the node-marker and arrow glyphs a
scattered-node graph would need are forbidden. **That is the correct reading and
it is what the project already does**; the packet's wording was the defect.

Gates run by the integrator on the integrated tree: recorded with the batch in
`docs/process/handoff.md`.

**The last line, and why it waited a batch.**

**1105 — closed in batch 42, and deferring it was the right call.**
`ProjectKnowledgeState` gained a cursor over every section's entries in render
order plus a `detail_open` flag; Enter opens a detail popup carrying the selected
memory's rationale, source session, source commit and lifecycle state; Esc closes
the detail first — cursor unmoved — and only then the overlay, mirroring
`handle_overview_entry_key`'s existing nested-Esc shape rather than inventing a
second idiom. **A memory missing a rationale, session or commit renders "not
recorded" for each**, never an empty field and never a fabricated one.

Production evidence: `shell::knowledge_detail(&MemoryRecord) -> state::MemoryDetail`,
called from `knowledge_section` alongside the existing `knowledge_line` so
`KnowledgeSection.details` stays index-aligned with `.lines`; driven by
`ShellState::{open_knowledge_detail, move_knowledge_cursor, close_knowledge_detail}`
through the real `handle_key` path, rendered by `view::render_knowledge_detail`.
**No `memory/**` change was needed** — the second package in a row to close boxes
against that module's existing public API without touching it.

Platform/external evidence: pure rendering over SQLite, no `#[cfg]` beyond
`#[cfg(test)]`.

Missing evidence:

- Nothing outstanding for the nine closed lines.
- The run-loop `Err`-arm gap above is shared with Phase 41 and belongs to
  whoever first builds a way to force `ProjectMemory::open` to fail.
