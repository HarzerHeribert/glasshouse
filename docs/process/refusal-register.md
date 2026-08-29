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

### Cluster A — a production caller passes an invented constant *(in-repo: YES — being attacked now by `GH-LEAD-PLACEHOLDERS`)*

| line | missing link | site |
|---|---|---|
| 1288 | `cheaper_adequate_resource_exists: false` hardcoded | `routing/disposable.rs:568` |
| 1290 | `user_override: false` hardcoded; no producer anywhere sets one | `routing/disposable.rs:568` |
| 1291 | blocked *by* 1288: with `cheaper_adequate…: false` the function falls through to `Allow`, so the imminent-reset branch is unobservable | `provider/quota.rs:2298` |
| 1294 | `task_nearly_complete: false` hardcoded | `routing/disposable.rs:568` |
| 1319 | `quota` carries the provider's `Retry-After`; `observe_exchange` is called 17 lines later without it; `session.rs:614` hardcodes `None` | `gateway/mod.rs:586-603` |

### Cluster B — a mechanism built, tested, and never installed in production *(in-repo: YES)*

**Batch 50 closed two of the four and disproved a third.** The cluster framing
was right and it paid: 1735 and 925 are done, and 531 turned out to be
mis-filed. What is left is one row.

| line | missing link |
|---|---|
| 922 (half) | `MemoryStore::resolve_conflict` (`memory/store.rs:1507`) has zero non-test callers. **Re-verified batch 50 and the reason is sharper:** the `memory revalidate` CLI that shipped since does *not* route through it — `revalidate_superseded` (`store.rs:1387`) calls `supersede` (`store.rs:1168`) directly. Glasshouse can raise a conflict and still cannot resolve one from the binary |

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
| 372 | **stale blocker**: `phase-9a.md:482` says `grep 'fn score\|Score'` is empty; it is not. Nothing selects among launch profiles, so still open |
| 1313 | latency aggregates have zero production readers; every candidate consumer is in another partition |
| 531 | **moved here from Cluster B in batch 50.** Missing caller *and* consumer: nothing in production distinguishes a request pool from a token-priced allowance, and no `FreePool` outlives one call. Needs a routing consumer that behaves differently for the two — see Cluster B's note |
| 930, 934 | **added batch 53, and they were being recommended as cheap.** Phase 21F's two remaining packageable lines qualify an injection that does not exist: *"inject only memories whose scope overlaps the current task"* and *"avoid injecting old ideas merely because they mention the same subsystem"*. **Phase 27, Context injection, is at 0/11.** The consuming seam is real and located — `api/unix.rs:659-661`, where `spawn_session` delivers a natural-language task to a freshly spawned session — but nothing selects memory before it. Package Phase 27 first; these two are its qualifiers and close cheaply afterwards |

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

Batch 51, Phase 15/16. **Ranking these by open-line count keeps recommending
them; the reason they are open is not effort.**

| line | missing link |
|---|---|
| 745 | *"enter any orchestrated worker while it is running"* is an **unmade Red-tier product decision**, which `phase-16.md` already recorded. `session::attach` is not its producer: it **spawns rather than adopts** (`attach.rs:107`) and its resume caller refuses a running session by name (`session/store.rs:1838`). Adopting a running session is a different capability from starting one |
| 746, 747 | follow 745 — there is nothing to send input to, or interrupt, until a user can be *in* a running worker |
| 748 | blocked on Cluster K's defect below, not on schema. The event kinds exist already: `text_delivered` and `interrupt_delivered` carry an `origin` field (`database.rs:89-101`) |
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
- **1681** — no recommendation producer exists to inspect without executing.
- **1661** — `max_router_latency_ms` is a configured ceiling, not a measurement.
- **1745, 1746** — no cmux-metadata path reaches project-scope validation, and
  there is no MCP surface. A grep for "cmux|mcp" hits doc comments and looks
  like a lead.
