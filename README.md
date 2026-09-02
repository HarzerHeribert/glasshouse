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

Under active implementation. `docs/product/capability-map.md` is
the authoritative specification and tracks what is done.

<!-- progress:start -->
## Progress

`█████████████████████████████████████░░░` 1269 / 1355 mandatory capabilities (93%)

<details>
<summary>Per-phase breakdown (81 of 109 phases complete)</summary>

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
| Phase 27 — Context injection | 10/11 |
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
| Phase 32F — Protected quota reserve | 7/8 |
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
| Phase 35B — Candidate scoring | 22/25 |
| Phase 35C — Capacity-aware tier escalation and downgrade | 9/9 ✅ |
| Phase 35D — Routing under subscription pressure | 8/8 ✅ |
| Phase 36 — Session affinity | 8/8 ✅ |
| Phase 37 — Basic session-aware router | 10/11 |
| Phase 38 — Quota-preserving routing | 5/7 |
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
| Phase 52 — Criteria before adding semantic/vector retrieval | 1/6 |
| Phase 53 — Criteria before adding graph storage | 3/5 |
| Phase 54 — Criteria before deeper cmux coupling | 4/4 ✅ |
| Phase 54A — Setup and portability completion criteria | 10/10 ✅ |
| Phase 55 — V1 completion definition | 23/23 ✅ |
| Phase 56 — Harness–subscription decoupling: choose the harness, route the subscription and model | 12/12 ✅ |
| Phase 56A — Entitlement pool and subscription broker: several accounts, one scheduler | 13/13 ✅ |
| Phase 57 — Context firewall: tool-output compaction between harness and model | 27/27 ✅ |
| Phase 58 — Context economy: cache-stable translation, entitlement-aware reduction, and a measured token budget | 15/15 ✅ |
| Phase 59 — Decompression: the code's physical shape catches up with its architecture | 2/8 |

</details>
<!-- progress:end -->

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
