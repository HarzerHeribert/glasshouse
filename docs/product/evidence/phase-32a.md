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

State: **COMPLETE** for lines 1198, 1199, 1200, 1201, 1202, 1204, 1207, 1211, 1214,
1217 and 1218 — eleven of twenty-nine. Added across four packages: 1207/1214 by
PHASE-32B, 1200/1202 by QUOTA-FOLLOWUP, and **1199, 1211, 1217 and 1218 by
PACKET-QUOTA-LIVE**, which is where the chain finally produced a number. Each
appended section below carries its own production caller.

**1217 and 1218 were open for four consecutive packages on the same honest
ground** — the model guaranteed the property structurally from 32A onward, and no
package would tick a guarantee that had never fired in the shipped binary. It fires
now: `glasshouse resources` renders

    capacity   99% of tokens (tokens, the `x-ratelimit-remaining-tokens` response header)

for `groq`, a provider this build templates, with the raw `tokens` and `requests`
pools still printed beside it — which is 1218's entire content — from Groq's own
measured header values rather than invented ones. **OPEN** for the other
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
  **COMPLETE, 2026-09-06** (`GH-PROVE-IT-BATCH-2`, tests only; the entry at
  the end of this file). `LongWindowRequests` carries its own
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

---

## Appended by BRIDGE-QUOTA, 2026-08-27 — the bridge is built; still zero close

QUOTA-FOLLOWUP's own account named the blocker precisely: "nothing in the
shipped binary asks that question of a gateway-captured reading" because a
gateway-backed session and a `glasshouse resources` invocation are different
processes, and bridging one into the other "needs either a shell-side surface
or a persisted cache, both outside this package's partition." This package's
whole brief was that bridge. See `.agent-runtime/report-BRIDGE-QUOTA.md` for
the full account; summarised against this file's own lines:

**Still zero of the sixteen close, and the reason has sharpened again rather
than moved.** `provider::telemetry::GatewayQuotaCache` now persists a
gateway-captured reading to a per-provider file under
`RuntimePaths::data_dir()`, exactly `provider::cache::ModelCache`'s own shape,
and `GatheredTelemetry::gather_gateway_quota` folds it back in through the
same `with_provider_headers` seam `--probe` already uses — proven at the
actual `report()` function with a reading planted where the cache would put
one. **Nothing in the shipped binary calls either new entry point.** Both
call sites are in `crates/glasshouse/src/main.rs`
(`gateway::start_if_required` on the write side, two call sites;
`GatheredTelemetry::new()` on the read side, one), which is
`PACKET-PHASE-48-CLI`'s this round — the report names the exact three lines.

**And a layer under that was found and is worth recording here rather than
only in the report.** Even with those three lines, 1199/1217/1218 would still
not close **for a real user** today: Groq — the only host observed anywhere
sending both halves of a pool in one unit — is not a provider this build
ships a registry template for (`grep -rl groq crates/glasshouse/src` outside
test fixtures returns nothing), and AnyRouter — a real shipped template —
has never been observed sending a remaining count on either its `/models`
path or (per the QUOTA-FOLLOWUP probe) authenticated. So closing these lines
for real needs the wiring **and** either a Groq template or new evidence from
a registered provider's inference path — neither of which this package
invented, per D1/§23.

**`shell/state.rs` and `shell/view.rs`, granted as a hedge, turned out to be a
dead end and it is worth recording why rather than leaving it to be
rediscovered.** `shell::run`'s own session-launch path
(`shell/mod.rs::start_session`) never resolves a gateway-backed profile —
every quick-opened session is `LaunchProfile::native`. A `Gateway` only ever
exists inside `main.rs::launch_session`'s or `resolve_resume_overlay`'s
single blocking call, never inside the interactive shell's process. Any
future package wanting the shell to show *live* gateway-backed quota, rather
than reading the persisted cache this package built, needs `shell/mod.rs`
itself in its partition — not the state/view split alone.

1210 stays open on the same grounds it has for three packages running: no
host anywhere publishes a window start. This is now the **fourth** package to
report the same absence.

---

## Appended by PACKET-QUOTA-LIVE, 2026-08-27 — both halves landed together, and the percentage renders for a registered provider

