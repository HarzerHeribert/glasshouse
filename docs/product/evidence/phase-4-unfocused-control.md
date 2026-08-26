# Capability evidence — phase 4-unfocused-control

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 4 unfocused control — "Support sending text programmatically to a PTY session without requiring the user to focus it."

Contract: Given several live sessions of which one is on screen, when the user
moves the overview's cursor to a session the viewport is not showing and sends
it a line, Glasshouse delivers that line to that session's pseudo-terminal,
while preserving which session the viewport and the session bar are presenting.

State: COMPLETE

Production evidence:
- `shell/state.rs: OverviewState::cursor` — a second, independent cursor. This
  is what makes the line real rather than nominal: before it, the overview
  highlighted `ShellState::selected_index()`, the same index `shell::sync_focus`
  hands to `SessionRuntime::focus`, so "the selected session" *was* the focused
  session by construction and an unfocused send was not expressible.
- `shell/mod.rs: send_session_text` — `Action::SendSessionText { id, text }`
  writes `"{text}\r"` through `SessionRuntime::send_text`. The bare `\r` is what
  a real Enter delivers and what `state::encode` already sends, so the harness
  cannot distinguish this line from a typed one.

Regression evidence:
- 16 new `shell::state::overview_tests` and 6 new `shell::view::tests` — cursor
  independence, that the bar keeps presenting what it was presenting, and that
  Tab still moves it underneath the popup. Executed on macOS.

Failure/isolation evidence:
- The focus-does-not-change assertions are the negative half: they fail if the
  send is implemented on the shared selection index.

Platform/external evidence:
- CI run `32957790931` on commit `9d9483b`: **all seven jobs green** — `lint`,
  `msrv` and `test` on each of `ubuntu-latest`, `macos-latest` and
  `windows-latest`.
- `an_unfocused_session_still_receives_sent_text` is a plain `#[test]` with no
  platform gate, so it executed on all three. The `overview_tests` module is
  pure state and likewise runs everywhere.

Missing evidence:
- None.

### Phase 4 unfocused control — "Support sending interrupt signals to a PTY session."

Contract: Given a live session the viewport is not showing, when the user
interrupts it from the overview, Glasshouse delivers an interrupt to that
session's process group, while preserving `Ctrl-C`'s existing meaning of
quitting Glasshouse itself.

State: LOCALLY VERIFIED

Production evidence:
- `shell/mod.rs: interrupt_session` — `Action::InterruptSession(id)` →
  `SessionRuntime::interrupt`, bound to `c` in the overview. Deliberately not
  `Ctrl-C`: stealing it would leave a user unable to exit.

Regression evidence:
- Two new interrupt tests, **`#[cfg(unix)]`**, executed on macOS.
- An explicit test that `Ctrl-C` still quits Glasshouse.

Failure/isolation evidence:
- `PtyProcess::interrupt` writes `ETX` (`0x03`) into the pseudo-terminal and
  relies on the Unix line discipline — or ConPTY's
  `PSEUDOCONSOLE_WIN32_INPUT_MODE` — to turn it into a process-group interrupt.
  Nothing added here is platform-specific.

Platform/external evidence:
- Pending. **The new interrupt tests are Unix-gated and will not execute on
  `windows-latest`**, so a green Windows run is not evidence for this line.

Missing evidence:
- A Windows-executed interrupt assertion. Until one exists this box stays open,
  because the product invariant is that PTY lifecycle is correct on every
  claimed platform, and ConPTY's path here has never been run.

### Phase 4 unfocused control — "Add a headless presentation mode in which a PTY continues running without occupying the visible session viewport."

Contract: Given a harness the user wants running but not drawn, when they start
it headless, Glasshouse runs it to completion and propagates its exit status,
while preserving the terminal for whatever else owns it and never orphaning the
child on a forced exit.

State: COMPLETE

Production evidence:
- `main.rs: run_headless` — `glasshouse launch <harness> --headless`, recording
  `SessionPresentation::Headless` and never claiming the terminal.
