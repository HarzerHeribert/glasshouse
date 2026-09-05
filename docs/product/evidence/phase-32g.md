# Capability evidence — phase 32G

Phase 32G — *provider-aware request-cost estimation*. Ten lines, and until
2026-09-01 all ten were unreachable for one reason: **there was no price data
anywhere in this build.** The census — which line is blocked by what, read
rather than assumed — is in `docs/process/refusal-register.md` under
*"Phase 32G — the census"*.

# Lines 1305, 1306 — mechanism landed, both boxes HELD OPEN (superseded below, same day)

Package `GH-PRICING-CHANNEL` (Sonnet, high, Amber; batch 76). §83's
*"attack the channel, not the lines waiting on it"*: the eight other lines
each need an estimate, and no estimate is possible without a price source.

**What shipped.** `provider/pricing.rs` — `PriceTable` with
`load_from_dir(dir)` reading `pricing.toml` (`PRICING_FILE_NAME`) out of the
runtime's config directory, `parse` rejecting negative, non-finite and
oversized values and oversized documents, and `price_for(provider, model)`.
`routing/session.rs` gains `SessionRouter::with_price_table`, defaulting to
`PriceTable::empty()` — what every candidate saw before this package — and
`expected_marginal_cost` now distinguishes three cases instead of two: a
**free** destination stays a known zero, a **metered** destination with a
price is priced from it, and a metered destination with **no entry** carries
a nonzero magnitude and says *unknown* in its evidence. A reader of the
routing explanation can tell *"costs nothing"* from *"nobody knows what this
costs"*.

**A production caller, added by the orchestrator at review.** The worker was
forbidden `main.rs` (two other workers held it), and said so plainly in its
own limits: *"no production caller wires `PriceTable::load_from_dir` into
main.rs yet."* `scripts/cluster-b.py`'s shape — a mechanism built, tested and
never installed — is behind **all ten** wrongly-ticked boxes in this
project's history, so the check was run before the ruling and confirmed it:
every reference to `PriceTable` lived inside the two new files or their own
doc comments. The wiring went into `session_router` (`main.rs`), the one
function all three ranking paths already share and whose doc comment says
so, so the path that acts and the path that reports read the same file.

**Why both boxes are HELD OPEN anyway.** The chain is complete on paper —
producer, production caller, propagation, consumer, tests, mutations — but
**nobody has yet watched a user's `pricing.toml` change what the shipped
binary prints.** The worker's own limit says its proof stops at the
`SessionRouter` public API (the boundary `interactive_score_terms` uses), and
the orchestrator's attempt at a live `glasshouse route --task` run produced
no ranked candidate for an unrelated profile-configuration reason and was
abandoned rather than dug into. Two independent signals of the same gap.
This project has ten precedents for ticking a box whose mechanism was real
and whose reachability was not, and every one was found by a later audit.
**One shipped-binary observation closes both**, and it is named as the
successor's first task.

**Gates (merged tree).** `routing_pricing` 6 passed / 0 failed;
`provider::pricing` 8 unit tests; `--lib provider` 358 passed / 0 failed;
`--lib routing::session` 1 passed / 0 failed; `interactive_score_terms`
7 passed / 0 failed; `route_command` **39 passed / 0 failed** with the wiring
in place, which is what says the added production call does not disturb the
binary's routing; clippy `-D warnings`, `cargo fmt --check`, rustdoc and
`check-doc-boundary.sh` all clean; `blast-radius.sh --targeted` — every
traced target passed.

**Two mutations, both KILLED.**

- *fake-zero-collapse*: the unknown-price arm's magnitude → `0.0`, the fake
  zero the line forbids — KILLED by
  `routing_pricing::a_metered_destination_with_no_price_entry_renders_as_unknown_not_free`
  (two further tests failed with it).
- *loader-ignores-user-file*: `load_from_dir` returns `empty()` before
  reading the directory at all — KILLED by
  `routing_pricing::an_unrecognized_providers_price_reaches_the_explanation_with_no_recompilation`.

**Recorded limits, kept.** No schema validation beyond TOML well-formedness
and per-field range checks: an unrecognized extra table is ignored rather
than refused, a design choice no test pins. No estimate is derived from a
price yet — that is 1298's work and 1298 has no input-size producer.

**The successor, and it is one package.** (1) The shipped-binary observation
that closes 1305 and 1306. (2) Line **1307**, *"record the estimated cost
used in a routing decision"* — `RoutingObservation` already carries
`pub cost: Option<ObservedCost>` (`routing/evidence.rs:451`) and
`EvidenceLedger::record` already accepts it; its production writers are
`main.rs:1678` and `:1730`. Both halves live in `main.rs` and both are
additive to the wiring above.


# Lines 1305, 1306 — COMPLETE 2026-09-01; the held boxes close on a shipped-binary observation

Package `GH-PRICING-RECORDED` (Sonnet, high, Amber; batch 77). **This package
changed no production code at all** — 161 insertions, all in
`crates/glasshouse/tests/route_command.rs` — which is exactly what the
holding ruling above asked for. The mechanism was already right; what was
missing was somebody watching it work in the real binary.

