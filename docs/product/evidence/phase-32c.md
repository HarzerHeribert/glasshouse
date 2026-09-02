# Phase 32C — Subscription capacity estimation

# Lines 1244, 1245, 1246, 1250, 1251, 1254 — COMPLETE 2026-09-01

Package `GH-SUBSCRIPTION-ESTIMATOR` (Sonnet, high, Amber; batch 74). The
phase became reachable this week: every input landed with 56A's telemetry.
An estimator **derived entirely on read** — no table, no migration, no
persisted state; today's history IS the ledger's rows in window.

`estimate_subscription_headroom` (`routing/evidence.rs`, beside the
credential readers whose widen-when-unsure narrowing it reuses verbatim):
accepted-request counts, throttle recency, the quota cache's reset reading,
and per-account session counts from `sessions.entitlement` (migration 22,
fail-soft). Returns `Option<SubscriptionHeadroomEstimate>` — a
`HeadroomBand` (Exhausted/Low/Moderate/Ample) + `Confidence` +
`HeadroomBasis` + `account_narrowed`. **The type structurally cannot carry
fictitious precision** (1250/1251): no numeric field on the band; a token
row changes only the basis label. An opaque-limit account estimates from
activity alone (1244). One contextless row widens the whole estimate to
provider scope (1246). Two accounts' rows never mix (1254 — flagship test
plus KILLED mutation `never-mix`).

Consumer: `populate_provider_facets` estimates whenever capacity is not
per-account authoritative — every reachable case today, and the guard steps
the estimate back the day per-account headers exist (the
authoritative-beats-estimate rule, KILLED mutation b). `to_routing` carries
the facet; `status`/`entitlements` render `headroom estimate:` as its own
segment, never merged into `capacity:`. Nothing scores on it yet —
`routing/session.rs` untouched; a scoring consumer is a later ruling.

13 new shipped-surface tests; targeted gate on the merged tree: 136+227+54
lib tests across the touched modules, 13/13 twice. Full sweep: the wave's
trailing run. Remaining 32C lines (1247–1249, 1252, 1253, 1255) need
plan-change detection, learned resets, multi-window distinction, and the
persistence/override/disable trio — each its own producer decision.

# Lines 1248, 1249, 1252, 1255 — COMPLETE 2026-09-01

Package `GH-ESTIMATOR-SIGNALS` (Sonnet, high, Amber; batch 76). Four facets
of the ONE estimator batch 74 shipped, and every one of them preserves its
architecture: **still derived on read** — no table, no migration, no
persisted estimator state.

**1248 — a learned reset window, and it never displaces a stated one.**
`learn_reset_window_seconds` (`routing/evidence.rs`) derives a fallback
window from the throttle→success recoveries already in the estimator's own
`scoped` slice, consulted **only** where the caller supplied no
`seconds_until_reset` at all — a `match seconds_until_reset { Some(s) =>
(Some(s), ResetBasis::Stated), None => …learned… }`, so an authoritative
reading is structurally unreachable by the fallback. One recovery is an
anecdote: `MIN_LEARNED_RESET_RECOVERIES = 2`, a named constant beside
`RECENT_SIGNAL_HORIZON_SECONDS`. `ResetBasis::{Stated,Learned,Unknown}` rides
on the estimate so a *learned* reading can never render as a provider's word
— `entitlement_facets` labels it `reset: learned`, which a stated reading
never carries.

**1249 — two horizons, and a third state that refuses to guess.**
`LONG_SIGNAL_HORIZON_SECONDS` (3 days) sits beside the existing recent
horizon, and `LongWindowPressure::{Present,NoPressure,Undistinguished}`
reports the long window as a fact **separate from the band** — the band match
is untouched, pinned by the no-new-signal regression. The map line's own
qualifier, *"when evidence allows"*, is the `Undistinguished` variant:
evidence that never reaches the long horizon reads *"we did not look that
far"*, never `NoPressure`. That is the same read-zero-versus-unread-signal
discipline `AffinityFacet` keeps.

