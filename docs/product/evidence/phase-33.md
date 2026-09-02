# Capability evidence — phase 33

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 33 — resource health: two lines proven, and a reachability wall behind the rest

Contract: Given configured resources whose providers expose quota and reset
information, when Glasshouse reports resource capacity, it reflects the real
observed quota state and reset timing — while never inventing a percentage for
telemetry it does not have.

State: **COMPLETE** for map lines 1314, 1315, 1320 and — as of 2026-08-29 —
1311, 1321, 1322 and 1324: seven of fifteen. **NOT STARTED** for the rest.

**The architectural finding below has been acted on.** It said these four lines
needed one consumer rather than four packages; `GH-HEALTH-CACHE` built that
consumer and all four closed together. The finding stands as written for the
remaining lines; see "The health-cache package" further down for what changed.

**This entry came from a *proof* package, not a feature package.** A recon had
reported nine of these lines as satisfied by shipped behaviour. That is a claim,
not a closure, so a worker was sent to write — for each line — a regression test
that **fails when the behaviour is removed**, or to report the line open. It
closed four, and the integrator ticked two.

Production evidence:

- `provider/resources.rs:572` `state.seconds_until_reset(options.now_unix)` →
  `:574` `reset_note = format!(", reset in {seconds}s")` → `:580`, the rendered
  band line — line 1315. The same value is exposed to the machine door as
  `"seconds_until_reset"` at `:433`.
- `provider/telemetry.rs::GatewayQuotaCache` → `GatheredTelemetry::gather_gateway_quota`
  → the registry loop `main.rs::resources_report` walks, and its API twin
  `api/unix.rs::resource_capacity` — line 1314.

Regression evidence (both new, both entering through the **shipped binary**, on
the precedent of `a_planted_gateway_reading_now_reaches_the_shipped_binarys_report`):

- `provider_discovery.rs` — a planted reading exposing a reset makes
  `glasshouse resources --verbose` print `", reset in "` with the real value.
- `provider_discovery.rs` — a planted quota/usage reading reaches the report as a
  real capacity band rather than `capacity unknown`.

**The rendered string was verified before the packet was written, and the recon
had guessed it wrong.** There is **no literal `resets`** anywhere in `src/`; the
text is `", reset in {N}s"`. A test asserting the guess would have matched
nothing and still printed `test result: ok` (practice §68) — which is precisely
how a line gets ticked on a test that proves nothing.

**Why thirteen stay open, and it is one wall, not thirteen.**
`routing::free::ResourceHealth` (`routing/free.rs:255-263`) is real and is
written for **every** exchange, paid included, by
`gateway/session.rs::observe_exchange` (`:371`) — an earlier recon's claim that
it is free-pool-only was wrong. It is also read back, at `:385`, to rotate
credentials. **But nothing outside the gateway module can observe it:**

- `observe_exchange` and its consumer are `pub(super)` — gateway-internal.
- `Gateway::routing().free_pool()` (`gateway/session.rs:234`) is `pub` and has
  **zero callers anywhere in `src/`**.
- The one production caller of the router that reads `FreePool::is_available`
  constructs `FreePool::new()` — an **always-empty pool**, by its own doc
  comment (*"There is no live `FreePool` to consult here"*).
- No CLI or API surface renders health at all. `api/unix.rs:331`'s own doc says
  so: *"no live health signal is exposed here: `routing::free::ResourceHealth`
  lives in whichever process's `Gateway` last computed a route, in memory."*

So **1311, 1321, 1322 and 1324 are blocked on a consumer, not on behaviour**, and
**1319 is blocked on a producer** — nothing reads a provider-declared
`Retry-After` as authoritative for scheduling. Closing the group needs one
package that gives health a durable, externally-readable surface; it is not
reachable from a test file, which is why the proof package correctly declined
rather than building a harness to reach `pub(super)`.

**Two lines were closed by the proof worker, declined by the integrator, and then
settled by a dedicated mutation pass. They split.**

