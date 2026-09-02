# Phase 32E — Burn rate and exhaustion forecasting

**0 of 10 closed at the time of writing. This file exists because the phase had
no evidence-ledger entry at all** — `discover.py --phase 32E` reported *"no file
paths found"*. A read-only census (`GH-RECON-32E`, 2026-09-02, Sonnet high)
established what blocks it, grouped by root cause per practice §87.

**The register's standing claim was that *"most of 32E"* waits on row P1b's
relay-path usage reader. That is wrong in the direction that matters: it is
*one* of ten lines.** Eight are packageable or need a ruling; only 1275 is held
by the ingress wall.

| line | cause | verdict |
|---|---|---|
| 1274 | **4** — already true when measurable | **packageable (proof-only)** |
| 1275 | **2** — ingress relay wall (P1b) | refused |
| 1276 | **1** — `task_class` computed and never persisted | **packageable** |
| 1277-1280, 1282 | **3** — no burn-rate symbol, but every input exists | **packageable** |
| 1281, 1283 | **3a** — restrain Cause 3's own deliverable | **ruled below** |

---

## Cause 1 — `task_class` is computed at every routing decision and never persisted

`RouterAnswer::task_class()` (`routing/request.rs:606`) is a real producer:
every routed request already carries a `TaskClass`. The caller that has it in
scope is `record_routing_latency` (`main.rs:4443`), which builds its
`NewObservation` from `harness` and `purpose` only — **`answer.task_class()`
sits unused in the same function.** `NewObservation` has eleven `with_*`
builders and **no `with_task_class`**; `routing_observations`
(`database.rs:1266-1301`) has no such column.

**Producer exists, caller has it in scope, propagation is the missing link.**
And crucially this is *not* a token-parsing problem: every completed request
produces a row whether or not tokens are exposed, so a moving average of
**request counts** per task class (1276) needs nothing from the ingress wall.

## Cause 2 — the ingress wall, for one line only

1275 wants token consumption per task class, and tokens are what
`gateway::ingress` is designed never to parse. Stays with register row **P1b**;
no successor until the `ingress` ruling.

## Cause 3 — no burn-rate symbol anywhere, but every input already exists

`cluster-b.py` finds no `moving_average` / `burn_rate` / `time_to_exhaustion`
symbol — **these do not exist rather than being built-and-unwired**, the
stronger finding. But remaining capacity, reset timing and per-request rows are
all present, so this is a mechanism to build on existing inputs, not a blocked
signal. Covers 1277, 1278, 1279, 1280, 1282.

## Cause 4 — 1274 already ships when measurable

`record_extraction_observation` (`main.rs:8642`) records real `with_tokens()`
usage where it is measurable, and `record_routing_latency` (`main.rs:4443`)
records the turn itself with **no fabricated value** where it is not. The
line's own hedge is *"when measurable"*, and honest silence is compliance.
**Proof-only package**, the same shape as Phase 33B's 1353/1359.

---

## Cause 3a — 1281 and 1283. RULED 2026-09-02: they are acceptance criteria of Cause 3's package.

The census laid out two defensible answers and declined to choose. **The
ruling is Answer A, with a condition that makes it safe.**

Both lines are Cluster-P-shaped, and today the restrained thing genuinely does
not exist: no rolling statistic to be made non-robust, and nothing surfaces a
forecast at all. By the letter of Phase 33B's Cause D they would be filed
REFUSED.

**They are different in kind from 33B's 1356/1360, and the difference decides
it.** Those restrain mechanisms nobody has a reason to build — one is blocked
by its own phase's wall, the other forbids the very thing that would close it.
**1281 and 1283 restrain the exact mechanism their own sibling lines
(1277-1280) are asking to be built, in the package this census just scoped.**

The 1152 ruling says restraint lines are mutation-proven by violating the
restraint, and that it *"only applies when the restrained thing exists."*
**That condition is about the moment of the tick, not the moment of the
census.** When Cause 3's package lands, the rolling statistic exists and can be
swapped for a naive mean; the forecast exists and can be reworded as a promise.
Both mutations are then real defects that real tests catch. Judging them
against "this instant, before the package" evaluates the wrong moment.

Answer B also carries a cost this project has been paying all week: it files a
refusal that is **stale the day the package lands**, and needs a follow-up
recon to undo. Six stale blockers were found in this repository in two days;
knowingly writing a seventh is not bookkeeping hygiene.

**The condition, and it is not optional.** 1281 and 1283 may tick **only in the
same commit as 1277-1280**, and **only** if each carries its own KILLED
mutation:

- **1281** — swap the robust rolling statistic for a naive mean; a single
  outlier request must then move the estimate and a test must catch it.
- **1283** — swap the estimate wording for promise-sounding text; a test must
  assert the surfaced text hedges.

If Cause 3's package lands without those two mutations killed, **both lines
stay open.** They are not free riders on a package that happens to build their
subject; they are two more mutation-proven lines inside it.