**1252 — an override that is visibly the user's.**
`ConfiguredHeadroomBand` is a newtype over `HeadroomBand` with its own
parse/serialize, so an override is **expressed in the band vocabulary and
cannot carry a percentage or a token figure** — 1250/1251 survive by
construction, not by convention. `[entitlements.<name>] headroom_override`
displaces a disagreeing derived band at the consumer and renders in its own
words, *"(your reading, overrides the estimate)"*, never the derived
estimate's confidence-and-basis phrasing. `deny_unknown_fields` on
`EntitlementConfig` is unchanged, so a typo is still refused.

**1255 — disabled means absent.** `disable_headroom_estimate` short-circuits
`populate_provider_facets` **before** the estimator runs, leaving
`headroom_estimate` at `None` — rendered `headroom estimate: unknown`, this
module's existing spelling of unknown. Not zero, not `Moderate`, not a band
labelled "disabled". Scoped per entitlement rather than globally: the
acceptance test requires two entitlements in one config to disagree, and the
proof is that a disabled entitlement renders unknown while an enabled one
beside it still estimates, with `capacity`/`reset`/`throttling`/`models`
unchanged.

**Gates.** `subscription_estimator` 20 passed / 0 failed;
`--lib routing::evidence` 54 passed / 0 failed; `--lib config` 136 passed /
0 failed; clippy `-D warnings` and `cargo fmt --check` clean; rustdoc clean.
Targeted blast radius on the merged tree green after attribution (see below).

**Four mutations, all KILLED — and two of them are the orchestrator's.**
The worker ran the two its packet named; **the packet named three mutations
for four boxes**, an orchestrator error, so 1249's and 1252's were run at
review before either box was ticked:

- *guard-inversion* (1248): the stated-reading match forced to the learned
  path — KILLED by
  `test_1248_two_or_more_recoveries_learn_a_window_and_never_displace_a_real_reading`,
  *"a real seconds_until_reset reading is never recomputed from recoveries:
  reset_basis: Learned (expected Stated)"*.
- *threshold-removal* (1248): `MIN_LEARNED_RESET_RECOVERIES` → 1 — KILLED by
  `test_1248_one_recovery_is_an_anecdote_and_learns_nothing`.
- *disable-neuter* (1255): `if self.disable_headroom_estimate` → `if false`
  — KILLED by
  `test_1255_a_disabled_entitlement_renders_unknown_while_an_enabled_one_beside_it_still_estimates`.
- *1249-guess-instead-of-undistinguished* (orchestrator): `Undistinguished`
  → `NoPressure` — KILLED by
  `test_1249_thin_evidence_renders_undistinguished_not_a_guessed_bucket`,
  *"nothing in `scoped` reaches the long horizon, so absence cannot be
  claimed"*.
- *1252-override-ignored* (orchestrator): `match (entry.headroom_override(),
  …)` → `match (None, …)` — KILLED by
  `test_1252_a_user_override_displaces_a_wrong_derived_band_at_the_consumer`.

**Attribution recorded, because a gate went red.** The merged-tree targeted
run failed
`shell::shell_entitlement_scrub_tests::a_shell_started_native_session_does_not_carry_a_configured_entitlements_variable`
(135 passed / 1 failed) while the wave-75 trailing sweep was still running on
the same checkout. The rule was stated before the result was read — a failure
inside a documented family and outside every changed file is load — and the
second run, alone, gave `--lib config` **136 passed / 0 failed**. The test
lives in `shell/mod.rs`, which this package does not touch.

**Recorded limits, from the worker and kept.** The learned window is proven
*derived, gated, non-displacing and distinctly rendered* — not that its
numeric value predicts a real reset. `LongWindowPressure::NoPressure` is
representable and reachable but has no dedicated test. 1252 is proven through
the shipped binary at one band pairing. Long-window pressure is additive: it
never enters the band match.

