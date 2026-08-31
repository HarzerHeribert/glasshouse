# Phase 34A — Workload tiers, 7 of 10 closed

Capability map lines 1395–1400 and 1404. Package `GH-WORKLOAD-TIERS`, worktree
`.worktrees/workload-tiers`; report in `.agent-runtime/report-workload-tiers.md`.
Integrated 2026-08-29 with `GH-SESSION-CONTEXT` in one `integrate.sh` run.

**1401–1403 are NOT closed and were deliberately excluded from the packet.**
They ask a workload tier to express *required capabilities*, which needs the
Phase 34 capability registry and a task-side capability vocabulary. See
`docs/product/evidence/phase-34.md`.

## What landed

`WorkloadTier` went from three variants to five: `Deterministic` (Tier 0)
before `Leaf`, and `Frontier` (Tier 4) after `Heavy`, in declaration order so
the derived `Ord` gives `Deterministic < Leaf < Standard < Heavy < Frontier`.

**`Leaf`, `Standard` and `Heavy` were not renamed.** The orchestrator's mapping
ruling (`Leaf → Tier 1`, `Standard → Tier 2`, `Heavy → Tier 3`) is realised by
repositioning the three existing variants inside a five-variant enum. The
worker's reasoning, which I accept: those names are referenced from four test
files outside the packet's scope, and a rename would have forced edits into
another worker's partition to achieve nothing the repositioning does not.

## The defect this package existed to avoid creating

`provider/quota.rs` compared `inputs.tier == WorkloadTier::Heavy` to decide
whether a task may spend protected reserve. **Adding a tier above `Heavy` with
that equality in place would have made `Frontier` compare unequal and fall
through to `ReserveDecision::Deny`** — the strongest work in the system losing
exactly the reserve it most justifies, silently, with every test still green.

Both comparisons are now thresholds (`>= Heavy`, `< Heavy`), and the mutation
that matters reverts one:

| mutation | result | killed by |
|---|---|---|
| `quota.rs` `if inputs.tier >= WorkloadTier::Heavy` → `== WorkloadTier::Heavy` | **killed** | `workload_tiers::frontier_tier_justifies_spending_the_reserve_at_line_1290` |
| `quota.rs` distant-reset `<` → `!=` | **killed** | `workload_tiers::frontier_tier_survives_the_distant_reset_threshold` |
| enum declaration order of `Heavy`/`Frontier` swapped, so `Ord` lies | **killed** | `workload_tiers::escalate_never_steps_down_for_any_tier` |

The worker confirmed each kill was behavioural rather than §80's
false-KILLED-by-non-compile: the `test result:` line showed 12 tests compiled
and ran with 1 failed, not a build error.

## The orchestrator's ruling on the four definitional lines

`scripts/evidence_from_report.py` **refused** this report on lines 1396, 1397,
1398 and 1404 — `verdict: closed` with no killed mutation attached to that
line, §14's trap. I am closing them anyway, and this is the reasoning rather
than a bypass:

These four are **definitional**. "Define Tier 1 as lightweight classification,
extraction, reranking, formatting, and simple factual codebase lookup" asks for
a named, ordered, documented position in the tier system. A mutation cannot
bite a doc comment. What *can* be mutated is the ordering the definitions
depend on — and the `Ord`-lie mutation above does exactly that, for all five
tiers at once, and was killed. That is the decisive mutation for the whole
definitional set; attaching a copy of it to each line would not add evidence.

