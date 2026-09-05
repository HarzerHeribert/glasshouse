# Phase −1 for a greenfield crate — a proposal

**Status: a proposal for the primary orchestrator to adopt into
`docs/product/design-decisions.md`.** Nothing here is adopted. This worker
does not write that file (its packet forbids it) and does not decide process.

## 1. The problem

`docs/process/assurance-economics.md`'s Phase −1 is a hard gate: a packet must
demonstrate, *from current production code*, that each claimed input has a
**producer**, a **caller that carries it**, a **propagation path**, and a
**consumer that can observe it**. Two packets that skipped it cost about $30
of worker compute on 2026-08-28, and `scripts/validate_round.py` enforces it
because the check is otherwise free.

`crates/pane/` is empty. Read literally, every Phase 61 line fails Phase −1 on
the first packet and passes trivially on the second, which is the worst of
both: a gate that blocks the work that needs it least and waves through the
work that needs it most.

The fix is not to exempt the crate. It is to notice that Phase 61's lines come
in **three kinds**, and that the gate's four links mean something different,
and equally checkable, in each.

## 2. The proposal: three classes, one gate

**Class A — a seam line.** Its behaviour crosses the process boundary into
Glasshouse. **Phase −1 applies unchanged.** The producer, caller, propagation
path and consumer are named in *current Glasshouse source*, by `file:line`,
and a link that cannot be shown returns the packet as premise-invalid. §3 does
this for every 61C seam line, and two of them come back with a missing link —
which is the gate working.

**Class B — a crate-internal line.** Producer and consumer both live in
`crates/pane/`. The four links become four strings, each one a thing that
exists on disk before the packet is dispatched:

| link | what it is |
|---|---|
| producer | a named section of a spec in `docs/product/pane/` — e.g. `runtime-contract.md §3` |
| caller | the module path in `crates/pane/src/` that will contain it — e.g. `crates/pane/src/runtime/preview.rs` |
| propagation | the named type or function the value travels as — e.g. `HandleTable::render` |
| consumer | the **named test** in `crates/pane/tests/` that fails when the behaviour is removed |

The spec section and the test name are the load-bearing halves: the first
means somebody decided the contract before the packet, and the second means
the packet knows what failure looks like. Both are greppable.

