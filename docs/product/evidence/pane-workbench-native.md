# Native workbench cutover — evidence, 2026-09-20

**State (2026-09-21): the presentation follows the mockup, and the whole pane
suite is green on macOS except one environmental red.** The cutover's own state
line, kept below, was accurate when it was written; what changed since is in
*Mockup pass and regression migration* at the end of this file.

**State at the cutover: implemented, targeted native checks pass; broader/platform
acceptance is not complete.** Base: `17904082d91a08b3d3da585f606ec62965c4cd25`. This is not a claim
that the browser mockup or the whole workspace has been fully implemented/verified.
The current contract is [workbench.md](../pane/workbench.md).

## Production path

The ordinary session renderer/event path now uses `workbench`, not the legacy TUI.
The document consumes real Conversation/Notebook/CellView/HelperRecord data. The
settings editor saves through Store; model changes use the existing session
control channel. `RuntimeState::agent_assignment` enforces explicit pinned/roster
model and roster effort before `agent.run` creates a background job. UI filtering
alone is not the policy. Existing approval/question/credential owners retain
precedence. The live model controls cannot activate unrelated saved-for-restart
configuration; the native settings boundary is exercised explicitly.

## Reproduction and observed results

Rust 1.98.0, Linux x86_64 root container, locked vendored dependencies and the
repository-pinned V8 library. No paid/model subscription calls. PTY tests use a
loopback scripted provider and spawn the real `pane` executable.

| Command / target | Observed |
|---|---|
| `cargo check -p pane --tests --locked` | pass |
| `cargo test -p pane --test workbench --test delegation_policy --test custom_agents --test settings_store --test config --locked` | 106 pass: 40 + 9 + 6 + 33 + 18 |
| `cargo test -p pane --test tui_live workbench_ --locked -- --test-threads=1` | 3 pass; 27 other legacy PTY tests not included |
| `cargo test -p pane --lib session::controls --locked` | 11 pass |
| `cargo test -p pane --test runtime_cells subagent --locked` | 4 pass |
| `cargo test -p pane --test tools the_agent_declaration_names_the_models_this_session_can_delegate_to --locked -- --exact` | 1 pass |
| `cargo fmt --all -- --check` / `git diff --check` | pass |
| `python3 scripts/check-file-sizes.py` | pass; the three pre-existing over-limit files did not grow |
| `cargo test -p pane --lib --locked` | 660 pass, 4 fail; same four failures reproduced on unchanged base (656 pass, 4 fail) |
| `cargo clippy -p pane --lib --tests --locked -- -D warnings` | not green; same baseline six production / ten including library-test findings reproduced; new module findings fixed |

The four reproduced library failures are
`session::tests::a_direct_provider_call_runs_and_answers_with_a_typed_result`,
`session::tests::independent_direct_calls_answer_individually_from_one_frame`,
`tools::invoke::search::tests::finding_5_an_unreadable_directory_keeps_the_matches_beside_the_error`,
and `tools::invoke::tests::a_bypassed_child_reads_what_the_sandbox_would_refuse`.
The host cannot provide the tested Landlock/seccomp boundary; root also bypasses
the unreadable-directory fixture. No sandbox code was weakened to make these pass.

A broader integration run was also attempted and was **not green**. Some fixtures
asserted superseded default inheritance, model overrides or old panel strings;
others require unavailable sandbox/build environment behavior. The explicit
custom-agent and selected runtime/declaration fixtures above were migrated and
retested. Remaining old UI and broader session fixtures still need review; they
are not all being attributed to the environment without a baseline comparison.

## Decisive mutation

Removed the roster's explicit model-versus-selected-slot mismatch check. The
native `roster_resolves_only_configured_slots_and_exact_models` test failed with
an unauthorized assignment, exit 101. Restored the original bytes, then ran all
nine delegation tests successfully. The actual V8 tests additionally assert
that forbidden model/effort choices leave the background-job count at zero.

## Boundaries still open

Exact provider/account/entitlement pinning and an enforced subscription-only or
no-API-spend fallback are **not** implemented. Catalogue account rows are labeled
availability evidence; routing stays gateway-owned. Favorites currently bind
model and effort. Side-by-side and working-tree/HEAD diffs are not implemented;
the native diff is a cell's captured before/after unified diff. The final answer
is the real model return, not a new answer-quality evaluator. Native Ghostty,
macOS and Windows acceptance and the remaining legacy regression migration are
required before treating this cutover as merge-ready. Existing legacy renderer
code is retained for DTO/test compatibility, not selected by the live UI.


