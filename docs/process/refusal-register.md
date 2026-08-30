# Refusal register — every line refused, and whether its cause is ours

> **This is not an archive. It is the input to the next package.** Practice §83.
>
> A line refused at Phase −1 is a correct outcome and a recorded one. What went
> wrong for three batches is that nothing ever came back for them: the ledger
> filled with well-reasoned refusals and the next batch picked different lines.
>
> **The register exists so refusals can be read together rather than per phase**,
> because that is the only way the clusters are visible. Four separate phases
> each refusing "no production caller" looks like four dead ends; read together
> it was one hardcoded struct literal.

## This file drifts, and it was audited 2026-08-30

**Seven of its rows were stale** — 1288, 1291, 1319, 930, 934, 748 and 1681 all
named lines that are now ☑ in the map. A register that lists closed lines as
refusals is worse than no register: it is read precisely when an orchestrator
is deciding what *not* to spend a worker on.

Two of the seven were written **the same day they went stale**, by the
orchestrator who then relied on them. One of those, Cluster L, asserted a
symbol had no production caller when it had one all along.

**So: re-derive from source before you commit to a phase, and treat the rows
below as leads rather than findings — including the ones that sound certain.**
The audit that found these was a read-only recon costing one worker slot
(`.agent-runtime/report-next-candidates-recon.md`); it is cheap and it should
be repeated when this file starts feeling authoritative.

## How to use it

1. **Before writing the handoff**, add every line this batch refused, with the
   missing link named in current source and the `in-repo?` column filled.
2. **Before planning the next batch**, read the `in-repo = yes` rows together
   and ask which share a cause. Two or more sharing one is the next package.
3. **Delete a row when its line closes.** A stale register is worse than none —
   that is the failure mode this whole document is about.

## The column that decides everything

**`in-repo?`** — is the missing thing inside this repository?

- **yes** → packageable. Someone can write the code that supplies it.
- **no** → not work. A setter whose provider signal never arrives, or a
  capability whose decisive input is outside Glasshouse's process boundary, is
  blocked on reality. Packaging it manufactures work and invites a fabricated
  producer.

Do not let a `no` become a `yes` because the line looks close.

---

## Open refusals, as of batch 50

### Cluster A — a production caller passes an invented constant *(in-repo: **the cluster is now EMPTY**; see 1294 below)*

| line | missing link | site |
|---|---|---|
| ~~1288~~ | **CLOSED — removed from this cluster.** `disposable.rs:598` now calls the real `cheaper_adequate_resource_exists` (`:828`); the hardcoded literal is gone |
| ~~1291~~ | **CLOSED.** Unblocked by 1288; map line is ☑ |
| ~~1319~~ | **CLOSED.** `gateway/mod.rs:614` now passes the quota through |
| ~~1290~~ | **CLOSED 2026-08-30** by `GH-RESERVE-INPUTS`. `user_override` is now set by `ReserveOverride`, a *scope* rather than a switch — there is no spelling of it that means every session |
| ~~1294~~ | **NOT A CLUSTER A ROW — MOVED to Standing refusals, 2026-08-30.** The constant is still there (`routing/disposable.rs:730`), but it is a **refusal written into the source**, not a gap awaiting wiring. `provider/quota.rs:2265-2285`: *"no path reports task progress … the only completion fact available there is that the turn is already over … A fabricated value here does not degrade the policy, it inverts it."* Two independent readers reached this on 2026-08-30 — the orchestrator, and `GH-REGISTER-AUDIT`. **Do not package.** |

### Cluster B — a mechanism built, tested, and never installed in production *(in-repo: YES)*

**Batch 50 closed two of the four and disproved a third.** The cluster framing
was right and it paid: 1735 and 925 are done, and 531 turned out to be
mis-filed. What is left is one row.

| line | missing link |
|---|---|
| ~~922~~ | **CLOSED 2026-08-30** by `GH-MEMORY-CONFLICT-RESOLVE`. `glasshouse memory conflicts` and `glasshouse memory resolve <id> active|superseded` give `resolve_conflict` its first production callers. **This row was right and it paid**: an ordinary `memory search` calls `mark_conflicted` in production, `is_current` answers `false` for `Conflicted`, and nothing in the binary could undo it — a live defect, not merely an unwired mechanism. See `phase-21e.md`. |