**Generalised, because this will recur:** *a restraint line whose restrained
mechanism is built by the same package is an acceptance criterion of that
package, provided it carries its own KILLED mutation and ticks in the same
commit. A restraint over a mechanism no package is building stays Cluster P.*

---

## Recommended package boundary

Cause 1 + Cause 3 + Cause 4 are **one implementer package** — migration 23,
`routing/evidence.rs`, `main.rs`, `routing/pressure.rs`, `shell/mod.rs` —
closing **1276-1280 and 1282**, proving **1274**, and carrying 1281/1283 as
acceptance criteria under the ruling above. That is **six to nine boxes, all
facets of one mechanism**, squarely in §87's 3-6 target range at the mechanism
level.

**It is Red tier: it contains a schema migration.** Follow migration 18's
`failure_class` `ALTER TABLE ... ADD COLUMN` pattern exactly
(`database.rs:1975`, `:2891`, `:3091`, `:3106`) with the matching rollback and
`columns_of` assertion, and expect the ripple into literal `version, N` pins in
test files that a migration always causes.

---

# Causes 1, 3 and 4 — CLOSED 2026-09-02 (`GH-BURN-FORECAST`, Red, Opus 5 high): eight of nine, and 1276 HELD

Migration 23 (`routing_observations.task_class`), a new pure reader
`routing/burn.rs`, an `exhaustion forecast` term beside `capacity_band_pressure`,
and hedged wording in the capacity overview. Nine mutations run, nine KILLED,
each with its killing test named and its panic quoted below. The targeted gate
caught two ripple stragglers the packet's grep shape missed (the schema census
in `session::store`, and a bare-literal version pin) — both fixed by the
worker and both recorded as packet errors for the next migration packet.

**The packet's own Phase -1 was wrong in one link and the worker caught it:**
`observations_in_window` filters `outcome IS NOT NULL`, and the only producer
holding a `RouterAnswer` writes no outcome, so every row carrying a task class
was invisible to the read the packet named. The worker added a sibling read,
`consumption_in_window`, rather than widening one that four classifiers depend
on, and pinned both halves. A Phase -1 consumer link must be checked for its
*filter*, not only its existence.

**1276 is held**, for the reason the project has now paid twelve times: the
per-class moving average exists, is tested, and is called from nowhere in
production. Its successor is named in its entry.

### Record capacity consumption per completed request or observed harness turn when measurable. (line 1274)

Contract: Given a completed routing decision or extraction turn, when it finishes, Glasshouse records one routing_observations row for it, carrying a token count only where a producer actually measured one, while never writing a fabricated or zero count for a turn whose size nothing measured.

State: **COMPLETE** — ruled 2026-09-02. Proof-only; the mutation sits one level below the two named writers, and both write through it — accepted because the test asserts the row's shape from each.

Production evidence:
- `src/main.rs` — `record_routing_latency`
- `src/main.rs` — `record_extraction_observation`
- `src/routing/evidence.rs` — `EvidenceLedger::record`

Regression evidence:
- `routing_burn::a_completed_request_produces_a_row_and_invents_no_token_count`
- `routing_burn::the_outcome_filtered_read_does_not_see_an_unfinished_routing_row`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| `        let (cost_micro_usd, cost_confidence) = match new.cost {` -> `        if true { return Ok(0); } let (cost_micro_usd, cost_confidence) = match new.cost {` in EvidenceLedger::record | `skip-state-update` | **killed** | `routing_burn::a_completed_request_produces_a_row_and_invents_no_token_count` |

> skip-state-update observed: thread 'a_completed_request_produces_a_row_and_invents_no_token_count' panicked at crates/glasshouse/tests/routing_burn.rs:189:5: assertion `left == right` failed: one row per completed request (4 other tests in the target failed with it)

Recorded scope limits — stated by the worker, not discovered later:
- Proof-only: no production code changed for this line.
- The mutation is on `EvidenceLedger::record`, one level below the two functions the line names; both write through it. No mutation was run on `record_extraction_observation` itself.
- It does not prove a token count is CORRECT where one exists, only that one is present where a producer measured it and absent where none did.

---

### Maintain a short moving average of requests consumed per task class. (line 1276)

Contract: Given routed requests recorded over a window, when a reader asks how much of each task class is being consumed, Glasshouse answers with a robust rolling per-class request rate built from the class each decision actually recorded, while naming no class for a row whose producer ran no classifier.

State: **PARTIALLY VERIFIED — HELD, not ticked.** Ruled 2026-09-02 at integration. The column, the builder, the launch-path propagation and the rows are all proven (the mutation on `record_routing_latency` is KILLED by a test that runs the shipped binary). But `task_class_request_rates` — the moving average the line's own words name — has **zero production callers**: its three call sites all sit after `burn.rs`'s `#[cfg(test)]` at `:558`. That is the shape of eleven of this project's twelve wrongly ticked boxes, and it was found by the same grep §90 asks for. Successor, named: `GH-CLASS-RATE-SURFACE` — call it from `build_project_overview_capacity` (which already reads the same rows) and print one hedged line per class, with a test and a mutation. Ticks then.

