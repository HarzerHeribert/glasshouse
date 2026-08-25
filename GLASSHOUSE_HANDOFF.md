# Glasshouse implementation handoff

Last updated: 2026-08-25 (Europe/Berlin)

## Current capability / phase

**Phase 2 complete, Phase 3 eleven of twelve, Phase 4 nine of twelve.**
Twenty-seven capabilities were closed this session; the checked count went from
63 to 90.

The through-line: Phase 2 gave Glasshouse its own durable session records,
Phase 3 gave it an interface, Phase 4 gave it live processes behind that
interface, and the shell now joins all three — `glasshouse` with no arguments
opens a real TUI, `n` starts a real harness in a real pseudo-terminal, and the
keyboard reaches it.

### The decisions this session made, and why

1. **Phase 2 first**, because it was the only unblocked group: 2C onboarding
   needs product decisions from the user, 2D settings needed a TUI, and Phase 3
   would be cheaper once a session model existed. It also retired the schema
   half of Phase 1 line 90.
2. **Phase 3 next**, as recorded when Phase 2 closed: the largest unblocked
   item, and `session::store` had given it something real to render.
3. **Phase 4 next**, as recorded when Phase 3 closed: the keystone that three
   Phase 3 boxes and all of Phase 5 and 11 were waiting on.
4. **Two keyboard modes**, recorded in
   `GLASSHOUSE_DESIGN_DECISIONS.md`. Single-key bindings and
   forwarding every keystroke to a harness cannot both be true. Control mode
   keeps the bindings; session mode forwards everything; `Ctrl-]` returns. One
   chord rather than a prefix, because the thing being escaped may be a
   runaway session.

## Verified completed work

### This session — live sessions behind the interface

- `session::runtime::SessionRuntime` holds several live harnesses, each with
  its own reader thread draining its pseudo-terminal into its own bounded
  `Scrollback`. Focus is only a statement about which session the keyboard
  reaches; `focus()` touches no process.
- `shell::run` is its production consumer: `n` starts a session, session mode
  forwards keystrokes, resize reaches the focused child, ticks poll exits and
  refresh the viewport.
- Exits come from asking the process, never from output going quiet. A harness
  thinking in silence must not be mistaken for one that has finished.

**The defect end-to-end testing found, that unit testing could not.** The
session-mode escape chord was implemented as `Ctrl` + `']'` — which is what the
synthetic `KeyEvent` in its unit tests looked like. Crossterm's Unix parser
decodes the control range `0x1C..=0x1F` arithmetically, so a real terminal's
`Ctrl-]` arrives as `Ctrl` + `'5'` and never matched. A user entering session
mode had **no way back**: precisely the failure the single-chord escape exists
to prevent. Both spellings are now accepted and separately tested.

**A test written and then deleted, twice, for different reasons** — both worth
remembering:

- Asserting that switching sessions changes only the view requires reading the
  frame currently on screen. A full-screen Ratatui application repaints
  differentially, so a captured pseudo-terminal stream cannot be sliced back
  into frames by content, and the assertion silently read every viewport ever
  drawn. Phase 5 needs a real terminal emulator anyway; that is when this
  becomes testable.
- Asserting exit detection is independent of output needs a process that exits
  while its output stream stays open. A direct probe showed macOS reports
  end-of-file on the pseudo-terminal master as soon as the foreground child
  exits, even with a background child still holding the slave, so the
  discriminating case cannot be built there.

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
- **Open question on Windows: does a bare carriage return satisfy a real
  harness?** `encode` sends `\r` for Enter, which is what a terminal sends. The
  Windows *fake* harness reads with `set /p`, which wants CRLF, so the shell's
  end-to-end round-trip test is Unix-only. Making `encode` emit CRLF would be
  wrong — every Unix harness would get a spurious extra newline per keystroke —
  and the harnesses Glasshouse actually targets read raw input and accept CR.
  But that is reasoning, not evidence; confirm it against a real harness on a
  real Windows install. The forwarding path itself is covered on Windows by
  `keystrokes_reach_the_focused_session` at the runtime layer, and the shell's
  mode machinery by `the_shell_enters_and_leaves_session_mode_in_a_real_terminal`.
- `session::runtime` (`SessionRuntime`) exists and is proven against real
  processes on all three platforms, but **has no production caller yet**, so
  seven Phase 4 boxes stay unchecked. `GLASSHOUSE_DESIGN_DECISIONS.md`
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

**Phase 5 — native terminal embedding — is the next capability.** The viewport
currently prints the focused session's scrollback as raw bytes. That is honest
but not a terminal: escape sequences are shown rather than obeyed, so a harness
that redraws itself looks like noise. Phase 5 replaces it with a real terminal
emulator over the same `Scrollback`.

Doing it also unblocks the two things this session could not test:

- The shell's multi-session switching has no end-to-end test, because a
  differential repaint cannot be sliced back into frames from a captured
  stream. An emulator in the test harness makes "what is on screen now" a real
  question with a real answer.
- The viewport's line handling is currently naive.

**The three unchecked Phase 4 lines need callers, not mechanisms.** Sending
text to an unfocused session and sending an interrupt are both orchestrator
operations (Phase 14); nothing yet creates a headless session because the shell
always starts them embedded. All three work and are tested against real
processes — do not rebuild them, wire them up when their feature arrives.

