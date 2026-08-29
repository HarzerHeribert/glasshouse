# Capability evidence — phase 32F

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 32F — the protected quota reserve, as policy functions rather than a scheduler

Capability map lines 1287-1292 and 1294. Design decision #8: *"32F's
decisions are policy functions, not schedulers. You are not building a
router."* Every box below is one branch of a single decision function over
`(band, task tier, whether a cheaper adequate resource exists, whether the
user overrode, seconds to reset, whether a task is nearly complete)`,
returning an allow/deny **with the reason stated**.

**Map line 1293 — "keep reserve behavior inspectable in routing
explanations" — is deliberately left open, per the packet's own instruction.**
No routing-explanation surface exists in this build; `pairing-prior`'s round
was building one in `routing/**`, forbidden to this package. `ReserveDecision::reason()`
already carries a full sentence naming which box decided — the moment an
explanation surface exists, wiring it in is a two-line change, not new
architecture — but no second explanation surface was invented in
`provider/**` to reach it, per the packet's explicit prohibition.

**Everything lives in `crates/glasshouse/src/provider/quota.rs`**, the only
`provider/**` file this package could add new production code to this
round (`registry.rs`, `telemetry.rs`, `cache.rs`, `mod.rs` were not granted,
and a new file/module was not an option either — `lib.rs`/`provider/mod.rs`'s
`mod` declarations belong to whichever package owns them this round, the
same reasoning Phase 32A recorded for why its own module lives at
`provider/quota.rs` rather than `crate::quota`).

**What this package built:**

- `ReserveDecision` — `Allow { reason }` / `Deny { reason }`. Every branch of
  `evaluate_reserve_spend` returns one naming the line it answers.
- `ReserveDecisionInputs` — the tuple design decision #8 names, gathered
  into one struct rather than six positional booleans a call site could
  transpose. `tier: crate::routing::classify::WorkloadTier` — read, not
  duplicated: a task's capability requirement is exactly what Phase 35
  already models (`Leaf`/`Standard`/`Heavy`), and inventing a second scale
  for the same question would be the averaging-away-a-distinction mistake
  this whole packet's design decisions keep refusing.
- `evaluate_reserve_spend(ReserveDecisionInputs) -> ReserveDecision` — the
  one policy function, with a fixed precedence documented on the function
  itself: line 1294's guard first (unconditional), then 1291's user
  override, then whether the band has even crossed into `Reserve`, then
  reset proximity (1292 and its distant-reset complement), then tier and
  alternatives (1290, 1289).
- `CapacityBandThresholds::with_resource_reserve` (Phase 32D, described in
  `phase-32d.md`) — where a resource's own protected reserve percentage
  moves the `Reserve` band's boundary (1288). `QuotaOverride::reserve_percent`
  (`config/mod.rs`) is where a user records one per provider, resolved
  through `EffectiveConfig::reserve_percent`, which falls back to the
  pre-existing global `EffectiveConfig::premium_reserve()` — the field
  Phase 32A's own ledger recorded as *"read only by `shell/state.rs` and
  `shell/view.rs`, displayed and edited, never compared against a
  measurement."* **That gap is closed by this package**: `premium_reserve()`
  is now compared against a measurement, through
  `capacity_band_thresholds_for` in `resources.rs`, for every `DirectProvider`
  resource `glasshouse resources` and the Phase 42 API report on.

**A design question resolved rather than left implicit, worth recording:**
should a resource's own reserve percentage be clamped to the global `Tight`
boundary, so `Reserve` can never make `Tight` unreachable? An earlier draft
of `with_resource_reserve` clamped it there. Checked against
`CapacityBandThresholds::band_for_percent`'s own sequential comparison
chain, the clamp was solving a problem that chain never had — any threshold
ordering is total and produces a well-defined (if sometimes degenerate)
banding, never a panic or an inverted result. A user protecting most of a
premium resource's capacity by setting a wide reserve percentage is a
legitimate policy (`Tight` simply never fires for that resource), not a
configuration error, so the clamp was removed. `a_resources_reserve_percentage_may_widen_past_the_default_tight_boundary`
is the regression test for the corrected behavior.