Production evidence:
- `src/database.rs` — `MIGRATIONS (migration 23, routing_observations.task_class)`
- `src/routing/request.rs` — `TaskClass::from_stored`
- `src/routing/evidence.rs` — `NewObservation::with_task_class`
- `src/routing/evidence.rs` — `row_to_observation`
- `src/routing/evidence.rs` — `EvidenceLedger::consumption_in_window`
- `src/routing/burn.rs` — `task_class_request_rates`
- `src/main.rs` — `record_routing_latency`

Regression evidence:
- `launch_classification::a_launch_that_classified_records_the_task_class_it_decided`
- `database::tests::migration_23_adds_task_class_and_undoes_cleanly`
- `database::tests::every_task_class_the_type_supports_is_one_the_schema_records`
- `routing_burn::a_recorded_row_carries_the_task_class_it_was_given`
- `routing::burn::tests::task_class_rates_name_only_the_classes_that_have_rows`
- `routing::burn::tests::task_class_rates_see_only_live_rows`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| `.with_task_class(Some(answer.task_class()))` -> `.with_task_class(None)` in main.rs::record_routing_latency | `drop-propagation` | **killed** | `launch_classification::a_launch_that_classified_records_the_task_class_it_decided` |

> drop-propagation observed: thread 'a_launch_that_classified_records_the_task_class_it_decided' panicked at crates/glasshouse/tests/launch_classification.rs:1137:5: assertion `left == right` failed: a launch whose classification needs nothing records `question`

Recorded scope limits — stated by the worker, not discovered later:
- The mutation's killing test runs the SHIPPED BINARY through two launches and reads the ledger it wrote, so the site is on the path the test exercises (practice §80 case 3).
- Only record_routing_latency writes a task class today; every gateway row carries None, which is honest but means the per-class rate describes routing DECISIONS, not harness turns.
- The rate is per-class over live rows; it does not attribute a class to a resource.

---

### Estimate current burn rate for each constrained resource. (line 1277)

Contract: Given a window of recorded requests against one provider, when a caller asks how fast that resource is being spent, Glasshouse answers with a robust requests-per-hour estimate keyed by provider and optionally narrowed to one credential, while answering with nothing at all below a named minimum row count and offering a token rate only from rows that already carry token counts.

State: **COMPLETE** — ruled 2026-09-02. Production callers: `destination_capacity` (`main.rs`) on the launch/route path and `build_project_overview_capacity` (`shell/mod.rs`), both verified from the worktree, both outside `#[cfg(test)]`.

Production evidence:
- `src/routing/burn.rs` — `burn_rate`
- `src/routing/burn.rs` — `ResourceKey`
- `src/routing/burn.rs` — `MIN_ROWS_FOR_BURN_RATE`
- `src/main.rs` — `destination_capacity`
- `src/shell/mod.rs` — `build_project_overview_capacity`

Regression evidence:
- `routing::burn::tests::a_burn_rate_below_the_minimum_row_count_is_absent`
- `routing::burn::tests::the_burn_rate_is_keyed_by_provider_and_narrowed_by_the_account`
- `routing::burn::tests::a_token_rate_exists_only_where_rows_already_carry_tokens`
- `routing_burn::a_thin_history_yields_no_forecast_and_a_real_one_does`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| `    Some(median * SECONDS_PER_HOUR / BUCKET_SECONDS as f64)` -> `    Some(0.0)` in median_rate_per_hour | `zero-the-signal` | **killed** | `routing_burn::a_thin_history_yields_no_forecast_and_a_real_one_does` |

> zero-the-signal observed: thread 'a_thin_history_yields_no_forecast_and_a_real_one_does' panicked at crates/glasshouse/tests/routing_burn.rs:514:10 -- the `.expect("thirty rows and a measured request count is a forecast")` on burn::forecast over real ledger rows

Recorded scope limits — stated by the worker, not discovered later:
- The unit is REQUESTS per hour. A token rate is offered only from rows whose token fields are already Some, and is None in nearly every real window; nothing here parses a response body (line 1275 is refused and untouched).
- The account narrowing is refused wholesale when any candidate row lacks a quota_context, matching recent_credential_throttles; both production callers pass quota_context: None, so today's forecasts are provider-wide.

---

### Estimate time-to-exhaustion when the remaining capacity and burn rate are sufficiently known. (line 1278)

Contract: Given a burn rate resting on enough rows and a remaining capacity the provider published as a whole count of requests, when a caller asks when the resource runs out, Glasshouse answers with a time-to-exhaustion, while answering None -- never a fabricated figure -- for a percentage, an unmeasured amount, a non-request unit, or a zero rate.

