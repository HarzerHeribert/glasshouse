# Glasshouse implementation handoff

Last updated: 2026-08-25 (Europe/Berlin)

## Current capability / phase

**Phase 2 (persistent project state) is complete, and Phase 3's TUI shell is
substantially complete.** Sixteen capabilities were closed this session; the
checked count went from 63 to 79.

Phase 2 — all six remaining boxes:

1. Persist session metadata independently of native harness files.
2. Persist the Glasshouse-ID to native-session-ID mapping.
3. Persist harness, times, role, lifecycle, and project identifier.
4. Persist the process presentation mode.
5. Persist enough to distinguish active, resumable, closed, and failed.
6. Never store provider credentials in the project database.

Phase 3 — nine of twelve, plus Phase 1's last renderable gap (line 93, the
active canonical project root in the TUI). The three left open are blocked, not
skipped: the project-memory view needs Phase 20, and both "return to the active
native session" and "propagate resize to the embedded terminal" need a live
embedded terminal, which is Phase 5.

### The decision this session made, and why

The previous checkpoint ended on a deliberate choice between three blocked
groups. **Phase 2 was chosen first** because it was the only one unblocked:
2C onboarding needs product decisions from the user, 2D settings needs a TUI,
and Phase 3 would be cheaper once a session model existed. Phase 2 also retired
the schema half of Phase 1 line 90.

**Phase 3 followed** for exactly the reason recorded when Phase 2 closed: it was
then the largest unblocked item, and `session::store` had given it something
real to render.

Phase 2 was Red (persistence, migrations, resume identity, secret boundaries)
and was the orchestrator's own work. Phase 3's state and view are Amber; the
terminal ownership it builds on already existed.

## Verified completed work

### This session — the TUI shell

- `glasshouse` with no arguments opens the shell; piped or redirected runs keep
  the plain summary rather than drawing a full-screen interface into a file.
- Split like the first-run wizard: `shell::state` answers keys without drawing,
  `shell::view` draws without deciding anything. That is what makes the
  interesting behaviour testable without a terminal.
- The session bar renders the records `session::store` keeps, so Phase 3 reads
  what Phase 2 wrote — the two halves of this session meet in production, not
  only in tests.
- The overview draws *over* the shell rather than replacing it, so it reads as
  somewhere you leave rather than somewhere you go. Escape leaves the overlay
  while one is open and leaves Glasshouse only when none is.
- Selection follows a session's identifier, not its index. Sessions sort by
  last activity, so a refresh reorders them, and holding an index would move
  the user to a different session behind their back.
- The status bar carries the key bindings plus a note when a key could not do
  anything — pressing Tab in a one-session project explains itself instead of
  looking like a dead keyboard.

**Mutation testing rejected a piece of this code, which is the point of it.**
The status bar originally measured the remaining width and truncated a note to
fit. Removing that measurement changed nothing on screen, because Ratatui
already clips the row. The measurement is gone; the property that matters —
bindings are needed permanently, a note only once — is now carried by writing
the bindings first and letting the clip fall where it should, and swapping the
order fails the test.

**It also exposed two vacuous assertions, both the same mistake.** The
real-terminal check for the project root survived having the root blanked out,
because the project's name and its root's last component are the same string
and a bare `contains` matched the title bar. The same flaw let the
narrow-terminal test pass while truncating from the wrong end. Both now read a
single specific row or field. The lesson generalises: **asserting against a
whole screen is nearly always weaker than it looks.**

The text-first constraint is enforced mechanically rather than by assertion:
Ratatui's decorative widgets all draw with Unicode block elements, so the test
fails on any character in U+2580..U+259F, and adding a sparkline-looking line to
the viewport fails it.

### This session — the session store

- `session::store` is Glasshouse's own record of the sessions in a project,
  deliberately not a view over any harness's session files. `native_session_id`
  is a nullable *reference*, so a record is complete before a harness has
  produced an identifier and stays valid after the harness's history is gone.
- **Project isolation is structural, not a query filter.** Migration 2 adds
  `BEFORE INSERT` and `BEFORE UPDATE OF project_id` triggers that abort any row
  whose `project_id` is not the identifier bound in `project_metadata`. No
  present or future query has to remember to filter. The comparison uses
  `IS NOT` rather than `<>` so that a missing binding aborts instead of
  evaluating to NULL and passing — mutation-proven, not merely argued.
