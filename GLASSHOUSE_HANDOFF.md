# Glasshouse implementation handoff

Last updated: 2026-08-25 (Europe/Berlin)

## Current capability / phase

**Phase 2 — Persistent project state is complete.** Six capabilities were
closed this session, each with a `COMPLETE` ledger entry:

1. Persist Glasshouse session metadata independently of native harness files.
2. Persist the Glasshouse-ID to native-session-ID mapping.
3. Persist harness, creation time, last activity time, role, lifecycle, and
   project identifier.
4. Persist the process presentation mode.
5. Persist enough to distinguish active, resumable, closed, and failed.
6. Never store provider credentials in the project database.

The checked count went from 63 to 69.

### The decision this session made, and why

The previous checkpoint ended on a deliberate choice between three blocked
groups. **Phase 2 was chosen** because it was the only one actually unblocked:
Phase 2C onboarding needs product decisions from the user, Phase 2D settings
needs a TUI that does not exist, and Phase 3 is larger and would be cheaper
after a session model exists. Phase 2 also does double duty — Phase 1 line 90
was blocked on exactly this table.

Risk class was Red (persistence, migrations, resume identity, secret
boundaries), so it was the orchestrator's own work; Ox is barred from it.

## Verified completed work

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

**Read this before picking anything up.** The three-group structure from the
previous checkpoint still holds, minus what Phase 2 retired.

**Group 1 — blocked on a later phase.** Two of the three remain:

- Phase 1 line 92 (cross-project memory retrieval disabled by design) needs the
  memory table — Phase 20. The pattern to copy is already in place: migration 2
  proves how to make a project boundary structural rather than a query filter,
  and the memory table should get the same trigger treatment.
- Phase 1 line 93 (display the project root in the TUI) needs the TUI —
  Phase 3.

Phase 1 line 90 (reject a cross-project session resume) is **no longer blocked
on schema** — the guard exists, is structurally enforced by triggers, and is
mutation-proven. It is now blocked on there being a resume path at all, which
needs a harness adapter (Phase 6/7/8). See its ledger entry, which says
explicitly not to close it with a `resume` command that can only ever report
"not resumable".

**Group 2 — blocked on facts this environment does not have.** Unchanged:
Antigravity's real executable name, and real minimum harness versions. Guessing
either produces confident, wrong results.

**Group 3 — needs product decisions.** Unchanged: the Phase 2C onboarding block
(provider and gateway configuration, routing-model choices, Configure now / Do
later) is interdependent and shapes everything after it. Worth agreeing the
shape with the user before implementing.

**The natural forward path is now Phase 3, the TUI shell.** It is the largest
remaining unblocked item, it unblocks Phase 1 line 93 and Phase 2D, and it now
has a session model to render: `glasshouse sessions` is effectively the
non-interactive version of Phase 11's session overview, and the columns it
prints are the ones the overview needs.

## Active worker tasks and results

Workers ran as visible normal-TUI `ox` panes in the workers cmux workspace,
started with `ox --prompt` pointing at a task-packet file — never `ox run`, and
never by pasting a packet into a running TUI. Reports went to
`.agent-runtime/report-<TASK-ID>.md`, because the ox viewport cannot be
scrolled back reliably.

- **Implementer (isolated worktree):** built `session/select.rs` with all ten
  required acceptance tests. It hit a real failure and fixed it itself,
  independently reaching the correct conclusion about the config schema. Two
  things it could not get right from its own vantage point were corrected at
  integration: it exceeded its stated size limit without saying so, and its
  diagnostic suggested a `--harness` flag that does not exist, because
  `cli.rs` was outside its permitted files.
- **Implementer (isolated worktree):** the `Option<bool>` config fix, with the
  regression test reproduced before the fix as instructed.
- **Reviewer (read-only):** ten-item checklist over the PTY/launch/shutdown
  diff. ACCEPT, 10/10 PASS, and its independent non-vacuity reasoning matched
  the mutation the orchestrator had actually run.
- **Inventory (read-only):** spawn-site inventory, re-run after the merge.
  Three production spawn sites, all project-bound; zero production callers of
  the generic `TerminalCommand::new`.

Every worker gate was re-run by the orchestrator rather than taken on report —
one worker's report would otherwise have carried tests that did not compile,
and one inventory row named a test that does not exist.

## Commands run and outcome

- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets
  --all-features -- -D warnings`, `cargo test --workspace --all-features`
  (194 unit + 26 PTY smoke), `rustup run 1.85.0 cargo check --locked`,
  `git diff --check` — all pass.
- Mutation checks, each observed to fail the test it targets: project-over-user
  precedence, the project-derived working directory, exit-code propagation, the
  PTY allocation retry, and the config tri-state fix.
- PTY stress: 40 rounds x 8 concurrent binaries; 27/320 runs failed, all with
  the same `openpty` refusal. Host probes measured the pseudo-terminal cap at
  511 and reproduced the race at 64 live allocations.
- CI `32788309876` on `e3295a7` — green on Linux, macOS, Windows, and lint,
  with the Windows job confirmed to have executed 186 lib and 20 PTY tests.

## Next exact step

Hand this checkpoint to Opus:

> Start with `git status`, `git log -5`, this handoff, and
> `.agent-runtime/CONTINUATION.md` — whose Part 1 is generic standing rules for
> any orchestrator session, including re-arming the context and usage-window
> watches, which do not survive a session. Pushing to run CI is standing
> authorization; do it without asking, and treat a red Windows job as ordinary
> work.
>
> Phase 2 is complete and Phase 1's remaining gaps are documented above with
> the specific later phase each is waiting on. **Read "Where to go next" before
> choosing** — the map's order still cannot be followed literally.
>
> The recommended next capability is **Phase 3, the TUI shell**: it is the
> largest unblocked item, it unblocks Phase 1 line 93 and Phase 2D, and
> `session::store` now gives it something real to render. If you take it,
> `main.rs: session_report` is the non-interactive version of the same listing
> and its columns are the ones Phase 11 asks for.
>
> Do not stub a blocked capability to keep the map's order looking intact. In
> particular, do not close Phase 1 line 90 with a `glasshouse resume` that can
> only ever report "not resumable", and do not guess an Antigravity executable
> name or a minimum harness version.
