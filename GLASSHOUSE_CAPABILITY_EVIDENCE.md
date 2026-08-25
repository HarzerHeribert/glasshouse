# Glasshouse capability evidence ledger

This ledger supports—but never replaces—the authoritative
[`GLASSHOUSE_IMPLEMENTATION_CAPABILITY_MAP.md`](GLASSHOUSE_IMPLEMENTATION_CAPABILITY_MAP.md).
It maps requirements to observable product contracts, production paths, and
non-vacuous regression evidence.

Populate entries incrementally as a capability becomes active or as previously
checked work is reconciled. Do not spend a whole implementation cycle filling
hundreds of future entries speculatively.

## Entry template

```markdown
### <phase and stable short name> — <exact capability text>

Contract: Given <context>, when <trigger>, Glasshouse <observable behavior>,
while preserving <invariant or failure behavior>.

State: NOT STARTED | SCAFFOLDED | PARTIALLY VERIFIED | LOCALLY VERIFIED |
CI VERIFIED | COMPLETE

Production evidence:
- `<file>: <symbol/path>` — why this is a real reachable production path

Regression evidence:
- `<test name>` — behavior proved and platforms actually executed

Failure/isolation evidence:
- `<test or probe>` — negative, fail-closed, cleanup, or boundary behavior

Platform/external evidence:
- `<CI run or runtime probe>` — commit and platforms covered

Missing evidence:
- exact remaining proof or implementation
```

## Evidence rules

- Quote the capability exactly enough to find it in the map.
- Keep the contract to one product sentence.
- Cite symbols and test names, not merely directories.
- State which platform actually executed a test.
- A test-only type or fake caller is not production evidence.
- A checked box requires **COMPLETE**.
- If later evidence contradicts an entry, downgrade it immediately and reopen
  the map checkbox if necessary.

## Active entries

### Phase 4 — the multi-session PTY runtime (covers seven map lines)

Lines: stream PTY output into an in-memory buffer; send text programmatically
without focus; send interrupt signals; bounded scrollback per live session;
keep inactive sessions running; switching changes only presentation focus;
headless presentation mode.

Contract: Given several harnesses started in one Glasshouse, when any of them
is acted on, it runs, buffers its own output within a fixed bound, and accepts
text or an interrupt whether or not it is the one on screen, while changing
which session is on screen never starts, stops, or signals any process.

State: PARTIALLY VERIFIED

Production evidence:
- `crates/glasshouse/src/session/runtime.rs: SessionRuntime`, `LiveSession`,
  `Scrollback` — each session gets its own reader thread and its own bounded
  buffer; focus is a field, and `focus()` touches nothing else.
- **No production caller yet.** `glasshouse launch` still uses
  `session::attach`, which is correct for handing one harness the whole
  terminal, and the shell reads records rather than live sessions. Until the
  shell drives this runtime, these seven boxes stay unchecked — the same
  standard applied to Phase 1 line 90 and to three Phase 3 lines.

Regression evidence (all in tests/pty_smoke.rs, real processes, real pseudo-
terminals, written by a Sonnet worker in an isolated worktree and re-verified
here):
- `two_sessions_run_concurrently_with_independent_scrollback`
- `an_unfocused_session_still_receives_sent_text`
- `focus_changes_nothing_but_focus` — pids recorded before and after five
  focus changes.
- `a_headless_session_runs_but_cannot_be_focused`
- `exit_is_detected_with_no_output_at_all`
- `scrollback_stays_bounded_under_real_output`
- `closing_one_session_leaves_the_others_running`
- `keystrokes_reach_the_focused_session`
- Plus unit tests for `Scrollback`: eviction order, an oversized chunk keeping
  its tail, a severed multi-byte character dropped rather than mangled, escape
  sequences preserved, zero capacity.

Failure/isolation evidence — mutations run by the orchestrator, each observed
to fail the test it targets: letting a headless session be focused; requiring
focus before `send_text`; making the scrollback unbounded; making `close` kill
every session; removing focus recovery after a close; giving all sessions one
shared scrollback; making `poll_exits` report nothing.

Two mutations did **not** fail, and both were acted on:

- Mutating `close` to kill every session initially left
  `closing_one_session_leaves_the_others_running` green, because `is_running()`
  reads the status cached by the last `poll_exits` — a freshly killed survivor
  still reported itself running. The test now polls the operating system over a
  window instead, and the mutation fails.
