# Capability evidence — phase 3

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 3 — return from overlays to the active native session, and propagate resize to it

Lines: "Allow the user to return from Glasshouse overlays to the active native
session without terminating it" and "Preserve terminal resize events and
propagate the new dimensions to the active embedded terminal".

Contract: Given an overlay open over a live harness session, when the user
leaves it, they are returned to that same session still running, and a resize
of Glasshouse's window reaches that session's own terminal.

State: COMPLETE

Both lines were blocked until this session, for the same reason: there was no
live native session to return to or to resize. `session::runtime` supplies one
and `shell::run` drives it.

Production evidence:
- `crates/glasshouse/src/shell/state.rs: close_overlay`, `enter_session_mode` —
  leaving an overlay restores the previous mode; entering session mode closes
  any open overlay. Neither touches a process.
- `crates/glasshouse/src/shell/mod.rs: run` — `Event::Resize` calls
  `screen.on_resize` and `SessionRuntime::resize` for the focused session.

Regression evidence:
- `leaving_an_overlay_returns_to_the_active_session_without_ending_it`
- `entering_session_mode_closes_any_open_overlay`
- `entering_and_leaving_session_mode_never_touches_a_real_process` — spawns a
  real child and checks its pid and liveness across the switch.
- `resizing_the_shell_reaches_the_harness_terminal` (Unix, tests/pty_smoke.rs)
  — the harness is asked `stty size` before and after Glasshouse's own terminal
  is resized, through the shipped binary.

Failure/isolation evidence:
- Mutation: making Escape always quit fails the overlay-first test.
- The resize test initially failed for two different reasons, both instructive:
  first because it asked before the SIGWINCH had travelled (a test timing
  fault, not a defect), and then because the escape chord never matched — see
  the Phase 4 entry.

Platform/external evidence:
- CI `32821964808` on `f77b9c8` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 287 lib and 33 PTY tests,
  including the shell's mode machinery in a real terminal. The resize test is Unix-only: `stty` is the
  portable way for a shell harness to report its terminal size and Windows has
  no equivalent a `.cmd` harness can run. The underlying `PtyProcess::resize`
  is covered on all three platforms by `resize_reaches_the_operating_system`
  and `a_resize_is_visible_to_the_child_process`.

Missing evidence:
- none.

### Phase 3 — Build the main interactive interface with Ratatui and Crossterm

Contract: Given a terminal on both standard input and standard output, when
`glasshouse` is run with no arguments, it opens a full-screen interface that
answers the keyboard and restores the terminal on the way out, while a piped or
redirected run falls back to the plain summary rather than drawing into a file.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/shell/mod.rs: run` — the event loop, entered from
  `main.rs`'s no-argument arm behind an `IsTerminal` check on both streams.
- `crates/glasshouse/src/tui/mod.rs: Screen` — terminal ownership, restored by
  `TerminalGuard` on a normal return, an error, a panic, or a signal.

Regression evidence:
- `the_shell_opens_in_a_real_terminal_and_answers_the_keyboard`
  (tests/pty_smoke.rs) — the shipped binary in a real pseudo-terminal: the
  interface draws, `o` opens the overview, Escape leaves it, `q` exits cleanly,
  and the alternate screen is left behind.
- The whole `shell::state` and `shell::view` suite, driven without a terminal.

Failure/isolation evidence:
- Mutation: making the no-argument arm fall through to the summary instead of
  the shell fails the pty_smoke test.
- The test asserts the alternate screen was left, so an exit that stranded the
  user on a dead frame would fail.

Platform/external evidence:
- CI `32816717226` on `5da067a` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 257 lib and 23 PTY tests,
  including the real-terminal shell test, rather than merely reporting green.

Missing evidence:
- none.

### Phase 3 — Create a persistent top bar that shows the project name, project root, and active session

Contract: Given any shell screen, when a frame is drawn, the project name, the
active canonical project root, and the session currently presented are all on
it, while a terminal too narrow for the root keeps the tail that identifies the
project rather than the head.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/shell/view.rs: render_title`, `render_root`.

