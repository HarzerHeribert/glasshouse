# Glasshouse process documents — index

These are the spec-to-evidence process documents `CLAUDE.md` and `AGENTS.md`
point every orchestrator at, in the order those files name. `README.md`,
`CLAUDE.md`, `AGENTS.md` and `CLAUDE_CODE_START_PROMPT.md` stay at the
repository root; everything else lives here, split into two worlds.

## The boundary: product vs. process

- **`docs/product/`** — what Glasshouse *is*. Capability requirements,
  behavioral contracts, and the design decisions that shaped them.
- **`docs/process/`** — how *we build it*. Orchestration mechanics, worker
  routing, model tiers, and the SDLC an agent follows to implement a
  capability.

**The rule that follows from it: product source code (`crates/**`) may cite
`docs/product/**` in its doc comments, and must never cite `docs/process/**`.**
A doc comment explaining a design decision or a capability belongs to the
product world; a citation of orchestration mechanics in shipped source is a
sign something is wired to the wrong layer.

## `docs/product/` — what Glasshouse is

- [`capability-map.md`](product/capability-map.md) — the authoritative
  capability map.
- [`design-decisions.md`](product/design-decisions.md) — settled design
  decisions and their reasoning.
- [`evidence/`](product/evidence/README.md) — the capability evidence ledger,
  split by phase; start at its `README.md`.

## `docs/process/` — how we build it

- [`handoff.md`](process/handoff.md) — current phase, verified work, next
  action.
- [`agent-sdlc.md`](process/agent-sdlc.md) — the implementation and
  verification lifecycle.
- [`worker-capabilities.md`](process/worker-capabilities.md) — Opus, Sonnet,
  and Ox responsibilities and limits.
- [`harness-hook-protocol.md`](process/harness-hook-protocol.md) — safe
  Claude Code/OpenCode completion reporting between worker sessions. Despite
  the name, this is a contract for project-local adapters, not a Glasshouse
  product feature.
- [`orchestrator-prompt.md`](process/orchestrator-prompt.md) — the reusable
  phase-independent Opus prompt.
- [`orchestration-practice.md`](process/orchestration-practice.md) — how to
  run the process without repeating mistakes that have already cost whole
  cycles.
- [`orchestration-measurements.md`](process/orchestration-measurements.md) —
  the standing inherited experiment on model-tier cost/quality.
