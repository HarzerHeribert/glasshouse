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
