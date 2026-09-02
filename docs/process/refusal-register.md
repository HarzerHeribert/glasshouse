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
| 1253 | **Added 2026-09-01** at `estimator-signals`' Phase −1. *"Preserve historical estimation data so the scheduler can improve over repeated usage."* The **inputs** are already durable and already preserved — the evidence-ledger rows the estimator reads have no prune, retain or purge path anywhere in `routing/evidence.rs`, and `estimate_subscription_headroom` (`routing/evidence.rs:1975`) is a pure function of them, so a past estimate is replayable today. **The line's second clause is what fails:** *"so the scheduler can improve"* names a consumer, and nothing scores on the estimate — `phase-32c.md` records it in its own words, *"Nothing scores on it yet — `routing/session.rs` untouched; a scoring consumer is a later ruling."* **Do not package this as a persistence task**: adding a table would answer a question the line is not asking and would break 32C's deliberate no-migration architecture. It closes the day a routing consumer reads the band. |

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

### Cluster L — Glasshouse refuses to parse the thing that carries the signal

> **2026-08-31, from GH-MAP-SIDE-EFFECT-AUDIT:** this cluster's note that *purpose is the one field nothing holds* is stale — `NewObservation::with_purpose` has five production call sites (routing-latency, classification, extraction, tier-escalation/downgrade, route-correlation). The ingress boundary below is unchanged. *(in-repo: the boundary is ours and deliberate — do not package without changing it first)*

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
| ~~327~~, ~~310~~ | **BOTH RETIRED 2026-08-30 — and the stated blocker was wrong for both.** **327 is CLOSED**: no lifecycle event was ever needed, because the line's *"or compaction-related "**state"* disjunct is satisfied by `sessions.observed_compactions` (migration 16), and a live Codex compacted five times against the shipped binary to prove it. **310 moved to Cluster E**: Claude Code emits no compaction event at all, so its blocker was never storage. |
| 1316 | a new persisted outcome value, same constraint. **Note: 1316 is a Phase 33 rate-limit line and was never a compaction line** — it sat in this cluster by transcription. |
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
| 1247 | **Added 2026-09-01** at `estimator-signals`' Phase −1, and it is Cluster H's shape in a phase Cluster H was not written for. *"Reset or re-calibrate an estimator when Glasshouse detects a plan change or materially different quota behavior."* The **present-tense** signal exists and is live in production: `KnownPlan` (`provider/quota.rs:1307`) is constructed at `provider/telemetry.rs:926` and `:1018`, and `RateCeilings` (`quota.rs:1134`) carries observed rate behavior. **Detecting a *change* needs two readings, and nothing keeps the earlier one.** The plan is not a column on `RoutingObservation`, so the ledger rows the estimator reads cannot witness it either, and 32C's estimator is deliberately stateless — *"no table, no migration, no persisted estimator state"* (`phase-32c.md`). This is therefore **not a wiring gap**: it needs a producer that durably records a prior plan reading, which is a schema decision (compare Cluster G) and a Red-tier ruling, not an implementation packet. |

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
| 1263 | *"Lower the score when user-defined spending budget is close to exhaustion."* The **producer exists** (`QuotaOverride::budget()`, config-loaded and layered) and the **consumer exists** (`remaining_capacity_score` → `normalized()` → `pools()`, which already includes `user_budget`). What is missing is **any count of what has been spent**. `routing/evidence.rs:66` states it: *"`cost_micro_usd`: not supplied."* There is no `SUM(cost…)` anywhere in the tree, and `provider/resources.rs:950` prints the budget to the user with the words **"Glasshouse does not count spend against this"**. "Close to exhaustion" is not computable. The only production writer of `CapacityState::user_budget` is `provider/telemetry.rs:1017`, which merges a *provider-reported* ceiling, not the user's configured budget; `resources.rs:2195` is past that file's `#[cfg(test)]` at `:1281`. ~~**Blocked on Phase 32G (provider-aware request-cost estimation), which is 10 open / 0 closed.**~~ **That sub-clause is STALE (2026-09-01): Phase 32G now stands 6/10, and 1307 closed, so `cost_micro_usd` IS written and round-trips through `EvidenceLedger::recent`. The row's verdict is unchanged and the line stays REFUSED — but for the other reason, and an orchestrator who re-checks only the stale half will package it.** The live blocker is the **relay-path usage reader**, stated in production at `main.rs:8613`: *"`routing_observations` has carried `input_tokens`, `output_tokens` and `cached_input_tokens` since migration 11 and nothing has ever written one: `gateway::ingress` relays a response body it is designed never to parse."* The one writer is `record_extraction_observation` (memory extraction), which is `None` *"for every run under the default configuration"*. `recent_credential_spend` (`routing/evidence.rs:1837`) is real, tested, and has **zero production callers** — it looks exactly like a Cluster B wiring job and is not one: wired today it would sum memory-extraction calls only, which is not the user's spend. This is register row **P1b**'s *"usage reader on the relay path"*, and it needs the `ingress` ruling first. Checked at Phase -1 on 2026-09-01 and **refused before dispatch**. |
| 1267 | Same function, stated in its own doc comment: *"**This build has no latency or concurrency reader anywhere** — nothing in `CapacityState` carries either quantity."* `remaining_capacity_score` returns a fixed high estimate for local inference carrying an explicit "no evidence" note, which is the honest answer and not the line. |

### Cluster N — a signal constant across the set being ranked *(in-repo: yes, and each has a tripwire)*

A signal that is the same for every candidate cannot change a ranking. This is a
distinct failure from "no producer", it looks exactly like a wiring gap, and it
has now cost two separate investigations.

| line | missing link |
|---|---|
| ~~1599~~ | **CLOSED 2026-08-30** by `GH-GATEWAY-HEALTH-BRIDGE`. The row was correct — no pool reached the router — and it expired the moment a bridge was built. **The lossy reverse map did not defeat it**: `provider_health` builds its own key from the destination, so the bridge renders each destination's label with the *same function the write side used* and compares forward only. No inverse is ever computed. Three ambiguities are **declined rather than resolved**, including two readings that disagree on one (label, model) — which is exactly what a genuine label collision looks like in the data. See `phase-37.md`. |
| 566, 569 | **STALE FOR `routing::session` (2026-09-02, `GH-RECON-56`): the constancy proof covers `routing::interactive`'s `UpstreamBackend` (no model field); `routing::session::Destination` carries a per-profile model, so a session-router candidate set CAN vary by `PairingClass`. Packaged as `GH-PAIRING-PRIOR` with 1540 and 1923; see `phase-56.md` 2026-09-02. The refusal below still holds for `routing::interactive`.** ~~Do not package.~~ `docs/product/evidence/phase-9j.md` records the full reasoning and a self-maintaining tripwire. `harness::pairing::classify` derives `PairingClass` from harness, model and user corrections — **never from the route** — while every candidate set the binary can construct varies *only* by route (`UpstreamBackend` has no model field; the one model arrives at `SessionRouting::bind` from `profile.model` and applies to every backend). So the native-pairing prior is constant across every set Glasshouse ranks. 569 is unreachable for the same reason: a warm session cannot outweigh a prior that never tipped anything. Separately, a fresh session does not reach the scorer at all — `best` has exactly two call sites, both in `on_provider_failure`. **The tripwire is `routing::interactive::tests::the_native_pairing_prior_is_constant_across_a_real_session_start_candidate_set`**: if anyone makes `classify` read the route, that test fails, and its failure means 566 has become reachable. |

