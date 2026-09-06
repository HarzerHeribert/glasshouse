# Glasshouse

**Two things that belong together, and either one works alone.**

- **`pane`** is a coding harness. You give it a task in a project and it works
  the task in turns. It needs an API key and nothing else — no Glasshouse, no
  daemon, no configuration.
- **`glasshouse`** is a control plane for coding harnesses — `pane`, Claude
  Code, Codex, whatever you already have installed. It starts them as real
  sessions you can see and type into, routes their requests across your
  subscriptions and keys, and remembers what the project learned.

You do not have to adopt both. Most of the value arrives before you do.

## Start where you like

Three steps. Each one is useful on its own, and **most people should stop at the
second.**

**1 — Just the harness.** An API key in the environment, and go:

```sh
pane session --root .
```

That is a complete coding agent. Nothing above it, nothing beside it.

**2 — Add routing, memory and cost.** Point the same binary at Glasshouse:

```sh
glasshouse launch --harness pane
```

Same harness, same behaviour, one extra network hop. Now your requests are
routed across every subscription and key you have configured, the project's
memory is injected and extracted, and you can see what each turn cost and which
account paid. Still one session. No orchestrator, nothing to coordinate.

**3 — Run several at once.** Only if you want it. Sessions become a list, one
can delegate to another, and Glasshouse tells you when two of them are heading
for the same file. This is the part that takes a change in how you work, and it
is the part you can ignore indefinitely.

## What makes `pane` different

Every shipping harness turns a tool result into text in the conversation. A grep
over a large repository is paid for when it arrives, and paid for again on every
turn that carries it afterwards.

> A tool result never becomes text in the conversation.

It becomes a **named object in a runtime the model addresses from code**. The
model gets a bounded preview and a handle; the object stays where it is, and the
model acts by writing a TypeScript program that calls tools by name on those
objects. Measured, in a test that runs on every commit: **275,020 bytes of grep
output cost 205 tokens**, and the handle is still filterable in the next cell.

Two consequences fall out of that one decision:

- **There is no edit tool, and there will not be one.** The model already holds
  the file as an object, so an edit is `text.replace(a, b)` in the cell followed
  by one `write`. A second tool with its own matching rules would be a worse
  version of the language.
- **Running out of context is survivable.** When the conversation stops fitting,
  the redundant parts are dropped first — losslessly, because every handle table
  is re-rendered whole each turn — and if that is not enough the conversation is
  replaced by a checkpoint **while the isolate keeps running**. A grep from turn
  three is still addressable afterwards. A harness whose results *are* its
  transcript cannot do that.

`pane` runs model-authored code, so it does it under an OS sandbox — seatbelt,
Landlock, Windows job objects — whose grants come from the `.claude/settings.json`
your project already has. No model-authored code ran before that sandbox existed,
and that ordering was not negotiable.

## What Glasshouse adds

Nothing in this list requires a second session.

- **Routes on evidence, and can explain it.** The router picks a destination
  from what it has measured — latency, reliability, prompt-cache temperature,
  remaining capacity and burn rate across several subscriptions and providers at
  once — and records the reasoning behind every choice.
- **Remembers the project.** Durable memory with authority classes, decision
  provenance and age decay, extracted from real sessions and injected into new
  ones as memory rather than as instruction.
- **Keeps context from filling up in the harnesses that need it.** A context
  firewall sits between the harness and the model and compacts tool output
  before it crosses. `pane` does not need it — its results never cross — which
  is exactly why the firewall stays: every other harness does.
- **Runs the real thing.** Every interactive session is backed by a real
  installed harness, launched through an isolated profile and driven over a PTY.
  Nothing is emulated, and nothing is wrapped in a loop you cannot see.
- **One project, hard boundaries.** Memory, sessions, logs and runtime state are
  scoped to a single project root, and cross-project access is disabled
  structurally rather than by convention.
- **One executable.** `glasshouse` is a single binary. No daemon, no background
  service, no Node, no Python. The local gateway starts itself when a launch
  needs one and stops with it.

## When you run more than one

Coding agents are excellent, and every one of them is a silo. Claude Code,
Codex, Antigravity, OpenCode — each owns its own sessions, its own memory, its
own credentials, its own idea of what the project is. Run two of them, or two
sessions of one, and nothing above them knows what is already warm, which
subscription is paying for the current turn, what one session learned an hour
ago that the next will re-derive from scratch, or that two of them are about to
edit the same file.

