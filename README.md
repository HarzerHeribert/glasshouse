# Glasshouse

**Coding agents are excellent, and every one of them is a silo.**

Claude Code, Codex, Antigravity, OpenCode — each owns its own sessions, its own
memory, its own credentials, its own idea of what the project is. Run two of
them, or two sessions of one, and nothing above them knows what is already warm,
which subscription is paying for the current turn, what one session learned an
hour ago that the next will re-derive from scratch, or that two of them are
about to edit the same file.

Glasshouse is the layer that knows. It is a lean, project-scoped control plane
for the harnesses you already have installed. It starts them as **real native
harness sessions** over a PTY, keeps every one of them directly observable and
typeable into, routes work between sessions and model resources on measured
evidence, records what the project learned, and lets an orchestrator session
delegate work to other first-class sessions.

It does not replace those products, and it does not hide them behind a
proprietary agent loop.

> Glasshouse orchestrates agents without hiding them.

## What it does today

- **Runs the real thing.** Every interactive session is backed by a real
  installed harness, launched through an isolated profile and driven over a
  PTY. Nothing is emulated, and nothing is wrapped in a loop you cannot see.
- **One project, hard boundaries.** Memory, sessions, logs and runtime state are
  scoped to a single project root, and cross-project access is disabled
  structurally rather than by convention.
- **Routes on evidence, and can explain it.** The router picks a destination
  from what it has measured — latency, reliability, prompt-cache temperature,
  remaining capacity and burn rate across several subscriptions and providers
  at once — and every choice it makes is recorded with the reasoning behind it.
- **Remembers the project.** Durable memory with authority classes, decision
  provenance and age decay, extracted from real sessions and injected into new
  ones as memory rather than as instruction.
- **Keeps context from filling up.** A context firewall sits between the harness
  and the model and compacts tool output before it crosses.
- **Coordinates parallel sessions.** Soft, project-scoped, turn-scoped file
  claims and structured edit intent, so that when two sessions are heading for
  the same file Glasshouse says so and the orchestrator re-plans only the
  conflicting part — the newest of these, and the one the table below still
  shows in progress.
- **One executable.** `glasshouse` is a single binary. No daemon, no background
  service, no Node, no Python.

## Status

Under active implementation. `docs/product/capability-map.md` is
the authoritative specification and tracks what is done.

<!-- progress:start -->
## Progress

`████████████████████████████████████░░░░` **1291 closed** · **107 active committed open** (92%)

Separately tracked, and not release-blocking: **7 deferred gate criteria** (Phase 52, Phase 53) awaiting a decision, and **229 parked experimental lines** under Maybe / Experimental.

<details>
<summary>Per-phase breakdown (83 of 109 active phases complete)</summary>

