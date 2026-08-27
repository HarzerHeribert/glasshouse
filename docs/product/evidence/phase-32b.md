# Capability evidence — phase 32B

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 32B — quota telemetry, and the caller that makes Phase 32 real

**The finding first, because it decides four of the fourteen lines.** The
packet's two hypotheses were tested before anything was built, and both came
back partly false. The shape of this package follows from the halves that
survived.

**H1 — "the endpoints Glasshouse already calls carry usable rate-limit
headers."** Partly true, on **one host of eight**, and the half that arrives is
not the half the phase most wants. Unauthenticated `GET <base>/models` was run
on 2026-08-27 against every shipped template with a reachable host —
OpenRouter, UnoRouter, AnyRouter, Kilo, Nous, z.ai, NVIDIA and opencode-zen.
Seven sent **no rate-limit header of any name**. AnyRouter sent:

    ratelimit-limit: 300
    ratelimit-policy: 300;w=60
    x-ratelimit-limit: 300
    x-ratelimit-tier: ip
    x-ratelimit-window: 60

The **ceiling and the window arrive; the remaining count does not** — although
the same response's `access-control-expose-headers` advertises
`RateLimit-Remaining` and `RateLimit-Reset`. Re-run with a cache-buster and
`Cache-Control: no-cache` (the first response was a Cloudflare `HIT`, which
would have been the obvious explanation) and the second response carried no
`cf-cache-status` at all and still no remaining count. So the absence is the
host's behaviour on this route, not a cache artefact.

That is why `RateLimitHeaders::apply_to` fills a pool's **limit** and leaves its
**remaining** half `Capacity::Unmeasured`, and why `glasshouse resources` shows
`capacity unknown` for AnyRouter even with a live reading in hand: a percentage
needs both halves, and the provider published one.

**H2 — "a harness exposes machine-readable usage."** **False as stated, true in
its weaker form**, checked against the installed binaries rather than recalled:

| harness | machine-readable interface | usage/quota content |
|---|---|---|
| Claude Code | `claude auth status --json` (`--json` documented as the default) | a plan (`subscriptionType`), no usage figure |
| Codex | `codex doctor --json`, stamped `"schemaVersion": 1` | **none** — 23 checks, zero usage/quota/limit/credit/remaining/reset/window/balance fields |
| Antigravity (`agy`) | none in `--help` | — |
| Cursor CLI | none in `--help` | — |

So line 1231's *"or status information"* clause is satisfiable and its
*"usage"* clause is not. `codex doctor --json` is genuinely stable and
machine-readable and is **deliberately not parsed**, because parsing it would
produce nothing — recorded here so the absence reads as a checked result rather
than an oversight.

**The caller.** `glasshouse resources` (`crates/glasshouse/src/cli.rs`,
`crates/glasshouse/src/main.rs::resources_report`,
`crates/glasshouse/src/provider/resources.rs`), modelled on `glasshouse
pairing` and `glasshouse response`. It is what Phase 32's ledger said was
missing — *"Nothing in the shipped binary currently prints 'here is everything
Glasshouse can describe' to a user"* — and what Phase 32A's said one layer
down. Every telemetry reader this phase builds is reached from that one arm.

State: **COMPLETE** for lines 1227, 1228, 1231, 1232, 1233, 1234, 1235,
1236, 1237, 1238 and 1240, and for map line 1761. **OPEN** for lines 1229, 1230
and 1239 — see the per-line breakdown, which names what each is waiting on.

**Line 1229 was moved to OPEN by the orchestrator, over this package's own
COMPLETE, and the package asked for that decision rather than taking it.** The
line reads *"from API **and gateway** responses"*. The API half is built,
proven and reached from `glasshouse resources --probe`; the gateway half is not
built. Applying the criterion in practice §33 — ask the capability as a question
a user would ask — *"does Glasshouse read rate-limit headers from API and
gateway responses?"* is answered "from API responses only", which is not a yes.
A ticked box stops being scheduled, and the gateway half would then never be
written.

The packet's design note D2 forbade this package the gateway path, citing Phase
9I's *"a parser there would make it a reader of the payload it exists to pass
through."* **That was the orchestrator's error and is withdrawn: reading a
response header is not reading the payload.** The gateway already parses the
header block in order to forward it; the body is what it streams untouched. The
worker declined to reverse a decision it was told not to reverse, which was
correct. The gateway half is now a small, unblocked follow-up.

