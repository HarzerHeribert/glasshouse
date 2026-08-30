# Phase 37 — basic session-aware router

### Eight lines closed, two returned open with measurements, one refused (batch 51)

State: **COMPLETE** for 1592, 1593, 1595, 1596, 1597, 1600, 1601, 1602.
**Open:** 1598, 1599. **Refused:** 1594.

Built in two packages, deliberately staged because `main.rs` was contended:
`GH-ROUTER` built and mutation-killed the scoring on eleven axes and could not
reach a production caller — the packet forbade `main.rs` while requiring a CLI
entry point, and `main.rs`'s dispatch is an exhaustive `match` with no wildcard,
so a new `Command` variant is a compile error until it gains an arm. That was an
orchestrator packet defect, not a design one. `GH-ROUTE-WIRING` then supplied the
callers, co-editing `main.rs` under §77.

Production: `routing/session.rs` (`SessionRouter`, `Destination`,
`RouterInputs`, `RoutingMoment`, `Routed`), the contributions in `routing/mod.rs`,
`glasshouse route` in `cli.rs`, and the `choose` calls in `main.rs`'s
`launch_session` and resume paths.

**Every `Consider X` line has a discriminating test** — two candidate sets
differing only in X, resolving differently. A test asserting X appears in the
explanation proves the renderer, not the router.

**The mutation that matters is on the call, not the callee.** The router's own
eleven mutations prove the scoring and none can prove the wiring, which is this
project's most common defect. `launch-path-ignores-the-user-override` re-run by
the orchestrator in the integrated tree: **KILLED**, by an assertion written to
say so — *"this is the assertion that fails when `launch_session` stops calling
`SessionRouter::choose`"*.

**1594 refused, premise-invalid**, with a tripwire test recording why; it would
not close even with the caller.

**1598 and 1599 returned open with measurements rather than excuses**: their
inputs have production callers, and neither input can take a non-empty value on
any path a test can reach.

**The §77 measurement, finally with two real edits.** `main.rs` carried three
claimants. One (`GH-WORKER-ACCESS`) declared done having never edited it — a
cost the protocol did not previously report, now surfaced by `coedit.sh done`.
The other two edited genuinely disjoint regions (one hunk at `:2930`; the rest
at `:146`–`:433`), so **both intents were preserved without a merge**.
`integrate.sh` correctly refused to choose a winner and handed reconciliation
back to the orchestrator, which is rule 5 working as written. The honest verdict:
co-editing cost nothing here and saved a queued round, but it has still not been
tested against edits that actually collide.

---

# Line 1598 closed, line 1599 refused — 2026-08-30, and the ledger was half wrong

Package `GH-QUOTA-HEALTH-ROUTING`. **Split verdict, and the correction to this
file's own earlier entry is the more useful half.**

## The entry above was wrong about 1598

It said of *both* lines: *"neither input can take a non-empty value on any path
a test can reach."* **True for 1599. False for 1598.**

`routing_destinations` (`main.rs:568`) reads its telemetry at `main.rs:577`
from `GatewayQuotaCache::new(runtime.paths())` — an **on-disk cache under
`runtime.paths().data_dir()`**, written by the gateway from responses it
forwards anyway. `--data-dir` is a first-class CLI flag, and
`tests/provider_discovery.rs` already plants into that cache for `glasshouse
resources`. Nobody had pointed it at the router.

**So 1598 closed with ZERO production change.** No seam was designed, no
`#[cfg(test)]` back door added: the binary reads the reading through the code it
always runs. The packet asked for the smallest seam that would make the acting
path observable; the honest answer was that one already existed.

## 1598 — contract and evidence

Given two resumable sessions equal on every scored axis but the provider whose
gateway quota cache they read, when `glasshouse launch` chooses a destination,
Glasshouse continues the session whose provider has more remaining quota — while
preserving the inert `0.0` contribution and its stated reason when nothing has
been read.

Production: `main.rs::routing_destinations`, `::destination_capacity`,
`::launch_session`; `routing/session.rs::quota_pressure`;
`provider/resources.rs::observed_capacity`;
`provider/telemetry.rs::GatewayQuotaCache::load`.