State: **COMPLETE** — ruled 2026-09-02. The report's own thinnest spot — *no provider is known to publish a remaining amount in a request unit* — was checked before ticking and is narrower than stated: `provider/telemetry.rs` builds a `requests` pool from `x-ratelimit-remaining-requests` (`:181`, `:322`, `:535`, `NativeAmount::whole(limit, "requests")`) for every provider that sends OpenAI-style rate-limit headers, and that pool is what `state.requests().remaining()` hands `forecast`. The active path is reachable in production for those providers; for the rest the honest answer is `None`, which is the line's own hedge.

Production evidence:
- `src/routing/burn.rs` — `forecast`
- `src/routing/burn.rs` — `measured_requests`
- `src/routing/burn.rs` — `REQUEST_UNITS`

Regression evidence:
- `routing::burn::tests::time_to_exhaustion_is_absent_without_a_measured_request_unit_amount`
- `routing::burn::tests::time_to_exhaustion_is_the_remaining_count_over_the_rate`
- `routing::burn::tests::a_rate_of_zero_produces_no_forecast_rather_than_an_infinity`
- `routing_burn::a_percentage_without_a_native_count_forecasts_nothing`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| `    let amount = remaining.reading()?.value();` -> `    let fabricated = NativeAmount::whole(100, "requests"); let amount = remaining.reading().map(|r| r.value()).unwrap_or(&fabricated);` in measured_requests | `fabricate-an-absent-value` | **killed** | `routing::burn::tests::time_to_exhaustion_is_absent_without_a_measured_request_unit_amount` |

> fabricate-an-absent-value observed: thread 'routing::burn::tests::time_to_exhaustion_is_absent_without_a_measured_request_unit_amount' panicked at crates/glasshouse/src/routing/burn.rs:721:13: assertion `left == right` failed: not applicable is not a count, so it cannot be divided by a rate