### Cluster O — Phase 34F has no producer for any of its eleven lines

> **2026-08-31: 1480's producer now exists** — `RoutingTierObserved` (1834, `record_routed_session`'s third row, both routed exits) beside the harness-verdict outcome row (1835). What 1480 still lacks is the join reader; packaged as `GH-TIER-OUTCOMES`. The other ten lines stay blocked exactly as below. *(in-repo: yes; it is a build, not a wiring join)*

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

### Phase 32G — the census, and the two lines that were reachable

Derived 2026-09-01 at `6117446` while writing `pricing-channel`'s Phase −1.
The phase is **0/10 and stays mostly 0/10 for one reason**: *there is no
price data anywhere in this build.* A grep for
`price|pricing|per_million|cost_per` across `crates/glasshouse/src` returns
budget configuration (`config/mod.rs`'s `amount_micro_usd`,
`RouterCostMicroUsd`) and two empty database columns, and nothing else.
Four independent production doc comments already record the consequence —
`config/mod.rs:1786`, `routing/mod.rs:1035`, `routing/evidence.rs:90` and
`:130` all say `routing_observations.cost_micro_usd` has no producer.

`expected_marginal_cost` (`routing/session.rs:1237`) is the phase's only
consumer and it is **purely structural**: free versus metered, with a flat
`EXPECTED_MARGINAL_COST_PENALTY`. It consults no price at all.

| line | missing link |
|---|---|
| 1298, 1299, 1304 | **No estimated input size at the routing decision point.** `SessionContextFacts` (`routing/session.rs:197`) is the router's own context type and its four fields are `observed_compactions`, `last_task`, `touched_files`, `task_named_paths` — **no size or token field**, and `WarmSession` makes an explicit refusal about accumulated context (`routing/session.rs:137`). `firewall::estimate::estimate_tokens` is public but needs text the router does not hold. A size producer is the blocker for all three. |
| 1300 | the pricing half is buildable; the **usage** half is not — no cached-input signal exists (`cached_input_tokens` has no setter). **STALE 2026-09-02 (`GH-RECON-33A-32G`): `with_tokens` sets `cached_input_tokens` and translated exchanges write it; the live blocker is `ModelPrice` having no cached rate, by design — see `phase-32g.md` 2026-09-02, ruling parked.** Same root as Cluster H's 1760. |
| 1301 | no expected-output-size signal; *"recent comparable tasks"* would have to be turned into a token count nothing measures. |
| 1302 | same root as Cluster D's **531** — nothing distinguishes a request pool from a token-priced allowance and no `FreePool` outlives one call. **STALE 2026-09-02 (`GH-RECON-33A-32G`): `Allowance::RequestPool`/`is_request_pool()` exist with zero production callers (Cluster B, not D), and `routing/burn.rs` gives a persisted request-unit burn rate. Packaged as `GH-REQUEST-POOL-COST`; see `phase-32g.md` 2026-09-02.** |
| 1303 | latency aggregates exist and have a production reader; **occupancy does not**. Half a signal. |
| 1305, 1306 | **NOT REFUSED — packaged 2026-09-01 as `pricing-channel`.** These two ARE the channel: a metadata source updatable without recompiling, and the rule that an absent price reads *unknown* rather than a fake zero. §83's *"attack the channel"* in its clearest form — building them is what makes the six rows above re-derivable rather than permanent. |
| 1307 | **CORRECTED 2026-09-01 — it IS blocked, and by the same producer as 1298/1299/1304.** This row previously read *"not refused, and closer than any row here"*, on the ground that `RoutingObservation::cost` and `EvidenceLedger::record` both already exist. They do, and it does not help. `GH-PRICING-RECORDED` went to wire it and found the reason: **a per-million-token RATE is reachable, and a rate is not a cost without a token count to multiply it by.** `record_tier_movement` (`main.rs:~1651`) receives no `Destination` at all — `TierMovement` carries tier labels and reasons, nothing priceable. `record_entitlement_fallback` (`main.rs:~1698`) does receive one, so `PriceTable::price_for` answers there — but writing a rate into `cost_micro_usd`, documented as a monetary reading, would misrepresent `ObservedCost` and make the line's own *"compare estimate against actual usage"* meaningless. The worker refused rather than fabricate, which its packet told it to do. **1307 joins 1298/1299/1304 waiting on ONE thing: an input-size producer at the routing decision point.** That single producer now unblocks four of this phase's ten lines and is the highest-leverage unbuilt thing on the board. |

**The generalisable point, and it is not the first phase to show it:** a
phase at 0/N is usually one missing producer wearing N hats, not N problems.
Counting open lines said *"ten"*; reading the one consumer said *"one missing
producer, plus two lines that describe that producer itself"*. Those two were
the package.

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


## Phase 33B — fourteen lines, four causes, censused 2026-09-02

`GH-RECON-33B`. The phase had **no evidence-ledger entry at all** until that
day; `docs/product/evidence/phase-33b.md` now carries the full account. Summary
so nobody re-derives it:

| lines | cause | verdict |
|---|---|---|
| 1347-1352, 1355, part of 1354 | **Cluster L / P1b** — the relay-path body-parsing wall. `gateway/ingress.rs:36`/`:647` and `main.rs:~8612` re-verified word-for-word as still true. TTFC/TTFT/decode-throughput/rounds-per-minute symbols **do not exist in the tree at all** — stronger than Cluster B's built-and-unwired shape, and `shell/state.rs:695` says so in production prose | **REFUSED — needs the `ingress` ruling** |
| 1357 (weight clause), 1358 | routing score weights are compile-time `const`s with no config surface; nothing under `src/config` names one. 1357's *term-preservation* clause is already satisfied by `Contribution`/`RoutingExplanation` | **PACKAGEABLE** — successor named in `phase-33b.md` |
| 1353, 1359 | **already implemented in production and never ticked** — `provider_health`'s additive floored penalty, and the coarse observation path that is the only path that has ever run | **PACKAGEABLE** — dispatched as `GH-COARSE-FALLBACK` |
| 1356, 1360 | **Cluster P/Q** — vacuous restraints. Nothing anywhere parses terminal text for timing; nothing computes or compares TTFC | **REFUSED** |

**1354 is half-built**: `FailureClass::EmptyCompletion` and `::StreamAbort` are
production-constructed and consumed, classified from status and framing with no
parsing wall. Only *"unusable tool calls"* and *"non-actionable turns"* need the
body. Reuse those two variants when Cause A unblocks; do not rebuild them.