## Mockup pass and regression migration — 2026-09-21

`docs/product/pane/pane-workbench.html` is committed beside this contract: it is
the design the workbench is drawn from, and it is what a later change is
measured against.

**What the cutover had dropped, and is back.** `/theme`, `/motion`, `/sidebar`,
`/statusline <value>` and `/fullscreen` are answered by the workbench itself
(`workbench/input.rs::local_command`) instead of reaching the model as unknown
commands; the telemetry instruments are the surface they always were, drawn by
`tui::telemetry::expanded` into the transcript's room so the status line they
are read against stays on screen; and the status line carries the sandbox
posture, the confinement word, the effort and the measured context reading
again, in that priority order, so a 60-column terminal keeps the boundaries and
loses the helper count instead. **Local notices are drawn where they happened**,
marked and muted, and the Activity surface still lists them — before this, a
presentation command that worked said nothing at all.

**What the presentation changed, and which tests moved with it.** The session
bar, the palette, the cell's header, tab strip, working mark and closing line,
the diff's two line-number gutters, the navigator's account column, count and
lock reason, and the shared surface head and foot are all the mockup's.
`bare_statusline_selector…` no longer stages a preview to cancel, because a
completed choice saves itself; scope is switched with F6 while Tab walks the
categories; Shift-Tab opens **Ask** and the rung is chosen there; the navigator
excludes unavailable accounts until F6 asks for every source, so its counts read
`307/308` and then `308/308`.

**Two probes were unsound rather than outdated, and both were hiding a race.** A
cell shows its own source now, so `contains("denied.txt")` matched the program
that was going to write it and answered an approval prompt that had not yet
appeared; `contains("LIVE RESULT INTACT")` matched the `answer()` call rather
than the answer, so `/exit` was typed into a running turn and queued as a
message. They now wait for the file to exist and for a line of its own
(`App::wait_for_file`, `App::contains_line`).

| Command / target | Observed |
|---|---|
| `cargo test -p pane` | 100 targets ok; `tui_live` 30/30 |
| `cargo test -p pane --test workbench` | 42 passed, 1 ignored (the `screenshot` dev aid) |
| `cargo test -p glasshouse --test pane_launch` | 1 passed |
| the targeted blast radius over all 32 changed files | every target green except the red below |
| `cargo clippy -p pane --lib --tests` | no finding in the changed files; the baseline findings are unchanged |
| `cargo fmt --all --check`, rustdoc, `check-file-sizes.py` | clean |

**The one red.**
`runtime_cells::a_grep_line_that_is_not_a_match_has_no_line_number` fails
identically on `main` in this checkout and is environmental (the host's ripgrep
on a binary hit), not this work.

**Still open.** The stale rows the navigator seemed to leave under a shrinking
filter were a harness effect — `contains` stops pumping the moment it is
satisfied, so an absence must be asserted after `settle` — and not a renderer
defect; it is recorded because two attempts to "fix" it in the renderer were
wrong, and the second one (clearing the terminal whenever the drawn shape
changed) turned 29 green PTY tests red. Native Ghostty, macOS and Windows
acceptance runs are still owed, and the GitHub sweep is the platform evidence.

## The application pass — instant updates, plain words, 2026-09-22

The contract: **a setting is in force in the session you are sitting in, every
control says what it does to your work, and the things you always use are always
on screen.** `docs/product/pane/workbench-ux.md` is the roadmap and carries the
ten principles and their sources; this entry is what changed and what watches it.

**The defect it started from.** 66 of the 71 keys in `settings/registry.rs`
carried `restart: true`, and `workbench/settings.rs::save` applied four of them
(`ui.*`) to the running session. Everything else printed *"Saved for a new
session"* — while the same choice typed as `/effort high` two lines lower
applied instantly, because `session/controls.rs::command` had answered `/effort`,
`/mode`, `/permissions` and `/model` against the live session all along. The
panel had no caller for them. `settings::live_command` is that caller, the
reducer drains it as an `Effect::Command`, and the six keys that can move
mid-session now do. `permissions.mode` was additionally declared `restart:
false` and applied by nothing at all.