Recorded scope limits — stated by the worker, not discovered later:
- All FIVE Capacity non-Measured states are asserted by name (the packet's FEASIBILITY listed only four -- DelegatedUpstream is a fifth).
- No provider in this build is KNOWN to publish a remaining amount in a request unit; every fixture exercising the active path constructs that reading by hand. If none ever does, the forecast is permanently absent in production despite every link being wired.

---

### Estimate whether the resource is likely to survive until its next reset at the current burn rate. (line 1279)

Contract: Given a time-to-exhaustion and a known seconds-until-reset, when a caller asks whether the resource lasts until its next reset, Glasshouse compares the two and answers yes or no, while answering None whenever either side is unknown rather than assuming survival.

State: **COMPLETE** — ruled 2026-09-02.

Production evidence:
- `src/routing/burn.rs` — `forecast (survives_until_reset)`
- `src/routing/burn.rs` — `ExhaustionForecast::survives_until_reset`

Regression evidence:
- `routing::burn::tests::survives_until_reset_compares_against_the_reset_and_is_none_without_one`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| `        seconds_until_reset.map(|reset| seconds_to_exhaustion >= reset);` -> `        seconds_until_reset.map(|reset| seconds_to_exhaustion < reset);` | `invert-comparison` | **killed** | `routing::burn::tests::survives_until_reset_compares_against_the_reset_and_is_none_without_one` |

> invert-comparison observed: thread 'routing::burn::tests::survives_until_reset_compares_against_the_reset_and_is_none_without_one' panicked at crates/glasshouse/src/routing/burn.rs:784:9: assertion `left == right` failed

Recorded scope limits — stated by the worker, not discovered later:
- The test derives both the survives and does-not-survive cases from the SAME forecast's own seconds_to_exhaustion (+/- 600s), so it cannot pass on a build that hard-codes either verdict.
- It compares against seconds_until_reset as the caller computed it; it does not verify that figure.

---

### Reduce routing preference for a resource that is forecast to exhaust well before its next reset. (line 1280)

Contract: Given two otherwise identical destinations, when one is forecast to exhaust well before its reset, the router scores it lower and says so in hedged words in the explanation, while contributing exactly zero and naming itself inert for any destination with no forecast, so a ranking on a build with no forecast is unchanged.

State: **COMPLETE** — ruled 2026-09-02. The term is pushed from `score()` and is inert-and-says-so without a forecast; the inert case is pinned as a byte-identical ranking. Native subscriptions and the gateway reach the inert arm by design (no row names them), recorded as a limit, not a gap: joining a harness name to a provider name would be an invented join.

Production evidence:
- `src/routing/pressure.rs` — `exhaustion_forecast_pressure`
- `src/routing/pressure.rs` — `EXHAUSTION_FORECAST_PENALTY`
- `src/routing/burn.rs` — `ExhaustionForecast::exhausts_well_before_reset`
- `src/routing/burn.rs` — `WELL_BEFORE_RESET_FRACTION`
- `src/routing/session.rs` — `Destination::with_burn_forecast`
- `src/routing/session.rs` — `score`
- `src/main.rs` — `destination_capacity / with_capacity`

Regression evidence:
- `routing_burn::a_destination_forecast_to_exhaust_early_ranks_below_an_identical_comfortable_one`
- `routing_burn::a_destination_with_no_forecast_ranks_exactly_as_it_did`
- `routing::burn::tests::well_before_is_half_the_window_and_is_false_without_a_reset`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| `    let Some(forecast) = inputs.forecast else {` -> `    let Some(forecast) = Option::<super::burn::ExhaustionForecast>::None else {` in exhaustion_forecast_pressure | `make-the-term-inert` | **killed** | `routing_burn::a_destination_forecast_to_exhaust_early_ranks_below_an_identical_comfortable_one` |

> make-the-term-inert observed: thread 'a_destination_forecast_to_exhaust_early_ranks_below_an_identical_comfortable_one' panicked at crates/glasshouse/tests/routing_burn.rs:380:5: assertion `left == right` failed: the forecast is the only axis these two differ in (and a_thin_history_yields_no_forecast_and_a_real_one_does failed with it)

Recorded scope limits — stated by the worker, not discovered later:
- The ranking pair differs in the forecast ALONE -- same percentage, band, reset, cost and freshness -- per phase-9j's constant-signal rule.
- Only DirectProvider resources get a production forecast; native subscriptions and the gateway reach the inert arm because no row's provider column names them, and joining a harness name to a provider name would be the invented join the packet said to stop at.

---

### Avoid overreacting to one unusually large request by using robust rolling statistics. (line 1281)

Contract: Given a window in which one bucket carries a burst many times the steady rate, when Glasshouse estimates the request rate, the estimate moves far less than an arithmetic mean would, so a single unusually large request cannot move a forecast.

State: **COMPLETE** — ruled 2026-09-02 under `phase-32e.md` Cause 3a: same commit as 1277–1280, its own KILLED mutation (naive mean for the median), and the test computes the mean from the same buckets so it cannot pass on a fixture without an outlier.

Production evidence:
- `src/routing/burn.rs` — `median`
- `src/routing/burn.rs` — `median_rate_per_hour`
- `src/routing/burn.rs` — `bucket_counts`
- `src/routing/burn.rs` — `BUCKET_SECONDS`

Regression evidence:
- `routing::burn::tests::one_outlier_bucket_moves_the_median_far_less_than_it_moves_a_mean`
- `routing::burn::tests::the_median_is_the_middle_and_averages_the_two_middles_when_even`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| `    let median = median(&counts)?;` -> `    let median = if counts.is_empty() { return None } else { counts.iter().sum::<f64>() / counts.len() as f64 };` | `replace-robust-statistic-with-naive-mean` | **killed** | `routing::burn::tests::one_outlier_bucket_moves_the_median_far_less_than_it_moves_a_mean` |

> replace-robust-statistic-with-naive-mean observed: thread 'routing::burn::tests::one_outlier_bucket_moves_the_median_far_less_than_it_moves_a_mean' panicked at crates/glasshouse/src/routing/burn.rs:676:9 (and a_rate_of_zero_produces_no_forecast_rather_than_an_infinity failed with it, at :1019)

Recorded scope limits — stated by the worker, not discovered later:
- Ticked in the same commit as 1277-1280 with its own KILLED mutation, per phase-32e.md Cause 3a's condition.
- The test computes the naive mean from the SAME buckets rather than asserting a hard-coded number, and additionally asserts the fixture contains an outlier a mean would actually notice (>50 req/h shift) -- so it cannot pass vacuously on a fixture with no outlier.
- Robustness is proven for the REQUEST rate only. tokens_per_hour is a plain total over a span and its robustness is not pinned.

---

### Reset or decay stale burn-rate history after long idle periods or quota resets. (line 1282)

Contract: Given rows spanning a long idle period or a quota reset that has already happened, when Glasshouse estimates a burn rate, rows before the idle gap and rows before the reset boundary contribute nothing, so a burst from yesterday cannot forecast today.

State: **COMPLETE** — ruled 2026-09-02. The idle-gap half is mutation-proven; the reset half locates a boundary only when the window has demonstrably turned, because nothing publishes a window length — recorded as the line's limit, and it is the conservative reading of *reset or decay*.

Production evidence:
- `src/routing/burn.rs` — `live_rows`
- `src/routing/burn.rs` — `last_reset_boundary`
- `src/routing/burn.rs` — `IDLE_GAP_SECONDS`

Regression evidence:
- `routing::burn::tests::rows_before_a_long_idle_gap_are_excluded`
- `routing::burn::tests::rows_before_the_last_reset_boundary_are_excluded`
- `routing::burn::tests::task_class_rates_see_only_live_rows`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| `        if gap > IDLE_GAP_SECONDS {` -> `        if false {` in live_rows | `remove-the-decay` | **killed** | `routing::burn::tests::rows_before_a_long_idle_gap_are_excluded` |

> remove-the-decay observed: thread 'routing::burn::tests::rows_before_a_long_idle_gap_are_excluded' panicked at crates/glasshouse/src/routing/burn.rs:929:9: assertion `left == right` failed: only the rows after the gap are still evidence about now (and task_class_rates_see_only_live_rows failed with it, at :1004)

Recorded scope limits — stated by the worker, not discovered later:
- The idle-gap half is fully wired and mutation-proven. The reset half only locates a boundary when seconds_until_reset is NON-POSITIVE (the window has demonstrably turned): nothing in provider::quota publishes a window LENGTH, so the previous turn cannot be derived from the next one without inventing a period. Stated in live_rows' own doc comment; the mutation above covers the idle gap, not the reset arm.

---

### Surface exhaustion forecasts as estimates rather than promises. (line 1283)

Contract: Given a resource with an exhaustion forecast, when Glasshouse surfaces it, the text hedges -- 'estimated to last about', 'may not reach its reset at the current rate' -- and never promises, while printing nothing new and a byte-identical line for a resource with no forecast.

State: **COMPLETE** — ruled 2026-09-02 under Cause 3a: same commit, its own KILLED mutation (promise wording), six promise phrasings asserted absent, and the no-forecast line pinned as an exact string.

Production evidence:
- `src/shell/mod.rs` — `forecast_note`
- `src/shell/mod.rs` — `resource_capacity_line`
- `src/shell/mod.rs` — `build_project_overview_capacity`
- `src/routing/pressure.rs` — `exhaustion_forecast_pressure (the explanation text)`

Regression evidence:
- `shell::project_overview_capacity_tests::a_surfaced_forecast_is_hedged_and_never_promises`
- `shell::project_overview_capacity_tests::a_resource_with_no_forecast_prints_exactly_what_it_printed_before`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| `        "; estimated to last about {hours:.1}h at the current rate \` -> `        "; will last {hours:.1}h at the current rate \` in forecast_note | `replace-hedge-with-promise` | **killed** | `shell::project_overview_capacity_tests::a_surfaced_forecast_is_hedged_and_never_promises` |

> replace-hedge-with-promise observed: thread 'shell::project_overview_capacity_tests::a_surfaced_forecast_is_hedged_and_never_promises' panicked at crates/glasshouse/src/shell/mod.rs:4220:9 -- the assertion that the line contains 'estimated to last about 1.5h at the current rate'

Recorded scope limits — stated by the worker, not discovered later:
- Ticked in the same commit as 1277-1280 with its own KILLED mutation, per phase-32e.md Cause 3a's condition.
- The test asserts the hedges positively AND six promise phrasings negatively ('will last', 'will run out', 'will not reach', 'guaranteed', 'certainly', 'exhausts at').
- The inert case is pinned as an EXACT string equality ('  openrouter (remote)  plenty 82% [measured]'), not as an absence of words, so a stray separator would fail it.
- It pins the shell overview's wording. The routing explanation's own hedged text is asserted in routing_burn's ranking test but has no separate mutation.

---

## REVIEW — the orchestrator owes an answer to each of these

This section is the point of the generator. Everything above is the
worker's facts, transcribed. Nothing below is decided.

- **1274** — verdict `closed`. Re-run one decisive mutation yourself, then rule (§79: a worker's packet does not bind the integrator).
- **1276** — verdict `closed`. Re-run one decisive mutation yourself, then rule (§79: a worker's packet does not bind the integrator).
- **1277** — verdict `closed`. Re-run one decisive mutation yourself, then rule (§79: a worker's packet does not bind the integrator).
- **1278** — verdict `closed`. Re-run one decisive mutation yourself, then rule (§79: a worker's packet does not bind the integrator).
- **1279** — verdict `closed`. Re-run one decisive mutation yourself, then rule (§79: a worker's packet does not bind the integrator).
- **1280** — verdict `closed`. Re-run one decisive mutation yourself, then rule (§79: a worker's packet does not bind the integrator).
- **1281** — verdict `closed`. Re-run one decisive mutation yourself, then rule (§79: a worker's packet does not bind the integrator).
- **1282** — verdict `closed`. Re-run one decisive mutation yourself, then rule (§79: a worker's packet does not bind the integrator).
- **1283** — verdict `closed`. Re-run one decisive mutation yourself, then rule (§79: a worker's packet does not bind the integrator).

**Packet errors the worker reported — read these BEFORE its results.**
Thirteen consecutive rounds a worker corrected its packet and was right:
- The packet's FEASIBILITY names EvidenceLedger::observations_in_window (evidence.rs:3055) as line 1276's consumer. That read filters `outcome IS NOT NULL`, and record_routing_latency -- the ONLY producer holding a RouterAnswer -- writes no outcome, so every row carrying a task class is invisible to it. Added EvidenceLedger::consumption_in_window rather than widening the existing read, which four classifiers depend on; pinned by routing_burn::the_outcome_filtered_read_does_not_see_an_unfinished_routing_row.
- The packet's FEASIBILITY says Capacity distinguishes four states; there are five -- DelegatedUpstream (quota.rs:477) is a fourth way of not knowing a remaining amount, and forecast() must answer None for it too.
- §81 line numbers that had moved: record_routing_latency is main.rs:4510, not :4454; score() is session.rs:4699, not :4643. Migration 18's three anchors, TaskClass, resource_capacity_line, recent_credential_throttles and Capacity were all correct.
- The packet's VERIFICATION COMMANDS do not reach session::store::the_project_database_schema_has_nowhere_to_put_a_credential, a census of EVERY column of every table that any migration breaks. --targeted caught it. A migration packet should name it.
- The migration ripple is larger than the packet's grep shape. Beyond `version, 22` and `SUPPORTED_SCHEMA_VERSION, 22`, tests/session_context.rs pins the version as a bare literal on its own line (`schema_version(&conn),` / `22,`). Swept every schema_version assertion in the crate by hand after --targeted caught it.

**Files touched outside EXPECTED FILES** — disclosed, not hidden:
- `crates/glasshouse/src/routing/request.rs` — TaskClass::from_stored and TaskClass::ALL. Reading the column back needs the inverse of as_str, and this codebase's stated rule (FAILURE_CLASSES' and migration 15's and 18's doc comments) is that a stored vocabulary lives in ONE place in Rust. Putting from_stored in evidence.rs would create a second spelling of the same five words that could drift. No existing caller's behaviour changed.
- `crates/glasshouse/src/routing/interactive.rs` — One RoutingObservation struct literal in its test module gains the new field -- the migration ripple the packet predicted.
- `crates/glasshouse/src/session/store.rs` — Migration ripple: two rollback blocks gain the task_class drop, two version pins bump, and the schema census gains routing_observations.task_class with the written reason that census requires of every column.
- `crates/glasshouse/tests/memory_store.rs` — Migration ripple only: one rollback block, one version pin.
- `crates/glasshouse/tests/memory_provenance.rs` — Migration ripple only: two rollback blocks, two version pins.
- `crates/glasshouse/tests/session_context.rs` — Migration ripple only: one rollback block, one bare-literal version pin the packet's grep shape did not match.
- `crates/glasshouse/tests/evaluation_observations.rs` — Migration ripple only: one rollback block, two version pins.
- `crates/glasshouse/tests/subscription_pressure.rs` — Its PressureInputs struct literal gains `forecast: None` -- and that literal is itself the proof the inert default is what a caller with no ledger carries.
- `crates/glasshouse/tests/launch_classification.rs` — 1276's mutation site (record_routing_latency) is on the LAUNCH path; practice §80 case 3 says a mutation whose site the test never reaches proves nothing. This is the only file that already drives a real launch and reads the ledger it wrote, so the killer went where the path is.

Gates the worker ran (re-run the decisive ones yourself):
- cargo check -p glasshouse --tests: clean
- cargo clippy -p glasshouse --all-targets --all-features -- -D warnings: clean, exit 0
- cargo fmt -p glasshouse: applied
- cargo doc -p glasshouse --no-deps: clean (two intra-doc links from public docs to private items in burn.rs were caught by --targeted's rustdoc leg and fixed)
- cargo test -p glasshouse --lib database::  -> test result: ok. 35 passed; 0 failed; 0 ignored; 0 measured; 1890 filtered out
- cargo test -p glasshouse --lib routing::burn  -> test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 1911 filtered out
- cargo test -p glasshouse --lib shell::  -> test result: ok. 299 passed; 0 failed; 0 ignored; 0 measured; 1626 filtered out
- cargo test -p glasshouse --lib session::store::  -> test result: ok. 69 passed; 0 failed; 0 ignored; 0 measured; 1856 filtered out
- cargo test -p glasshouse --test routing_burn  -> test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
- cargo test -p glasshouse --test routing_policy  -> test result: ok. 37 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
- cargo test -p glasshouse --test routing_cost  -> test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out (still 8/8)
- cargo test -p glasshouse --test launch_classification  -> test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
- cargo test -p glasshouse --test session_context  -> test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
- cargo test -p glasshouse --test subscription_pressure  -> test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
- scripts/blast-radius.sh --targeted <18 files>: exit 0, 'every traced target passed', 'rustdoc: clean', 18 targets green, 0 failures; it skipped 100 FULL-trace targets and says so -- the trailing sweep is the orchestrator's
- scripts/mutate.sh --script <9 mutations> --allow-dirty: 9 run, 9 KILLED, 0 SURVIVED; tree byte-identical afterwards (same 16 modified + 2 untracked files, and --lib routing::burn re-ran 14/14 on the restored source). mutate.sh prints a `test result:` line only on SURVIVED (its own §80 guard), so each KILLED is evidenced by its named failing test and quoted panic above.
- Two reds attributed as load flake, not regressions (§34, two runs each): database::tests::concurrent_first_bootstraps_serialize_on_one_database (3/3 alone, 2/2 in-family) and shell::shell_entitlement_scrub_tests::a_shell_started_native_session_does_not_carry_a_configured_entitlements_variable (3/3 alone, 2/2 in-family). Neither is in this diff.
- The pre-existing red the packet named (gateway::session::tests::observe_exchange_scores_a_real_failover_against_the_configured_preference) did NOT appear in any of the four --targeted runs. Not chased.
- co-edit §77: coedit.sh diff run once each on routing/session.rs, routing/mod.rs and main.rs at finalization. No peer hunk overlapped mine and nothing needed adapting; all three released with coedit.sh done, and main.rs's barrier is now OPEN for the orchestrator.

---

# 1276 — CLOSED 2026-09-02 (`GH-CLASS-RATE-SURFACE`, Green, Sonnet medium): the held line gets its caller

`build_project_overview_capacity` now calls `task_class_request_rates` over
the rows it already reads and prints one hedged line — *requests by task class
(recent, estimated)* — for every class with at least `MIN_ROWS_FOR_BURN_RATE`
live rows. Two KILLED mutations: drop the call, and drop the gate. The
no-forecast and empty-ledger lines are pinned byte-identical.

### Maintain a short moving average of requests consumed per task class. (line 1276)

Contract: Given routed requests recorded over a window, when a reader asks how much of each task class is being consumed, Glasshouse answers with a robust rolling per-class request rate built from the class each decision actually recorded, printing a class only when it has at least MIN_ROWS_FOR_BURN_RATE live rows, while naming no class for a row whose producer ran no classifier.

State: **COMPLETE** — ruled 2026-09-02. The reader's first production caller. The orchestrator's packet claimed the reader gated at `MIN_ROWS_FOR_BURN_RATE`; it does not, and the worker said so — so the gate lives at the surface, on the `rows` count `ClassRate` already carries, with its own KILLED mutation. A rate from one row is not something a person should read as a moving average.

Production evidence:
- `src/shell/mod.rs` — `build_project_overview_capacity`

Regression evidence:
- `shell::project_overview_capacity_tests::a_class_with_recent_rows_prints_a_hedged_line_and_an_absent_class_prints_nothing`
- `shell::project_overview_capacity_tests::a_class_below_the_minimum_row_count_does_not_print_even_though_the_reader_would_name_it`
- `shell::project_overview_capacity_tests::an_empty_ledger_prints_the_capacity_overview_byte_identical_to_before`
- `shell::project_overview_capacity_tests::a_resource_with_no_forecast_prints_exactly_what_it_printed_before`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| task_class_request_rates(rows, now_unix, None) call replaced with a hardcoded empty Vec<ClassRate> (push nothing) | `skip-state-update` | **killed** | `shell::project_overview_capacity_tests::a_class_with_recent_rows_prints_a_hedged_line_and_an_absent_class_prints_nothing` |
| .filter(|rate| rate.rows >= crate::routing::burn::MIN_ROWS_FOR_BURN_RATE) replaced with .filter(|_rate| true) | `remove-validation` | **killed** | `shell::project_overview_capacity_tests::a_class_below_the_minimum_row_count_does_not_print_even_though_the_reader_would_name_it` |

> skip-state-update observed: panicked at crates/glasshouse/src/shell/mod.rs:4543:32: no task-class line in []

> remove-validation observed: panicked at crates/glasshouse/src/shell/mod.rs:4612:9 (the !class_line.contains("investigation") assertion)

Recorded scope limits — stated by the worker, not discovered later:
- The MIN_ROWS_FOR_BURN_RATE gate on printed classes is enforced in shell/mod.rs, not in task_class_request_rates itself -- routing/burn.rs's reader still names a class from a single row; a future direct caller of the reader elsewhere would need its own gate.
- Does not re-prove task_class_request_rates's own median/windowing correctness -- that is burn.rs's own mutation suite, per the original packet.
- No Windows/Linux run for this change; the file is flagged platform-conditional for pre-existing unrelated code, not this diff.

---

## REVIEW — the orchestrator owes an answer to each of these

This section is the point of the generator. Everything above is the
worker's facts, transcribed. Nothing below is decided.

- **1276** — verdict `closed`. Re-run one decisive mutation yourself, then rule (§79: a worker's packet does not bind the integrator).

Gates the worker ran (re-run the decisive ones yourself):
- cargo test -p glasshouse --lib shell::project_overview_capacity_tests: ok. 14 passed; 0 failed
- cargo test -p glasshouse --lib shell::: ok. 302 passed; 0 failed
- cargo clippy -p glasshouse --all-targets --all-features -- -D warnings: clean
- scripts/blast-radius.sh --targeted crates/glasshouse/src/shell/mod.rs: every traced target passed


**Phase 32E stands at 9 of 10.** 1275 stays with register row P1b.