**CLOSED and removed:** 1735 (batch 50 — `DegradeRelay` in `main.rs`, a lazily
filled handle that holds and replays; see `phase-45.md`). 925 (batch 50 —
migration 13 `superseded_reason`; the recorded "needs a schema migration, Red
tier" blocker was wrong in a way that mattered, see `phase-21e.md`).

**MOVED OUT — 531 is not a Cluster B line and packaging it as one would have
manufactured work.** The register recorded "`declare_token_priced` has zero
non-test callers". True, and not the gap. Verified batch 50 against current
source: `is_request_pool` (`routing/free.rs:101`) *also* has zero production
callers — its only caller is `free.rs:769`, inside the `#[cfg(test)]` that
starts at `free.rs:617` — and the single production allowance read,
`free.rs:453`, asks `is_exhausted`, which pooled and token-priced credentials
both answer. Production never distinguishes the two anywhere. And no `FreePool`
outlives a single call: the only production caller of the routing entry point,
`memory/extract/disposable.rs:61`, constructs `FreePool::new()` per call and
drops it — its own doc comment says *"There is no live FreePool to consult
here ... this caller never makes one."*

So 531 is missing **caller and consumer**, not one link. Closing it honestly
means building a production consumer that behaves differently for pooled versus
token-priced credentials, and giving the pool a lifetime — a routing feature
whose consuming phases are still at zero. **It belongs in Cluster D.**

### Cluster C — an enum variant with no constructor *(in-repo: YES, but each needs a signal that may not exist)*

| line | missing link |
|---|---|
| 945 | `ReviewReason::ArchitectureDrift` — **no constructor anywhere, including tests**; reachable only by a human typing the string. Needs a "refactor starting" signal that does not exist |
| 946 | `ReviewReason::ProductionIncident` — identical shape, zero constructors |
| 1210 | `WindowCapacity::with_started_at` — one occurrence in the tree: its own definition |

### Cluster D — no consumer, because the consuming phase is at zero *(in-repo: yes, but correctly waiting)*

| line | missing link |
|---|---|
| 1239 | consumer belongs to routing phases still at zero |
| 1795 | no fallback-chain concept; routing-model selection is Phase 34C |
| 1796 | nothing maps a model to a tier ceiling. **Its recorded blocker is stale** — `WorkloadTier` ships at `routing/classify.rs:79` — but §14 applies: the blocker being gone is not the capability |
| 372 | **CLAUSE 1 IS CLOSED — updated 2026-08-30 by `GH-REGISTER-AUDIT`.** `59e9633` gave `ProfileConfig::enabled` a reader: `main.rs:642` filters `DestinationScope::Everything` on `effective.profile_enabled(name).value`, and `main.rs:1402-1409` refuses an explicit `--profile` launch of a disabled profile. **Only clause 2 remains** — *"when automatic routing is enabled"*. The rest of this row is the pre-fix analysis, kept for clause 2. Re-derived 2026-08-30 by `GH-PROFILE-SELECTION`; the old wording was wrong twice over. *"Nothing selects among launch profiles"* is false — `main.rs:~620` builds one fresh `Destination` per configured profile and the router ranks them. **Both of the line's actual qualifiers fail instead.** *Clause 1, "among **enabled** profiles":* `ProfileConfig::enabled` (`config/mod.rs:372`, default `true`) is **never read**, and the resolved `LaunchProfile` has no `enabled` field at all — a profile the user set to `enabled = false` still appears as a candidate. **That is a live defect, not just an open box.** *Clause 2, "when **automatic routing** is enabled":* the `DestinationScope::Everything` set is built only by `glasshouse route` (a diagnostic that starts nothing) and by `report_task_boundary_routing`, where `RoutingOverride::to(...)` forces the outcome and the ranking only prints a disagreement note. The one caller that **acts**, `launch_session` (`main.rs:1326`), uses a different scope |
| ~~1313~~ | **CLOSED 2026-08-30.** The blocker was real and is gone: `1b66889` gave the aggregates a production reader (`shell/mod.rs:1571`), and `GH-LATENCY-PROOF` then proved the whole chain with three production mutations. **This row is the cluster working as intended** — it held a line that genuinely could not close, and expired the moment its consumer landed. Compare the seven rows the audit found stale, which nobody retired |
| 531 | **moved here from Cluster B in batch 50.** Missing caller *and* consumer: nothing in production distinguishes a request pool from a token-priced allowance, and no `FreePool` outlives one call. Needs a routing consumer that behaves differently for the two — see Cluster B's note |
| ~~930, 934~~ | **CLOSED the same day this row was written.** Phase 27 landed, `GH-INJECTION-RECALL` closed both behind it. Kept struck through for one batch as the clearest evidence in this file that **a Cluster D row expires the moment its trunk lands** — I wrote this row and did not revisit it |

### Cluster E — the provider signal genuinely does not arrive *(in-repo: **NO** — do not package)*

| line | why it is not ours |
|---|---|
| 1205 | no header names an input-vs-output token split |
| 1206 | no `RATE_LIMIT_HEADERS` entry contains "cache" |
| 1208 | the one live account read answers `null` |
| 1213 | no concurrency header exists |
| 1215 | Groq's token ceiling arrives with **no window**; filing it per-minute would invent the period |
| 1216 | reader built and wired; no host has ever sent a window > 60s |
| 1317 | nothing states whether a 429 is provider-, model-, account- or pool-scoped |

### Cluster L — Glasshouse refuses to parse the thing that carries the signal *(in-repo: the boundary is ours and deliberate — do not package without changing it first)*

**Added batch 53, after a Phase −1 check killed a package before dispatch.**
Phase 33A ranks as "5 open, 10 closed" in `ORIENT.md`, which reads cheap. It is
not: three of its five open lines need the gateway to inspect a response
*stream*, and `gateway::ingress` is explicitly forbidden to.

`gateway/ingress.rs:456-458` states the rule in its own words — the quota
headers are read *"Headers only … **never the body, which stays a byte stream
this function never parses**"*. That is capability map line 1229's gateway half
and it is a decision, not an omission.

Everything downstream is already built and waiting, which is what makes this
look packageable: `database.rs:1164-1165` has the `first_byte_at` and
`first_token_at` columns, `routing/evidence.rs:250-251` has the fields as
`Option<i64>`, and both say so — `database.rs:1119` records that
*"`first_token_at`/`first_tool_call_at` are NULL from that producer today"* and
`routing/evidence.rs:42` lists them as **not supplied**. The schema, the struct
and the documentation are complete. Only the producer is missing, and it is
missing on purpose.

| line | why it is not ours today |
|---|---|
| 1331 | "time to first real token" and "time to first tool call" require reading the stream. **`first_byte_at` is the exception** — first byte is observable without parsing, so a package could honestly close that clause alone |
| 1332 | *requires* distinguishing whitespace padding, keepalives and reasoning-only deltas from a real token — the definition of parsing the body |
| 1333 | input/output/cached token counts. Same wall, and it is the same one that made map line 1158 a refusal this batch: `routing_observations`' token columns are documented as not supplied because filling them means parsing a body `ingress` will not parse |

**What would unblock it is a design decision, not a worker.** Either
`gateway::ingress` gains a bounded, streaming, non-buffering observer that
counts and timestamps without interpreting content, or these lines stay open
and should stop being counted as cheap. **Do not package 1331–1333 as wiring.**

**Amended the same day, by a read-only recon, and the amendment corrects me
rather than the cluster.** I wrote above that Phase 33A's ledger looked like
Cluster B with "no production caller". **That was wrong.**
`SessionRouting::record_routing_observation` (`gateway/session.rs:327`) is
called at `gateway/mod.rs:640`, both well before their `#[cfg(test)]`
boundaries, and the comment above the call site names it *"Phase 33A's
production producer"*. My grep looked for `.record(` and `RoutingObservation`
and missed a writer with a different name. **A false absence of a caller
manufactures work; this one nearly did.**

The corrected picture, per field, from
`.agent-runtime/report-evidence-ledger-recon.md`:

| line | status |
|---|---|
| **1330** | **six of its seven fields already flow** — `provider`, `model`, `route`, `quota_context`, `harness`, `dispatched_at` are all set at `gateway/session.rs:359-365`. **`purpose` is the one field nothing holds**: `NewObservation` has no `with_purpose` builder at all, `new` defaults it to `None`, and nothing in the tree — production or test — ever sets it. A live struct field and a live schema column with **no producer whatsoever**. A `SessionPurpose` exists on `SessionRecord` (`session/store.rs:507`), but whether `SessionRouting` can reach a session identity to look it up is **unverified** — check that before packaging |
| **1334** | **worse than unwired, and not uniformly.** All four counters are always `NULL`. `tool_rounds` is **above this cluster's line**: `gateway/ingress.rs` serves one HTTP request per connection, and a harness's tool loop spans several, so nothing at this layer has a "round" to count. `retries` and `repairs` — the recon **could not find the concept anywhere in the tree**, which is weaker than unwired; only `Retry-After` *header parsing* exists, which is what the provider said to wait, not a count of Glasshouse's own attempts. `failovers` is the one plausibly countable today |

**The reader side is split and that matters for what "recording" is worth.**
`EvidenceLedger::recent` has **zero** production callers — every site is in a
test module or in `gateway/conformance.rs`, itself `cfg(test)`-gated.
`summarize` has **one** production caller, `ObservedEvidenceSource::observed`
(`evidence.rs:1163`), reached in production from `gateway/session.rs:478` and
feeding a real failover ranking — **but it reads only `failure_rate`**. The
three latency aggregates have **zero hits tree-wide** outside `evidence.rs`,
which is Cluster D 1313's shape exactly.

So the ledger is written in production and *partly* read in production. It is
not write-only. **1330 is one honest field away; 1334 is not a wiring job.**

1330 and 1334 are *not* in this cluster — identity, purpose, timestamp, tool
rounds, retries and outcomes need no body inspection. A package scoped to those
two plus 1331's `first_byte_at` clause is honest and was not attempted this
batch only because both partitions that touch the gateway were already claimed.

### Cluster F — outside Glasshouse's process boundary *(in-repo: **NO** — a product boundary, not a gap)*

The decisive input is the user's source tree, the agent's plan, or the agent's
diff. Verified: nothing under `crates/glasshouse/src/` reads the user's tracked
source or runs their tests. Map line 932 declined this four times and
`memory/policy.rs:280-295` records the reason.

**932 is inside a nearly-finished phase, which is how it keeps getting
recommended.** Phase 21F reads as "4 open, 7 closed" and a handoff in batch 52
offered it as a cheap closure while naming only 930 and 934 as absent from this
register — true of those two, and not of the line between them. A phase's open
count says nothing about whether its lines are ours. **Check every open line
against this register individually, not the phase.**

| lines |
|---|
| 919, 920, 921, 923, 943, 947, 951 |

### Cluster G — needs a schema migration this project refuses casually *(in-repo: yes; design first)*

| line | missing link |
|---|---|
| 327, 310 | a compaction lifecycle event needs a new `LIFECYCLE_EVENT_KINDS` value, and SQLite cannot widen a `CHECK` in place — `database.rs:830`'s house rule forbids the rebuild |
| 1316 | a new persisted outcome value, same constraint |
| 1325 | of four provenance values the line names, production can emit one |

---

### Cluster H — a view whose data is never made durable *(in-repo: yes, but each needs a producer first)*

Batch 50, Phase 47. The decisive background fact, verified once and true of all
of them: `shell::run` (`shell/mod.rs:72`) takes only a `&Runtime` and is reached
from `main.rs:326`, while the gateway, router and live `FreePool` start only at
`main.rs:535`/`:1088`. **A shell debug view can render only what is durable on
disk**, and seven of Phase 47's eight open lines name data nothing persists.

| line | missing link |
|---|---|
| 1757 | `RoutingExplanation` (`routing/mod.rs:475`) has no durable sink — every production sink is a `tracing` line or an in-memory `Vec`. The propagation slot is dead too: `ShellState::record_disposable_choice` (`shell/state.rs:1216`) has zero production callers while `shell/view.rs:1793` already renders it |
| 1759 | the retrieved set is never recorded; `Extractor::run` (`memory/extract/mod.rs:413`) drops `existing` after `Prompt::build` |
| 1760 | no cache-temperature signal exists at all; `with_context_state` (`routing/evidence.rs:328`) has zero non-test callers and `cached_input_tokens` has no setter |
| 1763 | **needs a product ruling before code.** Production emits exactly one `GatewayFailure` class: `session::gateway_failure` maps only `Outcome::Unreachable`. Counts-by-class of one class is not the capability. The question is whether a non-2xx `Forwarded` should count as a gateway failure — the gateway currently says no on purpose |
| 1766 | two links: 1757's absent rationale, and nothing durably records that a decision happened |
| 1767 | nothing computes a correlation; `routing/domain.rs:31` says so in the source |
| 1769 | extraction never runs in the shell process; needs a durable extraction record plus a caller change where it does run |

**Do not read 1763 as unlocked by 1735.** The orchestrator recorded that during
batch 50 and it was wrong; the correction is in `phase-47.md`.

### Cluster I — the dependency cannot honour the requirement *(in-repo: yes, but the fix is a dependency decision)*

| line | missing link |
|---|---|
| 442 | `keyring` 3.6.3's Secret Service backend can block **up to a year** on an unlock prompt (`dbus-secret-service-4.1.0/src/prompt.rs:42`, `unwrap_or(ONE_YEAR_SECONDS)`); keyring never calls `connect_with_max_prompt_timeout` and a caller cannot reach it. A probe returns `NoEntry` before anything needs unlocking, so `detect()` reports healthy and the **first real credential read freezes the TUI**. Not a runner gap — "when available" has no correct implementation on this dependency. **Next step: does keyring 4.x's `zbus-secret-service-keyring-store` bound the prompt?** |

### Cluster J — the discriminating input is never read *(in-repo: yes, and it is a design change, not a wiring one)*

| line | missing link |
|---|---|
| 566, 569 | `harness::pairing::classify` derives `PairingClass` from harness + model + corrections and **never from the route**, while every candidate set the binary builds varies **only** by route (`UpstreamBackend` has no model field). So the native-pairing prior is constant across every set Glasshouse ranks, and a constant cannot change a ranking. **Tripwired:** `the_native_pairing_prior_is_constant_across_a_real_session_start_candidate_set` fails the moment `classify` reads the route, and that failure means 566 became reachable |

### Cluster K — a decision nobody has made, and a door that records nothing *(in-repo: yes)*


**745 is now CLOSED (`glasshouse api read`), and Phase 16 is finished.** The correction below stands as the record of why it was open for so long.

**745's entry in this cluster is WRONG, corrected 2026-08-30.** It frames the
line as an unmade Red-tier decision — *"the worker becomes `Embedded`"* versus
*"a pty handed between processes"* — on the grounds that no read path into a
running worker exists outside the process that owns it.

**A read path exists inside that process and has no production caller:**
`SessionApi::recent_output` (`session/api.rs:150`), project-scoped through
`SessionApi::resolve`, with its own test asserting it refuses a foreign session
(`api.rs:727`). Every call site is in its own `#[cfg(test)]` module — verified
on the integrated tree.

So 745 needs **one `Request` variant** exposing it, not an architecture
decision. 746 and 747 closed the same day once `api/client.rs` gave the door a
caller. **That finding lived only in `tests/worker_access.rs`'s module doc**,
where nobody deciding what to package would look — which is the second time
this session a decisive fact was recorded somewhere the register does not
reach.

Batch 51, Phase 15/16. **Ranking these by open-line count keeps recommending
them; the reason they are open is not effort.**

| line | missing link |
|---|---|
| ~~745~~ | **CLOSED 2026-08-30** by `GH-WORKER-READ` (`glasshouse api read`). It was never a Red-tier decision: `SessionApi::recent_output` existed all along — see the correction above |
| ~~746, 747~~ | **CLOSED 2026-08-30** by `GH-API-CLIENT` (`glasshouse api send` / `api interrupt`). **Phase 16 is finished** |
| ~~748~~ | **CLOSED.** Map line is ☑; commit `d9f6e75` |
| 740 | an ordering claim over 745; unreachable while 745 is |

**The defect underneath, and it is bigger than 748.** `glasshouse api serve`
writes **nothing** to the project event log — not interventions, not
`session_started`, not `process_exited`. `api/unix.rs:84` builds its runtime
with `SessionRuntime::new()`, a bus with no sink, where `shell/mod.rs:88` calls
`attach_event_log` first. Measured with a shipped-binary probe plus control:
`lifecycle_events` is empty.

**And the obvious fix closes nothing** — verified by a SURVIVED mutation.
Attaching a sink makes rows appear, correctly stamped `machine`, and
`Request::Events` still returns `[]`, because `observed_since` filters
`WHERE ... observed_harness IS NOT NULL` (`events/log.rs:376`). So a naive fix
buys a write-capable SQLite handle held for the door's whole life — §65's
hazard, on the platform where SQLite's locks are mandatory — and **zero
observable behaviour**. The write path and a read path that bypasses
`observed_since` have to be designed together.

## Standing refusals that are decisions, not blockers

- **1323** stays open by the user's own reasoning. Do not re-ask (§70).
- **828, 829** — a worker was asked to close them and declined, correctly: a
  keyword heuristic for "is this an obvious source-code fact" refuses real
  memories and admits fake ones. **Do not re-derive this.**
- ~~**1681**~~ — **CLOSED.** `recommend_route` ships (`62473a6`) and **Phase 42 is finished**.
- ~~**1661**~~ — **CLOSED 2026-08-30** by `GH-OVERVIEW-LATENCY`. The overview reads `median_duration_ms` from the evidence ledger, which is a measurement rather than a configured ceiling; the objection was about the wrong field.
- **1745, 1746** — no cmux-metadata path reaches project-scope validation, and
  there is no MCP surface. A grep for "cmux|mcp" hits doc comments and looks
  like a lead.


## Batch 54's refusals — three packages killed at Phase −1 before dispatch

**Added 2026-08-30.** Each of these looked packageable from `ORIENT.md`'s
open-line ranking and is not. Two were caught by the orchestrator's own Phase −1
check and one by an adversarial audit. **They are recorded here because the
ranking will keep recommending them.**

### Cluster M — the measured quantity is never measured *(in-repo: yes, but the counter does not exist)*

| line | missing link |
|---|---|
| 1263 | *"Lower the score when user-defined spending budget is close to exhaustion."* The **producer exists** (`QuotaOverride::budget()`, config-loaded and layered) and the **consumer exists** (`remaining_capacity_score` → `normalized()` → `pools()`, which already includes `user_budget`). What is missing is **any count of what has been spent**. `routing/evidence.rs:66` states it: *"`cost_micro_usd`: not supplied."* There is no `SUM(cost…)` anywhere in the tree, and `provider/resources.rs:950` prints the budget to the user with the words **"Glasshouse does not count spend against this"**. "Close to exhaustion" is not computable. The only production writer of `CapacityState::user_budget` is `provider/telemetry.rs:1017`, which merges a *provider-reported* ceiling, not the user's configured budget; `resources.rs:2195` is past that file's `#[cfg(test)]` at `:1281`. **Blocked on Phase 32G (provider-aware request-cost estimation), which is 10 open / 0 closed.** |
| 1267 | Same function, stated in its own doc comment: *"**This build has no latency or concurrency reader anywhere** — nothing in `CapacityState` carries either quantity."* `remaining_capacity_score` returns a fixed high estimate for local inference carrying an explicit "no evidence" note, which is the honest answer and not the line. |

### Cluster N — a signal constant across the set being ranked *(in-repo: yes, and each has a tripwire)*

A signal that is the same for every candidate cannot change a ranking. This is a
distinct failure from "no producer", it looks exactly like a wiring gap, and it
has now cost two separate investigations.

| line | missing link |
|---|---|
| ~~1599~~ | **CLOSED 2026-08-30** by `GH-GATEWAY-HEALTH-BRIDGE`. The row was correct — no pool reached the router — and it expired the moment a bridge was built. **The lossy reverse map did not defeat it**: `provider_health` builds its own key from the destination, so the bridge renders each destination's label with the *same function the write side used* and compares forward only. No inverse is ever computed. Three ambiguities are **declined rather than resolved**, including two readings that disagree on one (label, model) — which is exactly what a genuine label collision looks like in the data. See `phase-37.md`. |
| 566, 569 | **Do not package. `docs/product/evidence/phase-9j.md` records the full reasoning and a self-maintaining tripwire.** `harness::pairing::classify` derives `PairingClass` from harness, model and user corrections — **never from the route** — while every candidate set the binary can construct varies *only* by route (`UpstreamBackend` has no model field; the one model arrives at `SessionRouting::bind` from `profile.model` and applies to every backend). So the native-pairing prior is constant across every set Glasshouse ranks. 569 is unreachable for the same reason: a warm session cannot outweigh a prior that never tipped anything. Separately, a fresh session does not reach the scorer at all — `best` has exactly two call sites, both in `on_provider_failure`. **The tripwire is `routing::interactive::tests::the_native_pairing_prior_is_constant_across_a_real_session_start_candidate_set`**: if anyone makes `classify` read the route, that test fails, and its failure means 566 has become reachable. |

### Cluster O — Phase 34F has no producer for any of its eleven lines *(in-repo: yes; it is a build, not a wiring join)*

| lines | missing link |
|---|---|
| 1475–1485 | **All eleven blocked**, classified line by line by `GH-CAPABILITY-CALIBRATION-RECON` (`.agent-runtime/report-capability-calibration-recon.md`). There is **no benchmark ingestion of any kind** — no config field, no file format, no loader — and no per-(model, task-kind) capability rating anywhere. `WorkloadTier` (`routing/classify.rs:88`) is a *task*-classification output, not a per-model config value, and `classify.rs`'s own doc comment refuses to merge it with `CapabilityAxis`. `AssignedModel` is a bare name string, so 1485 cannot distinguish a local quantized model from a hosted one. **1480 is the nearest to reachable** — `RoutingObservation` (`routing/evidence.rs:338`) needs one new `tier` field and one caller passing it — but note the recon's separate finding first: `RoutingObservation` **has no `launch_profile` field**, so anything built on `routing_observations` inherits a key one axis coarser than 1482 requires. `EvidenceKey` (`harness/pairing.rs:502`) already keys correctly and should be reused instead. |

**Two things the recon established that a later package must not re-derive
carelessly (§81 — re-derive them, but start here):** nothing today silently
overwrites a user's configured value on the pairing axis (the only five config
setters have one production caller, `main.rs:5375`, behind an explicit user
command); and the decay-to-zero pattern 1484 wants already exists as
`decay_factor`/`FULL_DECAY_OBSERVATIONS` (`config/pairing.rs:277-287`).