Regression, both entering through the **shipped binary**:
`route_command::known_quota_pressure_decides_which_session_the_launch_path_continues`
and `route_command::with_no_quota_reading_the_term_is_present_and_weighs_nothing`.

| mutation | result | killed by |
|---|---|---|
| `quota_pressure` scores a constant regardless of capacity | **KILLED** | the quota test — *"the session on the provider with 95% remaining must win over the one on 5%"* |
| `destination_capacity` returns `None`, severing the producer | **KILLED** | same test, same assertion |

**The second mutation is the whole point of the package** — it distinguishes
"the scorer responds to capacity" from "the binary supplies capacity", and the
first is what `session_router.rs:512` already proved a batch ago while the box
stayed open. **Re-run independently by the orchestrator after integration** and
killed again, at `route_command.rs:1688`.

### Limits, stated rather than discovered later

- Proven for two **existing** sessions. `DestinationScope::Launchable` offers
  one fresh destination, so quota separating two *fresh* destinations is not
  exercised and cannot be on that path.
- The reading is planted in `GatewayQuotaCache`, not produced by a live gateway
  in the same test; the write side is proven separately in
  `gateway::conformance`.
- macOS only. The argv-log parsing the assertion rests on is **reasoned**, not
  measured, on Windows.
- Does not prove the weight is right — only that the binary supplies the input
  and the choice follows it.

## 1599 — REFUSED, structurally, and the refusal is executable

`RouterInputs.health` is a `FreePool`. `launch_session` constructs an **empty**
one immediately before `SessionRouter::choose`, and `main.rs` calls **no
`FreePool` mutator anywhere** — not `observe`, `record_pool`,
`declare_token_priced` or `rotate_from`. All four production `choose` call sites
do the same. The only live pool is `gateway::session`'s `SessionRouting.free`,
whose sole accessor `free_pool()` has exactly one caller inside its own module,
so **no pool ever reaches the router**.

`provider_health` therefore returns an identical contribution for every
candidate, and **a signal constant across the ranked set cannot change the
ranking whatever its weight** — `phase-9j.md`'s rule, and the shape of line 566.

### The measurement, which is better than the argument

The same mutation, run twice:

| mutation | target | result |
|---|---|---|
| `provider_health` reads a fresh empty pool instead of the supplied one | `--test route_command` (the binary's tests) | **SURVIVED** — 31 passed |
| the identical mutation | `--test session_router` (the library's tests) | **KILLED** |

**That pair is the refusal.** Severing `provider_health`'s input is invisible
through the binary because on that path it was already empty, and visible
through the library because those tests hand it a pool. The scorer is watched;
the wiring is not, because there is none.

### The refusal is written where the wiring would be attempted

`main.rs` gained **25 lines of comment and no behaviour**, at the
`FreePool::new()` site, naming the two hazards a future bridge must solve:
`FreeResource` is keyed by a `CredentialId` while a `GatewayHealthReading`
carries a rendered `credential_label`, and `ResourceHealth::cooling_down_until`
is an epoch-less `Instant` while a reading carries unix seconds — the exact
mixing `gateway::session::health_readings_for` documents itself avoiding.

`route_command::a_persisted_provider_health_reading_reaches_the_binary_but_never_the_launch_paths_router`
is the tripwire. **If it ever fails, the bridge was built and 1599 must be
re-opened — not the assertion relaxed.**

## Packet errors the worker reported, all three correct

1. The packet quoted this file's "neither input" sentence for **both** lines.
   Correct for 1599, wrong for 1598.
2. **The packet contradicted itself.** Its ACCEPTANCE TESTS put the tests in
   `tests/session_router.rs`, whose every test hands hand-built `Destination`s
   to `SessionRouter::choose` — the exact thing the same packet's REQUIRED
   BEHAVIOR forbade as proof. The worker moved them to `tests/route_command.rs`,
   declared the scope overflow, and gave the reason. It was right.
3. The packet warned line numbers might have shifted ~100 lines from a peer's
   landing. They had not — the peer's edits were already in this worktree's base.