| Phase | Done |
|---|---|
| Phase 0 — Repository and executable foundation | 8/8 ✅ |
| Phase 1 — Project-root detection and hard isolation | 15/15 ✅ |
| Phase 2A — Cross-platform runtime | 16/16 ✅ |
| Phase 2B — Agent and tool auto-detection | 16/16 ✅ |
| Phase 2C — First-run onboarding | 19/19 ✅ |
| Phase 2D — Settings foundation | 20/20 ✅ |
| Phase 2 — Persistent project state | 10/10 ✅ |
| Phase 3 — TUI shell | 12/12 ✅ |
| Phase 4 — Generic PTY session runtime | 12/12 ✅ |
| Phase 5 — Native terminal embedding | 8/8 ✅ |
| Phase 6 — Harness adapter interface | 13/13 ✅ |
| Phase 7 — Claude Code adapter | 10/10 ✅ |
| Phase 8 — Codex adapter | 10/10 ✅ |
| Phase 9 — Antigravity adapter | 5/7 |
| Phase 9A — Harness launch profiles | 26/26 ✅ |
| Phase 9B — Scoped harness wrappers and shims | 9/9 ✅ |
| Phase 9C — Provider protocol model | 12/12 ✅ |
| Phase 9D — Built-in provider templates | 14/14 ✅ |
| Phase 9E — Secret storage | 12/13 |
| Phase 9F — Direct provider launch profiles | 13/13 ✅ |
| Phase 9G — Glasshouse local gateway process | 19/19 ✅ |
| Phase 9H — Sticky gateway routing for harness-backed interactive sessions | 13/14 |
| Phase 9I — Free-pool routing | 14/14 ✅ |
| Phase 9J — Harness-model pairing model | 20/20 ✅ |
| Phase 9K — Harness-aware response profiles | 29/37 |
| Phase 10 — Unified session model | 14/14 ✅ |
| Phase 10A — Session supervision | 13/13 ✅ |
| Phase 11 — Session overview | 10/10 ✅ |
| Phase 12 — Unified lifecycle event bus | 8/8 ✅ |
| Phase 13 — Direct session messaging | 7/7 ✅ |
| Phase 14 — Orchestrator role | 11/11 ✅ |
| Phase 15 — Orchestrator wake-up flow | 7/8 |
| Phase 16 — Worker transparency | 7/7 ✅ |
| Phase 17 — cmux optional integration | 10/10 ✅ |
| Phase 18 — Raw event recording | 10/10 ✅ |
| Phase 19 — Portable session checkpoints | 14/14 ✅ |
| Phase 20 — Minimal durable project memory | 16/18 |
| Phase 21 — Memory extraction | 13/13 ✅ |
| Phase 21A — Memory authority classes | 12/12 ✅ |
| Phase 21B — Decision provenance and assumptions | 11/11 ✅ |
| Phase 21C — Validity conditions and invalidation | 11/11 ✅ |
| Phase 21D — Memory age and relevance decay | 9/9 ✅ |
| Phase 21E — Decision ladder and conflict handling | 8/12 |
| Phase 21F — Memory retrieval quality | 10/11 |
| Phase 21G — Memory revalidation | 3/9 |
| Phase 21H — Simplicity-first implementation policy | 10/10 ✅ |
| Phase 21I — Production-aware implementation checks | 11/11 ✅ |
| Phase 21J — Implementation review checklist | 9/9 ✅ |
| Phase 21K — Assumption-aware implementation guardrails | 42/43 |
| Phase 22 — Memory lifecycle and supersession | 9/9 ✅ |
| Phase 23 — Memory full-text search | 7/7 ✅ |
| Phase 24 — Memory reranking | 6/6 ✅ |
| Phase 25 — Project knowledge view | 10/10 ✅ |
| Phase 26 — Memory query for agents | 6/6 ✅ |
| Phase 27 — Context injection | 11/11 ✅ |
| Phase 28 — File-aware memory lookup | 5/5 ✅ |
| Phase 29 — Memory commits | 8/8 ✅ |
| Phase 30 — Session context metadata | 7/8 |
| Phase 31 — Compaction-aware behavior | 2/7 |
| Phase 32 — Resource registry | 12/12 ✅ |
| Phase 32A — Unified quota and capacity model | 14/21 |
| Phase 32B — Quota telemetry sources | 14/14 ✅ |
| Phase 32C — Subscription capacity estimation | 11/12 |
| Phase 32D — Normalized remaining-capacity score | 11/12 |
| Phase 32E — Burn rate and exhaustion forecasting | 10/10 ✅ |
| Phase 32F — Protected quota reserve | 8/8 ✅ |
| Phase 32G — Provider-aware request-cost estimation | 8/10 |
| Phase 33 — Resource health | 13/15 |
| Phase 33A — Routing evidence ledger | 15/15 ✅ |
| Phase 33B — Reliability-adjusted agent performance | 11/14 |
| Phase 33C — Failure, quota, and route correlation | 15/15 ✅ |
| Phase 34 — Capability registry | 10/10 ✅ |
| Phase 34A — Workload tiers | 10/10 ✅ |
| Phase 34B — Routing-model role | 15/15 ✅ |
| Phase 34C — Automatic routing-model selection | 12/13 |
| Phase 34D — Router request schema | 13/13 ✅ |
| Phase 34E — Router economics | 9/9 ✅ |
| Phase 34F — Model capability and tier calibration | 11/11 ✅ |
| Phase 35 — Lightweight task classification | 14/14 ✅ |
| Phase 35A — Candidate generation | 11/11 ✅ |
| Phase 35B — Candidate scoring | 24/25 |
| Phase 35C — Capacity-aware tier escalation and downgrade | 9/9 ✅ |
| Phase 35D — Routing under subscription pressure | 8/8 ✅ |
| Phase 36 — Session affinity | 8/8 ✅ |
| Phase 37 — Basic session-aware router | 10/11 |
| Phase 38 — Quota-preserving routing | 6/7 |
| Phase 39 — Gateway-backed disposable jobs | 9/9 ✅ |
| Phase 40 — Fresh-session handoff | 9/9 ✅ |
| Phase 41 — Project overview | 15/15 ✅ |
| Phase 42 — External control API | 13/13 ✅ |
| Phase 43 — MCP surface for orchestrators | 10/10 ✅ |
| Phase 44 — User control and override | 9/9 ✅ |
| Phase 45 — Failure handling | 9/9 ✅ |
| Phase 46 — Security and contamination tests | 8/8 ✅ |
| Phase 47 — Observability without spectacle | 15/15 ✅ |
| Phase 48 — CLI ergonomics | 8/8 ✅ |
| Phase 49 — Configuration | 16/16 ✅ |
| Phase 50 — Tracked project knowledge as an optional feature | 7/7 ✅ |
| Phase 51 — Evaluation hooks | 22/37 |
| Phase 52 — Criteria before adding semantic/vector retrieval (deferred experiment gate) | 1/6 |
| Phase 53 — Criteria before adding graph storage (deferred experiment gate) | 3/5 |
| Phase 54 — Criteria before deeper cmux coupling | 4/4 ✅ |
| Phase 54A — Setup and portability completion criteria | 10/10 ✅ |
| Phase 55 — V1 completion definition | 23/23 ✅ |
| Phase 56 — Harness–subscription decoupling: choose the harness, route the subscription and model | 12/12 ✅ |
| Phase 56A — Entitlement pool and subscription broker: several accounts, one scheduler | 13/13 ✅ |
| Phase 57 — Context firewall: tool-output compaction between harness and model | 27/27 ✅ |
| Phase 58 — Context economy: cache-stable translation, entitlement-aware reduction, and a measured token budget | 15/15 ✅ |
| Phase 59 — Decompression: the code's physical shape catches up with its architecture | 6/8 |
| Phase 60 — Parallel-session file coordination | 13/16 |
| Phase 61 — pane: the first-party harness | 0/34 |
| Phase 52 — Criteria before adding semantic/vector retrieval (deferred experiment gate) | 1/6 — deferred gate |
| Phase 53 — Criteria before adding graph storage (deferred experiment gate) | 3/5 — deferred gate |