### Cluster P — the restraint is unobservable because the thing restrained is never built

| line | missing link |
|---|---|
| 1455, 1456 | **RE-OPENED 2026-08-30 after being ticked in error.** *"Avoid sending full repository contents / session transcripts to the router."* **Nothing in this build constructs a request to a routing model** — `routing/classify.rs:23-27` and `:583-586` both say so in production source. A negative requirement over a request that is never made passes vacuously. The type that appears to enforce it, `TaskRequirements`, bounds the input to `SessionRouter`, an in-process function that sends nothing anywhere. **Do not re-package ahead of 1447**, the line that defines the schema and is still open. Full reasoning in `phase-34d.md`. |

## The register's own weakness, measured 2026-08-30

`GH-REGISTER-AUDIT` checked every open row against current source. Verdicts:
several `STALE`, several `WRONG`, and — the structural finding — **a large
number `UNCHECKABLE`, because the row names no symbol, file, or line to grep.**

`scripts/check-register.py` catches exactly one class: a row naming a line that
is already ☑. It is blind to **a row whose target line is still open but whose
stated reason for being blocked has become false** — which is the majority of
what the audit found, including 1294, 372 and 740.

**So: every new row above names a symbol and a file:line.** A row that does not
cannot be audited, and an unauditable refusal is how a wrong reason survives
long enough to send a worker at the wrong target.


