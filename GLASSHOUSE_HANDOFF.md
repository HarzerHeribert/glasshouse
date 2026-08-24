# Glasshouse implementation handoff

Last updated: 2026-08-24 (Europe/Berlin)

## Current capability / phase

Phase A reconciliation is complete. Phase B has completed the Phase 1 per-project SQLite isolation foundation. The next required unchecked capability in implementation order is Phase 1: ensure every spawned harness process starts in the active canonical project root.

## Verified completed work

- The merged working tree builds cleanly as one Rust workspace/binary.
- Local macOS integration gate is green: formatting, compilation, strict Clippy, 163 unit tests, and 20 PTY smoke tests.
- GitHub Actions run `32767245019` for `d04420d` passed lint plus build/test jobs on Ubuntu, macOS, and Windows.
- `glasshouse --version`, `glasshouse --help`, and `glasshouse doctor` work on the current host.
- Each project now gets exactly one eager `<state_dir>/glasshouse.db`, with no global or caller-selected database path. The database is bound to the canonical project ID and separate-project tests prove physically distinct files under one shared Glasshouse data root.
- SQLite schema version 1 and its migration ledger are applied atomically with project binding under `BEGIN IMMEDIATE`. New Unix files are `0600`; symlink, non-regular, read-only, corrupt, too-new, foreign, and unbound metadata states fail closed without replacement or adoption.
- Concurrent initialization is verified by a 16-thread unit regression (repeated ten times by the worker) and an independent process-boundary probe of three fresh projects with 32 simultaneous launches each.
- The Phase 1 database-isolation checkbox describes where project memory is constrained to live. It does **not** claim the Phase 20 memory table, kinds, extraction, or retrieval APIs; those remain unchecked and unimplemented.
- Phase 0 and the other currently checked Phase 1 claims have matching code/tests; harness spawn/resume isolation, cross-project retrieval-by-construction at the retrieval layer, and main-TUI root display remain unimplemented.
- Phase 2A/2B/2C checkbox claims have been reconciled conservatively: only CI-, test-, or runtime-proven behavior is checked.

## Unresolved loose ends

- Antigravity detection lacks a real-install verification; cmux control-environment detection and Ollama configured-endpoint detection are absent.
- `IntegrationId::minimum_version()` returns `None` for every integration, so unsupported-version classification exists but no real integration can currently reach it.
- The main session TUI, session metadata schema, harness adapters, durable memory table, and session persistence are not implemented.
- The database final-path symlink check is an open-time defense, not a file-handle/TOCTOU guarantee. The owner-only project state directory limits exposure; revisit if database paths become user-selectable.
- Project path guarding is path-based and therefore not a file-handle/TOCTOU defense; revisit when Glasshouse begins opening user-selected project files.

## Active worker tasks and results

- cmux workspace 7, surface `99FA…`, normal interactive `ox` TUI: implemented the SQLite batch and the confirmed concurrent-first-launch repair; no map/handoff edits and no commit.
- cmux workspace 7, surface `8D8C…`, normal interactive `ox` TUI: two independent read-only architecture/security reviews. Its must-fix contention finding was reproduced with real processes and repaired; its missing-project-ID hardening nit was also added.
- cmux workspace 7, surface `8569…`, normal interactive `ox` TUI: read-only design for the next project-root working-directory invariant. No edits.
- All worker diffs were inspected by the orchestrator; the final checks and runtime probes below were rerun independently.

## Commands run and outcome

- `cargo fmt --all -- --check` — pass.
- `cargo check --workspace --all-targets` — pass.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — pass.
- `cargo test --workspace --all-features` — pass (163 unit + 20 PTY smoke tests).
- `rustup run 1.85.0 cargo check --locked --workspace --all-targets` — pass against the workspace's declared MSRV.
- `rustup run 1.85.0 cargo test --locked -p glasshouse database::tests` — pass (13 database tests on the declared MSRV).
- `cargo test -p glasshouse database::tests -- --nocapture` — pass (13 database tests).
- Real executable contention probe: three fresh roots × 32 simultaneous `glasshouse doctor` launches — 96/96 pass; each database has schema version 1 and its matching project ID. The same probe failed before the transaction repair with `table project_metadata already exists`.
- `gh run view 32767245019 --json jobs,...` — reconciled baseline `d04420d` passed CI on Linux, macOS, and Windows; lint passed. The SQLite batch is locally verified but not yet pushed.
- Live CLI probes for `--version`, `--help`, and `doctor` — pass.
- Interactive `glasshouse setup` probe in a temporary config directory — pass; the wizard completed and persisted all expected integration decisions.

## Next exact step

Add a project-bound harness command construction/launch seam that derives `cwd` from the active canonical `Project`/`Runtime`, never from a harness adapter argument. Keep generic PTY commands usable for platform tests, add a real child `pwd`/`cd` smoke test through the project-bound seam, and leave the checkbox unchecked unless the mechanism and an actual harness-backed launch path are both concretely verified.