- `SessionRecord::disposition` derives active/resumable/closed/failed from
  lifecycle plus the presence of a native identifier, rather than storing a
  second column that could disagree with the first. A stopped session with no
  native identifier reads as closed, because offering a resume with nothing to
  resume to would produce a blank session wearing an old session's name.
- `glasshouse launch` now records what it starts, moving the session through
  `Starting` -> `Running` -> `Stopped`/`Failed`, and `glasshouse sessions`
  reads it back. Creating the record is fatal if it fails; every later state
  change is best effort, because once a harness is running, Glasshouse's
  bookkeeping is not worth failing the user's session over.
- The schema has nowhere to put a provider credential, and
  `the_project_database_schema_has_nowhere_to_put_a_credential` pins the exact
  `(table, column)` list so any future addition fails until someone reviews it.
  An allowlist, not a name pattern: `project_metadata.key` would false-positive
  on any name match, and a credential column could just as easily be `value`.

Two defects were caught by running the thing rather than reading it:

- Every `Display` impl used `Formatter::write_str`, which **silently ignores
  width and alignment**, so the session listing's columns were ragged. Fixed
  with `Formatter::pad` and pinned by a test.
- `too_new_schema_is_rejected_and_not_recreated` set *every* migration row to
  99, which worked with one migration and violated the primary key with two.
  The fixture now appends a row, which is also what a newer build would
  actually leave behind.

One documented claim turned out to be **wrong and was corrected**: the unique
index's `WHERE native_session_id IS NOT NULL` clause was justified as
preventing collisions between sessions with no identifier yet. It does not —
SQLite already treats NULLs as distinct in a unique index. The mutation that
should have failed passed, which is how it was caught. The clause is kept for
index size and intent, and the comment now says so; the real hazard it guards
against is a future `NOT NULL DEFAULT ''` refactor, which is now its own
mutation check.

- `glasshouse launch [harness] [-- args]` is the first production consumer of
  `HarnessLaunch`. Until now the Phase 1 promise rested on a mechanism no
  shipped code exercised.
- `session::select` resolves exactly one harness and one executable, preferring
  a project-level configured path over a user-level one and an explicit path
  over PATH discovery. It refuses ambiguity rather than guessing, and a
  configured path that will not resolve is an error, never a silent fallback to
  a different binary.
- `session::attach` is a transparent bridge, not a renderer. That is what makes
  ConPTY's startup handshake work with no terminal emulation in Glasshouse: the
  cursor-position query reaches the user's real terminal, which answers it as
  it would for any program. Nothing in Glasshouse may answer it as well, or the
  harness receives the reply twice, as input.
- `shutdown::RawModeGuard` takes raw mode without the alternate screen, which
  is what routes Ctrl-C to the harness instead of to Glasshouse.
- The reported parallel PTY flake was diagnosed and is **not a Glasshouse
  defect**. Under stress (320 binary runs, ~6,400 test executions, 27 failing
  runs) every failure had one cause: `openpty` refusing to allocate at spawn
  time. The test named in the earlier report failed zero times. Probes pinned
  it to a macOS `openpty(3)` race under concurrent allocation — 64 live
  pseudo-terminals against a cap of 511 reproduced it, while the same churn
  from one process at ~8,000/s produced none — and it leaves `errno` at `-6`,
  which is not a valid errno. `pty::open_pty` now retries the allocation only,
  five times, side-effect free by construction.

- Discovery no longer gives up when an executable is absent. Both the cmux and
  Ollama capability lines are an OR, and only the left half had been built, so
  Glasshouse running *inside* cmux reported cmux as not found. Presence
  evidence — cmux's control environment, Ollama's configured endpoint — is now
  consulted in the not-found path only, reporting the integration as configured
  with no executable, so `is_usable()` stays false and nothing tries to launch
  it. Only variable *names* are ever recorded: a live `doctor` run with a
  credential in `OLLAMA_HOST` shows zero occurrences of it.
- A `.cmd` harness in a UNC project is refused before any process exists.
  `cmd.exe` would not have failed there — it substitutes the Windows directory
  and runs — so the session would have looked alive while operating outside the
  project entirely.

### What CI caught the moment it was allowed to run

Pushing for CI turned up **two production defects** that every local gate, two
independent reviews, and a green 24-test PTY suite had all missed:

1. **`cmd.exe` cannot open a verbatim `\\?\` path** (`4aa31ad`). Resolving an
   executable canonicalizes it, canonicalizing on Windows yields the verbatim
   form, and that went straight into `cmd.exe /D /C <script>`, which answered
   "The system cannot find the path specified" and exit 1. npm installs
   `claude`, `codex`, and friends as `.cmd` shims, so **no harness could have
   started on Windows at all.**
2. **A project-level executable override silently disabled the harness**
   (`e937dda`). `IntegrationConfig::enabled` was a plain `bool` with
   `#[serde(default)]`, so a project file overriding only a path parsed as
   `enabled = false` and beat a user-level `true`. The decision is now
   `Option<bool>`, making the tri-state per field rather than per entry.

Two process lessons worth keeping:

- **A green Windows tick is not proof the suite ran.** When the lib target
  fails, cargo never reaches `tests/pty_smoke.rs`, so the `.cmd` and
  verbatim-path claims silently did not execute while an earlier ledger
  revision implied they had. Confirm execution, not just the conclusion.
- **Make a platform-only failure explain itself on the first red.** Two CI
  round trips were spent guessing before the test was changed to print
  program, argv, requested cwd, canonical root, marker presence, exit status,
  and both streams. That one change identified the bug immediately.

### Review findings, and one the reviewer got half right

A read-only Ox reviewer worked the batch as a ten-item checklist and returned
ACCEPT WITH FINDINGS. Both findings were real and both are fixed:

- `SessionRecord::disposition` led with `lifecycle if lifecycle.is_live()`. A
  **guarded arm does not count towards exhaustiveness**, so the match needed a
  wildcard, and a new `SessionLifecycle` variant would have silently become
  `Active` — the opposite of what its "unreachable" comment claimed. Both it
  and `is_live` now enumerate every variant with no `_`, verified by adding a
  variant and watching three compile errors appear.
- `format_age`'s explicit `seconds < 0` branch returned the same string as the
  arm below it.

The reviewer's *reasoning* on the second was wrong: it said `saturating_sub`
clamps to zero, making the branch dead. It does not — `i64::saturating_sub`
saturates at `i64::MIN`, so the value really can be negative and the branch was
reachable, merely redundant. Right conclusion, wrong mechanism. Checking it
rather than accepting it also turned up an edge the report missed: a row
holding `i64::MIN` prints an absurd age, now pinned by a test that asserts the
honest contract (finite, never negative) instead of a prettier one that would
have required a magic clamp.

## Unresolved loose ends

- **The shell's key bindings are plain single keys**, because no native session
  owns the keyboard yet. When one does (Phase 5) they must move behind a prefix
  or a mode, or they will steal keystrokes the harness needs.
  `ShellState::handle_key` is deliberately the only place that has to change.
- The shell reads sessions once at startup and on an explicit redraw event.
  Nothing yet raises that event, so a session started elsewhere while the shell
  is open does not appear until it is reopened. `AppEvent::Redraw` and
  `ShellState::refresh` are the seam, and `refresh` already reconciles by
  identifier rather than index.
- The viewport is reserved and empty. Phase 5 fills it.
- `session::runtime` (`SessionRuntime`) exists and is proven against real
  processes on all three platforms, but **has no production caller yet**, so
  seven Phase 4 boxes stay unchecked. `.agent-runtime/design-shell-session-modes.md`
  records the decision that unblocks it: the shell's single-key bindings cannot
  coexist with forwarding every keystroke to a harness, so control mode and
  session mode split, with `Ctrl-]` as a single-chord escape.
- `SessionRuntime::is_running()` reports the status cached by the last
  `poll_exits`, not a fresh answer from the operating system. That is honest —
  it is documented as observation-based — but it caught a test out: a mutation
  killing every session on `close` stayed green because the survivor had not
  been polled since. Any test asserting liveness must poll first.
- Exit detection cannot currently be proven independent of output *on macOS*.
  The discriminating case needs a process that exits while its output stream
  stays open, and a direct probe showed macOS reports end-of-file on the
  pseudo-terminal master as soon as the foreground child exits, even with a
  background child still holding the slave. The capability's real risk — a
  silent-but-running harness mistaken for a finished one — is covered.

- **Nothing calls `open_for_resume` in production.** The cross-project resume
  guard is implemented, structurally enforced, and mutation-proven, but there
  is no `glasshouse resume`, so Phase 1 line 90 is `PARTIALLY VERIFIED` and its
  box stays unchecked. Closing it needs a harness adapter, not more code here.