- Mutating `poll_exits` to wait for end-of-file before asking the process left
  `exit_is_detected_with_no_output_at_all` green. A harness that prints nothing
  and exits has its output end at the same instant, so that test cannot tell
  "asks the process" from "waits for output, then asks". An attempt to build
  the discriminating case — a harness leaving a background child holding the
  pseudo-terminal open past its own exit — was written and then removed,
  because a direct probe showed macOS reports end-of-file on the master as soon
  as the foreground child exits regardless of the background holder. The
  capability's real risk, mistaking a silent-but-running harness for a finished
  one, is covered by `exit_is_detected_from_the_process_not_from_quiet_output`.

Platform/external evidence:
- CI on this batch's commit.

Missing evidence:
- A production consumer. Wiring the runtime into the shell is the next batch,
  and it needs the keyboard-ownership decision recorded in
  `.agent-runtime/design-shell-session-modes.md`: the shell's single-key
  bindings cannot coexist with forwarding every keystroke to a harness.

### Phase 4 — Implement a generic PTY-backed child-process abstraction for interactive harnesses

Contract: Given any interactive harness, when Glasshouse runs it, it does so
through one pseudo-terminal abstraction that hides the platform difference
between Unix PTYs and Windows ConPTY, while exposing spawn, input, output,
resize, signal, and exit uniformly.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/pty/mod.rs: PtyProcess`, `TerminalCommand`,
  `PtyOutput`, `ExitStatus` — the only route by which Glasshouse starts a
  child, reached in production from `session::attach` (via `glasshouse launch`)
  and from `integrations`' version probes.

Regression evidence:
- `streams_output_and_reports_a_successful_exit`, `reports_a_failing_exit_code`,
  `forwards_input_to_an_interactive_child`,
  `terminating_stops_a_long_running_process`,
  `dropping_a_running_process_kills_it`,
  `terminate_reaches_the_session_leader_under_job_control` — all in
  tests/pty_smoke.rs, all against real processes in real pseudo-terminals.
- `the_launch_command_opens_the_configured_harness_inside_the_project_root` —
  the shipped binary end to end.

Failure/isolation evidence:
- `signalling_an_exited_process_is_reported_rather_than_misdirected` and its
  unpolled variant — a signal is never sent to a reused process identifier.
- `dropping_reaps_a_child_that_already_exited` — no zombies.
- `pty::open_pty` retries allocation five times, side-effect free, after the
  macOS `openpty(3)` race was diagnosed.

Platform/external evidence:
- CI on every batch this session — Linux, macOS and Windows, with the Windows
  job confirmed to have executed the PTY suite.

Missing evidence:
- none. Verified by reading the code and the named tests rather than taken from
  a worker's inventory, which had marked two neighbouring lines satisfied on
  the strength of production paths their named tests did not actually cover.

### Phase 4 — Detect process exit independently from textual terminal output

Contract: Given a harness that produces no output at all, when it ends,
Glasshouse notices from the process itself, while a harness that is merely
silent is never mistaken for one that has finished.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/pty/mod.rs: PtyProcess::try_wait` / `wait`, which read
  the child's status and cache it so it is reaped exactly once.
- `crates/glasshouse/src/session/attach.rs: supervise` polls `try_wait`, never
  output quiet, to decide a session is over.

Regression evidence:
- `exit_is_detected_from_the_process_not_from_quiet_output` — a child that
  prints nothing and lingers: `try_wait` reports it still running, then reports
  its exit. Silence and completion are proven distinguishable.
- `the_launch_command_opens_the_configured_harness_inside_the_project_root`
  asserts a distinctive exit code (7, so neither generic success nor generic
  failure) survives to the shipped binary's own exit code.

Failure/isolation evidence:
- `signalling_an_unpolled_but_exited_process_is_reported_rather_than_misdirected`
  — exit detection and signalling agree about a process that ended without
  anyone having polled it.

Platform/external evidence:
- CI on every batch this session — Linux, macOS and Windows.

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

### Phase 1 — Display the active canonical project root prominently in the TUI