## Cluster Q — a negative requirement over a capability that does not exist

**Added 2026-08-30, and it is a CLASS, not a list** (§83 — gather refusals by
root cause). A line of the form *"never do X"* / *"avoid X"* / *"keep Y
optional"* cannot be closed while nothing in the build could do X in the first
place. The test passes, the box looks green, and nothing is being watched.

**This class has already cost two ticked boxes.** 1455 and 1456 were closed and
un-ticked the same week. Three more lines have now been checked and hold the
same shape.

| line | the forbidden thing, and why nothing could do it |
|---|---|
| 1090 | *"Keep reranking optional so memory search still works offline without an LLM."* **There is no reranker.** `routing/classify.rs:583-586`: *"No cheap model is wired up in this build."* Search working without one is the only state that has ever existed. |
| 951 | *"Avoid automatic revalidation work when the memory is not about to affect any current task."* There is no automatic revalidation to avoid — confirms `phase-21g.md`'s own ruling. |
| 1142 | *"Keep file-aware retrieval advisory and never treat stale memory as stronger evidence than the current source code."* No mechanism anywhere reads current source and compares it to a memory's claim; `memory/policy.rs:280-295` says so in its own doc comment, and comparing a memory against live repository state was declined at 828, 829, 862 and 932. |
| ~~1455, 1456~~ | see Cluster P — the same shape, caught only after they were ticked. |

