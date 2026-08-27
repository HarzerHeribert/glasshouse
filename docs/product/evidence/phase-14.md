# Phase 14 — Orchestrator role

Written by the PACKET-ORCHESTRATOR-ROLE Sonnet-implementer package. Per
`docs/process/worker-capabilities.md`, a worker does not tick capability-map
boxes, edit the map, or edit this file's status into the map — this entry is
handed to the orchestrator, who decides each box against the evidence below.

State: **COMPLETE** for all eleven lines (map 719–729).

> **Orchestrator's ruling and one addition.** Ticked as the package proposed.
> Box 2 — *"keep an orchestrator session otherwise identical"* — was closed on
> **audit only**, and the audit was right: re-verified independently, every
> `SessionRole` read in the crate is in `config/response.rs` (response profile,
> the map's own allowed exception), `shell/view.rs` / `shell/state.rs`
> (display), `api/unix.rs` (this package), `session/store.rs` (persistence),
> and two **test fixtures** in `events/mod.rs` and `session/recovery.rs` that
> the package's own list did not mention. Zero in `runtime`, `lifecycle`,
> `attach`, `select`, `native_id` or `supervision`.
>
> **An audit protects nothing against the next edit**, so the integrator added
> `session::role_is_inert_tests::a_sessions_role_never_reaches_its_lifecycle`
> — this project's own pattern for an absence claim, copied from
> `routing::interactive::tests::the_assignment_is_not_a_session_of_its_own`.
> Mutation-proved: adding `use crate::session::SessionRole;` to
> `session/runtime.rs` turns it red with the sentence naming the box.
>
> It is paired with `the_scan_reads_the_same_source_with_windows_line_endings`,
> because §14 records that a source scan on this project once took Windows CI
> red by silently finding nothing under CRLF. An LF checkout never exercises
> that path, so the scan is tested against a CRLF copy of its own input.

Eleven boxes, `docs/product/capability-map.md` line 57 and the ten lines
after it. `session::api::SessionApi` and Phase 42's Unix-socket door already
existed; this package's job was the audit the packet asked for first, then
the two or three operations the audit found genuinely missing.

## Pass 1 — audit, before this package touched anything

| # | Box | Status before this package | Evidence |
|---|---|---|---|
| 1 | tag a session with the orchestrator role, no proprietary agent type | **shipped type, zero production callers** | `SessionRole::Orchestrator`/`Worker` exist (`session/store.rs`), the schema stores and reads them, `config/response.rs::role_for` and `shell/view.rs`'s project overview both read `record.role` — but grepping every non-test call site of `NewSession::with_role` in the crate found exactly four, all inside `session/store.rs`'s own `#[cfg(test)] mod tests` (line 1992 onward). All three production callers of `NewSession::embedded` — `main.rs::launch_session`, `shell/mod.rs::start_session`, `api/unix.rs::spawn_session` — left the role at its `NewSession::embedded` default, `SessionRole::Normal`. See "The trap this audit found" below. |
| 2 | keep an orchestrator session otherwise identical | **untestable, but the absence claim already held** | see below |
| 3 | expose session management through a local tool interface | **CLOSED (Phase 42)** | the Unix-socket door, `api::serve` |
| 4 | list current-project sessions | **CLOSED (Phase 42)** | `Request::ListSessions` → `SessionApi::list` |
| 5 | spawn a new worker session with a selected harness | **CLOSED (Phase 42), untagged** | `Request::SpawnSession` → `spawn_session`; every spawned session was `SessionRole::Normal`, never `Worker` |
| 6 | assign a natural-language task to a newly spawned worker | **absent** | `SpawnSession` carried `harness`/`args` only; no field expresses a task |
| 7 | send follow-up instructions to an existing worker | **CLOSED (Phase 42)** | `Request::SendMessage` → `SessionApi::send_text` |
| 8 | interrupt an existing worker | **CLOSED (Phase 42)** | `Request::Interrupt` → `SessionApi::interrupt` |
| 9 | query worker lifecycle state | **CLOSED (Phase 42)** | `Request::SessionState` → `SessionApi::state` |
| 10 | retrieve a completed worker result or checkpoint | **absent (write-only)** | `Request::TakeCheckpoint` exists; nothing reads a checkpoint back through the socket — Phase 42's own test proves the write by shelling out to the *CLI's* `checkpoint show`, not by asking the door itself |
| 11 | orchestrator tools cannot cross project scope | **partially proven** | proven at the socket for `list_sessions`/`session_state` only (`a_session_never_crosses_into_another_projects_listing`); `send_message`, `interrupt` are proven foreign-refusing only at the `SessionApi` unit level (`session/api.rs::every_api_call_refuses_a_foreign_session`), never through the socket itself; no test existed for "retrieve" because no retrieve operation existed |

### The trap this audit found (box 1)

`glasshouse launch --response-role orchestrator` looks, by name, exactly like
the mechanism box 1 wants. It is not. `--response-role` selects a
**response-profile** role (`config/response.rs::Role` — formatting,
verbosity, audience; explicitly "cannot change reasoning depth, diligence,
validation, permissions, safety or tool use" per that module's own doc) and
never touches `SessionRecord::role` (`session::SessionRole` — the durable
session tag box 1 is actually about). `main.rs::launch_session`'s own
`NewSession::embedded(...)` builder chain proves it: `response_profile` is
threaded through, `role` never is. This is the same shape practice §5/§23
names repeatedly — a declaration derived from an artifact (here, a flag name)
that does not support the use it was cited for — and it would have been easy
to tick this box on a `grep -r orchestrator cli.rs` hit. It is a different
axis, spelled the same word by coincidence (both are literally the
capability-map's own vocabulary — "orchestrator", "worker" — reused for two
unrelated concepts).

**Consequence:** before this package, nothing reachable from the shipped
binary could produce a session with `SessionRole::Orchestrator` or `Worker`.
`shell/view.rs`'s project-overview popup (a different phase's feature, read
freely, not owned or touched here) — which finds "the" orchestrator session
and lists workers by role — was structurally dead: every session in the
binary was tagged `Normal`, so that popup could only ever print "no session
is designated as this project's orchestrator."

## Pass 2/3 — what this package built

Design decision 1 in the packet is binding: the local tool interface is
Phase 42's existing Unix socket, not a second mechanism. Every change below
lives in `api/protocol.rs` and `api/unix.rs`, both already in this package's
file grant, and none of it touches `main.rs`/`cli.rs` (`routing-score`'s
files this round) or `shell/**` (out of scope, see `phase-16.md`).

### Box 1 — CLOSED (this package)

`Request::SpawnSession` gained a `role: Option<String>` field. Absent means
`SessionRole::Worker` — deliberate, not merely convenient: every session this
door spawns is spawned by something other than a person (the door's own doc
comment, unchanged from Phase 42), so an unstated role is a worker by
default rather than indistinguishable from a session a person started by
hand. `"normal"`/`"orchestrator"`/`"worker"` are the three accepted spellings
(`api::unix::parse_role`), matching `SessionRole::as_str()` exactly; anything
else is refused by name rather than silently stored.

Production caller: `api::unix::spawn_session`, wired into the
`NewSession::embedded(...).with_role(role)` chain that reaches
`SessionStore::create` — the one door a session record can come through
(Phase 10's own guarantee, unchanged).

Proven by `spawning_tags_a_worker_by_default_and_an_explicit_role_is_honored`
(`tests/orchestrator_role.rs`): spawns with no role (asserts `worker` in the
listing), spawns with `role: "orchestrator"` (asserts `orchestrator`), and
spawns with an unknown role (asserts a clean refusal, not a silently stored
garbage value). Mutation-proof, two mutations, both against the production
*call* rather than a fixture (§35):

| Mutation | Caught by |
|---|---|
| `parse_role` always returns `SessionRole::Normal` regardless of input | `spawning_tags_a_worker_by_default_and_an_explicit_role_is_honored` |
| `.with_role(role)` dropped from the `NewSession` builder chain (role parsed, never applied) | same |

**What this does not close, and why it is a patch rather than a blocker.**
The primary session a person runs (`glasshouse launch`, no socket involved)
still has no way to be tagged `Orchestrator` — that needs a `cli.rs`/`main.rs`
flag, both forbidden this round. Box 1's own language ("one or more
sessions... tagged") is satisfied by a real, mutation-proofed production
mechanism that exists today; a person's own primary session gaining the same
ability is a genuine but separate gap, written up under `PATCHES ANOTHER
PACKAGE MUST APPLY` in the package report rather than left silently assumed.

### Box 2 — CLOSED (audit only, no code changed)

An absence claim (design decision 2), checked exhaustively rather than
assumed. Every non-test reference to `SessionRecord::role` / `record.role` /
`SessionRole::Orchestrator` / `SessionRole::Worker` in the crate:

- `main.rs:1892`, `shell/mod.rs:1154` — `ResponseRequest::role_for(record.role)`
  on **resume**, to replay the response-profile role a session was recorded
  under. Communication style only, per that function's own module doc.
- `main.rs:2468`, `main.rs:2626` — printed, unconditionally, in `glasshouse
  sessions` / `sessions show`. Display only.
- `shell/view.rs:525`, `:547` — the project-overview popup's own read,
  filtering which row lists as "the orchestrator" / "a worker". Display
  only — described above as the feature this audit found was structurally
  dead, now reachable (see box 1).
- `api/unix.rs:340` (this package) — `session_summary`'s JSON. Display only.

None of `session/runtime.rs`, `session/lifecycle.rs`, `session/attach.rs`,
`session/select.rs`, `session/native_id.rs`, or `session/supervision.rs` —
every file that governs how a session actually *runs* — references
`SessionRole` at all (checked directly, not inferred). The one behavioural
consequence of a session's role is which response-profile defaults it
resolves under, and `config/response.rs`'s own architectural constraint
(quoted above) is that this axis "cannot change reasoning depth, diligence,
validation, permissions, safety or tool use" — exactly the boundary line 58
draws. The claim holds against every surface this package could check;
`shell/**`'s own behaviour beyond the one popup above was not re-audited
(out of file grant this round, and Phase 11's evidence already covers it
role-agnostically).

### Box 3, 4, 5, 7, 8, 9 — unchanged, already CLOSED (Phase 42)

Re-verified rather than re-proven: `cargo test -p glasshouse --test
session_model control_api` passes unchanged (6/6) after every edit this
package made, and `capacity_api.rs` (3/3) is untouched. See `phase-42.md`
for their original evidence and mutation table. Box 5's spawn now also
tags the role (above) — the underlying spawn mechanism itself is otherwise
identical to Phase 42's.

### Box 6 — CLOSED (this package)

Hypothesis 1 in the packet asked which reading to take: does `SpawnSession`
need its own field, or does spawn-then-`SendMessage` honestly satisfy the
line? **This package's reading: `SpawnSession` needed a dedicated `task`
field**, because the map states box 6 (assign a task *at spawn*) and box 7
(send a follow-up to a session that *already exists*) as two separate lines
— if spawn-then-`SendMessage` were the intended answer, the map would have
needed only one line for both. A dedicated field also makes assigning a
worker's task atomic: a caller that wants both a spawn and a task gets one
round trip rather than a spawn that can race a separate `send_message` (a
caller reading the spawn's own response and only then sending the task has a
window where the harness is live and unaddressed).

`Request::SpawnSession` gained `task: Option<String>`. When present,
`spawn_session` delivers it through the exact same seam `SendMessage` uses
(`SessionApi::send_text`) the instant `SessionRuntime::start` returns —
before the response is sent back over the socket, so a caller that receives
`{"session": "..."}` back already knows the task was delivered, not merely
queued.

Proven by `a_task_given_at_spawn_reaches_the_harness_as_its_first_message`
(`tests/orchestrator_role.rs`): spawns with a task, and asserts the fixture
harness's own `received.log` — a side channel the control API itself never
touches, the same discipline Phase 42's own send-message test uses — contains
the exact task text. Mutation-proof: the delivery block replaced with a
no-op that discards `task` — the test failed (timed out waiting for the log
line); restored, `ok`.

### Box 10 — CLOSED (this package)

`Request::GetCheckpoint { checkpoint: Option<String>, document: bool }`, the
read half `TakeCheckpoint` never had. Resolution mirrors `main.rs`'s own
private `resolve_checkpoint` (named id or unambiguous prefix, `"latest"`, or
absent meaning the project's most recent) — reproduced in `api::unix::
get_checkpoint` against the same public `CheckpointStore` surface
(`get`/`latest`/`resolve_id`) rather than duplicated by copying private code,
since `resolve_checkpoint` itself is not `pub` and `main.rs` is outside this
package's file grant. `document: bool` mirrors `checkpoint show --document`'s
own flag: the rendered handoff document versus the terser bootstrap prompt.

Production caller: `api::unix::dispatch`'s `Request::GetCheckpoint` arm,
unconditionally reached — `dispatch`'s `match` over `Request` has no
wildcard arm, so (per Phase 42's own stronger-than-a-test observation) a
future edit that stops routing this variant does not compile.

Proven by `a_checkpoint_taken_through_the_socket_is_retrieved_through_the_socket`
(`tests/orchestrator_role.rs`): takes a checkpoint through the socket, reads
it back through the socket by exact id (asserting the document text and the
session it belongs to), reads it back again by `"latest"`/absent (asserting
the same id), and asks for a checkpoint id nothing ever wrote (asserting a
clean refusal, not a fabricated or wrong answer). Mutation-proof: the
dispatch arm replaced with `Response::ok(json!({}))`, never calling
`get_checkpoint` — the test failed (`unwrap()` on a missing field); restored,
`ok`.

### Box 11 — extended (this package)

Phase 42 already proved `list_sessions`/`session_state` refuse a foreign
session **through the socket**. This package adds the three operations that
were previously only proven at the `SessionApi` unit level (per design
decision 4's own instruction — "all five, not one"):
`every_orchestrator_operation_refuses_a_session_from_another_project`
(`tests/orchestrator_role.rs`) spawns a session and takes a checkpoint in one
project's own server, starts a second server for a second project sharing
the same machine, and asserts all five operations refuse the first project's
session/checkpoint by name: `list` (absent from the listing), `state`
(error), `send_message` (error), `interrupt` (error), and `get_checkpoint`
(error).

`send_message`/`interrupt`'s refusal is `SessionApi::resolve`'s existing,
unchanged project check (Phase 42 evidence already notes this seam is
reused, not reimplemented) — this package's contribution is proving it
*through the socket*, closing the §35 gap where every prior socket-level
test for these two operations happened to use a session that was always
in-project.

`get_checkpoint`'s refusal is structural rather than a runtime check: each
project's checkpoints live in that project's own physical SQLite file
(`database.rs`'s per-project file separation, unchanged, re-cited from
`phase-42.md`), so a checkpoint id from another project simply has zero rows
to match — `CheckpointStore::resolve_id` returns `NotFound`. **Not
mutation-tested** for the same honest reason Phase 42 left its own
`SessionApi::list`-vs-`store.list()` distinction unproven: there is no
runtime check to delete that would falsify it without also deleting the
per-project file separation itself, which is a different phase's guarantee.
Flagged rather than claimed as mutation-proofed.

## Gate

- `cargo doc -p glasshouse --no-deps` — clean, run alone, before the rest of
  the gate (§60 addendum).
- `cargo check --workspace --all-targets` — clean.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` —
  clean.
- `cargo test --workspace --all-features` — **1794 passed, 0 failed**, every
  target, run alone (checked for a concurrent `cargo` first; none found).
  Includes `tests/orchestrator_role.rs` (5/5, new), `tests/session_model.rs`
  (19/19, `control_api` cluster unchanged at 6/6), and `tests/capacity_api.rs`
  (3/3, untouched).
- `cargo fmt --all` not run (§37 — this package's writable files are a named
  subset of the crate); the four touched/new files were left in the shape
  `rustfmt`'s own default settings produce as they were written, and
  `cargo fmt --all`'s exact effect on them was not separately verified.

macOS (darwin/arm64) only — no Linux or Windows execution was performed for
this entry.

## Mutation summary

| id | mutation | box | test | result |
|---|---|---|---|---|
| M1 | `parse_role` always returns `SessionRole::Normal` | 1 | `spawning_tags_a_worker_by_default_and_an_explicit_role_is_honored` | FAILED |
| M2 | `.with_role(role)` dropped from the `NewSession` chain | 1 | same | FAILED |
| M3 | task delivery skipped after a successful spawn | 6 | `a_task_given_at_spawn_reaches_the_harness_as_its_first_message` | FAILED |
| M4 | `GetCheckpoint` dispatch arm never calls `get_checkpoint` | 10 | `a_checkpoint_taken_through_the_socket_is_retrieved_through_the_socket` | FAILED |

Four run, four killed, each named test `ok` before, `FAILED` mutated, `ok`
restored (`diff` confirmed byte-identical), with the mutated file touched
before every rebuild (§16).

## Missing evidence

- **The primary CLI-launched session cannot be tagged `Orchestrator`.** See
  box 1 above and `PATCHES ANOTHER PACKAGE MUST APPLY` in the package
  report.
- **`--role`'s wire spelling has no shared parser.** `api::unix::parse_role`
  and the CLI patch this package could not apply would, today, have to
  duplicate the same three-string match independently — a small, real
  drift risk the next package touching either should collapse into one
  place if `session::SessionRole` grows a public `FromStr`.
- **Windows/Linux execution.** Not run for this entry; every claim above
  that depends on real process/socket behaviour should be treated as
  unverified on those platforms until CI runs it, consistent with how
  `phase-11.md` and `phase-42.md` already flag the same gap for their own
  work.