Regression evidence:
- `the_project_root_is_displayed_on_every_frame`
- `the_project_root_stays_visible_while_an_overlay_is_open`
- `a_narrow_terminal_keeps_the_end_of_the_project_root` and
  `a_wide_terminal_shows_the_whole_project_root` — asserted against the root's
  own row, not the whole frame.
- `the_shell_opens_in_a_real_terminal_and_answers_the_keyboard` — the root is
  checked in the real terminal's output, anchored to the `root ` field.

Failure/isolation evidence:
- Mutation: blanking the root fails both the unit test and the pty_smoke test.
  The first version of the pty_smoke assertion survived this mutation, because
  the project's name and its root's last component are the same string and a
  bare `contains` matched the title bar; it now anchors on the field.
- Mutation: truncating the root from the end instead of the start fails
  `a_narrow_terminal_keeps_the_end_of_the_project_root`. That test also
  initially survived, for the same reason, and now reads a single row.

Platform/external evidence:
- CI `32816717226` on `5da067a` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 257 lib and 23 PTY tests,
  including the real-terminal shell test, rather than merely reporting green.

Missing evidence:
- none.

### Phase 3 — Create a persistent session bar that lists currently known sessions

Contract: Given the project's recorded sessions, when a frame is drawn, every
one of them appears in the bar with the active one distinguished, while a
project with no sessions says so instead of showing an empty strip.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/shell/view.rs: render_session_bar`, over the records
  `shell::run` reads from `session::store`.

Regression evidence:
- `the_session_bar_lists_every_known_session`
- `an_empty_project_says_so_instead_of_showing_an_empty_bar`
- `the_shell_opens_in_a_real_terminal_and_answers_the_keyboard` starts two real
  sessions first, so the bar is drawn from records a real launch wrote.

Failure/isolation evidence:
- Mutation: dropping the per-session span fails the listing test.

Platform/external evidence:
- CI `32816717226` on `5da067a` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 257 lib and 23 PTY tests,
  including the real-terminal shell test, rather than merely reporting green.

Missing evidence:
- none.

### Phase 3 — Create a central viewport reserved for the active session terminal

Contract: Given a shell screen, when a frame is drawn, the central region is
reserved for the active session's terminal and describes what will occupy it,
while never drawing a convincing empty terminal for a session that is not
attached.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/shell/view.rs: render_viewport` — a bordered region
  sized by the layout solver, holding the active session's identity and an
  explicit note that the space is reserved.

Regression evidence:
- `an_empty_project_says_so_instead_of_showing_an_empty_bar` — with no session
  the viewport says so rather than looking like an idle terminal.
- `renders_without_panicking_at_absurd_sizes` — 1x1 through 200x60, with and
  without an overlay.

Failure/isolation evidence:
- Nothing here computes a size by subtraction, which is the usual way a
  "must not panic on a tiny terminal" claim fails; the 1x1 cases prove it.

Platform/external evidence:
- CI `32816717226` on `5da067a` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 257 lib and 23 PTY tests,
  including the real-terminal shell test, rather than merely reporting green.

Missing evidence:
- The viewport is reserved, not filled. Embedding a live harness terminal is
  Phase 5, and this deliberately does not fake it.

### Phase 3 — Create a compact bottom status bar for Glasshouse-level key bindings and status messages