Production evidence:
- `crates/glasshouse/src/main.rs::resources_report` — the production entry
  point. The `Command::Resources` arm is the only thing in the shipped binary
  that reads the capacity model past its quota shape.
- `crates/glasshouse/src/provider/resources.rs::report` and
  `::observed_capacity` — the fold: user configuration, then the harness's own
  report, then the provider's own headers, weakest source first.
- `crates/glasshouse/src/provider/discovery.rs::connectivity_with_headers` and
  `::rate_limit_headers_of` — D2's seam. `connectivity` now delegates to it and
  answers exactly what it always did, so no existing caller changed.
- `crates/glasshouse/src/provider/telemetry.rs` — `RateLimitHeaders`,
  `HarnessTelemetry`, `read_harness_plan`, `apply_provider_headers`,
  `apply_harness_report`, `apply_user_configuration`, `RATE_LIMIT_HEADERS`.
- `crates/glasshouse/src/provider/quota.rs` — `TelemetryClass`,
  `ReadingSource::{LocalObservation, InferredEstimate}` and the total
  `ReadingSource::class`, `Percentage`, `Confidence`, `Freshness`, `KnownPlan`,
  `Capacity::{telemetry_class, telemetry_class_str, describe_source, prefer}`,
  `CapacityState::{plan, last_observed_at_unix, telemetry_class}`.
- `crates/glasshouse/src/config/mod.rs` — `QuotaOverride`, `MonetaryBudget`,
  `BudgetPeriod`, `QuotaStaleAfterSeconds`, `ProviderConfig::quota`,
  `EffectiveConfig::{quota_override, quota_stale_after}`.
- `crates/glasshouse/src/provider/mod.rs` — OpenRouter's `usage_telemetry`
  promoted from `Unverified` to `Verified`, with a same-host control.

Regression evidence:
- `provider::quota::tests::*` (18 new) — the class mapping is total over every
  origin; the four unknown states have no class and render `unknown`;
  authoritative outranks every other class in both directions; the fresher of
  two same-class readings wins; an unknown never displaces a measurement; an
  exact percentage and an estimate at the same figure never render alike; the
  weakest reading decides an estimate's confidence; a subscription produces no
  percentage at all; the last observation is the latest reading anywhere in the
  state; the same reading is fresh under one configured age and stale under
  another.
- `provider::telemetry::tests::*` (22) — the header set AnyRouter really sent;
  a response with no rate-limit header reads as nothing rather than zero; a
  ceiling over a minute and one over an hour land in different fields; a
  ceiling with no stated window becomes no rate at all; the IETF spelling wins
  over the `x-` one; every allowlisted name has a field to land in.
- `provider::discovery::tests::*` (6 new) — the capture proven **through
  `connectivity_with_headers`** against a fixture serving the real header
  block, including a `429` carrying `Retry-After` and an unreachable endpoint.
- `provider::resources::tests::*` (16) — the report itself, including two
  providers with different configured ages disagreeing about the same reading.
- `crates/glasshouse/tests/provider_discovery.rs` (13 new) — the same model
  from outside the crate, plus **four tests that drive the shipped binary**.

Failure/isolation evidence:
- `telemetry::tests::a_source_description_is_built_only_from_names_glasshouse_chose`
  — header values shaped like the real ones `design-decisions.md` was written
  from (an account identifier, a masked credential tail, a `__cf_bm` cookie)
  are fed through the whole reader and none reaches any rendered string.
- `discovery::tests::nothing_but_an_allowlisted_header_survives_the_capture` —
  the same rule at the wire boundary. OpenRouter's `GET /api/v1/models`
  response really does carry `set-cookie: __cf_bm=…`, measured 2026-08-27, so a
  capture that kept "the response headers" would put a session cookie into a
  report a user is invited to share. `RATE_LIMIT_HEADERS` is an allowlist for
  that reason and not a filter for things that look interesting.
- `telemetry::tests::a_harness_report_carries_nothing_but_the_plan` and
  `tests/provider_discovery.rs::a_harness_status_body_yields_a_plan_and_nothing_else_about_the_account`
  — `claude auth status --json` emits eight keys of which **three identify the
  account holder**. The reader takes `subscriptionType` and there is no
  representation of the rest inside Glasshouse for a later change to start
  printing.