**The two lines this package deliberately did not take.** 1247 and 1253 were
refused at Phase −1 and are now rows in the refusal register: nothing
persists a prior plan reading to detect a change against (Cluster H), and
1253's *"so the scheduler can improve"* has no consumer while nothing scores
on the estimate (Cluster D). **Phase 32C therefore stands at 10/12, and its
remainder is blocked rather than merely open.**

## 1247 — CLOSED 2026-09-02 (`GH-ESTIMATOR-RESET`, Amber, Sonnet high): the quota-behaviour half has a producer, and the line is an *or*

**The line.** *Reset or re-calibrate an estimator when Glasshouse detects a
plan change or materially different quota behavior.* The refusal above stood
on the plan half — nothing persists a prior plan reading — and read the line
as needing both. It is an *or*: a materially different quota behaviour is
observable without any plan signal, from the gateway's own captured
rate-limit headers, and that half is what this package proves. The plan half
stays out of scope with no producer (`CapacityState::plan(...)` has no
production caller; design note *Re-calibrating the headroom estimator when
the quota regime changes* and the register's Cluster H row).

**Contract.** Given a provider whose gateway-captured rate-limit headers
state a ceiling (`limit`, `window_seconds` or `token_limit`) and a later
exchange whose headers state a different ceiling, when the gateway persists
the later reading, Glasshouse records that instant as the provider's regime
change (`regime_changed_at_unix` in the quota cache file), every headroom
estimate for that provider is thereafter derived only from routing
observations at or after it, and the `entitlements` line says *limits
changed <age>* — while preserving that `remaining`, `reset` or
`retry_after` moving alone is never a regime change, that a first reading
records none, that a cache file written before the field existed reads as
no change recorded and estimates over the whole window, and that rows before
the change are neither deleted nor relabelled.

**Production.** `provider/telemetry.rs :: stated_ceiling_changed` (the
comparison — a ceiling absent on either side is never evidence of change),
`GatewayQuotaCache::try_store` (records the instant on a change, carries the
earlier instant forward otherwise), `GatewayQuotaCache::regime_changed_at`
(the reader); `config/mod.rs :: ResolvedEntitlement::populate_provider_facets`
(the floor — `filter`, so a `None` floor keeps every row — and the
`since_unix` stamp on the returned estimate);
`routing/evidence.rs :: SubscriptionHeadroomEstimate::since_unix`;
`main.rs :: entitlement_facets` (the render, through `format_age`).

**Regression** (`tests/estimator_reset.rs`, 5/5, the first through a real
gateway on a loopback stub): a stated-ceiling change is detected and the
instant carried forward across an unchanged third exchange; the pool moving
alone records none; a field absent on either side never compares; the floor
keeps only rows at or after the change and the shipped binary's line says
*limits changed*; a pre-field cache file loads as no change and estimates the
whole window.

**Mutations, 4/4 KILLED** (worker's `mutate.sh`, restores byte-identical):
`never-detected` (the comparison → `false`), `remaining-counts` (`remaining`
added to the compared ceilings), `floor-dropped` (the filter → keep all),
`instant-not-carried` (an unchanged ceiling → `None`) — each named test's
failure text quoted in `.agent-runtime/report-estimator-reset.md`.

**Two open decisions the worker made, accepted:** a sibling accessor
`regime_changed_at(provider)` rather than widening `load`/`load_all`'s
tuples (a production caller in `provider/resources.rs` destructures the
triple); the caller pre-filters and stamps `since_unix` after the call
rather than threading a parameter through `estimate_subscription_headroom`'s
twenty test call sites.

**Recorded limits** (the worker's): only `limit`, `window_seconds` and
`token_limit` are compared — a change in a window's *meaning* with the same
numbers is unobservable from headers; `since_unix` is set by the
estimator's one production caller and a second caller would have to do the
same; `load_all` is unchanged; macOS only (no `#[cfg]` in the diff).

State: **COMPLETE** on the line's quota-behaviour disjunct. Phase 32C stands
at 11 of 12; 1253 stays refused (Cluster D).