Glasshouse is the layer that knows. It lets an orchestrator session delegate
work to other first-class sessions, and it coordinates them: soft,
project-scoped, turn-scoped file claims and structured edit intent, so that when
two sessions are heading for the same file Glasshouse says so and the
orchestrator re-plans only the conflicting part.

It does not replace those products, and it does not hide them behind a
proprietary agent loop.

> Glasshouse orchestrates agents without hiding them.

**Where the line falls between the two.** Anything that happens inside one turn
or one session belongs to `pane` — its plan, its handles, its sandbox, its
supervisor, its subagents. Anything that spans sessions, projects, providers or
machines belongs to Glasshouse — routing, entitlements, cross-session memory,
delegation, file coordination. `pane` is one session and can never own the
second list; Glasshouse does not own a turn and should not reach into one.

## Status

Under active implementation. `docs/product/capability-map.md` is
the authoritative specification and tracks what is done.

<!-- progress:start -->
## Progress

`█████████████████████████████████████░░░` **1348 closed** · **82 active committed open** (94%)

Separately tracked, and not release-blocking: **0 deferred gate criteria** (Phase 52, Phase 53) awaiting a decision, and **229 parked experimental lines** under Maybe / Experimental.

<details>
<summary>Per-phase breakdown (95 of 112 active phases complete)</summary>

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
| Phase 9 — Antigravity adapter | 7/7 ✅ |
| Phase 9A — Harness launch profiles | 26/26 ✅ |
| Phase 9B — Scoped harness wrappers and shims | 9/9 ✅ |
| Phase 9C — Provider protocol model | 12/12 ✅ |
| Phase 9D — Built-in provider templates | 14/14 ✅ |
| Phase 9E — Secret storage | 13/13 ✅ |
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
| Phase 15 — Orchestrator wake-up flow | 8/8 ✅ |
| Phase 16 — Worker transparency | 7/7 ✅ |
| Phase 17 — cmux optional integration | 10/10 ✅ |
| Phase 18 — Raw event recording | 10/10 ✅ |
| Phase 19 — Portable session checkpoints | 14/14 ✅ |
| Phase 20 — Minimal durable project memory | 18/18 ✅ |
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
| Phase 21K — Assumption-aware implementation guardrails | 43/43 ✅ |
| Phase 22 — Memory lifecycle and supersession | 9/9 ✅ |
| Phase 23 — Memory full-text search | 7/7 ✅ |
| Phase 24 — Memory reranking | 6/6 ✅ |
| Phase 25 — Project knowledge view | 10/10 ✅ |
| Phase 26 — Memory query for agents | 6/6 ✅ |
| Phase 27 — Context injection | 11/11 ✅ |
| Phase 28 — File-aware memory lookup | 5/5 ✅ |
| Phase 29 — Memory commits | 8/8 ✅ |
| Phase 30 — Session context metadata | 8/8 ✅ |
| Phase 31 — Compaction-aware behavior | 7/7 ✅ |
| Phase 32 — Resource registry | 12/12 ✅ |
| Phase 32A — Unified quota and capacity model | 21/21 ✅ |
| Phase 32B — Quota telemetry sources | 14/14 ✅ |
| Phase 32C — Subscription capacity estimation | 11/12 |
| Phase 32D — Normalized remaining-capacity score | 11/12 |
| Phase 32E — Burn rate and exhaustion forecasting | 10/10 ✅ |
| Phase 32F — Protected quota reserve | 8/8 ✅ |
| Phase 32G — Provider-aware request-cost estimation | 9/10 |
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
| Phase 35B — Candidate scoring | 25/25 ✅ |
| Phase 35C — Capacity-aware tier escalation and downgrade | 9/9 ✅ |
| Phase 35D — Routing under subscription pressure | 8/8 ✅ |
| Phase 36 — Session affinity | 8/8 ✅ |
| Phase 37 — Basic session-aware router | 11/11 ✅ |
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
| Phase 51 — Evaluation hooks | 24/37 |
| Phase 52 — Criteria before adding semantic/vector retrieval (deferred experiment gate) | 6/6 ✅ |
| Phase 53 — Criteria before adding graph storage (deferred experiment gate) | 5/5 ✅ |
| Phase 54 — Criteria before deeper cmux coupling | 4/4 ✅ |
| Phase 54A — Setup and portability completion criteria | 10/10 ✅ |
| Phase 55 — V1 completion definition | 23/23 ✅ |
| Phase 56 — Harness–subscription decoupling: choose the harness, route the subscription and model | 12/12 ✅ |
| Phase 56A — Entitlement pool and subscription broker: several accounts, one scheduler | 13/13 ✅ |
| Phase 57 — Context firewall: tool-output compaction between harness and model | 27/27 ✅ |
| Phase 58 — Context economy: cache-stable translation, entitlement-aware reduction, and a measured token budget | 15/15 ✅ |
| Phase 59 — Decompression: the code's physical shape catches up with its architecture | 8/8 ✅ |
| Phase 60 — Parallel-session file coordination | 16/16 ✅ |
| Phase 61 — pane: the first-party harness | 20/35 |
| Phase 62 — Parallel-session coordination, second slice: queueing, co-editing, drift, in-turn diagnostics | 0/14 |
| Phase 63 — pane's terminal interface | 0/5 |
| Phase 64 — pane: subagents | 0/5 |
| Phase 52 — Criteria before adding semantic/vector retrieval (deferred experiment gate) | 6/6 — deferred gate |
| Phase 53 — Criteria before adding graph storage (deferred experiment gate) | 5/5 — deferred gate |