Contract: Given the interactive shell, when any frame is drawn, the active
canonical project root is on its own labelled row, on every screen including
behind an overlay, while a narrow terminal drops the head of the path and keeps
the tail that identifies the project.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/shell/view.rs: render_root` — a dedicated row, not a
  corner. The value comes from `Project::display_root`, the same canonical root
  every access-control decision uses.

Regression evidence:
- `the_project_root_is_displayed_on_every_frame`
- `the_project_root_stays_visible_while_an_overlay_is_open` — "prominently"
  cannot mean "until you open something".
- `a_narrow_terminal_keeps_the_end_of_the_project_root`
- `a_wide_terminal_shows_the_whole_project_root`
- `the_shell_opens_in_a_real_terminal_and_answers_the_keyboard` — proved in a
  real terminal, anchored to the `root ` field rather than to any word that
  also appears in the title bar.

Failure/isolation evidence:
- Mutation: blanking the root fails the unit test and the real-terminal test.
- Mutation: truncating from the wrong end fails the narrow-terminal test.
- Both assertions were vacuous in their first form and were tightened after the
  mutations exposed them.

Platform/external evidence:
- CI `32816717226` on `5da067a` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 257 lib and 23 PTY tests,
  including the real-terminal shell test, rather than merely reporting green.

Missing evidence:
- none.

### Phase 2 — Persist Glasshouse session metadata independently from the native harness session files

Contract: Given a harness session started by Glasshouse, when the session is
recorded, Glasshouse stores its own session metadata in the project database
and can read it back in a later process, while never parsing, depending on, or
being invalidated by whatever session files the harness keeps for itself.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/session/store.rs: SessionStore::create` — the only
  writer of the `sessions` table; reached from `main.rs: launch_session`, which
  records a session before the harness process exists.
- `crates/glasshouse/src/main.rs: session_report` — `glasshouse sessions` reads
  the records back in a separate process.
- `crates/glasshouse/src/database.rs: MIGRATIONS[1]` — the `sessions` table.
  `native_session_id` is nullable, so a record is complete before any harness
  has produced an identifier and stays valid after the harness's own history is
  deleted.

Regression evidence:
- `launching_a_harness_records_a_session_that_a_later_command_reads_back`
  (tests/pty_smoke.rs) — the shipped binary, a real pseudo-terminal, two real
  harness runs, then a second process reading the records. Executed on macOS
  locally and on Linux, macOS and Windows in CI.
- `a_session_is_recorded_and_survives_a_reopen_with_no_harness_involved` —
  the record is complete with no harness identifier and survives a reopen.

Failure/isolation evidence:
- Mutation: making `create` skip its `INSERT` fails the pty_smoke test.
- Mutation: dropping the post-exit `note_lifecycle` call fails it.
- `a_session_write_is_refused_when_the_project_binding_is_missing` — writes are
  refused rather than orphaned when the database has no project bound.

Platform/external evidence:
- CI `32815286487` on `3d606e3` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 228 lib and 22 PTY tests
  (a green tick is not proof the suite ran: when the lib target fails,
  cargo never reaches the integration tests).

Missing evidence:
- none.

### Phase 2 — Persist a mapping between Glasshouse session IDs and native harness session IDs when native IDs are available

Contract: Given a harness that reveals its own session identifier, when
Glasshouse records it, the identifier is stored against exactly one Glasshouse
session and can be read back, while a Glasshouse session identifier never
changes and no native session can be claimed by two Glasshouse sessions.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/session/store.rs: SessionStore::set_native_session_id`
  — attaches the identifier after creation, which is when harnesses reveal it.
- `crates/glasshouse/src/database.rs: MIGRATIONS[1]` — the partial unique index
  `sessions_native_id` over `(harness, native_session_id)` is what makes the
  column a mapping rather than an annotation.

Regression evidence:
- `a_native_session_identifier_can_be_attached_later_and_read_back`
- `one_native_session_cannot_map_to_two_glasshouse_sessions`
- `two_harnesses_may_use_the_same_native_identifier`
- `many_sessions_may_have_no_native_identifier_at_once`

Failure/isolation evidence:
- Mutation: dropping the unique index lets one native session be claimed twice.
- Mutation: narrowing the index to `(native_session_id)` alone makes two
  harnesses collide.
- Mutation: replacing `NULL` with an empty-string sentinel makes every
  unidentified session collide — the reason the column stays nullable.

Platform/external evidence:
- CI `32815286487` on `3d606e3` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 228 lib and 22 PTY tests
  (a green tick is not proof the suite ran: when the lib target fails,
  cargo never reaches the integration tests).

Missing evidence:
- No harness adapter captures a native identifier yet (Phase 7/8), so in
  production the column is currently always `NULL`. The mapping mechanism is
  complete and proven; what feeds it is a later phase.

### Phase 2 — Persist the harness type, creation time, last activity time, role, lifecycle state, and project identifier for every session

Contract: Given any recorded session, when it is read back, every one of those
six fields is present and accurate, while creation time never changes and last
activity time advances on every state change and every recorded interaction.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/session/store.rs: SessionRecord`, `SessionStore::create`,
  `SessionStore::set_lifecycle`, `SessionStore::touch`.
- `crates/glasshouse/src/main.rs: launch_session` — moves a real session through
  `Starting` -> `Running` -> `Stopped`/`Failed`.