- No harness adapter captures a native session identifier yet, so in production
  `sessions.native_session_id` is always `NULL` and no session ever reaches the
  `Resumable` disposition. The mechanism is complete; what feeds it is Phase
  7/8.
- Only `Embedded` presentation occurs in production, because `glasshouse
  launch` is the only session producer. `Headless` and `External` arrive with
  Phase 4 and Phase 17.
- `glasshouse sessions` has no filtering, no sorting options, and no way to
  remove a record. Phase 11 owns the real overview; this is the minimum that
  makes the stored metadata observable.

- The forced-exit orphan is **fixed**: an attached session registers a cleanup
  that `shutdown`'s force path runs before `process::exit`. It is best effort
  by construction (`try_lock`, never `lock`) because a cleanup that waits could
  hang the one escape hatch whose purpose is to always work. If the lock is
  held at that instant the harness is still orphaned — no worse than before,
  and the alternative is a Glasshouse that will not die.
- `session::attach` owns the process's terminal for its whole life: its stdin
  pump cannot be cancelled, so the process exits out from under it. The
  multi-session TUI will need a different input path.
- Native Windows UNC project roots remain refused; `cmd.exe` cannot reliably
  hold a UNC working directory.
- Antigravity detection lacks a real-install verification. cmux
  control-environment and Ollama configured-endpoint detection are now
  implemented and checked.
- The UNC refusal's *premise* — that `cmd.exe` substitutes the Windows
  directory rather than failing — is documented Windows behaviour, not
  something a live run confirmed. No real UNC share was exercised; the refusal
  itself is platform-independent and runs in CI everywhere.
- `IntegrationId::minimum_version()` returns `None` for every integration, so
  unsupported-version classification exists but is unreachable. Declaring a
  real minimum needs verified release data this environment does not have.
- The main session TUI, session metadata schema, harness adapters, durable
  memory table, and session persistence are not implemented.
- Strict rustdoc still fails on 15 pre-existing lib-doc diagnostics, 9 of them
  public docs linking to private items. The count in an earlier revision of
  this file said 12 and was simply wrong; this session added none, verified by
  measuring the baseline with the branch stashed.
- The cross-harness completion protocol remains design documentation. This
  session used its durable-file half — each worker wrote
  `.agent-runtime/report-<TASK-ID>.md` — with manual visible pane polling and
  no automatic wake, exactly as the protocol prescribes until its safety tests
  exist.

## Where to go next

**Phase 4 — the generic PTY session runtime — is the next capability, and it is
unblocked.** It is also the keystone: three Phase 3 boxes, Phase 5's terminal
embedding, and the whole of Phase 11's overview all wait on sessions being live
in-process rather than only recorded.

Phase 4 asks for keystroke forwarding to the active PTY, programmatic sends,
interrupts, a bounded scrollback per session, inactive sessions that keep
running, switching that changes only presentation, and a headless mode. Two
things already built are the foundation and two are the obstacle:

- `pty::PtyProcess` and `launch::HarnessLaunch` already start a real harness in
  a real PTY bound to the project root.
- `session::attach` is **not** reusable as-is. It owns the process's terminal
  for the whole of its life and its stdin pump cannot be cancelled — it relies
  on the process exiting out from under it. A multi-session runtime needs an
  input path that can be handed between sessions, which is new work, and it is
  Red: PTY lifecycle, job control, and concurrency.

Still blocked, unchanged:

- Phase 1 line 90 — the cross-project resume guard is complete, structurally
  enforced and mutation-proven, but nothing can *attempt* a resume until a
  harness adapter exists (Phase 6/7/8). **Do not close it with a `glasshouse
  resume` that can only ever report "not resumable".**
- Phase 1 line 92 and Phase 3's project-memory view — Phase 20's memory table.
  Migration 2 is the pattern to copy: make the project boundary a trigger, not
  a query filter.
- Antigravity's executable name and real minimum harness versions — facts this
  environment does not have.
- Phase 2C onboarding — interdependent product decisions that need the user.
- Phase 2D settings — now unblocked in principle, since a TUI exists. It is a
  reasonable alternative to Phase 4 if smaller, lower-risk work is wanted.

## Active worker tasks and results