- `shell/mod.rs` — `N` starts one from the shell.
- `main.rs` — a `shutdown::on_forced_exit` registration bound to a named guard
  (`let _forced_exit`, not `_`), closing the session under `try_lock` per that
  module's non-blocking rule.

Regression evidence:
- 7 new `pty_smoke` tests against a real pseudo-terminal, executed on macOS.

Failure/isolation evidence:
- **A real defect, found by running the shipped binary against Claude Code
  2.1.246 and caught by no test:** `shutdown::install_signal_handler`
  force-exits when the terminal is not engaged, and a forced exit runs no
  destructor. A headless launch is the first path that both owns a PTY child
  and engages no terminal, so the real harness survived and was left running.
- The test that now covers it was itself defective and is the more useful
  finding: its fake harness was a plain `/bin/sh`, and a process exiting closes
  the pty master, so the kernel hung the shell up whether or not Glasshouse did
  anything. `trap '' HUP` makes the fake model what Glasshouse actually runs.
  Proved in both directions — hardened test on clean code `ok. 1 passed`;
  hardened test with the cleanup removed `FAILED`, "the harness (pid 61125)
  outlived the Glasshouse that started it".
- `shutdown.rs`'s registry is `Mutex<Vec<Cleanup>>` with ids from
  `NEXT_CLEANUP_ID`, `ForcedExitGuard::drop` doing
  `retain(|(id, _)| *id != self.id)`, and `run_forced_exit_cleanup` iterating
  `.rev()` under `try_lock` with each callback in `catch_unwind`. The
  single-slot hazard recorded in earlier handoffs is closed; this is its second
  caller and it is safe by construction. Note that a headless launch and an
  attached session cannot both be live in one process today, so two concurrent
  callers are not yet exercised.
- **A second defect, found by CI and fixed in `close_before_forced_exit`.** The
  first fix took a single `try_lock` on the runtime, and the headless poll loop
  takes that same lock every 20ms — so cleanup was a coin flip with no retry
  anywhere above it, and losing it orphaned the harness permanently. It
  surfaced as an intermittent red `test (macos-latest)` on `3ec4973` that
  passed on rerun against the identical commit, and was then **reproduced
  locally at 1 orphan in 100 runs under 3x CPU load**. `close` sends
  `ProcessSignal::Kill`, which is how the competing theory — the fake harness's
  `trap '' HUP` letting it survive — was eliminated by reading rather than
  guessing.
- The repair is a bounded retry, and it is covered by a **deterministic**
  regression rather than the 1-in-100 one: `hold_lock_for` takes the lock on
  purpose, so `a_forced_exit_cleanup_waits_out_a_briefly_held_lock` fails every
  time without the fix. Proved in both directions — the one-shot mutation kills
  it with "the cleanup gave up while the lock was merely busy".
  `a_forced_exit_cleanup_gives_up_rather_than_hanging` asserts the bound is
  honoured, because a forced exit that will not exit is the worse failure, and
  `a_single_attempt_loses_the_race_that_the_bound_wins` pins the pre-fix
  behaviour so it cannot quietly return.

Platform/external evidence:
- CI run `32957790931` on commit `9d9483b`: **all seven jobs green** — `lint`,
  `msrv` and `test` on each of `ubuntu-latest`, `macos-latest` and
  `windows-latest`.
- `a_headless_launch_runs_the_harness_without_taking_the_terminal` and
  `a_session_started_headless_runs_and_is_listed_but_never_reaches_the_viewport`
  are plain `#[test]`s with no platform gate: the behaviour this line actually
  claims executed on Windows, Linux and macOS.

Missing evidence:
- None for the claimed behaviour.

Known limit, recorded rather than hidden:
- `interrupting_a_headless_launch_does_not_leave_the_harness_behind` — the
  regression for the orphan defect — is `#[cfg(unix)]`. The forced-exit path
  exists on Windows and is unproven there. That is a robustness property rather
  than the text of this line, so the box is checked and the gap is named here.