</details>
<!-- progress:end -->

## pane — the first-party harness

Every shipping harness turns a tool result into text in the conversation. The
context firewall reduces what crosses; it does not change the shape of the
problem. A grep over a large repository is paid for when it arrives, and paid
for again on every turn that carries it afterwards.

`pane` is a harness built on one decision made differently.

> A tool result never becomes text in the conversation.

It becomes a **named object in a runtime the model addresses from code**. The
model receives a bounded preview and the handle; the object stays where it is,
and the model acts by returning a program that calls tools by name on those
objects. Everything else about the harness may be ordinary — that one decision
is the harness.

`pane` is a second crate with its own binary, joined to Glasshouse by protocol
rather than by linkage. Neither side depends on the other at compile time.
Glasshouse reaches `pane` exactly as it reaches Claude Code: a declared
executable, declared arguments, a PTY. `pane` reaches Glasshouse the way any
harness does — a gateway base URL, an MCP endpoint, a hook command — each one
optional. Standalone is simply the mode where nobody answers on the socket:
every Glasshouse-provided capability degrades to a local one, never to an error.

What it is being built to be:

- **Drop-in.** It reads `CLAUDE.md`, `AGENTS.md`, the hooks and permissions in
  `.claude/settings.json`, `.claude/commands`, the skills directories and
  `.mcp.json` from the project, with nothing edited.
- **Sandboxed before it is useful.** The project's existing permissions file
  becomes the sandbox grant, and no model-authored code runs until the sandbox
  exists. That ordering is fixed: the alternative is a window in which generated
  code runs beside your keyring with nothing in between.
- **Honest about what paid.** A two-region interface — conversation on one side,
  telemetry on the other — showing which entitlement served each request and
  what it cost. When Glasshouse is absent, the sidebar collapses instead of
  guessing.