**Do not let the 1152 restraint ruling be misapplied to 1356/1360.** That ruling
(*restraint lines are mutation-proven by violating the restraint*) works because
1152's two stores both exist. Cause D restrains a mechanism that does not, so
the "violation" would mean building the forbidden feature in order to forbid it.
**The test is whether the restrained thing exists**, and the two shapes are
indistinguishable from the map line alone.

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
| **P1b** | usage reader on the relay path | 1333, 1263, 1158, ~~most of 32E~~ **32E line 1275 ONLY (censused 2026-09-02, see `phase-32e.md`: eight of the ten are packageable or need a ruling — *"most of 32E"* was wrong in the direction that keeps a phase shut)** + 32G, much of 51 | no — **needs the `ingress` ruling** |
| ↳ P1b, ruled in part 2026-08-31 | design-decisions §Phase 56 *"the user's answer on pairs"*: `ingress` keeps relaying served protocols byte for byte and never parses them — but a **translated** exchange (a target the provider does not serve, entering a codec) is parsed by construction, so its usage is recorded as *exact* where the provider states it. P1b therefore opens for translated pairs as `gateway-translate` (T1) lands, and stays refused for relayed ones. Readers must carry tokens where a row has them and say *not exposed* where it does not (`harness-efficiency`). | partly — translated pairs only |
| **P2** | a caller that dispatches a Classification/Reranking disposable job | ~38 (34C, 34D, 34E, 1089–1092, 1455/1456) | no |
| **P3** | measured quantities for the evaluation channel | Phase 51 (34), 627–630 | no — **mostly P1+P2 renamed** |
| **P4** | durable sink for a routing decision | 1757, 1766, 1767, 1769, 1307 | likely — **in flight, do not repackage** |
| **P5** | Glasshouse as an MCP server | 1746 + Phase 43 (10) | no |
| **P6** | file-path association on memories | Phase 28 (5) | **YES** |
| **P7** | a retrieval-quality signal (score computed and dropped, `memory/search.rs:443`) | 1129, 1094, 939 | no |
| **P8** | provider health reaching the router | 1599, 1433, 531 in part | no |
| **P9** | a compaction event record | **row corrected — see “Phase 31 and P9 scoped” below** | **no** |
| **P10** | a model axis on the candidate set | 566, 569, 35A/35B unchecked | no — **now REQUIRED by Phase 56 line 1953 (2026-08-31)** |
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


## Phase 28 scoped: the producer is OBSERVATION, and 1139's qualifier is unsatisfiable

> **2026-08-31:** Cluster H's row 417 ("no file-path association exists at all", 1139/1140) is stale for **1140** — migration 17 landed `memory_files` and `MemoryStore::for_path` reads it; what 1140 lacks is a production caller, packaged as `GH-WIRE-FILE-MEMORY` (with 1143). 1139 stays refused exactly as below.

`GH-FILE-MEMORY-RECON`, 2026-08-30. It corrected **four** claims it was handed
(§81), including that the `memories` column list in circulation is **migration
4's**: migrations 6, 10 and 13 have since added eighteen more, for **30 columns**
(`database.rs:654-668`, `:1058-1070`, `:1298`). None is a path, so the
conclusion survives — but the evidence for it had drifted.

**No existing table or column can hold a file association.** `memories` has no
path column; `evaluation_observations` is a *deliberately prunable* ledger whose
`subject` is documented as *"never a count key on its own"*;
`checkpoints.document` holds real observed paths but at **session** granularity,
in opaque JSON, reachable only by a full scan.

**So a migration is needed — migration 17, one table plus one index**, reusing
migration 11's project-scope trigger pair. No `ALTER`, no rebuild, no existing
`CHECK` touched, `lifecycle_events` untouched — **Cluster G's 310 / 327 / 1316
keep their refusal exactly.**

### The finding that decides the package

**On the production automatic extraction path the model's input contains no
prose at all.** So map line 1139's own qualifier — *"when extraction can
identify them reliably"* — is **not merely unmet there, it is unsatisfiable.**

**The honest producer is observation, not extraction**:
`WorkingTreeStatus::changed_files` (`checkpoint/git.rs:117-126`) already ships
in production at four call sites. Rows therefore carry provenance **`observed`**
and **never `referenced`** — because *observed-dirty* is not *explicitly
referenced*, and asserting otherwise closes a box by fabricating a producer.

**`GH-MEMORY-FILE-OBSERVER` is dispatched and closes ZERO of Phase 28's five
boxes, deliberately.** That is the fourth producer package this week to close
nothing on purpose. The consumer, and the boxes, come after.

**§79's fifth question is answered**: the value genuinely varies — two sessions
with disjoint dirty sets produce disjoint rows, and a clean tree produces none.
The characteristic mutation is making the writer store the whole index instead
of the changed subset, which is exactly what a producer that associates every
memory with every file would look like.


## The signal-disposition class: audited, and it was one test

`GH-SIGNAL-ENV-AUDIT`, after the interrupt "flake" turned out to be a
`SIG_IGN`-on-entry defect. **`worker_access.rs` was the only test at risk and it
is fixed.** Everything else either sends `SIGKILL` or probes with signal `0` —
neither affected by an ignored disposition — or goes through the product's pty
path.

**And the product is structurally immune, for a reason nobody wired on purpose.**
`portable-pty` 0.9.0 resets `SIGCHLD`/`SIGHUP`/`SIGINT`/`SIGQUIT`/`SIGTERM`/
`SIGALRM` to `SIG_DFL` in its own `pre_exec` before every child, on every
platform this project runs. That is the same fix `worker_access.rs` now applies
by hand, already present one layer down for every real harness Glasshouse
starts. **So a user launching `glasshouse` from a `SIG_IGN` context can still
interrupt a worker** — and it is why `pty_smoke`'s interrupt tests passed in the
same gate leg where the other one failed.

**Recorded because it is load-bearing and undocumented**: a dependency upgrade
that dropped that `pre_exec` would silently reintroduce the defect in
production, where no test currently watches for it.

## Phase 28's read door needs a ruling, and an invariant from this morning caught it

`GH-MEMORY-FILE-OBSERVER` built migration 17 and **stopped** on `for_path`.
Beyond `group()` being module-private, the real reason is a design question:

**`group()` takes `Scored` — a record *plus a BM25 relevance* — and a path lookup
has no relevance to supply.** Passing `0.0` would manufacture a relevance for a
memory no query ever matched, which is exactly what `RetrievalResult.relevances`
was made private to prevent. Its doc comment, written the same day by
`GH-RETRIEVAL-SCORE`, says so: *"a caller that could insert into it could
manufacture a relevance for a memory no query ever matched"*, and a zero *"would
be a fabrication that reads as 'matched as badly as possible' rather than 'was
not asked about'."*

**An invariant landed in the morning stopped a fabrication in the evening**, one
level below the one that package had already refused.

**The ruling, so the next package does not re-derive it:** the honest shape is
`Scored`'s relevance becoming `Option<f64>`, with `None` meaning *not asked
about* — which is exactly the distinction the doc comment draws, and it
*strengthens* the invariant rather than piercing it. A path lookup then keeps
the ladder ordering that map line 1141 wants, without inventing a number.


## Phase 31 and P9 scoped: the census row was wrong in three ways

`GH-COMPACTION-RECON`, 2026-08-30, read-only. The orchestrator independently
confirmed the two decisive claims (1316's phase, and `SessionContext`'s caller
count) before acting on any of it.

**The row said `310, 327, 1316, Phase 31 (7)` and "migration: probably not".
Every part of that needed fixing.**

1. **1316 is not a compaction line and never was.** `capability-map.md:1316` is
   *"Track recent rate-limit responses separately from transport or model
   failures"*, under **Phase 33 — Resource health** (`capability-map.md:1309`),
   filed at `phase-33.md:463`. It entered P9 by transcription and stayed
   because nobody re-read it. **Removed from P9.**
2. **310's blocker was never storage, so Cluster G is the wrong home.**
   `harness/claude_code.rs:28-38` — a catalogue documented as *observed, not
   catalogued* — has **no compaction event of any kind**, and
   `session/lifecycle.rs:157-158` says so in its own words. The missing link is
   outside this repository. **Re-filed from Cluster G to Cluster E: the signal
   genuinely does not arrive, do not package.**
   *Caveat worth keeping:* the catalogue is observed rather than documented, so
   only a fresh probe of a live installation can change this verdict — an
   observation, not code. And two records disagree about `TeammateIdle`
   (`phase-7.md:43-46` lists it, `harness/claude_code.rs:28-38` does not); one
   is stale and this recon did not resolve which.
