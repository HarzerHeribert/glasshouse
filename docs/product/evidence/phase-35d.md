# Capability evidence — phase 35D

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 35D — routing under subscription pressure, 7 of 8 (lines 1570–1577), plus Phase 38's 1606 and 1612; 1610 refused

Package `GH-SUBSCRIPTION-PRESSURE`, 2026-08-31, Fable specialist at xhigh.
**Eleven lines against one policy module**: `routing/pressure.rs` (new) — pure
functions of values, no clock, no store, no socket — feeding two new
contributions into `SessionRouter::score`, over capacity facts the caller reads
from the same cache `disposable_candidate_capacity` already reads.

Contract: Given the destinations a launch or task boundary can choose among,
when a premium resource behind one of them is in the tight or reserve band,
Glasshouse prefers an adequate available alternative, protects the reserve for
heavy work, keeps a warm high-value session rather than moving it over
tightness alone, relaxes conservation as the window's reset approaches, and
names every term that moved the ranking — while preserving that a signal
constant across the candidate set (or unread for a destination) is an inert,
labelled term and never a guess.

State: **COMPLETE** for 1570, 1571, 1572, 1573, 1574, 1575, 1576, 1606, 1612.
**PARTIALLY VERIFIED** for 1577 (interactive half complete; background half has
no reader — see below). **REFUSED** for 1610.

Production evidence:
- `crates/glasshouse/src/routing/pressure.rs` — `capacity_band_pressure` (1570,
  1571, 1573, 1574, 1577) and `low_tier_spend` (1575) over `CapacityFacts { band,
  seconds_until_reset }`, `Option<WorkloadTier>`, `ReservePolicies { interactive,
  background }` selected by `ReserveScope`, and set-level `Alternatives` the
  router computes (`session.rs::alternatives_for`). The reserve-band arm reuses
  Phase 32F's `evaluate_reserve_spend` **unchanged** when the tier is known.
- `crates/glasshouse/src/routing/session.rs` — `Destination::with_capacity_facts`,
  `SessionRouter::with_reserve_policies`, the two `score` pushes,
  `alternatives_for` (requires `provider_available` for both halves — decision 7).
- `crates/glasshouse/src/main.rs` — `destination_capacity` fills the facts from
  `observed_capacity` (same cache, no network); `destination_backend` reads
  `ProviderConfig::cost_of`, so a free-model profile is a **zero-cost
  destination** — `Cost::Free` gains a production producer on this path;
  `session_router(effective, override)` attaches the configuration on all three
  ranking paths (launch, route, resume).
- `crates/glasshouse/src/config/mod.rs` — `[routing.reserve] interactive |
  background = "protect" | "spend"`, layered per field; `exclude` is refused by
  the loader (decision 3: an excluded sole destination would make `choose`
  answer `None` and the launch proceed with no routing line — a silence
  indistinguishable from "nothing excluded"; a penalty stays in the explanation).

Regression evidence (`tests/subscription_pressure.rs`, 15 tests, five on the
shipped binary; `routing::pressure` unit tests 8; `routing_policy` 28;
`route_command` 36; `routing_score` 4):
- `a_tight_premium_destination_loses_to_a_healthy_adequate_alternative` (1570)
- `the_reserve_band_is_kept_for_top_tier_work` (1571, 1606)
- `a_warm_high_value_session_is_not_abandoned_over_tightness_alone` (1572)
- `an_imminent_reset_relaxes_conservation_and_the_explanation_says_so` (1573/1574)
  and, on the binary with a planted `ratelimit-reset` header,
  `the_launch_path_reads_the_band_and_the_reset_…`
- `a_low_tier_task_does_not_spend_a_subscription_when_a_free_adequate_resource_is_healthy` (1575)
  and `a_free_model_profile_is_a_zero_cost_destination_on_the_routing_path` (binary)
- `interactive_and_background_reserve_policies_are_independent` (1577) and
  `the_interactive_reserve_policy_is_read_from_configuration` (binary)
- `pressure_terms_with_no_reading_are_inert_and_named_as_such` (1576)
- `the_policy_names_no_provider_or_model` (1612) — whole-word scan against
  `provider::templates()`, `IntegrationId::ALL` and a model-family list
