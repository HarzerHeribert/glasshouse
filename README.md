# Glasshouse

A lean, project-scoped control plane for existing coding-agent harnesses such as
Claude Code, Codex, and Antigravity.

Glasshouse does not replace those products or hide them behind a proprietary
agent loop. It starts and manages **real native harness sessions**, keeps every
session directly observable and interactive, routes work between sessions and
available model resources, records project-specific knowledge, and lets an
orchestrator session delegate work to other first-class sessions.

> Glasshouse orchestrates agents without hiding them.

## Status

Under active implementation. `GLASSHOUSE_IMPLEMENTATION_CAPABILITY_MAP.md` is
the authoritative specification and tracks what is done.

<!-- progress:start -->
## Progress

`████████░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░` 255 / 1267 mandatory capabilities (20%)

<details>
<summary>Per-phase breakdown (10 of 103 phases complete)</summary>

| Phase | Done |
|---|---|
| Phase 0 — Repository and executable foundation | 8/8 ✅ |
| Phase 1 — Project-root detection and hard isolation | 14/15 |
| Phase 2A — Cross-platform runtime | 16/16 ✅ |
| Phase 2B — Agent and tool auto-detection | 16/16 ✅ |
| Phase 2C — First-run onboarding | 19/19 ✅ |
| Phase 2D — Settings foundation | 13/20 |
| Phase 2 — Persistent project state | 10/10 ✅ |
| Phase 3 — TUI shell | 11/12 |
| Phase 4 — Generic PTY session runtime | 11/12 |
| Phase 5 — Native terminal embedding | 8/8 ✅ |
| Phase 6 — Harness adapter interface | 12/13 |
| Phase 7 — Claude Code adapter | 8/10 |
| Phase 8 — Codex adapter | 9/10 |
| Phase 9 — Antigravity adapter | 5/7 |
| Phase 9A — Harness launch profiles | 19/26 |
| Phase 9B — Scoped harness wrappers and shims | 9/9 ✅ |
| Phase 9C — Provider protocol model | 12/12 ✅ |
| Phase 9D — Built-in provider templates | 14/14 ✅ |
| Phase 9E — Secret storage | 11/13 |
| Phase 9F — Direct provider launch profiles | 11/13 |
| Phase 9G — Glasshouse local gateway process | 19/19 ✅ |
| Phase 9H — Sticky gateway routing for harness-backed interactive sessions | 0/14 |
| Phase 9I — Free-pool routing | 0/14 |
| Phase 9J — Harness-model pairing model | 0/20 |
| Phase 9K — Harness-aware response profiles | 0/37 |
| Phase 10 — Unified session model | 0/14 |
| Phase 11 — Session overview | 0/10 |
| Phase 12 — Unified lifecycle event bus | 0/8 |
| Phase 13 — Direct session messaging | 0/7 |
| Phase 14 — Orchestrator role | 0/11 |
| Phase 15 — Orchestrator wake-up flow | 0/8 |
| Phase 16 — Worker transparency | 0/7 |
| Phase 17 — cmux optional integration | 0/10 |
| Phase 18 — Raw event recording | 0/10 |
| Phase 19 — Portable session checkpoints | 0/14 |
| Phase 20 — Minimal durable project memory | 0/18 |
| Phase 21 — Memory extraction | 0/13 |
| Phase 21A — Memory authority classes | 0/12 |
| Phase 21B — Decision provenance and assumptions | 0/11 |
| Phase 21C — Validity conditions and invalidation | 0/11 |
| Phase 21D — Memory age and relevance decay | 0/9 |
| Phase 21E — Decision ladder and conflict handling | 0/12 |
| Phase 21F — Memory retrieval quality | 0/11 |
| Phase 21G — Memory revalidation | 0/9 |
| Phase 21H — Simplicity-first implementation policy | 0/10 |
| Phase 21I — Production-aware implementation checks | 0/11 |
| Phase 21J — Implementation review checklist | 0/9 |
| Phase 21K — Assumption-aware implementation guardrails | 0/43 |
| Phase 22 — Memory lifecycle and supersession | 0/9 |
| Phase 23 — Memory full-text search | 0/7 |
| Phase 24 — Memory reranking | 0/6 |
| Phase 25 — Project knowledge view | 0/10 |
| Phase 26 — Memory query for agents | 0/6 |
| Phase 27 — Context injection | 0/11 |
| Phase 28 — File-aware memory lookup | 0/5 |
| Phase 29 — Memory commits | 0/8 |
| Phase 30 — Session context metadata | 0/8 |
| Phase 31 — Compaction-aware behavior | 0/7 |
| Phase 32 — Resource registry | 0/12 |
| Phase 32A — Unified quota and capacity model | 0/21 |
| Phase 32B — Quota telemetry sources | 0/14 |
| Phase 32C — Subscription capacity estimation | 0/12 |
| Phase 32D — Normalized remaining-capacity score | 0/12 |
| Phase 32E — Burn rate and exhaustion forecasting | 0/10 |
| Phase 32F — Protected quota reserve | 0/8 |
| Phase 32G — Provider-aware request-cost estimation | 0/10 |
| Phase 33 — Resource health | 0/15 |
| Phase 33A — Routing evidence ledger | 0/15 |
| Phase 33B — Reliability-adjusted agent performance | 0/14 |
| Phase 33C — Failure, quota, and route correlation | 0/15 |
| Phase 34 — Capability registry | 0/10 |
| Phase 34A — Workload tiers | 0/10 |
| Phase 34B — Routing-model role | 0/15 |
| Phase 34C — Automatic routing-model selection | 0/13 |
| Phase 34D — Router request schema | 0/13 |
| Phase 34E — Router economics | 0/9 |
| Phase 34F — Model capability and tier calibration | 0/11 |
| Phase 35 — Lightweight task classification | 0/14 |
| Phase 35A — Candidate generation | 0/11 |
| Phase 35B — Candidate scoring | 0/25 |
| Phase 35C — Capacity-aware tier escalation and downgrade | 0/9 |
| Phase 35D — Routing under subscription pressure | 0/8 |
| Phase 36 — Session affinity | 0/8 |
| Phase 37 — Basic session-aware router | 0/11 |
| Phase 38 — Quota-preserving routing | 0/7 |
| Phase 39 — Gateway-backed disposable jobs | 0/9 |
| Phase 40 — Fresh-session handoff | 0/9 |
| Phase 41 — Project overview | 0/15 |
| Phase 42 — External control API | 0/13 |
| Phase 43 — MCP surface for orchestrators | 0/10 |
| Phase 44 — User control and override | 0/9 |
| Phase 45 — Failure handling | 0/9 |
| Phase 46 — Security and contamination tests | 0/8 |
| Phase 47 — Observability without spectacle | 0/15 |
| Phase 48 — CLI ergonomics | 0/8 |
| Phase 49 — Configuration | 0/16 |
| Phase 50 — Tracked project knowledge as an optional feature | 0/7 |
| Phase 51 — Evaluation hooks | 0/37 |
| Phase 52 — Criteria before adding semantic/vector retrieval | 0/6 |
| Phase 53 — Criteria before adding graph storage | 0/5 |
| Phase 54 — Criteria before deeper cmux coupling | 0/4 |
| Phase 54A — Setup and portability completion criteria | 0/10 |
| Phase 55 — V1 completion definition | 0/23 |

</details>
<!-- progress:end -->

## Agent development process

Agent-assisted implementation follows a spec-to-evidence SDLC rather than
accepting generated code as proof of completion:

- [`GLASSHOUSE_AGENT_SDLC.md`](GLASSHOUSE_AGENT_SDLC.md) — implementation and
  verification lifecycle;
- [`GLASSHOUSE_WORKER_CAPABILITIES.md`](GLASSHOUSE_WORKER_CAPABILITIES.md) —
  Opus, Sonnet, and Ox responsibilities and limits;
- [`GLASSHOUSE_HARNESS_HOOK_PROTOCOL.md`](GLASSHOUSE_HARNESS_HOOK_PROTOCOL.md)
  — safe Claude Code/OpenCode completion reporting;
- [`GLASSHOUSE_CAPABILITY_EVIDENCE.md`](GLASSHOUSE_CAPABILITY_EVIDENCE.md) —
  behavioral contracts mapped to production and regression evidence;
- [`GLASSHOUSE_ORCHESTRATOR_PROMPT.md`](GLASSHOUSE_ORCHESTRATOR_PROMPT.md) —
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
