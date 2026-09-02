# Capability evidence — phase 34C

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 34C — line 1443 only: what "resource diagnostics" means

**No box in this phase is closed.** Phase 34C (automatic routing-model selection)
is 0 of 13, and twelve of its lines are blocked for one reason a read-only recon
established this round and a proof worker then confirmed independently:
**nothing in the shipped binary ever calls a routing model.** `RoutingModelChoice`
can be configured, resolved and rendered; no code path asks it to classify
anything.

This entry exists for **line 1443** alone, because a worker did the right thing
and refused to decide it.

Contract: Given a configured routing model, when the user inspects Glasshouse's
resource diagnostics, Glasshouse names the routing model currently selected —
while never implying a model is in use when nothing calls one.

State: **NOT STARTED.**

### The question, and why a worker was right to escalate it

`GH-PROOF-ROUTER` was asked to confirm or refuse eleven lines a recon called
closable. It closed seven, refused three, and for 1443 **reported both readings
and picked neither**:

- **TUI reading** — the Settings overlay's `RoutingRow` (`shell/mod.rs:1668-1687`,
  rendered in `shell/view.rs`) already shows the resolved routing-model choice.
  On this reading the line is closed today.
- **CLI reading** — `resources_report` (`main.rs:2233-2380`) renders no routing
  information at all; a grep for "routing" in that function's body returns
  nothing. On this reading the line is open.

That is exactly the escalation practice §33 asks for: *"a worker may hand you a
judgement; take it, and say which way you went."*

### The orchestrator's ruling: the CLI reading. The line stays OPEN.

**Three reasons, in order of weight.**

1. **A settings screen is where you choose a value, not where you diagnose one.**
   The Settings overlay showing your current selection is Phase 2D's settings
   capability — *"does using a control do something real and durable?"* — and
   those boxes are already closed on it. Counting the same rendering twice, once
   as configuration and once as diagnostics, would make the ledger say a
   diagnostic surface exists when the only thing that exists is a config editor.
2. **The map distinguishes surfaces deliberately.** Phase 41's line 1661 —
   *"Show the currently selected routing model **and its recent latency**"* — is
   the project-overview surface, and it is separately open. A map that names the
   same fact for two different views is not being redundant; it is naming two
   views. `glasshouse resources` is the third, and it is the one called
   *resources*.
3. **The line's purpose is answering "why did routing behave that way".** That
   question is asked at a diagnostic surface, next to capacity, health and quota
   — not on the screen where the user just set the value themselves.

**Closing it is cheap and is not blocked**, which is worth saying so nobody
records this as architecture: `resources_report` already receives the
`EffectiveConfig` that resolves the choice. It is a rendering line and a test.
It was not done in this batch only because `main.rs` was owned by another
worker's un-integrated diff.

**One honesty constraint on whoever closes it.** Nothing calls the routing model
(`routing::classify::classify` has one production caller, the `glasshouse
classify` CLI diagnostic; nothing constructs a `TaskClassification` outside
`#[cfg(test)]`). So the diagnostic must name the model that **would** be selected
and must not imply one is in use. Rendering a "currently selected routing model"
beside live capacity numbers, with no signal that it classifies nothing, is the
spectacle Phase 47 exists to prevent.

Missing evidence:

- A routing-model line in `resources_report` and its `api/unix.rs` twin, with a
  test entering through the shipped binary — the precedent is
  `provider_discovery.rs::a_planted_gateway_reading_now_reaches_the_shipped_binarys_report`.
- Wording that distinguishes *configured* from *in use* while line 1425/1426
  remain open.

---

# Lines 1431, 1433, 1443 — closed 2026-08-30; 1437 open, 1438 and 1440 refused

Package `GH-AUTO-ROUTING-MODEL`. **Six lines against one mechanism** — the
first package sized by mechanism rather than by line, after batch 55 measured
0.77 boxes per package (§87).

## 1431 — CLOSED. It was unevidenced, not unbuilt

State: **COMPLETE**

The chain was already production-wired: `main.rs::classify_with_routing_model`
reads `effective.routing_model_resolution()`, and its
`RoutingModelResolution::Automatic` arm calls `automatic_classification_model`
→ `automatic_classification_choice` → `DisposableRouting::choose`.

**The evidence that makes it non-vacuous asserts on the wire.**
`classification_call::automatic_routing_asks_the_resource_the_routing_policy_chose`
configures two free providers at one canned endpoint and puts the *second*
candidate first in `routing.free_resource_order` — an input **only `choose`
reads**. The assertion is that the request body names `zeta-model` and does not
name `alpha-model`. Anything that reached a model without going through the
policy would name the wrong one.

## 1433 — CLOSED, and it needed production code

State: **COMPLETE**

Killed by
`provider_discovery::an_unhealthy_resource_is_not_the_one_automatic_routing_would_select`.

**Limit, stated by the package and carried here:** the health pool reaches only
**free** candidates. `choose` never asks health about a metered one.

## 1443 — CLOSED, the CLI half

State: **COMPLETE**

Built as this entry already ruled: `glasshouse resources`, not the Settings
overlay. `render_routing_model` is appended by `resources_report` in the same
shape the `PROBES` block uses, so `provider/resources.rs` is untouched.