Contract: Given any shell screen, when a frame is drawn, one row carries
Glasshouse's own key bindings for that screen, and a key that could not do
anything leaves a note beside them, while the bindings survive a terminal too
narrow for both.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/shell/view.rs: render_footer`
- `crates/glasshouse/src/shell/state.rs: ShellState::set_status`, set when
  navigation has nowhere to go and cleared by the next keystroke.

Regression evidence:
- `the_status_bar_always_shows_the_key_bindings` — including the overlay's
  different bindings.
- `the_status_bar_shows_a_note_next_to_the_bindings`
- `a_note_is_dropped_rather_than_crowding_out_the_bindings`
- `a_status_note_is_cleared_by_the_next_keystroke`

Failure/isolation evidence:
- Mutation: dropping the hint span fails the bindings test.
- Mutation: removing the status message fails the navigation test.
- Mutation: writing the note before the bindings fails the narrow-terminal
  test, which is the entire mechanism — the row clips on the right, so order
  decides what is lost.

Platform/external evidence:
- CI `32816717226` on `5da067a` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 257 lib and 23 PTY tests,
  including the real-terminal shell test, rather than merely reporting green.

Missing evidence:
- none.

### Phase 3 — Allow the user to move to the previous / next session with a keyboard shortcut

Contract: Given a project with several sessions, when the user presses Tab or
Shift-Tab (or Right/Left), the shell presents the next or previous session,
wrapping at either end, while changing nothing about any session itself.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/shell/state.rs: ShellState::next_session`,
  `previous_session`, reached from `handle_key`.

Regression evidence:
- `tab_moves_to_the_next_session_and_wraps`
- `shift_tab_moves_to_the_previous_session_and_wraps`
- `arrow_keys_navigate_the_same_way_as_tab`
- `navigating_changes_only_which_session_is_presented` — the session list is
  compared before and after, so navigation cannot be quietly mutating records.
- `navigating_with_fewer_than_two_sessions_explains_itself`
- `a_refresh_keeps_the_same_session_presented_even_when_the_order_changes` —
  the selection follows the session's identifier, not its index, so a
  background refresh cannot move the user to a different session.

Failure/isolation evidence:
- `an_empty_project_has_no_active_session_and_does_not_panic`
- Mutation: removing the no-op status message fails the explanatory test.

Platform/external evidence:
- CI `32816717226` on `5da067a` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 257 lib and 23 PTY tests,
  including the real-terminal shell test, rather than merely reporting green.

Missing evidence:
- Bindings are plain single keys because no native session owns the keyboard
  yet. When one does (Phase 5) they must move behind a prefix or a mode, or
  they will steal keystrokes the harness needs. `handle_key` is deliberately
  the only place that has to change.

### Phase 3 — Allow the user to open a session overview from the keyboard

Contract: Given any shell screen, when the user presses `o`, an overview opens
showing every session with the detail the bar has no room for, while the shell
stays visible around it and the active session is unchanged.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/shell/state.rs: ShellState::open_overview`
- `crates/glasshouse/src/shell/view.rs: render_overview` — drawn over the
  shell, not in place of it.

Regression evidence:
- `o_opens_the_session_overview_from_the_keyboard`
- `the_overview_shows_detail_the_session_bar_has_no_room_for`
- `leaving_an_overlay_returns_to_the_active_session_without_ending_it`
- `escape_leaves_an_overlay_first_and_only_then_leaves_glasshouse`
- `the_shell_opens_in_a_real_terminal_and_answers_the_keyboard` opens it with a
  real keystroke in a real terminal.

Failure/isolation evidence:
- Mutation: making Escape always quit fails the overlay-first test, which is
  what stops Escape closing Glasshouse from inside the overview.

Platform/external evidence:
- CI `32816717226` on `5da067a` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 257 lib and 23 PTY tests,
  including the real-terminal shell test, rather than merely reporting green.

Missing evidence:
- none.

### Phase 3 — Keep the visual design text-first and avoid decorative graph visualizations that do not expose actionable state

Contract: Given any shell screen, when a frame is drawn, it contains only text
and the box-drawing characters that frame it, and never a gauge, sparkline, or
bar chart.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/shell/view.rs` uses only `Paragraph`, `Block`, and
  `Clear`. No Ratatui chart, gauge, sparkline, or canvas widget is imported.