**The test before packaging any negative line: name the code path that could do
the forbidden thing, with a file and a line.** If you cannot, the line is not
closeable and saying so is the finding. **A test that passes because the feature
is absent is not evidence of restraint.**

## Memory phases 28, 21G and 24 — seventeen lines checked, 2026-08-30

`GH-MEMORY-PHASES-RECON` classified all seventeen against current source.
`ORIENT.md` ranks these phases as cheap (0 closed, few open); **sixteen of the
seventeen are not packageable.** Full citations in
`.agent-runtime/report-memory-phases-recon.md`.

| lines | missing link |
|---|---|
| 1139, 1140 | **No file-path association exists at all.** The `memories` table (`database.rs:303-326`) has no path column — its columns are `id, project_id, kind, authority, status, subject, body, source_session_id, source_commit, superseded_by, created_at, updated_at` — and a tree-wide grep for `file_path\|FilePath\|referenced_file\|associated_file` returns **zero hits**. `memory/extract/` has no path-identification logic. Nothing produces the signal, so there is nothing to retrieve by. Note §71 for 1140 when it becomes reachable: it needs an **enumeration** by path, not a lookup by known id. |
| 1141 | **Half already ships and the other half is 1139's blocker.** `memory/inject.rs::briefing` (234-254) already orders invariants, constraints and failed attempts ahead of ordinary matches, tested by `context_injection.rs:820`. But the line sits under a *file-aware* phase and nothing scopes that preference to a file about to be edited. **Do not tick it on the generic evidence** — the phase's premise is the file scoping. |
| 943–947 | Blocked, confirming `phase-21g.md`'s existing NOT STARTED ruling. 945 and 946 remain **Cluster C** — `ReviewReason::ArchitectureDrift` and `::ProductionIncident` still have no constructor anywhere, including tests. |
| 1089, 1091, 1092 | All three need a reranking stage that does not exist (`classify.rs:583-586`, no cheap model wired). `memory/search.rs`'s BM25+decay ranking (`search.rs:412-441`) is the **lexical ranker**, not a reranker, and the lines ask for the latter. |
| 1094 | **No debug-mode concept exists in this build** — `debug_mode\|DebugMode\|--debug` returns zero hits across `crates/glasshouse/src`, and there is no verbosity-gated diagnostic path in `memory/`. Worse, the diagnostics it would record are discarded: `memory/search.rs::search` computes BM25 relevance × `retrieval_weight` purely to sort, then drops the scores at `search.rs:443`. Both halves are missing. |