Still blocked, unchanged:

- Phase 1 line 90 — the cross-project resume guard is complete and
  mutation-proven, but nothing can *attempt* a resume until a harness adapter
  exists (Phase 6/7/8). **Do not close it with a `glasshouse resume` that can
  only ever report "not resumable".**
- Phase 1 line 92 and Phase 3's project-memory view — Phase 20's memory table.
  Migration 2 is the pattern to copy: make the project boundary a trigger, not
  a query filter.
- Antigravity's executable name and real minimum harness versions — facts this
  environment does not have.
- Phase 2C onboarding — interdependent product decisions that need the user.
- Phase 2D settings — unblocked now that a TUI exists, and the smaller,
  lower-risk alternative to Phase 5.

## Active worker tasks and results

Three worker tiers, settled during this session at the user's direction:

- **Opus (orchestrator):** red risk — PTY lifecycle, signals, terminal
  restoration, persistence and migrations, concurrency, resume identity, secret
  boundaries — plus every design decision and every judgment about whether a
  capability is complete.
- **Claude Code with Sonnet, in a visible cmux surface:** implementation, to
  save orchestrator tokens. Give it its own `git worktree` and a task packet
  carrying real context — the API it will call, the files it may touch, the
  helpers to reuse, the exact gates. Start it with
  `claude --model sonnet --permission-mode acceptEdits`. It wrote 625 lines of
  cross-platform integration tests and then the shell wiring, respected every
  file boundary it was given, and reported honestly what it could not do.
- **Ox: trivial tasks only** — pure enumeration. It listed every test in a file
  and every production call site accurately. It is **not** reliable for
  verdicts: asked to judge twelve capability lines it marked two "already
  satisfied" whose named production paths had no test covering them.

Start Ox by running **plain `ox`** and typing into its visible TUI, as
`GLASSHOUSE_ORCHESTRATOR_PROMPT.md` has always said. `ox --prompt` is listed in
`ox --help` but does not reliably start the turn.

Practical cmux notes: a surface is only readable while its workspace is
selected and it is the visible tab, so give a worker its own workspace rather
than a second surface in a busy pane. A new surface needs several seconds
before its shell accepts input; text sent earlier is silently eaten.

Every worker gate, and every worker verdict, was re-run or re-derived here. That
caught: a report whose tests did not compile; an inventory naming a test that
does not exist; a correct conclusion reached from false reasoning; and two
worker-written tests that passed while the behaviour they claimed to check was
broken.

## Commands run and outcome

- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets
  --all-features -- -D warnings`, `cargo test --workspace --all-features`
  (295 lib + 2 bin + 39 PTY smoke), `rustup run 1.85.0 cargo check --locked`,
  `git diff --check` — all pass.
- Strict rustdoc reports 15 pre-existing lib-doc diagnostics, 9 of them public
  docs linking to private items. This session added none, verified by measuring
  the baseline with the branch stashed.
- Roughly thirty mutation checks, each observed to fail the test it targets.
  Four did **not** fail, and each was acted on rather than ignored: a false
  justification for a schema clause; a status-bar width measurement that turned
  out to duplicate Ratatui's own clipping (deleted); a liveness assertion
  reading a cached status instead of asking the operating system (fixed); and
  an exit-detection test that could not discriminate on macOS (removed, with
  the platform reason recorded).
- CI, all green on Linux, macOS, Windows and lint, with the Windows job
  confirmed each time to have executed the suite rather than merely reporting
  green: `32815286487`, `32815757547`, `32816717226`, `32819167010` and
  `32821964808`.
- One red Windows job (`32821591638`), and it was worth having. The new
  end-to-end keystroke test failed there — not a product defect, but the fake
  `.cmd` harness reading with `set /p`, which wants CRLF where a real Enter key
  is a bare carriage return. The test was split rather than the encoding
  weakened; see the loose end above.

## Next exact step

Hand this checkpoint to Opus:

> Start with `git status`, `git log -5`, this handoff, and
> `.agent-runtime/CONTINUATION.md` — whose Part 1 is generic standing rules,
> including re-arming the context and usage-window watches, which do not
> survive a session. Pushing to run CI is standing authorization; do it without
> asking.
>
> Phases 2, 3 and 4 are largely closed. **Read "Where to go next" before
> choosing** — what remains nearby needs a later phase or a product decision,
> not more effort here.
>
> The recommended next capability is **Phase 5, native terminal embedding**. It
> replaces the viewport's raw-byte rendering with a real emulator over
> `session::runtime`'s `Scrollback`, and it is also what makes the shell's
> multi-session behaviour testable end to end for the first time.
>
> Three habits from this session are worth keeping:
>
> - **Drive the shipped binary through a real pseudo-terminal.** Every defect
>   found this session was invisible to unit tests: a `.cmd` path Windows could
>   not open, ragged columns from a `Display` that ignored width, and an escape
>   chord that no real terminal ever produces.
> - **Assert against a specific row or field, never a whole screen.** Several
>   tests passed while the thing they claimed to check was broken, because the
>   matched string also appeared elsewhere on screen.
> - **Treat a mutation that does not fail as information about the code.** One
>   correctly identified a piece of the status bar as doing nothing, and it was
>   deleted rather than tested harder.
>
> Do not stub a blocked capability to keep the map's order looking intact.