Four tests, on the shipped binary's own fixture (a planted harness on PATH,
an argv log, a real config dir), with `plant_pricing` writing `pricing.toml`
where `PriceTable::load_from_dir` actually resolves it — so the orchestrator's
`session_router` wiring is exercised end to end rather than asserted:

- **1306** — `a_pricing_toml_this_binary_was_never_compiled_with_reaches_the_real_route_output`:
  a provider/model this binary has no compiled knowledge of, and the real
  `glasshouse route` output contains *"its price is known"*, *"$3.00 per
  million input tokens"* and *"$9.00 per million output tokens"* — the exact
  figures from the planted file.
- **1306** — `correcting_the_price_in_the_file_changes_the_next_runs_real_output`:
  `$1.00`/`$2.00` before, `$5.00`/`$20.00` after, **and a negative assertion
  that the old figure is gone**. Updated independently of the router, with no
  recompilation, which is the line's whole claim.
- **1305** — `unknown_and_free_are_textually_distinct_in_the_real_route_output`:
  *"its price is unknown"* for a metered destination with no entry, *"is a
  zero-cost resource"* for a free one. The distinction the line exists for,
  in real output rather than at an API boundary.
- **1305** — `with_no_pricing_toml_the_base_fixture_still_says_unknown_never_a_fabricated_zero`:
  the default state of every user who has not written the file.

`route_command` goes 39 → **43 passed, 0 failed**. Mutations were not re-run
and correctly so: the report claims no production code changed, and
`git diff --stat` confirms one test file — so `GH-PRICING-CHANNEL`'s two
KILLED mutations still stand over the same production source.

**Recorded limits, kept.** Proven at the `SessionStart` moment (no `--task`,
`movement = None`); a tier-movement moment takes a separate documented
zero-priced early return (`session.rs:1254-1261`) and was not exercised —
expected behaviour, not a gap. macOS and Linux locally; the Windows VM leg
was not run.

**Why this is worth a paragraph in its own right.** The holding ruling cost
one extra package and produced a proof that the API-level tests could not
give. Eleven times in this project a box has been ticked whose mechanism was
real and whose reachability was not, and ten of those were found later by an
audit worker. This is the one that was caught first, and the follow-up that
closed it took a Sonnet under half an hour.

# Line 1307 — REFUSED 2026-09-01, and the refusal corrected the register

`GH-PRICING-RECORDED` was also asked to give
`routing_observations.cost_micro_usd` its first producer. **It refused, and
it was right to.** The orchestrator's own register row had called 1307
*"not refused, and closer than any row here"* because `RoutingObservation::cost`
and `EvidenceLedger::record` both already exist. They do, and it does not
help:

- `record_tier_movement` (`main.rs:~1651`) receives **no `Destination` at
  all** — `TierMovement` carries tier labels and reasons, nothing priceable.
- `record_entitlement_fallback` (`main.rs:~1698`) does receive one, so
  `PriceTable::price_for` answers there — **but a per-million-token rate is
  not a cost without a token count to multiply it by.** Writing a rate into a
  column documented as a monetary reading would misrepresent `ObservedCost`
  and make the line's own *"compare estimate against actual usage"*
  meaningless.
- A crate-wide grep for any reachable size estimator found none; the single
  hit, `firewall::store::original_token_estimate`, belongs to the context
  firewall and is not in scope at any routing call site.

The packet told the worker not to fabricate a second estimate when the value
is not in scope at the writer, and it followed that instead of producing a
green box. **1307 therefore joins 1298, 1299 and 1304 waiting on one thing:
an input-size producer at the routing decision point.** That single producer
unblocks four of this phase's ten lines. The register's Phase 32G census has
been corrected accordingly.

# Lines 1298, 1299, 1304 — COMPLETE 2026-09-01. **1307 HELD OPEN on a SURVIVED mutation.**

Package `GH-INPUT-SIZE-PRODUCER` (Sonnet, high, Amber; batch 77). The
producer this phase's census named as the single blocker behind four lines.

**Where the code actually landed, because the commit message lies.** The
implementation — 1005 insertions across `config/mod.rs`, `main.rs`,
`routing/{evidence,mod,session}.rs` and two test files — is in **`645d6cf`**,
whose message is entirely about correcting a measurements entry. The
orchestrator integrated this package, was interrupted mid-review, and then
ran `git add -A` for an unrelated docs commit, sweeping the whole worker
diff in with it. The code is correct and was gated (`blast-radius.sh
--targeted`, every traced target passed, 143+227+54+15+13 quoted); only the
message is wrong, and history was already pushed, so it is corrected forward
here rather than rewritten. **Anyone bisecting this phase should look at
`645d6cf`, not at this commit.**

**1304 — the estimate is measured, not modelled.** Project memory is counted
by calling `memory::inject::briefing` with the real task and running
`firewall::estimate::estimate_tokens` over the text it would actually
inject — a measurement of the real briefing, not a constant. Checkpoints are
measured from the real document via `checkpoint::store::latest_for`, never
from `MAX_BYTES` (a ceiling is not a size). **"Likely repository reads" is
deliberately OMITTED** and recorded as a limit: nothing in this build
predicts which files an agent will open, and inventing a figure there would
fabricate the largest component of the estimate. The line's own *"when
possible"* is what permits the omission. Mutation
*briefing-replaced-by-constant* — KILLED by
`estimated_project_memory_tokens_measures_the_real_briefing_and_changes_with_it`.