### ~~The one that IS reachable~~ — CLOSED 2026-08-30

**1093** — *"Return only a small number of high-value memories for automatic
prompt injection"* — is `ALREADY TRUE` and independent of reranking.
`memory/inject.rs::briefing` applies `.take(MAX_INJECTED_MEMORIES)`
(`inject.rs:253`) where the constant is `3` (`inject.rs:87`), after the
ladder-rung/decay ordering. Both are live production code. **Closed by `GH-INJECTION-CAP`** with two tests and no production change. The
existing test asserted *at most* three on a fixture where the 900-byte
`MAX_INJECTED_BYTES` ceiling was also live, so it could not tell which ceiling
fired (§41). The new fixture uses five short candidates so only the count cap
can act, and asserts **exactly** three.


## Phases 9K and 47 — 18 lines checked, ZERO reachable, 2026-08-30

`GH-CLOSED-PHASES-RECON` tested a deliberate hypothesis: **open lines sitting
beside many closed ones should be open for smaller reasons.** Phase 9K is 26
closed / 11 open and Phase 47 is 8 closed / 7 open — the two most-closed phases
with work left.

**The hypothesis is refuted for both. Not one of the 18 is `REACHABLE`.**

### The reason, and it generalises to the whole map

> *"Every line that closed did so by reading something **already** durable on
> disk — session events already logged, gateway caches already written by a
> different process — rather than by adding a new producer. The seven still open
> all require a **new** durable producer first. The producers that already exist
> were the ones that closed the map's easy lines. What is left is producers that
> do not exist yet."*

**So "look where the producers already exist" is exhausted as a search
heuristic.** A phase being mostly closed is now evidence *against* its
remainder being cheap, not for it. Plan the next batches around building a
producer, not around finding an unwired one.

| lines | missing link |
|---|---|
| 616, 622 | **Cluster Q.** 616 (*"avoid repeatedly injecting an unchanged response contract on every turn"*) — `harness::response::apply` (`harness/response.rs:322`) is called exactly **once**, from `session/select.rs::install_session_document` off the launch path; no per-turn call site exists. 622 (*"do not run a second language model to rewrite every final answer"*) — every production `fn complete(` belongs to memory extraction (`memory/extract/{mod.rs:237,model.rs:308,disposable.rs:136}`), none touches a final answer, and `rewrite\|RewritePass\|PostProcess\|second_pass` returns zero hits. |
| 619, 620, 618 (part) | One missing in-session surface, shared. |
| 623 | A standalone missing surface. |
| 627–630 | **One shared root**: a measurement channel four lines all wait on, which does not exist. Attack the channel, not the four lines (§83). |
| Phase 47's seven (all) | **Cluster H, uniformly** — every signal ends at a `tracing` line or an in-memory `Vec`; the machinery producing them either runs in a separate process invocation (extraction) or never makes anything durable. Re-derivation added no member and removed none. |

### A RULING, because the recon named the temptation

