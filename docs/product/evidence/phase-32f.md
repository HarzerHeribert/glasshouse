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

---

## 1288 and 1291 — CLOSED by the blocker-resolution package, batch 49

**The measurement this package exists to justify.** The mutation disabling
`evaluate_reserve_spend`'s imminent-reset branch (`quota.rs:2298`) is recorded
above as **SURVIVED**, run by the orchestrator with fifteen tests genuinely
running. Re-run against the same function, unchanged, after fixing only the
*caller's* input:

    mutation disable-the-imminent-reset-branch: KILLED
    line_1291_an_imminent_reset_makes_the_policy_spend_a_reserve_it_would_otherwise_keep

Nothing about the branch moved. What moved is that it now decides something.
**1291 closed as a side effect of fixing 1288** and was on nobody's task list —
a line-at-a-time pass would have re-derived the same SURVIVED verdict and
returned it blocked for a third time.

### `cheaper_adequate_resource_exists` became real, and the definition was not invented

The obvious reading of "cheaper" is money, and Glasshouse has no price model —
`Cost` is `Free | Metered` and never compares two metered models. On that
reading 1288 is premise-invalid and would have been refused a fourth time.

**The correct reading was in the field's own doc comment**, written by Phase
32F: *"Whether a resource outside the reserve band could adequately serve this
task instead."* "Cheaper" is denominated in reserve capacity, not money, and
`CapacityBand` is `Ord` with `Exhausted` lowest precisely so a policy can ask
that as a comparison. Only reading the placeholder and its consumer together
surfaced it.

The input is now: does another eligible metered candidate have a **read** band
above `Reserve`? Every part is observed or configured — `capacity.band` comes
from `main.rs::disposable_candidate_capacity`, built per provider from
`observed_capacity` and that provider's own `EffectiveConfig::reserve_percent`.

Three refusals inside the ruling, each a place it could have become an invention:

1. **An unread band is not a cheaper resource.** Only `Some(band) > Reserve`
   counts; `None` might be deep in its own reserve. Deliberately the opposite
   default from `choose`'s `unwrap_or(CapacityBand::Plenty)` one field away,
   and both are the same rule: an unobserved band never withholds a resource
   and never withholds another one either.
2. **Free candidates are not consulted** — reaching the metered loop already
   proved none can serve.
3. **"Adequately" is inherited, not introduced** — this module has no
   per-candidate capability model and did not gain one.

**Recorded limit:** thresholds are per provider, so two models of one provider
always share a band and the branch fires only across providers.

Mutations, all killed, one re-run by the orchestrator:

| site | change | result |
|---|---|---|
| `disposable.rs` the call | `cheaper_adequate_resource_exists(&metered, index)` → `false` | killed — both 1288 and 1291 tests |
| `quota.rs:2298` | imminent branch disabled | **killed — the mutation this file records as SURVIVED** |
| `disposable.rs` | `band > Reserve` → `band >= Reserve` | killed |
| `disposable.rs` | `.is_some_and(..)` → `.is_none_or(..)` (an unread band counts) | killed |

### 1290 and 1294 stay refused

- **1290** — no producer anywhere sets a user override, and a disposable
  background job has no live user present to grant one.
- **1294** — premise-invalid *for this caller*: no progress model exists, and
  this decision does not move tasks between sessions.

### Where the root-cause framing did NOT pay, recorded honestly

The lead's own finding: **"hardcoded constant" is a symptom class, not a
cause.** Site A's three constants were three different problems wearing one
shape — one had a producer nobody had looked for, one has no producer at all,
one describes something this caller does not do. And 1319 (site B) was one
argument in one file exactly as `phase-33.md` described it; bundling it here
made it no smaller. It was included because the partition was free, not because
the framing helped.

---

# Lines 1289, 1290 closed and 1294 refused — 2026-08-30

Package `GH-RESERVE-INPUTS`; report in `.agent-runtime/report-reserve-inputs.md`.