- `the_policy_does_not_invent_task_completion` (1610's tripwire)
- `a_reserve_band_destination_is_not_denied_in_favour_of_an_unavailable_alternative`
  (decision 7 — found by `route_command.rs`'s 1599 tests on the first full run)
- `an_unknown_tier_decides_exactly_as_the_lowest_tier_would` — holds the
  unknown-tier mirror equal to `evaluate_reserve_spend` on `is_allowed`.

Failure / isolation evidence — fifteen mutations, fifteen KILLED, five on the
shipped binary, none by compile failure (`scripts/mutate.sh`):
- `TIGHT_BAND_PENALTY` → 0 (1570) — *"the healthy band must win over the tight
  one at the same percentage, in either order"*; → −3.0 (1572) — *"a warm session
  is worth more than tightness costs"*.
- `RESERVE_DENIED_PENALTY` → −0.35 (1571) — the warm reserve session kept
  standard work: `left: "reserve" right: "healthy"`.
- reset relief removed (1573/1574) — killed by the unit test **and** the binary
  test: *"the destination whose reset is imminent must win the tie on tightness"*.
- `LOW_TIER_SPEND_PENALTY` → 0 (1575) — *"leaf work leaves a tight subscription
  for a healthy free resource"*.
- band word dropped from the evidence string (1576) — three tests, incl. binary
  `paid.contains("in the tight band")`.
- `for_scope` reading the wrong field (1577) — *"the background policy must not
  move an interactive ranking"*, unit and binary.
- tier never admits the reserve (1606); a provider name planted in the module
  doc (1612); `task_nearly_complete: true` — the fabricated producer (1610) —
  *"the reserve verdict must pass the refused input as `false` exactly once"*.
- **§35 wiring mutations on the binary**: facts never attached
  (`CapacityFacts::UNREAD`) — the deciding launch resumed the wrong session and
  `route` printed no band; every destination metered again — *"the free profile
  must be a zero-cost destination"*; router ignoring the configured policy; the
  `capacity_band_pressure` push commented out; `provider_available` dropped from
  `alternatives_for` — killed by the 1599 tests in `route_command`.

Decisions inside the packet's ruling, accepted with reasons in doc comments at
each site: **"premium" is `Backend::cost() == Metered`** (`ResourceFacts` carries
no such fact; the reset time, not a second flag, separates quota shapes);
**unknown tier at the reserve gate is conservative, not inert** (1459 — an inert
spending protection would leave the reserve unprotected on every production
path today, where the tier is `None`; the low-tier *positive* claim stays inert
on `None`); `spend` removes the denial, not the tight-shaped pressure; 1575
requires the premium destination to be under pressure (band ≤ Tight) — a
subscription with plenty of room is not *exhausted* by leaf work; line 1290's
`reserve_override_sessions` is honoured through `disposable::ReserveOverride`
for an existing named session, never a fresh destination. Magnitudes are
judgement pinned by orderings, not numbers: −0.35 < capability-absent 0.4 ≪
warmth 1.5; −2.0 > 1.5 and < 2.5 (warmth + cold bootstrap) so a denial can move
warm work to a comparable alternative but never to a session starting from
nothing; −3.0 > 2.5, the one term the contract lets outweigh a warm session.

**1577 — PARTIALLY VERIFIED, and why it is not ticked.** Both fields exist, layer
per field, round-trip and reject an unknown spelling; the session router reads
`interactive` on every ranking path. **Nothing reads `background`**: the site is
`routing/disposable.rs:712`, the `evaluate_reserve_spend` call, where a `Spend`
policy should skip the reserve gate — `disposable.rs` belonged to
`routing-economics` this round. Successor: **`GH-BACKGROUND-RESERVE-READER`**, one
Green box (folded into the next disposable-router package).

**1610 — REFUSED**, line 1294's standing refusal (`quota.rs:2265-2285`): nothing
in this build observes that a task is nearly complete — every `LifecycleEvent`
is binary and retrospective, and the router runs at session start or a task
boundary, where the only completion fact is that the previous turn is over.
`reserve_verdict` passes `task_nearly_complete: false` with the refusal cited at
the site, and the tripwire test kills the fabrication. Becomes in-repo only if a
harness reports task progress.

Recorded limits — the thin spots, named by the worker so verification is spent
there:
- **The tier is `None` on every production path in this tree** (the launch path
  classifies only once `launch-classifier` lands), so 1575's term and 1571/1606's
  Heavy admission are proven at the router and on the binary's *conservative*
  branch; the zero-cost producer is binary-proven. **Composed-tree check owed
  after `launch-classifier` integrates**: with a free-model profile and a tight
  premium reading planted, `glasshouse route --task "<leaf-shaped text>"` prints
  `low-tier spend` moving the ranking.
- **A native subscription's band is unread on the routing path** — only the
  gateway quota cache is gathered; `GatheredTelemetry::gather_harness_status`
  (`resources.rs:250`) is a file read of the same cost that would let the
  harness's own subscription reach the router. Named successor (Green): one call
  in `routing_destinations`. Direct-provider bands are read and binary-proven.
- macOS only; the `#[cfg(windows)]` fake-harness arm is copied from
  `route_command.rs` and untested here.
- `Cost::Free` reaches a destination only for a direct-provider profile whose
  named model is in `free_models`; a `HarnessDefault` profile stays metered,
  fail-closed.