</details>
<!-- progress:end -->

## Where `pane` stands

`pane` is a second crate with its own binary, joined to Glasshouse by protocol
rather than by linkage. Neither side depends on the other at compile time.
Glasshouse reaches it exactly as it reaches Claude Code — a declared executable,
declared arguments, a PTY. `pane` reaches Glasshouse the way any harness does: a
gateway base URL, an MCP endpoint, a hook command, each one optional. Standalone
is simply the mode where nobody answers on the socket.

It runs. It reads your `CLAUDE.md`, `AGENTS.md`, `.claude/settings.json` hooks
and permissions, `.claude/commands` and skills with nothing edited. It works a
real task to a real result, keeps a plan it writes itself, runs background jobs
and monitors whose completions arrive as one batched event rather than one turn
each, resumes from a rollout file, and is watched by a cheaper model that catches
a planted three-turn loop within two turns.

**What is not built, stated plainly:**

- **The comparison that would justify the whole thing.** The ruler scores per
  workload tier and refuses to measure tokens per turn, but the two-column run
  against Claude Code on a fixed task set has not happened. Every performance
  claim here except the 205-token one is therefore architecture, not evidence.
- **MCP tools are read, not callable.** `.mcp.json` is parsed and its permission
  patterns compile into the sandbox profile; no server is connected yet.
- **Subagents.** Planned, and `pane`'s rather than Glasshouse's: a subagent is
  spawned by one session and returns into it, which is inside-a-session work.
  Glasshouse's delegation is between peer sessions you can see, which is a
  different thing.
- **Standing handlers and a session inbox.** The batch machinery they would ride
  is built; these two are not.
- **The terminal interface.** Turn blocks, a persistent input area, slash-command
  completion and a status line are being built now.

The success criterion is deliberately narrow: *on at least one workload tier,
measured over completed tasks rather than turns, `pane` beats the adapter path on
tokens or wall-clock without losing on outcome.* One tier is enough. A head-on
comparison against harnesses better resourced and better tuned against their own
models is a bet lost slowly, and it is not the bet being made.

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

The harness, on its own — an API key in the environment is the only requirement:

```sh
pane session --root .              # work tasks from stdin in this project
pane session --root . --task "…"   # one scripted task, non-interactive
pane session --root . --yolo       # grant the project root and every command
```

The control plane, when you want routing, memory and cost:

```sh
glasshouse                    # operate on the current project
glasshouse launch             # start a harness session, gateway and all
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

#### Provider keys

A provider key is filed, not exported. `glasshouse credentials store <VAR>`
prompts for the value — never on the command line — and puts it in the operating
system's own secure store, where the harness and its hooks can read it. A key
exported only in your shell reaches the launcher and stops there.
`glasshouse credentials list` shows where each configured variable resolves
from, and reads no value to do it.

## License

MIT OR Apache-2.0