## The register was stale and this package corrects it

Cluster A listed three hardcoded literals in the reserve inputs. **Only two
were still there.** `routing/disposable.rs:598` already called the real
`cheaper_adequate_resource_exists` (`:828`), closing 1288 and unblocking 1291,
and neither had been noticed. Both rows are now struck through in the register.

## 1290 — an override that is a scope, not a switch

`user_override` had no producer anywhere: a literal `false`. It is now set by
`ReserveOverride`, which pairs **the sessions the user named** with the session
being decided for, and `glasshouse sessions reserve <ID>` is the control.

**There is no spelling of it that means "every session"** — no constructor, no
config value, no flag. That was the packet's ruling 2 and it is enforced by
construction rather than by documentation, which matters because the thing
being overridden is a spending protection.

Six mutations, all killed, **two of them at the "and not elsewhere"
assertion** — the half of the test that carries the ruling.

## 1289 — closed on the tier, and the capability set deliberately refused

The workload tier *is* the capability-requirement scale at this decision. The
worker refused to plumb `hard_capabilities` in beside it and gave §79's reason:
**it varies with the wrong thing.** The refusal is written at the field, with a
test, rather than only in this report — so the next reader finds it where they
would otherwise add the wiring.

This is the right call and it is consistent with `phase-34.md`: the registry
answers *"can this resource do the work"*, not *"does this work deserve the
reserve"*. Merging them is the collapse `classify.rs:79` already refuses.

## 1294 — refused

Nothing in this build can observe that a task is nearly complete.
`task_nearly_complete` stays a literal `false` with the refusal recorded at the
field. A proxy from turn counts or elapsed time would have made the policy
confidently wrong at exactly the moment the line exists to protect — the line
asks Glasshouse *not* to move an almost-done task, so a false positive moves
work that should have stayed.

**This is the second reserve-input field with no producer, and the pattern is
worth naming:** a hardcoded `false` in a policy input is not a safe default, it
is an unobservable branch. Cluster A's line 1291 was unreachable for exactly
that reason until 1288 gained a producer.

---

### Allow high-tier tasks to consume protected reserve when their capability requirement justifies it. (line 1289)

Contract: Given a task whose required workload tier is Heavy or above, when the protected-reserve policy evaluates a metered candidate in the Reserve band, Glasshouse allows the spend and says which map line justified it, while preserving the denial for every tier below Heavy and refusing to let a hard capability requirement — which no model choice can satisfy — buy the reserve.

State: **COMPLETE**

Production evidence:
- `src/provider/quota.rs` — `evaluate_reserve_spend`
- `src/provider/quota.rs` — `ReserveDecisionInputs::tier`

Regression evidence:
- `reserve_inputs::the_tier_is_what_decides_whether_a_capability_requirement_justifies_the_reserve`
- `reserve_inputs::the_hard_capability_set_is_not_a_reserve_input`
- `reserve_inputs::every_reserve_decision_reachable_before_this_package_is_unchanged`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| quota.rs: 'which justifies spending protected reserve (line 1289)' -> '(line 1290)' | `wrong-map-line-in-user-visible-reason` | **killed** | `reserve_inputs::the_tier_is_what_decides_whether_a_capability_requirement_justifies_the_reserve` |
| quota.rs: 'if inputs.band > CapacityBand::Reserve {' -> 'if inputs.band >= CapacityBand::Reserve {' | `widen-a-gate` | **killed** | `reserve_inputs::every_reserve_decision_reachable_before_this_package_is_unchanged` |

> wrong-map-line-in-user-visible-reason observed: 1 failed, 14 passed; the assertion the test is named for, requiring the reason to cite the line it answers

> widen-a-gate observed: 8 of 15 failed, including the 280-combination sweep — the sweep is not vacuous

