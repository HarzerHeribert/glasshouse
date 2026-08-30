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

---

# Line 1599 — CLOSED 2026-08-30, and the refusal above was right until it wasn't

Package `GH-GATEWAY-HEALTH-BRIDGE`. The entry above refused this line and
tripwired it. **That refusal was correct** — no `FreePool` reached the router —
and it expired the moment someone built the bridge, which is a refusal cluster
working exactly as intended.

## The packet said "refuse if the label mapping is lossy". It is lossy. It closed anyway.

`CredentialId::label` (`routing/mod.rs:117`) renders
`Environment { var }` as `"{provider}/{var}"` and
`OsCredential { service, account }` as `"{provider}/{service}:{account}"`.
**Nothing is escaped and `provider` is free text**, so a parser must guess two
things: where the provider ends (`a/b` + var `c` and `a` + var `b/c` both render
`a/b/c`) and which variant it is (`Environment { var: "s:a" }` and
`OsCredential { service: "s", account: "a" }` both render `p/s:a`). The
structured identity cannot ride along either — `SecretRef` has no serde impl,
and `secret/mod.rs:88-91` says why.

**So: never parse a label.** The bridge does not.

## The consumer states its own key, so the map is only ever run forward

`provider_health` (`routing/session.rs:890`) looks nothing up by label. It builds
`FreeResource::new(destination.backend().credential().clone(),
destination.backend().model().label())`. **Both halves are in hand at the bridge
site** — `routing_destinations` returns the candidates before `RouterInputs` is
built. So `observed_provider_health` (`main.rs:999`) walks the *destinations*,
renders each label with **the very function the write side rendered it with**,
and compares string equality forward only.

| | write side | read side |
|---|---|---|
| credential | `resource.credential().label()` (`gateway/session.rs:265`) | `credential.label()`, same fn |
| model | `model_key(..)` = `AssignedModel::label` (`gateway/session.rs:579-581`) | `destination.backend().model().label()` |

One renderer, both ends, **no inverse computed anywhere.**

## Three ambiguities, all DECLINED rather than resolved

Forward matching is exact only if labels are unique across what is being
attributed. Each of these refuses instead of choosing:

1. **Cross-provider** — a reading's provider must equal the credential's own.
   Two providers sharing a `credential_env` are *"two separate allowances"*
   (`CredentialId`'s doc) and the label keeps them apart.
2. **Cross-model** — health is per credential **and** model; sharing one entry
   *"would take every model out of service because one of them was busy"*.
3. **Contradiction** — two readings naming one destination's (label, model) and
   disagreeing means **neither is used**. A file Glasshouse wrote cannot contain
   those. A file it did not write can — **and so would a genuine label
   collision, which is exactly what a collision looks like in the data**: one
   rendered name, two different claims. Picking one would be choosing by file
   order.

That third refusal is the honest answer to the packet's stop condition: the
ambiguity is not resolved, it is detected and declined.

## Hazard 2, the epoch-less clock

`ResourceHealth::cooling_down_until` is an `Option<Instant>` with no epoch; a
persisted reading carries unix seconds. Both clocks are read **as one pair**,
once, for every reading. An already-elapsed deadline becomes *not cooling down*
— proven by `an_already_elapsed_persisted_cooldown_does_not_suppress_a_destination`.

## Mutations

| mutation | result | killed by |
|---|---|---|
| sever the bridge — `launch_session` gets `FreePool::new()` again | **KILLED** | 3 tests, launch assertion in each |
| break identity — attribute by provider alone | **KILLED** | `a_sibling_credentials_refusal_…` |
| Hazard 2 — an elapsed deadline becomes a future `Instant` | **KILLED** | `an_already_elapsed_persisted_cooldown_…` |
| contradictory readings resolve to file order | **KILLED** | `two_readings_that_disagree_…` |
| sever the **task-boundary report's** bridge site | **SURVIVED**, predicted | nothing — a real unwatched second site |

**Re-run independently by the orchestrator** after integration, severing at the
source: KILLED by three tests including
`observed_provider_health_decides_which_session_the_launch_path_continues`.

**A first draft of the mirrored test pair SURVIVED the sever**, and the worker
strengthened it rather than shipping it — a mutation catching a weak test again.

## The tripwire was updated, not deleted

`a_persisted_provider_health_reading_reaches_the_binary_but_never_the_launch_paths_router`
is now `a_persisted_provider_health_reading_reaches_the_launch_paths_router` and
asserts the new behaviour.

## Limits

- The **task-boundary report** site is bridged but unwatched (the SURVIVED
  above). Named rather than hidden.
- Scope overflow, both declared: `api/unix.rs` (+1, compile-forced — a second
  caller of `routing_caveats`, which gained a parameter) and
  `tests/routing_api.rs` (an assertion pinning the old caveat wording, updated
  and kept rather than weakened).
- macOS only; the cross-platform gate has not run since this landed.
