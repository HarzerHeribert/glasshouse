# Capability evidence — phase 33

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 33 — resource health: two lines proven, and a reachability wall behind the rest

Contract: Given configured resources whose providers expose quota and reset
information, when Glasshouse reports resource capacity, it reflects the real
observed quota state and reset timing — while never inventing a percentage for
telemetry it does not have.

State: **COMPLETE** for map lines 1314 and 1315 — two of fifteen. **NOT STARTED**
for the rest, and the reason is one shared architectural finding rather than
thirteen separate gaps.

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

**Two more lines were closed by the worker and deliberately NOT ticked by the
integrator.** 1320 and 1323 were closed *on existing tests* — 1323 partly by a
source-scan proof of absence. Practice §14 records that a source-scanning test is
a line-ending trap, and map line 1748 was un-ticked once for a vacuous absence
claim. Both deserve the mutation check the integrator did not have budget to run
this round. **They are strong candidates, not oversights** — see the handoff.

Platform/external evidence: covered by the macOS and Ubuntu legs of the batch
gate; Windows recorded with the batch in `docs/process/handoff.md`.

Missing evidence:

- A durable, externally-readable surface for `ResourceHealth` — the single thing
  that unblocks 1311/1321/1322/1324.
- A consumer treating provider-declared `Retry-After` as authoritative — 1319.
- The mutation check for 1320 and 1323 before either is ticked.