3. **327's blocker moved and the register did not notice.** `phase-8.md:42`
   says it waits on *"Phase 30's per-session compaction count"*. **Line 1159 is
   now ☑ and that count ships.** What 327 lacks is a **production reader** —
   which is in-repo, and small.

**No line in P9 needs a migration.** Not one. `lifecycle_events` stays
untouched (`LIFECYCLE_EVENT_KINDS` is eleven values, `database.rs:105-117`, none
a compaction), so `design-decisions.md:2493-2495`'s schema claim still stands —
**but the sentence attached to it no longer describes 327**, and should not be
quoted as if it did.

### Cluster Q claims three more: 1169, 1172, 1173

**Glasshouse never compacts anything.** The only `/compact` string in the tree
is `shell/state.rs:5622`, inside a `#[cfg(test)]` block (boundary `:4967`) whose
whole point is that Glasshouse forwards the keystrokes untouched. No production
path decides to compact, requests a compaction, or schedules one.

So *"**never** compact … because the cache is cold"* (1169), *"**prefer**
compaction at semantic boundaries"* (1172), and *"**allow** the harness its own
mechanism rather than **replacing** it"* (1173) each forbid or prefer something
no code path could do. 1173 is structurally immune twice over: `glasshouse hook`
drains its payload into `io::sink()` unread (`main.rs:3362-3364`), so the binary
never holds a session's conversation at all.

**A test passing here passes because the feature is absent.** That is the exact
shape 1455 and 1456 were un-ticked for. **Do not package.**

### What IS reachable: 1171, and only 1171

> **STALE as of 2026-08-31: 1171 is ☑ on the map.** The analysis below stood when written; the line has since closed. `cluster-b.py` still lists `NewObservation::with_context_state` (`evidence.rs`) with no production caller — that is Phase 33A's 1334 `context_state`, blocked with 1331–1334 on the `ingress` ruling, not 1171.

*"Prefer creating or refreshing a portable checkpoint before intentional
compaction when practical."* All four Phase −1 links stand in current
production code, no ruling in front of it, no migration:

- **producer** `harness/codex.rs:34-44` and `:63-71` — Codex's `PreCompact`,
  in production `REPORTED_EVENTS`, asserted to reach the `hooks.json` Codex
  actually reads (`session/select.rs:1102-1108`);
- **caller** `main.rs:3411-3444` — the `PreCompact` arm, which today counts
  (`:3429`) and runs extraction (`:3437`) and does nothing about checkpoints;
- **propagation** `Checkpoint::capture` + `CheckpointStore::save`, the shape
  `checkpoint_after_turn` already uses nineteen lines away (`main.rs:3488-3503`,
  gated at `main.rs:2530`);
- **consumer** `glasshouse checkpoint list` (`main.rs:4363`) and
  `resolve_bootstrap_prompt` (`main.rs:4331`) — both production.

`main.rs` is structurally contended; claim it with `scripts/coedit.sh` (§77)
rather than queueing behind it.

### A RULING on 327, so the next package does not re-derive it

> **Does a durable per-session count with no timestamp —
> `sessions.observed_compactions` — satisfy 327's "Record observed Codex
> compaction events *or compaction-related state* when available"?**

**Yes, the count satisfies the *state* disjunct.** The line is written
disjunctively, and reading it to require one timestamped row per occurrence
reads the "or" out of it. The disjunct exists for exactly this case: the only
signal Codex sends is *"about to compact"*, nothing confirms the compaction
afterwards (`session/lifecycle.rs:145-156` excludes `PostCompact` on purpose),
and a per-occurrence row would therefore carry a time and no verified event.
The column is durable, project-scoped, and distinguishes `NULL` (nobody was
counting) from `0` (counted, none seen) from `n` — `database.rs:1682-1698`
argues that distinction at length, and any reader that prints `0` for `NULL`
collapses it and is wrong.

**327 still does not close on that ruling alone.** It needs two things:

1. a **production reader** — one `line(…)` in `session_detail`
   (`main.rs:5795-5857`), which today prints nineteen fields and not this one;
2. **one real Codex `/compact` observed end to end**, which `phase-8.md:39`
   already named as the remaining work. That is a runtime probe, not a test.

So 327 is packageable *with* a manual probe attached, and 1171 is packageable
without one. **If the board wants a box that closes without waiting on a
person, it is 1171 alone.**

### The lead this recon flagged and did not chase

`SessionStore::context` (`session/store.rs:2020`) has **zero production
callers** — confirmed independently by the orchestrator: `SessionContext`
appears in `crates/glasshouse/src/` only at its definition (`store.rs:802`), its
one constructor (`:2060`), a doc comment (`:1999`), the re-export
(`session/mod.rs:61`), and an unrelated comment (`database.rs:1633`). All 14
callers are in `tests/session_context.rs`.

It is the only place **five already-ticked Phase 30 lines** produce anything a
caller could see: **1161–1165**, all ☑. `GH-PHASE30-AUDIT` is dispatched against
them. 1159 and 1160 are *not* affected — 1159's claim is the write at
`main.rs:3429`, and `last_activity_at` is rendered by `session_detail`.


## Cluster R — P5 has no producer either, and it is a product-scope decision

**Checked by the orchestrator 2026-08-30, directly, because P5 sits high in the
census and its ten open lines read as cheap.** They are not.

`grep -rin "mcp" crates/glasshouse/src/` returns **only harness capability
declarations** — `Declared<bool>` fields recording that *Claude Code, Codex,
OpenCode, Cursor, Hermes and Antigravity* support MCP (`harness/mod.rs:636`,
and one per adapter). **Nothing in this tree makes Glasshouse an MCP server**,
and `Cargo.toml` names no MCP dependency at all.

So every line of Phase 43 — 1694–1703, *"Expose session listing / worker
spawning / session messaging / session status / worker interruption /
project-memory search / checkpoint retrieval through MCP"* — plus 1746's
*"automated tests proving MCP operations remain bound to the active project"*
is a **build, not a wiring join**. This is Cluster O's shape exactly, one phase
over: there is no producer to connect, and no amount of Phase −1 diligence
turns ten "expose X" lines into a package when the surface they expose through
does not exist.

**And it is not merely large — it is the orchestrator's scope boundary.** An
MCP server exposing *worker spawning*, *session messaging* and *worker
interruption* adds an external control surface to a product whose invariants
say cross-project access is disabled structurally and that PTY/process
ownership is correct on every claimed platform. It needs a dependency choice, a
transport decision, and a security model for who may call it. `worker-capabilities.md`
puts *"broaden product scope"* under the orchestrator's **Do not**.

**So: do not package Phase 43 from the map alone. It is a design decision for
the user, and 1702/1703 are the two lines that make it one.** Recorded here so
the next orchestrator ranking phases by open-line count does not spend a round
discovering this again.


## ~~NOT a refusal — 1161–1165 un-ticked~~ — CLOSED the same day, 2026-08-30

**`GH-PHASE30-AUDIT`, 2026-08-30.** Recorded here so the next orchestrator does
not read five freshly-opened lines as a phase that failed. They are open because
they were **wrongly ticked**, and they are among the cheapest closes on the
board.

