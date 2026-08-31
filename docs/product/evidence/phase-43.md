# Capability evidence — phase 43

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 43 — MCP surface for orchestrators, 10 of 10 (lines 1694–1703), plus Phase 46's line 1746

Package `GH-MCP-SERVER`, 2026-08-31, Fable specialist at xhigh effort. **Eleven
lines against one mechanism**: a stdio JSON-RPC 2.0 / MCP (2025-06-18) server
whose eight tools are each a thin adapter onto one `api::protocol::Request`
variant, answered by the same `dispatch` the Unix-socket door answers with.

Contract: Given a compatible orchestrator harness configured to start
`glasshouse mcp serve` inside a project, when it calls a Glasshouse tool,
Glasshouse performs exactly the operation the local API already performs for
that request — listing, spawning, messaging, status, interruption, memory
search, checkpoint retrieval — bound to the one project the server was started
in, while preserving the rule that every state-changing operation is its own
explicitly named tool whose description and annotations say so, so the
harness's native permission controls can gate it.

State: **COMPLETE** for 1694, 1695, 1696, 1697, 1698, 1699, 1700, 1701, 1702,
1703 and 1746 — with one recorded platform limit (below) that the next
`ci-local.sh --windows-vm` run owes.

**The design ruling this rests on** is in `docs/product/design-decisions.md`,
*"Phase 43: the MCP surface is a transport over the existing API door"*: hand-
rolled JSON-RPC over stdio on `serde_json` (no new dependency), every tool a
`Request` variant through the shared dispatch, dangerous operations as separate
named tools, origin always `Machine`, one project per process. The register's
Cluster R had parked Phase 43 as *"a design decision for the user"*; the
decision is made from the tree — nothing is exposed that `glasshouse api serve`
did not already expose.

Production evidence:
- `crates/glasshouse/src/api/mcp.rs` (new) — `serve`, `serve_frames`,
  `handle_frame`, `dispatch_method`, `call_tool`, `TOOLS`. Eight tools:
  `glasshouse_list_sessions` → `ListSessions`; `glasshouse_session_status` →
  `SessionState`; `glasshouse_recent_output` → `RecentOutput`;
  `glasshouse_spawn_session` → `SpawnSession`; `glasshouse_send_message` →
  `SendMessage { origin: Machine }`; `glasshouse_interrupt_session` →
  `Interrupt { origin: Machine }`; `glasshouse_search_memory` → `QueryMemory`;
  `glasshouse_get_checkpoint` → `GetCheckpoint`.
- `crates/glasshouse/src/api/unix.rs` — **`ServerContext`**, the seam that
  makes the boundary structural: it owns the six things `dispatch` needs plus
  the background tick and offers one verb, `handle(Request) -> Response`. The
  socket `serve` and `mcp::serve` each hold a context and nothing else;
  `dispatch` is unchanged. `api::protocol` and `api::unix` are no longer
  `#[cfg(unix)]` as modules — the socket-specific items carry the gate item by
  item — so the stdio door compiles on Windows.
- `crates/glasshouse/src/cli.rs` — `Command::Mcp`, `McpCommand::Serve`, whose
  help text is the harness registration (a one-line `mcpServers` example) and
  says the server must be started inside the project.
- `crates/glasshouse/src/main.rs` — the `Command::Mcp` arm; and
  `routing_moment_slug` un-gated from `#[cfg(unix)]`, which the Windows
  cross-check found as a build error once the modules were un-gated.

Regression evidence (`tests/mcp_server.rs`, 5 tests; `tests/mcp_project_scope.rs`,
4 tests, all cross-platform; 3 unit tests in `api::mcp`):
- `mcp_server::an_orchestrator_can_initialize_list_tools_and_list_sessions_over_stdio`
  — the shipped binary, `initialize` → `tools/list` → `tools/call`, over its
  own stdin/stdout.
- `mcp_server::state_changing_tools_are_separate_and_marked_so_a_harness_can_gate_them`
  — the three state-changing tools carry `readOnlyHint:false`, the five others
  `readOnlyHint:true, destructiveHint:false`; descriptions lead with
  `STARTS A PROCESS` / `INJECTS INPUT INTO A RUNNING HARNESS` / `INTERRUPTS A
  RUNNING HARNESS`.
- `mcp_server::a_malformed_frame_is_answered_with_a_parse_error_and_the_server_keeps_serving`
- `mcp_server::the_server_exits_cleanly_when_the_client_closes_stdin`
- `mcp_server::live::send_message_reaches_the_session_as_a_machine_origin_message`
  (`#[cfg(unix)]`, shell-script harness) — spawn, send, status, recent output,
  interrupt, all against a live PTY; the project's event log records
  `MessageOrigin::Machine` for both the line and the interrupt.
