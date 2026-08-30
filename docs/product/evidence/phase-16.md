# Phase 16 — Worker transparency

Written by the PACKET-ORCHESTRATOR-ROLE Sonnet-implementer package. Per
`docs/process/worker-capabilities.md`, a worker does not tick capability-map
boxes, edit the map, or edit this file's status into the map — this entry is
handed to the orchestrator, who decides each box against the evidence below.

State: **COMPLETE** for map lines 744, 749 and 750. **NOT STARTED, blocked**
for 745, 746, 747 and 748.

> **The finding is sharper than "it needs the TUI", and it is a design question
> the next round must answer before it can write code.** Phase 11 already built
> the entry mechanism — `Enter` on the session overview — and **gated it to
> `SessionPresentation::Embedded`. Every worker this door spawns is
> `Headless`.** So the existing mechanism does not merely fail to cover these
> sessions; it *actively refuses exactly the sessions this phase is about*.
>
> Lines 746 and 747 are blocked behind 745 for the same reason. Line 748 has its
> raw material — `MessageOrigin::Machine` / `UserKeystroke` are recorded — and
> no read path that surfaces an intervention to an orchestrator.
>
> **The decision to take first: does an orchestrated worker become `Embedded`,
> or does the overview learn to attach to a `Headless` one?** That is a product
> question about what a "real session the user can inspect" means for a session
> nobody is looking at, and it is not a Sonnet's to settle.
>
> **Integrator's follow-up, and it makes the question bigger rather than
> smaller.** Two facts, both checked rather than assumed:
>
> 1. **`glasshouse api serve` is a separate process from the TUI.** It is its
>    own top-level command (`main.rs`'s `Command::Api` arm), and nothing in
>    `shell/**` starts or reaches it. A worker spawned through the socket lives
>    in the API server's `SessionRuntime`; the TUI is a different process and
>    cannot render a pty it does not own.
> 2. **`session/attach.rs` cannot be reused for this.** Its own module doc says
>    so in as many words: *"Nothing here is reusable from inside a longer-lived
>    interface; the session runtime that multiplexes several harnesses needs a
>    different input path, not this one."* It is a whole-process transparent
>    bridge that owns the terminal for the life of the process, deliberately.
>
> So the overview's refusal — *"there is no viewport to focus into"* — is not a
> gate somebody forgot to relax. **It is an accurate statement about where the
> worker's pty actually lives**, and neither option is small:
>
> - **(a) an orchestrated worker becomes `Embedded`** — which means the socket
>   door must spawn into a runtime the TUI owns, i.e. the two processes become
>   one, or learn to share;
> - **(b) the TUI learns to attach to a running headless session** — which means
>   a pty handed between processes, and `attach.rs` says the input path for that
>   does not exist.
>
> **This is Red tier** (`worker-capabilities.md`: PTY lifecycle, process
> ownership, cross-process state) and it is a product decision before it is an
> implementation one: *what does "a real session the user can inspect" mean for
> a session whose process nobody is attached to?* Recorded here rather than
> guessed, because a Sonnet package sent at these four boxes would either invent
> an architecture or stall, and both cost a round.
>
> Lines 749 and 750 close on audit — no LLM client is reachable from
> `api::unix` or `session::api`; every request routes to a real
> `SessionRuntime` and a real pty. Line 744 closes on the stronger evidence: a
> **wholly separate `glasshouse sessions` process** reads what the socket
> spawned.

Seven boxes. Hypothesis 2 in the packet asked, before anything else, whether
these boxes have a production surface at all from this package's file grant
— `shell/**` is forbidden this round — and whether the answer changes box by
box. It does.

**Headline finding: box 1 does not need the TUI, and closes this package.
Boxes 2–5 do need it, and one of them (box 2) needs more than access — Phase
11 already built a mechanism that actively excludes exactly the sessions box
2 is about. Boxes 6–7 are structural claims already true, checked and cited
below, not built.**

## Box 1 — CLOSED (this package)

*"Ensure every worker created by an orchestrator appears immediately in the
normal Glasshouse session list."*

"The normal Glasshouse session list" is not the orchestrator's own view
(Phase 14 box 4, `list_sessions` through the socket) — it is the surface a
person runs: `glasshouse sessions`. Both read `SessionStore::list` against
the same project database file (`session/store.rs`, unowned by either
listing surface), so nothing structurally *could* separate them — but that
was an inference until tested against two genuinely separate processes.

Proven by
`a_worker_spawned_through_the_socket_appears_in_the_ordinary_sessions_listing`
(`tests/orchestrator_role.rs`): a worker is spawned through `glasshouse api
serve`'s socket in one process, and a **wholly separate** `glasshouse
sessions` invocation — a fresh process, same `--scope`/`--data-dir`/
`--config-dir`, no socket involved — is run afterward and asserted to list
it, role and all. "Immediately" is proven by there being no synchronization
step between the two calls beyond the spawn's own response: the CLI process
did not exist yet when the socket call returned.

Not mutation-tested beyond what box 1 of `phase-14.md` already covers: the
worker's role appearing correctly in this same listing is the identical
production path (`SessionStore::create` → `record.role.to_string()` in
`main.rs`'s row-printing, unowned and unedited by this package) that mutation
M1/M2 in `phase-14.md` already kills.

## Box 2 — NOT CLOSED, and not merely "needs the TUI"

*"Allow the user to enter any orchestrated worker while it is running."*

This needs `shell/**` (forbidden), confirming hypothesis 2's premise — but
reading `shell/**` (permitted; only editing is forbidden) found something
sharper than "no mechanism exists yet." One already does, and it refuses
exactly the sessions this box is about.

Phase 11 (`phase-11.md`, line 687) built "focus" — the overview's `Enter`
key, `ShellState::focus_overview_target` — and gated it, deliberately, on
`session.presentation == SessionPresentation::Embedded`. Every session this
package's door spawns is `SessionPresentation::Headless`
(`api::unix::spawn_session`, unchanged by this package, matches Phase 42's
own deliberate choice). **A worker spawned through the orchestrator's door is
therefore refused by name if a person tries to focus it from the overview
today** — not because nothing tries, but because Phase 11's own refusal,
built for a different reason (distinguishing "bring into the viewport" from
"this session has nowhere to be presented"), fires on it.

This is not a defect in Phase 11 — box 687 never claimed to cover a
headless session, and headless-vs-embedded is a real, load-bearing
distinction elsewhere in the codebase. It is a scoping fact the next package
that owns `shell/**` needs before touching `focus_overview_target`: closing
box 2 is not "add a key", it is "decide what focusing a *headless* session
even means" (there is no embedded viewport slot such a session already
occupies — focusing it would have to *change* its presentation, which is a
real design question, not a one-line gate change) — genuinely `gateway`/
gateway-shaped architecture-adjacent, in the sense practice §36 means: a
consumer being built by someone else is not automatically a consumer of your
policy, and here the existing consumer is actively the opposite of one.

**Left open.** The patch this needs is not named under `PATCHES ANOTHER
PACKAGE MUST APPLY` because it is not a small, mechanical addition — it is a
design decision (per worker-capabilities.md's own boundary: a Sonnet
implementer stops short of new architecture) that belongs to whichever
package next owns `shell/**` with the authority to make it.

## Boxes 3, 4, 5 — NOT CLOSED, need the TUI, no sharper finding

*"Allow direct user input to an orchestrated worker without requiring the
orchestrator as an intermediary." / "Allow the user to interrupt an
orchestrated worker directly." / "Record user intervention so the
orchestrator can be informed that the worker state may have changed."*

All three presuppose box 2 (a person is already looking at the worker's own
viewport) or a comparable TUI-side mechanism this package's file grant does
not reach. `shell/state.rs`'s existing `c` (interrupt) and message-sending
paths (Phase 11's `enter_session_mode`/send-text machinery) already operate
on *whichever session is presented* — but "presented" is exactly what box 2
cannot yet do for a headless worker, so these three inherit that same
blocker rather than adding one of their own. Box 5 in particular
("record user intervention") has no event-log shape decision made anywhere
in this package's file grant to build against; `events::MessageOrigin`
(`session/api.rs`'s own seam, Phase 14 box 2's evidence) already
distinguishes `Machine` from `UserKeystroke` at the point of delivery, so the
raw material for "was this a person or the orchestrator" exists — but
*surfacing* that to the orchestrator (a read path, not the write path this
package owns) is undesigned.

**Left open**, for the same package that resolves box 2.

## Box 6 — CLOSED (audit only, no code changed)

*"Never implement orchestration workers as hidden in-process LLM calls when
a native harness session was requested."*

Structural, per design decision 3: proven the same shape
`routing/interactive.rs` already uses — a test that scans source for a
forbidden pattern and fails the build if it appears, rather than a runtime
assertion. `api::unix::spawn_session` (this package's own file) has exactly
one production path to a running worker: `SessionRuntime::start`, which
spawns a real OS process through `HarnessLaunch` (`launch.rs`, unowned,
re-verified by reading it: `spawn()` calls through to `portable-pty`'s
`CommandBuilder`/`PtySystem`, never an in-process model call). There is no
branch anywhere in `api/unix.rs` or `session::api::SessionApi` that answers a
request with generated text instead of routing to a live session — every
`Request` variant either reads `SessionStore`/`CheckpointStore` state or
writes to a `SessionRuntime`-held process.

Grepped the whole crate for anything an in-process LLM call would need
(an HTTP client constructed anywhere near `session::api` or `api::unix`, an
`anthropic`/`openai`-shaped request builder reachable from either): nothing
found. `provider::` clients exist for routing/gateway purposes elsewhere in
the crate, entirely unreferenced from this package's files.

## Box 7 — CLOSED (audit only, no code changed)

*"Preserve the rule that every worker remains a real session the user can
inspect."*

Same evidence as box 6, from the other direction: every session
`api::unix::spawn_session` creates goes through `SessionStore::create` (the
one door a session record can come through, Phase 10's unchanged guarantee)
and is therefore listed, showable, and closeable through every ordinary
surface — proven directly by box 1's own test above, which is exactly "the
user can inspect it" made concrete: a wholly separate `glasshouse sessions`
process sees it. Box 2's gap (above) means a person cannot yet *enter* it
interactively, but "inspectable" (listed, its state queryable, its harness
and role visible) does not require that — box 685's own reading in
`phase-11.md` draws the same distinction between a fact being *shown* and a
session being *actionable* from a given surface, and applies it the same
way here: box 7 asks for visibility, not for the interactive-entry
capability box 2 is separately about.

## Gate

Same run as `phase-14.md` — this entry adds no new tests of its own beyond
the one cited for box 1, already counted there. No file this entry's claims
depend on was edited by this package; boxes 6 and 7's evidence is entirely
read-only inspection of `api/unix.rs` (owned, unchanged in the relevant
paths) and `launch.rs`/`session::api` (read-only, per the packet's grant).

## Missing evidence

- **Boxes 2–5 need a `shell/**`-owning package with room to make a design
  decision**, not merely file access — see box 2 above for exactly what
  that decision is.
- **Box 5's event-log shape for "the orchestrator learns a worker's state
  may have changed"** is undesigned; `MessageOrigin` gives raw material,
  nothing surfaces it as a read path today.
- **Windows/Linux execution.** Not run for this entry, same caveat as
  `phase-14.md`.

---

## Phase 15 lines 733-739 — CLOSED, batch 49 (team lead, two subcontractors)

Seven of twelve closed; 740, 745, 746, 747 and 748 returned.

**The ruling, first.** The orchestrator can be woken and act, *and* the user can
have already moved the worker underneath it. Those contradict only if the
notification claims to describe the worker **now**. It does not: it is a
statement about the past, and the orchestrator re-reads current state through
the door before acting.

**734 was the missing wire, and the gap was real.**
`api::unix::spawn_session` was **the one launch path in the binary that
installed no lifecycle hooks** — `main.rs::launch_session` always has; this
door never did. So an orchestrator's own worker was the only kind of Glasshouse
session that could finish a turn and leave no trace of finishing. The producer
for this entire phase did not exist for exactly the sessions the phase is about.
`install_worker_hooks` now uses the same public seam `main.rs` uses, best-effort,
and *"when available"* is literal: a harness with no verified hook mechanism
returns no arguments and the session still starts.

Regression evidence: `tests/worker_wakeup.rs` (9 tests, through the real socket).

### M6 SURVIVED, and that was the finding rather than a weak mutation

Deleting `row.session != watch.worker` — so a watch delivers **every**
completion in the project — survived eight passing tests. Read under §80's three
questions it was **unwatched behaviour**, not an irrelevant mutation: the
command ran the tests, the target held them, and the line is on the pumped path
for every row.

Every one of those eight tests ran a single worker. An orchestrator running five
would have been told the wrong one finished — **worse than not being told**,
because it sends the orchestrator to inspect a session that is still working,
which is the exact failure this phase exists to remove. The ninth test has the
*unwatched* worker finish first, so the leak would arrive at a lower log
position and the absence is an ordering assertion rather than a race. M6b then
killed.

### The five returned

740, 745, 746, 747, 748 — the user-direct-access half. Returned with an
adversarial subcontractor having tried and failed to falsify the claim.

---

### Phase 16 — Record user intervention so the orchestrator can be informed the worker state may have changed (line 748)

State: **COMPLETE** — orchestrator ruling, batch 51.

Contract: Given a session driven through `glasshouse api serve`, when a user
intervenes, an orchestrator in another process reads that intervention back
through the same door — while the API door does not hold a write-capable SQLite
handle for its whole life.

**The defect this closed is larger than the line.** `serve` built its runtime
with `SessionRuntime::new()` — an `EventBus` with no sink — where
`shell::run` calls `attach_event_log`. So the API door wrote **nothing** to the
project event log: not interventions, not `session_started`, not
`process_exited`. Measured by the discovery package with a shipped-binary probe
plus a control.

**Both halves were needed, and that is the point.** The discovery package's
SURVIVED mutation had already proved the write half alone buys nothing: rows
appear, correctly stamped `machine`, and `Request::Events` still returns `[]`,
because `observed_since` filters `observed_harness IS NOT NULL`. So this package
had to argue what that filter was *for* before routing around it, and add
`EventLog::since` beside it rather than loosening it. **Removing either half
alone now fails a test** — which is precisely what the SURVIVED mutation could
not say. `drop-the-write-half` re-run by the orchestrator in the integrated
tree: KILLED.

**A product defect found while testing, and fixed.** `serve` printed
`control API listening on …` immediately after `bind` and **before**
`ProjectSessions::open`. Every shipped-binary test in this repository uses that
line as its ready signal and it was not one: a door that has announced can still
exit, because the database open below it can fail. The window is exactly where
`EventRecorder::attach` now sits. The announcement moved below the last thing
that can refuse to start; the socket is bound by then, so a client connecting in
the remaining window waits in the backlog rather than being refused. Diagnosed
by reading the actual failure — `EOF while parsing a value`, then the server's
stderr saying `project database … was opened read-only` — not by retrying.

**Cross-worker handoff worth recording.** `GH-WORKER-ACCESS` returned 748 open
and left a test pinning the gap, asserting the *wrong* behaviour on purpose so
the gap had a name, with the instruction that whoever closed the line must
**invert it, not delete it**. It was inverted:
`an_intervention_through_the_door_never_reaches_the_orchestrators_event_read_path`
became `..._reaches_...`. Two workers who never spoke, coordinating through a
test left as a marker.

Limits: 745, 746 and 747 remain open on a separate unmade product decision
(refusal register Cluster K) — a user still cannot *enter* an orchestrated
worker. This line is about the record, not the access.


---

# Lines 746, 747 closed; 745 refused — 2026-08-30

Package `GH-API-CLIENT`; report in `.agent-runtime/report-api-client.md`.

## The gap was a client, and it came from a test file's doc comment

`tests/worker_access.rs` opens by recording that four of Phase 15/16's five
lines were returned **premise-invalid**, with the reason stated exactly:
*"**No user surface reaches this door at all.** … `UnixStream::connect`
appears nowhere in `crates/glasshouse/src/`, and `cli::ApiCommand` has exactly
one variant, `Serve`."* Glasshouse could **answer** its control socket and could
not **knock** on it.

Both facts were re-verified before dispatch and both held.
`src/api/client.rs` is the half that knocks:

    glasshouse api send      --session <ID> --text <TEXT>
    glasshouse api interrupt --session <ID>

`api/unix.rs` and `api/protocol.rs` were **not touched** and no `Request`
variant was added — the verbs already existed and were already proven against
the shipped binary.

**That finding lived only in a test file's module doc.** It was not in the
refusal register and not in this ledger, so nobody could act on it without
reading a test they had no reason to open.

## 745 — refused, and the recorded reason is wrong

*"Allow the user to enter any orchestrated worker while it is running."* There
is no verb on this wire that returns a worker's **output**, so a client built
from the existing verbs can put input in and cannot show what came back. Adding
one was outside this packet, and the worker stopped and reported as instructed.

**But the premise underneath 745 is stale, and it is the most useful thing in
the report.** `phase-16.md` and the register's Cluster K both frame 745 as an
unmade **Red-tier** design decision — *"the worker becomes `Embedded`"* versus
*"a pty handed between processes"* — on the grounds that **no read path into a
running worker exists outside the process that owns it**.

A read path exists **inside** that process, and it has no production caller:

    src/session/api.rs:150
        pub fn recent_output(&self, id: &SessionId, max_bytes: usize) -> Result<String, ApiError>

It resolves through `SessionApi::resolve`, so it is project-scoped by the same
seam every other verb uses, and its own test asserts it refuses a foreign
session (`api.rs:727`). **Its only call sites are in its own `#[cfg(test)]`
module** — verified by the orchestrator on the integrated tree.

**So 745 is one `Request` variant away, not a Red-tier architecture decision.**
That is the next package, and it is small.

---

### Allow the user to enter any orchestrated worker while it is running. (line 745)

Contract: Given a running orchestrated worker, when the user asks to enter it, Glasshouse shows them the worker's terminal — which no request on this wire returns.

State: NOT STARTED — worker refused the line; see its reason

Recorded scope limits — stated by the worker, not discovered later:
- No `Request` variant returns a worker's terminal output, and ruling 1 forbids adding one; the packet's own stop condition was taken.
- The premise recorded in phase-16.md and refusal-register Cluster K is STALE: `session::api::SessionApi::recent_output` (session/api.rs:150) is a project-scoped read of a live worker's scrollback, inside the process that owns the pty, with NO production caller — its only call sites are its own `#[cfg(test)]` tests. 745 needs one Request variant plus one client verb, not a pty handed between processes.
- Whether a poll-based read counts as `enter` is a product judgement; `attach.rs`'s objection still stands for a transparent full-terminal attach.

---

### Allow direct user input to an orchestrated worker without requiring the orchestrator as an intermediary. (line 746)

Contract: Given an orchestrated worker running under `glasshouse api serve`, when a person runs `glasshouse api send` from their own terminal, Glasshouse delivers that exact text onto the worker's pseudo-terminal without any agent being consulted or taking a turn, while preserving project scope, the canonical-line refusal, and the absence of any filesystem path in what the person is told.

State: **COMPLETE**

Production evidence:
- `src/api/client.rs` — `send_message`
- `src/api/client.rs` — `call`
- `src/api/client.rs` — `socket_path_for`
- `src/cli.rs` — `ApiCommand::Send`
- `src/main.rs` — `run (Command::Api / ApiCommand::Send arm)`

Regression evidence:
- `worker_access::a_message_sent_by_the_client_reaches_a_real_worker_process`
- `worker_access::the_client_cannot_reach_another_projects_worker`
- `worker_access::a_line_over_the_canonical_limit_is_refused_to_the_user_and_the_session_survives`
- `worker_access::the_client_says_what_went_wrong_without_naming_a_path`
- `worker_access::the_client_finds_the_door_the_server_actually_bound`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| in send_message: `    )?;` -> `    );` (the call's Result is discarded) | `ignore-the-doors-error` | **killed** | `worker_access::a_line_over_the_canonical_limit_is_refused_to_the_user_and_the_session_survives` |
| socket_path_for: scan `state_dir().parent()` and return any other project's `control.sock` that exists before computing this project's | `cross-the-project-boundary` | **killed** | `worker_access::the_client_cannot_reach_another_projects_worker` |
| println! of the delivery line gains ` at {}`, socket_path_for(runtime).display() | `leak-a-path` | **killed** | `worker_access::the_client_says_what_went_wrong_without_naming_a_path` |
| const MAX_SOCKET_PATH_BYTES: usize = 90; -> = 200; (client always takes the preferred branch) | `drift-the-duplicated-limit-up` | **killed** | `worker_access::a_message_sent_by_the_client_reaches_a_real_worker_process` |
| const MAX_SOCKET_PATH_BYTES: usize = 90; -> = 20; (client always takes the fallback branch) | `drift-the-duplicated-limit-down` | **killed** | `worker_access::the_client_finds_the_door_the_server_actually_bound` |

> ignore-the-doors-error observed: panicked at worker_access.rs:901 — `a line the terminal cannot take must not be reported as delivered`; also failed the_client_says_what_went_wrong_without_naming_a_path and the_client_cannot_reach_another_projects_worker

> cross-the-project-boundary observed: panicked at worker_access.rs:799 — `a client scoped to beta must not deliver into alpha's worker`; only that test failed, so the mutation is precisely targeted

> leak-a-path observed: panicked at worker_access.rs:1018 — `the `ok` case named a filesystem path`; proves the no-path absence assertion is live rather than vacuous

> drift-the-duplicated-limit-up observed: 4 tests failed; the client looked in the state directory for a door the server had bound in the temp directory

> drift-the-duplicated-limit-down observed: panicked at worker_access.rs:1081; the short-path half found no door because the server had bound in its state directory

Recorded scope limits — stated by the worker, not discovered later:
- The delivery is recorded as `MessageOrigin::Machine` — session/api.rs:129 hard-wires it — so an orchestrator reading the event log cannot tell a user's intervention from its own message. Bears on already-ticked line 748. Fixing it needs session/api.rs and protocol.rs, both forbidden here.
- `client::socket_path_for` duplicates the private `unix::socket_path_for`. Proven to agree on BOTH branches against the shipped server, not deduplicated: unix.rs is forbidden. The integrator can make the original `pub(super)` and delete the copy.
- A read timeout is genuinely ambiguous about whether the text arrived; the message says so rather than guessing.
- The non-Unix refusal is compile-proven via a cfg flip, never executed on Windows.

---

### Allow the user to interrupt an orchestrated worker directly. (line 747)

Contract: Given an orchestrated worker running under `glasshouse api serve`, when a person runs `glasshouse api interrupt` from their own terminal, a real SIGINT is raised in the worker's own process by a 0x03 on its own terminal, while preserving the session — it still takes input afterwards.

State: **COMPLETE**

Production evidence:
- `src/api/client.rs` — `interrupt`
- `src/api/client.rs` — `call`
- `src/cli.rs` — `ApiCommand::Interrupt`
- `src/main.rs` — `run (Command::Api / ApiCommand::Interrupt arm)`

Regression evidence:
- `worker_access::an_interrupt_sent_by_the_client_makes_the_worker_react`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| in interrupt: `"op": "interrupt", "session": session,` -> `"op": "send_message", "session": session, "text": "",` | `interrupt-that-is-only-a-message` | **killed** | `worker_access::an_interrupt_sent_by_the_client_makes_the_worker_react` |

> interrupt-that-is-only-a-message observed: panicked at worker_access.rs:385 — `timed out waiting for the worker to handle a real SIGINT`. Read verbatim per §80 case 5: this is the assertion the test is named for, not a fixture setup guard.

Recorded scope limits — stated by the worker, not discovered later:
- The reaction is proven by the worker's own SIGINT trap. It proves a signal was raised in that process; it does not prove any particular harness's semantics for handling one.
- The interrupt is recorded as `MessageOrigin::Machine`, same limit as 746.

---

## REVIEW — the orchestrator owes an answer to each of these

This section is the point of the generator. Everything above is the
worker's facts, transcribed. Nothing below is decided.

- **745** — verdict `refused`. Confirm the worker's reason against current source before recording it.
- **746** — verdict `closed`. Re-run one decisive mutation yourself, then rule (§79: a worker's packet does not bind the integrator).
- **747** — verdict `closed`. Re-run one decisive mutation yourself, then rule (§79: a worker's packet does not bind the integrator).

**Packet errors the worker reported — read these BEFORE its results.**
Thirteen consecutive rounds a worker corrected its packet and was right:
- Acceptance test 3 required a boundary refusal `distinguishable from 'no such session'`. It is not satisfiable and should not be: each project has its own database, so another project's session is ABSENT rather than foreign, `ApiError::ForeignProject` is unreachable between two real projects, and saying more would confirm the session exists elsewhere. The test asserts the scoped sentence (`no session `x` in this project`) plus the absence of a `--socket` argument instead.
- Required mutation (a), `drop the client's project-scope check`, has no site: the client has no project-scope check. Its scope is structural — the socket is a pure function of the resolved project and there is no parameter to aim. Two substitute mutations were run against the mechanism that does carry the boundary; both KILLED.
- docs/process/refusal-register.md:267 (Cluster K) says of 746 and 747 `there is nothing to send input to, or interrupt, until a user can be in a running worker`. Both halves are now false. STALE — reported, not edited.
- docs/product/evidence/phase-16.md and refusal-register Cluster K both frame 745 as blocked on a Red-tier pty-ownership decision because no read path into a running worker exists. `SessionApi::recent_output` (session/api.rs:150) is exactly that read path, project-scoped, with no production caller. STALE — reported, not edited.
- The existing test `the_door_refuses_to_deliver_into_another_projects_worker` passes on `ApiError::NotFound`, never on `ApiError::ForeignProject`, for the same per-project-database reason. Still a real test; not testing the variant its name suggests.

Gates the worker ran (re-run the decisive ones yourself):
- cargo build: clean
- cargo fmt --all -- --check: clean
- cargo clippy --all-targets --all-features -- -D warnings: clean
- cargo test --test worker_access: 9 passed
- cargo test --test api_event_log: 6 passed
- cargo test --test project_isolation: 7 passed
- cargo test --test canonical_line_limit: 6 passed
- cargo test --bin glasshouse: 44 passed
- scripts/check-doc-boundary.sh: clean
- scripts/blast-radius.sh: every traced target passed — 40 cargo targets (--lib, 38 integration tests, --bin glasshouse) plus rustdoc clean, exit 0
- non-Unix path via §18 cfg flip across api/mod.rs and main.rs: cargo clippy --bin glasshouse -- -D warnings clean; both files restored byte-identical



---

# Line 745 — closed 2026-08-30. **Phase 16 is finished, 7 of 7.**

Package `GH-WORKER-READ`; report in `.agent-runtime/report-worker-read.md`.

`glasshouse api read --session <ID> [--max-bytes N]` is the third client verb,
and the one that turns send and interrupt into a person being *in* a running
worker rather than typing into one blind:

    ApiCommand::Read -> api::read_output -> socket -> Request::RecentOutput
                     -> SessionApi::recent_output (session/api.rs:150)

**`recent_output` now has a production caller** (`api/unix.rs:619`). Every call
site was previously in its own `#[cfg(test)]` module.

## The blocker was recorded wrong, and that is the story of this line

`phase-16.md` and the register's Cluster K both framed 745 as an unmade
**Red-tier** design decision — *"the worker becomes `Embedded`"* versus *"a pty
handed between processes"* — because no read path existed outside the process
owning the worker.

**One existed inside it, and had for some time.** `GH-API-CLIENT` found it while
correctly refusing this same line the day before, for a different and real
reason: there was no verb returning output, and its packet said stop rather
than add one. It stopped, and then checked *why* the line was open. Cluster K
is corrected.

## The four answers that must stay distinguishable

`recent_output` refuses `ApiError::NotLive` rather than returning an empty
string, because *"returning an empty string would be a lie the caller has no
way to detect"*. That distinction had to survive the wire, and the mutation
that removes it is killed:

| mutation | result | killed by |
|---|---|---|
| dispatch reads `SessionRuntime` directly, **keeping the bound**, so only the project-scope resolve is gone | **KILLED** | `another_projects_worker_cannot_be_read_and_a_crafted_id_says_the_same_thing` |
| `NotLive` answered as `{"output": ""}` | **KILLED** | `a_live_worker_…different_answers` |
| ceiling raised from 64 KiB to 64 MiB | **KILLED** | `an_absurd_byte_bound_still_comes_back_bounded` |
| client sends the wrong `op` | **KILLED** | **all six** reading tests |

The first is the one worth noting: it removes the scope check *without*
removing the bound, so a test that only checked the byte limit would have
passed. The last is §35 in executable form — no test enters below the
production client.

---

### Allow the user to enter any orchestrated worker while it is running. (line 745)

Contract: Given an orchestrated worker running under `glasshouse api serve`, when a person runs `glasshouse api read` from their own terminal, Glasshouse shows them that worker's own recent terminal output — bounded server-side, on standard output and verbatim — while preserving project scope, keeping `no live process`, `no such session` and `live but silent` as three different answers, writing nothing to the session, and naming no filesystem path.

State: **COMPLETE**

Production evidence:
- `src/api/protocol.rs` — `Request::RecentOutput`
- `src/api/protocol.rs` — `default_recent_output_bytes`
- `src/api/unix.rs` — `MAX_RECENT_OUTPUT_BYTES`
- `src/api/unix.rs` — `dispatch (Request::RecentOutput arm)`
- `src/api/client.rs` — `read_output`
- `src/api/mod.rs` — `read_output (export, and the non-Unix refusal)`
- `src/cli.rs` — `ApiCommand::Read`
- `src/main.rs` — `run (Command::Api / ApiCommand::Read arm)`
- `src/session/api.rs` — `SessionApi::recent_output (unchanged; now has its first production call site, at src/api/unix.rs:619)`

Regression evidence:
- `worker_access::output_a_real_harness_printed_comes_back_through_the_client`
- `worker_access::a_live_worker_with_nothing_to_say_and_a_session_with_no_process_are_different_answers`
- `worker_access::another_projects_worker_cannot_be_read_and_a_crafted_id_says_the_same_thing`
- `worker_access::an_absurd_byte_bound_still_comes_back_bounded`
- `worker_access::reading_a_worker_changes_nothing_about_it`
- `worker_access::a_worker_whose_process_died_under_a_live_door_still_reads_back_what_it_printed`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| the Request::RecentOutput arm: `let api = SessionApi::new(&store, &mut guard); match api.recent_output(&SessionId::new(session), max_bytes.min(MAX_RECENT_OUTPUT_BYTES))` -> read `guard.get(&SessionId::new(session))`'s scrollback directly, keeping an identical byte bound so ONLY the project-scope resolve is removed | `go-around-the-project-scope-seam` | **killed** | `worker_access::another_projects_worker_cannot_be_read_and_a_crafted_id_says_the_same_thing` |
| the Request::RecentOutput arm gains `Err(ApiError::NotLive { .. }) => Response::ok(serde_json::json!({ "output": "" })),` above the general Err arm | `empty-string-instead-of-the-not-live-refusal` | **killed** | `worker_access::a_live_worker_with_nothing_to_say_and_a_session_with_no_process_are_different_answers` |
| const MAX_RECENT_OUTPUT_BYTES: usize = 64 * 1024; -> = 64 * 1024 * 1024; | `raise-the-server-side-ceiling` | **killed** | `worker_access::an_absurd_byte_bound_still_comes_back_bounded` |
| src/api/client.rs read_output: `"op": "recent_output",` -> `"op": "session_state",` | `client-asks-a-different-verb` | **killed** | `worker_access::output_a_real_harness_printed_comes_back_through_the_client` |
| src/api/client.rs read_output: the empty-output notice `eprintln!` -> `println!` | `silence-notice-onto-stdout` | **killed** | `worker_access::a_live_worker_with_nothing_to_say_and_a_session_with_no_process_are_different_answers` |
| the Request::RecentOutput arm: `max_bytes.min(MAX_RECENT_OUTPUT_BYTES),` -> `MAX_RECENT_OUTPUT_BYTES,` | `ignore-the-callers-own-lower-bound` | **killed** | `worker_access::an_absurd_byte_bound_still_comes_back_bounded` |

> go-around-the-project-scope-seam observed: panicked at worker_access.rs:1472 — `the refusal must scope itself, so the answer is about this project rather than about the session's existence anywhere`. Also failed a_live_worker_...different_answers at 1370 (the refusal no longer named the session). Both are the assertions those tests are named for, not fixture guards (§80 case 5). A first, coarser version of this mutation also removed the bound and was discarded rather than reported.

> empty-string-instead-of-the-not-live-refusal observed: panicked at worker_access.rs:1358 — `a session no process is running has no output to give, and saying so is the whole point of the verb`. Exactly one test failed, so the mutation is precisely targeted at ruling 3's collapse and nothing else.

> raise-the-server-side-ceiling observed: panicked at worker_access.rs:1607 — `a caller asking for a hundred million bytes received 152106 of them; the door's ceiling is not being applied`. §80 case 6 checked: no test input derives from the constant — the request is the literal 100000000, the assertion is a literal 64*1024 restated in the test file, and the burst size is fixed, so the mutation moved the answer and not the goalposts.

> client-asks-a-different-verb observed: all six reading tests failed. §35's check: no test enters below the production client — every one of them drives `glasshouse api read` as the shipped binary, so the new production caller is the one under test rather than a socket shortcut beside it.

> silence-notice-onto-stdout observed: panicked at worker_access.rs:1340 — `there was nothing to show, so nothing may be shown`. Proves the stdout/stderr split is a live assertion rather than an incidental property.

> ignore-the-callers-own-lower-bound observed: panicked at worker_access.rs:1621 — `a caller may lower the ceiling: 65536`. The ceiling and the caller's own lower request are separately watched.

Recorded scope limits — stated by the worker, not discovered later:
- It is not an interactive attach. Ruling 5's reading — that send, interrupt and read together are entering a worker — is what this is built to, and whether that closes the line is the orchestrator's ruling, not the worker's. `session::attach`'s objection to a transparent full-terminal attach is untouched.
- A read is a snapshot, not a stream: there is no follow/tail mode.
- A tail, not history. The runtime keeps 256 KiB per session, this door returns at most 64 KiB of it, and nothing persists terminal output — output older than the scrollback cannot be given by any verb.
- Truncation is silent. The response carries `output` and nothing else, so a caller that received exactly the ceiling cannot tell `that is all there was` from `there was more`. No `truncated` flag was invented, because `recent_output` does not report whether it cut.
- MEASURED FINDING (test 6): `ok` means this door holds a pseudo-terminal record for the session, NOT that a process is alive. A worker that dies under a still-running door reads back `ok` with what it printed, because SessionRuntime deliberately keeps an exited session's scrollback. Consequence: a worker that died before printing anything is indistinguishable through this verb from a live silent one — `session_state` and the event log can tell them apart.
- The 8192-byte default is asserted nowhere: the CLI omits `max_bytes` rather than restating it, so one source of truth was bought at the price of an unpinned default.
- The non-Unix refusal is compile-proven via a §18 cfg flip, never executed on Windows.

---

## REVIEW — the orchestrator owes an answer to each of these

This section is the point of the generator. Everything above is the
worker's facts, transcribed. Nothing below is decided.

- **745** — verdict `closed`. Re-run one decisive mutation yourself, then rule (§79: a worker's packet does not bind the integrator).

**Packet errors the worker reported — read these BEFORE its results.**
Thirteen consecutive rounds a worker corrected its packet and was right:
- Acceptance test 3 required a boundary refusal `distinguishable from 'no such session'`. It is not satisfiable and should not be: each project has its own database, so another project's session is ABSENT rather than foreign, `ApiError::ForeignProject` is unreachable between two real projects, and saying more would confirm the session exists elsewhere — a worse leak for a read verb than for a write one. Re-verified on current source rather than inherited from GH-API-CLIENT. The test asserts the scoped sentence, that a crafted id gets the identical sentence, and by content that a marker planted in alpha's scrollback appears nowhere in beta's answer.
- The packet's acceptance test 2 says `a session with no live process`. That is not where the refusal falls: `NotLive` means this process holds no pseudo-terminal for the session (a door restarted, or a session started elsewhere), not that the process behind it exited. Pinned by test 6 rather than left to be found later. src/session/runtime.rs:1151 poll_exits keeps an exited session; src/session/runtime.rs:1349 reuses the same scrollback across a restart.
- docs/process/refusal-register.md:259 Cluster K's 745 entry was corrected on 2026-08-30 and is now stale in a second way: 745 is no longer a refusal at all and the entry should be retired rather than corrected again. STALE — reported, not edited.

Gates the worker ran (re-run the decisive ones yourself):
- cargo build: clean
- cargo fmt --all -- --check: clean
- cargo clippy --all-targets --all-features -- -D warnings: clean
- cargo test --test worker_access: 15 passed
- cargo test --test project_isolation: 7 passed
- cargo test --test api_event_log: 6 passed
- scripts/blast-radius.sh: exit 0, every traced target passed — 43 targets (--lib 1548 passed, 41 integration tests, --bin glasshouse 44 passed) plus rustdoc clean
- non-Unix path via §18 cfg flip across api/mod.rs and main.rs (unix -> windows): cargo clippy --bin glasshouse -- -D warnings clean; both files restored byte-identical, verified with cmp