Recorded scope limits — stated by the worker, not discovered later:
- Production's one caller of DisposableRouting::choose passes classification: None (memory/extract/disposable.rs::RoutedNoModel::new, from main.rs:2513), so the tier the shipped binary presents is always Leaf and the >= Heavy branch is reachable only through new_for_request. Practice §79 deliberately refused wiring new_for_request into main.rs and that refusal stands.
- The identical objection applies to line 1288, which is already ticked: same function, same inputs, same reachability, and it is 1289's exact dual. The pair should be ruled the same way.
- Does not prove a hard capability requirement is irrelevant to routing generally — only that it must not buy protected reserve, because it names something no model choice can supply.

---

### Allow the user to override reserve protection for a specific task or session. (line 1290)

Contract: Given a user who has named one session with `glasshouse sessions reserve <ID>`, when that session's background jobs reach the protected-reserve policy, Glasshouse allows them to spend protected reserve and names the session in the routing explanation, while preserving the policy's ordinary denial for every session the user did not name and offering no setting, flag or constructor that could mean 'every session'.

State: **COMPLETE**

Production evidence:
- `src/routing/disposable.rs` — `ReserveOverride`
- `src/routing/disposable.rs` — `DisposableRouting::with_reserve_override`
- `src/routing/disposable.rs` — `DisposableRouting::choose`
- `src/config/mod.rs` — `RoutingConfig::reserve_override_sessions`
- `src/config/mod.rs` — `EffectiveConfig::reserve_override_sessions`
- `src/cli.rs` — `SessionCommand::Reserve`
- `src/main.rs` — `reserve_override_session`
- `src/main.rs` — `disposable_extraction_model`
- `src/main.rs` — `report_hook`

Regression evidence:
- `reserve_inputs::the_override_grants_the_reserve_for_the_session_the_user_named`
- `reserve_inputs::the_override_does_not_reach_a_session_the_user_did_not_name`
- `reserve_inputs::the_same_override_decides_two_sessions_differently`
- `reserve_inputs::no_reserve_override_means_everywhere`
- `reserve_inputs::a_routing_policy_that_names_no_override_is_unchanged`
- `reserve_inputs::a_granted_override_names_its_session_in_the_explanation`
- `reserve_inputs::the_user_override_branch_outranks_every_automatic_denial`
- `reserve_inputs::the_almost_complete_guard_still_answers_before_the_override`
- `reserve_inputs::the_reserve_override_setting_layers_project_over_user`
- `reserve_inputs::the_reserve_override_setting_round_trips_through_the_config_file`
- `glasshouse(bin)::tests::the_reserve_override_a_user_records_reaches_the_routing_decision`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| disposable.rs: 'user_override: self.reserve_override.applies(),' -> 'user_override: false,' | `restore-the-hardcoded-literal` | **killed** | `reserve_inputs::the_override_grants_the_reserve_for_the_session_the_user_named` |
| disposable.rs ReserveOverride::applies: 'self.deciding_for.as_deref().is_some_and(|session| self.sessions.contains(session))' -> '!self.sessions.is_empty()' | `widen-a-scope-to-global` | **killed** | `reserve_inputs::the_override_does_not_reach_a_session_the_user_did_not_name` |
| main.rs disposable_extraction_model: 'ReserveOverride::for_sessions(effective.reserve_override_sessions().value).deciding_for(session.to_string())' -> 'ReserveOverride::none()' | `skip-the-production-read-back` | **killed** | `glasshouse(bin)::tests::the_reserve_override_a_user_records_reaches_the_routing_decision` |
| main.rs disposable_extraction_model: '.deciding_for(session.to_string())' -> '.deciding_for(first configured override, falling back to session)' | `ignore-the-subject-of-a-scoped-decision` | **killed** | `glasshouse(bin)::tests::the_reserve_override_a_user_records_reaches_the_routing_decision` |

> restore-the-hardcoded-literal observed: 3 of 15 failed; 'assertion failed: reserve_allows(&named)' at the two-session test's own assertion

