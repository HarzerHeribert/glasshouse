# Capability evidence — phase 5

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 5 — native terminal embedding (complete, 8 of 8)

Contract: Given a live harness session, when Glasshouse draws it, the harness's
own interface appears as it drew it — colours, cursor, wrapping and control
sequences intact — and the harness's own commands, prompts and controls keep
working, while Glasshouse's chrome stays out of the way.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/session/runtime.rs`: each `LiveSession` owns a
  `vt100::Parser` fed by the reader thread; `answer_terminal_queries` replies to
  `ESC[6n`; `resize` moves the emulator grid and the child's pseudo-terminal
  together.
- `crates/glasshouse/src/shell/mod.rs`: `build_viewport_grid`, `cell_style` and
  `convert_color` — the single place vt100's colour model meets Ratatui's.
  The tick rebuilds the grid, answers terminal queries, and sizes the child from
  the viewport's inner rect rather than the outer terminal.
- `crates/glasshouse/src/shell/view.rs`: `GridView` draws the grid cell by cell.
  The border is dropped once a live grid exists, so the harness gets the whole
  area — the chrome is four rows and the harness's name is already in the title.

Regression evidence:
- `an_embedded_session_answers_the_cursor_position_query_itself` — a real
  harness asks, and receives exactly `ESC[1;1R`.
- `colours_bold_inverse_and_cursor_position_survive_the_conversion`,
  `line_wrapping_is_preserved_in_the_grid`, `a_hidden_cursor_is_not_shown`,
  `a_fresh_screen_converts_to_a_full_grid_of_blank_cells`.
- `the_viewport_border_is_dropped_once_a_live_grid_is_shown`,
  `the_viewport_does_not_panic_with_a_real_grid_at_absurd_sizes`,
  `a_cursor_outside_the_render_area_does_not_panic`.
- The cursor-query scanner is tested at every one of the five possible read
  splits, one byte at a time, on a near miss, and on `ESC ESC [ 6 n`.

Failure/isolation evidence — mutations, each observed to fail its target:
- Nothing answers the cursor query: the harness hangs for the full timeout.
- The reply uses vt100's zero-based cursor: emits `ESC[0;0R`, not a position.
- The scanner forgets a byte that begins a fresh match.
- Every colour converts to default; every modifier is dropped.

**Two findings worth recording.**

*The responder had no production caller.* It shipped in `a1fa6c0` called only
from its own test — exactly the standard applied to Phase 1 line 90 and to the
runtime boxes, missed by the orchestrator who wrote it and caught by the worker
implementing the rendering. An embedded harness sending `ESC[6n` at startup
would have hung in the real shell while every test passed. It now runs on the
tick.

*The viewport's clipping clamp is not observable.* Removing
`area.height.min(grid.rows())` changes no rendered frame: `Buffer::cell_mut`
refuses anything outside the buffer, and the chrome below the viewport is drawn
after it. A containment test written to catch this passed for the wrong reason —
render order, not clipping — and was deleted rather than kept. The clamp stays,
with a comment saying plainly that it is cheap insurance rather than the thing
keeping the frame intact, because the render order it currently relies on is not
a property the widget can see.

Platform/external evidence:
- CI `32830685235` on `79a0600` — green on Linux, macOS, Windows and lint, with
  the Windows job confirmed to have executed 321 lib and 33 PTY tests including
  the colour, wrapping and border tests by name.

Missing evidence:
- Fidelity is asserted against synthetic escape sequences, not against Claude
  Code's or Codex's real TUI. The stated bar is "usable", which only a real
  harness can settle. `vt100` was chosen partly because swapping to
  `alacritty_terminal` is a bounded change if it proves insufficient.

### Phase 5 — the input half of native terminal embedding

Lines: "Preserve native harness input behavior instead of replacing it with a
Glasshouse chat composer"; "Allow native slash commands to pass directly to the
underlying harness"; "Add an escape key sequence that temporarily captures input
for Glasshouse-level navigation without permanently stealing input from the
harness".

Contract: Given a session on screen, when the user types, every keystroke
reaches the harness as the bytes its own interface expects — including the keys
Glasshouse binds for itself — while one reserved chord borrows input for
Glasshouse and hands it straight back.

State: COMPLETE

These three are satisfied by the session-mode design (see
`docs/product/design-decisions.md`) rather than by new work, and are checked here
as reconciliation. The rest of Phase 5 is the *rendering* half and needs a
terminal emulator.

Production evidence:
- `crates/glasshouse/src/shell/state.rs: handle_key`, `encode` — in session
  mode the mode is consulted before any binding, and every key is encoded to
  the bytes a terminal would send. Glasshouse has no composer, no input buffer,
  and no interpretation of `/`.
- `crates/glasshouse/src/shell/state.rs: is_session_escape` — one chord,
  `Ctrl-]`, both platform spellings.

Regression evidence:
- `a_slash_command_passes_straight_through_to_the_harness` — every character of
  `/compact` forwarded verbatim.
- `keys_glasshouse_binds_elsewhere_belong_to_the_harness_in_session_mode` —
  `q`, `n`, `o`, `i`, Tab, Esc, Enter, Backspace and Up all reach the harness.
- `the_escape_captures_input_only_until_it_is_handed_back` — control mode's
  bindings work again, then input returns to the harness, with no session
  touched. "Temporarily" and "without permanently stealing", asserted.
- `the_shell_enters_and_leaves_session_mode_in_a_real_terminal` — the same
  round trip through the shipped binary on all three platforms.

Failure/isolation evidence:
- Mutation: consulting bindings before the mode makes `q` quit instead of
  reaching the harness.
- Mutation: accepting only one spelling of the escape chord fails the
  real-terminal test — the defect that shipped past a full unit-test suite.

Platform/external evidence:
- CI `32821964808` on `f77b9c8` — Linux, macOS, Windows and lint.

Missing evidence:
- The rendering half of Phase 5 is untouched. The viewport prints raw bytes, so
  escape sequences are shown rather than obeyed. Until an emulator exists,
  "native permission prompts remain interactive" and the colour/cursor/wrapping
  line stay unchecked — a prompt the user cannot read is not interactive, even
  if the keystrokes would reach it.