Workers run as visible normal-TUI `ox` panes in the workers cmux workspace.
**Start `ox` plainly and type the prompt into its visible TUI** — that is what
`GLASSHOUSE_ORCHESTRATOR_PROMPT.md` has always said, and an earlier revision of
this file was wrong to suggest `ox --prompt`. The flag is listed in `ox --help`
but does not reliably start the turn, leaving a pane that looks like a stalled
worker. Never `ox run`. Reports go to `.agent-runtime/report-<TASK-ID>.md`,
because the ox viewport cannot be scrolled back reliably.

Two practical notes learned the hard way: a surface just created with
`cmux new-surface` needs several seconds before its shell accepts input, and
text sent earlier is silently eaten — read the *target* surface, not a sibling,
before sending. And prefer a fresh surface over a pane still holding a live `ox`
session from an earlier task.

- **Reviewer (read-only), this session:** a ten-item checklist over the session
  store. Returned ACCEPT WITH FINDINGS with two real defects, both fixed in
  `cdd6656`. Its reasoning on one was wrong even though its conclusion was
  right — it called a branch dead because "`saturating_sub` clamps to zero",
  which `i64::saturating_sub` does not do. Checking rather than accepting also
  turned up an edge case the report had missed. **Verify the reasoning, not
  just the verdict.**
- Earlier sessions: two implementers in isolated worktrees (`session/select.rs`
  and the `Option<bool>` config fix), a reviewer over the PTY/launch/shutdown
  diff, and a spawn-site inventory.

Every worker gate is re-run by the orchestrator rather than taken on report —
one worker's report would otherwise have carried tests that did not compile,
and one inventory row named a test that does not exist.

## Commands run and outcome

- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets
  --all-features -- -D warnings`, `cargo test --workspace --all-features`
  (265 lib + 2 bin + 28 PTY smoke), `rustup run 1.85.0 cargo check --locked`,
  `git diff --check` — all pass.
- Strict rustdoc reports 15 pre-existing lib-doc diagnostics, 9 of them public
  docs linking to private items. This session added none, verified by measuring
  the baseline with the branch stashed.
- Mutation checks, each observed to fail the test it targets: the cross-project
  resume refusal; the schema trigger; the trigger's `IS NOT` fail-closed
  behaviour; the native-session unique index, its scope, and its nullability;
  the resumable disposition; lifecycle-as-activity; session recording in
  `launch`; the recorded outcome; the project root in the TUI (unit and real
  terminal); root truncation direction; the session bar; Escape's
  overlay-first semantics; the status bar's bindings and notes; and the
  text-first block-element guard.
- Two mutations did **not** fail, and both were informative rather than
  ignorable. One showed a documented justification for the unique index's
  `WHERE` clause was simply false. The other showed the status bar's width
  measurement did nothing, because Ratatui already clips the row; that code was
  deleted rather than tested harder.
- CI `32815286487` on `3d606e3` and `32815757547` on `cdd6656` — green on
  Linux, macOS, Windows and lint, with the Windows job confirmed to have
  executed 228 lib and 22 PTY tests rather than merely reporting green.

## Next exact step

Hand this checkpoint to Opus:

> Start with `git status`, `git log -5`, this handoff, and
> `.agent-runtime/CONTINUATION.md` — whose Part 1 is generic standing rules for
> any orchestrator session, including re-arming the context and usage-window
> watches, which do not survive a session. Pushing to run CI is standing
> authorization; do it without asking, and treat a red Windows job as ordinary
> work.
>
> Phase 2 is complete and Phase 3 is nine boxes of twelve, with the three
> remaining blocked on Phase 5 and Phase 20 rather than unstarted. **Read
> "Where to go next" before choosing.**
>
> The recommended next capability is **Phase 4, the generic PTY session
> runtime**. Read the note there about `session::attach` before starting: it is
> not reusable as-is, because it owns the terminal for the process's whole life
> and its stdin pump cannot be cancelled. Designing an input path that can be
> handed between sessions is the real work, and it is Red — PTY lifecycle, job
> control, concurrency — so it belongs to the orchestrator, not to Ox.
>
> Two habits from this session are worth keeping. **Assert against a specific
> row or field, never a whole screen**: two tests here passed while the thing
> they claimed to check was broken, because the matched string also appeared
> elsewhere. And **treat a mutation that does not fail as information about the
> code, not just the test** — one such mutation correctly identified a piece of
> the status bar as doing nothing, and it was deleted rather than tested harder.
>
> Do not stub a blocked capability to keep the map's order looking intact.
