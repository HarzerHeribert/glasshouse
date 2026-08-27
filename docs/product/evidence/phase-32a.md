# Capability evidence — phase 32A

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 32A — the capacity model, and the eighteen lines that need a measurement

**Audit first, and this time the audit found nothing.** Phase 32 has
repeatedly been the shape where a third of a phase turns out already shipped;
32A is not. A grep of `crates/glasshouse/src` for capacity, quota, budget,
credit, rate-limit, remaining and window vocabulary finds **no capacity value
anywhere in the crate** — no remaining count, no balance, no window start, no
reset time, no rate ceiling, no spending ceiling. Six symbols are adjacent and
none satisfies a box:

| symbol | file | what it is | why it is not a box |
|---|---|---|---|
| `QuotaModel` | `provider/registry.rs` | the four quota *shapes* | a shape, never a number — Phase 32 line 1184 already owns it |
| `Locality` | `provider/registry.rs` | `Local`/`Remote` | Phase 32 line 1185 |
| `PremiumReservePercent` | `config/mod.rs` | a user *threshold* on a normalized percentage | a threshold with nothing to compare against; the eventual consumer of line 1217 |
| `RouterCostMicroUsd` | `config/mod.rs` | a per-decision price cap | not a spending budget (lines 1203/1209); it is the fixed-point money precedent this package reuses |
| `ModelCatalogue::fetched_at` | `provider/cache.rs` | `i64` unix seconds | the timestamp precedent for lines 1210/1211, not a quota window |
| `Declared<T>` | `harness/mod.rs` | verified vs. nobody-checked | the honesty precedent; static-evidence shaped, wrong for a runtime reading |

`premium_reserve()` is read only by `shell/state.rs` and `shell/view.rs` —
displayed and edited, never compared against a measurement. That is the
confirmation: **no capacity number is produced anywhere in Glasshouse today,
so eighteen of these twenty-one lines describe a value nothing can supply.**

**What this package built.** `crates/glasshouse/src/provider/quota.rs` —
`CapacityState`, and the vocabulary it is made of: `Capacity<T>`, `Reading<T>`,
`ReadingSource`, `NativeAmount`/`UnitScale`, `Pool`, `NormalizedCapacity`,
`TokenBudget`, `WindowCapacity`/`Windows`/`WindowShape`, `RateCeilings`,
`LongWindowRequests`, `LimitingUnit`/`LimitingUnits`.

Three design decisions carry the phase's own fixed requirement — *one model,
without pretending native quota semantics are identical*:

1. **`CapacityState` is not a percentage and has no percent field.** It is a
   record of several *independent* pools, each separately unknown and each
   measured in the provider's own unit. `Pool::normalized` derives a score on
   demand and returns a `NormalizedCapacity` that **carries both raw readings
   in**. There is no constructor that takes a bare percentage, so line 1218 —
   raw telemetry is not discarded because a score was computed — is a shape
   rather than a rule somebody has to remember.
2. **"Unknown" is four states, not one.** `Capacity::Inapplicable` (no such
   pool — a local server has no credit balance), `ProviderOpaque` (the pool
   exists and the provider publishes nothing, and `is_readable()` answers
   `false` so a later telemetry pass cannot fill it in), `Unmeasured` (the
   provider publishes it, nothing has read it — every one of these is Phase
   32B), and `DelegatedUpstream` (it belongs to whichever upstream the gateway
   is bound to). The map's rule that Glasshouse must never invent exact token
   balances for opaque subscriptions is `ProviderOpaque` plus `is_readable()`.
3. **`LimitingUnits` has two answers that are not a set of units.** `None`
   (nothing can exhaust this — local inference, line 1204) and `Delegated`
   (the gateway). An empty set would have read as "nothing can exhaust it",
   which is true of Ollama and false of the gateway — the same mistake Phase
   32 refused when it declined to call the gateway `MeteredBalance`.

**Production reach, stated exactly (practice §5).**
`ResourceKind::quota` is now `self.capacity().model()` — the quota *shape* is
projected out of the capacity model rather than computed beside it. That puts
`CapacityState` on the production launch path with no new caller and no edit
outside this package: `profile::apply_direct_provider` and
`profile::apply_gateway` already call `quota()` for every session's
`"resource kind"` mechanism note, which `main.rs::mechanism_summary` renders
into the "opening a harness session" log line.