Regression evidence:
- `nothing_draws_with_block_elements_so_the_design_stays_text_first` — renders
  the shell, the overview, and a screen carrying a status note, and fails on
  any character in U+2580..U+259F. Ratatui's decorative widgets are all drawn
  from that block-element range, so a frame containing none of it cannot be
  rendering one. Border characters live in a different range and stay allowed.

Failure/isolation evidence:
- Mutation: adding a `load ▇▇▅▂▁` line to the viewport fails the test, so it
  is a real check rather than a restatement of intent.

Platform/external evidence:
- CI `32816717226` on `5da067a` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 257 lib and 23 PTY tests,
  including the real-terminal shell test, rather than merely reporting green.

Missing evidence:
- Mechanical rather than aesthetic: the test proves no block-element widget is
  drawn, not that the layout is well judged.

---

## Phase 3 line 234 — CLOSED 2026-08-29 (batch 48). Phase 3 is now 12/12.

Contract: Given a project with durable memory records, when the user presses
the project-memory key from the shell, Glasshouse opens a view of that
project's memory — including record kinds and statuses the knowledge overlay
does not show — while never displaying another project's records and never
failing the shell when memory cannot be read.

State: COMPLETE

**The gap was not "no view exists".** `Overlay::ProjectKnowledge` is opened by
`k` and is built from memory records — but `knowledge_section` is called for
five kinds only (`shell/mod.rs:1584-1599`, `record.kind == kind`), each with a
status filter. A `MemoryKind::Finding` record, and any record at a status those
filters drop, was unreachable from every keyboard-reachable surface.

Production evidence:
- `crates/glasshouse/src/shell/state.rs` — a new overlay and its `Action`,
  opened by `M`, closed by `Esc` and by the same key, following
  `ProjectKnowledge`'s shipped pattern.
- `crates/glasshouse/src/shell/mod.rs` — builds the view over `MemoryKind::ALL`
  and every status, reusing `knowledge_detail`'s existing `MemoryDetail`
  mapping rather than a second one.
- `crates/glasshouse/src/shell/view.rs` — renders it, and the status bar
  advertises the key.

**`ProjectKnowledge` was deliberately not widened.** It is Phase 25's curated
knowledge view, constrained by map lines 1098-1107; changing its shape to serve
a Phase 3 line would have altered a shipped, spec-constrained surface. The
decision was made in the packet, not by the worker.

Regression evidence:
- `shell::project_memory_tests::a_finding_record_appears_in_the_project_memory_view`
  — the test that distinguishes this view from `ProjectKnowledge`.
- `shell::project_memory_tests::opening_the_project_memory_view_shows_real_memory`
- `..::the_project_memory_view_says_so_when_there_is_nothing_recorded` — the
  honest empty state.
- `the_status_bar_always_shows_the_key_bindings` — the key is advertised; a
  view nobody is told about is not keyboard-reachable in the sense the line
  means.

Mutation, re-run by the orchestrator:

| mutation | vocabulary | result |
|---|---|---|
| the view's `MemoryKind::ALL` narrowed to `ProjectKnowledge`'s own five kinds, dropping `Finding` | `skip-state-update` | **killed** — `a_finding_record_appears_in_the_project_memory_view` and `opening_the_project_memory_view_shows_real_memory` both FAILED under `--lib shell::` |

**A packet error the worker caught, and it was the orchestrator's.** The packet
claimed `MemoryKind` has seven variants including `Invariant`, and wrote the
acceptance test against that. `MemoryKind` has **six**; `Invariant` belongs to
`MemoryAuthority`, a different field on `MemoryRecord` describing how binding a
memory is rather than what kind of thing it is. The orchestrator had read one
enum's range into the next. The worker refused to add a seventh kind — which
would have needed a schema migration for a variant the map never asks for —
said so first in its report, and built the view kind-agnostically instead, which
satisfies the contract as written. Fifteenth consecutive round in which a
worker corrected its packet.

Platform/external evidence: no `#[cfg]` added; the shell suite runs on Windows
for real on the ARM64 VM. Missing: nothing for this line.
