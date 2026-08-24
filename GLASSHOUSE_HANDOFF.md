# Glasshouse implementation handoff

Last updated: 2026-08-25 (Europe/Berlin)

## Current capability / phase

Phase B continues. The Phase 1 capability at map line 89 — "Ensure every
spawned harness process starts with its working directory set to the current
project root" — now has a real production consumer and end-to-end evidence, and
its evidence-ledger entry has moved from PARTIALLY VERIFIED to **LOCALLY
VERIFIED**. The checkbox stays open for exactly one reason: the contract makes
Windows-specific claims and the merged commit has not run `windows-latest` CI.
Nothing else is outstanding for it.

## Verified completed work

- The previously reported parallel PTY flake is **diagnosed and was never a
  Glasshouse defect**. Reproduced under stress (320 binary runs, ~6,400 test
  executions, 27 failing runs): every failure had a single cause — `openpty`
  refusing to allocate at spawn time, before any Glasshouse code runs. The test
  named in the earlier report, `streams_output_and_reports_a_successful_exit`,
  failed **zero** times; failures landed on whichever of seven different tests
  happened to be spawning.
- The mechanism was pinned by direct probe, not inferred: macOS `openpty(3)`
  races under *concurrent* allocation. Sixteen processes holding at most four
  pseudo-terminals each — 64 live against a `kern.tty.ptmx_max` of 511, with 17
  in use on the host — reproduced it, while the same churn driven from a single
  process at ~8,000 allocations a second produced none. The failure leaves
  `errno` at `-6`, which is not a valid errno, so the condition cannot be
  classified from the error value.
- `pty::open_pty` therefore retries the allocation only, five times, 20ms
  apart. This is not the blind retry wrapper the previous handoff warned
  against: it covers one call, and nothing has been started when that call
  fails — no child, no file, no engaged terminal — so retrying is side-effect
  free by construction. A genuinely exhausted host still fails, with the real
  error preserved as the source.
- `glasshouse launch [harness] [-- args]` is the first **production consumer**
  of `HarnessLaunch`. Until now the Phase 1 promise rested on a mechanism no
  shipped code exercised.
- `session::select` chooses exactly one harness and one executable, preferring a
  project-level configured path over a user-level one and an explicit path over
  PATH discovery. It refuses ambiguity instead of guessing, and a configured
  path that will not resolve is an error rather than a silent fallback to some
  other binary.
- `session::attach` is a transparent bridge, not a renderer. That is what makes
  ConPTY's startup handshake work with no terminal emulation in Glasshouse: the
  cursor-position query reaches the user's real terminal, which answers it as it
  would for any program, and the reply is forwarded back into the pty. Nothing
  in Glasshouse may answer it as well — the harness would receive the reply
  twice, as input.
- `shutdown::RawModeGuard` takes raw mode without the alternate screen. Raw
  mode is what routes Ctrl-C to the harness rather than to Glasshouse: the line
  discipline stops generating signals and the keystroke arrives as an ordinary
  `0x03` byte, forwarded like any other input. Restoration runs through the same
  flag and path as the full-screen guard, so a panic or signal restores it too.
- `HarnessLaunch::size` lets a session start at the real terminal size instead
  of coming up at 24x80 and being resized after its first frame.
- The end-to-end test runs the **shipped binary** in a real pseudo-terminal and
  proves project binding by filesystem identity, project-over-user precedence
  via a decoy executable that must not run, and exit-code propagation.
- Every load-bearing claim was mutation-checked by actually running the
  mutation: removing the project layer from `EffectiveConfig::executable` makes
  the decoy run; making `for_harness` use the process cwd makes no harness
  report the project root; making `exit_code_for` always succeed breaks
  propagation; setting `PTY_ALLOCATION_ATTEMPTS` to 1 breaks the retry test. All
  four were observed to fail.

## Unresolved loose ends

- **A project-level config entry that sets only `executable` also disables the
  integration.** `IntegrationConfig::enabled` is a plain `bool` with
  `#[serde(default)]`, so an entry written to override just a path carries
  `enabled = false`, and `EffectiveConfig` reads that as an explicit
  project-level refusal overriding a user-level `enabled = true`. Making
  `enabled` an `Option<bool>` is the obvious fix; it touches config, onboarding,
  and their tests, so it belongs in its own batch.
- A second interrupt while a session is attached still reaches
  `shutdown`'s force path, which calls `process::exit` and therefore skips
  `PtyProcess`'s destructor, orphaning the harness. The first interrupt is
  handled properly (terminate, then kill after a grace period), and raw mode
  means Ctrl-C never produces a signal at all, so this needs a deliberate
  external `kill` twice. Fixing it means giving the global signal handler a way
  to reach the active session.