**What stopped naming mechanisms.** `sandbox 3p/1c YOLO unconfined · net:off` —
a path-rule count, a command-pattern count, a joke and an internal applier's
name — is one named boundary with one sentence of consequence, and the counts
moved to the Access surface underneath it. `Rung::label` and `Rung::sentence`
give the status bar, the Ask surface, the Shift-Tab notice and the settings row
one vocabulary; there used to be three. The settings foot named a dotted key and
a three-valued provenance and now names the layer that won, in words.

**Two safety changes fell out of it.** Shift-Tab was dead — the workbench
consumed `BackTab` and opened a surface, so `ui.rs`'s live cycler never ran and
choosing a rung took four keystrokes through a surface (which is what made
`slash_mode_walks_…` flaky-pass twice). It cycles in place now, and it **steps
over `full`**: one unconfirmed keystroke from the default rung to the rung that
never asks again is a serious error to prevent structurally, so that rung keeps
the two routes that explain themselves first. And a lifted boundary is now
exempt from the session bar's narrowing rule — the old rule dropped it first, so
an 80-column window running with full access looked ordinary.

**One regression I introduced and the tests caught inside a minute.** The first
attempt at removing the panel's `unset` rows put the runtime defaults into the
*effective configuration*. `a_project_document_cannot_turn_off_confinement…` and
`native_permissions_render…` both went red: an absent `permissions.full_access`
and a present `false` are the same to a reader and very different to the loader,
which drops a project document's copy of that key precisely by noticing it is
there — and an injected empty `modes.explore.writable` would have replaced the
built-in writable path rather than inherited it. `settings::shown_default` is
the display-only answer, and its doc comment carries the trap.

| Command / target | Observed |
|---|---|
| `cargo test -p pane` | every target green but the environmental red below |
| `cargo test -p pane --test tui_live` | 31 passed (was 30; one added) |
| `cargo test -p pane --test workbench` | 43 passed, 1 ignored (the `screenshot` dev aid) |
| `cargo test -p pane --test tui_look` | 60 passed |
| `cargo test -p glasshouse --test pane_launch` | 1 passed |
| the targeted blast radius over all 15 changed files | every traced target passed |
| `cargo clippy -p pane --lib --tests` | no finding in a changed file; the baseline is unchanged |
| `cargo fmt --all --check`, rustdoc, `check-file-sizes.py` | clean |

**Tests that now watch the claims.**
`a_setting_chosen_on_the_panel_is_in_force_in_this_session` walks a real PTY:
effort is stepped on the panel and the status strip — which reads the live
session, never the file — has to agree, and the file has to agree too.
`a_lifted_boundary_is_never_the_control_a_narrow_terminal_drops` checks 60, 80,
100 and 140 columns. `saved_permissions_do_not_change_running_authority` now
states the real invariant it was named for: a *grant* never reaches a running
session and is absent from `live_command`; a *rung* does, by handing the loop the
command, never by writing the ladder behind the session's back. The slash menu
gained a no-duplicates assertion, which immediately found a second one: `/exit`
had been listed both as a built-in and as a literal. `App::refute` settles before
asserting an absence, so the trap that made two earlier probes unsound is paid
for once.

**Discovery defects fixed on the way.** `/login` existed as a variant, was
answered by the key handler, and was in no list — the same defect `/exit` had.
`/tool` and `/mouse` likewise. `/config` was listed twice with two different
descriptions. Bare `/statusline` opened the panel with the status line nowhere in
sight. `limits.task_tokens` is labelled retired, does nothing, and was still
offered as a choice.

**The one red.** `runtime_cells::a_grep_line_that_is_not_a_match_has_no_line_number`
fails identically on `main` in this checkout and is the host's ripgrep on a
binary hit, not this work.

**Still owed.** Native Ghostty and Windows acceptance; the GitHub sweep is the
platform evidence. The status strip's helper and subagent facts are set when the
session starts and are not re-read if the file changes underneath it — correct
today, because neither key can move mid-session, and a line to revisit if one
ever can.

### The follow-ups, and the CI attribution

Four commits after the main one, each its own small thing:

- **A picker marks the option the session is on.** Both pickers highlighted the
  row under the cursor and nothing else, so opening Work or Ask to check what
  the session was *doing* told you only where the cursor had stopped. The mark
  is a glyph and the word `now`, never a colour alone. The Work surface's prose
  ("within the session's grants", "configured write exceptions still apply")
  became what each mode does to your files.