- `telemetry::tests::a_reader_cannot_fill_in_a_pool_the_provider_publishes_nothing_for`
  — Phase 32A called `is_readable()` its best property; this is the first
  reader with the opportunity to break it, and it does not.
- `tests/provider_discovery.rs::no_telemetry_reader_can_hand_a_caller_an_error_to_fail_a_session_on`
  — garbage in every position yields the exact state a resource with no
  telemetry has. **No function in the telemetry path returns a `Result`**, so
  line 1238 is a property of the signatures rather than of caller discipline.
- `tests/provider_discovery.rs::nothing_the_registry_can_describe_claims_a_telemetry_class_it_did_not_earn`
  — Phase 32A's standing guard, extended to the class as well as the value.
- `resources::tests::a_report_with_no_telemetry_shows_no_capacity_figure_at_all`
  — with nothing measured, the character `%` does not appear anywhere in the
  report, and every resource says `last observed never`.

Mutation evidence (practice §41, and §35 for the call rather than the callee) —
**19 mutations, 19 killed**, each `ok` before, `FAILED` mutated, `ok` after
restore, in a private `CARGO_TARGET_DIR` with every source `touch`ed before
each build (§16):

- **M1, the §35 one.** `main.rs`'s `Command::Resources` arm stops printing
  (`let _ = resources_report(…)?`) → `FAILED` at
  `the_shipped_binary_reports_every_resource_it_can_describe`. The production
  call, not the callee.
- **M16, the second §35 one.** `observed_capacity` stops reading the user's
  overrides → `FAILED` at `the_shipped_binary_reads_a_users_own_quota_overrides`.
- **M20, the third.** The report hardcodes `telemetry measured` → `FAILED` at
  `the_shipped_binary_names_the_telemetry_source_of_every_resource`.
- M2 `UserConfiguration` claims `Authoritative` → `FAILED`.
- M4 (D4) every percentage returns `Percentage::Exact` → `FAILED` at
  `one_non_authoritative_reading_makes_the_whole_percentage_an_estimate`.
- M5 (D4) an estimate renders as a bare figure → `FAILED` at
  `an_estimate_and_an_exact_reading_at_the_same_figure_never_render_alike`.
- M6 (D5) the header allowlist becomes a pass-through → `FAILED`.
- M7 (D5) the plan reader takes `email` instead of `subscriptionType` → `FAILED`.
- M8 an hourly ceiling is filed as a per-minute one → `FAILED`.
- M9 the reader drops its `is_readable` guard and fills an opaque pool → `FAILED`.
- M11 the preference order is inverted → `FAILED`.
- M12 nothing is ever stale → `FAILED`.
- M13 the *earliest* observation is reported as the last → `FAILED`.
- M14 the staleness age stops being provider-specific → `FAILED`.
- M15 the project layer's quota table stops winning over the user's → `FAILED`.
- M17 `discovery.rs` goes back to discarding response headers → `FAILED`.
- M18 (D5) the capture keeps every header, cookie included → `FAILED`.
- M19 the policy's quota figure is read as its window → `FAILED`.

**Two mutations needed a second attempt, and both are §41's own lesson.**

**M10 SURVIVED as first aimed, and the mutation was weak, not the test.**
`Capacity::prefer`'s `theirs.rank() < mine.rank()` mutated to `<=` was run
against `an_authoritative_reading_outranks_every_other_class_in_both_directions`
— a test using two *different* classes, where `<=` and `<` cannot differ. The
mutation only changes the *same-class* path. Re-aimed at
`between_two_readings_of_one_class_the_fresher_one_wins`, which is the test
that covers that path, it `FAILED` immediately (M10a). The genuinely
order-inverting mutation for the different-class path is M11, which also
`FAILED`. Recorded because the first result would read as a coverage hole and
was not one.

**M17 SURVIVED as first aimed, and that one was a real gap in this package's
own tests.** `rate_limit_headers_of` returning `Vec::new()` — deleting the
whole D2 capture — broke nothing, because every header test in
`provider::telemetry` and `tests/provider_discovery.rs` constructs
`RateLimitHeaders::read(…)` directly, which is *below* the function the binary
calls. That is practice §35 exactly, found in my own work by the check the
packet asked for. Six tests were added in `provider::discovery` that go through
`connectivity_with_headers` against a `FixtureProvider` serving AnyRouter's
real header block; M17 and M18 then both `FAILED`.