Regression evidence:
- `every_required_field_is_persisted` — asserted by value against an injected
  clock, not by round-trip.
- `every_role_and_lifecycle_value_round_trips`
- `activity_time_advances_while_creation_time_stays_put`
- `sessions_are_listed_most_recently_active_first`

Failure/isolation evidence:
- Mutation: stopping `set_lifecycle` from touching `last_activity_at` fails the
  activity test.
- Mutation: recording every ended session as `Stopped` fails the pty_smoke
  test, because a failed harness stops being distinguishable.
- `the_schema_rejects_enum_values_it_does_not_define` — `CHECK` constraints
  reject a role, lifecycle, or presentation the schema does not define.
- `an_unrecognized_stored_enum_value_is_reported_rather_than_guessed` — a value
  written by a future build surfaces as a typed error naming the column, not a
  panic or a silent default.
- `touching_an_unknown_session_reports_it_missing_rather_than_inventing_one`

Platform/external evidence:
- CI `32815286487` on `3d606e3` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 228 lib and 22 PTY tests
  (a green tick is not proof the suite ran: when the lib target fails,
  cargo never reaches the integration tests).

Missing evidence:
- none.

### Phase 2 — Persist the process presentation mode for every session

Contract: Given a session presented embedded, headless, or externally, when it
is recorded and read back, its presentation mode is preserved exactly, while an
undefined presentation value cannot be stored at all.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/session/store.rs: SessionPresentation`, stored by
  `SessionStore::create` and shown by `main.rs: session_report`.
- Vocabulary is the map's own (Phase 10/11: "embedded, headless, or externally
  presented"), not invented here.

Regression evidence:
- `every_presentation_mode_is_persisted` — all three modes.
- `launching_a_harness_records_a_session_that_a_later_command_reads_back`
  asserts the presentation column reaches the listing.

Failure/isolation evidence:
- `the_schema_rejects_enum_values_it_does_not_define` covers `presentation`.
- `stored_values_honour_format_width_so_listings_align` — the `Display` impls
  use `Formatter::pad`, so the listing's columns cannot silently go ragged.

Platform/external evidence:
- CI `32815286487` on `3d606e3` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 228 lib and 22 PTY tests
  (a green tick is not proof the suite ran: when the lib target fails,
  cargo never reaches the integration tests).

Missing evidence:
- Only `Embedded` occurs in production today, because `glasshouse launch` is the
  only session producer. `Headless` and `External` arrive with Phase 4's
  headless mode and Phase 17's cmux panes.

### Phase 2 — Persist enough metadata to distinguish active, resumable, closed, and failed sessions

Contract: Given the stored metadata alone, when Glasshouse classifies a
session, it separates active, resumable, closed, and failed without consulting
any harness, while never reporting a session resumable when nothing was
recorded to resume it to.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/session/store.rs: SessionRecord::disposition` — derived
  from lifecycle plus the presence of a native identifier, deliberately not a
  second stored column that could disagree with the first.
- `crates/glasshouse/src/main.rs: session_report` — the STATE column of
  `glasshouse sessions`.

Regression evidence:
- `the_four_dispositions_are_distinguishable_from_stored_metadata` — all seven
  lifecycle states, with and without a native identifier.
- `launching_a_harness_records_a_session_that_a_later_command_reads_back` — a
  clean exit reads as `closed` and a failing one as `failed`, end to end.

Failure/isolation evidence:
- Mutation: treating a stopped session with no native identifier as resumable
  fails the disposition test.
- `a_stopped_session_with_no_native_identifier_is_not_resumable` and
  `a_live_session_is_not_resumable` — the refusals `open_for_resume` enforces.

Platform/external evidence:
- CI `32815286487` on `3d606e3` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 228 lib and 22 PTY tests
  (a green tick is not proof the suite ran: when the lib target fails,
  cargo never reaches the integration tests).

Missing evidence:
- none.

### Phase 2 — Never store provider credentials directly in the project memory database

Contract: Given the project database at any schema version this build produces,
when its full schema is enumerated, there is no column and no key/value slot in
which a provider credential could be stored, while any future schema change
that adds one fails the build's tests until it is deliberately reviewed.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/database.rs: MIGRATIONS` — the complete schema is three
  tables: `project_metadata`, `schema_migrations`, `sessions`. None has a
  credential column.
- `crates/glasshouse/src/session/store.rs: NewSession` — the only way to create
  a session, and it has no field a secret could be passed through.