`SessionStore::context` (`session/store.rs:2020`) is the sole construction site
of every value 1161–1165 name, and it has **zero production callers** — all
fourteen are in `tests/session_context.rs`. `session_detail`
(`main.rs:5795-5857`) renders eighteen fields and never calls it, and
`CheckpointRecency::is_current` (`store.rs:727`) has no caller anywhere, not
even a test. Confirmed by the orchestrator directly before un-ticking.

**The repair, already scoped:** one `store.context(&id)` call in
`session_detail`, four render lines, and `Display` for `CheckpointRecency` and
`TaskContinuity` (`AdvisoryCacheState` has one already, and it prints
`"hot (estimated)"`, which carries 1163 by itself). ≈30 lines, two files, no new
type, no migration. The regression test belongs in `tests/session_model.rs`,
which already drives the real binary and has a `field(&report, label)` helper
for `glasshouse sessions show`; deleting the `store.context` call must fail it,
so the mutation lands on the **call** (§35).

**RESOLVED.** `GH-SESSION-CONTEXT-DOOR` made the repair in 34 lines of
`main.rs` and 32 of `store.rs`, and all five are ticked again — this time on
tests that drive the real binary through `glasshouse sessions show`, with the
delete-the-call mutation KILLED. Kept here because the *pattern* below is the
transferable part, not because the lines are still open.

**Two cautions for that package.** `main.rs` is contended — use `coedit.sh`.
And production prompt-cache reasoning already exists under a *different*
vocabulary: `routing::session::prompt_cache_state` (`routing/session.rs:800`,
called at `:1459` from `main.rs:1354`) speaks `Preserved`/`Lost`/`LikelyLost`
about a comparison **between two backends**, for lines 1596/1597. Do not
introduce a second user-visible cache vocabulary that disagrees with it.

### The generalisable finding, and it is the third instance

The package that shipped these five **found this exact defect in inherited code
and did not turn the test on itself.** Its own `packet_errors` says
`SessionStore::touch` has *"NO production caller at all — all four call sites
are tests … `touch` is a Cluster B candidate"*. Correct, and still true. It
never asked the same question about `SessionStore::context`, which it was
shipping in the same diff, and no reviewer asked it either.

**So: when a report names a Cluster B candidate in inherited code, ask the same
question about the code that report is shipping.** That is now the cheapest
known way to catch this class, and it has produced un-ticks twice —
1455/1456 on 2026-08-30 morning, 1161–1165 the same evening.

**And the review that would have caught it was already written down.**
`phase-30.md`'s own REVIEW section says, for each of these five: *"verdict
`closed`. Re-run one decisive mutation yourself, then rule (§79: a worker's
packet does not bind the integrator)."* The step exists, it was owed, and it was
skipped.


## Phase 51's thirty-four lines reduce to FOUR root causes, not thirty-four

`GH-PHASE51-RECON`, 2026-08-30. **1829 and 1830 were the one packageable pair
and they closed the same evening.** This is what is left, grouped by first
failing link (§83) so nobody re-derives thirty-four paragraphs.

**RC-A — decided in production, announced to the user, dropped.** The quantity
is computed on a path the shipped binary takes, printed to a terminal or a
`tracing` line, and never written. **Nothing was missing but the producer
call.** This was 1829/1830, and it is now **empty** except the recordable half
of 1851. *This is the cheap cluster; check it first in any phase.*

**RC-B — no outcome is ever learned.** `EvaluationOutcome::Unknown` is the only
value written, and `routing_observations.outcome` is written only by the
gateway, where `evidence.rs:70-83` calls it *"a transport-level fact, not a
statement about whether the turn actually helped the user."* Every line whose
verb is *succeeds*, *is useful*, *is justified*, *correctly identifies*,
*predicts*, or *causes a poor decision* fails here: **1821, 1823, 1824, 1825,
1831, 1834, 1835, 1836, 1837, 1846, 1852, 1854**.

> **This is twelve lines behind one question, and the question is a product
> decision, not a wiring job: how does Glasshouse learn whether a routing
> decision was good?** Nothing in the build observes that today, and inventing
> a proxy would be a fabricated denominator of the kind line 1294 refuses. **Do
> not package any RC-B line until a person answers it.** It is the single
> highest-leverage unanswered question on the board.

**RC-C — the column exists and has no producer.** `first_byte_at`,
`first_token_at`, `first_tool_call_at`, `tool_rounds`, `retries`, `repairs`,
`failovers`, `cost_micro_usd` — schema present since migration 11, no builder,
no writer. Lines **1820, 1839, 1845, 1849, 1850, 1855**, plus the *money*
reading of 1832/1833. Buildable, but each needs its own producer.

**RC-D — the feature the line measures does not exist.** A missing subject, not
a missing measurement: **1827, 1828, 1838, 1840, 1841, 1842, 1843, 1844, 1847,
1848, 1853**.

### Three specific traps recorded so they are not re-derived

- **1849 looks like the best package in the phase and is not.** Its consumer is
  already built and already reachable — `routing_latency_phrase`
  (`shell/mod.rs:1585`) prints *"unknown — not enough observations yet"* to
  every user today — and the missing link is one clock read. **It loses on
  §36**: the line says latency *"added before interactive task execution"*, and
  `glasshouse classify` is the only production caller of the classification
  path. It is a command a person types, not something on `launch_session`'s
  path. **It becomes a good package the day classification is wired into
  launch, and not before.**
- **1845 is not "wire the pairing evidence".** It is already wired;
  `ObservedEvidenceSource::observed` hardcodes **five** of the line's six
  quantities to `None` because their columns have no writer. The package is
  three producers, not a join.
- **1852 would report a tautology.** `FailureDomain::Independent` is *"the
  state this build never earns"* (`routing/domain.rs:30-34`). The honest answer
  is a documented constant, not a count.


## 1331 — three of five timestamps recorded, and the box stays OPEN

**Ruled by the orchestrator 2026-08-30 after `GH-GATEWAY-FIRST-BYTE` landed.
The worker was forbidden from deciding this and correctly reported
`verdict: open`.**

The line: *"Record dispatch time, first-byte time, time to first real token,
time to first tool call, and completion time **when the protocol exposes
them**."*

The package is good and it shipped: `first_byte_at` now has its **first
production producer and its first production consumer** — the gateway takes the
clock once when upstream's first byte arrives, and `glasshouse routing-cost`
prints a per-group sample count and mean time-to-first-byte, with an untimed
group saying *"not recorded"* rather than `0ms`. Both mutations killed.

**But three of five is not five, and the qualifier does not rescue it.**
*"When the protocol exposes them"* is a statement about the **protocol's**
capability, not about what Glasshouse chooses to look at. For a streaming
provider the protocol **does** expose a first-token boundary; `gateway::ingress`
declines to parse the body that carries it. That refusal is deliberate,
documented, and correct — it is **Cluster L**, *"Glasshouse refuses to parse the
thing that carries the signal … the boundary is ours and deliberate — do not
package without changing it first."*

So closing 1331 here would mean reading the qualifier as *"when Glasshouse
chooses to look"*, which is the same qualifier-stretch that un-ticked **1455 and
1456** the same morning. **Do not tick it on this evidence.**

**What 1331 actually needs:** a decision about whether the relay may observe
*framing* — the boundary between response chunks — without reading content.
That is a narrower question than "parse the body", and it may have an honest
answer. **It is a product decision and it belongs with the `ingress` ruling
that already blocks P1b's relay path.** Until someone answers it, the line's
remaining two timestamps have no honest producer.