- **Watched.** A supervisor reads a compressed trajectory every few turns with a
  cheaper model and makes exactly one decision: intervene or not. Its target is
  a planted three-turn loop caught within two turns, with no human in the room.

`pane` is not "a better harness", and it is not trying to be. It arrives as one
row in the capability registry — a destination with an unusual cost profile that
the router picks when the workload suits it and ignores when it does not. Two
destinations that behave identically teach a router nothing; two that fail
differently are evidence.

## What pane is built to achieve

These are aims, in the future tense, each with the measurement that would settle
it named beside it.

- **Tokens per turn that stay roughly flat as a task grows.** A preview does not
  grow with the object behind it, so the conversation should stop inflating with
  the work. Settled by tokens per *completed task*, per workload tier.
- **A 48k-token tool result that costs a preview line to know about and nothing
  to compute over.** Settled the same way, on a workload where a large result is
  produced early and used repeatedly.
- **The success criterion, and it is deliberately narrow:** *on at least one
  workload tier, measured over completed tasks rather than turns, native beats
  the adapter path on tokens or wall-clock without losing on outcome.* One tier
  is enough. A head-on comparison against harnesses better resourced and better
  tuned against their own models is a bet lost slowly, and it is not the bet
  being made here.

## There are no measurements yet

Not one. `pane` does not run today; the crate is being stood up now. The first
piece of work in its phase is not the runtime and not the sandbox — it is the
ruler: one fixed task set through an existing harness and through the candidate,
reporting tokens per completed task, wall-clock and outcome side by side, scored
per workload tier so that a win on one tier is visible when the aggregate is
not. That comes first precisely because the interesting claim is the kind that
feels true and often isn't, and because a comparison that measures tokens per
turn instead of tokens per completed task is not allowed to count. Recording
that a tier showed no win is a complete outcome of that work, not a failure of
it.

The table above is what exists, phase by phase, with the evidence behind each
closed line in `docs/product/evidence/`. Everything under *pane* is an aim.

## Agent development process

Agent-assisted implementation follows a spec-to-evidence SDLC rather than
accepting generated code as proof of completion:

- [`docs/process/agent-sdlc.md`](docs/process/agent-sdlc.md) — implementation and
  verification lifecycle;
- [`docs/process/worker-capabilities.md`](docs/process/worker-capabilities.md) —
  Opus, Sonnet, and Ox responsibilities and limits;
- [`docs/process/harness-hook-protocol.md`](docs/process/harness-hook-protocol.md)
  — safe Claude Code/OpenCode completion reporting;
- [`docs/product/evidence/README.md`](docs/product/evidence/README.md) —
  behavioral contracts mapped to production and regression evidence;
- [`docs/process/orchestrator-prompt.md`](docs/process/orchestrator-prompt.md) —
  reusable phase-independent Opus prompt;
- [`CLAUDE_CODE_START_PROMPT.md`](CLAUDE_CODE_START_PROMPT.md) — short prompt
  for starting a new primary Claude Code session.

## Build

```sh
cargo build --release
```

The result is a single `glasshouse` executable with no daemon, background
service, Node, or Python requirement.

### Cross-platform checks

Glasshouse targets macOS, Linux, and native Windows. CI builds and tests on all
three, but most portability breakage (`cfg`-gated dead code, platform-only
imports) is catchable locally without leaving Linux:

```sh
rustup target add x86_64-apple-darwin x86_64-pc-windows-msvc
cargo check --workspace --all-targets --target x86_64-apple-darwin
cargo check --workspace --all-targets --target x86_64-pc-windows-msvc
```

## Usage

```sh
glasshouse                    # operate on the current project
glasshouse --scope <path>     # select a project root explicitly
glasshouse --help
```

Glasshouse operates on exactly one project root — the containing Git repository
when there is one, otherwise the current directory. All state, sessions, and
memory are isolated per project root.

### Environment

| Variable | Purpose |
| --- | --- |
| `GLASSHOUSE_DATA_DIR` | Override the per-user application-data directory |
| `GLASSHOUSE_CONFIG_DIR` | Override the per-user configuration directory |
| `GLASSHOUSE_LOG` | Enable logging with a tracing filter, e.g. `debug` |

## License

MIT OR Apache-2.0