**1298 / 1299 — a cost only where both halves are known.** A metered
destination with a known price and a known size is priced; **unknown size
makes the cost unknown even when the price is known**, and free stays a
known zero. 1299's cold resume estimates from that session's own latest
checkpoint — the honest approximation the line's *"or approximated"* allows
— and a session with no checkpoint is unknown, not zero. `WarmSession`'s
standing refusal about accumulated context is untouched. Mutation
*fake-zero-on-unknown-size* (`total_tokens()?` → `.unwrap_or(0)`) — KILLED
by `routed_cost_is_none_when_size_is_unknown_even_with_a_known_price`,
*"unknown size must record no cost row at all, never a fabricated zero"*.

## 1307 — HELD OPEN, and the worker's own mutation is why

The worker returned `verdict: closed` for 1307. **The orchestrator overrode
it to OPEN**, on evidence the worker itself produced and reported honestly.

Its third mutation — `main.rs`, `record_entitlement_fallback`:
`.with_cost(cost)` → `.with_cost(None)` — **SURVIVED** against 130 tests
(`routing_pricing` 63, `routing_evidence` 39, `entitlement_broker` 15,
`--bin glasshouse` 13). Deleting the cost from the writer changes nothing
any test observes.

That writer matters more than the count suggests: `record_tier_movement`
receives no `Destination` and nothing priceable, so
**`record_entitlement_fallback` is the ONLY production path that can write a
cost row**. A SURVIVED mutation there means the one link the line is about —
*"record the estimated cost used in a routing decision"* — is unproven in
production. The surrounding facts are all tested (unknown-size ⇒ no row,
unknown-price ⇒ no row, free ⇒ known zero, a written cost survives its
process, an absent cost leaves the column absent); the delivery is not.

The worker named the reason precisely rather than dodging it:
`EntitlementFallback` has private fields and no public constructor, built
only inside `session.rs`'s fallback-decision logic, so proving the flow
needs a genuine fallback driven through a shipped-binary launch — and it
named the existing fixture shape that does exactly that,
`tests/entitlement_broker.rs::a_launch_that_falls_back_records_the_fallback_with_its_reason`.

**This is the same ruling 1305/1306 got hours earlier, applied to a package
that reported itself complete.** A mechanism that is real but whose
production reach is unproven does not tick here; that shape accounts for all
ten of this project's historical un-ticks, and holding it costs one small
follow-up. **Successor: one shipped-binary test on the `entitlement_broker`
fixture that drives a real fallback and asserts a non-NULL `cost_micro_usd`
with its confidence.** When that mutation is KILLED, 1307 closes.

**Phase 32G now stands at 5/10** (1298, 1299, 1304, 1305, 1306), with 1307
one test away and the remaining four blocked on signals the census names.

---

# Line 1307 — CLOSED 2026-09-01, by exactly the successor the hold named

The hold above asked for *"one shipped-binary test on the `entitlement_broker`
fixture that drives a real fallback and asserts a non-NULL `cost_micro_usd`
with its confidence."* That is what landed, and it took the **priced** path
rather than the free-model escape the packet permitted as a fallback — so the
limit the hold anticipated ("the priced path is unwatched") **does not apply**
and is not recorded.

`entitlement_broker::a_launch_that_falls_back_records_the_chosen_destinations_estimated_cost`
seeds a project checkpoint (so `latest_checkpoint_tokens` is `Some`), writes a
`pricing.toml` for `prov-b`/`shared-model` into the binary's own config
directory, records a throttled `prov-a` observation, then runs the **compiled
binary** as a subprocess — `glasshouse launch claude-code --headless` — and
reads the fallback row back out of `EvidenceLedger::recent`. It asserts
`cost.micro_usd > 0` and `cost.confidence == CostConfidence::Estimated`.

The production reach that was unproven at the hold is now proven by that
subprocess: nothing in the test constructs the row itself.

Mutation `drop-cost-from-fallback-row` (`.with_cost(cost);` -> `;` at
`main.rs:1869`) — the very mutation that SURVIVED and caused the hold — is now
**KILLED**, by that test:

    thread '...records_the_chosen_destinations_estimated_cost' panicked at
    crates/glasshouse/tests/entitlement_broker.rs:2411:28:
    the fallback row carries an estimated cost:

Recorded limits, stated rather than discovered later:

- only the priced/estimated branch of `estimated_cost` is watched; the
  free-model zero-cost branch (`micro_usd: 0`) is not asserted by this test;
- the exact `micro_usd` value is not pinned, only `> 0` — the rendered token
  count is an implementation detail of `Checkpoint::render()`, not a promise of
  this line;
- macOS only; the Linux and Windows legs were not run for this box.

**Phase 32G now stands at 6/10** (1298, 1299, 1304, 1305, 1306, 1307). The
remaining four are blocked on signals the census names.