Platform/external evidence:
- `cargo test -p glasshouse` (macOS, this worktree, run alone per practice
  §40): every target green — `--lib` 1244 passed / 0 failed,
  `provider_discovery` 29 / 0, `pty_smoke` 71 / 0.
- `cargo clippy -p glasshouse --all-targets -- -D warnings`: clean.
- `cargo doc -p glasshouse --no-deps` (practice §60 addendum): clean. It earned
  its two seconds — it caught a public doc comment on `gather_harness_status`
  linking to the private `HARNESS_STATUS_ARGS`.
- `cargo fmt -p glasshouse -- --check`: clean. `rustfmt` was run on this
  package's own files only, never `cargo fmt --all` (§37).
- **The shipped binary, run for real**, against a temporary config and project
  directory: `glasshouse resources` reports Claude Code's plan as
  `max [authoritative] from the harness interface 'claude auth status --json'`
  with a dated observation, and `glasshouse resources --probe anyrouter`
  reports `limit 300 requests [authoritative] from the 'ratelimit-limit'
  response header` and `requests/minute 300 requests` beside
  `remaining unmeasured (unknown)`.
- Live probes, 2026-08-27, unauthenticated, no credential used: eight
  `GET <base>/models`; three OpenRouter paths with a same-host `404` control;
  one cache-busted AnyRouter re-probe.
- **Not run:** the local gate (`scripts/ci-local.sh`), so the Linux and Windows
  legs and the MSRV job are unproven for this change. One thing here is
  genuinely platform-shaped and worth the orchestrator's attention:
  `read_harness_status` spawns a child process. It is guarded by
  `ResolvedExecutable` resolution and captures rather than inherits its
  streams, and no test invokes it (every binary-level test passes
  `--no-harness`), but that is an argument, not a run.

Missing evidence:
- No authenticated read of a provider usage endpoint — line 1230. See below.
- No test drives `read_harness_status`'s subprocess. Deliberate: a test that
  ran a developer's real `claude auth status` would observe their account and
  would fail on a machine without the harness installed. The parser and the
  merge are tested with a report; only the spawn is not.

---

## Per-line disposition

The criterion is practice §33's: *ask the box as a question a user would ask,
and see whether the honest answer is yes in the shipped binary.*

**1227 — Define quota telemetry sources as authoritative, observed, estimated,
manual, or unknown.** **CLOSED.** `TelemetryClass` names the four classes and
`ReadingSource::class` is total over all six origins, including the two this
phase added (`LocalObservation`, `InferredEstimate`). The fifth term is the
absence of a reading: `Capacity::telemetry_class` answers `Option`, and
`telemetry_class_str` renders `UNKNOWN_TELEMETRY` for the four non-`Measured`
states — D1's rule kept rather than weakened. Reaches production in every row
of `glasshouse resources`. M2 and M20.

**1228 — Prefer authoritative provider or harness usage telemetry when it is
available.** **CLOSED.** `Capacity::prefer`, ordered by `TelemetryClass::rank`,
with ties broken by freshness. Exercised for real: a harness-reported plan
displaces a user-configured one, proven at the report level by
`a_providers_own_header_outranks_the_plan_a_user_typed_for_the_same_provider`.
M10a and M11.

**1229 — Read rate-limit and usage headers from API and gateway responses when
the provider exposes them.** **CLOSED for API responses, on one provider.**
`discovery.rs::connectivity_with_headers` captures, `RateLimitHeaders` parses,
`glasshouse resources --probe` shows it. Proven against a real host's real
header block and through the shipped binary. **The gateway half is deliberately
not built** — D2, and Phase 9I line 528's decision that the gateway forwards
headers without reading them, which this package did not reverse. The line's
own wording covers both and the API half is what a user can exercise; the
orchestrator should decide whether the gateway clause is satisfied or whether
this line stays open. M9, M17, M18.

**1230 — Read provider usage endpoints when they are documented and can be
queried without excessive request cost.** **OPEN.** The prerequisite is
established and the reading is not. `GET https://openrouter.ai/api/v1/key` and
`/api/v1/credits` both answer `401` with a JSON error envelope while
`/api/v1/glasshouse-nonexistent-control` answers `404` on the same host in the
same minute — so the routes exist, gate on authentication, and cost one request
with no inference. That is now `Provider::usage_telemetry` = `Verified` for
OpenRouter, with the control named in the evidence string (practice §23: a
control has to be run against the host it is being used to justify). **What has
not been read is a success body**, because that needs the user's own key, so
**no parser for one exists and none was invented**. The verb in the line is
*read*; it is not satisfied. Waiting on one authenticated probe — see the
report's `PROBES I NEED RUN`.