**The rendering is honest about what it does not know**, which is why it
closes:

    in use    nothing yet. `glasshouse classify` is the only command that asks
              a routing model; no other Glasshouse decision calls one, so this
              names a choice rather than a habit.

**Why it was open until today** is recorded above: `main.rs` belonged to
another worker's un-integrated diff. It closed because §77 let this package
co-edit `main.rs` rather than queue behind it.

## 1437 and 1438 — the same vacuous qualifier, and both refused on it

**1437** *"prefer currently free candidates **after capability and latency
requirements are satisfied**"* — **OPEN.**
**1438** *"prefer local candidates **when they satisfy the configured latency
and quality requirements**"* — **REFUSED.**

The free preference in 1437 is **real**: `DisposableRouting::choose`'s free
loop returns before the metered loop. But `RouterLatencyMs` — a real, layered,
validated config value — has **exactly two consumers, both in the settings
overlay** (`shell/mod.rs:2230`, `:2628`). **No routing decision anywhere reads
it.**

So closing either line would be **closing a qualifier by absence**, which is
precisely why 1455 and 1456 were un-ticked the same morning. 1437 is `open`
rather than `refused` because its preference half is genuinely implemented and
one producer — a latency reading that reaches `choose` — would close it. 1438
is `refused` because locality is not represented on a candidate at all.

## 1440 — REFUSED

*"Avoid a scarce premium subscription session as the classifier when a cheaper
adequate routing resource exists."* See the package's report for the decisive
`file:line`; the reserve gate does not distinguish a subscription-backed
candidate in the way this line requires.

## A finding outside this package's diff, recorded so it is not lost