- `mcp_project_scope::a_tool_call_naming_another_projects_session_is_refused_without_leaking_its_path`
  — two real projects; both the honest foreign id (`NotFound`) and a
  foreign-tagged row planted into the server's own file (`ForeignProject`) are
  refused on all four session tools, naming ids and never a path.
- `mcp_project_scope::memory_and_checkpoints_are_answered_only_for_the_project_the_server_was_started_in`
- `mcp_project_scope::the_mcp_layer_opens_no_store_of_its_own` — a source scan
  of `mcp.rs` for every store constructor, and the premise that it goes through
  `ServerContext::open` / `.handle(`.
- `mcp_project_scope::no_tool_argument_can_name_a_project_a_path_or_a_socket` —
  every schema property walked against a denylist; every schema
  `additionalProperties:false`; every argument type `deny_unknown_fields`.

Failure / isolation evidence — twelve mutations, twelve KILLED, each restored
byte-identical (`scripts/mutate.sh`):
- **Scope check disabled** (`session/api.rs::resolve`, `if false && …`) →
  `a_tool_call_naming_another_projects_session_is_refused_…` FAILED on the
  planted-row half: *"a row tagged with another project must be refused even
  from this project's own file"*. The honest half still passed under the
  mutation — which is exactly why the planted half exists.
- **Read-only annotation flipped** → `state_changing_tools_are_separate_…`
  FAILED: *"`glasshouse_get_checkpoint` only reads and must say so"*.
- **Origin forged** (`Machine` → `User`) on send and on interrupt → the live
  test FAILED reading the event log: `left: UserKeystroke, right: Machine`.
- **`deny_unknown_fields` removed** → an `"origin":"user"` argument was
  answered instead of refused with `-32602`.
- **`additionalProperties` opened** → `no_tool_argument_can_name_…` FAILED.
- **Tick call deleted** (`context.start_tick()`, the call itself — §35) →
  `worker_wakeup` 7 of 7 FAILED: after the refactor there is one tick for both
  doors, and the socket door's wake-up suite watches it.
- **Wrong request mapping / wrong argument forwarding**, one per tool line
  (1695, 1696, 1698, 1700, 1701) → each killed by the test that exercises that
  tool, failure text quoted in the report.

Gates: fmt clean; clippy `-D warnings` clean; `cargo doc` with
`RUSTDOCFLAGS=-D warnings` clean; `mcp_server` 5/5, `mcp_project_scope` 4/4,
`api_event_log` 8/8, `project_isolation` 7/7, `api::mcp` unit 3/3;
`blast-radius.sh` every traced target passed (50 targets; lib 1577 passed);
`cargo check --target x86_64-pc-windows-gnu -p glasshouse --all-targets` exit 0
with zero warnings.

Recorded limits — stated by the worker before anyone asked:
- **Windows: compiled, not run.** Both new test files compile for
  `x86_64-pc-windows-gnu`; they have not executed on Windows. The live-harness
  test is `#[cfg(unix)]`. **The next `scripts/ci-local.sh --windows-vm` run is
  the evidence this entry lacks**; a red there re-opens whichever line it names.
- **No real harness was registered against it.** The consumer in every test is
  the test's own JSON-RPC client over the shipped binary's stdio. The help text
  gives the registration; the first real Claude Code / Codex registration is a
  probe worth running.
- `initialize`-before-`tools/*` ordering is not enforced (the spec's SHOULD).
- The tick is watched by `worker_wakeup`, not by an MCP test; if the socket door
  ever stopped sharing `ServerContext`, the MCP tick would be unwatched.

Decisions the worker made inside the ruling, accepted: `unix.rs` keeps its name
(a rename mid-co-edit would turn `user-control`'s diff into a rewrite; the module
doc names the rename as a follow-up); batches → `-32600`; notifications never
answered; `send_message` and `interrupt` annotated `destructiveHint:true` (what a
harness does with a line is unbounded — the cautious reading is the honest one);
tool results are the handler's JSON verbatim as one text block, which is how
"credentials never appear" is kept; one stderr line at start naming the project
root. **EOF semantics corrected against the packet**: an exiting server cannot
promise a spawned harness keeps running (the kernel closes the ptys and each
harness gets SIGHUP); the documented promise is the one that holds — Glasshouse
never signals, stops or marks them.

**Co-edit note for the barrier** (§77): `user-control`'s in-flight diff threads
`muted: &Muted` as an eighth argument through `serve` → `handle_connection` →
`dispatch`. Reconciliation is three lines, not a merge of bodies: `muted`
becomes a `ServerContext` field built in `open` and passed in `handle`;
`dispatch`'s new signature and arms stay exactly as `user-control` wrote them.