**1231 — Read native harness usage or status information when a stable
machine-readable interface exists.** **CLOSED, on the "status" clause.**
`read_harness_status` runs `claude auth status --json` and
`read_harness_plan` takes its `subscriptionType`. `--json` is documented as
that subcommand's default output, which is as stable a declaration as a CLI
gives, and it was checked on the installed binary rather than recalled. The
"usage" clause is **not** satisfiable today and the table above says why. M7.

**1232 — Allow harness adapters to expose subscription-usage telemetry
independently from API-provider telemetry.** **CLOSED on the independence,
with a placement objection recorded.** `HarnessTelemetry` and
`RateLimitHeaders` are separate types with separate `ReadingSource` variants
and separate appliers; neither can write into the other's fields, and applying
them in either order gives byte-identical results
(`the_two_telemetry_seams_do_not_overwrite_each_other`). **The architecturally
correct home for the status *arguments* is the harness adapter trait**, which
`IntegrationId::executable_candidates`'s own doc comment argues for — and
`crates/glasshouse/src/harness/**` is outside this package's partition. The
executable *name* is not duplicated (it is resolved through
`executable_candidates`); only `HARNESS_STATUS_ARGS` lives in
`provider/resources.rs`. See the report for the two-line trait method this
wants to be.

**1233 — Allow a user to enter a known plan or manual budget when the provider
exposes no usable telemetry.** **CLOSED.** `[providers.<name>.quota]` with
`plan` and `budget`, read into `ReadingSource::UserConfiguration` readings,
shown in `glasshouse resources` marked `[manual]` with the layer named. The
report also tells a user the option exists at the moment they are looking at a
screen of `unknown`. M15, M16.

**1234 — Never label an inferred subscription percentage as exact.**
**CLOSED, structurally — D4.** `NormalizedCapacity::percent` answers
`Percentage`, not `u8`. `Percentage::exact` returns `None` for an estimate;
`Percentage::estimated` is the only other route to the digits and hands back
the confidence and the source with them; the digits-only accessor is private
and exists solely so `Ord` can find the binding pool. `render` is the one path
to text and marks an estimate. For a *subscription* specifically the guard
fires a layer earlier: opaque pools produce no percentage at all
(`a_subscription_produces_no_percentage_for_line_1234_to_have_to_label`). M4
and M5.

**1235 — Attach a confidence value and source description to every estimated
capacity value.** **CLOSED.** Both are fields of `Percentage::Estimated`, so an
estimate cannot be constructed without them, and the confidence is the *weaker*
of the two readings' (`Confidence::weaker`, named rather than written as
`a.max(b)` because that line inverts easily). `ReadingSource::describe` is the
source description, and what may go into one is enforced at the boundary rather
than trusted.

**1236 — Record the timestamp of the last successful quota observation.**
**CLOSED.** `CapacityState::last_observed_at_unix` — the latest across every
pool, both window timestamps, every rate ceiling and the plan. "Successful" is
not a separate flag: `Capacity::Measured` is the only state that exists because
an observation succeeded. Shown as `last observed unix <n>` in the report. M13.

**1237 — Mark quota telemetry stale after a provider-specific configurable
age.** **CLOSED.** `QuotaStaleAfterSeconds` on `[providers.<name>.quota]`,
resolved per provider by `EffectiveConfig::quota_stale_after`, applied by
`Reading::freshness`. The test that makes it *provider-specific* rather than
merely configurable is
`two_providers_with_different_configured_ages_disagree_about_the_same_reading`:
one reading, two ages, two answers. M12 and M14.

**1238 — Fall back from authoritative telemetry to observed estimates without
failing the active coding session.** **CLOSED, structurally.** No function in
`provider::telemetry` or `provider::resources`'s telemetry path returns a
`Result`; a missing, malformed, negative or unrecognised reading produces
`Capacity::Unmeasured`, and `Capacity::prefer` never lets an unknown displace a
measurement. There is no error for a caller to fail a session on because there
is none to propagate. A **stale reading is still reported**, not discarded
(`a_stale_reading_is_still_reported_rather_than_discarded`).