`routing/disposable.rs`'s `choose` doc cites
`tests::scoring_never_reorders_the_existing_free_selection` as holding the
invariant that scoring never reorders the free-tier selection. **That test does
not exist** — `grep` returns one hit, the citation itself. The invariant is real
and the code holds it (the free loop returns on the first available candidate in
`FreePreferences::arrange`'s output and never consults `score` for ordering),
but **nothing would fail if a future change broke it**. That is §49's
"doc-comment lines counted as call sites" in a new costume, and it is a Green
packet for somebody.

---

# Line 1434 — closed 2026-08-30; 1441 and 1442 built but NOT wired

Package `GH-ROUTING-STICKINESS`.

## 1434 — CLOSED

State: **COMPLETE**

*"Filter automatic candidates by minimum requests-per-minute headroom when
known."*

The signal already reached `DisposableRouting::choose`; what was missing was an
**eliminating** consumer. `GH-ROUTING-FILTERS` established the distinction and
it is the reusable part: the RPM figure was read in exactly one place —
`score()`'s ranking contribution — while every place `choose` *removed* a
candidate ignored it. A candidate at 0% headroom was ranked last and never
excluded, which is a different claim from the line's word *"filter"*.

`has_no_known_headroom` (`routing/disposable.rs:285`) is now called inside
`choose` immediately after `apply_hard_constraints`, before the free/metered
split, and `choose`'s ordered doc comment was renumbered to name it as step 2.

**The load-bearing part is the honesty rule, and it was reused rather than
invented.** `has_no_known_headroom` returns `false` for `None`: a candidate
nothing is known about is never removed. That is the **identical** rule
`CandidateCapacity`'s own doc and `score()`'s `None` arm
(`routing/disposable.rs:826-830`) already stated for the scoring path — this
package applied it to elimination rather than writing a second rule that could
drift. Eliminating on absence would turn *"we have no telemetry"* into *"this
provider is full"*.

Mutation: make the elimination step also drop candidates with an absent
reading — **KILLED** by
`an_absent_capacity_reading_never_eliminates_a_candidate`, and by every other
test in the target, since the mutation eliminates every candidate the file's
helpers build without a capacity reading.

## 1441 and 1442 — CLOSED 2026-08-31 by `GH-STICKY-WIRING`

State: **COMPLETE** (both). The Cluster B recorded below lasted about an hour.

`main.rs::automatic_classification_choice` now calls
`choose_for_automatic_classification` through a project-scoped
`RoutingStickyCache`, and the `Retained` arm carries a fully-built
`DisposableChoice` **constructed inside `routing/disposable.rs`** using the
module-private `choice(...)`.

**The first attempt refused and it was right to** — and the blocker was the
orchestrator's packet, not the worker. `DisposableChoice` has no public fields
(a stated invariant) and its only constructor is private to
`DisposableRouting`, so the wiring **cannot** be done from `main.rs`, which is
the only file that packet allowed. The reissue granted `routing/disposable.rs`
for the `Retained` arm and the wiring completed.

**The honesty requirement held.** A retained pick did not win a ranking and no
`score` ran, so its `RoutingExplanation` says it was **retained** rather than
reading as `score`'s output for a comparison that never happened — the same
class of fabrication refused elsewhere in this project (a `0.0` relevance for
an unmatched memory; a `0` token count for an unmeasured call).

**Mutations, both killed on the production call:** returning the retained pick
without re-checking health dies at
`a_retained_pick_whose_provider_turned_unhealthy_is_not_returned`, and dropping
the store dies at
`classification_call::two_successive_classify_processes_reuse_the_same_routed_resource`
— a test spanning **two `classify` processes**, which is the level a persisted
cache has to be proven at.

**The invariant that makes stickiness honest:** a retained pick is never
returned without re-checking its health. Stickiness must not outlive the
healthiness it was predicated on.

## ~~1441 and 1442 — OPEN, and this is a deliberate Cluster B~~ (superseded, kept for the record)

State: **SCAFFOLDED**, and it must not be read as anything more.

The mechanism they need **is built and tested**: an on-disk
`RoutingStickyCache` (`provider/telemetry.rs`, following `GatewayQuotaCache`'s
shape — no migration) and
`DisposableRouting::choose_for_automatic_classification`
(`routing/disposable.rs`), with tests covering the retained pick, the health
re-check, and a missing or corrupt record deciding fresh.

**Nothing in the shipped binary calls it.** `main.rs::automatic_classification_choice`
still calls `choose` directly. The packet forbade `main.rs` — it was held by
another worker — and the worker correctly stopped at the boundary and named the
insertion point rather than taking an unclaimed file.

**This is the exact shape that un-ticked five boxes earlier the same day**: code
that is correct, tested, and never run. It is recorded here as `SCAFFOLDED`
rather than quietly left to look finished, and **1441/1442 must not be ticked
until the call site lands.** The follow-up is one call site in
`automatic_classification_choice`.

**The invariant the wiring must preserve**, already enforced by the built code
and its tests: *a retained pick is never returned without re-checking its
health.* Stickiness must not outlive the healthiness it was predicated on.


---

# Package `GH-ROUTING-ECONOMICS` — 2026-08-31, Fable specialist at xhigh

**Seventeen lines against one selector**: `DisposableRouting::choose_for_automatic_classification`
(`routing/disposable.rs`) — the function `main.rs::automatic_classification_choice`
calls on `RoutingModelResolution::Automatic`, itself reached from
`classify_with_routing_model` by `glasshouse classify` and, once the launch path
classifies, by `glasshouse launch`. **Thirteen closed, four refused with one
producer each.** This is the "build the named producer" follow-up practice §83
asks for: `GH-AUTO-ROUTING-MODEL` refused six 34C filters because reliability,
latency and cost were unmeasured; this package measures two of the three.

Contract (the package's): Given automatic routing-model selection, when Glasshouse
picks which model classifies a task, it prefers candidates the evidence ledger
shows to be fast, high-headroom and reliable at returning the schema, excludes
those below a reliability floor or above the configured latency ceiling, walks a
user-configured fallback chain once when the chosen model fails, may be confined
to a local model, and can say what fraction of resources routing itself consumes
— while preserving that a candidate is filtered only on a quantity actually
measured, and that an unmeasured quantity leaves an inert, labelled term.

**The producer that made it possible**: `main.rs::record_classification_observation`
now records the **parse outcome** (`Outcome::Succeeded`/`Failed` — a reply that
could not be read as a classification failed at its purpose; migration 11's
`CHECK` fixes the vocabulary and a `Malformed` variant would have been a
migration) and the **clock either side of the call** (`with_timing`, whole Unix
seconds — every duration is a multiple of 1000 ms, stated as a limit). It is
called **after** `parse_classification`, from the new `classify_through_chain`.
Readers: `EvidenceLedger::classification_record` (`routing/evidence.rs`; median
withheld below `MIN_SAMPLE_FOR_SUMMARY`), attached per candidate by
`main.rs::attach_classification_records`.

**Preferences act; they do not only explain** (ruling accepted). `choose`'s free
loop walks `FreePreferences::arrange` and never consults `score`, and
`scoring_never_reorders_the_existing_free_selection` still holds. So the four
preferences are applied as a **stable pre-order** of the *unranked* admitted
candidates before `arrange` re-sorts by the user's own order; a user-ranked
candidate is never displaced. One definition, two consumers:
`classification_preferences` feeds both `score()` and the pre-order.

Gates (final tree): fmt clean; clippy `-D warnings` clean; `routing_economics`
**22 passed**; `routing_disposable_tier` 7; `classification_call` 9;
`provider_discovery` 45; lib filter `routing::disposable routing::evidence
evaluation:: config::` 120 passed; `blast-radius.sh` exit 0 — **54 targets, 2363
passed, 0 failed**; `mutate.sh --script` — **22 KILLED, 0 SURVIVED**, every
restore byte-identical.

**One red result, attributed and fixed**: the first blast-radius run failed
`routing::tests::the_two_policy_classes_do_not_name_each_other`, which scans
`disposable.rs` production code for the word *interactive* — the new latency
evidence string had quoted line 1421's own wording. Reworded; the guard was
doing its job on a string literal. Worth knowing for the next packet that quotes
1421 into that file.

## Phase 34B — routing-model role

- **1420** ☑ COMPLETE — *requests-per-minute headroom* preference
  (`classification_preferences`, `REQUESTS_PER_MINUTE_DIMENSION`; the reading's
  producer is the unchanged `disposable_candidate_capacity`). Killed:
  `more_request_headroom_scores_higher_and_says_so` (zeroed term → *"the roomier
  candidate was listed second and must still win"*). Limit: the RPM figure is
  visible only when RPM is the *tightest* dimension `remaining_capacity_score`
  found; otherwise the term says it is unstated.
- **1421** ☑ COMPLETE — *classification latency* preference. Killed:
  `a_faster_candidate_is_preferred_among_unranked_free_candidates`. Limit:
  one-second resolution; the line's "than direct harness use" is not measured —
  this prefers lower classification latency, not a classification-vs-harness
  delta.
- **1422** ☑ COMPLETE — *structured-output reliability* preference, on the parse
  outcome now recorded. Killed twice: zeroed term →
  `a_more_reliable_candidate_is_preferred_among_unranked_free_candidates`;
  producer lying (`if parsed.is_ok()` → `if true`) →
  `a_fallback_chain_is_walked_once_and_every_attempt_is_recorded` (*alpha's row
  recorded Succeeded, Failed expected*). Limit: per `(provider, model)` over 7
  days; fewer than five outcome-carrying calls → inert and labelled.
- **1423** ☑ COMPLETE — `classify_through_chain` walks `routing.model_fallback`
  once after the chosen model fails, never re-tries a candidate (`tried`
  set — mutation *retry the same candidate* killed: *"3 requests seen"*), records
  every attempt, names the walk in the classification's `source` label. Without
  a chain, today's stderr text is unchanged
  (`without_a_chain_a_parse_failure_degrades_to_the_heuristic_as_before`).
  Limit: consulted only after a *chosen* model failed, not when automatic
  selection admits nothing.
- **1427** ☑ COMPLETE — `routing.classification_local_only` confines candidates to
  the registry's local providers (`ResourceKind::locality()`: the `ollama` /
  `llama-cpp` slugs); **unstated locality fails closed** — the one deliberate
  inversion of "absence never eliminates", because a privacy constraint that
  admits on silence would send a request off the machine. Guarded twice (the
  policy and `classify_through_chain`'s guard before any model is built); both
  mutations killed by `local_only_never_sends_a_remote_request` /
  `local_only_admits_no_remote_or_unstated_candidate_and_says_why`. Limit: a
  local runner configured under another name is treated as remote and refused.
- **1419** ☐ **REFUSED — producer: a per-model price.** `cost_micro_usd` has no
  writer in `src/`; `RouterCostMicroUsd` is a config ceiling with no consumer;
  `metered_models` carries names only; the registry carries no price.

## Phase 34C — automatic routing-model selection

- **1432** ☑ COMPLETE — reliability floor (`CLASSIFICATION_RELIABILITY_FLOOR`
  0.8 after `CLASSIFICATION_RELIABILITY_MIN_OBSERVATIONS` 5) in
  `classification_verdict`, applied by `choose_for_automatic_classification`
  before ranking, reason names the ratio; fewer observations → admitted and
  explained as *unproven, not unreliable*. Three mutations killed, including the
  §35 one — `attach_classification_records` skipped →
  `a_candidate_the_ledger_shows_unreliable_is_not_asked_by_the_shipped_binary`
  (*the wire request named alpha-model*). Limit: a pinned model is never filtered;
  rows written before this package carry no outcome and count toward neither side.
- **1435** ☑ COMPLETE — the ceiling is the **existing** `routing.max_router_latency_ms`
  (`RouterLatencyMs`, layered, validated, default 2000 ms, previously without a
  routing consumer) — not the packet's proposed new key; the register's own 1435
  row named it. Median above the ceiling excludes with both figures; no median
  → inert and explained. Killed: admit-everything →
  `a_slow_candidate_is_excluded_by_the_configured_latency_ceiling`; timing
  dropped → the chain test (*the row must carry the clock either side of the
  call*); sample floor lowered →
  `the_ledger_withholds_a_classification_median_below_the_sample_floor`.
- **1437** ☑ COMPLETE — free candidates pass `classification_verdict` (latency
  ceiling, reliability floor) before `choose`'s free-before-metered order applies.
  Killed: free candidates bypassing the verdict →
  `a_free_candidate_is_preferred_only_after_the_latency_ceiling_is_satisfied`
  (*the 3000 ms free candidate chosen over the metered one*). Limit: "capability
  requirements" are `apply_hard_constraints`' existing gate; no per-candidate
  capability model exists for classification.
- **1438** ☑ COMPLETE — *locality* preference from `DisposableCandidate::with_locality`,
  set by `disposable_candidates` from the registry. Killed:
  `a_local_candidate_is_preferred_over_an_equally_adequate_remote_one`. Limit:
  "quality requirements" are the reliability floor; no other quality signal is
  on the path.
- **1436** ☐ **REFUSED** — same producer as 1419; `EffectiveConfig::max_router_cost`
  already resolves the ceiling, the missing half is the reading.
  **→ CLOSED 2026-09-02 (evening), `GH-CLASSIFIER-PRICE-CEILING`; see the entry at the end of this file.**
- **1439** ☐ **REFUSED** — same producer; once a price exists the comparison is
  *(1 − parsed_fraction) × median latency* against the price difference.
- **1440** ☐ **REFUSED — producer: a subscription-backed classification candidate.**
  `disposable_candidates` builds only `DirectProvider` candidates with a resolving
  credential; a `NativeSubscription` resource is never a classifier in this
  build, so there is nothing to avoid, and closing by absence would be the
  vacuous-qualifier shape this register has refused before.

## Phase 34E — router economics

- **1463** ☑ COMPLETE — `EvaluationObservations::routing_decision_rate`
  (`evaluation/mod.rs`, own `impl` block): `routing_continuation_decided` rows
  over the window divided by **interactive hours** — epoch-aligned hours that at
  least one session's `(created_at, last_activity_at)` span touches, clipped to
  the window; wall-clock hours would report a project that ran once on Monday as
  deciding at a vanishing rate all week. Rendered in `glasshouse resources`'
  `ROUTING ECONOMICS` block with the derivation. Killed: count zeroed and
  hour-range made exclusive (`interactive_hours_count_the_hours_a_session_touched`).
  Limit: `glasshouse classify` diagnostics are not decisions and are not counted.
- **1465** ☑ COMPLETE — `RoutingOverhead::from_consumption` (`routing/evidence.rs`)
  separates `purpose = classification` rows from everything else via
  `consumption_by_purpose`; **spend is tokens** (input + output; cached excluded,
  providers disagree on where it sits) because cost has no producer; an
  uncounted side prints *not comparable — <why>*, never `0%`. Killed: purpose
  check disabled → spend lines missing.
- **1466** ☑ COMPLETE — `RoutingOverhead::exceeds` at `ROUTING_OVERHEAD_WARNING_FRACTION`
  (one tenth, a constant with a stated rationale) → a `warning` line in the same
  block. Killed: threshold raised → warning line missing.

## Phase 49 — configuration

- **1795** ☑ COMPLETE — `routing.model_fallback`: a list of `{ provider = ..,
  model = .. }` tables (`FreeResourceRef`'s on-disk shape, not the packet's
  `"provider/model"` strings), layered project-over-user
  (`the_fallback_chain_and_local_only_layer_project_over_user`); read by
  `classify_through_chain`. Killed: configured chain ignored → the chain test.
  Limit: not validated against configured providers at load — an entry naming an
  unconfigured provider is skipped at walk time with a named reason, like a
  stale pin.


---

## Line 1436 CLOSED — 2026-09-02 (`GH-CLASSIFIER-PRICE-CEILING`, Amber, Sonnet high): the classifier's candidates carry a price, and the ceiling the settings surface wrote for months is finally read

`EffectiveConfig::max_router_cost` (`[routing] max_marginal_cost`, micro-USD, default 1 000) had a settings-surface writer and no router reader — the Cluster B shape. Now: `DisposableCandidate::price: Option<ModelPrice>` attached in `main.rs::automatic_classification_choice` by a new `attach_prices` from `PriceTable::load_from_dir(config_dir)` (the same call `session_router` makes; an absent or malformed file leaves every candidate unpriced); `ClassificationPolicy::with_max_marginal_cost_micro_usd` set from the effective ceiling; and in `classification_verdict`, between 1427's locality gate and 1432's reliability floor, a price gate with four arms — *free: the ceiling does not apply*; metered and unpriced: *no entry in pricing.toml — the ceiling is inert; unpriced, not expensive*; metered, priced and over: `Excluded` naming the estimate, the ceiling and map line 1436; under: an admitted note with both figures. **The estimate is a ceiling, stated as one**: `estimated_classification_cost_micro_usd(price)` prices the whole `TASK_TEXT_CEILING_BYTES` on top of `CLASSIFICATION_PROMPT_CONTRACT` at `BYTES_PER_TOKEN_ESTIMATE = 4` plus `CLASSIFICATION_REPLY_TOKENS = 64` of output — so a candidate is excluded only when even the largest permitted call is over the line. `Cost` stays the category; the price is the number; no score term reads it, so no ordering among admitted candidates moves. A ceiling of `0` admits only free candidates, as `RouterCostMicroUsd`'s doc promised.

**Worker corrections, accepted:** `shell::state::format_usd` renders six fraction digits, not four, and lives behind `crate::config` which `routing::disposable` must not depend on — a local `format_micro_usd` on a bare `u64` reproduces the shape; three rustdoc links to private items became code spans.

**What this leaves for 1419 and 1439** (the report's own accounting): 1439's price half now exists on the same candidates its latency half (1435's median) is read from — only the comparison and its ruling remain (`design-decisions.md`, *Preferring a cheap metered classifier over an unreliable free one*); 1419 still lacks a producer for *the premium capacity it protects* and a threshold ruling; 1440 is unaffected because no subscription-backed candidate is ever built.

### Filter automatic candidates by maximum marginal routing cost. (line 1436)

Contract: Given a project whose pricing.toml prices a metered model and whose effective [routing] max_marginal_cost (micro-USD, default 1000) is below what one bounded classification call to that model would cost, when Glasshouse chooses a classifier automatically, Glasshouse excludes that candidate with a reason naming its estimated cost and the ceiling and chooses among the rest -- while preserving that a free candidate is never priced out, that a metered candidate with no price in the table is unpriced (noted, never excluded, exactly as an unmeasured latency is), that a ceiling of zero admits only free candidates, that no ordering among admitted candidates changes, and that glasshouse route's explanation carries the exclusion.

State: **COMPLETE** — ruled 2026-09-02 (evening) by the orchestrator after reading the estimate and the gate in the worktree. Amber tier: 4/4 mutations KILLED with output, every acceptance target run one at a time with counts (the worker caught practice §68's trap in the packet's own combined command and refused it), targeted blast exit 0.

Production evidence:
- `crates/glasshouse/src/routing/disposable.rs` — `DisposableCandidate::with_price`
- `crates/glasshouse/src/routing/disposable.rs` — `ClassificationPolicy::with_max_marginal_cost_micro_usd`
- `crates/glasshouse/src/routing/disposable.rs` — `estimated_classification_cost_micro_usd`
- `crates/glasshouse/src/routing/disposable.rs` — `classification_verdict`
- `crates/glasshouse/src/main.rs` — `automatic_classification_choice`
- `crates/glasshouse/src/main.rs` — `attach_prices`

Regression evidence:
- `classification_cost_ceiling::an_overpriced_metered_candidate_is_excluded_and_a_cheaper_one_is_chosen`
- `classification_cost_ceiling::the_default_ceiling_admits_a_model_a_stricter_one_would_exclude`
- `classification_cost_ceiling::free_and_unpriced_candidates_are_admitted_with_distinct_notes`
- `classification_cost_ceiling::a_zero_ceiling_admits_only_free_candidates`
- `classification_cost_ceiling::the_estimate_uses_the_task_text_ceiling_not_zero`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| if estimate > u64::from(ceiling) { -> if false { | `ceiling-ignored` | **killed** | `classification_cost_ceiling::an_overpriced_metered_candidate_is_excluded_and_a_cheaper_one_is_chosen` |
| candidate.with_price(price) -> candidate.with_price(None) | `price-not-attached` | **killed** | `classification_cost_ceiling::an_overpriced_metered_candidate_is_excluded_and_a_cheaper_one_is_chosen` |
| match candidate.cost { -> match Cost::Metered { | `free-priced-as-metered` | **killed** | `classification_cost_ceiling::free_and_unpriced_candidates_are_admitted_with_distinct_notes` |
| CLASSIFICATION_PROMPT_CONTRACT.len() + TASK_TEXT_CEILING_BYTES -> CLASSIFICATION_PROMPT_CONTRACT.len() + 0 | `estimate-uses-actual-text` | **killed** | `classification_cost_ceiling::the_estimate_uses_the_task_text_ceiling_not_zero` |

> ceiling-ignored observed: panicked at crates/glasshouse/tests/classification_cost_ceiling.rs:219:5 (exclusion-note assertion not found); also failed a_zero_ceiling_admits_only_free_candidates at :357

> price-not-attached observed: panicked at crates/glasshouse/tests/classification_cost_ceiling.rs:219:5; also failed a_zero_ceiling_admits_only_free_candidates at :357

> free-priced-as-metered observed: panicked at crates/glasshouse/tests/classification_cost_ceiling.rs:289:5; also failed a_zero_ceiling_admits_only_free_candidates at :351

> estimate-uses-actual-text observed: assertion left == right failed: a known price must give a known micro-USD figure, computed from the task-text ceiling

Recorded scope limits — stated by the worker, not discovered later:
- estimated_classification_cost_micro_usd is a bytes/4 approximation, not a real tokenizer
- does not decide 1419 (premium capacity threshold) or 1439 (time-vs-price exchange rate)
- does not build a subscription-backed candidate for 1440
- ranking among admitted candidates is untouched -- price is a filter only, not a score term

---

## REVIEW — the orchestrator owes an answer to each of these

This section is the point of the generator. Everything above is the
worker's facts, transcribed. Nothing below is decided.

- **1436** — verdict `closed`. Re-run one decisive mutation yourself, then rule (§79: a worker's packet does not bind the integrator).

**Packet errors the worker reported — read these BEFORE its results.**
Thirteen consecutive rounds a worker corrected its packet and was right:
- shell::state::format_usd renders six fraction digits (micro-USD exact), not four as the packet's OBJECTIVE 4 states (shell/state.rs:4780); reproduced the actual six-decimal shape in a local format_micro_usd instead of importing the config-coupled function
- the packet's VERIFICATION COMMANDS example (--test A --test B ... --lib routing::disposable) is practice §68's trap: the trailing --lib filter is a global test-name substring filter, so every named --test target reports '0 passed ... N filtered out' under that exact command; ran each target separately (all 84 tests genuinely green)

Gates the worker ran (re-run the decisive ones yourself):
- cargo fmt --all -- --check: clean
- cargo clippy -p glasshouse --all-targets --all-features -- -D warnings: clean
- cargo doc -p glasshouse --no-deps: clean (after fixing 3 private-intra-doc-link errors)
- cargo test -p glasshouse --test classification_cost_ceiling: test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
- cargo test -p glasshouse --test classification_call: test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
- cargo test -p glasshouse --test launch_classification: test result: ok. 24 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
- cargo test -p glasshouse --test routing_model_config: test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
- cargo test -p glasshouse --test routing_pricing: test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
- cargo test -p glasshouse --test routing_disposable_tier: test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
- cargo test -p glasshouse --lib routing::disposable: test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 2019 filtered out
- scripts/blast-radius.sh --targeted <3 changed files>: exit 0, every traced target passed



---

## Line 1439 CLOSED — 2026-09-02 (`GH-CLASSIFIER-TIME-PRICE`, Amber, Sonnet high): a preference that the worker first proved could never fire, re-ruled and then real

**The first ruling was vacuous, and the worker's proof is the reason to record it.** `design-decisions.md`'s *Preferring a cheap metered classifier over an unreliable free one* first said *unreliable enough* meant expected wasted time — `(1 − parsed) × median` — over `max_router_latency`. But wasted time is never more than the median, and line 1435 already excludes a candidate whose median exceeds that same limit (with 1432's 80 % parse floor, an admitted free candidate wastes at most a fifth of the limit by construction). So the rule could fire only on a candidate the verdict had already removed: an account of an exclusion, never a preference. The worker's first implementation (report `…-v1-superseded`) built a seam over the pre-admission list to make it observable at all; the orchestrator withdrew it and amended the ruling the same evening: **the comparison is between two times** — the free candidate's expected wasted retry time against the **metered candidate's own median classification latency** over the same record floor — with *cheap enough* unchanged (estimated call cost at or below `max_marginal_cost`), asked over the **admitted** list only, so what it prefers passed every other gate and, when it fires, the choice changes. `max_router_latency` plays no part.

**What landed** (`routing/disposable.rs`): `time_price_preference(policy, free, metered)` reads both candidates' `ClassificationRecord`s — either missing, below `CLASSIFICATION_RELIABILITY_MIN_OBSERVATIONS`, or without a median → inert with a note naming which; `wasted_ms = (1 − parsed/outcomes) × free_median` compared to `metered_median`; then the cost half. `cheapest_priced_metered` picks the cheapest admitted priced metered candidate (ties by order). `DisposableRouting::time_price_seam` asks the rule about the free candidate `choose` would pick from the admitted list, **before** the retained-pick reuse, so a retained free pick whose inputs now fire is overridden; the *time versus price* contribution (magnitude 0, both figures and both limits in the `why`) rides whichever choice is made. **The worker's second finding, accepted:** because both candidates come from the admitted list, the metered candidate's `estimate ≤ ceiling` is exactly 1436's admission, so the rule's own over-ceiling arm is unreachable in production and the reachable cost-side inert path is the unconfigured ceiling (*no maximum marginal cost is configured — no candidate is cheap enough*); test (c) and the cost mutation target that path, and the redundant arm stays for the function's own contract.

### Prefer a cheap metered model over an unreliable free model when failed routing attempts would cost more time than the price difference. (line 1439)

Contract: Given automatic classification with a free candidate whose classification record shows a parsed fraction and a median latency over the sample floor, and an admitted priced metered candidate whose own classification record shows a median latency over the same floor, when the free candidate's expected wasted time -- (1 - parsed_fraction) x median_ms -- exceeds the metered candidate's own median classification latency, and the metered candidate's estimated call cost is at or below the effective [routing] max_marginal_cost, Glasshouse chooses the metered candidate and its explanation names both figures with map line 1439 -- while preserving that nothing moves when either condition fails or either candidate's record is unmeasured, below the floor, or without a median, that [routing] max_router_latency plays no part (that stays 1435's alone), that the free selection is never reordered among free candidates, that every existing exclusion still applies first, and that the retained pick (1441/1442) is honoured as before unless this preference newly fires.

State: **COMPLETE** — ruled 2026-09-02 (late evening) by the orchestrator, on the AMENDED rule and after reading `time_price_preference` and `time_price_seam` in the worktree. Amber tier: 4/4 mutations KILLED against the amended comparison with output; every target run singly with counts; targeted blast green; test (e) proves the preference changes a real decision through the shipped binary (a retained free pick overridden once the metered candidate's own record measured). The first ruling's vacuity is recorded above so nobody re-derives it.

Production evidence:
- `crates/glasshouse/src/routing/disposable.rs` — `time_price_preference`
- `crates/glasshouse/src/routing/disposable.rs` — `cheapest_priced_metered`
- `crates/glasshouse/src/routing/disposable.rs` — `DisposableRouting::time_price_seam`
- `crates/glasshouse/src/routing/disposable.rs` — `DisposableRouting::choose_for_automatic_classification`

Regression evidence:
- `classification_time_price::an_unreliable_free_candidate_is_passed_over_for_a_faster_cheap_metered_one`
- `classification_time_price::a_free_candidate_is_kept_when_its_wasted_time_is_within_the_metered_candidates_own_latency`
- `classification_time_price::a_free_candidate_is_kept_when_no_marginal_cost_ceiling_is_configured`
- `classification_time_price::a_free_candidate_below_the_sample_floor_is_treated_as_unmeasured_by_the_preference`
- `classification_time_price::a_parsed_fraction_of_one_wastes_zero_time`
- `classification_time_price::a_retained_free_pick_whose_inputs_now_fire_the_rule_is_overridden_and_not_reused`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| if wasted_ms <= metered_median_ms { -> if false { | `always-prefer-metered` | **killed** | `classification_time_price::a_free_candidate_is_kept_when_its_wasted_time_is_within_the_metered_candidates_own_latency (and a_parsed_fraction_of_one_wastes_zero_time)` |
| let Some(max_cost) = policy.max_marginal_cost_micro_usd else { -> let Some(max_cost) = Some(policy.max_marginal_cost_micro_usd.unwrap_or(u32::MAX)) else { | `ignore-the-cost-ceiling` | **killed** | `classification_time_price::a_free_candidate_is_kept_when_no_marginal_cost_ceiling_is_configured` |
| if free_record.outcomes_recorded < CLASSIFICATION_RELIABILITY_MIN_OBSERVATIONS { -> if false { | `floor-ignored` | **killed** | `classification_time_price::a_free_candidate_below_the_sample_floor_is_treated_as_unmeasured_by_the_preference` |
| let wasted_ms = ((1.0 - fraction) * free_median_ms as f64).round() as i64; -> let wasted_ms = free_median_ms; | `wasted-time-uses-median-alone` | **killed** | `classification_time_price::a_free_candidate_is_kept_when_its_wasted_time_is_within_the_metered_candidates_own_latency (and three others)` |

> always-prefer-metered observed: assertion left == right failed: a parsed fraction of 1.0 wastes no time ... free alpha-model expects 0ms of wasted retries per call, over metered beta-model's own 1ms median classification latency; metered beta-model at ~$0.000007 per call is under the $1.000000 ceiling

> ignore-the-cost-ceiling observed: assertion left == right failed: with no cost ceiling configured, no candidate is ever cheap enough ... metered beta-model at ~$0.000007 per call is under the $4294.967295 ceiling (retargeted from the estimate > max_cost line, which is unreachable once both candidates come from the admitted list -- see report body)

> floor-ignored observed: the 'unmeasured: 2 of 3 ... fewer than the 5 needed' assertion no longer matched -- the mutated code fell through to the no-median branch instead

> wasted-time-uses-median-alone observed: four of the six tests failed at once -- every test whose figure depends on the (1 - fraction) factor

Recorded scope limits — stated by the worker, not discovered later:
- time_price_preference's own estimate > max_cost comparison is unreachable through its only caller (cheapest_priced_metered only ever returns admitted, hence already-cheap-enough, candidates); kept for the function's own caller-independent correctness, not exercised by production traffic
- the metered candidate's own entitlement job-constraint (map line 1947) and the protected-reserve/headroom gates (map lines 1434, 1550) are not independently re-checked by this preference -- inherited automatically now, since both candidates come from the same classification-admitted list choose's own hard-constraint and headroom filtering already produced
- does not decide 1419 (premium capacity threshold) or 1440 (no subscription-backed classification candidate exists in this build)

---

## REVIEW — the orchestrator owes an answer to each of these

This section is the point of the generator. Everything above is the
worker's facts, transcribed. Nothing below is decided.

- **1439** — verdict `closed`. Re-run one decisive mutation yourself, then rule (§79: a worker's packet does not bind the integrator).

**Packet errors the worker reported — read these BEFORE its results.**
Thirteen consecutive rounds a worker corrected its packet and was right:
- Test (c)'s literal packet wording ('metered over the cost ceiling -> free kept') is unconstructible once both candidates in the comparison must be classification-admitted: an admitted metered candidate's estimate <= ceiling is exactly map line 1436's own admission condition, so an over-ceiling admitted metered candidate cannot exist. Built test (c) around the still-valid 'a None ceiling means no candidate is cheap enough' inert path instead, and retargeted the ignore-the-cost-ceiling mutation at that reachable check.

Gates the worker ran (re-run the decisive ones yourself):
- cargo fmt --all -- --check: clean
- cargo clippy -p glasshouse --all-targets --all-features -- -D warnings: clean
- cargo test -p glasshouse --test classification_time_price: test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
- cargo test -p glasshouse --test classification_cost_ceiling: test result: ok. 5 passed; 0 failed
- cargo test -p glasshouse --test classification_call: test result: ok. 10 passed; 0 failed
- cargo test -p glasshouse --test launch_classification: test result: ok. 24 passed; 0 failed
- cargo test -p glasshouse --test routing_model_config: test result: ok. 5 passed; 0 failed
- cargo test -p glasshouse --lib routing::disposable: test result: ok. 20 passed; 0 failed; 2022 filtered out
- cargo test -p glasshouse --test routing_economics (extra, not in packet list): test result: ok. 22 passed; 0 failed
- scripts/blast-radius.sh --targeted crates/glasshouse/src/routing/disposable.rs crates/glasshouse/tests/classification_time_price.rs: every traced target passed