`docs/product/evidence/phase-33a.md` records the entry as **PARTIALLY
VERIFIED**, which is the state `agent-sdlc.md` defines for exactly this: one
contract clause proven, another required clause missing.


## Phase 34C's remaining filters — six refused, and every one names its producer

`GH-ROUTING-FILTERS`, 2026-08-30. **0 closed / 1 open / 6 refused, zero files
changed.** Read that as the phase being honestly mapped, not as a failed
package: the sibling package closed 1431, 1433 and 1443 against the same
selector hours earlier, and what is left genuinely lacks signals.

**Grouped by root cause (§83), because six paragraphs would hide that there are
only four reasons.**

- **1435 (latency) and 1436 (cost) — the qualifier has no reading.**
  `RouterLatencyMs` has exactly two consumers, both in the settings overlay, and
  **no routing decision reads it**; `cost_micro_usd` has no production producer
  at all. Same ground that refused 1437 and 1438. **Do not package either
  without building the reading first.**
- **1432 (structured-output reliability) — not represented on a candidate.**
  The concept does not exist on the type `choose` ranks. A build, not a join.
- **1439 (cheap metered over unreliable free) — fails on the price half.** It
  needs both a reliability signal and a price comparison, and the price side is
  1436's missing producer.
- **~~1441 and 1442~~ — CLOSED 2026-08-31, and this row is why.** The refusal
  named one shared missing producer; `GH-ROUTING-STICKINESS` built it and
  `GH-STICKY-WIRING` wired it, both within hours. **This is what the register
  is for** — a refusal that names its producer is the input to the next
  package, not an archive entry (§83). Original reasoning kept below.
  Nothing retained the last
  automatic pick: `automatic_classification_choice` is a pure function of its
  inputs and re-runs `choose` in full every call, and `classify` is a fresh
  process each time. 1441 needs a prior choice to *reconsider*; 1442 needs one
  to *hold onto*.

### The distinction that keeps these honest, and it is reusable

**1434 is `open`, not closed, and the reason generalises.** The RPM-headroom
figure **does** reach `choose` — but it is read in exactly one place, `score()`'s
normalized-capacity contribution, which affects **ranking, never eligibility**.
Every place `choose` actually removes a candidate ignores it. *"A candidate at
0% headroom is scored lowest but is never excluded, which is a different claim
from the line's 'filter'."*

**A signal that reaches a decision is not the same as a signal the decision acts
on.** Check which one a line asks for before crediting it.

The worker also refused the tempting close on 1441, unprompted: crediting it
with *"every call already recomputes against live health"* would prove the
inputs are current, **not** that a prior pick is being re-evaluated.

### A packet defect, recorded against the orchestrator

**That packet anchored its worker toward refusal.** It pre-judged 1435 and 1436,
told the worker to *"expect"* 1439 to fail on price, and pre-framed 1441/1442 as
one missing mechanism. §44 says a packet's hypothesis is an anchor and must be
labelled killable; that one was not.

The verdicts survive scrutiny on their own `file:line` evidence — the 1441
reasoning in particular was reached against the packet's framing rather than
along it — but **the next packet on contested ground must say plainly that
disagreeing with it is a good outcome.** `GH-ROUTING-STICKINESS` says so.


## "Fewest open lines" is DEAD as a selection heuristic — six of six refused

`GH-LAST-LINES-RECON`, 2026-08-31, closing the question the same day it was
raised.

`ORIENT.md` ranks phases by open lines, fewest first, and ten phases sit **one
line from complete**. That reads as the cheapest board on the map. **It is
not.** Six were checked against current source and the register:

| line | phase | why it is one line from done |
|---|---|---|
| 1263, 1267 | 32D | Cluster M — no spend counter, no latency reader |
| 1294 | 32F | standing refusal; the source itself refuses it |
| 1158 | 30 | refused in `phase-30.md` |
| **514** | 9H | **REFUSED — missing caller** |
| **531** | 9I | **REFUSED — missing caller *and* consumer** (Cluster D holds; one supporting fact corrected) |
| **1594** | 37 | **REFUSED — and it was already recorded** in `phase-37.md:6,32`, reproduced here with a passing tripwire test |

**Six of six.** A phase is one line from complete *because* that line is the
hard one — the cheap lines in it were closed first, by construction. **Ranking
by fewest-open finds the residue, not the opportunity.**

**What to use instead**, both proven on this map the same week:

- **Size by mechanism**, as practice section 87 sets out: a phase whose first line is a mechanism and whose
  rest are its filters (34C — 1431 selects, 1432–1443 are its rules), or several
  lines that are fields of one returned value (Phase 30's 1161–1165, one
  `SessionContext`). Those produced 3-closed and 5-closed packages.
- **A refusal that names its producer**, as practice section 83 asks. 1441/1442 were refused with one
  shared missing producer named, and closed **within hours** once it was built
  and wired.

### And read the evidence entry before queueing a line

**1594 was already refused in `phase-37.md` and the orchestrator queued it
anyway**, having checked only the map and the register. The ledger is the third
place a refusal can live and it was not consulted. `discover.py --phase <id>`
prints the entry alongside the open lines for exactly this reason.

## The register's own rows go stale, and three of them did — 2026-08-31

Checked while choosing batch 57's refill, and each was checked *because* the
row said the work was buildable. **A stale "buildable" row costs a dispatch;
a stale "refused" row costs nothing.** That asymmetry is why these are
recorded here rather than left to the next reader.

| row | said | is now |
|---|---|---|
| **P2** — a caller that dispatches a Classification job | *"buildable today, ~38 open lines"* | **CLOSED by `58e4d2c`.** `classify_with_routing_model` (`main.rs:4453`) is called from `main.rs:154` and `:2130`, and `classify_for_routing` (`:2066`) from `:1881` and `:2580`. The census's *"its only production caller hardcodes `None`"* is no longer true. |
| **P2's note 4** — *"`NewObservation` has no `with_purpose` builder"* | a gap | **EXISTS** at `routing/evidence.rs:624`. |
| **P7** — a retrieval-quality signal | *"buildable, no migration"*, closing 1129 | **1129 is REFUSED IN THE SOURCE.** `memory/inject.rs:59-60` and `:200-239`: *"Glasshouse has no honest retrieval-confidence signal to threshold today"*, and it distinguishes the *relevance* BM25 gives from the *confidence* the line asks for — a confidence derived from BM25 *"would be high for"* a match nothing should inject. The score is produced and carried now (`Scored`, `search_scored`); the consumer is the part that was ruled against. **Do not package 1129.** 939 and 1094 are untouched by this. |

### And the handoff's refill list is a fourth place a refusal can live

`.agent-runtime/CONTINUATION.md` offered **514** as a ready candidate. This
register has had it as **"REFUSED — missing caller"** since batch 50. A
checkpoint is written under time pressure at the end of a session and is not
re-checked against the register; **the register outranks it.** Read this file
before taking a candidate from a handoff, exactly as before taking one from
the map.

## A THIRD verification tool was answering about the wrong question — and this one manufactured a KILLED

**Found 2026-08-31 by `GH-IMPLEMENTATION-POLICY`; fixed the same day.**

`scripts/mutate.sh --script` parsed its rows with
`while IFS=$'\t' read -r file find replace name testargs`. **Tab is an IFS
*whitespace* character**, so a run of tabs collapses into one delimiter and
every later field shifts left. A row with an **empty replacement** — which is
exactly what a *deletion* mutation is, section 35's own shape and the most valuable kind
there is — therefore became four fields:

- `replace` received the mutation's **name**, so the wrong text was substituted;
- `testargs` came out **empty**, so `TEST_ARGS=()` and `cargo test` ran the
  **whole workspace**, whose failure was then reported as a **KILLED for the
  mutation you named**.

Both of that package's *"delete the delivery call"* rows came back KILLED from a
command that was never the one named. Re-run with a compiling non-empty
replacement (`if false { deliver_policy(...); }`), one of them was a **real
SURVIVED**: every assertion reached `deliver_policy` through `spawn_session`,
and the `Request::SendMessage` call site was a caller no test entered through.
The worker closed it with a test rather than by adjusting the claim.

**This is the third instance of one shape** — after `blast-radius.sh` and
`mutate.sh`'s own `--file` resolution — and the first to *manufacture* a
verdict rather than lose one. A false SURVIVED costs a look; **a false KILLED
retires a question that was never asked.**

Fixed by splitting on `\x1f` (not IFS whitespace, so empty fields survive) and
refusing any row that is not exactly five tab-separated fields. **Every
`--script` mutation reported before 2026-08-31 whose replacement was empty is
unreliable.** Batch 57's other packages were checked and are not affected —
their deletion-shaped mutations all used compiling non-empty replacements
(`-> let landed: Option<String> = None;`, `-> match (landed, false)`).

### Cluster E, two more rows — Phase 9's Antigravity lifecycle lines, checked 2026-08-31

| line | why it cannot be packaged |
|---|---|
| **340** *"Integrate structured Antigravity lifecycle events where the CLI exposes them."* | The qualifier is the whole answer: the CLI exposes none. `harness/antigravity.rs:194` and `:333` record, in the adapter's own words, that there is *no hook, event, or notification mechanism anywhere in it* — the adapter learns a session's conversation identifier by reading a shared index file after the fact (`phase-9.md`, lines 2 and 3), which is the opposite of an event. A signal that does not arrive cannot be integrated. |
| **341** *"Translate supported Antigravity lifecycle state into Glasshouse lifecycle events."* | Downstream of 340: nothing to translate until 340 has a producer, and 340's producer is the vendor's, not ours. |

Both stay open. A tripwire is not worth writing: the moment the CLI grows an
event surface, the adapter's own `:194` comment is what a worker would have to
delete first, and that deletion is the signal.

## Phases 52 and 53 — eleven lines censused 2026-09-02: six Cluster Q, one refused with a successor, four packaged

`GH-RECON-52-53`; the rulings are in `phase-52.md` and `phase-53.md`. The
tree-wide fact: no vector, embedding, semantic-retrieval or graph-database
code exists anywhere in `crates/glasshouse/src`, and `JobKind::Reranking` is a
declared variant with no production caller.

| line | why it cannot be packaged |
|---|---|
| **1867** *"If semantic retrieval is added, combine it with lexical retrieval…"* | Cluster Q: no second retrieval path exists to combine with the lexical one. |
| **1868** *"Keep project isolation physically intact when adding embeddings."* | Cluster Q: no embeddings table or column exists to isolate. |
| **1869** *"Ensure semantic retrieval respects memory lifecycle status…"* | Cluster Q: nothing can resurrect a superseded memory while one retrieval path exists; the lexical precedent to copy is `memory/search.rs:44-54`. |
| **1870** *"Evaluate semantic retrieval on real Glasshouse queries before making it part of the default path."* | Cluster Q in an evaluation gate's clothing: the object of the evaluation does not exist. |
| **1879** *"Do not add a graph database solely to visualize project memory."* | Cluster Q: no graph database exists; line 1107's tripwire guards the widget, not a database, and would not tick this regardless. |
| **1882** *"Evaluate whether SQLite relations are insufficient before adopting a dedicated graph database."* | Cluster Q on the restraint reading; the evaluation itself is recorded in `phase-53.md` (one relationship ever needed, ever built) and closes the line the day a graph database is proposed. |
| **1866** *"Define concrete retrieval cases that lexical search cannot solve…"* | **Not Cluster Q** — a reachable question with no material: no recorded lexical failure exists. Successor: revisit after `GH-RETRIEVAL-CRITERIA`'s miss rows accumulate. |

Packaged: **1865** (`GH-RETRIEVAL-CRITERIA`, Amber) and **1880, 1881, 1883**
(`GH-RELATIONSHIP-PROOFS`, Green, tests only).

### Wave-80 audit, same day — four boxes un-ticked, all the "no producer" shape

`GH-AUDIT-WAVE80` confirmed eighteen of twenty and re-opened **1517** and
**1513** (`phase-35a.md`): the fact `is_adequate` and the tool-semantics gate
exclude on is never constructed by any adapter, template or config key —
`Destination::with_resource_facts` has one caller and it is a test. The same
day's `cluster-b.py` reading re-opened **1822** and **1826** (`phase-51.md`):
`stale_retrievals` has no production caller. Successors: `GH-CAPABILITY-FACTS`
(a declared producer for both facts) and `GH-RETRIEVAL-CRITERIA` (the
readout), respectively. Twelve wrongly-ticked boxes had this shape before
today; **all sixteen were found by an audit or a script, none by the diff
read that preceded the tick.**


## 1367 and 1369 — censused 2026-09-02, and the finding underneath both

| line | missing link |
|---|---|
| 1367 | *"Reserve known paced capacity at dispatch so concurrent workers do not all consume the same apparent allowance."* Overlap is real and supported (two hook processes, or a hook racing `memory commit`), but the apparent allowance lives nowhere a second dispatch could see: `RoutedNoModel::new` builds an empty `FreePool` per call and drops it, and — the finding — **chooses and then calls no model at all** (`memory/extract/disposable.rs:1-30`, its own words), while a configured extraction model bypasses the disposable router entirely (`main.rs::disposable_extraction_model`). A reservation protects capacity a dispatch does not spend. Successor `GH-DISPATCH-RESERVATION-ROW` (Red) is **blocked behind `GH-ROUTED-EXTRACTION-CLIENT`** (Red): the routed choice must drive the real extraction request, record it, and feed `FreePool::observe`, before a reservation means anything. Full census: `phase-33c.md` 2026-09-02. |
| 1369 | *"Reduce or suppress active probes when probing would consume a material fraction of a scarce request pool."* **Packageable** — `probe_provider` (`provider/resources.rs:1287`) spends against a paced credential unconditionally from `glasshouse resources --probe`, and nothing checks the cached allowance before it fires. `GH-PROBE-BUDGET-1369` (Amber). |


## Headroom concepts refused by name — 2026-09-02