---

# Independent audit, 2026-09-01 (`GH-AUDIT-BATCH-78`) — 1298, 1299, 1304 and 1307 CONFIRMED

A read-only auditor was dispatched to prove these four **wrong**, on the
standing evidence that all ten of this project's historical un-tickings were
found this way and every one was the shape *"production code whose only callers
are tests"*. It found none of it here.

Method, and it is the one that has actually worked: `cluster-b.py` over the
whole crate first — none of `record_entitlement_fallback`, `routing_destinations`,
`estimated_cost`, `session_checkpoint_tokens`, `estimated_project_memory_tokens`,
`latest_checkpoint_tokens` or `record_tier_movement` appears in its
zero-production-caller list — then each symbol traced by hand, every call site
compared against its file's first `#[cfg(test)]`.

The load-bearing finding for **1307**: `record_entitlement_fallback`
(`main.rs:1833`) is called once, at `main.rs:4928`, inside the **shared** launch
decision block — **not** inside a `--headless` branch. `main.rs`'s first
`#[cfg(test)]` is at line 12696. The routing and fallback block runs *before*
the later `if headless { .. } else { .. }` split, which decides only how the
session is attached. So the test's `--headless` subprocess exercises the same
production path a real launch takes; it is not a test-only door.

**One correction to the record:** 1298/1299/1304's implementation landed in
`645d6cf`, **not** `cd62e83` — `cd62e83` touches only `README.md`, `ORIENT.md`,
`capability-map.md` and `phase-32g.md`. The audit packet said `cd62e83` and the
auditor checked rather than believed it.

Note recorded for future auditors, not a gap: `EstimatedInputSize` has no field
for *"likely repository reads"*, grepped and confirmed absent — so the omission
this phase's entries record is real rather than asserted.

---

## Censused 2026-09-02 (`GH-RECON-33A-32G`) — two register reasons stale, one join named, one ruling parked

- **1302 — the register's reason is STALE; packageable.** *"Nothing distinguishes a request pool from a token-priced allowance"* — `Allowance::RequestPool` and `is_request_pool()` (`routing/free.rs:78-102`) exist and have zero production callers (Cluster B, not Cluster D). *"No `FreePool` outlives one call"* — `routing/burn.rs` now computes a persisted, request-unit-aware `burn_rate`/`forecast` per `ResourceKey` from ledger rows. **Successor: `GH-REQUEST-POOL-COST`** (Amber, `routing/session.rs`): a term beside `expected_marginal_cost` that, where `is_request_pool()`, prices the scarce unit from the burn reader. **Ruling to carry into the packet:** its own axis (`request-pool cost`), not folded into the money term, and inert whenever 1280's `exhaustion forecast` term is already active for the same resource, so one forecast is never priced twice.
- **1301 — a missing join, not a missing wire.** `record_routing_latency` writes `task_class` on a row with no tokens; `record_routing_observation` writes tokens on a row with no class; `Assignment` has no `TaskClass` field. **Successor: `GH-TASK-CLASS-COST-JOIN`** (Amber), sequenced after `GH-TRANSLATED-USAGE-PROOF` so the tokens side is proven first. Shape to rule before dispatch: widen `Assignment` with the class, or join the routing-decision row by session and window.
- **1300 — refused for a different, narrower reason.** The register's *"no cached-input signal exists"* is stale (translated exchanges write `cached_input_tokens`); the live blocker is `ModelPrice` (`provider/pricing.rs:71-75`), which carries input and output rates only, by Cluster H's own deliberate note. **Parked ruling:** whether `pricing.toml` grows an optional `cached_input_per_million_usd` (absent = unknown, no cached estimate) is decided after the translated-usage proof lands and shows the signal is real in production; until then 1300 stays refused for the stated reason.
- **1303 — refused, unchanged.** No occupancy concept exists anywhere; the latency half is closed elsewhere.

---

## 1302 — mechanism landed, HELD OPEN 2026-09-02 (`GH-REQUEST-POOL-COST`, Amber, Sonnet high)