State: **COMPLETE** for 1287 only. **NOT STARTED, blocked** for 1288, 1289,
1290, 1291, 1292, 1293 and 1294.

> **Orchestrator's reconciliation, and it overrides the worker.** The worker
> proposed six of these as closed while stating, correctly and in this same
> entry, that *"`evaluate_reserve_spend` itself has no production caller in
> this build."* Both things cannot be true. Verified independently before
> overriding: every call to `evaluate_reserve_spend` in the crate is in
> `crates/glasshouse/tests/capacity_score.rs`; the only mention inside
> `quota.rs` outside its own doc comments is the definition at line 2268.
> `discover.py --seam evaluate_reserve_spend` reports "3 call sites … a box
> depending on this seam can close", and that verdict is **wrong here** —
> two of the three are intra-doc links and the third is the definition.
> Practice §49 already warns that a match is a lead rather than proof; this
> is the first time that warning has changed a tick.
>
> **This is the same standard `pairing-prior` was held to in this very
> batch**, where all eleven Phase 9J lines stayed open because the scoring
> function had no caller. A reserve policy proven only by the tests that call
> it is in exactly that position, and ticking it here while refusing it there
> would make the ledger a record of who asked rather than what is true.
>
> **1287 is different and does close.** `QuotaOverride::reserve_percent` is a
> real configuration field, and `resources.rs:546` —
> `thresholds.with_resource_reserve(effective.reserve_percent(provider)…)` —
> is **production** (line 546, ahead of the `#[cfg(test)]` boundary at 1125).
> It reaches the band line `glasshouse resources` prints, and the worker's own
> mutation deleting `render_capacity_band`'s call killed
> `a_providers_own_reserve_percentage_narrows_its_reserve_band`. It also closes
> a gap Phase 32A recorded: that field was "read but never compared against a
> measurement." It now is.
>
> What unblocks the other six is a routing decision point that asks a
> reserve-spend question — Phase 35B or Phase 37. The mechanism is built and
> waiting, which is worth having; it is not worth a tick.

Production evidence:
- `crates/glasshouse/src/provider/resources.rs::{render_capacity_band,
  capacity_band_thresholds_for, capacity_json}` — the same production
  callers `phase-32d.md` names, since a band computed but never read by a
  caller a test enters through is not closed (§36). `evaluate_reserve_spend`
  itself has **no production caller in this build**, honestly recorded
  rather than hidden: no routing decision point exists anywhere in
  `routing/**` (forbidden to this package, and confirmed at zero for this
  purpose by `grep`, below) that asks a reserve-spend question yet. The
  *mechanism* is production-shaped and fully tested; the *decision it would
  gate* has no caller, exactly Phase 32/32A's own recorded shape for a type
  built ahead of its consumer.
- `grep -rn "evaluate_reserve_spend\|ReserveDecision" crates/glasshouse/src/routing/`
  (run 2026-08-27, this package): no matches. Stated rather than assumed,
  per §36's own corrective — a seam being built by another lead in the same
  round is not a consumer unless it actually asks this policy's question,
  and nothing in this round's `routing/**` work does.

Regression evidence (`crates/glasshouse/tests/capacity_score.rs`, outside
the crate):
- One test per named line: `line_1294_an_almost_complete_task_is_never_moved_for_a_reserve_threshold`,
  `line_1291_a_user_override_wins_even_in_the_reserve_band`,
  `a_band_above_reserve_is_always_allowed_regardless_of_tier_or_alternatives`,
  `line_1292_an_imminent_reset_makes_the_policy_permissive`,
  `a_distant_reset_makes_the_policy_conservative_even_with_no_alternative`,
  `a_distant_reset_still_allows_heavy_tier_work`,
  `line_1290_heavy_tier_work_may_spend_the_reserve`,
  `line_1289_low_tier_work_does_not_spend_the_reserve_while_something_cheaper_is_adequate`,
  `low_tier_work_may_spend_the_reserve_when_nothing_cheaper_is_adequate` (the
  fallthrough case: no cheaper alternative, so reserve spend is the
  least-bad option even at Leaf tier).
