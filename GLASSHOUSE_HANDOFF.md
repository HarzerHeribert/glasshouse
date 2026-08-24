# Glasshouse implementation handoff

Last updated: 2026-08-24 (Europe/Berlin)

## Current capability / phase

Phase A reconciliation of the merged Claude Code implementation (`d04420d`). The first required unchecked capability in implementation order is Phase 1: create a physically separate SQLite memory database for each project. No Phase 1 database/session work has started yet.

## Verified completed work

- The merged working tree builds cleanly as one Rust workspace/binary.
- Local macOS baseline is green: formatting, compilation, strict Clippy, 150 unit tests, and 20 PTY smoke tests.
- GitHub Actions run `32767245019` for `d04420d` passed lint plus build/test jobs on Ubuntu, macOS, and Windows.
- `glasshouse --version`, `glasshouse --help`, and `glasshouse doctor` work on the current host.
- Phase 0 and the currently checked Phase 1 claims have matching code/tests; Phase 1 memory database, session spawn/resume isolation, cross-project memory-by-construction, and main-TUI root display are not implemented.
- Phase 2A/2B/2C checkbox claims have been reconciled conservatively: only CI-, test-, or runtime-proven behavior is checked.

## Unresolved loose ends

- Antigravity detection lacks a real-install verification; cmux control-environment detection and Ollama configured-endpoint detection are absent.
- `IntegrationId::minimum_version()` returns `None` for every integration, so unsupported-version classification exists but no real integration can currently reach it.
- The main session TUI, persistent SQLite state, harness adapters, and session persistence are not implemented.
- Project path guarding is path-based and therefore not a file-handle/TOCTOU defense; revisit when Glasshouse begins opening user-selected project files.

## Active worker tasks and results

- cmux workspace 7, pane 1, normal interactive `ox` TUI: Phase 2A/2B/2C review complete; no edits. It identified five stale Phase 2A claims and a conservative set of Phase 2B/2C claims now reflected in the map.
- cmux workspace 7, pane 2, normal interactive `ox` TUI: Phase 1 SQLite design review complete; no edits. It recommends bundled `rusqlite`, a project-derived DB path, eager validation/creation, and no arbitrary-path constructor.

## Commands run and outcome

- `cargo fmt --all -- --check` — pass.
- `cargo check --workspace --all-targets` — pass.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — pass.
- `cargo test --workspace --all-features` — pass (150 unit + 20 PTY smoke tests).
- `gh run view 32767245019 --json jobs,...` — current HEAD CI passed on Linux, macOS, and Windows; lint passed.
- Live CLI probes for `--version`, `--help`, and `doctor` — pass.
- Interactive `glasshouse setup` probe in a temporary config directory — pass; the wizard completed and persisted all expected integration decisions.

## Next exact step

Implement the Phase 1 per-project SQLite database capability with migrations and isolation tests. Keep its public constructor project-bound so callers cannot supply a foreign database path.