**Class C — a model-facing line.** Its consumer is the model, which is not a
call site. The consumer link is a **golden-file test** over the rendered bytes
(`model-contract.md` §7's worked turn is that file), the propagation is the
renderer, and the producer is the spec section. Class C is Class B with the
consumer fixed by rule, and it exists so that "the model sees it" can never be
written as a Phase −1 answer on its own.

**What `validate_round.py` can check, mechanically:**

1. the `FEASIBILITY` block names a class, `A`, `B` or `C`;
2. class A: every link is `path:line`, the path exists in `crates/glasshouse/`,
   and the line is within the file;
3. class B and C: the producer resolves to an existing heading in an existing
   file under `docs/product/pane/`; the consumer is `<file>::<test_name>` and
   the file path is under `crates/pane/tests/`;
4. class C additionally: the consumer test's name appears in a file that also
   contains the string `include_str!` or `golden`, so a golden-file claim is
   backed by a golden file.

Rule 4 is the only new machinery, and it is four lines of the validator that
already exists. Everything else is a string check on paths.

## 3. The 61C seam lines, link by link

### Seam 1 — the gateway

> *Route through Glasshouse's gateway when `ANTHROPIC_BASE_URL` names it;
> behave identically otherwise except for the hop.*

- **producer** — `Gateway::base_url()`,
  `crates/glasshouse/src/gateway/mod.rs:469`. The listener binds
  `127.0.0.1:0`; there is no configuration in that module that could bind
  elsewhere (module doc, lines 33–41).
- **caller** — `profile::resolve_with_gateway`,
  `crates/glasshouse/src/profile/mod.rs:935`. Substitutes the gateway's
  loopback address for the base URL and the gateway's token for the provider
  credential.
- **propagation** — `HarnessAdapter::direct_provider_launch`,
  `crates/glasshouse/src/harness/mod.rs:1258`, which asks the *adapter* for
  the variable name; Claude Code's is `BASE_URL_ENV = "ANTHROPIC_BASE_URL"`,
  `crates/glasshouse/src/harness/claude_code.rs:107`. pane's adapter declares
  the same name, `Verified` against its own binary.
- **consumer** — the child's HTTP client, answered by
  `crates/glasshouse/src/gateway/ingress.rs`. Complete.

### Seam 2 — which entitlement served, and what it cost

> *Show which entitlement served each request and what it cost, from the
> gateway's response and routing ledger.*

- **producer** — `Gateway::serving_provider()` (`gateway/mod.rs:540`) and
  `Gateway::quota_headers()` (`gateway/mod.rs:516`).
- **caller** — `gateway::ingress::forward`, which records the exchange.
- **propagation** — `gateway::usage::Extractor`
  (`crates/glasshouse/src/gateway/usage.rs:246`), a table-driven scan over at
  most 512 retained bytes; it reads provider-reported token counts for
  relayed exchanges as well as translated ones, under the user's ruling of
  2026-09-03 (`gateway/ingress.rs:127–133`).
- **consumer** — `routing::evidence::ledger::EvidenceLedger::record`,
  `crates/glasshouse/src/routing/evidence/ledger.rs:51`, whose
  `routing_observations` row carries `provider, model, route, quota_context,
  harness, purpose` and the three token columns. `harness` is where the string
  `pane` appears. Complete — and it is also the ruler's meter (`ruler.md` §3).

### Seam 3 — the hook protocol

> *Emit the harness hook protocol's events so Glasshouse's memory extraction,
> context firewall and event bus see `pane` unchanged.*

- **producer** — pane itself, emitting the harness's own event spellings. The
  vocabulary is fixed by Glasshouse and is PascalCase for both existing
  harnesses (`crates/glasshouse/src/session/lifecycle.rs:5–13`).
- **caller** — `glasshouse hook --session <id> --event <name>`,
  `crates/glasshouse/src/cli.rs:426`; and for tool results,
  `glasshouse context-firewall hook --session <id>`, `cli.rs:1025`.
- **propagation** — `session::lifecycle::event_for`,
  `crates/glasshouse/src/session/lifecycle.rs:49` — *"the only place in the
  crate that knows a harness's vocabulary."* The five names it translates are
  `SessionStart`, `UserPromptSubmit`, `PermissionRequest`, `Stop`,
  `StopFailure`; `SessionEnd` maps to `None` deliberately.
- **consumer** — `commands::hook::report_hook_with`,
  `crates/glasshouse/src/commands/hook.rs:255`, which drives memory
  extraction, the automatic checkpoint and the event bus; and
  `firewall::process`, `crates/glasshouse/src/firewall/mod.rs:284`, for tool
  results. Complete.

**One consequence worth a packet's attention.** `PostToolUse` is not among
`REPORTED_EVENTS` (`claude_code.rs:78`) — the lifecycle deliberately ignores
per-tool events — so pane's per-call `PreToolUse`/`PostToolUse` emission
(61E's third line) reaches the **firewall** subcommand, not the lifecycle one.
Those are two different commands with two different consumers, and a packet
that names only one has named the wrong half.

### Seam 4 — the MCP surface

> *Read memory and checkpoints from Glasshouse's MCP surface when reachable,
> and from a local store when not.*

- **producer** — `glasshouse mcp serve`, `crates/glasshouse/src/cli.rs:1137`.
- **caller** — pane's own MCP client, registered exactly as the CLI's own help
  text says (`cli.rs:1144–1147`): `{"mcpServers": {"glasshouse": {"command":
  "glasshouse", "args": ["mcp", "serve"]}}}`.
- **propagation** — JSON-RPC 2.0 over **stdio**, newline-delimited
  (`crates/glasshouse/src/api/mcp.rs:39–44`).
- **consumer** — `glasshouse_search_memory` (`api/mcp.rs:614`) and
  `glasshouse_get_checkpoint` (`api/mcp.rs:656`), both answered through the
  same `ServerContext::handle` the Unix-socket door uses. Complete — **but
  see `packet_errors`: the brief calls this "an MCP URL", and it is not a URL.
  There is no HTTP MCP transport in this build.** A pane packet that plans to
  connect to an address will find nothing listening. pane spawns
  `glasshouse mcp serve` as a child over stdio, or it has no MCP surface.

### Seam 5 — the project's own configuration files

> *Load `CLAUDE.md` and `AGENTS.md`, `.claude/settings.json` hooks and
> permissions, `.claude/commands`, the skills directories and `.mcp.json` from
> the project with nothing edited.*

- **producer** — the user's own files in the project. Not Glasshouse.
- **caller** — pane's loader. Does not exist yet; Class B.
- **propagation** — none in Glasshouse.
- **consumer** — pane. Class B.

**This seam has no Glasshouse link and it is not a defect.** Glasshouse never
reads these documents as project configuration. It *writes* adjacent ones: its
own session settings document (`SETTINGS_FILE_NAME = "claude-settings.json"`,
`crates/glasshouse/src/harness/claude_code.rs:202`, merged into one file
because a second `--settings` flag silently discards the first) and
`CLAUDE.local.md` via `glasshouse memory export-local`
(`crates/glasshouse/src/memory/export_local.rs:1–17`).

So the line is **Class B with one external invariant**: pane must not edit
what it reads, and the check for that is Glasshouse's own — a pane session must
leave `.claude/settings.json` byte-identical, and the sandbox already denies
writing it (`sandbox-grants.md` invariant 5). That is the Phase −1 answer for
this line, and it is a stronger one than a manufactured producer would be.

## 4. The four open decisions

**Single-binary promise.** `DECISION PARKED`, default taken: the README's *"no
daemon, no Node, no Python"* scopes to the `glasshouse` binary alone. pane
embeds a V8 isolate in its own binary; it is still no daemon and still no
Node runtime on the user's machine.

**TypeScript or nothing.** `DECISION PARKED`, default taken: TypeScript only.
No second generated language.

**`Vendor::Glasshouse`.** `DECISION PARKED`, default taken, and the doc
sentence is already written for it — `Vendor`'s own doc comment
(`crates/glasshouse/src/harness/mod.rs:110–117`) says a vendor is *who
publishes the executable*, deliberately not who developed or serves the model.
`Vendor::Glasshouse` on the pane adapter is exactly that and nothing more.
Note for 61B: `Vendor` and `IntegrationId` are both exhaustively matched —
`structured_pre_tool_hook` (`harness/mod.rs:1380`) is one such site and 52
files in `crates/glasshouse/src` name `IntegrationId` — so adding the variant
is a compile-error-driven propagation path, which is the cheapest kind.

**`canonical.rs` must round-trip reasoning blocks byte-identically.** This one
is **not parked; it was answered *no* when written, and the answer became *yes* on 2026-09-05** — `Block::Thinking { thinking, signature }` and `Block::RedactedThinking { data }` landed in `a4d5911` with the byte-identity round-trip test `GH-CANONICAL-THINKING` was to write; the paragraph below is kept as the record of the question and is otherwise stale.
`gateway::translate::canonical::Block`
(`crates/glasshouse/src/gateway/translate/canonical.rs:302`) has exactly four
variants — `Text`, `Image`, `ToolUse`, `ToolResult`. There is no thinking or
reasoning variant and no `signature` field anywhere in the file. Therefore:

- on the **relayed** path (Anthropic upstream), byte-identity holds trivially:
  the relay decodes nothing, and `gateway/tests.rs`'s
  `no_part_of_the_relay_deserializes_anything` keeps it that way;
- on any **translated** path, a `thinking` block with its signature cannot
  survive, because there is nothing for it to survive as.

pane's prompt cache therefore holds across the gateway hop **only while the
route relays**. A packet that assumes otherwise is premise-invalid.

**Successor packet, named as this document's obligation:**
`GH-CANONICAL-THINKING` — add `Block::Thinking { text, signature }` and
`Block::RedactedThinking { data }` to `canonical.rs`, with a byte-identity
round-trip test over an Anthropic request carrying a signed thinking block.
Amber; its decision is one enum variant and its serde shape. It is not a
Phase 61 line and it does not block 61B or 61C — it blocks any 61E claim about
cache behaviour on a translated route.

## 5. What this proposal does not decide

- **Whether `validate_round.py` changes at all.** §2's rules 1–4 are what the
  validator *could* check; whether the four lines are worth writing is the
  primary's call under CLAUDE.md's five-question test for new machinery.
- **The tiering of Phase 61's sub-phases.** 61A Amber · 61B Green · 61C Amber ·
  61D Red · 61E Red · 61F Amber is the brief's, unchanged.
- **Anything in `design-decisions.md`.** This is a proposal; the primary
  adopts, edits or refuses it.

CONTRACT
behaviour:  A Phase 61 packet declares its line's class — A (a Glasshouse seam), B (crate-internal) or C (model-facing) — and supplies four links whose form is fixed by that class, so a greenfield crate is gated rather than exempted.
invariant:  A class-A link is `path:line` in current `crates/glasshouse/` source and a missing one returns the packet premise-invalid; a class-B or C consumer is a named test that does not yet pass, never a description of one.
path:       `docs/product/design-decisions.md` (the primary adopts §2), and optionally four string checks in `scripts/validate_round.py`'s existing FEASIBILITY parser.
test:       `scripts/tests/test_validate_round.py::a_greenfield_packet_without_a_named_consumer_test_is_refused` — a fixture packet declaring class B with a consumer that names no file under `crates/pane/tests/` must exit non-zero.
