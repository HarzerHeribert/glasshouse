# Capability evidence — phase 45

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 45 — the crash report's race, and why the box was right anyway

The Linux leg of the local gate had been failing randomly for days across two
different pty tests (practice §34). Rate, measured under full workspace load in
the container: **8 failures in 17 runs before; 0 in 20 after.**

**The cause, measured rather than inferred.** A pty child's exit becomes
observable before its output does: the exit comes from `waitpid`, while the
output must cross the pseudo-terminal and be copied into a buffer by a
*different thread* which, beside ~900 siblings, does not always get a CPU slice
in time. Instrumenting the failing assertion to keep looking after it failed
caught the window directly — at the moment of the empty read the output had not
ended, and the bytes arrived **1.1ms to 2.2ms later**.

**The orchestrator's hypothesis was wrong and the worker killed it with data.**
The packet proposed that Linux discards unread buffered output when the last
slave descriptor closes. It does not: 200 trials at each of three delays, child
reaped before the first read, **600 trials, zero bytes lost**, `EIO` every time
*after* the data. Linux hands the reader everything that was written and then
reports end-of-file.

**So the box stands and the defect was smaller than it looked.** Glasshouse
never lost a crashed harness's output on Linux; `crash_report` *reported* it as
absent when asked inside the window. The fix is `OutputEnd`, a `Mutex<bool>`
plus a `Condvar`, so `crash_report` waits to be **woken** by the reader rather
than sleeping and looking again — bounded at 250ms, deliberately the same bound
`session::attach` already allows its own pump, because on Windows no
end-of-file ever arrives while the pty is open and nothing else would end the
wait.

Known limit, recorded rather than hidden:
- A **different**, rarer failure survives:
  `a_direct_provider_profile_reaches_a_real_child_and_only_that_child`, once in
  37 runs, with the child killed by `SIGABRT`. That is a child that died, not
  output that had not arrived, and the drain fix does nothing for it. Ruled out
  with evidence: the `EIO` hypothesis (600 trials), a non-blocking master fd
  (portable-pty never sets `O_NONBLOCK`), `malloc` between `fork` and `exec`
  (2400 spawns against 24 allocation-churning threads, 0 aborts), and
  mislabelling (`strsignal(6)` really is `SIGABRT`). A ranked list of where to
  look next is in the report.

---

## Phase 45 line 1735 — NOT CLOSED, and the gap is now one named question

Contract: Given a session whose backend is served through the Glasshouse
gateway, when that gateway's upstream fails, Glasshouse records the failure
against the resource — while leaving the harness session running, on screen and
steerable, because a gateway failing is not a harness process failing.

State: **PARTIALLY VERIFIED.** The mechanism is built, wired through the
gateway's own production path and proven behaviourally. **The shipped binary
never installs it**, so the box does not close.

**Why this was attempted:** `events::degrade_resource` publishes
`GatewayUnhealthy` and its doc states this line's contract nearly verbatim —
*"A gateway failing is not a harness process failing, and the two need opposite
responses"* — with `implied_state` mapping the event to `None` so a live session
is not marked failed. `scripts/discover.py --seam degrade_resource` returned
**zero non-test call sites**. Nothing connected the gateway's own detection to
it.

What now exists (`GH-GATEWAY-DEGRADE`, integrated):
- `gateway::session::gateway_failure(&Exchange) -> Option<events::GatewayFailure>`
  — a pure classifier beside the existing `classify`. **Only
  `Outcome::Unreachable` maps to a failure.** A `Forwarded` exchange never does,
  even a 5xx, because an application error the gateway passed through is not a
  gateway failure.
- `gateway::DegradeSink` + `start_if_required_with_degrade_sink`, invoked from
  `accept_loop` immediately after `observe_exchange`.