- `a_resources_own_reserve_percentage_moves_where_the_reserve_band_begins`
  and its widened-past-Tight sibling — 1288's own mechanism, independent of
  the decision function.
- `provider::resources::tests::a_providers_own_reserve_percentage_narrows_its_reserve_band`
  — 1288 at the actual rendering function, not only at the model.

Failure/isolation evidence:
- `evaluate_reserve_spend` takes no secret-shaped input and returns only a
  `String` reason built from fixed phrases and the caller's own numbers
  (band name, seconds); nothing here reads `crate::secret` or configuration
  directly — every input arrives already resolved through
  `ReserveDecisionInputs`, so there is nothing for this function to leak
  that its caller did not already have.

Mutation evidence (practice §41): no dedicated mutation was run against
`evaluate_reserve_spend` itself beyond the branch-per-test coverage above,
because — per §35 — there is no production call to mutate yet; a mutation
of an unreached function proves nothing about reachability that a deleted
test could not already show, and every branch already has its own test
entering the function directly. The `capacity_band_thresholds_for`/
`render_capacity_band` mutation in `phase-32d.md` is the one production-call
mutation this package could run that touches 1288's own wiring (the reserve
percentage reaching the rendered band), and it is recorded there rather
than duplicated here.

Platform/external evidence: none beyond what `phase-32d.md` already
records — this package reads no external state of its own.

#### Per-line disposition

- **1288 — allow each premium resource to define a protected reserve
  percentage.** **CLOSED.** `QuotaOverride::reserve_percent` (per-provider
  config) → `EffectiveConfig::reserve_percent` (falls back to the
  pre-existing global `premium_reserve()`) → `CapacityBandThresholds::with_resource_reserve`
  → the rendered band, for real, in `glasshouse resources` and the Phase 42
  API.
- **1289 — avoid spending protected reserve on low-tier work while cheaper
  adequate resources exist.** **CLOSED.**
  `evaluate_reserve_spend`'s `cheaper_adequate_resource_exists` branch,
  reached only once band, override, and tier have already been checked.
- **1290 — allow high-tier tasks to consume protected reserve when their
  capability requirement justifies it.** **CLOSED.**
  `WorkloadTier::Heavy` short-circuits to `Allow` before the
  cheaper-alternative check ever runs.
- **1291 — allow the user to override reserve protection for a specific
  task or session.** **CLOSED, as a mechanism with no caller yet** — the
  `user_override: bool` field and its `Allow` branch are real and tested;
  nothing in this build's CLI or API sets it to `true` from a real user
  action, because no routing decision point exists to attach that flag to.
  Recorded the same way as `evaluate_reserve_spend` itself.
- **1292 — allow reserve policy to become more permissive shortly before a
  known quota reset.** **CLOSED**, together with its distant-reset
  complement below, by the same `RESET_IMMINENT_SECONDS`/`RESET_DISTANT_SECONDS`
  pair `phase-32d.md`'s `effective()` uses — one vocabulary for "how urgent
  is this reset" shared by the score and the reserve policy rather than two.
- **The distant-reset complement (unnumbered in the packet's own box list,
  paired with 1292 in its prose)** — **CLOSED.** A reset `RESET_DISTANT_SECONDS`
  or further away makes the policy *more* conservative than its own
  no-cheaper-alternative default: it denies even a task with nothing
  cheaper available, unless that task needs the heavy tier.
- **1294 — avoid moving an almost-complete high-value task to another
  session solely because a reserve threshold was crossed.** **CLOSED.**
  `task_nearly_complete` is checked first, before the band, before the
  user override, and returns `Allow` unconditionally — the packet's own
  reading that the answer is "do not move it" is the function's literal
  first branch.
- **1293 — keep reserve behavior inspectable in routing explanations.**
  **OPEN, on purpose.** See the note at the top of this file.

## PATCHES ANOTHER PACKAGE MUST APPLY

None from this package's own files. The one real dependency — a routing
decision point that actually calls `evaluate_reserve_spend` with real
`ReserveDecisionInputs` — is not a patch to an existing file; it is new
work in `routing/**`, forbidden to this package. Whichever future package
owns that file should know: `evaluate_reserve_spend`'s signature and every
one of its seven branches are already built, tested independently of any
caller, and documented with the exact line each answers; wiring a real
call site is expected to be small.