The comparison and the taken half are in `design-decisions.md` (*Headroom, compared*) and Phase 58 (map 2014–2040). These are the concepts deliberately **not** taken, recorded so a future reader meets the decision rather than the temptation: header-sniffed auth mode (we have entitlements); a telemetry beacon on by default (against the project's own telemetry rule); base-URL wrapping of fifteen harnesses instead of adapters; steering text appended to the system prompt at a proxy (our native-mechanism route with the verification floor is proven); deleting or summarising conversation history (Headroom itself abandoned it; the map keeps native compaction and project memory separate).

## Phase 51's memory proxy — 1821 and 1831, censused 2026-09-02 by `GH-RETRIEVAL-ATTRIBUTION`

*"Measure how often retrieved memory is actually useful to the receiving agent"* and *"… prevents repetition of a recorded failed approach."* The explicit halves are closed (`memory rate`). The proxy joins a session-attributed `MemoryRetrieved` row to the same session's `RoutingOutcomeObserved` row. **Both producers now exist and never meet on one session** — the shape of Cluster K's *door that records nothing*, in a different door:

| fact | where |
|---|---|
| the only production memory delivery into a session is the machine door's `deliver_memory`, reached from `Request::SpawnSession` / `Request::SendMessage` | `api/unix.rs::deliver_memory`, `spawn_session`, `dispatch` |
| a door-spawned session is never routed: nothing on that path calls `record_routed_session`, so `record_routing_outcome` — which refuses to write for a session with no routed destination — writes nothing for it at the turn's end | `evaluation/mod.rs::record_routing_outcome`; `main.rs::launch_session` is `record_routed_session`'s one caller |
| a CLI-launched session is routed and never briefed: `launch_session` calls neither `select_memory` nor `deliver_memory`; `glasshouse route`'s injection-scope record measures a would-be briefing and records only its miss | `main.rs::launch_session`, `estimated_project_memory_tokens` |
| the machine door's `QueryMemory` carries no session field, so `memory_search_grouped`'s new session parameter has no caller supplying `Some` | `api/protocol.rs:458–466`, `api/mcp.rs:636–653` |

**Do not package this as "thread the session id further"** — that half is done. The gap is a **design ruling**: either (a) the door's spawn records the routing decision it embodies (the profile it was given is a destination; `record_routed_session` would then have something true to write, and the turn's outcome attributes to it), or (b) the harness-reported turn outcome becomes a row that does not require a routed destination when the reader is the memory proxy — the proxy's definition (`design-decisions.md`, *an explicit rating when given, a labelled proxy otherwise*) is about the *session's* turn, not the *route's*. (a) is the smaller change and the truer one when the door was handed a profile; (b) is right if a door spawn is ever un-profiled. Successor: **`GH-TURN-OUTCOME-FOR-BRIEFED-SESSIONS`** (Amber after the ruling; `api/unix.rs::spawn_session`, `evaluation/mod.rs`, `tests/memory_rating.rs`'s one remaining planted row goes). 1821 and 1831 tick on its landing with the readers already written.

## A defect with a second producer — the health cache's fixed temporary name, found 2026-09-02 by `GH-ROUTED-EXTRACTION-CLIENT`

Not a map line; a Green fix-forward the batch left behind on purpose. `provider/cache`'s `write_json_atomically` writes `<path>.json.writing` and renames. With one producer (the gateway) that was a private race with itself; `persist_support_work_health` is now a second producer on the same provider files from a different process. Two writers collide on the temporary name, and a process killed between write and rename leaves a `.json.writing` file that `GatewayHealthCache::load_all*` reads back as a second reading for the provider. `observed_health_of` is fail-safe about it (identical readings collapse, contradictory ones leave the resource unobserved), so nothing misroutes — the cost is an honest reading discarded. **Successor: `GH-ATOMIC-WRITE-UNIQUE-TEMP`** (Green, Sonnet low): a per-writer unique temporary (pid and a counter) and a loader that ignores `*.writing`; one test that plants a leftover temporary and asserts it is not a reading.

> **Landed the same afternoon** (`GH-ATOMIC-WRITE-UNIQUE-TEMP`, Green, Sonnet low, 2/2 KILLED): `write_json_atomically` now uses `<stem>.<pid>-<n>.writing` and the three loaders skip anything that is not `*.json`. **Residue, named by the worker:** `main.rs` carries two hand-rolled copies of the same pattern (near the checkpoint and session-document writers, ~4831 and ~4902) that were out of that packet's scope because `main.rs` had five co-editors; successor **`GH-ATOMIC-WRITE-MAIN-COPIES`** (Green): route both through `provider::cache::write_json_atomically`.

## Phase 58, after its first four packages — two rows, 2026-09-02

| line | missing link |
|---|---|
| 2019 | **The per-session clause has no producer — Cluster G.** `routing_observations` (migration 11, `database.rs:1299`) carries no session column, and the gateway that writes the translated-exchange rows is minted per instance, not per session (`ingress.rs`'s own header). `GH-SAVINGS-READOUT` shows the cache ratio per `(route, quota_context)` — the credential label — beside the routing evidence; the *per-session* reading needs a session identity on the row, which is a schema decision to be designed with the two other Cluster G rows, not added to close a line. The measuring half is done (`phase-58.md`). |
| — (2014's *recorded reason*) | **`translate::field_rows()` has no production caller** — the pair table's per-field rows, including the new `CacheDisposition`, have never been printed by the shipped binary; the strip's reason reaches a user only through the gateway's opt-in debug record, which is the convention every `Exchange::record` line follows. Not a blocker for 2014 (ruled closed on that convention), but a Green candidate: **`GH-PAIR-TABLE-PRINT`** — `glasshouse gateway pairs` printing `pairs()` with each pair's field rows, one test. |
| 2039 | **No producer, one level before Cluster B — censused 2026-09-02 by `GH-RECON-EFFORT-CLAMP`.** `canonical::Request` has no effort field; `thinking` is refused at decode (`anthropic.rs`, `REFUSED_FIELDS`) for every translated target; no encoder emits `reasoning_effort` / `reasoning.effort` / `thinkingConfig`. A shadow of a mapping nobody has written is not a smaller box, it is an earlier one. **Do not dispatch a clamp or a shadow.** Route: a design-decisions entry, *carrying effort across a translated pairing* — the field on the canonical form (the `cache_requested` pattern), the decode-side carry of `thinking` (a behaviour change for every request that sets it, ruled on its own), the per-target vocabulary researched from the providers' documentation, then `GH-EFFORT-CARRY` (Amber) and, on top of it, the shadow measurement joined to the harness's `TurnEnded` verdict (the ledger's `outcome` is a 2xx proxy). Green residue: the `thinking` refusal's reason text names OpenAI Chat only. |

> **2026-09-02, user ruling (design-decisions, *Memory is the project's, not the launch path's*):** the *launch never briefs* half of the Phase 51 memory-proxy row above is a defect, not a design fact. `GH-LAUNCH-BRIEFING` closes it on the CLI launch through the harness's additive mechanism; with `GH-TURN-OUTCOME-ROW` (live) the proxy then covers manual sessions as well as door-spawned ones.

> **2026-09-02, later:** `GH-TURN-OUTCOME-ROW` landed option (b) — `TurnOutcomeObserved` is written for every session at the hook's `TurnEnded`, the memory proxy joins it, **1821 and 1831 are CLOSED** (`phase-51.md`). `GH-EFFORT-CARRY` landed 2039's producer (`phase-58.md`); the shadow measurement it enables needs the harness-turn rows to carry a session id before they can join the turn's verdict — the same Cluster G decision 2019 waits on, now with two lines behind it.
>
> **2026-09-02, evening:** designed — `design-decisions.md`, *A session identity on the routing evidence rows — Cluster G's first column*: Glasshouse's own session id (never the wire's `user_id`), handed to the gateway by the launch after the record exists; migration 24 adds `session_id`, `effort_level` and `turn_shape` in migration 23's shape so the shadow needs no second migration. `GH-OBSERVATION-SESSION-COLUMN` (Red) dispatched for 2019; `GH-EFFORT-CLAMP-SHADOW` (Amber) follows for 2039.