**A structural constraint the orchestrator's packet did not anticipate, and it
changed the design.** `gateway/mod.rs`'s header and the existing test
`the_gateway_imports_none_of_the_modules_that_would_make_it_a_harness` forbid
every file under `gateway/` from naming `crate::session` in production code.
`degrade_resource` takes `&[crate::session::SessionRecord]`, so the direct call
the packet's FEASIBILITY implied is a **compile-provable violation**. The wire
therefore had to be an opaque callback, built by whoever holds both an
`EventBus` and the session list — which is exactly the shape `quota_cache`,
`evidence_ledger` and `health_cache` already have.

Regression evidence:
- `gateway_degrade::a_real_gateway_failure_degrades_only_the_bound_session_and_moves_no_lifecycle`
  — against a real gateway, asserting the failure is reported once, names the
  resource and variant, touches only the bound session, and **moves no
  lifecycle**.

Mutation, re-run by the orchestrator: deleting the sink invocation in
`accept_loop` → **killed**, on a real assertion rather than a build break
(§80's case 4 checked), and the mutated line is the call itself (case 3
checked).

### Why the box stays open, and the exact question that closes it

`main.rs` calls plain `start_if_required_with_telemetry` at both gateway launch
sites (`launch_session`, `resolve_resume_overlay`). Neither was touched —
`main.rs` was that package's `FORBIDDEN FILES`. **So `degrade_sink` is never
`Some` in the shipped binary and the new branch never fires against a real
session.** The worker flagged this itself, prominently and against its own
interest, and declined to guess at the closure. That was correct.

**The orchestrator then found why it is not a five-minute follow-up.**
`degrade_resource` needs an `EventBus` and a live session list. `EventBus::new()`
occurs once in `main.rs`, at `:1832`, and **there is no bus in scope anywhere in
the gateway-launch region** — the gateway is started *before* the session, and
therefore before the bus exists. A sink built at that point cannot capture a bus
by value; it needs a lazy handle that can produce the bus and the current
session list at exchange time, which is a lifetime and ownership question about
the launch path, not a wiring patch.

**That is the whole remaining gap, and it is Opus-specialist shaped.** Until it
is answered, this line is a mechanism with no production installation — the same
shape this ledger refused for 531 and for `degrade_resource` itself, and it is
refused here for consistency rather than ticked because the work was good.

Note also: `GatewayFailure::TimedOut` and `::Rejected` are never produced.
`ingress::Outcome` has no production path distinguishing either from
`Unreachable`; its `detail` is a tracing phrase, not a second outcome, and
string-matching it would make the gateway a reader of output its own module doc
forbids it to parse. Recorded as a narrower mapping than the line's wording
suggests.

---

### Phase 45 — Detect gateway failure separately from harness-process failure (line 1735)

State: **COMPLETE** — promoted by the orchestrator, batch 50.

Contract: Given a running Glasshouse session backed by a local gateway, when
the gateway's upstream cannot be reached, Glasshouse records a durable gateway
failure against that session and **no process exit**; and when the harness
process itself dies, it records a process exit and **no gateway failure** —
while never panicking, never blocking the gateway's start path, and never
losing a failure that arrives before the event recorder exists.

**The gap this closed.** `DegradeSink` (`gateway/mod.rs:111`) and
`start_if_required_with_degrade_sink` (`gateway/mod.rs:812`) shipped in an
earlier batch and `main.rs` never called them, so the sink was `None` on every
path the binary took and `events::degrade_resource` had zero production
callers. The refusal register recorded a real ownership question:
`EventRecorder::open` is `main.rs:719` (launch) and `main.rs:2293` (resume),
while the gateway starts 184 lines earlier at `main.rs:535` and at `:1088`, so
the `EventBus` does not exist when the gateway starts.