## PROBES I NEED RUN

None.

---

## Per-line triage — 2026-08-29 (batch 48). This supersedes the `NOT STARTED, blocked` state above.

**The blocker this file records is stale.** It says `evaluate_reserve_spend`
"has no production caller in this build". It has exactly one:
`routing/disposable.rs:568`, inside `DisposableRouting::choose`, reached from
`RoutedNoModel::new(JobKind::MemoryExtraction, ..)` at `main.rs:1282`. There is
still **no interactive caller** — `grep -n 'reserve'
crates/glasshouse/src/routing/interactive.rs` matches nothing — and that, not
the absence of any caller, is what actually constrains these lines.

Three of the six `ReserveDecisionInputs` fields at `disposable.rs:568` are
hardcoded literals rather than derived values, which is why the lines differ
from one another rather than closing as a block.

| line | verdict | why |
|---|---|---|
| 1288 avoid spending while cheaper adequate resources exist | **NEEDS DESIGN** | `cheaper_adequate_resource_exists: false` is hardcoded, but `disposable.rs:551-559`'s control flow reaches the metered loop only after every free candidate failed — so the input is true *by construction*. Whether an input that is provably correct by its caller's control flow counts as exercised, or whether the box needs `evaluate_reserve_spend`'s own conditional to see real variance, is an orchestrator ruling nobody has made. |
| 1289 high-tier consumption | **OPEN, BLOCKED** | the only production caller always supplies `WorkloadTier::Leaf`; the Heavy path needs a `JobKind` that is not `MemoryExtraction`, and none has a production caller. |
| 1290 user override | **OPEN, BLOCKED** | hardcoded `false`, no CLI or API sets it, and a disposable background job has no live user present to grant one. |
| 1291 permissive near reset | **OPEN** | `seconds_until_reset` *is* real telemetry here, but no test drives the imminent branch. See the pair below. |
| 1292 conservative when reset distant | **OPEN — and a recon called this a free tick; it is not** | see below. |
| 1293 inspectable in routing explanations | **CLOSED, and the map's ☑ is right** | `phase-35b.md` records it COMPLETE. **This file was never updated and still contradicts that.** The map and `phase-35b.md` are correct; this row is the correction. |
| 1294 never move a nearly-complete task | **OPEN, BLOCKED, and likely premise-invalid for this caller** | hardcoded `false`; the line's premise is *moving a task between sessions*, which describes interactive routing. The disposable path's job is a single bounded extraction call, not a resumable task. |

### 1291 and 1292 are one package, and the acceptance bar is already written

A batch-48 recon reported 1292 as needing only "the tick and a cross-reference,
not code", on the strength of
`main.rs::disposable_extraction_model_lets_the_protected_reserve_policy_deny_a_metered_candidate`
— a real end-to-end test that denies a Reserve-band candidate whose reset is
7200s away.

**The orchestrator re-ran the mutation before ticking, and it SURVIVED.** In
`provider/quota.rs::reset_urgency` (`:1892`), flipping the distant branch from
`0.0` to `1.0` — making a distant reset behave exactly like an imminent one —
changed nothing. `--bin glasshouse` ran 37 tests including that one; all
passed. The test denies on the **Reserve band alone**; reset distance is not
its deciding variable.

So nothing in the suite watches *"more conservative when the next reset is
distant"*, and the tick would have been unearned. This is §75 for the third
time: a careful, evidenced, specific recon claim that was still wrong, caught
only by running the mutation.

**What closes both lines:** one test pair where reset distance is the *only*
difference — same candidate, same Reserve band, `seconds_until_reset` either
side of `RESET_IMMINENT_SECONDS` (300) and `RESET_DISTANT_SECONDS` (3600),
asserting opposite outcomes. The `reset_urgency` mutation above must then kill
it. Tests only; no mechanism is missing.

---

## CORRECTION to the section above, and lines 1291/1292 ruled — 2026-08-29 (batch 48)