1404 ("short, inspectable, and configurable rather than opaque proprietary
scores") is closed on the same basis plus the shape of the code: an ordered
`enum` with `as_str()`, `Display`, and doc comments naming what each tier
*means*, with no numeric score anywhere.

## The limit, stated rather than discovered later

**Tier 0 (`Deterministic`) and Tier 4 (`Frontier`) have no producer.** Nothing
classifies work into either one; `classify_heuristically` still emits only the
middle three. This is correct and was explicitly authorised by the packet —
this project adds variants as producers land, never in advance
(`evaluation/mod.rs:89` states the same rule for its own enum) — but it means
these two tiers are a vocabulary, not yet a behaviour.

Tier 4 is the partial exception and the reason the package is worth more than
a vocabulary: it is *consumed* today. A `Frontier` task reaching the reserve
policy is allowed to spend protected reserve, and that path is
mutation-proven above. Tier 0 is consumed by nothing.

### Lines 1401–1403 — required capabilities independent of raw intelligence

Package `GH-TIER-CEILING`, 2026-08-31, Opus at high. Nine mutations, nine killed. The worker **refused OBJECTIVE 3** — attaching adapter-declared `ResourceFacts` to destinations — and the orchestrator verified the refusal: `capability_fit` (`routing/session.rs:786`) already reads `adapter_for(destination.harness())` and `prefer()` falls through to those declarations whenever the facts are `Unverified`, so the wiring would have changed no score and survived its own mutation; `Destination::with_resource_facts` keeps no production caller, deliberately. 1402's large-context half is recorded as a limit: nothing produces `large_context` today.


### Allow workload tiers to express required capabilities independently from raw model intelligence. (line 1401)

Contract: Given a classified task, when Glasshouse ranks destinations, the required workload tier and the required hard capabilities act as two independent inputs -- a destination can be admitted on tier and penalised on capability, or the reverse -- while preserving that neither is inferred from the other.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts.

Production evidence:
- `src/routing/session.rs` — `TaskRequirements { minimum_tier, hard_capabilities }`
- `src/routing/session.rs` — `workload_tier_fit and capability_fit as separate contributions`
- `src/main.rs` — `destination_tier_ceiling (the tier input's producer)`

Regression evidence:
- `tier_ceiling::a_hard_capability_outranks_raw_model_cheapness_at_a_lower_tier`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| main.rs: the with_tier_ceiling producer severed (mutation 1 above) | `skip-state-update` | **killed** | `tier_ceiling::a_hard_capability_outranks_raw_model_cheapness_at_a_lower_tier` |

> skip-state-update observed: the tier term collapses to 0.000 for both candidates while the capability term keeps its +0.400/0.000 split -- which is the independence this line names, demonstrated by removing one axis and watching the other survive

Recorded scope limits — stated by the worker, not discovered later:
- Independence is shown for the pair (tier, hard capability). Nothing here shows the tier is independent of cost or health, which are separate terms with their own evidence.


---


### Allow a task to require a lower reasoning tier but a specific capability such as browser use or a very large context window. (line 1402)

Contract: Given a task classified at a lower reasoning tier that nonetheless requires a specific harness capability, when Glasshouse ranks destinations it prefers the destination that declares the capability over one with a higher ceiling that does not -- while preserving that a resource declaring nothing about the axis is scored as not established rather than as absent.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts.

Production evidence:
- `src/routing/session.rs` — `capability_fit`
- `src/routing/capability.rs` — `ResourceCapabilities::describe / axis_for (unchanged)`
- `src/main.rs` — `destination_tier_ceiling`

Regression evidence:
- `tier_ceiling::a_hard_capability_outranks_raw_model_cheapness_at_a_lower_tier`
- `routing_capability::an_unverified_axis_scores_strictly_better_than_an_established_absent_one`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| main.rs: the with_tier_ceiling producer severed (mutation 1 above) | `bypass-fallback` | **killed** | `tier_ceiling::a_hard_capability_outranks_raw_model_cheapness_at_a_lower_tier` |

> bypass-fallback observed: the standard-tier run's tier terms both read `nothing has established ...'s ceiling`, and the two-condition experiment's required-run assertions fail at tier_ceiling.rs:405 and below

Recorded scope limits — stated by the worker, not discovered later:
- The line names browser use OR a very large context window. The capability half is closed on real harness declarations (codex code_editing verified present vs opencode Unverified). The LARGE-CONTEXT half has no producer: large_context, fast_cheap_analysis and repository_review are the three ResourceFacts axes no harness declares, axis_for maps no HardCapability to any of them, and Destination::with_resource_facts still has no production caller.


---


### Allow a task to require a minimum harness capability even when a cheap raw model would otherwise score highly. (line 1403)

Contract: Given a required harness capability and a cheaper destination that does not declare it, when Glasshouse ranks them it chooses the capable destination even though the cheap one is free and has more raw headroom -- while preserving that the price preference still applies among candidates the capability term could not separate.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts.

Production evidence:
- `src/routing/session.rs` — `capability_fit (CAPABILITY_ESTABLISHED_PRESENT 0.4 against CAPABILITY_UNVERIFIED 0.0)`
- `src/routing/session.rs` — `cost_preference (METERED_COST_PREFERENCE -0.1, four times smaller)`

Regression evidence:
- `tier_ceiling::a_hard_capability_outranks_raw_model_cheapness_at_a_lower_tier`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| routing/session.rs: `const METERED_COST_PREFERENCE: f64 = -0.1;` -> `= 0.0;` | `remove-guard` | **killed** | `tier_ceiling::a_hard_capability_outranks_raw_model_cheapness_at_a_lower_tier` |

> remove-guard observed: assertion `left == right` failed: with nothing required beyond a leaf tier, the free destination is the one to take -- the control condition, which is what shows price is a live term this line has to outrank rather than a dead one

Recorded scope limits — stated by the worker, not discovered later:
- Cheapness here is Cost::Free vs Cost::Metered from the user's own free_models list. No marginal-price estimate exists (Phase 32G), so `cheap` is a two-state fact, not a magnitude.

