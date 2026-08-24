# Glasshouse implementation handoff

Last updated: 2026-08-25 (Europe/Berlin)

## Current capability / phase

Five capabilities were completed and checked this session, each with a
`COMPLETE` ledger entry and green cross-platform CI:

1. **Phase 1** — every spawned harness process starts in the project root.
2. **Phase 2A** — unsupported platform/harness combinations fail with a clear
   diagnostic instead of half-starting.
3. **Phase 2A** — native Windows is a first-class runtime.
4. **Phase 2B** — cmux is detected via its control environment.
5. **Phase 2B** — Ollama is detected via a configured endpoint.

The checked count went from 58 to 63. `main` is clean and pushed, and the
latest CI run is green on Linux, macOS, Windows, and lint.

What remains nearby is **blocked, not merely unstarted** — see "Where to go
next", which is the first thing to read.

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

## Unresolved loose ends

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
- Strict rustdoc still fails on 12 pre-existing intra-doc-link diagnostics.
- The cross-harness completion protocol remains design documentation. This
  session used its durable-file half — each worker wrote
  `.agent-runtime/report-<TASK-ID>.md` — with manual visible pane polling and
  no automatic wake, exactly as the protocol prescribes until its safety tests
  exist.

## Where to go next

**The easy, unblocked work is now done.** Everything still unchecked nearby
falls into one of three groups, and the next orchestrator's first job is to
pick a group deliberately rather than work down the list.

**Group 1 — blocked on a later phase.** The database has only
`project_metadata` and `schema_migrations`, which is what blocks these:

- Phase 1 line 90 (reject a cross-project session resume) needs a sessions
  table and a resume path — Phase 2.
- Phase 1 line 92 (cross-project memory retrieval disabled by design) needs the
  memory table — Phase 20.
- Phase 1 line 93 (display the project root in the TUI) needs the TUI —
  Phase 3.

The map's order cannot be followed literally here. **Do not stub any of them.**
The real decision is whether to pull Phase 2's session metadata schema forward
to unblock line 90 — that is a product/architecture call worth making
explicitly and recording.

**Group 2 — blocked on facts this environment does not have.**

- "Detect Antigravity when a supported Antigravity CLI executable is present"
  needs a real install to confirm the executable name. Guessing aliases is
  worse than missing the detection: `ag` collides with the-silver-searcher and
  would produce a confident, wrong detection.
- "Mark every detected integration as available, configured, unconfigured,
  unsupported-version, or unknown" is implemented for four of the five states.
  `UnsupportedVersion` is unreachable only because `minimum_version()` returns
  `None` everywhere, and inventing a minimum would produce false reports for
  users on perfectly good installs. It needs verified release data, not code.

**Group 3 — needs product decisions, and is a coherent block rather than
individual boxes.** The Phase 2C onboarding items (provider and gateway
configuration, the routing-model choices, Configure now / Do later) are
interdependent and shape how providers and routing work for everything after
them. Worth agreeing the shape with the user before implementing.

If none of the above is desirable, Phase 2D (the settings view) and Phase 3
(the TUI shell) are the natural forward path, and Phase 3 would also unblock
line 93.

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
> Five capabilities were closed this session and the unblocked near-term work
> is done. **Read "Where to go next" above before picking anything up** — what
> remains nearby is blocked rather than merely unstarted, and it sorts into
> three groups with different reasons. Working down the list in order will not
> work.
>
> The first real decision is a product/architecture one: whether to pull
> Phase 2's session metadata schema forward to unblock Phase 1 line 90, or to
> move to Phase 2C onboarding (which needs the user's input on provider and
> routing shape), or to start Phase 3's TUI shell (which would also unblock
> line 93). Make that decision explicitly and record it. Do not stub a blocked
> capability to keep the map's order looking intact, and do not guess an
> Antigravity executable name or a minimum harness version — both would produce
> confident, wrong results, which is worse than the gap they would close.
