# Capability evidence — phase 4

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 4 — the multi-session PTY runtime (covers seven map lines)

Lines: stream PTY output into an in-memory buffer; send text programmatically
without focus; send interrupt signals; bounded scrollback per live session;
keep inactive sessions running; switching changes only presentation focus;
headless presentation mode.

Contract: Given several harnesses started in one Glasshouse, when any of them
is acted on, it runs, buffers its own output within a fixed bound, and accepts
text or an interrupt whether or not it is the one on screen, while changing
which session is on screen never starts, stops, or signals any process.

State: COMPLETE for six of the seven lines; see Missing evidence.

Production evidence:
- `crates/glasshouse/src/shell/mod.rs: run` — **the production consumer.** The
  shell owns a `SessionRuntime`, starts sessions with `n`, forwards keystrokes
  in session mode, forwards resize events to the focused session, polls exits
  on every tick, and renders the focused session's scrollback in the viewport.
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
- CI `32819167010` on `bb4c383` — green on Linux, macOS, Windows and lint, with
  the Windows job confirmed to have executed 267 lib and 31 PTY tests including
  every multi-session test by name. Several concurrent ConPTY sessions with
  independent scrollbacks work, which was the platform risk worth checking.

End-to-end evidence through the shipped binary (tests/pty_smoke.rs):
- `a_keystroke_typed_into_the_shell_reaches_a_real_harness_and_comes_back` —
  `glasshouse` with no arguments, `n` starts a real harness, session mode hands
  it the keyboard, the typed bytes arrive and its reply is drained into the
  scrollback and drawn. The payload begins with `q` on purpose: in session mode
  that belongs to the harness, so a broken mode split would quit instead.
  Mutations caught: swallowing the bytes before `write_to_focused`; never
  refreshing the viewport from the scrollback.
- `resizing_the_shell_reaches_the_harness_terminal` (Unix) — asks the harness
  `stty size`, resizes Glasshouse's own terminal, asks again. Proves the chain
  from Crossterm's resize event to the child's pseudo-terminal is joined up,
  which nothing previously did.

**A real defect this found, that unit tests could not.** The session-mode
escape chord was implemented as `Ctrl` + `']'`, matching the synthetic
`KeyEvent` its unit tests constructed. Crossterm's Unix parser decodes the
control range `0x1C..=0x1F` arithmetically, so a real terminal's `Ctrl-]`
arrives as `Ctrl` + `'5'` and never matched — leaving the user in session mode
**with no way back**, which is precisely the failure the single-chord escape
exists to prevent. `is_session_escape` now accepts both spellings, with the
Windows path (virtual key codes, `']'`) and the Unix path (`'5'`) documented
and separately tested. Reverting to the single spelling fails the resize test.

Missing evidence:
- Three of the twelve Phase 4 lines stay unchecked, all for the same reason —
  no production caller: sending text to an **unfocused** session and sending an
  **interrupt** are both orchestrator operations (Phase 14), and nothing yet
  creates a **headless** session, because the shell always starts sessions
  embedded. The runtime supports all three and each is tested against real
  processes; what is missing is a caller, not a mechanism.
- The shell's own multi-session switching has no end-to-end test. One was
  written and removed: a full-screen Ratatui application repaints
  differentially, so a captured pseudo-terminal stream cannot be sliced back
  into frames without a real terminal emulator, and an assertion about "the
  viewport" silently reads every viewport ever drawn. Phase 5 needs an emulator
  anyway; that is when this becomes testable. Meanwhile the behaviour is proven
  at the runtime layer against real processes
  (`focus_changes_nothing_but_focus` records pids across five switches), and
  the shell's only route to switching is through that layer.

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