**1320 — CLOSED, and it is a real code-enforced invariant.**
`provider/quota.rs::CapacityState::metered_balance` (`:1402-1414`) constructs
`Pool::unmeasured()` for every field Glasshouse cannot read, and
`for_resource` (`:1477-1490`) routes every remote direct provider through it;
`render_resource` prints a percentage only when both halves of a pool are
measured. **Mutation:** fabricate a `Capacity::Measured` reading for `requests`
where production never reads one. It **fails**
`provider_discovery.rs::nothing_the_registry_can_describe_reports_a_capacity_number_it_could_not_have_read`
at the assertion naming that very field (*"openrouter (remote) claims a measured
requests remaining"*). Restored byte-identically, re-run green.

**1323 — STAYS OPEN, and this is why declining it was right.** Its cited test is
a source scan, and the scan *does* mechanically kill a mutation — but **the kill
only proves the literal-match scan works, not that any rate guard exists.** The
mutation worker searched beyond `routing/` for any background prober, interval,
timer, or rate/cooldown guard — `main.rs`, `shell/mod.rs`, `shell/state.rs`,
`provider/discovery.rs` — and **found none**. Every probe path is user-invoked:
the CLI `--probe <name>` flag (`main.rs:2236-2271`, opt-in by its own doc) and
the settings-screen `t` key (`shell/state.rs:1100 begin_provider_test`). There is
no periodic or automatic trigger anywhere.

So the property holds **only because the feature that would need guarding does
not exist** — *"no, because nothing probes at all"*, not *"no, because a guard
stops it"*. **That is map line 1748's shape exactly**, and practice §14's
line-ending trap in its general form: a source-scanning test proves the scan, not
the behaviour. The line stays open until either a real probing path with a
rate/interval guard exists, or the map's intent is confirmed to mean *"no
automatic probing exists"* rather than *"probing is rate-limited when it
happens"* — **which is a map decision, not an engineering one.**

Platform/external evidence: covered by the macOS and Ubuntu legs of the batch
gate; Windows recorded with the batch in `docs/process/handoff.md`.

Missing evidence:

- A durable, externally-readable surface for `ResourceHealth` — the single thing
  that unblocks 1311/1321/1322/1324.
- A consumer treating provider-declared `Retry-After` as authoritative — 1319.
- The mutation check for 1320 and 1323 before either is ticked.

---

### The health-cache package, 2026-08-29 — the "one consumer" the wall needed (lines 1311, 1321, 1322, 1324)

**The wall this entry described is down for four lines.** The finding above was
that `ResourceHealth` is real and written for every exchange but **nothing
outside the `gateway` module can observe it** — so 1311/1321/1322/1324 needed one
consumer, not four packages. `GH-HEALTH-CACHE` built that consumer (worktree
`.worktrees/health-cache`, report `.agent-runtime/report-health-cache.md`).

**The design, and the one piece of real design in it.** `GatewayHealthCache`
(`provider/telemetry.rs`) is `GatewayQuotaCache`'s exact shape: a versioned JSON
file per provider under `paths.data_dir().join("gateway-health")`, atomic-rename
write, fail-soft read where absent / truncated / wrong-version / wrong-provider
all mean "no reading here". The design work is the **cooldown conversion**:
`ResourceHealth`'s cooldown is an `Instant`, which has no epoch and **cannot
cross a process boundary**, so it is converted to an absolute unix second on
write. Everything else is a deliberate copy of a shipped precedent.

The five links, end to end in current code:

| link | evidence |
|---|---|
| producer | `routing/free.rs` — `ResourceHealth`, untouched (it was on the packet's FORBIDDEN list) |
| caller | `gateway/session.rs` — `health_readings_for`, called from `gateway/mod.rs`'s accept loop, beside the existing quota-cache write |
| propagation | `provider/telemetry.rs` — `GatewayHealthCache`, built at both of `main.rs`'s launch sites |
| consumer | `provider/resources.rs` — `render_health` (text) and the JSON path, reached only through `report` / `capacity_json`, read from `main.rs` and `api/unix.rs` |
| fifth (varies, and changes behaviour) | `consecutive_failures`, `cooling_down_until_unix` and `credential_rejected` differ per real exchange and change what a **later, separate** `glasshouse resources` invocation prints — proven by the write-path test, not asserted |

**Out-of-partition file, flagged rather than hidden.** `gateway/conformance.rs`
was not in EXPECTED FILES. Widening `Gateway::start_with_telemetry` to thread the
cache through the accept loop forced updating its two existing call sites or
nothing compiled, and once there it was the only place the real write-path test
could live. Accepted.

Mutations, 3/3 killed:

- **`remove-persistence-call`** — the health-cache write deleted from the accept
  loop. Killed by
  `gateway::conformance::a_real_forwarded_exchanges_health_is_persisted_for_the_next_process`:
  *"no health reading was persisted for `fixture` within 2s of a completed
  exchange"*.
- **`accept-stale-state`** — `GatewayHealthCache::load`'s three fail-soft branches
  made to fabricate a healthy entry instead of returning empty. Killed, 5 failures.
  **Honest caveat, recorded because the worker raised it:** this proves the
  property at `load` only. `load_all` — the method `gather_gateway_health`
  actually calls in production — is a structurally separate directory scan with
  its own fail-soft branches, and the two binary-level "unknown" tests that go
  through it stayed green under this mutation. `load_all`'s per-entry
  parse-failure branch is exercised (not mutated) by the corrupt-file tests. A
  faithful `accept-stale-state` on `load_all` would have to invent a provider key
  too — the provider name lives inside the bytes that failed to parse — which
  stops being the same defect. **This is a known, bounded gap in the mutation
  evidence, not a claim of full coverage.**
- **`invert-condition`** — `render_health`'s `until > options.now_unix` flipped.
  Killed at both the unit and shipped-binary level.

---

### Phase 33 — Track whether each configured resource is currently available (line 1311)

Contract: Given a resource the gateway has forwarded exchanges through, when the
user asks Glasshouse for resource state in a **later process**, it reports that
resource's observed availability, while reporting `unknown` for a resource it has
never observed rather than assuming it is healthy.

State: COMPLETE

Production evidence: the five links above.

Regression evidence:
- `gateway::conformance::a_real_forwarded_exchanges_health_is_persisted_for_the_next_process`
  — a real `Gateway`, a real bound assignment, a real 200 OK exchange, then a
  poll-read of `GatewayHealthCache::load("fixture")` off disk.
- `provider::resources::tests::a_resource_with_no_health_observation_reports_unknown`
  and `provider_discovery.rs::a_resource_with_no_health_observation_reports_unknown_through_the_shipped_binary`
  — the fail-closed half: every registry entry prints `health unknown`, never a
  number, with no cache present at all.

---

### Phase 33 — Allow a resource to be temporarily marked degraded after repeated failures (line 1321)<br>Phase 33 — Allow a degraded resource to recover after successful probes or requests (line 1322)

Contract: Given a resource that has failed repeatedly, when Glasshouse reports
it, the resource is shown as degraded with its observed failure count; and when
its cooldown has elapsed, it is shown as available again — while a rejected
credential stays distinct from a paced one, because waiting does not fix a
rejection.

State: COMPLETE (both)

Regression evidence:
- `provider::resources::tests::a_cooling_down_resource_is_shown_as_paced_not_broken`
  and `provider_discovery.rs::a_cooling_down_resource_is_shown_as_paced_through_the_shipped_binary`
  — the degraded half (1321).
- `provider::resources::tests::an_elapsed_cooldown_reads_as_available_again` — the
  recovery half (1322): a cooldown already past renders as available with no fresh
  observation, matching `ResourceHealth::is_available`'s own in-memory rule.
- `provider::resources::tests::a_rejected_credential_is_shown_as_rejected_not_paced`
  — keeps the two failure kinds visually distinct.
- `provider::telemetry::gateway_health_cache_tests::a_stored_reading_round_trips_including_a_cooldown_deadline`
  — the `Instant` → absolute-unix-second conversion survives the process boundary,
  which is the whole reason either line is observable at all.

---

### Phase 33 — Keep resource health separate from immediate availability so a healthy paced route can remain temporarily unschedulable without being scored as broken (line 1324)

Contract: Given a resource that is merely pacing, when Glasshouse renders it,
it says paced and names the cooldown deadline, while never rendering it in the
vocabulary it uses for a broken or credential-rejected resource.

State: COMPLETE

This is the line the `invert-condition` mutation targets directly: with
`until > options.now_unix` flipped, a resource still cooling down rendered as
`available, 3 consecutive failure(s)` — i.e. exactly the conflation of "paced"
with "broken" this line forbids. Killed at both levels:

- `provider::resources::tests::a_cooling_down_resource_is_shown_as_paced_not_broken`
- `provider_discovery.rs::a_cooling_down_resource_is_shown_as_paced_through_the_shipped_binary`

Fail-soft evidence, so a bad cache cannot take the command down with it:
- `provider::telemetry::gateway_health_cache_tests::{a_truncated_health_cache_file_reads_as_an_empty_list, a_reading_stored_by_a_future_format_version_is_ignored, a_provider_with_no_stored_health_reads_as_an_empty_list}`
- `provider::resources::tests::a_corrupt_health_cache_file_leaves_the_report_working_with_no_health`
- `provider_discovery.rs::a_corrupt_gateway_health_cache_file_leaves_the_shipped_binary_working`
  — overwrites the actual file `store` just wrote and asserts the command still
  exits 0 and prints `unknown` for that provider.

---

### A ledger correction this package forced

`phase-33.md`'s "Correction, same session" section still described wiring quota
into `main.rs` as future work (*"main.rs is this package's FORBIDDEN FILES"*).
That was true of an earlier packet and not of current code — quota was already
fully wired by the time this package ran. The paragraph is frozen at an earlier
moment than the section beside it. Nothing was acted on incorrectly, because this
package's own feasibility table carried the corrected citations. Recorded here
because it is the **same failure mode as line 1663's** in the same batch: an
entry that records a blocker does not expire when the blocker does.

---

### 1323 settled by the user: the line stays open, and why that is the answer

Asked directly on 2026-08-29 whether the line means *"no automatic probing
exists"* or *"probing is rate-limited when it happens"*, the user chose neither
reading as a closure and gave the reason:

> *"If there ever happens any probing and it would hit a free endpoint like
> openrouter it would use a request budget — so best is to not probe at all if
> not necessary and if necessary only sporadically. And if we want model data
> maybe the providers have data endpoints and measured availability for us
> instead of doing it ourselves."*

**1323 stays open.** Recorded in full, with what a future prober owes, in
`docs/product/design-decisions.md` — *"Probing costs a request budget, so the
cheapest probe is the one nobody runs"*.

The part that matters for this ledger: the line is **not** line 1748's shape
after all, and the earlier note here suggesting it might be is superseded. 1748
was reworded because its property is structural — physical separation holds
against any deleter. 1323's property holds only because a feature is **absent**,
so it would evaporate silently the first time anyone writes a prober. Keeping the
box open is what keeps the requirement pointed at the code that will need it.

Note also that **line 537 is already ☑ and already implements the principle from
the other side**: `FreePool::observe` learns health *entirely from work that was
going to happen anyway*, and `WorkloadOutcome::Served` clears a cooldown, so
recovery is learned from real traffic too. Glasshouse already gets its health
signal without spending a request. That is the strongest existing argument that
no prober is needed, and it is shipped code rather than an intention.

### The recommended "one consumer unblocks four" package is PREMISE-INVALID as written

Two checkpoints have recommended, as the cheapest next step, *"give
`ResourceHealth` an externally-readable surface — one consumer unblocks 1311,
1321, 1322 and 1324."* **Checked against current source before writing a packet,
and it does not connect.** Recorded here so the next orchestrator does not
re-derive it.

**What is genuinely already there, and it is more than the checkpoints credited.**
`ResourceHealth` (`routing/free.rs:236-263`) is not a stub — it already
implements the whole state machine those four lines describe:

- `consecutive_failures` + `cooling_down_until`, with a bounded doubling from
  `BASE_COOLDOWN` and the provider's own `retry_after` preferred when given —
  that is **1321's** degradation.
- `WorkloadOutcome::Served` resets `consecutive_failures` to 0 and clears the
  cooldown — that is **1322's** recovery, learned from work.
- `is_available(now)` — that is **1311's** availability.
- `FreePool::is_available` is `health.is_available(now) && !allowance.is_exhausted(now)`,
  so health, schedulability and quota are already three separate concepts —
  that is **1324's** separation.
- `FreePool::observed()` (`:449`) returns every observed resource in a stable
  order and its own doc says it is *"for a settings or diagnostic view"* — so
  even §71's sixth question (*where does the set come from?*) is answered.

**The link that fails is propagation, and it fails twice.**

1. **The rendering path has no gateway.** `resources_report` is called at
   `main.rs:140`, inside the **CLI command dispatch**, whose only argument is
   `&runtime`. The gateway is started at `main.rs:534`, on the **session-launch**
   path — a different branch of a different function. `glasshouse resources` never
   has a `Gateway` in scope, so a health section added there would render an
   empty pool on every run, forever. That is the identical always-empty-pool
   defect this entry already records for the router's caller, reproduced one
   surface further out.
2. **The pool is never persisted.** `free_pool()` (`gateway/session.rs:234`)
   returns `self.lock().free.clone()` — a clone of in-memory state owned by one
   `Gateway` instance. Nothing writes it to SQLite. `api/unix.rs:331` already says
   so in its own doc: health *"lives in whichever process's `Gateway` last
   computed a route, in memory."* So there is no durable artifact for any second
   surface to read.

The only process holding a live pool is the one that launched a session, where
the handle is bound to `_gateway_guard` (`main.rs:2138`) and deliberately never
read again.

**So the real package is one of two, and neither is small:** persist health
(a migration, Red tier), or thread the `Gateway` into the shell and add a TUI
overlay that shows only the current process's own pool. **It is not "one small
consumer", and a packet claiming so would have been the fifth dispatch-gate
casualty in five rounds** — each one the previous checkpoint's own recommended
next step, which is the pattern `docs/process/assurance-economics.md` exists to
catch.

Missing evidence, restated precisely:

- **1311/1321/1322/1324** — the behaviour is built and exercised on every
  exchange; what is missing is a *durable or in-process* surface that can observe
  it. Decide persistence versus a shell overlay **before** writing a packet.
- **1319** — still no producer: nothing treats a provider-declared `Retry-After`
  as authoritative for scheduling. (`ResourceHealth::observe` does use
  `retry_after` for its own cooldown length, which is adjacent but is not the
  scheduling authority the line names.)
- **1323** — settled as open by the user's decision above.

### Correction, same session: the surface is NOT a migration, and the precedent is already shipped

The paragraph above concluded that persisting health means *"a migration, Red
tier."* **That is wrong, and the correction changes the tier and the cost of the
package.**

`provider::telemetry::GatewayQuotaCache` already solves this exact problem for
quota, and its own module comment states the problem in the same words this entry
reached for independently (`telemetry.rs:1071-1080`):

> *"…both only ever run inside a `glasshouse run`/`glasshouse launch` process
> that is blocked on the harness it started. `glasshouse resources` — the one
> caller that turns a reading into a rendered line — is a separate invocation of
> the binary, and nothing in memory connects the two. **This is that connection:**
> the gateway process writes what it captured, and a later `glasshouse resources`
> process reads it back."*

It is **not** SQLite and **not** a migration. It is a versioned JSON file per
provider under `paths.data_dir().join("gateway-quota")` (`:1165`, `:1175`),
written atomically by rename (`:1129`), and fail-soft on every read path —
absent, truncated, wrong version and wrong provider all mean *"no reading here"*
rather than a failed command (`:1191`).

**So the five links for a health surface are all citable today:**

| link | evidence |
|---|---|
| producer | `routing/free.rs:236` `ResourceHealth`, written per exchange at `gateway/session.rs:371` |
| caller has it | `Gateway` already holds `quota_cache: Option<GatewayQuotaCache>` — `gateway/mod.rs:301, 331, 483, 634, 655`. A sibling health cache follows the identical construction path. |
| propagation | the on-disk cache pattern, built at `main.rs:537` and `main.rs:1079`, exactly where the quota cache already is |
| consumer | `resources_report` via `gather_gateway_quota`'s sibling (`provider/resources.rs:278`, called from `main.rs:2246`) **and** the API door at `api/unix.rs:530`, which already reads quota this way |
| fifth | the health values genuinely vary — `consecutive_failures`, cooldown and credential rejection differ per `(credential, model)` and already drive credential rotation at `gateway/session.rs:385` |

`provider/cache.rs:318` even records that *"`GatewayQuotaCache` keys a second
per-provider directory the same way"* — the keying convention is established and
a third user of it is expected.

**And the acceptance test has a precedent to copy**, which is what makes this
closable rather than merely buildable:
`provider_discovery.rs:890::a_planted_gateway_reading_now_reaches_the_shipped_binarys_report`
plants a reading in the cache and asserts the **shipped binary's** report renders
it. The health version is the same test with a different payload, and it is
non-vacuous by construction because deleting the write makes it fail.

**Revised assessment: an Amber (Sonnet) package, not Red.** Partition
`provider/telemetry.rs` + `provider/resources.rs` + `gateway/**` + `main.rs`'s two
construction sites, with `api/unix.rs` following the quota precedent. It plausibly
closes **1311, 1321, 1322 and 1324** together, and is the best-specified open work
in this phase.

**Why this was missed for four checkpoints, which is the transferable part:** every
prior assessment asked *"can anything outside the gateway observe `ResourceHealth`?"*
and correctly answered no. None asked *"has this codebase already carried a
gateway-only observation across the same process boundary?"* — and it had, once,
in the module the consumer already calls. **When a propagation link fails, look for
a sibling signal that already crosses the same boundary before concluding the
boundary is the problem.**

---

## Phase 33 — batch 49 team-lead pass: 1312 CLOSED, five refused with the failing link named

Run by an Opus team lead with two subcontractors. **One line closed, five
returned unbuilt** — and four of the five fail Phase −1 at a link that is
outside the partition or outside the packet's authority, not at "nobody built
it yet". That distinction is the package's value.

### Line 1312 — track recent request failures for gateway-backed resources. CLOSED.

Contract: Given routing observations recorded for a resource, when Glasshouse
asks how that resource has recently behaved, failures inside the window count
against it, failures outside the window do not, and another resource's failures
never do.

State: COMPLETE

Production evidence: `routing::evidence::EvidenceLedger::summarize`'s windowed,
identity-filtered query, consumed through `ObservedEvidenceSource`.

Regression evidence (`tests/routing_evidence.rs`):
- `an_old_failure_does_not_contribute_to_a_recent_failure_rate`
- `another_resources_failures_are_not_this_resources_failures`
- `observed_evidence_source_reflects_recorded_failures`

**A §35 hole one level in, found by the subcontractor rather than the lead.**
The pre-existing `observed_evidence_source_is_reachable_from_outside_the_crate`
records **only successes**, so a `task_success_rate` that always answered `1.0`
would pass it — a reachability test that cannot observe the value it is named
for. The third test above exists because of that.

Mutations — 3/3 killed, all run by the lead, one re-run by the orchestrator:

| mutation | result |
|---|---|
| `AND observed_at >= ?6 AND observed_at <= ?7` → `AND ?6 <= ?7` (arity preserved, so a real semantic mutation rather than a binding error) | **killed** — the window test |
| `WHERE provider = ?1 AND model = ?2` → `AND ?2 = ?2`, anchored on the `context_state = ?5` line so it lands in `summarize` and not in `recent`, which shares the literal | **killed** — the identity test |
| `.map(\|reading\| 1.0 - reading.value())` → `.map(\|_reading\| 1.0)` | **killed** — `observed_evidence_source_reflects_recorded_failures`, re-run by the orchestrator |

That second mutation is worth keeping: `mutate.sh` refuses a non-unique find,
and that refusal is what surfaced the collision between `summarize` and
`recent` sharing a literal. The tool's strictness did the work.

### The five refused, with the failing link

| line | verdict | the link that fails |
|---|---|---|
| 1313 recent latency | **blocked-by-partition** | consumer: latency aggregates have **zero** production readers; every candidate lives in `provider/**` or `shell/**` |
| 1316 rate-limit vs transport | **blocked, no authority** | needs a new persisted outcome value — a migration the packet forbade — or a health surface in `provider/**` |
| 1317 rate-limit scope | **premise-invalid** | producer: nothing states whether a 429 is provider-wide, model-, account- or pool-specific, and deriving it would be the fabrication the packet forbids |
| 1319 Retry-After authoritative | **blocked-by-partition** | producer→caller: `gateway/session.rs:614` hardcodes `retry_after: None` |
| 1325 observation provenance | **premise-invalid** | of the four provenance values the line names, production can only ever emit one |

### 1319's missing wire is one argument, and it is now unblocked

The lead's most useful finding is not the closed line. `gateway/mod.rs:586`
binds `let (exchange, quota) = ingress::serve(..)`, and `quota` carries the
provider's own `Retry-After`. `observe_exchange` is called **seventeen lines
later at `:603`** and is not given it; `gateway/session.rs:614` then hardcodes
`retry_after: None`. Verified by the orchestrator.

The lead fixed the *rule* 1319 names inside its own partition — a stated
`Retry-After` is preferred over any cooldown Glasshouse would invent, proven by
three further mutations — and correctly **did not tick the box**, because the
provider's value never reaches the rule. `gateway/**` was another worker's that
round; it is free now, and this is a small, precisely located follow-up.

---

## Phase 33 line 1319 — CLOSED, batch 49

The gap `phase-33.md` located above is closed: `gateway/session.rs` now reads
the provider's stated wait out of the rate-limit headers already bound at
`gateway/mod.rs:586` and passes it into `WorkloadOutcome::RateLimited`, instead
of hardcoding `None`.

Regression evidence: `tests/gateway_retry_after.rs`, through a real socket.

Mutations, all killed:

| site | change | result |
|---|---|---|
| `gateway/mod.rs` the call | `session::stated_retry_after(&quota)` → `None` | killed |
| `gateway/session.rs` | `retry_after: stated_retry_after` → `retry_after: None` | killed |
| `gateway/session.rs` | `.retry_after_seconds()` → `.or(Some(60))`, inventing a wait | killed — `one_rate_limit_with_no_stated_wait_blocks_nothing` |

That third mutation is the important one: it proves Glasshouse does not
substitute a default when the provider stated nothing. `None` stays a fact.

---

# Line 1313 — closed 2026-08-30 by proof, with no production change

Package `GH-LATENCY-PROOF`; report in `.agent-runtime/report-latency-proof.md`.
**Tests-only diff.**

## Why a proof packet was the right shape

1313 was **Cluster D** in the refusal register: *"latency aggregates have zero
production readers; every candidate consumer is in another partition."* That
was true until `1b66889`, when Phase 41's line 1661 made the project overview
read `median_duration_ms` at `shell/mod.rs:1571`.

The chain was then complete and nobody had checked. Building anything would
have been manufacturing work; the honest question was *"is this already
true, and can it be proven?"*

## The chain, hop by hop, each above its file's `#[cfg(test)]` boundary

| hop | where | test boundary |
|---|---|---|
| producer | `SessionRouting::record_routing_observation`, `gateway/session.rs:327`, called in production at `gateway/mod.rs:640` | `:739` / `:830` |
| derivation | `RoutingObservation::duration_ms`, `routing/evidence.rs:374` | `:1283` |
| aggregate | `EvidenceLedger::summarize`, `evidence.rs:784` | `:1283` |
| consumer | project overview, `shell/mod.rs:1571` | — |

The worker checked every boundary rather than asserting the hops were
production — which is the check that would have caught three of this session's
wrong claims.

## §14 was the bar, and it was met with production mutations

A tests-only closure resting on tests that already pass proves nothing. All
three mutations targeted **production** code and were killed by named tests:

| mutation | site | killed by |
|---|---|---|
| `duration_ms` returns `None` | `evidence.rs:380` | `a_recorded_observation_yields_duration_ms_and_a_real_median` |
| `summarize`'s window bound → `i64::MIN` | `evidence.rs:791` | `an_old_latency_outlier_does_not_skew_the_recent_median` — the median took a 100,000,000 ms outlier |
| minimum-sample gate fabricates `Some(0)` | `evidence.rs:979` | `below_the_minimum_sample_the_latency_summary_is_unknown_not_zero` |

The second is the one that matters for the word *recent*: without the window
bound, "recent latency" silently includes all history, and the test names the
failure.

## The limit, recorded rather than smoothed over

`dispatched_at` is **an approximation** — `evidence.rs:30` says so: the
accept-loop handoff, not the true wire-dispatch instant inside
`ingress::forward`. The line's own *"where measurable"* permits this, and a
future reader must not mistake the number for a true round-trip.

---

### Track recent observed latency for gateway-backed resources where measurable. (line 1313)

Contract: Given a gateway exchange that actually reached a provider, when the accept loop finishes it, Glasshouse records dispatch and completion timestamps as a routing observation and can summarize them into a real recent median/tail/EWMA latency for that resource's (provider, model, route, harness, context_state) identity within a bounded window, while staying honestly 'unknown' rather than a fabricated zero below the minimum sample, never blending in an observation outside the window or from another project, and never claiming a true first-byte/first-token latency it cannot see.

State: **COMPLETE**

Production evidence:
- `crates/glasshouse/src/gateway/session.rs` — `SessionRouting::record_routing_observation`
- `crates/glasshouse/src/gateway/mod.rs:640 (production caller, inside the connection-handling loop)`
- `crates/glasshouse/src/routing/evidence.rs` — `RoutingObservation::duration_ms`
- `crates/glasshouse/src/routing/evidence.rs` — `EvidenceLedger::summarize`
- `crates/glasshouse/src/routing/evidence.rs` — `EvidenceLedger::summarize_latest_for_model`
- `crates/glasshouse/src/shell/mod.rs` — `build_project_overview_routing (production caller, project overview)`

Regression evidence:
- `routing_evidence::a_recorded_observation_yields_duration_ms_and_a_real_median`
- `routing_evidence::an_old_latency_outlier_does_not_skew_the_recent_median`
- `routing_evidence::below_the_minimum_sample_the_latency_summary_is_unknown_not_zero`
- `routing_evidence::recent_latency_never_crosses_a_project_boundary`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| evidence.rs:380 (completed - dispatched).checked_mul(1000) -> None | `duration-ms-always-none` | **killed** | `routing_evidence::a_recorded_observation_yields_duration_ms_and_a_real_median` |
| evidence.rs:791 let earliest = now_unix.saturating_sub(window_seconds); -> let earliest = i64::MIN; | `summarize-drop-window-bound` | **killed** | `routing_evidence::an_old_latency_outlier_does_not_skew_the_recent_median` |
| evidence.rs:979-981 return None; -> return Some(AggregateReading::new(0,0,0,durations.len(),ReadingSource::LocalObservation(what.to_owned()))); | `duration-aggregate-zero-below-minimum` | **killed** | `routing_evidence::below_the_minimum_sample_the_latency_summary_is_unknown_not_zero` |

> duration-ms-always-none observed: assertion left == right failed: each observation carried a 2-second dispatched/completed pair

> summarize-drop-window-bound observed: median assertion failed; the 100,000,000ms outlier leaked into the window

> duration-aggregate-zero-below-minimum observed: assert!(summary.median_duration_ms.is_none()) failed — mutant fabricated Some(0)

Recorded scope limits — stated by the worker, not discovered later:
- dispatched_at_unix is an accept-loop-handoff approximation, not the true wire-dispatch instant (stated in evidence.rs's own header, unchanged by this package).
- first_byte_at/first_token_at/first_tool_call_at are never recorded by this producer; map lines 1331/1332 stay open.
- Only one identity (the pinned routing model) currently has a production screen reading its latency; the tracking mechanism itself is generic over any observed (provider, model, route, harness) identity.
- No shipped-binary PTY run performed for this package; matches phase-41's own stated precedent for this overlay.

---

### Worker-reported packet errors and gates (transcribed at closure)

Gates the worker ran (re-run the decisive ones yourself):
- cargo build: clean
- cargo test --test routing_evidence: 13 passed, 0 failed
- cargo test --test observability_views: 6 passed, 0 failed
- cargo test --lib routing::evidence: 22 passed, 0 failed
- cargo clippy --all-targets --all-features -D warnings: clean
- cargo fmt --all -- --check: clean
- scripts/blast-radius.sh: every traced target passed (lib 1548, events_log 6, memory_snapshot 8, routing_evidence 13, cargo doc clean)

---

## From `GH-FAILURE-TAXONOMY` (2026-08-31)

**1316 and 1318 are CLOSED** and **1325/1317 stay as ruled above**. The relay now records a nine-way `failure_class` per exchange from status, headers and framing (never body content — the ruling *"framing is not content"* in `design-decisions.md`), so rate-limit responses are counted apart from transport and model failures (1316), and a relayed 429's own headers are proven to move the capacity estimator's band (1318). Full entry, mutations and limits: `phase-33c.md`, *GH-FAILURE-TAXONOMY*.

### Line 1317 — the scope of a rate-limit failure: provider-wide or model-specific

Package `GH-RATE-LIMIT-SCOPE`, 2026-08-31, Sonnet at high (Amber). One derived classification over the correlation reader GH-ROUTE-CORRELATION landed the same afternoon: `throttle_scopes` reads the same `throttle` rows, uses the reader's own `CORRELATION_OVERLAP_TOLERANCE_SECONDS` and `MIN_CORRELATION_SAMPLE` — the packet refused a second tolerance or minimum and the worker added none — and answers provider-wide when throttles on two or more models of one provider overlap, model-specific when a sibling model succeeded inside the same window, `Unknown` below the sample. Computed on read, never persisted; surfaced in `glasshouse route`'s explanation; not yet acted on by the failover ranking (the line says *track*). Three mutations, three killed. Gates quoted: fmt, clippy, `routing::evidence` 44 passed, `rate_limit_scope` 2 passed, blast radius exit 0.

### Track whether a rate-limit failure appears to be provider-wide, model-specific, account-specific, or request-pool-specific when evidence permits. (line 1317)

Contract: Given routing observations recording FailureClass::Throttle on one or more models of a provider, when Glasshouse is asked whether a throttle on a route is provider-wide, model-specific, or unknown, it reports ProviderWide when a throttle on that route overlapped (within CORRELATION_OVERLAP_TOLERANCE_SECONDS) a throttle on another model of the same provider, ModelSpecific when every informative throttle on that route overlapped a sibling model recording a non-throttle outcome, and Unknown with the sample size and the required minimum below MIN_CORRELATION_SAMPLE informative events — while preserving that account-specific and request-pool-specific scopes are never fabricated, matching the refusal register's ruling that neither has a producer in this build.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts and the diff of the decision. **Two of the line's four scopes are not produced, by ruling, not omission:** *account-specific* — this build routes one account per provider and no observation row carries an account identity, so the scope has no discriminating input; *request-pool-specific* — `is_request_pool` has no production caller and production never distinguishes pooled from token-priced allowance (refusal register, 531). `ThrottleScope` therefore reads `ProviderWide | ModelSpecific | Unknown`, and `Unknown` says so with its count below `MIN_CORRELATION_SAMPLE`. The line says *track*, and the two scopes that can be tracked are; the two that cannot are named here rather than fabricated.

Production evidence:
- `src/routing/evidence.rs` — `ThrottleScope`
- `src/routing/evidence.rs` — `classify_throttle_scope`
- `src/routing/evidence.rs` — `classify_throttle_scopes`
- `src/routing/evidence.rs` — `ThrottleScopes`
- `src/routing/evidence.rs` — `EvidenceLedger::throttle_scopes`
- `src/main.rs` — `throttle_scope_section`
- `src/main.rs` — `route_report (wires the new section into `glasshouse route`)`

Regression evidence:
- `routing::evidence::throttle_scope_tests::overlapping_throttles_on_sibling_models_read_as_provider_wide`
- `routing::evidence::throttle_scope_tests::a_throttle_overlapped_by_a_sibling_models_success_reads_as_model_specific`
- `routing::evidence::throttle_scope_tests::one_overlapping_throttle_among_many_lone_ones_still_reads_as_provider_wide`
- `routing::evidence::throttle_scope_tests::below_the_minimum_sample_the_scope_is_unknown_and_says_the_count`
- `routing::evidence::throttle_scope_tests::a_throttle_with_no_sibling_observed_is_uninformative`
- `routing::evidence::throttle_scope_tests::an_upstream_5xx_is_not_a_throttle_and_contributes_nothing`
- `routing::evidence::throttle_scope_tests::a_different_providers_model_is_not_a_sibling`
- `routing::evidence::throttle_scope_tests::classify_throttle_scopes_covers_every_throttled_route_and_no_others`
- `rate_limit_scope::throttle_scopes_are_read_from_a_real_ledger_with_their_sample_size_and_window`
- `rate_limit_scope::the_route_command_prints_every_routes_throttle_scope`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| if sample_size < MIN_CORRELATION_SAMPLE { -> if sample_size < 1 { | `drop-min-sample` | **killed** | `routing::evidence::throttle_scope_tests::below_the_minimum_sample_the_scope_is_unknown_and_says_the_count` |
| if overlaps > 0 { -> if false { | `collapse-provider-wide` | **killed** | `routing::evidence::throttle_scope_tests::overlapping_throttles_on_sibling_models_read_as_provider_wide` |
| (true, false) => lone += 1, -> (true, false) => {} | `ignore-sibling-success` | **killed** | `routing::evidence::throttle_scope_tests::a_throttle_overlapped_by_a_sibling_models_success_reads_as_model_specific` |

> drop-min-sample observed: assertion `left == right` failed at evidence.rs:4409 — four informative events read back as a verdict instead of Unknown

> collapse-provider-wide observed: assertion `left == right` failed: every throttle on x overlapped a throttle on y of the same provider

> ignore-sibling-success observed: assertion `left == right` failed: every throttle on x was observed against a sibling that kept serving

Recorded scope limits — stated by the worker, not discovered later:
- not wired into the live failover's ranking or scoring — recorded and readable, not acted on, per the objective's own instruction
- computed on read, never persisted
- the shipped-binary CLI test drives macOS only, no Windows leg
- only looks within one provider's own recorded models; cross-provider correlation remains correlate_routes/RouteCorrelations' job

