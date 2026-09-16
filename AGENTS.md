# Codex project instructions

Glasshouse uses a spec-to-evidence, multi-harness development process — the
same process for every harness. Read `CLAUDE.md` first; it is authoritative
and applies to Codex exactly as it applies to any other harness. Then read
`docs/product/architecture.md` for the three-component split (Pane, the
inference gateway, Glasshouse) this repository builds.

## Codex-specific notes

Keep Codex, OpenCode/Ox, and other native harness workers visible in cmux.
Use isolated worktrees for editors. Start Ox with the normal `ox` TUI — never
`ox run` or a headless loop. Follow the worker do/don't rules and the safe
hook protocol rather than personal global routing configuration.

Current phase and next action belong in `docs/process/handoff.md`; do not
encode phase-specific assumptions in this file.