BRIDGE-QUOTA left both bridge halves built and neither wired, and named the
layer under that: even wired, no *registered* provider had ever sent both
halves of a pool. This package's own packet framed that as "a template and
three lines" — neither closes anything alone (§35: a mechanism with no
production caller does not get its box; a template nothing captures for is
the same shape one level up) — and asked that both land together or neither
be claimed. See `.agent-runtime/report-QUOTA-LIVE.md` for the full account;
summarised against this file's own lines.

**HALF ONE.** `crates/glasshouse/src/provider/mod.rs::templates()` gained a
`groq` entry — base URL and wire protocol read off the live host (not
documentation) by the orchestrator, 2026-08-27, per
`.agent-runtime/probe-quota-headers-2026-08-27.md`; `openai-chat` only, so it
cannot back Codex, the same consequence NVIDIA's own entry already records.
`registry()` now includes a `DirectProvider("groq")` kind for the first time,
and the cache key `GatewayQuotaCache` writes under (`exchange.provider`, the
gateway's own accept loop) and the key `observed_capacity` reads
(`ResourceKind::DirectProvider { provider, .. }`) agree by construction —
both are the template's own `name` field, checked rather than assumed (the
packet's falsification check #1).

**HALF TWO.** The three `main.rs` edits `report-BRIDGE-QUOTA.md` located and
this package applied, re-reading the live file first (line numbers had
already moved, per §61): `launch_session` and `resolve_resume_overlay`
(which gained a `paths: &RuntimePaths` parameter, its one call site in
`resume_session` updated) now call
`gateway::start_if_required_with_quota_cache(..., Some(GatewayQuotaCache::new(paths)))`;
`resources_report` now calls `GatheredTelemetry::gather_gateway_quota` before
folding in `--probe`, matching the precedence
`an_explicit_probe_reading_overrides_a_persisted_gateway_one_for_the_same_provider`
requires. Every pre-existing test — including every `gateway::conformance`
test and the whole `--lib` suite — passed unmodified against both edits.

**The read side is mutation-proven at the production call (§35).** Deleting
`resources_report`'s new `gather_gateway_quota` line kills
`tests/provider_discovery.rs::a_planted_gateway_reading_now_reaches_the_shipped_binarys_report`
(BRIDGE-QUOTA's own negative, flipped: the doc comment, the name, and the
assertion all now say what actually happens) and the new
`groqs_own_real_headers_reach_the_shipped_binarys_report_as_groq`. Restored,
both pass again.

**The percentage renders, for real Groq header values, through the real
`Command::Resources` arm.** `groqs_own_real_headers_reach_the_shipped_binarys_report_as_groq`
plants Groq's own real inference-response headers — the exact values
`.agent-runtime/probe-quota-headers-2026-08-27.md` recorded, not invented
ones — at the path `GatewayQuotaCache::new` resolves to, and drives the
compiled binary as a subprocess. It renders:

    groq (remote)
      quota shape     metered balance
      locality        remote
      limited by      tokens, requests, credits
      telemetry       authoritative
      capacity        99% of tokens (tokens, the `x-ratelimit-remaining-tokens` response header)
      tokens          remaining 5991 tokens [authoritative] ... limit 6000 tokens [authoritative] ...
      requests        remaining 6999 requests [authoritative] ... limit 7000 requests [authoritative] ...
      rolling window  starts unmeasured (unknown), resets unix 1787800012 [authoritative] ...

This is the first time anywhere in this project that `glasshouse resources`
has shown a `Percentage::Exact` for a provider the shipped binary actually
templates, a token-limited classification for one, or a populated reset time
for one — proving all four of this package's boxes fire in the rendering
function, for real captured values, not a synthetic stand-in.

**1199 — token-limited resources.** The `limited by` line above evidences
`LimitingUnit::Tokens` (`with_evidenced`, QUOTA-FOLLOWUP's own mechanism) for
a registered template, reached from the real `Command::Resources` arm — the
same mutation that kills the percentage also removes `tokens` and `requests`
from this line, leaving only `credits`. The honest answer to "does the
shipped binary ever classify a resource as token-limited, from evidence?" is
now yes, for Groq, once a reading reaches the cache.

**1211 — current quota reset time.** The `rolling window` line's
`resets unix 1787800012 [authoritative]` is `RateLimitHeaders::resets_at_unix`
read off Groq's real `x-ratelimit-reset-requests` header, reaching
`render_windows` (built by QUOTA-FOLLOWUP, previously reachable only through
a probe, never through the gateway) for the first time via a gateway-shaped
reading, for a registered template.

**1217 and 1218 — native units preserved alongside a normalized percentage;
raw telemetry never discarded for it.** `capacity 99% of tokens (tokens, the
`x-ratelimit-remaining-tokens` response header)` is the percentage next to
its own native unit and source in one rendered line, and the `tokens` and
`requests` pool rows below it are the raw readings the percentage was
computed from, still fully present — `NormalizedCapacity` carrying both
readings in, structurally guaranteed since 32A (M4) and now shown firing for
real values through the real caller.

**The honest limit, stated rather than elided.** What this package
mutation-proved at the production call is the **read** side —
`resources_report`'s new line, killed and restored. The **write** side (the
two `gateway::start_if_required_with_quota_cache` call sites) is
compile-checked, changes no existing test's outcome across the whole `--lib`
and `provider_discovery` suites, and is byte-identical to the old path when
its cache argument is `None` — a guarantee BRIDGE-QUOTA already mutation-
proved through a real accept loop and a real socket at the gateway/module
level. This package's own packet scoped its §35 requirement to the read side
only ("Delete the read-side line in `resources_report` and confirm a named
test goes red"), and no CLI-level test harness exists in this crate today
that spawns a real gateway-backed session against a fixture upstream and
observes the write side's *production* reach through `main.rs` specifically,
as opposed to through the gateway module BRIDGE-QUOTA already proved.
Building one is a real, separate piece of work — a fixture upstream server
plus a `glasshouse launch` invocation against a gateway-backed profile — not
attempted here because the packet did not ask for it and this package's own
proof stands on real captured values reaching the real rendering function
without it.

**A second honest limit, upstream of both of the above: whether a real user
can get a live session routed to Groq through the gateway at all.** Groq now
has a template and is protocol-compatible (`openai-chat`), but which
provider a gateway session's upstream actually resolves to is decided by
routing machinery this package did not touch and did not verify — Phase
32A's own record that `PremiumReservePercent` is read but never compared
against a measurement, and QUOTA-FOLLOWUP's own finding that nothing in
`routing/` asks a capacity question, both describe a routing layer (phases
33–37) this package's partition does not reach. This package proves the
*bridge* end to end with real data; it does not prove that today's build
routes a live session to Groq specifically. That is a fact about routing
selection, not about this package's own boxes.

**Disposition, offered rather than ticked (practice §33, §5, and this
project's standing rule that only the primary orchestrator updates the
capability map).** On the criterion applied throughout this file — ask the
box as a question a user would ask, and see whether the honest answer is yes
in the shipped binary — this package's own reading is that **1199, 1211,
1217 and 1218 are now answerable yes**, for a registered provider, through
the real rendering function, from real captured values, contingent only on a
live session actually reaching Groq through the gateway (the second honest
limit above, which this package did not verify and is not this package's
to verify). The four boxes stay unticked here, per the packet's own
instruction; this section states the evidence and the orchestrator's call
is the tick.

---

## Appended by PACKET-PHASE-32D, 2026-08-27 — the normalized score built on
this module, and both ways its own packet's hypothesis could have been wrong

`crates/glasshouse/src/provider/quota.rs::CapacityState::remaining_capacity_score`,
`RemainingCapacityScore`, `CapacityBand`/`CapacityBandThresholds`, and the
Phase 32F reserve-spend policy are all built directly on this module's own
`CapacityState::normalized`, `Pool`, and `Percentage` — no new module, no
edit to anything this file already recorded as closed. Full account in
`phase-32d.md` and `phase-32f.md`; summarised against this file's own two
open questions about `CapacityState::normalized`:

**Does the existing minimum exclude the user's spending budget or
credits?** Checked, not assumed: no. `CapacityState::pools()` already
listed both `credits` and `user budget` since this file's own original
package. The concern named in `PACKET-PHASE-32D`'s own hypothesis section
did not materialize.

**Does `Percentage`'s `Ord` do the right thing when an `Estimated` 5%
competes with an `Exact` 5%?** Checked: yes, and deliberately — the type's
own doc comment already states *"where the two tie, the one Glasshouse can
defend is the one to report,"* and an exact reading sorts as the tighter
one at a tied percentage. A router should prefer the exact reading at a
tie, and this module already does.

Both checks came back negative — no defect, no widening needed beyond what
`PACKET-PHASE-32D` itself built for line 1261 (short-window request
pressure, genuinely invisible to the old minimum). Recorded here because a
killed hypothesis that killed nothing is still the finding practice §44
asks for, not a reason to skip stating it.

---

## Phase 32A — batch 49 team-lead pass: 1203 CLOSED, nine lines refused

Run by an Opus team lead with two subcontractors. **One line closed, nine
returned premise-invalid.** That ratio is the result, not a disappointment: the
deliverable of this package is nine defensible "nothing produces this" rulings.

### Line 1203 — user-defined monetary budgets. CLOSED.

Contract: Given a user who has configured a spending ceiling for a metered
provider, when Glasshouse builds that resource's capacity state, the ceiling is
carried as the user-budget pool's limit — while the *remaining* half stays
explicitly unmeasured, because Glasshouse counts no spend.

State: COMPLETE

**This orchestrator's packet told the lead the opposite, as established fact.**
It said `with_user_budget` had two call sites, both `#[cfg(test)]`, and that
nothing wired the budget into a `CapacityState`. There are three, and the third
is production:

- producer — `config::MonetaryBudget` / `QuotaOverride::budget`
  (`config/mod.rs:814-914`), the `[providers.<name>.quota.budget]` table.
- caller — `provider/resources.rs:370`, in `observed_capacity`.
- propagation — `provider/telemetry.rs:1017`, in `apply_user_configuration`:
  `state = state.with_user_budget(pool)`, as a `Measured` `NativeAmount` in USD
  with `ReadingSource::UserConfiguration`. **That file's first `#[cfg(test)]` is
  at line 1491**, 474 lines below — verified by the orchestrator.
- consumer — `render_resource`'s loop over `CapacityState::pools()`; the row
  renders without `--verbose`.

Proven against the shipped binary. With a 25 USD budget configured, the
`anyrouter` block renders:

    user budget     remaining unmeasured (unknown), limit 25.000000 USD [manual] from the user's own configuration

That single row is 1203 closed **and** 1209 visibly open — the unit Glasshouse
can observe is its own dimension; the one it cannot is explicitly unknown.

**A real §35 hole, measured rather than argued.** The pre-existing binary test
`the_shipped_binary_reads_a_users_own_quota_overrides` does **not** watch this
wire: severing `with_user_budget` leaves it green (SURVIVED, 38 passed),
because every assertion it makes is satisfied by the `CONFIGURED QUOTA
OVERRIDES` block, which `render_configuration_note` prints straight from
configuration and would keep printing with the capacity model cut.

Regression evidence:
`provider_discovery::a_users_own_monetary_budget_reaches_the_shipped_binarys_capacity_state`
— drives the compiled binary, asserts the premise first (`quota shape metered
balance`, so the pool is readable rather than `Inapplicable`), reads the **pool
row** rather than the note, and asserts that row does *not* contain the note's
own wording, so it cannot pass by reading the wrong thing.

Mutation, re-run by the orchestrator: `state = state.with_user_budget(pool)` →
`let _ = pool;` → **KILLED**, the new test failing at
`provider_discovery.rs:911`. Not a compile break — `let _ = pool;` compiles,
and the same mutation SURVIVED the same target before the test existed.

### The nine refused, with the deciding fact

| line | why it is not closeable |
|---|---|
| 1205 input/output token split | `TokenBudget::with_input`/`with_output` have test callers only; no header names the split |
| 1206 cached input | `with_cached_input` test-only; no `RATE_LIMIT_HEADERS` entry contains "cache" |
| 1208 provider credits | producer writes to a throwaway state, and the one live account read answers `null` |
| 1209 remaining monetary budget | **nothing in the crate counts money spent** — a product decision, not a gap |
| 1210 window start | `WindowCapacity::with_started_at` has **one occurrence in the tree: its own definition** |
| 1212 rolling vs calendar | filled by two different readers; no shipped path holds both |
| 1213 concurrent limits | `with_max_concurrent_requests` test-only; no concurrency header exists |
| 1215 tokens per minute | `with_tokens_per_minute` test-only; Groq's ceiling arrives with **no window**, so filing it per-minute would invent the period |
| 1216 requests per day | reader built and wired to real state; no host has ever sent a window > 60s |

Two rendering rows were added anyway (`tokens/minute`, `max concurrent`), each
mutation-killed, so the dimensions are visible as unknown rather than absent —
which is what 32A's own vocabulary demands.

### Line 1212 — rolling and calendar windows tracked separately, proved

Package `GH-PROVE-IT-MISC`, 2026-08-31, Sonnet at medium (Green): mechanisms the recon found already in production, proved by tests only. Four mutations, four killed in the worker's tree. **1174 is NOT closed by this package** — its test (`precompact_memory.rs`) failed one run in three on the merged tree even single-threaded (model called once, no memory stored within a 10 s bounded wait); see `.agent-runtime/defect-hook-extraction-may-lose-its-write.md`.


### Track rolling-window capacity separately from fixed calendar-window capacity. (line 1212)

Contract: Given a provider whose rolling-window and calendar-window quota reset independently, when Glasshouse observes either via its own producer, it tracks the two windows as distinct values, neither overwriting the other, regardless of application order

State: COMPLETE — ruled 2026-08-31 by the orchestrator: a line the code already satisfied, proved by a test and a killed mutation (GH-MAP-SIDE-EFFECT-AUDIT's finding; no production change).

Production evidence:
- `src/provider/quota.rs` — `Windows::rolling/calendar, WindowCapacity`
- `src/provider/telemetry.rs` — `RateLimitHeaders::apply_to (rolling, from response headers), ProviderUsage::apply_to (calendar, from a usage-endpoint body)`

Regression evidence:
- `quota_windows::a_rolling_reset_header_populates_only_the_rolling_window`
- `quota_windows::a_calendar_reset_body_populates_only_the_calendar_window`
- `quota_windows::a_rolling_and_a_calendar_reset_are_tracked_as_two_distinct_values`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/provider/quota.rs: Windows::calendar() returns &self.rolling instead of &self.calendar | `skip-state-update` | **killed** | `quota_windows::a_calendar_reset_body_populates_only_the_calendar_window, quota_windows::a_rolling_and_a_calendar_reset_are_tracked_as_two_distinct_values` |

> skip-state-update observed: assertion `left == right` failed: the calendar window must carry its own reading, not the rolling window's

Recorded scope limits — stated by the worker, not discovered later:
- exercises the two producer functions directly, not through the shipped binary's telemetry-fetch path; header-parsing edge cases stay provider/telemetry.rs's own unit-test suite's responsibility


## 1209 — CLOSED 2026-09-02, on `GH-BUDGET-SPEND-COUNTER`'s evidence

The refusal table above named the one missing thing — *nothing in the crate
counts money spent* — and called it a product decision. The decision was
reversed by `design-decisions.md` (*Counting money spent against the user's
budget*): money is a read-time product of recorded tokens and `pricing.toml`
rates, and the budget pool's remaining half is now measured (`phase-32d.md`,
1263; `phase-35a.md`, 1519).

**Contract.** Given a user-configured spending ceiling, when Glasshouse builds
a provider's capacity state, Glasshouse tracks the remaining monetary budget as
its own pool — `CapacityState::user_budget`, limit from the configuration,
remaining from the counted spend — separate from and beside the provider's own
request and token pools, while preserving that a budget nobody could count
against stays *remaining unmeasured* rather than borrowing a figure from
provider quota.

**Evidence** is the 1263 entry's:
`budget_spend::priced_rows_under_the_budget_are_counted_and_lower_the_score`
reads *6.000000 USD remaining* and a band line *bound by user budget* — the
score can bind on the user-budget dimension only because that pool carries its
own measured remaining; the pool row itself was proven for 1203 by
`provider_discovery::a_users_own_monetary_budget_reaches_the_shipped_binarys_capacity_state`;
the `remaining-not-set` mutation isolates the pool wiring. Independence from
provider quota is structural: the fixture carries no provider quota telemetry
at all and the budget pool is the binding dimension regardless.

State: **COMPLETE**. Phase 32A stands at 14 of 21; the seven still open are
Cluster E — a provider signal that does not arrive (1205, 1206, 1208, 1210,
1213, 1215, 1216).

## Closed as decided-out — 2026-09-06, the user's decision 5 (`GH-REFUSED-LINES-CENSUS`)

The user authorised closing lines whose producer is a vendor's that will never ship or a concept the design dropped, with the register's reasoning recorded (`design-decisions.md`, *Steering decisions of record — 2026-09-06*, item 5). The census (`.agent-runtime/report-refused-lines-census.md`, Table 1) named these; each box is ticked on that decision, not on a mechanism.

### Track input-token budget independently from output-token budget when the provider exposes separate limits. (line 1205)

State: **COMPLETE — decided-out**, ruled 2026-09-06 by the orchestrator under decision 5. Why the producer will not exist: no provider header names an input-vs-output token split; the signal does not arrive (register:129 (Cluster E)).

Limits: no behaviour is verified; the box records that the input the line depends on is not Glasshouse's to produce. It comes back off the day a provider states the split in a header Glasshouse reads — that package cites this entry in its Phase −1.

### Track cached-input usage independently when the provider exposes cache telemetry. (line 1206)

State: **COMPLETE — decided-out**, ruled 2026-09-06 by the orchestrator under decision 5. Why the producer will not exist: no `RATE_LIMIT_HEADERS` entry of any observed provider contains "cache" (register:130 (Cluster E)).

Limits: no behaviour is verified; the box records that the input the line depends on is not Glasshouse's to produce. It comes back off the day a provider states a cache quota in its headers — that package cites this entry in its Phase −1.

### Track provider credits independently from raw tokens when credits are the actual limiting unit. (line 1208)

State: **COMPLETE — decided-out**, ruled 2026-09-06 by the orchestrator under decision 5. Why the producer will not exist: the one live account-credits read answers `null` (register:131 (Cluster E)).

Limits: no behaviour is verified; the box records that the input the line depends on is not Glasshouse's to produce. It comes back off the day a provider's account-credits endpoint answers with a number — that package cites this entry in its Phase −1.

### Track concurrent-request limits when they materially affect routability. (line 1213)

State: **COMPLETE — decided-out**, ruled 2026-09-06 by the orchestrator under decision 5. Why the producer will not exist: no concurrency header exists on any observed provider (register:132 (Cluster E)).

Limits: no behaviour is verified; the box records that the input the line depends on is not Glasshouse's to produce. It comes back off the day a provider states a concurrency limit in a header — that package cites this entry in its Phase −1.

### Track tokens-per-minute limits when known. (line 1215)

State: **COMPLETE — decided-out**, ruled 2026-09-06 by the orchestrator under decision 5. Why the producer will not exist: Groq's token ceiling arrives with no window; filing it per minute would invent the period (register:133 (Cluster E)).

Limits: no behaviour is verified; the box records that the input the line depends on is not Glasshouse's to produce. It comes back off the day a provider states the window with the ceiling — that package cites this entry in its Phase −1.



---

### Line 1216 — closed 2026-09-06: a window longer than a minute is tracked as a long-window pool carrying its period

State: **COMPLETE** — `GH-PROVE-IT-BATCH-2` (Sonnet, Green; report `.agent-runtime/report-prove-it-batch-2.md`). Ruled by the primary against the register's Cluster E row: unlike the five Cluster E lines closed as decided-out under decision 5, this reader exists and is wired, and the line says *when known*.

Contract: given a provider response whose rate-limit policy states a window longer than 60 s, when the gateway reads it, Glasshouse tracks a long-window request pool carrying that limit and that period and the capacity model can read it back, while preserving that the per-minute reading is untouched.

Production: `provider/telemetry :: RateLimitHeaders::read` → `apply_to` → `CapacityState::rate_ceilings().long_window_requests()` (`provider/quota/mod.rs :: LongWindowRequests { limit, window_seconds }`); rendered by `provider/resources/mod.rs :: render_rate_ceilings`. Test: `provider::telemetry::tests::a_ceiling_over_a_longer_window_becomes_a_long_window_pool_carrying_its_period` (existing; `ratelimit-policy: 300;w=3600` → limit 300, window 3600). Limits: no observed provider has ever sent a window longer than 60 s — the proof is a synthetic header; the `resources` render is exercised only with an unmeasured pool by the existing suite.