- **The instant budget is measured, not asserted.** Every arrow press on a
  Choice row re-opens the store, re-reads the target file for optimistic
  concurrency, re-validates the whole effective configuration through the
  parser a session start uses, writes atomically and reloads
  global-then-project. That is correct and had never been timed: **15 ms** on
  this machine. `a_settings_keystroke_stays_inside_the_instant_budget` puts a
  deliberately loose 50 ms ceiling on it — a regression catcher, not a
  benchmark, and loose so a loaded runner cannot make it flaky.
- **Why the three model arms of `live_command` exist.** A model row opens the
  navigator, and the navigator already hands the loop the same `/model …` line
  the map would produce, so those arms never fire. They are there so
  `applies_now` and what actually happens cannot give one row two answers.
- **One command's probe got its own session.** See below.

**CI, run 35661907481 (`3af277ff`).** `lint`, `audit` and every `test` cell
green. Both `pane` cells red:

- `a_broad_grep_skips_what_the_project_says_it_generates` — still red alone on
  ubuntu **and** macOS, and red in `main`'s baseline before this work. Not this
  change.
- `slash_mode_walks_into_a_plan_mode…` — **mine**, macOS only. I had appended a
  `/permissions full` probe to the end of that test, after a provider turn,
  three mode changes and two panels, where nothing was waiting for anything in
  particular; the command was typed into whatever was still settling. It is now
  `the_rung_that_stops_asking_is_reachable_by_typing_it_in_full`, a short test
  in a session doing nothing else, and it checks what the long one could not:
  that the session bar then reads `Never asks`, the same word it uses
  everywhere else. Ran three times locally, green each time.

The lesson is the one this file already carries twice: **a probe appended to a
long test inherits none of that test's waiting.** The new `App::refute` is the
other half of it — it settles before asserting an absence, so the trap that
made two earlier probes unsound is paid for once, in one place.

### The sweep that settled it — run 35663971828 (`0dca0005`)

**14 of 17 cells green**: `lint`, `audit`, and all twelve `test` cells across
five OS/arch targets, the declared compiler, MSRV, beta and nightly. The three
`pane` cells are red, and each red is attributed:

| Cell | Still red | Attribution |
|---|---|---|
| `pane (ubuntu-latest)` | 1 | `a_broad_grep_skips_what_the_project_says_it_generates` — red alone, and red in `main`'s baseline before this work |
| `pane (macos-latest)` | 1 | the same one. Two flaky-passes beside it (`approval_resumes_remaining_compute_budget…`, `live_a_second_escape…`), both load-sensitive and neither touched here |
| `pane (windows-latest)` | 45 | baseline 43. **The set was diffed, not counted** |

That diff is the only honest way to read the Windows cell, because its
`tui_live` target is wholly red there and a count moves whenever a flaky test
changes sides. Against the last baseline run (`3458c3cd`, job 106511610710) the
new reds are exactly two names:

    a_setting_chosen_on_the_panel_is_in_force_in_this_session
    the_rung_that_stops_asking_is_reachable_by_typing_it_in_full

— the two PTY tests this work adds, landing inside a target where every other
test on that cell was already red. Nothing that was green on Windows went red.
`slash_mode_walks_into_a_plan_mode…`, the one red that *was* mine two runs
earlier, is green.

Locally, `cargo test -p pane` is green on macOS but for the same environmental
grep case; `ruler_run`'s two names went red once under a full-suite run beside
a release build and a CI watcher, and are 46/46 twice when run alone — load, and
in a subsystem this work does not touch.

## The Windows pass — every red cell attributed, and six of them were defects

**Contract: `pane` is green on all three CI cells, and every red it had is
either fixed or named.** The previous section closed with two `pane` cells red
on one environmental case and a Windows cell "wholly red" — read as background
noise for as long as nobody diffed it. Diffing it found **six product defects
and three untrue tests**, not a platform that happens not to work.

### The one that mattered: Enter never sent on Windows

`session/ui.rs` rewrites a plain Enter into `Alt`+Enter — a newline rather than
a send — when more input is already behind it. That rule is right, and it was
added on 2026-09-19 for a real failure: a multi-line prompt typed into a pty
without bracketed-paste markers used to send its first line as the whole task
and keep the rest as a draft nobody was told about.