The recon observed that 616 and 622 are "closeable with a test rather than a
feature" — a source-scanning guard asserting the forbidden path stays absent.

**Do not do that. That is exactly how 1455 and 1456 were closed and un-ticked.**
A guard test over an absent capability is worth having as a **tripwire**, and a
tripwire does **not** tick a box. Both lines stay open, in Cluster Q.

### The one correction, and the cheapest partial close in either phase

**`phase-9k.md`'s claim about line 618 is stale.** It records "no reader outside
`harness/mod.rs`"; `glasshouse doctor` now reads `StyleChange` in production
(`integrations/mod.rs:1168-1178`, wired via `fc16943`). **Two of the line's
three terms are closed.**

**The third is NOT cheap, and the recon's "one enum variant plus one adapter
declaration" was wrong — checked at Phase −1 before anything was dispatched.**
`StyleChange` (`harness/mod.rs:693`) is two *mutually exclusive* states;
"invalidates prompt caching" is **orthogonal** (a change can be in-place *and*
invalidate the cache), so it needs a separate field across every adapter, not a
variant. And every value here is `Declared::verified(value, evidence)` carrying
a **measured** justification naming harness version and date
(`claude_code.rs:120-135`). Declaring cache invalidation honestly requires
**observing it on a real harness** — nothing in this repository can answer it.
**Do not package as wiring; inventing the value is 1294's error.**


## A dependency that would not resolve — 627–630 on Phase 47

**The most expensive thing this recon found, because it is invisible until
someone finishes the phase being waited on.**

`phase-9k.md` records that map lines **627–629** wait on **Phase 47** for a
durable metrics channel, and 630 additionally on Phase 33A. The wait is real —
there is no measurement channel in this build at all: `fn score\|Score` across
`crates/glasshouse/src` finds only `RemainingCapacityScore`
(`provider/quota.rs:1779`) and its routing consumers, which are about remaining
quota for routing decisions and unrelated to measuring a response profile's
effect on a conversation. No per-pairing observation storage, no
output-token-reduction counter, no cognitive-load signal.

**But the recorded blocker understates the gap in a way that matters.** Phase
47's seven open lines are uniformly **Cluster H** — a debug *view* over data
that is never made durable. So **Phase 47 closing as currently scoped would not
unblock 627–630.** It would deliver a view; those lines need a *producer*, which
nothing in Phase 47 currently asks anyone to build.

**Do not treat 627–630 as "unblocked once 47 lands."** They are blocked on a
measurement producer that no phase in the map currently owns. Closing 47 and
then packaging them would burn a round discovering this.

**This is the shape to check for elsewhere**: a line whose blocker names another
phase, where that phase — read as written — does not actually supply the missing
link. The register records blockers; it has not until now recorded whether the
thing being waited on would satisfy them.


## THE REGISTER'S BIGGEST CLAIM WAS FALSE — corrected 2026-08-30

`GH-PRODUCER-CENSUS` re-derived it. **"There is no measurement channel in this
build at all"** — repeated from `phase-9k.md`, and the reason lines 627–630 and
all thirty-four of Phase 51 read as unreachable — **is wrong.**

`evaluation_observations` shipped as **migration 15** (`database.rs:1515-1545`):
`kind`, `outcome`, `subject`, `session_id`, a `feature`/`arm` A/B pair,
`memory_id`. Its `kind` column deliberately carries **no SQL `CHECK`**
(`database.rs:1521-1522`) so the vocabulary can grow, and `evaluation/mod.rs:89-90`
says so: *"One variant, because this package lands one producer. Variants are
added as producers land, never in advance."* One production writer exists today
(`main.rs:4230` → `record_memory_retrieval`).

**The earlier recon searched for `fn score|Score`. The channel is not called a
score.** That is how a whole phase's blocker went stale without anyone noticing.

**The blocker moves from "no channel" to "no measured quantity."** And when you
ask which quantity, ~40 lines across six phases give one answer.

### Nobody counts tokens — and the wall around it has a door

`prompt_tokens|completion_tokens|total_tokens|cached_tokens` has **zero readers**
tree-wide. `routing/evidence.rs:65-67` names four columns "not supplied", and
`provider/resources.rs:952-954` prints **"Glasshouse does not count spend against
this"** to the user.

**Cluster L is right about the relay path and wrong as a blanket claim.**
`gateway/ingress.rs:455-458` refuses to parse a relayed body — deliberate, and
not to be revisited. **But on the disposable path Glasshouse makes the request
itself and already deserializes the whole document**:
`memory/extract/model.rs::content_of` (`:391`, production; `#[cfg(test)]` at
`:470`) calls `serde_json::from_str` and walks `choices[0].message.content`.
**`usage` is a sibling key, already parsed, already in memory.**

So the largest group splits: **buildable today** on the disposable path
(`GH-USAGE-READER`, dispatched), and **blocked on a design ruling** on the relay
path. **Do not conflate them again.**

### The ranked producer census — what to build, in order

| # | missing producer | open lines | migration? |
|---|---|---|---|
| **P1a** | usage reader on Glasshouse's own model calls | ~12 | **no** — in flight |
| **P1b** | usage reader on the relay path | 1333, 1263, 1158, most of 32E + 32G, much of 51 | no — **needs the `ingress` ruling** |
| **P2** | a caller that dispatches a Classification/Reranking disposable job | ~38 (34C, 34D, 34E, 1089–1092, 1455/1456) | no |
| **P3** | measured quantities for the evaluation channel | Phase 51 (34), 627–630 | no — **mostly P1+P2 renamed** |
| **P4** | durable sink for a routing decision | 1757, 1766, 1767, 1769, 1307 | likely — **in flight, do not repackage** |
| **P5** | Glasshouse as an MCP server | 1746 + Phase 43 (10) | no |
| **P6** | file-path association on memories | Phase 28 (5) | **YES** |
| **P7** | a retrieval-quality signal (score computed and dropped, `memory/search.rs:443`) | 1129, 1094, 939 | no |
| **P8** | provider health reaching the router | 1599, 1433, 531 in part | no |
| **P9** | a compaction event record | 310, 327, 1316, Phase 31 (7) | probably not |
| **P10** | a model axis on the candidate set | 566, 569, 35A/35B unchecked | no |
| **P11** | per-model capability ratings | 1475–1485 | likely |