**1239 — Treat completely unknown quota as a routing uncertainty rather than as
zero or one hundred percent remaining.** **OPEN. Nothing asks a routing
question of capacity, verified rather than assumed.**
`discover.py --seam CapacityState` finds no call site outside
`provider/**`. `grep` over `crates/glasshouse/src/routing/` finds capacity and
quota only in prose: `routing::free`'s `Health` cools a resource down after
*failures* it observed, never after reading a capacity, and
`PremiumReservePercent` is read only by `shell/state.rs` and `shell/view.rs`,
displayed and edited, never compared against a measurement — which is what
Phase 32A recorded and is still true. The consumer belongs to the routing
phases (33–37), which the packet states are all at zero. Half the line is
nonetheless already unbreakable here: `Capacity::reading()` answers `None` for
every unknown state, so a router *cannot* read an unknown as `0` or `100`. What
does not exist is anything that treats it **as uncertainty** in a decision. No
consumer was invented to close it.

**1240 — Surface the telemetry source in debug and resource views.**
**CLOSED.** Every row of `glasshouse resources` names its class, every reading
names its origin in a sentence, and every unknown says `unknown`. Driven over
the whole registry rather than a sample. M20.

**Map line 1761 (Phase 47) — Add a debug view showing quota information and
whether it is measured, inferred, or unknown.** **CLOSED**, by the same work:
`glasshouse resources --verbose` shows every pool, window and rate ceiling
including the ones nothing is known about, each with its class. Phase 47's
other fourteen lines are untouched.

**Phase 49 — provider-specific quota overrides / a monthly or rolling monetary
budget.** **CLOSED.** `QuotaOverride` and `MonetaryBudget` with `BudgetPeriod`
(`calendar-month`, `rolling-thirty-days`), layered project-over-user like every
other provider field. Deliberately not `RouterCostMicroUsd`, which caps the
price of one decision rather than cumulative spend — the distinction Phase 32A
recorded. **Glasshouse counts no spend against it**, so the ceiling reaches
`CapacityState::user_budget` as the pool's *limit* with the remaining half
unmeasured, and the report says so in words rather than implying a balance.

---

## Phase 32A's sixteen lines

Phase 32A's ledger marked sixteen lines *OPEN — Phase 32B*, on the correct
principle that **a structural guarantee is not a closed box** and that these
close only when a reading is actually taken and a caller shows it. Applying
that criterion honestly, this package takes a real reading for **two** of them
and leaves fourteen where they were. Appended to `phase-32a.md`; summarised
here:

**Now measured through the shipped binary, from a live host:**

- **1207 — request count tracked independently from token consumption.**
  AnyRouter's `ratelimit-limit: 300` fills the request pool's ceiling while
  every token pool stays `Unmeasured` in the same `CapacityState`. That the two
  do not alias was a structural claim in 32A; it is now a thing that visibly
  happened to one of them and not the other.
- **1214 — requests-per-minute limits, when known.** The same reading with
  `ratelimit-policy: 300;w=60`, filed as a per-minute ceiling because the
  window says a minute. Now known, for one provider.

**Open, with the reader built and tested but no host having supplied a
number — the distinction 32A was right to insist on:**

- **1216 — long-window request pools.** `LongWindowRequests` is filled by the
  same parser when the window is longer than a minute, and
  `a_ceiling_over_a_longer_window_becomes_a_long_window_pool_carrying_its_period`
  proves the split. No host Glasshouse can reach has sent one. **Open.**
- **1217 — provider-native units preserved alongside any normalized
  percentage.** Every reading carries its `NativeAmount` and the report prints
  `300 requests`, never a converted figure — but the line's antecedent is *"any
  normalized percentage"*, and **Glasshouse still computes none in the shipped
  binary**, because no pool has had both halves read. 32A recorded exactly this
  and it is still true. **Open.**

**Still open, each needing a number no provider Glasshouse can reach has
published:** 1199, 1200, 1202, 1205, 1206, 1208, 1210, 1211, 1212, 1213, 1215,
1218. The model represents each and the readers would fill each in; nothing has
supplied one. 1202 additionally remains blocked on
`crates/glasshouse/src/profile/mod.rs`'s `BackendResource::Native => {}` arm,
which is outside this package's partition exactly as it was outside 32A's —
**the same file has now blocked the same box for two consecutive packages**,
which is worth a round's attention on its own.