- `session::attach` owns the process's terminal for its whole life: its stdin
  pump cannot be cancelled (a thread blocked reading stdin is not interruptible
  without stealing the keystroke that unblocks it), so the process exits out
  from under it. The multi-session TUI will need a different input path.
- Antigravity detection lacks a real-install verification; cmux control-
  environment detection and Ollama configured-endpoint detection are absent.
- `IntegrationId::minimum_version()` returns `None` for every integration, so
  unsupported-version classification exists but is unreachable.
- The main session TUI, session metadata schema, harness adapters, durable
  memory table, and session persistence are not implemented.
- The database final-path symlink check and the project path guard are
  path-based, not file-handle/TOCTOU guarantees.
- Strict rustdoc still fails on 12 pre-existing intra-doc-link diagnostics.
- The cross-harness completion protocol remains design documentation. Worker
  reporting in this session used the durable-file half of it — each worker wrote
  `.agent-runtime/report-<TASK-ID>.md` — with manual visible pane polling and no
  automatic wake, which is exactly what the protocol prescribes until its safety
  tests exist.

## Active worker tasks and results

Workers ran as visible normal-TUI `ox` panes in the workers cmux workspace,
started with `ox --prompt` pointing at a task-packet file — never `ox run`, and
never by pasting a packet into a running TUI.

- **Implementer, isolated worktree `ox/session-select`:** built
  `session/select.rs` with all ten required acceptance tests. It hit one real
  failure and fixed it itself, independently reaching the correct conclusion
  about the config schema's per-entry `enabled` bool. Two things it could not
  get right from its own vantage point were corrected during integration: it
  exceeded the ~400-line stop condition without saying so, and its diagnostic
  suggested a `--harness` flag that does not exist, since `cli.rs` was outside
  its permitted files.
- **Reviewer, read-only on main:** ten-item mechanical checklist over the
  PTY/launch/shutdown diff. Returned ACCEPT with 10/10 PASS, and its independent
  non-vacuity reasoning matched the mutation the orchestrator had actually run.
  Its one finding — that raw-mode restoration also emits `LeaveAlternateScreen`
  — was deliberate behavior that was undocumented, and is now documented as the
  repair it is.
- **Inventory, read-only on main:** full spawn-site inventory, re-run after the
  merge. Three production spawn sites, all project-bound; zero production
  callers of the generic `TerminalCommand::new`; `for_harness` unreachable from
  outside the crate.

Every worker gate was re-run by the orchestrator rather than taken on report —
one worker's report would otherwise have carried tests that did not compile.

## Commands run and outcome

- `cargo fmt --all -- --check` — pass.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — pass.
- `cargo test --workspace --all-features` — pass (186 unit + 24 PTY smoke).
- `rustup run 1.85.0 cargo check --locked --workspace --all-targets` — pass.
- `git diff --check` — pass.
- Four mutation checks, each observed to fail the test it targets (above).
- PTY stress reproduction: 40 rounds x 8 concurrent binaries; 27/320 runs failed,
  all with the same `openpty` allocation refusal.
- Host probes: pseudo-terminal cap measured at 511; concurrent-allocation race
  reproduced at 64 live pseudo-terminals; single-process churn at ~8,000/s
  produced zero failures.
- Live CLI probes: `glasshouse launch` in `--help`; the no-terminal refusal;
  the unknown-harness diagnostic; the non-harness-integration diagnostic.

## Next exact step

Hand this checkpoint to Opus:

> Start by inspecting `git status`, `git log -5`, this handoff, and the Phase 1
> entry in `GLASSHOUSE_CAPABILITY_EVIDENCE.md`. Re-run the full local gates to
> confirm the tree is still green, then check the latest CI run with
> `gh run list`. Pushing to run CI is standing orchestrator authorization — do
> it without asking. The only thing standing between line 89 and a checked box
> is a green `windows-latest` job on the current commit. When all three
> platforms and lint are green, set the ledger entry to COMPLETE, check line 89,
> and commit. Never check it on local evidence alone: the first Windows run
> failed on a defect that three green local suites had hidden.
>
> Then proceed to line 90, "Reject any attempt to resume a Glasshouse-managed
> session whose project identifier differs from the current project identifier."
> Note that this needs session persistence that does not exist yet, so the first
> question to settle is whether a resume path can be built without pulling Phase
> 2's session metadata schema forward — decide that before assigning any work.
>
> Consider taking the `Option<bool>` config fix in the loose ends first: it is
> small, self-contained, and a latent user-facing bug where overriding a project
> executable path silently disables the harness.

Session continuity is armed and documented in `.agent-runtime/CONTINUATION.md`,
whose Part 1 is generic standing rules for any orchestrator session — read it
before starting. Worker worktrees `glasshouse-wt-attach` and
`glasshouse-wt-session-select` are merged and disposable once confirmed idle.