Regression evidence:
- `the_project_database_schema_has_nowhere_to_put_a_credential` — asserts the
  exact `(table, column)` list. Deliberately an allowlist rather than a name
  pattern: `project_metadata.key` would false-positive on any name match, and a
  credential column could just as easily be called `value`. Any new column
  fails this test until someone updates the list, which is the moment to ask
  whether it can hold a secret.
- `project_metadata_holds_only_the_project_identifier` — the one key/value table
  is pinned to its single known key, closing the route by which a secret could
  be stored without a schema change.

Failure/isolation evidence:
- The test fails by construction on any schema addition; it is an exact
  equality, so it cannot pass vacuously.

Platform/external evidence:
- CI `32815286487` on `3d606e3` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 228 lib and 22 PTY tests
  (a green tick is not proof the suite ran: when the lib target fails,
  cargo never reaches the integration tests).

Missing evidence:
- Provider credentials do not exist yet (Phase 9E). This entry proves the
  project database is not where they can land; it does not yet prove where they
  do land.

### Phase 1 — Reject any attempt to resume a Glasshouse-managed session whose project identifier differs from the current project identifier

Contract: Given a session record whose project identifier differs from the
active project's, when anything attempts to resume it, Glasshouse refuses and
names both projects, while leaving the record untouched and while the database
itself refuses to store such a record in the first place.

State: PARTIALLY VERIFIED

Production evidence:
- `crates/glasshouse/src/session/store.rs: SessionStore::open_for_resume` — the
  only resume entry point in the codebase; compares the stored project
  identifier against the active one before returning anything actionable.
- `crates/glasshouse/src/database.rs: MIGRATIONS[1]` — `BEFORE INSERT` and
  `BEFORE UPDATE OF project_id` triggers abort any row whose `project_id` is
  not the identifier bound in `project_metadata`. Structural, so no present or
  future query has to remember to filter by project.

Regression evidence:
- `resuming_a_session_belonging_to_another_project_is_refused` — the error names
  both projects and the planted record is left byte-for-byte intact.
- `the_database_refuses_to_store_a_session_from_another_project`
- `a_stored_session_cannot_be_reassigned_to_another_project`
- `a_stopped_session_of_this_project_can_be_resumed` — the permitted case, so
  the refusals above are not merely "resume never works".
- `two_projects_have_independent_session_lists`

Failure/isolation evidence:
- Mutation: removing the project comparison in `open_for_resume` fails the
  cross-project test.
- Mutation: dropping the `BEFORE INSERT` trigger fails the structural test.
- Mutation: weakening the trigger's `IS NOT` to `<>` fails
  `a_session_write_is_refused_when_the_project_binding_is_missing`, which is
  what proves the guard fails closed rather than silently passing a NULL
  comparison.

Platform/external evidence:
- CI `32815286487` on `3d606e3` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 228 lib and 22 PTY tests
  (a green tick is not proof the suite ran: when the lib target fails,
  cargo never reaches the integration tests).

Missing evidence:
- **No production caller.** There is no `glasshouse resume`, because resuming a
  harness needs an adapter that knows the harness's own resume mechanism
  (Phase 6/7/8), and no adapter captures a native session identifier yet. The
  guard is implemented, structurally enforced, and mutation-proven at the only
  layer a resume can pass through — but the capability says "reject any attempt
  to resume", and today no attempt can be made. The map box stays unchecked
  until a real resume path exists to reject. Do not close this by adding a
  `resume` command that can only ever report "not resumable"; that would be a
  stub dressed as a capability.

### Phase 1 — Ensure every spawned harness process starts with its working directory set to the current project root

Contract: Whenever Glasshouse invokes an installed harness—including discovery
probes and interactive sessions—the child starts in the active canonical
project root and never inherits an unrelated caller directory.

State: COMPLETE

Production evidence:

- `main::launch_session` is the production consumer: `glasshouse launch
  [harness]` resolves a harness through `session::select::select` and starts it
  through `launch::HarnessLaunch`, which is the only route that reaches PTY
  spawn for a harness. This closes the gap recorded in the previous revision of
  this entry, where `HarnessLaunch` had no production caller at all.
- `session::attach::attach` runs the resulting session against the real
  terminal: raw mode, input/output pumps, resize forwarding, exit propagation,
  and restoration on every exit path.
- `launch::HarnessLaunch::spawn` reaches PTY spawn only through
  `TerminalCommand::for_harness`, which derives the directory from
  `Project::display_root` and is `pub(crate)`.
- `integrations::Discovery::run(&Project)` threads the active project into
  `version::probe_version`, which sets `Command::current_dir` from
  `Project::display_root`.

Regression evidence:

- `the_launch_command_opens_the_configured_harness_inside_the_project_root`
  (pty_smoke, macOS) runs the *shipped binary* in a real pseudo-terminal and
  matches the harness's own report of its working directory against the project
  root by filesystem identity. Glasshouse itself is deliberately run from a
  different directory, so an inherited cwd cannot pass.
- `a_fake_installed_harness_launches_inside_the_discovered_project_root`
  proves the same for the `HarnessLaunch` mechanism directly.
- `version_probe_child_starts_in_the_active_project_root` uses a resolved fake
  probe that prints a version only in the correct child directory.
- `project_configured_executable_wins_over_user_level` and the rest of
  `session::select::tests` pin executable precedence and every refusal path.
- Windows-only tests pin verbatim drive and UNC prefix conversion.

Failure/isolation evidence:

- The end-to-end test installs a *decoy* executable at the user level and the
  real one at the project level; a precedence failure runs the decoy and fails
  the test loudly rather than silently passing.
- `a_failing_configured_executable_never_falls_back_to_path` proves a broken
  configured path is an error, not a silent substitution of another binary.
- `attaching_without_a_terminal_is_refused` fails closed rather than hanging on
  a pty query nothing can answer.
- `PtyProcess::spawn` refuses a working directory that does not exist instead
  of starting the child somewhere else.
- Unsafe Windows-script arguments are rejected before `HarnessLaunch` spawns.

Non-vacuity (mutations actually run, each observed to fail the test):

- Removing the project layer from `EffectiveConfig::executable` → the decoy
  runs; the end-to-end test fails on precedence.
- Making `TerminalCommand::for_harness` use the process cwd instead of the
  project → no harness reports the project root; the test fails.
- Making `exit_code_for` always return success → the test fails on exit-code
  propagation.
- Setting `PTY_ALLOCATION_ATTEMPTS` to 1 → the pty retry test fails.

Platform/external evidence:

- **CI run `32788123095` on commit `f3effe6` is green on `ubuntu-latest`,
  `macos-latest`, and `windows-latest`, plus lint.** The Windows job was
  confirmed to have *actually executed* the tests cited here — 186 lib tests
  and 20 PTY smoke tests — rather than only reporting a green tick;
  `the_launch_command_opens_the_configured_harness_inside_the_project_root`
  and `a_fake_installed_harness_launches_inside_the_discovered_project_root`
  both appear as `ok` in the Windows log.
- Local macOS: `cargo fmt --check`, `cargo clippy -D warnings`, unit and PTY
  smoke tests, MSRV 1.85.0 `cargo check --locked`, `git diff --check`, and
  live CLI probes of `glasshouse launch` (help, no-terminal refusal, unknown
  harness, non-harness integration).
- An independent spawn-site inventory, re-run after the merge, found three
  production spawn sites, all project-bound, and zero production callers of
  the generic `TerminalCommand::new`.

What CI caught that local evidence had not:

- `cmd.exe` cannot open the verbatim `\\?\` path that resolving an executable
  produces on Windows, so **no `.cmd`-shimmed harness could start there at
  all** — and npm installs most of them that way. Fixed in `4aa31ad`.
- A prior revision of this entry cited a Windows job that had never run
  `tests/pty_smoke.rs`: when the lib target fails, cargo never reaches the
  integration tests, so the `.cmd` and verbatim-path claims were unproven
  while the entry implied otherwise. Confirming *execution*, not just the
  job's conclusion, is part of this evidence and not a formality.

Remaining caveats (recorded, not blocking):

- Native Windows UNC project roots are still refused rather than supported;
  `cmd.exe` cannot reliably hold a UNC working directory. This is a
  documented limitation of the contract, not a gap in it.


### Phase 2B — Detect cmux when a usable cmux executable or supported cmux control environment is present

Contract: Given a machine where the `cmux` executable is not on `PATH` but
Glasshouse is running inside a cmux surface, when discovery runs, cmux is
reported as detected and configured with the evidence that proves it, while
never being reported as launchable and never recording any environment
variable's value.

State: COMPLETE

Production evidence:

- `integrations::presence_without_executable` reads the cmux control
  environment (`CMUX_SOCKET_PATH`, corroborated by `CMUX_SURFACE_ID` and
  `CMUX_WORKSPACE_ID`).
- `integrations::detect_one_with` consults it in the `ResolveOutcome::NotFound`
  arm only, yielding `IntegrationStatus::Configured` with `executable: None`.
  The `Usable` and `Unusable` arms are untouched.

Regression evidence:

- `cmux_socket_path_set_yields_evidence_naming_it`,
  `cmux_corroborating_variables_are_also_named`,
  `empty_cmux_socket_path_counts_as_unset`, `no_cmux_variables_yields_no_evidence`
  pin the decision, over injected lookups so no test mutates the environment.
- `absent_executable_but_presence_evidence_is_configured_not_launchable` proves
  the wiring, including that `is_usable()` stays **false** — a detected
  integration with no executable must never be mistaken for a launchable one.
- `absent_executable_with_no_presence_evidence_stays_not_found` pins the
  unchanged behaviour for everything else.

Failure/isolation evidence:

- `evidence_notes_never_contain_a_value_only_names` fills every consulted
  variable with sentinels and asserts no note contains one.
  `CMUX_SOCKET_CAPABILITY` is a capability token and is never read at all.
- Non-vacuity (mutation actually run): making the `NotFound` arm ignore the
  presence evidence makes the wiring test fail.

Platform/external evidence:

- Live probe on macOS inside a real cmux surface, with `cmux` removed from
  `PATH`: `glasshouse doctor` reports `cmux [configured]` with the notes
  "candidates tried: cmux", "CMUX_SOCKET_PATH is set", "CMUX_SURFACE_ID is set".
- CI green on `ubuntu-latest`, `macos-latest`, `windows-latest`, and lint. The
  behaviour is environment-variable based with no `cfg` gating, so all three
  platforms execute the same code.

### Phase 2B — Detect Ollama when a usable ollama executable or configured local endpoint is present

Contract: Given a machine with no `ollama` executable on `PATH` but a
configured local endpoint, when discovery runs, Ollama is reported as detected
and configured, while never being reported as launchable and never recording
the endpoint's value — which can carry credentials.

State: COMPLETE

Production evidence:

- `integrations::presence_without_executable` treats a set, non-empty
  `OLLAMA_HOST` as a configured endpoint, wired through the same
  `detect_one_with` seam as cmux above.
- Deliberately no network request: discovery stays non-destructive and adds no
  HTTP dependency. The capability asks whether an endpoint is *configured*, not
  whether a server is answering.

Regression evidence:

- `ollama_host_set_unset_and_empty` pins set, unset, and empty-as-unset.
- The same wiring and non-launchability tests listed above cover Ollama, which
  is the integration they are written against.

Failure/isolation evidence:

- `evidence_notes_never_contain_a_value_only_names` covers `OLLAMA_HOST`.
- Live probe: `glasshouse doctor` with
  `OLLAMA_HOST=http://user:SUPERSECRET@127.0.0.1:11434` reports
  `Ollama [configured]` with the note "OLLAMA_HOST is set", and the whole
  report contains **zero** occurrences of the secret.

Platform/external evidence:

- Same CI run as the cmux entry above; no `cfg` gating, so all three platforms
  execute this code.

Missing evidence:

- None for the stated contract. Whether a configured endpoint is actually
  *reachable* is deliberately out of scope and would need a network probe.

### Phase 2A — Make unsupported platform/harness combinations fail with a clear diagnostic rather than a partial broken session

Contract: Given a platform and harness combination Glasshouse knows cannot
work, when a session or probe would otherwise be started, Glasshouse refuses
before any process exists and says what is wrong and what to do about it,
rather than starting something that appears alive while operating on the wrong
directory or the wrong process namespace.

State: COMPLETE

Production evidence — the combinations Glasshouse knows, and where each is
refused:

- **UNC project root + `.cmd`/`.bat` harness** — `launch::unsupported_combination`,
  called by `HarnessLaunch::build_command` before the command is returned, so
  `spawn` never reaches PTY creation. `cmd.exe` cannot hold a UNC working
  directory and does not fail when asked to: it substitutes the Windows
  directory and runs, so the session would have looked alive while operating
  outside the project entirely.
- **WSL + a Windows-interop executable** — `platform::exec::resolve_with`
  filters `/mnt/c`-style hits and returns `ResolveError::WindowsInteropOnly`,
  whose message explains that the child would run in the Windows process
  namespace where the project's Linux path is meaningless.
- **No usable executable** — `session::select::SelectionError::NotInstalled`
  names the candidate names that were tried.
- **A requested integration that is not a harness** —
  `SelectionError::NotAHarness` names the category and lists the harnesses
  that can be launched.
- **A harness turned off in configuration** — `SelectionError::Disabled`.
- **No terminal to attach to** — `session::attach::attach` refuses rather than
  hanging on a pty query nothing can answer.

Regression evidence:

- `a_script_harness_in_a_unc_project_is_refused_with_a_diagnostic` asserts the
  message names the directory, the reason, and the remedy — not merely that it
  failed.