**The design, and the option nobody offered.** The packet asked the worker to
choose between panicking and silently swallowing a failure that arrives before
the recorder exists. It chose a third: `DegradeRelay`, a lazily filled shared
handle that **holds and replays** such failures in arrival order under one
`Mutex` covering both lives. Panicking would end a user's live session over a
telemetry ordering problem; discarding would blind the one window this line
exists to observe. Bounded at `EARLY_GATEWAY_FAILURES = 32`, past which
failures are counted and named in a `tracing::warn!` rather than kept; `Drop`
warns if the relay still holds anything; installing twice warns and does
nothing.

`EventRecorder.log` became `Option<Mutex<EventLog>>` because `rusqlite::
Connection` is `Send` and not `Sync`, so the recorder could not otherwise be
shared with the gateway's connection thread. `Arc` rather than `Weak` is
deliberate: locals drop in reverse declaration order, so `events` (line 719)
drops **before** `gateway` (line 535), and a `Weak` would be dead exactly when
the gateway's last exchange reports.

Production: `main.rs::DegradeRelay` (`new`/`sink`/`install`/`report`/`Drop`),
both gateway call sites switched to `start_if_required_with_degrade_sink`.
Verified by the orchestrator directly: **both paths construct, pass and install
the relay** — launch `main.rs:540`/`:564`/`:735`, resume
`main.rs:2466`/`:2475`/`:2554`.

Regression, entering at `glasshouse launch` rather than at the seam:
`gateway_degrade::the_shipped_binary_records_a_gateway_failure_against_the_session_it_launched`
uses a real fixture project whose provider `base_url` is `http://127.0.0.1:1`,
a fake harness that dumps its environment and waits, and reads
`ANTHROPIC_BASE_URL`/`ANTHROPIC_AUTH_TOKEN` **out of the child's environment**
to send a real `POST /v1/messages` — the door the harness itself would knock
on. The gateway answers 502 and the durable event log holds exactly one
`GatewayUnhealthy { resource: "glasshouse-gateway", reason: Unreachable }`
against the launched session and **no `ProcessExited`**. The other direction is
`a_harness_that_dies_records_a_process_exit_and_no_gateway_failure`. That pair
is what "separately" means; neither alone would show it.

The pre-existing seam test is kept and its file header now says plainly that it
is a seam test and does not close this line.

Mutations — two KILLED by the worker; the first re-run by the orchestrator in
the integrated tree, where it failed
`the_shipped_binary_records_a_gateway_failure_against_the_session_it_launched`
on *"the shipped binary must record exactly one gateway failure for a launch
whose upstream refused the connection; it recorded [\"session_started\"]"*.
The seam test stayed green throughout, which is §35 demonstrated rather than
asserted. The second, `install(..., Vec::new())`, proves the *installation* is
watched and not merely the sink's existence.

The structural guard `every_gateway_the_binary_starts_is_given_the_evidence_ledger`
now counts **both** entry points, so a version counting only the old name
cannot pass with zero gateways found (§68). A sibling guard counts the relay's
three elements on both paths — which is exactly the batch-49 failure mode where
`api::unix::spawn_session` was a second launch path installing no hooks.

Missing evidence, recorded rather than discovered later:

- **The resume path has no behavioural test.** It is wired and installed and
  covered by the structural scan, but only `launch_session` is driven end to
  end; §65 says a scan is presence, not behaviour.
- **A degradation reaches this process's own session only.** The relay is
  installed with the sessions this process owns rather than a fresh
  `SessionStore::list`, deliberately: reading fresh needs a second SQLite
  connection held open on the gateway thread for a read that fires only after
  an upstream has already failed (§65, which cost 37 minutes of Windows hang),
  and a gateway is per instance — another process's session is served by *its*
  gateway. This narrows `degrade_resource`'s "every session that was running on
  it".
- **Only `GatewayFailure::Unreachable` is ever produced.** `session::gateway_failure`
  maps every other `Outcome` to `None`. Nothing here widens that, and Phase 47
  line 1763 depends on it being widened — see `phase-47.md`.
- **The pre-install replay path is exercised by no test**, because in the
  shipped binary no exchange can happen before `install`.