`routing/session.rs::request_pool_cost`, pushed from `score()` beside the
money term on its own axis: for a request-pool allowance with a known
remaining count and a burn forecast that does *not* already exhaust well
before reset, a bounded negative magnitude
(`REQUEST_POOL_COST_PENALTY = -0.5`, strictly below the forecast term's
`-0.7` and warm affinity's `1.5`) that grows as time-to-exhaustion shrinks
(`PENALTY · 12h / (12h + hours)`), naming the count, the rate and the words
*request pool*. Inert and saying so for a token-priced allowance, an unknown
count, no burn rate, or an active exhaustion forecast — one forecast is
priced once. `is_request_pool` has its first production caller. `free.rs`
untouched: `FreePool::allowance` was already `pub` and the enum's fields are
visible from `session.rs`.

### Estimate request-pool cost for free providers whose scarce unit is requests rather than tokens. (line 1302)

Contract: Given a destination whose allowance is a request pool, when Glasshouse ranks it, it prices the scarcity of that pool's requests from the persisted burn rate and remaining count as a qualitative cost, saying so — while contributing nothing, and saying so, for a token-priced allowance, for a pool with no burn rate, and whenever the exhaustion-forecast term is already active for the same resource, so one forecast is never priced twice.

State: **PARTIALLY VERIFIED — HELD, not ticked.** Ruled 2026-09-02. The term is on the real ranking path, both mutations are KILLED through `SessionRouter::choose`, and it is inert and says so in every case the ruling named. Its input is not: `FreePool::allowance` answers `unknown_pool()` (a request pool with `remaining: None`) for every credential the router sees, because nothing in production calls `record_pool` with a provider's remaining-requests reading or `declare_token_priced` for a per-token price — `observe()` writes `Some(0)` only on a rate-limit answer, and the router's pool is rebuilt per call from `adopt_observed`, which carries health and no allowance. So on the shipped path the term always reads *inert: … remaining count is not yet known*. The packet's Phase −1 (inherited, spot-checked by two orchestrators) named `is_request_pool`'s missing caller as the only gap and did not ask whether the allowance's *value* had a producer; it does not, and that is the same shape that re-opened 1517 and 1513 this morning. Successor, named: `GH-POOL-ALLOWANCE` — at the router's pool construction, `record_pool` from the gateway quota cache's remaining-requests reading (the join the 1280 audit traced) and `declare_token_priced` where the price table prices the pair per token; 1302 ticks then, and so does 531, whose refusal named exactly this producer and this consumer.

Production evidence:
- `crates/glasshouse/src/routing/session.rs` — `request_pool_cost`
- `crates/glasshouse/src/routing/session.rs` — `score (the push, beside expected_marginal_cost)`
- `crates/glasshouse/src/routing/free.rs` — `Allowance::is_request_pool (now has a production caller)`
- `crates/glasshouse/src/routing/free.rs` — `FreePool::allowance (existing pub accessor, reused)`
- `crates/glasshouse/src/routing/session.rs` — `Destination::burn_forecast (existing pub accessor, reused)`

Regression evidence:
- `routing::session::request_pool_cost_tests::a_request_pool_spending_fast_scores_lower_than_a_token_priced_twin`
- `routing::session::request_pool_cost_tests::inert_when_the_exhaustion_forecast_term_already_prices_the_resource`
- `routing::session::request_pool_cost_tests::token_priced_or_unknown_is_inert_and_ranking_is_unchanged`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| Contribution::new(REQUEST_POOL_COST_TERM, magnitude, ...) -> Contribution::new(REQUEST_POOL_COST_TERM, 0.0, ...) | `zero-the-magnitude` | **killed** | `routing::session::request_pool_cost_tests::a_request_pool_spending_fast_scores_lower_than_a_token_priced_twin` |
| if forecast.exhausts_well_before_reset() { -> if false && forecast.exhausts_well_before_reset() { | `drop-the-double-pricing-guard` | **killed** | `routing::session::request_pool_cost_tests::inert_when_the_exhaustion_forecast_term_already_prices_the_resource` |

> zero-the-magnitude observed: panicked at crates/glasshouse/src/routing/session.rs:6207:9: a fast-spending pool must cost something: Contribution { name: "request-pool cost", magnitude: 0.0, evidence: "request pool has 40 requests remaining at an estimated 20.0 requests/hour — about 2.0h left at the current rate, over 12 observations" }

> drop-the-double-pricing-guard observed: panicked at crates/glasshouse/src/routing/session.rs:6277:9: assertion `left == right` failed
  left: -0.48868778280542985
 right: 0.0

Recorded scope limits — stated by the worker, not discovered later:
- the magnitude's half-life (12h) and ceiling (-0.5) are reasoned constants, not derived from a measured distribution of real request pools
- Allowance::RequestPool.remaining and the exhaustion forecast's own remaining reading (via CapacityState) are two different producers that this term does not reconcile; each of the three inert branches covers either one being absent
- no end-to-end gateway-response-to-ranking exercise for this term specifically; tested at the same level (direct FreePool construction) as every sibling term in this module's own test suite

---

---

---

## 1302 — CLOSED 2026-09-02 (`GH-POOL-ALLOWANCE`, Amber, Sonnet high): the HELD entry above, lifted

`main.rs::observed_provider_health` now takes `effective`, gathers the same
telemetry `routing_destinations` reads, and per destination: a `Measured`
remaining-requests reading → `FreePool::record_pool` with the provider's own
limit, remaining and `seconds_until_reset` (positive only, never guessed); else
a `pricing.toml` entry for a `Cost::Metered` pair → `declare_token_priced`;
else untouched. The packet named a private accessor; the worker used the
public one a layer up and said so. 363 lines, one file.

### Estimate request-pool cost for free providers whose scarce unit is requests rather than tokens. (line 1302)

Contract: Given a destination whose allowance is a request pool, when Glasshouse ranks it, it prices the scarcity of that pool's requests from the persisted burn rate and remaining count as a qualitative cost, saying so -- while contributing nothing, and saying so, for a token-priced allowance, for a pool with no burn rate, and whenever the exhaustion-forecast term is already active for the same resource, so one forecast is never priced twice.

State: **COMPLETE** — ruled 2026-09-02, lifting this morning's HELD. The allowance now has a producer on the shipped path: `observed_provider_health`, the one function behind all three `RouterInputs.health` builders, records the provider's own limit, remaining and reset from the same `observed_capacity` reading `destination_capacity` uses, and declares a priced pair token-priced. The first acceptance test drives the real `routing_destinations` → `observed_provider_health` → `session_router().choose()` chain from a stored quota reading and reads a non-zero `request-pool cost` term back; both `skip-state-update` mutations are KILLED.

Production evidence:
- `crates/glasshouse/src/main.rs` — `observed_provider_health`
- `crates/glasshouse/src/routing/free.rs` — `FreePool::record_pool (now has a production caller)`
- `crates/glasshouse/src/routing/free.rs` — `FreePool::declare_token_priced (now has a production caller)`
- `crates/glasshouse/src/routing/session.rs` — `request_pool_cost (unchanged; now reachable in a non-inert state)`

Regression evidence:
- `tests::pool_allowance_1302_531_a_measured_remaining_requests_becomes_a_request_pool_and_prices_the_term`
- `tests::pool_allowance_1302_531_a_pricing_toml_entry_with_no_quota_reading_becomes_token_priced`
- `tests::pool_allowance_1302_531_neither_signal_leaves_the_pool_unknown`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| health.pool.record_pool(credential, &PoolReading { limit, remaining, resets_in }, now); -> let _ = (credential, limit, remaining, resets_in, now); | `skip-state-update` | **killed** | `tests::pool_allowance_1302_531_a_measured_remaining_requests_becomes_a_request_pool_and_prices_the_term` |
| health.pool.declare_token_priced(credential); -> let _ = credential; | `skip-state-update` | **killed** | `tests::pool_allowance_1302_531_a_pricing_toml_entry_with_no_quota_reading_becomes_token_priced` |

> skip-state-update observed: panicked at crates/glasshouse/src/main.rs:16692:17: assertion `left == right` failed: the provider's own limit, nothing derived

> skip-state-update observed: panicked at crates/glasshouse/src/main.rs:16796:9: assertion `left == right` failed: a priced pair with no quota reading must declare token-priced, never a pool

Recorded scope limits — stated by the worker, not discovered later:
- resets_in does not distinguish a rolling reset from a calendar one; both feed the same field
- provider classification for the telemetry/price lookup is by Backend::provider() string identity, which could theoretically collide with a harness slug or the gateway's fixed label
- no exercise beyond direct FreePool/CapacityState construction from a real cache file and a real ledger -- no live gateway response in the loop

---

---


---

## Line 1301 CLOSED — 2026-09-02 (`GH-TASK-CLASS-COST-JOIN`, Amber, Sonnet high): the served rows learn the launch's class, and the cost term's evidence states the expected output cost

The join `GH-RECON-33A-32G` named, in the shape the register ruled the same evening: the class rides on the gateway's own state the way the session id does. `SessionRouting::serve_task_class(Option<TaskClass>)` — a separate setter rather than a second parameter, so a caller with a reason to set one fact does not lose the other — is called in `launch_session` at the `serve_session` site with the routed answer's class; `record_routing_observation` stamps `task_class` on every served row (`NewObservation::with_task_class` gains its second production caller). `routing/burn.rs::output_tokens_by_class(rows, now, window) -> Vec<ClassOutput { class, samples, median_output_tokens }>` reads `HARNESS_TURN_PURPOSE` rows with a class and output tokens, the median withheld below `MIN_SAMPLE_FOR_SUMMARY` (returned with its count, never dropped); `main.rs::comparable_output_tokens` reads the window once beside the price table and `session_router` carries it into the router's inputs; `expected_marginal_cost`'s evidence gains `expected_output_cost_evidence`'s sentence — *recent comparable {class} tasks ({n} in the window) produced a median of {m} output tokens, putting expected output cost at roughly ${x}* — or *unmeasured* with the floor or *no task class established*. **The magnitude does not move** (line 1298's precedent, pinned by the fourth mutation).

**The worker's finding, verified against the unmodified binary and accepted:** every real classified task establishes `movement.is_some()` — `RouterAnswer::requirements()` sets `minimum_tier` whenever it sets a classification, and `decide_tier_movement` then always answers — so `expected_marginal_cost`'s known-price arms were unreachable through `glasshouse route --task`; the existing pricing suites exercise them only with no task. The evidence is therefore appended to the tier-established early return as well. This is a fact about Phase 32G's 1298/1299 as shipped: their dollar figure was reachable only for unclassified runs. Recorded here, not re-opened — the contract those lines closed on holds where it was proven.

**Fix-forward carried (practice §39, relayed mid-package):** `gateway/session.rs` had named `crate::session::SessionId` since `GH-OBSERVATION-SESSION-COLUMN`, which the gateway's own scan forbids; `State.session_id` is now `Option<String>`, `serve_session` takes `&str`, both `main.rs` callers pass `.as_str()`, and three call sites in `tests/routing_session_column.rs` moved with it. The whole `gateway` module: 208/208.

### Estimate expected output cost from task tier and recent comparable tasks when useful. (line 1301)

Contract: Given a launch the router classified into a task class and served through a gateway, when that gateway records the session's exchanges, Glasshouse stamps each row with the launch's task class; and given a later routing decision for a metered destination whose class has at least the standing floor of recent comparable rows with output tokens, Glasshouse's expected-marginal-cost evidence states the median output tokens of those rows and the resulting expected output cost at the destination's output rate -- while preserving that below the floor or with no class the evidence says unmeasured and never invents a size, that the term's magnitude is unchanged, that a gateway told no class writes NULL, and that record_routing_latency's own row still carries the class it always has.

State: **COMPLETE** — ruled 2026-09-02 (evening) by the orchestrator after reading `expected_output_cost_evidence` and its two call sites in the worktree. Amber tier: 4/4 mutations KILLED with output (the fourth pins the magnitude at `+0.000`); every target run singly with counts; targeted blast green; the relayed fix-forward carried with the whole `gateway` module quoted green (208/208).

Production evidence:
- `crates/glasshouse/src/gateway/session.rs` — `SessionRouting::serve_task_class`
- `crates/glasshouse/src/gateway/session.rs` — `SessionRouting::record_routing_observation`
- `crates/glasshouse/src/routing/burn.rs` — `output_tokens_by_class`
- `crates/glasshouse/src/routing/session.rs` — `expected_marginal_cost`
- `crates/glasshouse/src/routing/session.rs` — `expected_output_cost_evidence`
- `crates/glasshouse/src/main.rs` — `launch_session (serve_task_class call site)`
- `crates/glasshouse/src/main.rs` — `comparable_output_tokens`
- `crates/glasshouse/src/main.rs` — `session_router`

Regression evidence:
- `task_class_cost_join::a_launched_sessions_classified_task_stamps_its_gateways_served_rows`
- `task_class_cost_join::record_routing_latencys_own_row_is_unchanged_by_this_package`
- `task_class_cost_join::rows_above_the_floor_are_cited_with_their_median_and_output_cost`
- `task_class_cost_join::rows_below_the_floor_read_as_unmeasured_with_the_floor_named`
- `task_class_cost_join::with_no_task_classified_the_evidence_says_no_class_established`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| crates/glasshouse/src/gateway/session.rs :: remove `.with_task_class(task_class)` from record_routing_observation's builder chain | `never-stamp-class` | **killed** | `task_class_cost_join::a_launched_sessions_classified_task_stamps_its_gateways_served_rows` |
| crates/glasshouse/src/routing/burn.rs :: `&& row.task_class == Some(class)` -> `&& true` in output_tokens_by_class | `median-ignores-class` | **killed** | `task_class_cost_join::rows_above_the_floor_are_cited_with_their_median_and_output_cost` |
| crates/glasshouse/src/routing/burn.rs :: `(samples >= MIN_SAMPLE_FOR_SUMMARY)` -> `(true)` | `estimate-below-floor` | **killed** | `task_class_cost_join::rows_below_the_floor_read_as_unmeasured_with_the_floor_named` |
| crates/glasshouse/src/routing/session.rs :: tier-established branch's Contribution magnitude 0.0 -> -0.5 | `magnitude-moves` | **killed** | `task_class_cost_join::rows_above_the_floor_are_cited_with_their_median_and_output_cost` |

> never-stamp-class observed: assertion `left == right` failed: ... row must carry it: RoutingObservation { ... task_class: None, ... }

> median-ignores-class observed: panicked at crates/glasshouse/tests/task_class_cost_join.rs:774:5 (the median-of-1200 assertion)

> estimate-below-floor observed: panicked at crates/glasshouse/tests/task_class_cost_join.rs:817:5 (the unmeasured-wording assertion)

> magnitude-moves observed: panicked at crates/glasshouse/tests/task_class_cost_join.rs:783:5 (the +0.000 magnitude assertion)

Recorded scope limits — stated by the worker, not discovered later:
- macOS only; Linux and Windows legs not run for this box
- no test pins the exact evidence wording independently of which branch (tier-established vs known/unknown-price) produces it

---

## REVIEW — the orchestrator owes an answer to each of these

This section is the point of the generator. Everything above is the
worker's facts, transcribed. Nothing below is decided.

- **1301** — verdict `closed`. Re-run one decisive mutation yourself, then rule (§79: a worker's packet does not bind the integrator).

**Packet errors the worker reported — read these BEFORE its results.**
Thirteen consecutive rounds a worker corrected its packet and was right:
- the FEASIBILITY block implied the output-cost evidence attaches within expected_marginal_cost's known-price match arms; in production every classified task also establishes movement.is_some() (RouterAnswer::requirements() always sets minimum_tier), so those arms are unreachable through glasshouse route --task and the evidence had to be appended to the movement.is_some() early-return branch too -- verified empirically against the unmodified binary before making the change (see report's 'Architectural finding' section)
- mid-package fix-forward (relayed by the orchestrator, practice §39): crates/glasshouse/src/gateway/session.rs pre-existing code (from GH-OBSERVATION-SESSION-COLUMN) named `crate::session::SessionId`, which gateway::tests::the_gateway_imports_none_of_the_modules_that_would_make_it_a_harness forbids; fixed as directed (State.session_id -> Option<String>, serve_session(&str)), which forced three call-site edits in tests/routing_session_column.rs (not in this packet's YOURS) to keep it compiling

**Files touched outside EXPECTED FILES** — disclosed, not hidden:
- `crates/glasshouse/tests/routing_session_column.rs` — three serve_session(&served) call sites only compiled against the pre-fix-forward &SessionId signature; updated to served.as_str(), forced by the orchestrator-directed signature change, no other edit to this file

Gates the worker ran (re-run the decisive ones yourself):
- cargo fmt --all -- --check: clean
- cargo clippy -p glasshouse --all-targets --all-features -- -D warnings: clean
- cargo doc --no-deps -p glasshouse: clean
- cargo test -p glasshouse --test task_class_cost_join: test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
- cargo test -p glasshouse --lib routing::burn: test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 2028 filtered out
- cargo test -p glasshouse --lib routing::session: test result: ok. 24 passed; 0 failed; 0 ignored; 0 measured; 2018 filtered out
- cargo test -p glasshouse --lib gateway::session: test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 2029 filtered out
- cargo test -p glasshouse --lib gateway (fix-forward, whole module): test result: ok. 208 passed; 0 failed; 0 ignored; 0 measured; 1834 filtered out
- cargo test -p glasshouse --test routing_burn --test routing_session_column --test routing_score: test result: ok. 7 + 6 + 4 passed; 0 failed
- cargo test -p glasshouse --test routing_pricing --test route_command: test result: ok. 13 + 43 passed; 0 failed
- scripts/blast-radius.sh --targeted (all six changed files): every traced target passed; 94 full-trace targets skipped (named)


---

## 1300 — COMPLETE 2026-09-05, the parked ruling resolved the day its condition was met

`GH-CACHED-INPUT-PRICE` (Sonnet, Amber — a configuration field and a changed cost computation), worktree `.worktrees/cached-input-price`, report **`.agent-runtime/report-cached-input-price.md`**.

**The ruling this entry parked** — *"whether `pricing.toml` grows an optional `cached_input_per_million_usd` (absent = unknown, no cached estimate) is decided after the translated-usage proof lands and shows the signal is real in production"* — resolved **yes** on 2026-09-05, when `GH-CACHE-TEMPERATURE` (`phase-35b.md`) proved `cached_input_tokens` end to end through the shipped writer and read the ratio back into scoring. The register's older row *"no cached-input signal exists"* was already stale; this entry's narrower blocker was the rate, and the rate is now optional in the file.

### Estimate cached-input cost separately from uncached-input cost when provider pricing supports caching. (line 1300)

Contract: Given a model whose `pricing.toml` entry declares a cached-input rate and a destination whose route has a measured prompt-cache read ratio over the standing sample floor, when Glasshouse estimates that destination's input cost, it prices the expected cached share at the cached rate and the rest at the full rate — while preserving that a model with no cached rate, or a route with no measured ratio, is priced exactly as today to the micro-dollar.

State: **COMPLETE** — ruled 2026-09-05 by the orchestrator.

Production: `provider/pricing.rs :: ModelPrice::cached_input_per_million_usd` (`Option<f64>`, `#[serde(default)]`, validated by the same `validate_price` naming provider and model); `routing/session/scoring.rs :: input_cost_micro_usd` — the one place `tokens × rate` is computed, splitting only when **both** the rate and `destination.route_responsiveness().cache_read_ratio` are `Some`, and otherwise the byte-identical flat expression; both `estimated_cost` and `expected_marginal_cost` call it, so the recorded cost and the printed explanation cannot disagree. Confidence stays `Estimated`: the tier names provenance, not how many of this build's own readings the arithmetic combines.

Regression: `routing_score::a_declared_cached_rate_and_a_measured_ratio_split_the_estimate` and its two preservation siblings (rate without ratio; ratio without rate — each asserting the exact micro-dollar figure), plus `provider::pricing`'s malformed-rate load error; three fixture struct literals in `classification_cost_ceiling`, `classification_time_price` and `last_lines_33c_34b` gained the field.

Mutation: a missing `cached_input_per_million_usd` treated as `0.0` — **KILLED**. *A missing rate is not a free cache* is the failure that would under-price silently and never crash, which is why it was the one mutation owed.

Limits, the worker's and kept: the split is an estimate on a **historical** ratio, not a per-request fact; no test here proves any provider's actual cached-input billing matches the rate someone puts in the file; a route's first `MIN_SAMPLE_FOR_SUMMARY` exchanges are priced flat.

Gates on the merged tree: `routing_score` 14/14 · `--lib provider::pricing` 10/10 · `--lib routing::session` 24/24 · the three fixture targets green · `blast-radius.sh --targeted` exit 0.

**Phase 32G stands at 9 of 10.** Only 1303 (local compute cost through latency and occupancy) is open, on the standing reason: no occupancy concept exists anywhere.
