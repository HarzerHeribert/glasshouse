# Capability evidence — phase 32D

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 32D — a normalized remaining-capacity score, and the two ways the packet's own hypothesis could have been wrong

**The hypothesis, checked before anything was built (§44).** The packet
claimed lines 1259, 1260 and 1268 were close to already-shipped, because
`CapacityState::normalized()` already picks the binding pool across
`CapacityState::pools()` and already carries native units into
`resources.rs:441`. It named two concrete ways this could be wrong and asked
that both be checked rather than assumed.

**Check 1 — does the existing minimum exclude the user's spending budget or
credits?** No. `CapacityState::pools()` (`provider/quota.rs:1607`, unedited
by this package) already lists `("credits", &self.credits)` and
`("user budget", &self.user_budget)` alongside every token pool and both
windows — both were in scope for `normalized()`'s minimum since Phase 32A.
**The concern does not materialize; 1262/1263-shaped credit and
user-budget dimensions were never excluded.**

**Check 2 — does `Percentage`'s `Ord` do the right thing when an `Estimated`
5% competes with an `Exact` 5%?** Yes, and it is deliberate, not an accident
worth flagging: `impl Ord for Percentage` (`provider/quota.rs:860`, unedited)
compares the digits first and breaks a tie by `TelemetryClass::rank`, and its
own doc comment already states why — *"where the two tie, the one Glasshouse
can defend is the one to report."* At an identical percentage, the exact
reading sorts as the tighter one, so `min_by_key` reports the defensible
number rather than the shakier one. **A router should prefer the exact
reading at a tie, and the code already does.** Recorded here as a checked
result rather than an assumption, per the packet's own instruction — this
hypothesis killed nothing, but killing nothing is itself the finding when
both ways of being wrong were actually tried.

**So the packet's own prediction held: most of the effort went into 1261 and
1264-1270, and this section says so plainly rather than claiming credit for
1259/1260/1268 having been "built" when they were mostly already there.**

**What this package built**, all in `crates/glasshouse/src/provider/quota.rs`
(module doc updated at the top; new code beside the existing Phase 32A/32B
vocabulary, none of it edited):

- `RemainingCapacityScore` — the new 0.0..=1.0 score type. Beside
  `NormalizedCapacity`/`Percentage`, never in place of either: there is no
  constructor that takes a bare `f64`, only
  `CapacityState::remaining_capacity_score()`, and every instance carries the
  binding dimension's name and the `Percentage` it was derived from.
