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
1236, 1237, 1238 and 1240, and for map line 1761. **1229 and 1230 closed by QUOTA-FOLLOWUP** —
see the appended section. **OPEN** for line 1239 alone — see the per-line breakdown, which names what each is waiting on.

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
zero or one hundred percent remaining.** **CLOSED**, and the paragraph this
replaces was **stale, not wrong when written**. It said *"nothing asks a routing
question of capacity"*; that ceased to be true when line 1598 closed
(`phase-37.md:51`), and the consumer it said belonged to phases 33-37 now
exists. Nothing was invented to close this line — the mechanism was already
there and unwatched.

Two mechanisms carry it, and they are **not** the same one:

- `quota_pressure` (`routing/session.rs:2228`) prices an unread destination's
  `known quota pressure` term at exactly `0.0`, with evidence reading *"nothing
  has been read about ... neither preferred nor withheld"*.
- the line-1587 affinity facet (`routing/session.rs:2062`) marks the same
  destination `AffinityFacet::unknown` rather than applying a known band's
  penalty.

**The honest limit, and it is the reason this entry is longer than the tick.**
The pressure term alone does **not** separate unknown from known-empty: a `0%`
capacity gives `routing_fraction() * WEIGHT == 0.0`, numerically identical to
the unread arm's `0.0`. The new test's third assertion
(`unread >= empty`) is therefore satisfied by **equality** and proves nothing
on its own, despite a failure message that reads as though it would catch the
"treated as zero" case. That assertion is not the evidence for this line and
must not be cited as such.

What actually separates unknown from empty is the affinity facet, and that is
mutation-proven: turning the `None` arm from `AffinityFacet::unknown` into a
penalised `known` facet is KILLED by
`session_affinity::the_reserve_band_costs_a_session_affinity_and_the_healthy_band_does_not`
— **re-run by the integrator at integration**, independently of the worker, on
the merged tree (`mutate.sh: mutation session.rs: KILLED`), precisely because
this was the half the new test does not watch. The "not one hundred percent"
half is watched by the new test
(`session_router::unknown_quota_is_scored_as_uncertain_not_full_or_empty`,
mutation `assume-full` KILLED).

Scope limits: no claim is made about `None`-arm handling of any term other than
these two.

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

---

## Appended by QUOTA-FOLLOWUP, 2026-08-27 — the readings arrive; three more
close, and 1202 turns out to have closed itself