**The batch-48 triage section above mutated the wrong function, and its
conclusion about 1292 was wrong. This corrects it.**

That section reported: *"flipping `reset_urgency`'s distant branch from `0.0` to
`1.0` changed nothing … nothing in the suite watches 'more conservative when
the next reset is distant'."* The mutation ran, 37 tests ran, and the verdict
was still meaningless — because **`reset_urgency` is not on the reserve gate's
path at all.**

- `provider::quota::reset_urgency` (`quota.rs:1892`) has exactly one caller,
  `quota.rs:1865`, in the capacity-scoring path.
- `evaluate_reserve_spend` (`quota.rs:2268`) does **not** call it. It compares
  `RESET_IMMINENT_SECONDS` and `RESET_DISTANT_SECONDS` inline, at `quota.rs:2298`
  and `:2307`.

So a SURVIVED verdict there meant *"this mutation was irrelevant to the gate"*,
not *"the gate is unwatched"*. That is the same trap as a filter matching zero
tests, wearing different clothes: the `test result:` line was honest, the target
was right, and the **mutation site** was wrong. Found by the worker sent to
satisfy the bad acceptance bar, which read the call graph instead of obeying it.

### Line 1292 — CLOSED

Contract: Given a metered candidate in the protected reserve band and no
cheaper adequate resource, when the next quota reset is distant, reserve policy
denies the spend for anything below the heavy tier.

State: COMPLETE

Production evidence: `provider::quota::evaluate_reserve_spend`, `quota.rs:2307`
— `if seconds >= RESET_DISTANT_SECONDS && inputs.tier != WorkloadTier::Heavy`
→ `Deny`. Reached in production through `routing/disposable.rs:568` inside
`DisposableRouting::choose`, where `seconds_until_reset` is **real telemetry**
(`candidate.value().capacity.seconds_until_reset`), unlike three hardcoded
sibling fields.

Regression evidence: `routing::disposable::tests::reset_distance_alone_flips_the_protected_reserve_decision`
— same candidate, same Reserve band, `seconds_until_reset` the only field that
moves, referenced by constant name rather than literal. Premise asserted per
§17 by comparing the two `CandidateCapacity` values for inequality and then for
equality once the field is stripped from both, so a reader can see nothing else
changed.

Mutation, run by the orchestrator on the correct site this time:

| mutation | vocabulary | result |
|---|---|---|
| `quota.rs:2307`'s distant-deny condition disabled (`if false && ..`) | `remove-guard` | **killed** — three tests failed: `the_protected_reserve_policy_gates_the_metered_fallback`, `a_real_classification_changes_the_metered_fallback_outcome_at_the_same_call_site`, and the new `reset_distance_alone_flips_the_protected_reserve_decision` |

### Line 1291 — STAYS OPEN, and the blocker is now named exactly

The imminent branch exists (`quota.rs:2298`: a reset within
`RESET_IMMINENT_SECONDS` → `Allow`), and the new test shows `choose()` allows at
that distance. **But disabling that branch entirely changes nothing** —
verified by the orchestrator, SURVIVED with 15 tests genuinely run (not a void
filter).

The reason is structural, not a missing test. `evaluate_reserve_spend`'s tail
(`quota.rs:2325-2337`) denies only when `cheaper_adequate_resource_exists`, and
otherwise falls through to `Allow`. The sole production caller hardcodes
`cheaper_adequate_resource_exists: false` (`disposable.rs:568`). So in
production the imminent branch's `Allow` and the default `Allow` are **the same
decision**, differing only in their reason string.

"Become **more permissive** shortly before a reset" therefore cannot be
observed: the policy was already permissive. Closing 1291 needs a caller that
supplies a real `cheaper_adequate_resource_exists: true`, so that the imminent
window actually flips a `Deny` into an `Allow` — the same blocker that holds
1288, 1289 and 1290, not a separate one.

**A mis-citation to fix when someone next edits that function:** the imminent
branch's reason string cites *"(line 1292)"* and the `user_override` branch
cites *"(line 1291)"*. Both are one line off — imminent-permissive is 1291,
override is 1290.