**Not work, so nobody re-derives them:** Cluster E, Cluster F, 442 (a `keyring`
dependency decision), and the standing refusals 1294, 828, 829, 1323.


## A THIRD VERIFICATION TOOL WAS ANSWERING ABOUT THE WRONG TREE

**Found 2026-08-30 by `GH-USAGE-READER`, and it is the second instance of one
shape in a single day.**

`scripts/mutate.sh` derives `REPO` from its own location and resolves a relative
`--file` against it (`:111-116`). `scripts/` is tracked, so **every worktree has
its own copy**, and the invocation form silently decides which tree the tool
operates on. A worker running the **main checkout's** copy from its worktree
**mutates one tree and compiles the other.**

- Two real attempts reported **`KILLED`** falsely — cargo had exited non-zero
  with *"no test target named `usage_reader`"*, a target that existed only in
  the worktree.
- **The unhit case is the dangerous one.** When the file *and* the target exist
  in both trees, the mutation changes nothing that is compiled, the tests pass,
  and the tool reports a **clean false `SURVIVED`** with a perfect `test result:`
  line. **This project reads a SURVIVED as a finding** — practice calls it the
  most valuable outcome — so a false one manufactures a conclusion rather than
  losing information. **This is a new entry in §80's list of ways a mutation
  lies, and it is invisible in the tool's own output.**
- **The guard against it is disabled by the same bug.** The dirty-file check
  (`:124`) runs `git -C "$REPO" status --porcelain -- "$path"`; under an
  absolute-path invocation `$path` is outside `$REPO`, the query returns
  nothing, the file reads clean, and the guard no-ops.

### The shape, now seen three times — check every tool for it

`blast-radius.sh` had it (fixed today: it `cd`'d to its own checkout and
answered *"no changed .rs files"*, **exit 0**, about the wrong tree).
`mutate.sh` has it. Both derive a repo root from `BASH_SOURCE` and never ask
what tree the **caller** meant.

**Every script in `scripts/` that resolves a path or runs a build should be
audited for this**, because the failure mode is not an error — it is a
confident, plausible, wrong answer from the tools this project uses to decide
whether evidence holds. `GH-MUTATE-TREE` is fixing `mutate.sh` against
`blast-radius.sh`'s now-proven shape.

**Until it lands: invoke `mutate.sh` worktree-relative, never by absolute path
from another checkout.**


## P2 scoped: the classification consumer is written and waiting for a caller

`GH-CLASSIFICATION-JOB-RECON`, 2026-08-30. **It re-derived every line number and
found the census's had moved** (`content_of` is now a *test* helper; the
production parse is `parse_reply`) — §81 working as intended.

**1. This is a Cluster B join, not a build.**
`classify(request_text, Some(TaskClassification))` (`routing/classify.rs:576-581`)
takes exactly the object a model reply would produce, and its only production
caller — `report` (`classify.rs:595`, from `main.rs:145`) — **hardcodes `None`.**

**2. The extraction job is not the reusable half people assume.** `Prompt` is a
newtype with **one** constructor taking a `&SessionChunk`
(`memory/extract/mod.rs:146-201`), and the reply parser is extraction's own
schema. **The transport is reusable; the prompt, schema and parser are not.**

**3. The spend gate has one production call site, and neither branch both calls
and gates.** `evaluate_reserve_spend` is called only at
`routing/disposable.rs:712`, inside `DisposableRouting::choose`.
`disposable_extraction_model` (`main.rs:2554`) **early-returns**
`configured_extraction_model` before any routing decision, while the branch that
*does* route reaches `RoutedNoModel`, whose `complete` is
`Err(ModelError::Unavailable)` (`memory/extract/disposable.rs:133-135`). So
**nothing that passes the gate makes a call, and nothing that makes a call
passes the gate.**

**Stated carefully, because the framing matters.** For *extraction* this is
arguably correct and is deliberate — the model is one the user configured
explicitly, it runs once per completed turn, and `main.rs:2652-2655` documents
why no rationale is recorded on that branch. **It is decisive for
classification**, which is a request per routing decision — the cost Phase 34E's
lines 1463–1466 exist to bound. **A classification job copying
`configured_extraction_model`'s shape would inherit the bypass at far higher
frequency.** `GH-CLASSIFICATION-CALL` is therefore required to obtain its model
from `DisposableRouting::choose` on `Automatic`, and to fall back to the
heuristic rather than reach around for one.

**4. Two small honest gaps, no migration either side.**
`routing_observations.purpose` is `TEXT` with no `CHECK` (`database.rs:1161`),
bound at the INSERT and read back — the exact axis 1464/1465 need to separate
routing spend from task spend — **but `NewObservation` has no `with_purpose`
builder**; `purpose` is set nowhere but its `None` default. And
`ModelCall::observation()` deliberately leaves it unwritten: **extraction's rows
staying `NULL` is correct and must not be back-filled.**

## A gap in this project's own gate, found the same day

**`validate_round.py` supports §77 co-edits and requires them to be MUTUAL** —
both packets must carry a `COEDIT: <path>` line, because *"one worker knows it
is sharing and the other does not … is worse than a plain overlap."* That rule
is right, and it caught a real instance: `gateway-health-bridge` was dispatched
as `main.rs`'s sole claimant, and `classification-call` then joined.

**The packet template does not emit a `COEDIT:` line**, so the declaration is
only ever added by hand, after the validator refuses. Worth closing in
`new-packet.sh` — and until then, **when a peer joins a file after a worker is
already live, the live worker must be told**, because its packet was written
when it was alone. That relay was sent.
