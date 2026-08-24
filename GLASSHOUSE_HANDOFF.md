# Glasshouse implementation handoff

Last updated: 2026-08-25 (Europe/Berlin)

## Current capability / phase

**Phase 1 line 89 is COMPLETE and checked**: "Ensure every spawned harness
process starts with its working directory set to the current project root."
Its evidence-ledger entry is `COMPLETE`, backed by CI run `32788309876` on
commit `e3295a7`, green on `ubuntu-latest`, `macos-latest`, `windows-latest`,
and lint.

The next unchecked Phase 1 boxes — 90, 92, and 93 — are **all blocked on later
phases**, which is a real ordering problem in the map rather than a reason to
stop; see "Where to go next".

## Verified completed work

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

## Unresolved loose ends

- A second interrupt while a session is attached reaches `shutdown`'s force
  path, which calls `process::exit` and so skips `PtyProcess`'s destructor,
  orphaning the harness. The first interrupt is handled properly, and raw mode
  means Ctrl-C never raises a signal at all, so this needs a deliberate
  external `kill` twice. Fixing it means giving the global signal handler a way
  to reach the active session.
- `session::attach` owns the process's terminal for its whole life: its stdin
  pump cannot be cancelled, so the process exits out from under it. The
  multi-session TUI will need a different input path.
- Native Windows UNC project roots remain refused; `cmd.exe` cannot reliably
  hold a UNC working directory.
- Antigravity detection lacks a real-install verification; cmux
  control-environment and Ollama configured-endpoint detection are absent.
- `IntegrationId::minimum_version()` returns `None` for every integration, so
  unsupported-version classification exists but is unreachable. Declaring a
  real minimum needs verified release data this environment does not have.
- The main session TUI, session metadata schema, harness adapters, durable
  memory table, and session persistence are not implemented.
- Strict rustdoc still fails on 12 pre-existing intra-doc-link diagnostics.
- The cross-harness completion protocol remains design documentation. This
  session used its durable-file half — each worker wrote
  `.agent-runtime/report-<TASK-ID>.md` — with manual visible pane polling and
  no automatic wake, exactly as the protocol prescribes until its safety tests
  exist.

## Where to go next

**Phase 1's three remaining boxes are each blocked on a later phase**, and the
database confirms it: the only tables are `project_metadata` and
`schema_migrations`.

- Line 90 (reject a cross-project session resume) needs a sessions table and a
  resume path — Phase 2.
- Line 92 (cross-project memory retrieval disabled by design) needs the memory
  table — Phase 20.
- Line 93 (display the project root in the TUI) needs the TUI — Phase 3.

So the map's stated order cannot be followed literally here. Do not fake any of
them with a stub; decide deliberately whether to pull Phase 2's session
metadata schema forward to unblock line 90, and record that decision.

The genuinely actionable work that needs no later phase is in Phase 2B:

- "Detect cmux when a usable cmux executable **or supported cmux control
  environment** is present" — the executable half already works; the missing
  half is the `CMUX_*` control environment.
- "Detect Ollama when a usable ollama executable **or configured local
  endpoint** is present" — same shape, via `OLLAMA_HOST` or the conventional
  local endpoint.

Both are well-scoped, testable without new architecture, and independent of
everything blocked above.

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
> Line 89 is done. Read "Where to go next" above before picking anything up:
> the next three Phase 1 boxes are each blocked on a later phase, so the first
> real decision is whether to pull Phase 2's session metadata schema forward to
> unblock line 90, or to take the two unblocked Phase 2B detection capabilities
> (cmux control environment, Ollama configured endpoint) first. Make that
> decision explicitly and record it; do not stub a blocked capability to keep
> the order looking intact.