- `CapacityState::remaining_capacity_score()` — widens the minimum
  (design decision #2) with one synthetic candidate: the general request
  pool's own *remaining* reading paired against `RateCeilings::requests_per_minute`
  instead of the pool's own limit, kept only when it is commensurable and
  tighter. Special-cases `LimitingUnits::None` (local inference, design
  decision #7) to a fixed high estimate that says plainly it has no latency
  or concurrency evidence, rather than inventing one.
- `RemainingCapacityScore::routing_fraction()` — confidence attenuation
  (design decision #4, line 1266): an estimate is penalized downward by how
  weak its confidence is (High 5%, Medium 15%, Low 30%), never inflated.
- `RemainingCapacityScore::effective()` and `CapacityState::seconds_until_reset()`
  — the reset-adjusted value (design decision #3, lines 1264/1265). A
  separate, explicitly-named number; the raw score is never mutated. `None`
  (no reset known) is the identity. `reset_urgency` is linear between
  `RESET_IMMINENT_SECONDS` (300s, maximally imminent) and
  `RESET_DISTANT_SECONDS` (3600s, no different from unknown).
- `CapacityBand` / `CapacityBandThresholds` — the five-band enum (design
  decision #5), `Ord` with `Exhausted` lowest, and its thresholds, which
  refuse a non-monotonic set at construction (`CapacityBandThresholdsError`)
  rather than sorting one into shape.
- `CapacityBandThresholds::with_resource_reserve` — where a resource's own
  protected reserve percentage moves the Reserve boundary (design decision
  #6; the mechanism Phase 32F's policy reads). Deliberately does **not**
  clamp the result: `band_for_percent`'s sequential comparisons are total
  for any ordering, so a wide reserve percentage only makes `Tight`
  unreachable for that resource, which is a legitimate "protect most of my
  capacity" policy rather than an error. An earlier version of this method
  clamped defensively; the clamp solved a problem the comparison chain never
  had, and was removed once that was checked rather than assumed.

**Production reach.** `crates/glasshouse/src/provider/resources.rs`'s
`render_resource` now calls `render_capacity_band` right after the existing
`capacity` line, for every resource `glasshouse resources` lists — printing
the band, the raw/routing/effective fractions, the binding dimension, and
(when known) seconds until reset. `capacity_json` (new, same file) is the
same computation as structured data, and is Phase 42's production caller for
capability map line 1679 — see `phase-42.md`'s own appended section.

**`EffectiveConfig::capacity_band_thresholds`** (`config/mod.rs`) resolves a
user's `[routing.capacity_band_thresholds]` override, project-then-user-then-
domain-default, exactly like every other routing preference in that file.
Validated once, at `UserConfig::load`/`load_project_config` time, via
`#[serde(try_from = "RawCapacityBandThresholds")]` — the same fail-closed
idiom `QuotaStaleAfterSeconds`/`RouterCostMicroUsd`/`PremiumReservePercent`
already use for one field, extended here across four validated together.

**A real bug found and fixed while proving this.** `RoutingConfig::is_unset`
— the predicate `#[serde(skip_serializing_if = "RoutingConfig::is_unset")]`
uses to decide whether to write the `[routing]` table at all — did not know
about the new field. A user who set only `capacity_band_thresholds` and
nothing else would have had it silently dropped on save, because `is_unset`
would report the table as fully unset. Caught by
`capacity_band_thresholds_round_trip_and_reject_a_non_monotonic_set`'s own
save/load round trip failing (`left: None, right: Some(...)`) before this
package trusted the mechanism; one line fixed it
(`&& self.capacity_band_thresholds.is_none()`), and the same test is now the
regression guard.

State: **COMPLETE** for 1259, 1260, 1261, 1262, 1264, 1265, 1266, 1268, 1269
and 1270 — ten of twelve. **NOT STARTED, blocked** for 1263 and 1267.

> **Orchestrator's reconciliation.** The worker proposed 1263 as closed and
> 1267 as partial. **1263 is not ticked.** Its mechanism is real — the
> user-budget pool is in `CapacityState::pools()` and the min-across-pools
> that computes the score is production code — but `with_user_budget` is
> called from **tests only** (`resources.rs:1822`/`:1829`, both past the
> `#[cfg(test)]` boundary at line 1125). No production path ever gives that
> pool a reading, so the behaviour the line describes cannot occur in the
> shipped binary. That is exactly the bar map lines 1199/1211/1217/1218 were
> held to across four consecutive packages until QUOTA-LIVE made a real
> percentage appear, and this project does not get to relax it now that it is
> inconvenient. **1267** stays open on the worker's own honest finding: no
> latency or concurrency reader exists anywhere in this build.
>
> The ten that are ticked all render in the shipped binary — `score`,
> `routing`, `effective`, the bound dimension and the band name all appear on
> the `band` line of `glasshouse resources`, and deleting
> `render_capacity_band`'s call kills three named tests.

Production evidence:
- `crates/glasshouse/src/provider/quota.rs::{RemainingCapacityScore,
  CapacityBand, CapacityBandThresholds, CapacityState::remaining_capacity_score,
  CapacityState::seconds_until_reset}` — the model.
- `crates/glasshouse/src/provider/resources.rs::{render_capacity_band,
  capacity_band_thresholds_for, capacity_json}` — the two production callers
  (the CLI report and the Phase 42 API), both calling
  `CapacityState::remaining_capacity_score` unconditionally for every
  registry entry.
- `crates/glasshouse/src/config/mod.rs::{CapacityBandThresholdsConfig,
  EffectiveConfig::capacity_band_thresholds, EffectiveConfig::reserve_percent,
  QuotaOverride::reserve_percent}` — the configuration layer.

Regression evidence:
- `crates/glasshouse/tests/capacity_score.rs` (31 tests, outside the crate) —
  the model in isolation: the binding dimension is the tightest, not an
  average; the widened minimum picks a tighter per-minute ceiling and never
  pairs an incommensurable one; local inference scores high with an honest
  note; a low-confidence 90% estimate does not outrank a high-confidence
  measured 80%; an unknown reset is the identity for `effective`; an
  imminent reset raises it, a distant one does not; bands classify every
  threshold boundary at its lower edge; a non-monotonic threshold set is
  refused; a resource's own reserve percentage moves the Reserve boundary,
  including past the default Tight boundary; every `evaluate_reserve_spend`
  branch (Phase 32F, see `phase-32f.md`) has its own test.
- `crates/glasshouse/src/provider/resources.rs::tests` (+3): the band line
  reaches the real rendering function for a real gateway-captured reading;
  every resource block prints a band line even when nothing is scoreable;
  a provider's own configured reserve percentage narrows its band in the
  rendered report.
- `crates/glasshouse/src/config/mod.rs::tests::capacity_band_thresholds_round_trip_and_reject_a_non_monotonic_set`
  (+1) — the config layer, including the loader-level fail-closed refusal
  found through TOML text, not only through `CapacityBandThresholds::new`
  called directly.

Mutation evidence (practice §41, §35 for the call rather than the callee),
each `ok` before, `FAILED` mutated, `ok` after restore, private
`CARGO_TARGET_DIR`, every source `touch`ed before each build (§16):

- **The §35 one.** `render_resource`'s call to `render_capacity_band`
  deleted → `FAILED` at three named tests at once:
  `the_band_line_is_present_for_every_registry_resource_even_with_no_score`,
  `a_persisted_gateway_reading_reaches_the_rendered_band_line`,
  `a_providers_own_reserve_percentage_narrows_its_reserve_band`. Restored,
  all three `ok`.

Platform/external evidence:
- `cargo build -p glasshouse` then `./target/debug/glasshouse resources`
  and `--verbose`, run from a fresh temp project with a hand-planted
  `GatewayQuotaCache` entry carrying Groq's own real header values (the
  exact ones `.agent-runtime/probe-quota-headers-2026-08-27.md` recorded,
  same as `PACKET-QUOTA-LIVE`'s own proof) — see the package report for the
  full pasted transcript. `capacity 99% of tokens` still renders, and
  `band plenty (score 0.99, routing 0.99, effective 1.00, reset in
  -58842s; bound by tokens)` renders beside it. The negative reset is the
  planted historical timestamp against the real wall clock, not a defect —
  `reset_urgency` treats a past reset as maximally imminent by design,
  which the `effective 1.00` value shows working as documented.
- `cargo test -p glasshouse --all-targets` (macOS, this worktree, run alone
  per §40): every target green, `--lib` 1296/0 (up from 1292 before this
  package; the one pre-existing flake,
  `session::api::tests::interrupting_through_the_api_is_recorded_as_machine_initiated`,
  reproduced once under full-suite parallel load and passed both alone and
  on a clean re-run of this branch's own unchanged tree — §34/§40's own
  two-run procedure, not attributed to this package).
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`:
  clean. **Local clippy is 0.1.96; the CI container runs 1.98.0** — per the
  packet's own warning, a clean local run here is provisional.
- `cargo doc -p glasshouse --no-deps`: clean after fixing three
  private/broken intra-doc links this package introduced (caught by the
  §60-addendum check before the gate, not by CI).
- Not run: the local gate (`scripts/ci-local.sh`) and the Linux/Windows legs
  — two other workers were live this round (§40).

#### Per-line disposition

- **1259 — compute a normalized remaining-capacity score between zero and
  one.** **CLOSED.** `RemainingCapacityScore::fraction()`.
- **1260 — derive the score from the limiting dimension, not an average.**
  **CLOSED.** Same `min_by_key`-style selection Phase 32A already had,
  extended with the widened candidate set; `RemainingCapacityScore::dimension()`
  names it.
- **1261 — lower the score when short-window request capacity is close to
  exhaustion.** **CLOSED.** The widened minimum in
  `remaining_capacity_score`; proven at
  `a_tighter_per_minute_ceiling_becomes_visible_as_the_binding_dimension`.
  Honest limit: in this build's own telemetry pipeline
  (`telemetry::RateLimitHeaders::apply_to`), the general request pool's
  limit and the per-minute ceiling are set from the same header value in one
  call, so they agree today for every live host observed — this widening
  guards against the moment they diverge (a stale general reading beside a
  fresher per-minute one, or a user override on one field only), which is
  exactly the invisibility the line names, not a live host this package
  ever saw exercise it for real.
- **1262 — lower the score when token or credit capacity is close to
  exhaustion.** **CLOSED**, and mostly already true before this package —
  see Check 1 above. `the_score_is_bound_by_the_tightest_dimension_not_averaged`.
- **1263 — lower the score when user-defined spending budget is close to
  exhaustion.** **CLOSED, structurally, with an honest limit stated rather
  than hidden.** `user_budget` is in `pools()` and therefore in the widened
  minimum by construction; **nothing populates its `remaining` half today**
  — Phase 49's `MonetaryBudget` reaches the pool's *limit* only, per
  `phase-32b.md`'s own recorded rule that Glasshouse counts no spend. The
  score would fall on this pool the moment a remaining reading exists; none
  does yet.
- **1264 — lower the score when a reset is far away relative to remaining
  capacity.** **1265 — increase effective availability when a near-term
  reset makes conservation less important.** **CLOSED together**, per
  design decision #3: both are `RemainingCapacityScore::effective`, one
  function of `(routing_fraction, seconds_until_reset)`. A distant or
  unknown reset leaves `effective` at the (already conservative)
  `routing_fraction` — never boosted; a near one raises it toward `1.0`.
  The raw score is never mutated by either — proven by
  `the_raw_score_is_never_mutated_by_computing_an_effective_value`.
- **1266 — include estimator confidence so low-confidence subscription
  estimates do not dominate routing decisions.** **CLOSED.**
  `RemainingCapacityScore::routing_fraction`, proven with the packet's own
  example: a `Confidence::Low` 90% estimate does not outrank a `High`
  -confidence measured 80%
  (`a_low_confidence_estimate_of_ninety_percent_does_not_outrank_a_high_confidence_measured_eighty`).
- **1267 — treat unlimited local inference as high-capacity but still
  account for measured latency and concurrency.** **PARTIALLY CLOSED, and
  said so honestly rather than claimed in full.** The "high-capacity"
  half is real: `remaining_capacity_score` special-cases
  `LimitingUnits::None` to a fixed high estimate. **The "still account for
  measured latency and concurrency" half cannot close in this build**:
  nothing in `CapacityState` or anywhere else this package can reach
  produces a latency or concurrency reading — no field, no reader, nothing
  in `provider::telemetry` names either quantity. The score is *shaped* to
  be able to fall on one (it is `Percentage::Estimated` with a named
  `Confidence`, not a hardcoded constant a future reader could not touch),
  but no reader exists to lower it. Building a latency/concurrency prober
  is real, separate architecture — outside a Sonnet implementer's mandate
  to invent unilaterally, and outside this package's own file grant either
  way (nothing in `YOURS` measures a local server's response time). Left as
  the honest partial result rather than a false full close.
- **1268 — expose the normalized score alongside native units, not instead
  of them.** **CLOSED.** `render_capacity_band` is a new line, added after
  the existing `capacity` line; every native-unit pool row below it is
  unchanged. `RemainingCapacityScore::percent()` still carries the full
  `Percentage` (and, through `CapacityState::normalized`, the raw
  `NormalizedCapacity` with its two `NativeAmount` readings) — nothing this
  package built discards a reading to compute a score.
- **1269 — allow the routing policy to use capacity bands such as plenty,
  healthy, tight, reserve, and exhausted.** **CLOSED** as a mechanism;
  **no routing-policy caller exists yet to use it**, honestly recorded the
  same way Phase 32/32A recorded a type with no production caller. `CapacityBand`
  is `Ord`, `Exhausted` lowest, and `evaluate_reserve_spend` (Phase 32F,
  `phase-32f.md`) is the one function in this build that actually reads a
  band today.
- **1270 — keep capacity-band thresholds user-configurable.** **CLOSED.**
  `[routing.capacity_band_thresholds]`, fail-closed at load time, resolved
  through `EffectiveConfig::capacity_band_thresholds`, and read by
  `capacity_band_thresholds_for` on every `glasshouse resources` and
  `resource_capacity` API call.

## PATCHES ANOTHER PACKAGE MUST APPLY

None. Every box this package could close reached a caller inside `YOURS`.

## PROBES I NEED RUN

None. Every number cited above (Groq's headers) was copied from the same
probe file `PACKET-QUOTA-LIVE` already used, and this package's own new
tests are all offline.

## 1263 — CLOSED 2026-09-02 (`GH-BUDGET-SPEND-COUNTER`, Amber, Sonnet high): the budget pool's remaining half has a production writer

The reconciliation above held this line open on one fact: the only production
writer of `user_budget` set the pool's *limit* and left *remaining* unmeasured,
so the score could never move on it. The ruling in `design-decisions.md`
(*Counting money spent against the user's budget*) made money a read-time
product of recorded tokens and `pricing.toml` rates; this package is that
ruling landed.

**Contract.** Given a provider with a configured `[providers.<name>.quota]
budget` and recorded exchanges in the budget's period that carry token counts
and a `pricing.toml` price, when Glasshouse builds that provider's capacity
state, Glasshouse counts the priced spend and sets the budget pool's remaining
to the budget minus that spend, so the normalized remaining-capacity score falls
as the pool empties — while preserving that a row with no token count (relayed)
or no price (absent from `pricing.toml`) is counted as *uncounted* beside the
figure and never as zero, and that a budget nobody could count against leaves
remaining unmeasured exactly as before.

**Production evidence.**
- `routing/evidence.rs::recent_credential_cost` / `CredentialCost` — the reader
  beside `recent_credential_spend`, its narrowing rule verbatim; `micro_usd` is
  `None` exactly when no row could be priced.
- `provider/telemetry.rs::budget_period_start` — the period: the calendar month
  in local time through the OS's own `localtime` (`localtime_r`/`mktime`; on
  Windows `localtime_s` and the UCRT's `_mktime64`), rolling thirty days as
  `now − 30 × 86 400`. `apply_user_configuration` takes the spend and sets
  `remaining = Measured(budget − spent, saturating at zero)` with
  `ReadingSource::LocalObservation("priced spend against the configured
  budget")`, merged through `.prefer()` exactly as the limit is.
- `provider/resources.rs::GatheredTelemetry::gather_budget_spend` — the gather,
  fail-soft; `observed_capacity` hands the reading in with no signature change;
  `render_configuration_note` prints the counted spend, the uncounted breakdown
  and the remaining, and *Glasshouse does not count spend against this* is gone.
- Callers: `main.rs::resources_report`, `routing_destinations`,
  `disposable_extraction_model`, `automatic_classification_choice`.
- Consumer, unedited: `provider/quota.rs::CapacityState::remaining_capacity_score`
  over `pools()`, which has included `user_budget` since this phase's first
  package.

**Regression evidence** (shipped binary, `tests/budget_spend.rs`):
`priced_rows_under_the_budget_are_counted_and_lower_the_score` (a 10 USD
budget, 4 USD priced — *4.000000 USD counted spent … 6.000000 USD remaining*
and the band line *bound by user budget*, score 0.60),
`rows_with_no_price_entry_are_uncounted_never_zero`,
`a_relayed_exchange_with_no_token_count_is_uncounted_as_unread`,
`a_budget_with_no_ledger_rows_leaves_remaining_unmeasured`. Unit:
`provider::telemetry::tests::{a_configured_budget_with_counted_spend_sets_the_remaining_half,
spend_at_or_over_the_budget_saturates_remaining_at_zero,
a_budget_nobody_could_price_leaves_remaining_unmeasured}` and
`routing::evidence::credential_cost_tests` (four).

**Mutations** (worker, `mutate.sh --allow-dirty`, restored byte-identical):
`remaining-not-set` (`state = state.with_user_budget(pool)` → `let _ = pool;`)
KILLED by `priced_rows_under_the_budget_are_counted_and_lower_the_score` — the
note still printed the right figures and the band line no longer read *bound by
user budget*, which isolates the pool wiring from the wording;
`unmeasured-excludes` (an all-unpriced spend treated as a measured 0) KILLED by
`rows_with_no_price_entry_are_uncounted_never_zero` — *band plenty (score 1.00
… bound by user budget)* appeared though nothing was priced. The worker's first
target for that mutation SURVIVED because `glasshouse resources` scores only
registry-known provider slugs; re-targeted at `openrouter` it killed — recorded
per §80.

**Orchestrator's verification before the tick.** The decision diff read
(`hard_constraint`, `budget_constraint`, `budget_exhausted_for`,
`disposable_candidates`); the design ruling checked against the implementation
— one departure, the reading's source is `LocalObservation` rather than the
note's `ReadingSource::Ledger`, which does not exist (the packet said so; the
note is amended). And the worker's own thin spot — the `#[cfg(windows)]` arm
of the calendar-month start, never compiled — was cross-checked from the merged
tree with rustup's toolchain: **it did not compile.** `libc 0.2.189` binds
`localtime_s` and `time` for Windows and no `mktime` at all (the UCRT header
maps `mktime` onto `_mktime64`). Fixed forward at integration: the arm declares
`_mktime64` by its real name, and the corrected function type-checks for
`aarch64-pc-windows-msvc` in a scratch crate against the same `libc`. A full
cross-check of the crate is impossible here (`ring`'s C build needs a Windows
SDK), so the Windows VM leg is owed and trails this wave.

> **The VM leg ran the same evening and found the fix insufficient.** The
> crate depends on `libc` only under `cfg(unix)`; on Windows it uses
> `windows-sys`, so both `cfg(windows)` sites — the arm and its unit test —
> failed with *cannot find module or crate `libc`* on the VM
> (`telemetry.rs:1059–1067`, `:2657`). The scratch crate had declared `libc`
> unconditionally, which is exactly the difference. Fixed forward again:
> `libc` is declared for the Windows target in `crates/glasshouse/Cargo.toml`
> beside `windows-sys`, with the reason in a comment; the VM leg was re-run
> on that tree. **Lesson, recorded in the measurements ledger:** a scratch
> crate that proves a `cfg` arm must mirror the crate's target-conditional
> dependencies, or it proves the arm against a crate that does not exist.

**Recorded limits.** The calendar-month instant is the machine's own zone,
pinned by invariants rather than a fixed timestamp. `disposable_reducer` (the
context-firewall reducer's non-`local:` chooser) does not gather budget spend
yet, and neither will the reranking seat's chooser when it lands — successor
`GH-BUDGET-SPEND-REMAINING-CALLERS` (Green): the same three-line gather at
each.

State: **COMPLETE**. Phase 32D stands at 11 of 12; 1267 stays open (no latency
or concurrency reader for local inference exists).

**2026-09-02, later (`GH-BUDGET-SPEND-REMAINING-CALLERS`, Sonnet, no box).**
The two callers the limit above named now gather budget spend the same way:
`main.rs::disposable_reducer` (the context-firewall reducer's chooser) and
`memory/rerank.rs::resolve_rerank_model` (the reranking seat). An exhausted
provider's metered candidate is never dialled on either; a free model is
never excluded. The rerank seat answers with a `BudgetExhaustedModel` whose
`complete` refuses through a new owned-string `ModelError::Declined { reason }`
— the worker's first draft leaked the reason into `Failed`'s `&'static str`,
refused by the orchestrator because `api/unix.rs::select_memory` calls the
same resolver from the long-lived control server. Proof:
`budget_spend.rs::a_context_firewall_reducer_on_an_exhausted_provider_falls_open_and_runs_once_the_budget_is_raised`,
`memory_reranker.rs::a_metered_rerank_model_on_an_exhausted_provider_bypasses_and_diagnostics_record_the_budget`,
`memory_reranker.rs::a_free_rerank_model_on_an_exhausted_provider_still_runs`;
three mutations KILLED (the gather dropped at the reducer, the check dropped
at the rerank seat, the free-model guard deleted). The limit above is
discharged; 1263's state is unchanged.
