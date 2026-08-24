# Glasshouse implementation handoff

Last updated: 2026-08-24 (Europe/Berlin)

## Current capability / phase

Phase A reconciliation is complete. Phase B has completed the Phase 1 per-project SQLite isolation foundation and the project-bound harness-launch mechanism. The next required unchecked capability remains Phase 1: ensure every spawned harness process starts in the active canonical project root. The mechanism is verified locally, but the checkbox stays open until a named production harness consumer is forced through it and the path passes Windows CI.

## Verified completed work

- The merged working tree builds cleanly as one Rust workspace/binary.
- Local macOS integration gate is green: formatting, compilation, strict Clippy, 169 unit tests, and 23 PTY smoke tests.
- GitHub Actions run `32767245019` for `d04420d` passed lint plus build/test jobs on Ubuntu, macOS, and Windows.
- `glasshouse --version`, `glasshouse --help`, and `glasshouse doctor` work on the current host.
- Each project now gets exactly one eager `<state_dir>/glasshouse.db`, with no global or caller-selected database path. The database is bound to the canonical project ID and separate-project tests prove physically distinct files under one shared Glasshouse data root.
- SQLite schema version 1 and its migration ledger are applied atomically with project binding under `BEGIN IMMEDIATE`. New Unix files are `0600`; symlink, non-regular, read-only, corrupt, too-new, foreign, and unbound metadata states fail closed without replacement or adoption.
- Concurrent initialization is verified by a 16-thread unit regression (repeated ten times by the worker) and an independent process-boundary probe of three fresh projects with 32 simultaneous launches each.
- The Phase 1 database-isolation checkbox describes where project memory is constrained to live. It does **not** claim the Phase 20 memory table, kinds, extraction, or retrieval APIs; those remain unchecked and unimplemented.
- Phase 0 and the other currently checked Phase 1 claims have matching code/tests; harness spawn/resume isolation, cross-project retrieval-by-construction at the retrieval layer, and main-TUI root display remain unimplemented.
- Phase 2A/2B/2C checkbox claims have been reconciled conservatively: only CI-, test-, or runtime-proven behavior is checked.
- `HarnessLaunch` is now the public sanctioned launch boundary: it accepts a resolved executable, arguments, and child-only environment changes, exposes no cwd/program setter, translates Windows scripts through `ResolvedExecutable::spawn_command`, and reaches PTY spawn only through crate-private `TerminalCommand::for_harness`.
- `HarnessLaunch` has a manual redacted `Debug`: argument contents and environment values are structurally unreachable from its rendering. A secret-shaped regression test and an independent security re-review both verify this boundary.
- A fake installed harness is resolved through the real executable resolver and launched through `HarnessLaunch`; its own cwd report is matched to the project root by filesystem identity rather than string comparison. The Windows branch installs a `.cmd` harness and exercises the real `cmd.exe /D /C` translation.
- The cwd smoke test answers ConPTY's startup DSR, strips CSI only for path parsing, retains raw output for failures, and polls before process exit to avoid racing the PTY collector. The focused test passed ten consecutive local runs.

## Unresolved loose ends

- Antigravity detection lacks a real-install verification; cmux control-environment detection and Ollama configured-endpoint detection are absent.
- `IntegrationId::minimum_version()` returns `None` for every integration, so unsupported-version classification exists but no real integration can currently reach it.
- The main session TUI, session metadata schema, harness adapters, durable memory table, and session persistence are not implemented.
- The database final-path symlink check is an open-time defense, not a file-handle/TOCTOU guarantee. The owner-only project state directory limits exposure; revisit if database paths become user-selectable.
- Project path guarding is path-based and therefore not a file-handle/TOCTOU defense; revisit when Glasshouse begins opening user-selected project files.
- `TerminalCommand::new` and `PtyProcess::spawn` remain public for generic PTY/platform work, so Rust cannot prevent an in-crate caller from misclassifying a harness as a generic process. No named production harness consumer exists yet; the first one must use `HarnessLaunch` exclusively.
- Native Windows UNC project roots remain an adapter-stage limitation: `cmd.exe` cannot use a UNC current directory reliably. Refuse unsupported cmd-mediated UNC launches with a clear diagnostic when the first adapter lands.
- Strict rustdoc currently fails on 12 pre-existing intra-doc-link diagnostics in config, integrations, onboarding, platform execution, project scope, and PTY process documentation. The new launch module and the edited PTY module add no rustdoc diagnostic.

## Active worker tasks and results

- cmux workspace 7, surface `99FA…`, normal interactive `ox` TUI: implemented `HarnessLaunch`, its resolver/Windows-script integration, fake-installed-harness smoke test, and the manual secret-redacted `Debug` repair. No map/handoff edits and no commit.
- cmux workspace 7, surface `8D8C…`, normal interactive `ox` TUI: independent security review rejected derived `Debug` because it exposed argv/environment values; after the repair it ran all five launch tests plus strict Clippy and returned `ACCEPT` with no remaining must-fix finding.
- cmux workspace 7, surface `8569…`, normal interactive `ox` TUI: independent Windows/mechanism review returned `ACCEPT`, then separately designed project-local leaf-to-verifier-to-root Ox completion routing. No edits.
- All worker diffs were inspected by the orchestrator; the final checks and runtime probes below were rerun independently.

## Commands run and outcome

- `cargo fmt --all -- --check` — pass.
- `cargo check --workspace --all-targets` — pass.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — pass.
- `cargo test --workspace --all-features` — pass (169 unit + 23 PTY smoke tests).
- `rustup run 1.85.0 cargo check --locked --workspace --all-targets` — pass against the workspace's declared MSRV.
- Ten consecutive focused runs of `a_fake_installed_harness_launches_inside_the_discovered_project_root` — pass.
- `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps` — fails on 12 pre-existing intra-doc-link diagnostics; filtered output confirms no diagnostic in `launch.rs` or the edited `pty/mod.rs`.
- `rustup run 1.85.0 cargo test --locked -p glasshouse database::tests` — pass (13 database tests on the declared MSRV).
- `cargo test -p glasshouse database::tests -- --nocapture` — pass (13 database tests).
- Real executable contention probe: three fresh roots × 32 simultaneous `glasshouse doctor` launches — 96/96 pass; each database has schema version 1 and its matching project ID. The same probe failed before the transaction repair with `table project_metadata already exists`.
- `gh run view 32767245019 --json jobs,...` — reconciled baseline `d04420d` passed CI on Linux, macOS, and Windows; lint passed. The SQLite batch is locally verified but not yet pushed.
- Live CLI probes for `--version`, `--help`, and `doctor` — pass.
- Interactive `glasshouse setup` probe in a temporary config directory — pass; the wizard completed and persisted all expected integration decisions.

## Next exact step

Add the first named production harness consumer. Resolve an enabled `IntegrationId` from the effective project/user configuration and discovery result, reject disabled/unusable integrations, and construct `HarnessLaunch` without exposing a cwd input. Cover explicit override precedence, disabled/unusable outcomes, direct executables, and Windows-script argument rejection without spawning a real interactive vendor harness. Check Phase 1 line 89 only after every production harness-spawn route uses this consumer and Windows CI is green.