> widen-a-scope-to-global observed: 3 of 15 failed; 'assertion failed: !reserve_allows(&other)' — the second half, which is the half that matters

> skip-the-production-read-back observed: SURVIVED first (44 passed, 0 failed) against a test that read the config through its own helper — practice §35. After rewriting the test to drive disposable_extraction_model itself: 1 failed at 'the session the user named must be allowed to spend the reserve', printing the real denial reason.

> ignore-the-subject-of-a-scoped-decision observed: 1 failed at 'a session the user never named must not inherit another session's override', printing the full explanation with the other session's id in it

Recorded scope limits — stated by the worker, not discovered later:
- Session-scoped only. The 'task' half of 'task or session' is refused: a disposable job carries a JobKind (a class of work), not a task identity, so a JobKind-scoped override would be a category-wide switch. Recorded in ReserveOverride's doc comment.
- Writes the user layer only; no project-layer path and no Settings-screen surface (shell/mod.rs is outside this packet's expected files).
- The override never expires and is not reaped when a session closes. A stale entry is inert — it can only match a session with that identifier — the same reasoning free_resource_pin documents.
- Does not prove a real harness hook run spends the reserve: RoutedNoModel never calls a model, so the evidence stops at the routing decision and its explanation.

---

### Avoid moving an almost-complete high-value task to another session solely because a reserve threshold was crossed. (line 1294)

Contract: n/a — the decisive input has no producer and must not be approximated.

State: NOT STARTED — worker refused the line; see its reason

Regression evidence:
- `reserve_inputs::nothing_in_this_build_produces_task_nearly_complete`
- `reserve_inputs::the_event_vocabulary_cannot_express_almost_complete`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| disposable.rs: 'task_nearly_complete: false,' -> 'task_nearly_complete: true,' | `fabricate-the-missing-producer` | **killed** | `reserve_inputs::nothing_in_this_build_produces_task_nearly_complete` |

> fabricate-the-missing-producer observed: 5 of 15 failed; the scan test names the site and says why a proxy is not acceptable there

Recorded scope limits — stated by the worker, not discovered later:
- The refusal is about producers, not about the consumer: evaluate_reserve_spend honours task_nearly_complete correctly and outranks the user override with it, proven by reserve_inputs::the_almost_complete_guard_still_answers_before_the_override.
- in-repo: NO for now. Every LifecycleEvent is binary and retrospective, no integrated harness reports task progress, and the one path reaching this policy runs after TurnEnded { Completed }. It becomes in-repo only if a harness begins reporting progress.

---

### Worker-reported packet errors and gates (transcribed at closure)

**Packet errors the worker reported — read these BEFORE its results.**
Thirteen consecutive rounds a worker corrected its packet and was right:
- The packet said the reserve-inputs struct is built at disposable.rs:596-602 (correct) but the refusal register cites disposable.rs:568 for all four Cluster A rows; the literal has not been at :568 since before 62473a6.
- Refusal register Cluster A row 1288 says `cheaper_adequate_resource_exists: false` is hardcoded. It is not: disposable.rs:598 calls the real function at disposable.rs:828, and map line 1288 is already ticked (☑), so the row should have been deleted under the register's own rule 3.
- Refusal register Cluster A row 1291 says the imminent-reset branch is 'blocked by 1288' because a false cheaper-adequate makes the function fall through to Allow. The mechanism is backwards — quota.rs evaluates the imminent-reset branch before the cheaper-adequate check, so that input could never block it — and map line 1291 is ticked (☑) with dedicated tests at disposable.rs:1292, :1560 and :1578.
- The packet's Phase -1 says 'the tier half already works'. It works as a policy, but production's single caller passes classification: None (memory/extract/disposable.rs::RoutedNoModel::new from main.rs:2513), so the shipped binary always presents WorkloadTier::Leaf to this decision and the >= Heavy branch has no production producer.
- Every capability-map citation in ReserveDecisionInputs and evaluate_reserve_spend was one line too high against the current map (band 1288->1287, cheaper-adequate 1289->1288, tier 1290->1289, user override 1291->1290, imminent reset 1292->1291), while disposable.rs's own tests used the correct numbers. Corrected inside that one function because two of the five are this packet's box lines and a partial fix would have printed one number twice.
- Still wrong and left alone for the orchestrator: config/mod.rs:903 and :946 and provider/resources.rs:615 and :1536 cite 1288 for the reserve-percentage line, which is now 1287; tests/workload_tiers.rs:90 and :152 name the tier branch 'at_line_1290' when it is 1289, and phase-34a.md quotes the second name as evidence.
- scripts/blast-radius.sh traced 12 targets and rustdoc but named none of capacity_score, routing_score, routing_disposable_tier, routing_policy or reserve_inputs, despite routing/disposable.rs and provider/quota.rs being in the diff. The full workspace suite was run instead.

Gates the worker ran (re-run the decisive ones yourself):
- cargo build: clean
- cargo test --test reserve_inputs: 15 passed, 0 failed
- cargo test --test workload_tiers: 12 passed, 0 failed
- cargo test --test capacity_score: 31 passed, 0 failed
- cargo test --test route_command: 12 passed, 0 failed
- cargo test --workspace: 65 targets, all ok, 0 failed
- cargo clippy --all-targets --all-features -- -D warnings: clean
- cargo fmt --all -- --check: clean
- scripts/blast-radius.sh: every traced target passed; rustdoc clean


---

## Task progress is declared — lines 1294 and 1610 closed together, 2026-09-03

One package, `GH-TASK-PROGRESS` (Opus, **Red** — migration 28 and a persisted, session-scoped row), worktree `.worktrees/task-progress`, packet `.agent-runtime/packet-task-progress.md`, report **`.agent-runtime/report-task-progress.md`**. The design ruling it implements is `design-decisions.md`, *A task's progress is declared, never guessed*.

**The two lines are one mechanism seen from two phases**, which is why they closed together and why this entry is the same in both ledgers: 1294 is the reserve-threshold guard (`provider/quota/mod.rs :: evaluate_reserve_spend`, its *first* branch), 1610 the quota-conservation guard (`routing/pressure.rs :: reserve_verdict`).

**What the earlier refusal got right, and what changed.** The standing refusal was that *nothing in this build observes that a task is nearly complete*, and that a turn-count or elapsed-time proxy would report "almost complete" for work that had merely been running a while — inverting the protection at the one moment it matters. **That is still true and nothing here weakens it.** No signal Glasshouse already observes was touched; the event vocabulary is unchanged, and `reserve_inputs::the_event_vocabulary_cannot_express_almost_complete` still passes untouched — it is now the *reason* a declaration was the only honest source rather than the evidence for a refusal. The producer is a person or orchestrator saying so on purpose, through `glasshouse task-progress --session <id>`, and the statement expires.

**The three properties the design required, and how each is held:**

1. **Never infer.** The only thing that sets the field true is a declaration. A source scan forbidding the words `turn_count`/`elapsed` was written and **removed**, because it failed on the module's own doc comments explaining why such a proxy inverts the policy — a pin that punishes stating the invariant is worse than no pin.
2. **Scoped and expiring, never sticky.** The source is a store row, not a configuration value: a settings value is sticky by nature, and a sticky declaration re-creates the inversion by the slower route. `TASK_PROGRESS_EXPIRES_AFTER` is **30 minutes and deliberately shorter than `STALE_CLAIM_AFTER`**, with the asymmetry argued at the constant — expiring early falls back to today's behaviour, expiring late keeps a dead statement outranking every other signal the policy has. A `const _: () = assert!(…)` **fails the build** if the two are ever made equal.
3. **A default that changes nothing.** `DeclaredTaskProgress::default()` can never match — no constructor means "everywhere", and `deciding_for` is `None` for every caller predating these lines, exactly as `ReserveOverride` arrived as a no-op for line 1290.

**Both production construction sites are fed, and that was proven by mutation rather than by reading.** `routing/disposable/mod.rs`'s per-candidate loop and `routing/pressure.rs :: reserve_verdict` both read the declaration; `commands/routing_destinations.rs :: session_router` is the one constructor every real ranking goes through, without which the field would be wired structurally and always false in production — `cluster-b.py`'s shape.

Six mutations, **all KILLED**: `guard-does-not-fire`, `declaration-never-expires`, `drop-scope-predicate`, `drop-liveness-check`, `disposable-site-unfed`, `pressure-site-unfed`. The last two are the ones that matter most — they prove each site independently, and either surviving would have meant a site with no test.

Gates: fmt, `cargo check --all-targets`, clippy `-D warnings`, rustdoc `-D warnings`, `check-doc-boundary.sh` and the size ratchet all clean; `--test task_progress` 20/20, `--test subscription_pressure` 18/18, `--test reserve_inputs` 18/18, `--test capacity_score` 31/31, `--test support_work_economy` 13/13, `--test v1_criteria_routing` 8/8, `--test session_context` 18/18; `blast-radius.sh --targeted` over 27 changed files exit 0.

**Limits, and the first is the one to read:**

- **The declaration scopes to a *session*, not a task.** The lines say "task"; a disposable job carries a `JobKind`, not a task identity, so a session is the narrowest real scope this build has. `ReserveOverride` records the identical limit for line 1290, so this is consistent with the existing precedent rather than a new compromise — but a session running several tasks is protected as a whole for the horizon.
- The 30-minute horizon is a judgement argued from the asymmetry of the two failure directions, **not a measurement of real task lengths**.
- The declaration is honoured only inside `evaluate_reserve_spend` and `reserve_verdict`; no other Glasshouse decision consults it.
- `declared_task_progress_sessions` is best-effort: an unopenable database yields an empty set, so a broken database silently loses a declaration rather than failing a routing decision.
- Migration 28's rollback is proven on **macOS only** in this worktree; the trailing sweep owns the other two platforms.

**Four errors in the orchestrator's own packet, found by the worker and recorded here rather than in five places:**

1. **The packet named one source-scanning pin; there are two.** `tests/reserve_inputs.rs::nothing_in_this_build_produces_task_nearly_complete` asserts the identical refusal over `disposable/**`, `provider/quota/mod.rs` and `main.rs`+`commands/*`, is not reachable from the packet's traced targets, and had to be re-stated by the same argument (renamed `::nothing_in_this_build_infers_task_nearly_complete`).
2. **The packet said to extend the scan using practice §81's `#[cfg(test)]` boundary; applied to `routing/disposable/mod.rs` that is wrong.** Its only `#[cfg(test)]` is `mod tests;` at line 55 — the unit tests live in a sibling file — so slicing there discards ~1,470 lines of production code **including the construction site the scan exists to watch**. That is §68's shape (a filter matching nothing reads as a pass) hiding inside the fix for §81's. `disposable_production_source()` treats the whole file as production **and asserts that assumption**, so it cannot be silently kept if an inline test block ever appears. *(General lesson: after Phase 59 moved inline tests into sibling files, `mod tests;` is a declaration, not a boundary. Check where a file's tests live before writing any scan over it, and assert what you scanned.)*
3. **The migration ripple is 13 version pins and 25 rollback fixtures, not nine pins** — 4 in `src/database/tests.rs`, 4 in `src/session/store/tests.rs`, 5 under `crates/glasshouse/tests/`.
4. **A 14th pin is invisible to any comma-anchored grep**: `tests/session_context.rs:242` is a bare `27,` on its own line after `schema_version(&conn),`. The targeted blast radius caught it (left: 28, right: 27). A successor packet should say *"any bare version literal on its own line"*.
