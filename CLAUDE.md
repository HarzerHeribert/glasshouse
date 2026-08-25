# Claude Code project instructions

Glasshouse uses a spec-to-evidence, multi-harness development process. Before
working as the primary orchestrator, read these files completely in this order:

1. `GLASSHOUSE_IMPLEMENTATION_CAPABILITY_MAP.md`
2. `GLASSHOUSE_HANDOFF.md`
3. `GLASSHOUSE_AGENT_SDLC.md`
4. `GLASSHOUSE_WORKER_CAPABILITIES.md`
5. `GLASSHOUSE_HARNESS_HOOK_PROTOCOL.md`
6. `GLASSHOUSE_CAPABILITY_EVIDENCE.md`
7. `GLASSHOUSE_ORCHESTRATOR_PROMPT.md`
8. `GLASSHOUSE_DESIGN_DECISIONS.md`

The capability map is authoritative. Work in its stated order. Do not check a
box until its evidence-ledger entry is `COMPLETE`. Only the primary Opus
orchestrator integrates, commits, and updates project-status records.

Keep Claude Code, OpenCode/Ox, and other native harness workers visible in cmux.
Use isolated worktrees for editors. Start Ox with the normal `ox` TUI—never
`ox run` or a headless loop. Follow the worker do/don't rules and the safe hook
protocol rather than personal global routing configuration.

Current phase and next action belong in `GLASSHOUSE_HANDOFF.md`; do not encode
phase-specific assumptions in this file.