**The honest limit: the launch path reads exactly one projection out of the
model — which shape the resource's quota takes.** Every pool, window and rate
ceiling below that is proven only by this module's own tests. That is recorded
here rather than implied, the same way Phase 32 recorded that `registry()` has
no production caller.

**Nothing user-visible changed.** The mechanism note's text is byte-identical
(`QuotaModel::as_str` and `ResourceKind::label` are untouched), which the
unchanged `profile::tests` assertions on that note's string confirm.

State: **COMPLETE** for lines 1198, 1200, 1201, 1202, 1204, 1207 and 1214
(1207 and 1214 added by PHASE-32B, 1200 and 1202 by QUOTA-FOLLOWUP — see the
appended sections below, which carry each one's production caller). **OPEN** for the other
eighteen — see the per-line breakdown below; sixteen of them wait on Phase
32B, and two are blocked on files outside this package's partition.

Production evidence:
- `crates/glasshouse/src/provider/quota.rs::CapacityState` — the model.
- `crates/glasshouse/src/provider/quota.rs::CapacityState::for_resource` —
  the classification the launch path reaches.
- `crates/glasshouse/src/provider/registry.rs::ResourceKind::quota`
  (rewritten) — `self.capacity().model()`; the projection that makes the
  capacity model production-reachable.
- `crates/glasshouse/src/profile/mod.rs::apply_direct_provider` and
  `::apply_gateway` (**unedited**; the existing callers) — every
  direct-provider and gateway launch now builds a `CapacityState`.

Regression evidence:
- `provider::quota::tests::*` (22 tests) — one per behaviour the twenty-one
  lines describe, including: every registry entry's quota shape agrees with
  its own capacity state; two different remote providers produce byte-equal
  capacity models (provider-independence asserted, not claimed); a
  subscription's token pools are `ProviderOpaque` and `is_readable()` is
  false; local inference answers `LimitingUnits::None` while the gateway
  answers `Delegated`; input, output and cached-input token pools are three
  different pools; a rolling and a calendar window are tracked at once; every
  rate ceiling is its own field and a long window names its own period; a
  normalized score carries its provider-native readings, unit, scale,
  observation time and source; two incommensurable readings refuse to
  normalize; the *binding* pool is what a resource's normalized capacity
  reports.
- `crates/glasshouse/tests/provider_discovery.rs` (5 new tests) — the same
  model from outside the crate, and over every template the binary ships
  rather than over a sample: every shipped template's quota shape agrees with
  its capacity state; **nothing the registry can describe reports a measured
  number or a normalized score**, asserted over every pool of every entry; a
  subscription, a local server and a metered account give three *different*
  unknowns; a caller outside the crate can record a reading and normalize it
  without losing the unit.

Failure/isolation evidence:
- `nothing_the_registry_can_describe_reports_a_capacity_number_it_could_not_have_read`
  — the standing guard against this phase's characteristic failure. Any
  future change that makes the shipped binary claim a capacity figure with no
  telemetry behind it fails this test.
- `a_subscription_a_local_server_and_a_metered_account_give_three_different_unknowns`
  — the guard for Phase 32B: it must be able to tell which resources it may
  legitimately fill in from those it must leave alone.
- `two_incommensurable_readings_do_not_normalize_into_a_confident_number` —
  a percentage over two different units is not a percentage. Preserving the
  native unit is what makes this detectable at all.
- No credential can reach this module: `CapacityState` holds integers, unit
  names, unix seconds and `ReadingSource` strings; nothing here is resolved
  through `crate::secret` and nothing reads configuration.

Mutation evidence (practice §41, and §35 for the call rather than the callee):
- **M1, the §35 one.** `CapacityState::for_resource` mutated so a *local*
  direct provider returns `metered_balance()`: `ok` before, `FAILED` at
  `profile::tests::resolving_a_direct_provider_profile_records_whether_it_is_local_or_remote`
  — a test that enters at `apply_direct_provider`, which is what the binary
  calls — `ok` after restore. A change confined to the new module changes
  what a real launch logs; the wiring is not a helper's.
- M3 `Pool::normalized`'s commensurability check deleted → `FAILED` at
  `two_incommensurable_readings_do_not_normalize_into_a_confident_number`.
- M4 `Pool::normalized` keeps the score but substitutes the limit for the raw
  remaining reading → `FAILED` at
  `a_normalized_score_carries_the_provider_native_readings_it_was_computed_from`.
- M5 `opaque_subscription()`'s token pools made `unmeasured` instead of
  `opaque` → `FAILED` at
  `a_subscription_a_local_server_and_a_metered_account_give_three_different_unknowns`.
- M6 `CapacityState::normalized` reports the roomiest pool instead of the
  binding one → `FAILED` at
  `the_binding_pool_is_what_a_resources_normalized_capacity_reports`.
- M7 the gateway answers `LimitingUnits::None` → `FAILED` at
  `local_inference_the_gateway_and_a_remote_provider_answer_the_limiting_unit_question_apart`.
- M8 `tokens_per_minute()` answers out of `requests_per_minute` → `FAILED`.
- M9 `cached_input()` answers out of `input` → `FAILED`.
- M10 `Windows::calendar()` answers out of `rolling` → `FAILED`.
- M11 `user_budget()` answers out of `credits` → `FAILED`.
- All ten `ok` before, `FAILED` mutated, `ok` after restore.

**One mutation SURVIVED, and it is a weak mutation rather than a coverage gap
— practice §41's own question, "what did the test and the mutation both
assume".** M2 reverted `ResourceKind::quota` to the hand-written match Phase
32 shipped, computing the shape beside the capacity model instead of out of
it: the whole suite passed. It has to. The two derivations agree by
construction, so
`every_resource_kinds_quota_shape_is_projected_out_of_its_capacity_state`
asserts a property that cannot fail while both exist, and the mutation attacks
the same unreachable property. That test is not what proves the wiring —
**M1 is**, because it changes the capacity model alone and kills a test that
enters at the launch path. Recorded so the survivor is not later read as a
hole.

Platform/external evidence:
- `cargo test -p glasshouse` (macOS, this worktree, run alone per practice
  §40): every target green, including `--lib` 1160 passed / 0 failed,
  `provider_discovery` 16 / 0, and `pty_smoke` 71 / 0.
- `cargo clippy -p glasshouse --all-targets -- -D warnings`: clean.
- `cargo doc -p glasshouse --no-deps` (practice §60 addendum): clean.
- `cargo fmt -p glasshouse -- --check`: clean.
- **Not run: the local gate** (`scripts/ci-local.sh`), so the Linux and
  Windows legs and the MSRV job are unproven for this change. Nothing here is
  platform-conditional — no `cfg`, no path, no process, no clock — but that
  is an argument, not a run.

#### Per-line breakdown

The criterion applied throughout is practice §33's: *ask the box as a question
a user would ask, and see whether the honest answer is yes in the shipped
binary.* A representation the model can express but the binary never
constructs is recorded as open, with what is missing named.

- **1198 — define a provider-independent CapacityState model.** **COMPLETE.**
  `CapacityState` exists, names no provider, harness or protocol, and every
  direct-provider and gateway launch computes one. M1 proves the reach.
- **1199 — token-limited resources.** **OPEN.** The model represents one
  (`a_token_limited_resource_is_representable`), but nothing in the binary
  classifies any resource as token-limited: that needs either a per-provider
  declaration nobody has established, or telemetry. **Phase 32B**, or a
  template declaration with real evidence behind it.
- **1200 — request-limited resources.** **OPEN**, identically. OpenRouter's
  free tier is request-limited in fact; no evidence for that was established
  in this package and none was invented.
- **1201 — credit-limited resources.** **COMPLETE.** Every remote
  direct-provider launch constructs `LimitingUnit::Credits`. The *balance* is
  never read — that is line 1208, which stays open.
- **1202 — subscription resources with opaque provider-defined limits.**
  **OPEN — blocked on a file outside this partition.** The model represents
  one and `ResourceKind::NativeSubscription` classifies one, but
  `crates/glasshouse/src/profile/mod.rs`'s `BackendResource::Native => {}`
  arm pushes no mechanism note, so the shipped binary never constructs an
  opaque-subscription capacity state. The fix is the three lines Phase 32
  added to `apply_direct_provider`, in the native arm. That file is FORBIDDEN
  to this package.
- **1203 — user-defined monetary budgets for metered APIs.** **OPEN —
  blocked on a file outside this partition.** The model represents one, and
  `ReadingSource::UserConfiguration` is the source it would arrive by. There
  is no configuration field for a spending ceiling: `RouterCostMicroUsd` caps
  the price of one decision, not cumulative spend. Needs a `[routing]` field
  in `crates/glasshouse/src/config/mod.rs`, which is FORBIDDEN here.
- **1204 — effectively unlimited local inference, separately from remote
  quota.** **COMPLETE.** Every Ollama and llama.cpp launch constructs
  `LimitingUnits::None` with `Locality::Local`; it is distinct from the
  gateway's `Delegated` and from any remote provider's units, and every pool
  is `Inapplicable` rather than unmeasured. M1 kills at the launch path.
- **1205 — input-token budget independent of output-token budget.** **OPEN —
  Phase 32B.** Four independent pools exist (`combined`, `input`, `output`,
  `cached_input`) and M9/`input_and_output_token_budgets_are_tracked_independently`
  prove they do not alias. Nothing reads a provider's token headers.
- **1206 — cached-input usage, independently.** **OPEN — Phase 32B.** Own
  pool, proven independent by M9.
- **1207 — request count independent of token consumption.** **OPEN — Phase
  32B.** Both may be named at once (`LimitingUnits::These`), proven by
  `request_count_and_token_consumption_can_constrain_one_resource_at_once`.
- **1208 — provider credits independent of raw tokens.** **OPEN — Phase
  32B.** Separate pool with its own native unit; no balance is ever read.
- **1209 — remaining monetary budget independent of provider quota.**
  **OPEN — Phase 32B *and* line 1203's configuration field.** Two things are
  missing, not one: somewhere for the user to state a ceiling, and something
  that counts spend against it.
- **1210 — current quota window start.** **OPEN — Phase 32B.**
  `WindowCapacity::started_at_unix`, per window.
- **1211 — current quota reset time.** **OPEN — Phase 32B.**
  `WindowCapacity::resets_at_unix`. Deliberately `Unmeasured` rather than
  `ProviderOpaque` even for a subscription: harnesses do print when a window
  turns, so this is a number 32B may legitimately read.
- **1212 — rolling-window capacity separately from calendar-window
  capacity.** **OPEN — Phase 32B.** `Windows` holds one of each rather than a
  discriminant, so a resource with a five-hour allowance and a monthly cap
  loses neither; M10 proves they do not alias.
- **1213 — concurrent-request limits.** **OPEN — Phase 32B.**
  `RateCeilings::max_concurrent_requests`.
- **1214 — requests-per-minute limits.** **OPEN — Phase 32B.**
- **1215 — tokens-per-minute limits.** **OPEN — Phase 32B.** M8 proves it
  does not alias 1214's field.
- **1216 — requests-per-day or equivalent long-window request pools.**
  **OPEN — Phase 32B.** `LongWindowRequests` carries its own
  `window_seconds`, so "or equivalent" needs no new variant for a weekly or
  hourly pool.
- **1217 — preserve provider-native quota units alongside any normalized
  percentage.** **OPEN — Phase 32B.** Structurally guaranteed and tested (M3,
  M4): the unit is a field of `NativeAmount`, `NormalizedCapacity` carries
  both readings, and normalization refuses across units. But Glasshouse never
  computes a normalized percentage today, so the line's antecedent never
  fires in the shipped binary.
- **1218 — never discard raw telemetry merely because a normalized capacity
  score was computed.** **OPEN — Phase 32B.** Same shape: the guarantee is
  built and proven by M4, and there is no telemetry to discard.

#### Note on where the module lives

The packet's `YOURS` list named `crates/glasshouse/src/quota/**` as a new
module. A top-level module requires a `pub mod quota;` line in
`crates/glasshouse/src/lib.rs`, which the same packet placed under
`FORBIDDEN FILES` with four other workers live in the round. The module was
therefore built at `crates/glasshouse/src/provider/quota.rs` and declared from
`provider/mod.rs`, both inside the partition — and it is arguably the better
home regardless, since a `CapacityState` is a derived view over a
`provider::registry::ResourceKind` exactly as the registry is a derived view
over `provider::templates`. Moving it to `crate::quota` later is a rename plus
one line in `lib.rs`; nothing depends on the path.

---

## Appended by Phase 32B, 2026-08-27 — which of the sixteen actually closed

Phase 32A left sixteen lines *OPEN — Phase 32B* on the principle that **a
structural guarantee is not a closed box**: they close when a reading is
actually taken and a caller shows it. Phase 32B built the readers
(`crates/glasshouse/src/provider/telemetry.rs`) and the caller
(`glasshouse resources`). Applying 32A's own criterion, **two** of the sixteen
now have a real reading from a live host and **fourteen** do not. Nothing in
32A's entries above is amended; this records what happened to them.

**Closed by Phase 32B:**

- **1207 — request count independent from token consumption.** AnyRouter's
  `ratelimit-limit: 300`, read on 2026-08-27 from `GET /api/v1/models` — the
  endpoint Glasshouse already calls — fills the request pool's ceiling while
  every token pool of the same `CapacityState` stays `Unmeasured`. 32A proved
  they *could not* alias; this is one of them being filled and the others not.
  Visible in the shipped binary as
  `requests remaining unmeasured (unknown), limit 300 requests [authoritative]`.
- **1214 — requests-per-minute limits when known.** The same response's
  `ratelimit-policy: 300;w=60`. The window is what makes it a per-minute
  ceiling rather than an unqualified `300`, and a ceiling whose window is
  longer lands in `LongWindowRequests` instead.

**Still open, and the reason has changed for two of them.** 1216 and 1217 now
have a *working, tested reader* and still no number: no host Glasshouse can
reach has sent a long-window request pool, and — 1217's antecedent — **the
shipped binary still computes no normalized percentage at all**, because no
pool has had both halves read. That is the same finding 32A recorded, unchanged
by having built the parser. 1199, 1200, 1205, 1206, 1208, 1210, 1211, 1212,
1213, 1215 and 1218 are unchanged for the original reason: nothing publishes
the number.

**One correction to 32A's own framing, offered rather than asserted.** 32A
wrote that line 1211's reset time is *"a number Phase 32B can legitimately
read"* because "harnesses do print when a subscription window turns". Checked
against the installed binaries on 2026-08-27, **none of them exposes it
machine-readably**: `codex doctor --json` is stable and schema-stamped and
carries no reset field, and `claude auth status --json` carries a plan and no
window at all. The reset time is legitimately readable from a *provider's*
`RateLimit-Reset` header — `RateLimitHeaders::resets_at_unix` reads one, and
`a_reset_field_reaches_the_rolling_window_and_not_the_calendar_one` proves
where it lands — but no host Glasshouse ships a template for has sent one on
the route it calls. Leaving 1211's pools `Unmeasured` rather than
`ProviderOpaque` was still the right call; the justification for it is the
header, not the harness.

**And 1202's blocker is unchanged, which is now a fact about the rounds rather
than about the code.** 32A recorded it as blocked on
`crates/glasshouse/src/profile/mod.rs`'s `BackendResource::Native => {}` arm,
outside its partition. That file was outside Phase 32B's partition too, so the
same box has been blocked by the same three lines in the same file for two
consecutive packages. See `.agent-runtime/report-PHASE-32B.md` for the exact
patch.

---

## Appended by QUOTA-FOLLOWUP, 2026-08-27 — three more close, and 1202 was
already fixed by the time this package started

See `docs/product/evidence/phase-32b.md`'s own "Appended by QUOTA-FOLLOWUP"
section for the full account. Summarised against this file's own sixteen
lines: **1200 closed** (AnyRouter's real request header now evidences
`LimitingUnit::Requests` at the report's "limited by" line, capability map
line 1200's own text). **1202 was found already closed** — a commit between
`phase-32b`'s package and this one (`60f8c9f`, per this packet's own
freshness note) added the three-line fix to `BackendResource::Native`
independently; `profile::tests::resolving_a_native_profile_records_it_as_a_
subscription_resource` already proves it. **1216 is unchanged** — still no
host has sent a long-window request pool. **1217 gained real live data and
still does not fire in the shipped binary**: Groq's real inference response
(the one path `provider::discovery` cannot reach without spending a token)
gives both halves of a token pool in one unit for the first time anywhere,
proven producing a real `Percentage::Exact(99)` at the model level — but
nothing in the shipped binary asks that question of a gateway-captured
reading, so the antecedent still does not fire for a real user. 1199, 1205,
1206, 1208, 1210 (a caller now exists — `render_windows` — and still no host
publishes a start), 1212, 1213, 1215 and 1218 are unchanged for the original
reason: nothing publishes the number to a caller this package's partition
can reach.