- `every_other_combination_is_allowed` keeps the refusal narrow: a `Direct`
  harness in a UNC directory, and a script harness in an ordinary local or
  verbatim-drive directory, must all still launch.
- `unc_detection_covers_both_spellings_but_not_a_verbatim_drive` pins that
  `\\?\C:\...` is a local path despite also starting with two backslashes.
- `cmd_and_bat_are_windows_scripts_only_on_windows`,
  `a_nonsense_slug_is_unknown_and_names_the_valid_ones`, `cmux_is_not_a_harness`,
  and `attaching_without_a_terminal_is_refused` cover the other refusals.

Failure/isolation evidence:

- The refusal happens in `build_command`, before `PtyProcess::spawn`, so no
  process, pty, or terminal state exists when it fires. Non-vacuity (mutation
  actually run): disabling the condition makes the refusal test fail.

Platform/external evidence:

- CI green on `ubuntu-latest`, `macos-latest`, `windows-latest`, and lint. The
  check is a function of a path's shape and an executable's kind, with no
  `cfg` gating, so all three platforms execute it.

Honest limits:

- No real UNC share was exercised. The *refusal* is platform-independent code
  that CI runs everywhere; what is taken from documented Windows behaviour,
  not from a live run, is the premise — that `cmd.exe` would substitute the
  Windows directory rather than fail. That premise is why the refusal exists,
  and it was already recorded as a known limitation before this change.
- This capability covers the combinations Glasshouse currently knows about. A
  newly discovered one would reopen it.

### Phase 2A — Support native Windows as a first-class Glasshouse runtime where the selected harness is available

Contract: Given native Windows with an installed harness, everything Glasshouse
can currently do — resolve a project, isolate its state, discover harnesses,
probe versions, and open a real harness session inside the project root — works
the same way it does on macOS and Linux, and any combination that cannot work
is refused rather than half-started.

State: COMPLETE

This is a summary capability. It is checked because every capability it
summarises is checked and because the same test suite that backs the macOS and
Linux boxes now runs, and passes, on `windows-latest` — not because Windows was
judged by a weaker standard than its siblings.

Production evidence — the Windows-specific paths, all reachable in production:

- `pty`: ConPTY through `portable-pty`, with `process::JobHandle` giving
  Windows a kill-the-whole-tree equivalent that `TerminateProcess` alone does
  not provide — which matters precisely because a `.cmd` harness makes the real
  process a grandchild.
- `platform::exec`: `.exe`/`.cmd`/`.bat` classification, `cmd.exe /D /C`
  translation, `plain_script_path` conversion of the verbatim form `cmd.exe`
  cannot open, and rejection of `cmd.exe` metacharacters in arguments.
- `platform::paths::strip_verbatim_prefix` and `Project::display_root`: a
  canonical Windows root is verbatim, which is correct as an identity and
  unusable at a process boundary, so it is stripped there and only there.
- `launch::unsupported_combination`: refuses the one combination known not to
  work rather than starting a session that would run outside the project.

Regression evidence actually executed on `windows-latest`:

- CI run `32790669974` on `53e98f0`: **197 lib tests and 21 PTY smoke tests,
  0 failures**, plus lint, alongside green `ubuntu-latest` and `macos-latest`.
- `the_launch_command_opens_the_configured_harness_inside_the_project_root`
  runs the shipped binary in a real ConPTY and confirms a `.cmd` harness
  starts in the project root, with project-over-user executable precedence and
  exit-code propagation.
- `a_fake_installed_harness_launches_inside_the_discovered_project_root`
  exercises the real `cmd.exe /D /C` translation and asserts the canonical root
  was verbatim before `display_root` stripped it.
- `a_direct_executable_launches_through_the_harness_seam` covers the `.exe`
  branch, whose verbatim path had never been confirmed acceptable to
  `CreateProcess`.
- The PTY smoke suite proves output streaming, input, resize, and exit
  detection against a real ConPTY child.

Failure/isolation evidence:

- Windows CI caught two defects local gates could not: `cmd.exe` refusing the
  verbatim script path (so **no `.cmd` harness could start at all**), and a
  test comparing Windows path spellings that differ while denoting the same
  directory. Both are fixed and covered.
- Project-root refusals, the canonical-path guard, and per-project database
  isolation all execute in the Windows lib suite.

Honest limits — these are true on every platform, not Windows-specific:

- The interactive multi-session TUI does not exist yet (Phase 3), so
  "first-class runtime" means parity with macOS and Linux in what Glasshouse
  can do *today*, which is exactly the standard those two boxes were checked
  against.
- UNC project roots are refused for script harnesses rather than supported.