It asked the wrong question. It asked the *terminal* whether another record was
waiting, and on Windows crossterm emits a `KeyEventKind::Release` record after
every press. So the poll taken the instant Enter was read was answered by
**Enter's own release**, every Enter became a newline, and nothing a Windows
user typed was ever sent. That is the whole `tui_live` target — twenty-nine
tests — plus the four `approval_boundary` tests, which never see an approval
because the message that would cause one is still sitting in the composer.

The screen dump in the CI log says it in one line: the composer reads
`❯ fail this` and the transcript is empty.

The fix is in `session/ui/terminal_input.rs`, where the Windows console's
shape was already understood:

- `accept` now drops a key release **outright** rather than only inside an open
  report run. Nothing downstream reads one — `ui.rs`, `workbench::input` and
  `settings_ui::key` each discard it on arrival — so what a queued release did
  was answer questions asked of the queue.
- `typing_waiting` is the question `ui.rs` meant: it drains what the terminal
  already holds into the resolved queue, where releases are dropped and report
  runs reassembled, and reports what survived.

`a_key_release_is_never_delivered` is the killing test, and it runs on every
host because the resolver is not platform-specific. Mutation: restore the
narrow `&& matches!(self.hold, Hold::Open { .. } | Hold::Pasting { .. })`
guard — **KILLED**, `left` holding four events where `right` holds two.

### The rest

| Defect | Where | What was wrong |
|---|---|---|
| The permission ladder could not judge a single command | `sandbox/modes.rs` | a blanket `cfg!(windows)` bail-out meant `git status --short` asked for confirmation forever and a person's own `[modes] commands` list was never read. It now screens for the ten characters `cmd.exe` spells differently and hands everything else to the same reader POSIX uses — additively, so Windows is never more permissive |
| `grep` was a different language on Windows | `tools/invoke/search.rs` | the in-process engine was POSIX **BRE** while the product promises the extended dialect everywhere else (`-E` on the spawned form, `rg` where it is installed), so `a\|b` and `need+le` matched nothing |
| `grep` read what the project calls generated | `tools/invoke.rs` | `ignored_directories` was computed for a broad search and applied only in the spawned arm; the in-process walk pruned `.git` and nothing else |
| `doctor` ignored a measurement it already had | `cli_workflows.rs` | the Windows arm returned `warning` unconditionally, while `sandbox::windows::network_isolation()` — which queries the Firewall service — sat unused beside it. A fully confined machine was told to go and check |
| A sentence's full stop stayed in a clickable path | `tui/paths.rs` | Win32 discards a trailing dot, so `src/tui.rs.` *exists* there and the untrimmed spelling was tried first |
| The VM runner sent the gate at a dead lease | `scripts/dev/glasshouse-windows-ci` | two leases for `glasshouse-ci`, `tail -n 1` picked the expired one, and the runner said "not reachable, start it in VMware Fusion" about a VM that was up and answering. It now asks each lease in turn |

And three tests that were not true:

- `a_broad_grep_skips_what_the_project_says_it_generates` asserted ripgrep's
  guarantee of whichever backend served the call. The developer machine has
  ripgrep and the runners do not, so it only ever passed here — and the
  fallback's own doc comment two tests below says in as many words that
  `--exclude-dir` finds *more* than this. It now states the guarantee the
  backend actually makes and names which one ran.
- `prompt_write_grants_do_not_include_denied_path_components` compared a sorted
  list as an ordered one. The producer sorts for determinism and the sentence it
  feeds is a join; on Windows `\\?\C:\…` sorts after `Write(src/**)` and on
  POSIX `/…` sorts before it. Membership was the contract.
- `the_event_stream_announces_a_cell_before_it_runs` interpolated a raw path
  into a script line, and `cmd.exe` ate the backslashes. Its assertion is now an
  exact match against a per-platform command rather than a `starts_with`.

### `a_settings_keystroke_stays_inside_the_instant_budget` judged a mean

It averaged five passes against a 50 ms ceiling, on a machine that had measured
15 ms. Run 35665385746 descheduled one of the five and the test went red. A
shared runner stealing 200 ms is not a fact about the code, so it now takes the
**fastest of nine** against a loose 250 ms — a ceiling that still catches the
regression class it was written for, a blocking call in the save path, because
that makes every pass slow. Measured after the change: 9.7 ms.