Applying 32A's and 32B's own criterion throughout — *a structural guarantee
is not a closed box; it closes when a reading actually reaches a caller in
the shipped binary.* This package's brief was `.agent-runtime/packet-quota-
followup.md`, working from a live probe
(`.agent-runtime/probe-quota-headers-2026-08-27.md`) neither prior package
had: Groq's real `POST /chat/completions` response, and OpenRouter's real
`GET /api/v1/key` response (field names and types only, never values).

**1202 was found already closed, by neither this package nor a patch.**
Twice-deferred as blocked on `profile/mod.rs`'s `BackendResource::Native`
arm, outside two consecutive packages' partitions. It is inside this one's,
and the fix was to go read the file rather than apply the verbatim patch the
prior report wrote — which assumed a `BackendResource::Native { harness }`
shape the arm no longer has. Some commit between `phase-32b`'s package and
this one's start (the freshness note this packet itself carried named
`60f8c9f` as the commit that gave the arm a resource kind) already added
the three-line fix, and
`profile::tests::resolving_a_native_profile_records_it_as_a_subscription_resource`
already proves it reaches `resolve`. Nothing was changed; the box was
re-verified against the live file rather than assumed still blocked.

**Closed this package, three lines:**

- **1229's gateway half.** D1 (this packet's own design note) reverses the
  packet-phase-32b D2 exclusion: reading a response *header* is not reading
  the *payload* a pass-through gateway must not parse. `gateway::ingress::
  forward` now reads `RATE_LIMIT_HEADERS` off every response it forwards,
  before relaying it, never a byte of the body; the reading reaches
  `SessionRouting::observe_quota_headers` and is exposed as
  `Gateway::quota_headers()`. Proven through a real `Gateway`, a real TCP
  exchange, and the accept loop's own unmodified call —
  `gateway::conformance::a_real_forwarded_exchanges_rate_limit_headers_reach_the_gateway`
  — and mutation-killed by disabling the accept loop's call to
  `observe_quota_headers`. **The line's own wording asks for both API and
  gateway responses; both now have a working, reached reader.** Line 1229 is
  therefore CLOSED, superseding phase-32b's "closed for API responses,
  one provider" note above — the gateway half was the only piece missing.
- **1200 — request-limited resources, evidenced.** `RateLimitHeaders::apply_to`
  now evidences `LimitingUnit::Requests` whenever a request pool reading
  actually lands, via `LimitingUnits::with_evidenced`. AnyRouter's real,
  `--probe`-reachable header (`ratelimit-limit: 300`, unchanged from
  phase-32b's own probe) is enough: `describe_limits`'s "limited by" line for
  `anyrouter` reads `credits` before a probe and `requests, credits` after
  one, proven at the report level
  (`a_request_ceiling_a_reader_measured_reaches_the_limited_by_line`) through
  `report`, which `main.rs::resources_report` calls unmodified.
- **1230 — a provider's own usage endpoint, read.** `crate::provider::
  usage_endpoint` names OpenRouter's `/key`;
  `discovery::read_response_body` fetches it (a second request, behind
  `--probe`, never on the no-flag path — D2 respected);
  `telemetry::ProviderUsage::read` parses `data.limit`, `data.limit_remaining`
  and `data.limit_reset`, each as **present-and-null → `Inapplicable`,
  present-and-a-number → `Measured`, absent → left alone** — D3 in code, not
  merely stated. Tested against the *exact* real body shape this account
  answered with (all three fields null). Wired into `probe_provider` (which
  now makes the connectivity request **and** the usage request when one is
  declared) and `render_probe`, both called by `main.rs` unmodified;
  mutation-killed by disabling the usage fold and watching
  `probe_provider_makes_both_the_connectivity_request_and_the_usage_request`
  fail. `usage`, `usage_daily/weekly/monthly` and `rate_limit.interval` are
  read into `ProviderUsage`'s own fields and deliberately **not** folded into
  `CapacityState` — see the type's own doc comment for why folding a
  cumulative spend counter into "remaining capacity" would assert a
  relationship the endpoint never stated, and why `interval`'s format was
  never observed as a value, only a type.

**Extended, not closed — 1199, 1217, 1218.** Groq's real inference response
gives the token pool's own headers
(`x-ratelimit-{limit,remaining,reset}-tokens`) — the first and only seam
observed anywhere with both halves of a token pool in one unit — and
`RateLimitHeaders` now reads them, including the `12.342s`/`90ms`
duration-suffixed reset format neither AnyRouter's plain-integer style nor
this parser's old shape could read (`parse_reset_seconds`). Fed through the
model directly, this produces a real `Percentage::Exact(99)` for both the
token and request pools —
`telemetry::tests::groqs_reading_produces_a_real_exact_percentage_from_the_model_alone`
— which is 32A's and 32B's own standing test for whether the antecedent of
1217/1218 has ever fired.

**It has not fired in the shipped binary, and the reason is precise rather
than the old "nothing computes one" of phase-32b.** Groq's headers arrive on
exactly one path — real inference, which only the gateway forwards, since
Glasshouse must not spend a token to check a quota. The gateway now captures
them (`Gateway::quota_headers()`, this package's own 1229 closure) — but
nothing in the shipped binary asks a percentage question of a
gateway-captured reading: `main.rs`, `cli.rs` and `shell/**` are this
package's `FORBIDDEN FILES`, and every existing caller of `Pool::normalized`
enters through `glasshouse resources`'s registry loop, which a live gateway
session's readings never reach. So **1199 stays open** — the token pool can
be filled and `LimitingUnit::Tokens` evidenced (proven at the model level,
`a_reading_of_both_pools_evidences_both_limiting_units_at_once`), but no
`--probe`-reachable host has ever sent a token header, so no caller in
`glasshouse resources` constructs one for real. **1217 and 1218 stay open**
for the matching reason on the percentage side: the usage-endpoint path
*does* have a real caller (`render_usage_probe`, above), but the one live
account this project has ever read has a `null` limit, so no percentage
fires there either. Two working, reached parsers; zero live accounts that
happen to supply both halves at once. Recorded rather than closed on the
strength of a synthetic test, per practice §5's own rule about a packet's
own claims.

**1210 and 1211 gained the caller they were missing, and stay open on their
own honest terms.** `WindowCapacity::started_at_unix` and `::resets_at_unix`
were tracked and tested since 32A and **never rendered** —
`provider::resources::render_resource` had no line for either. `render_windows`
is that line now, reached from the same unmodified `main.rs` arm and proven
through the binary
(`the_shipped_binary_shows_every_windows_start_and_reset_state_when_verbose`).
1210 stays open because no host anywhere — including Groq — has ever
published a window *start*, only resets. 1211 stays open because the one
real reset any `--probe`-reachable host has sent (AnyRouter's, if it ever
sends one — it has not) has not arrived; Groq's own request-pool reset
*would* reach `render_windows` through the rolling window the same way
AnyRouter's would, but only via the gateway, which — as with 1199/1217/1218
— has no bridge into `glasshouse resources`. Groq's *token*-pool reset
specifically is read (`RateLimitHeaders::token_reset_seconds`) but
deliberately **not** folded into `CapacityState` at all: `Windows` is
one-per-*resource* in 32A's model, already spoken for by the request pool's
reset, and folding a second, different reset into the same field would
silently pick a winner between two real numbers. Flagged rather than
patched — widening `Windows` to be per-pool is an architecture change this
package's tier is not authorized to make unilaterally.

**Unchanged:** 1205, 1206, 1208, 1212, 1213, 1215 — no provider anywhere has
ever published a separate input/output split, a cached-input figure, a raw
credit balance, more than one window at once, a concurrent-request cap, or a
tokens-per-minute window (Groq's token ceiling arrives with no window at
all, so filing it as a per-minute rate would be inventing the period).

Production evidence, beyond what is cited per-line above:
- `crates/glasshouse/src/gateway/ingress.rs::forward` — the header capture,
  headers only, never the body.
- `crates/glasshouse/src/gateway/session.rs::SessionRouting::{observe_quota_headers,
  quota_headers}` and `crates/glasshouse/src/gateway/mod.rs::Gateway::quota_headers`
  — the accessor.
- `crates/glasshouse/src/provider/discovery.rs::{BodyFetch, read_response_body}`
  — the second kind of probe capability map line 1230 needed, generic over
  any future provider-specific body read.
- `crates/glasshouse/src/provider/telemetry.rs::{ProviderUsage, apply_provider_usage,
  UsageField}` — D3 in code.
- `crates/glasshouse/src/provider/quota.rs::LimitingUnits::with_evidenced` —
  1199/1200's own seam.
- `crates/glasshouse/src/provider/resources.rs::{usage_probe, render_windows,
  describe_timestamp}`.

Mutation evidence (practice §41, §35 for the call rather than the callee),
each `ok` before, `FAILED` mutated, `ok` after restore:
- `gateway::mod.rs`'s `routing.observe_quota_headers(...)` call in the accept
  loop, disabled → `FAILED` at
  `a_real_forwarded_exchanges_rate_limit_headers_reach_the_gateway`.
- `provider::telemetry.rs`'s `apply_to`'s final `.limited_by(limits)`,
  disabled → `FAILED` at
  `a_reading_of_both_pools_evidences_both_limiting_units_at_once`.
- `provider::resources.rs`'s `probe_provider`'s usage-endpoint fold,
  disabled → `FAILED` at
  `probe_provider_makes_both_the_connectivity_request_and_the_usage_request`.
- `provider::resources.rs`'s `render_resource`'s call to `render_windows`,
  disabled → `FAILED` at
  `a_window_reset_a_reader_supplied_reaches_the_report`.

Platform/external evidence:
- `cargo test -p glasshouse` (macOS, this worktree, run alone per practice
  §40): every target green — `--lib` 1273/0, `provider_discovery` 30/0,
  `pty_smoke` 71/0, every other integration target unchanged and green.
- `cargo clippy -p glasshouse --all-targets -- -D warnings`: clean (one
  `large_enum_variant` finding fixed by boxing `ProbeReading::Answered`'s new
  field rather than its existing `headers` field, which `main.rs` already
  destructures by value and this package may not edit).
- `cargo doc -p glasshouse --no-deps`: clean (seven private-intra-doc-link
  warnings fixed — linking to a private `mod`, a private constant and a
  private enum's variants from public doc comments).
- `rustfmt` on this package's own ten files only, never `cargo fmt --all`
  (§37).
- **Not run:** the local gate (`scripts/ci-local.sh`) — another worker was
  live this round (§40) — so the Linux and Windows legs and the MSRV job are
  unproven for this change. Nothing here is platform-conditional (no `cfg`,
  no new process spawn, no path handling), which is an argument and not a
  run.
- **No provider key was used anywhere in this package.** Every live number
  cited (Groq's headers, OpenRouter's null fields) was copied verbatim from
  `.agent-runtime/probe-quota-headers-2026-08-27.md`, itself run by the
  orchestrator; every test using a non-null number is labelled as
  synthetic in its own doc comment.

See `.agent-runtime/report-QUOTA-FOLLOWUP.md` for the full audit table,
probes still needed, and what this packet got wrong.

---

## Appended by BRIDGE-QUOTA, 2026-08-27 — D2 answered, still no caller

QUOTA-FOLLOWUP's own account of 1199/1217/1218 (above) named the remaining
gap exactly: "the gateway now captures them... but nothing in the shipped
binary asks a percentage question of a gateway-captured reading," because
`main.rs`, `cli.rs` and `shell/**` were that package's own `FORBIDDEN FILES`.
This package's brief (`.agent-runtime/packet-bridge-quota.md`) was to answer
D2's own open question — persist, share a process, or report that a durable
store is a bigger piece of work than one package — and build whichever answer
survived testing. See `.agent-runtime/report-BRIDGE-QUOTA.md` for the full
account; summarised against this file's own line 1229 and its neighbours:

**D2 answered: a durable, per-provider cache
(`provider::telemetry::GatewayQuotaCache`), written by the gateway's accept
loop and read by `GatheredTelemetry`.** Not a shared process — confirmed
before building anything that no shipped code path runs the interactive
shell and a gateway-backed session in the same process (`shell/mod.rs`'s own
`start_session` never resolves a gateway-backed profile at all), so
`shell/state.rs`/`shell/view.rs` had nothing live to read from regardless of
partition.

**Both halves are built, tested against their own real production-shaped call
site, and neither is wired.** The write side
(`Gateway::start_with_quota_cache`, additive — `Gateway::start` still takes
no cache and behaves exactly as this package found it) is mutation-killed
through a real accept loop and a real socket, the same discipline this file's
own `a_real_forwarded_exchanges_rate_limit_headers_reach_the_gateway` set for
1229's in-memory half. The read side
(`GatheredTelemetry::gather_gateway_quota`) is mutation-killed through the
actual `report()` function `main.rs::resources_report` calls unmodified. What
neither can be is a §35 proof of *production* reach, because the three lines
that would call them from `main.rs` belong to `PACKET-PHASE-48-CLI` this
round — named exactly, with their line numbers, in the report.

**Line 1229 itself is unaffected — it was already CLOSED by this file's own
QUOTA-FOLLOWUP section**, and this package did not reopen it: the gateway
already reads and exposes rate-limit headers from both API and gateway
responses, in memory, which is what that line's text asks. What this package
adds is a second consumer of the same in-memory reading (a durable copy,
written alongside the existing `SessionRouting::observe_quota_headers` call,
never instead of it) — 1229's own box stays exactly as QUOTA-FOLLOWUP left
it.

**The honest ceiling, recorded here because it is this file's own 1199 that
it bears most directly on:** even once `main.rs` calls both new entry
points, 1199 does not close for a real user by that alone. Groq — the only
host observed anywhere sending a token pool's both halves — has no registry
template in this build; a gateway session against any provider this build
does ship a template for has never been observed sending a token header at
all. The wiring removes the structural blocker; it does not manufacture the
evidence, and this package did not invent any to compensate, per D1 and
practice §23.

---

## Appended by PACKET-QUOTA-LIVE, 2026-08-27 — the missing template landed beside the wiring

The gap this file's own BRIDGE-QUOTA section left was named precisely: Groq
had no registry template, so `report()`'s registry loop would never look up a
`GatewayQuotaCache` entry keyed `"groq"` even once `main.rs` called both new
entry points. This package's packet was exactly that — a `groq` template in
`provider::templates()` plus the three `main.rs` edits, landed together
because "template without wiring shows nothing; wiring without template shows
nothing; both together produce a real `Percentage::Exact` in the shipped
binary."

Both landed. `tests/provider_discovery.rs::groqs_own_real_headers_reach_the_shipped_binarys_report_as_groq`
plants Groq's own real inference-response header values (not invented ones)
at `GatewayQuotaCache`'s real path and drives the compiled binary as a
subprocess, rendering a real `capacity 99% of tokens` line for the `groq`
resource `glasshouse resources` now lists. See `phase-32a.md`'s own appended
section for the full per-line account against 1199/1211/1217/1218, the
mutation proof at the read-side call, and the two honest limits this package
did not close: an automated write-side production-reach proof through
`main.rs` specifically (the packet scoped its own §35 requirement to the read
side only), and whether today's routing machinery (phases 33–37, outside this
partition) actually assigns a live gateway session to Groq. See
`.agent-runtime/report-QUOTA-LIVE.md` for the full account.

---

# Independent audit, 2026-09-01 (`GH-AUDIT-BATCH-78`) — 1239 CONFIRMED

Dispatched to prove 1239 wrong, and pointed straight at the thin spot rather
than left to find it: the `quota_pressure` term's `unread >= empty` assertion
passes by equality and cannot be 1239's evidence.

It confirmed that reading exactly — `None` and a known `routing_fraction() == 0.0`
both price to `0.0` — and then established what does carry the line. The
line-1587 affinity facet (`routing/session.rs:2042-2071`) is keyed on
**`CapacityBand`**, not on the raw fraction:

- `Some(band) if band <= CapacityBand::Reserve` (a known, empty-or-nearly reading)
  → `AffinityFacet::known` with a **negative** magnitude — penalised;
- `Some(band)` otherwise → `known`, magnitude `0.0`;
- `None` → `AffinityFacet::unknown`, magnitude `0.0` — **not** penalised.

A known-empty reading therefore scores strictly worse than an unknown one,
which is the *"not treated as zero"* half of the line. `AffinityBreakdown::total()`
(`session.rs:1647`) sums these magnitudes and is asserted equal to the
`session affinity` `Contribution` that `SessionRouter::choose` ranks on — this
is not display-only. `choose` is called from `main.rs:4842`, before `main.rs`'s
`#[cfg(test)]` at 12696.

It also checked the thing an orchestrator most often takes on trust: that the
killing test hits the `None` arm rather than passing coincidentally. It does —
`the_reserve_band_costs_a_session_affinity_and_the_healthy_band_does_not`
builds an `unread` session with **no** `with_capacity_facts` call, so `band()`
is genuinely `None`, and asserts `!unread.is_known() && unread.magnitude() == 0.0`.
The penalised-`known` mutation flips both halves of that exact assertion.

**Recorded for future auditors, and it is not a gap:** unknown and
known-`Healthy`/`Plenty` both score `0.0` here, because this mechanism has no
positive bonus for a good band. The facet separates unknown from known-empty
but not from known-full — which is the *"neither preferred nor withheld"*
stance the line asks for, not a hole in it.
