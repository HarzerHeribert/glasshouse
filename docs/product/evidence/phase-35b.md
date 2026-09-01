# Capability evidence — phase 35B

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 35B — candidate scoring, wired into the one routing decision the binary actually makes

Contract: Given several eligible routing candidates for a disposable job, when
Glasshouse picks one, it scores them with an inspectable weighted function whose
every contribution is named and carries its evidence — while preserving the rule
that a candidate failing a hard constraint is never scored at all, only refused.

State: **COMPLETE** for map lines 1530, 1532, 1536, 1547, 1549, 1552, 1553 and
1554 — eight of twenty-five. **PARTIALLY VERIFIED** for 1548. **NOT STARTED**
for the other sixteen, each with its missing source named below.

Production evidence:
- `main.rs:1120` → `DisposableRouting::for_support_work(...)` →
  `memory::RoutedNoModel::new(JobKind::MemoryExtraction, …)` →
  `DisposableRouting::choose` — **the one routing decision the shipped binary
  makes today.** `choose` now builds a real `routing::RoutingExplanation` for
  the candidate it picks, and that explanation reaches
  `ExtractionModel::describe()`, the string `Extractor::run` records on every
  `ExtractionOutcome`.
- `main.rs::disposable_candidate_capacity` reads a real on-disk
  `GatewayQuotaCache` through `provider::resources::observed_capacity` — the
  same call `glasshouse resources` makes — so lines 1536 and 1549 are fed by
  live telemetry rather than a fixture.

Regression evidence:
- `routing::disposable::tests::the_winning_candidate_carries_a_named_inspectable_explanation`
- `routing::disposable::tests::real_capacity_and_reset_data_reach_the_explanation`
- `main.rs::tests::disposable_extraction_model_prefers_a_configured_free_model_and_names_the_reason`
- `main.rs::tests::disposable_extraction_model_reflects_real_cached_capacity_telemetry`
  — the last two enter through `disposable_extraction_model`, the function
  `main.rs`'s own hook path calls, and one plants a real cache file first.
- `tests/routing_score.rs` — four tests at the public-API boundary.

Failure/isolation evidence:
- **The decisive mutation, re-run independently by the integrator.** Replacing
  `choose`'s free-branch `self.score(...)` with an empty `RoutingExplanation`
  fails all four tests above — two of them in the binary target, which is the
  half that proves the shipped path rather than the library. Restored
  byte-identical.
- **A mutation that survived, and the dead mechanism it exposed.** The worker's
  first draft called `apply_hard_constraints` *and* kept a redundant
  `if !self.metered.permits_metered()` early check. Mutating the constraint
  closure to admit everything left 13/13 green: the hard-constraint call was
  **decorative**. It removed the redundant check so the metered-withheld result
  derives from the eligible set alone, re-ran the same mutation, and it now
  fails `glasshouses_own_run_refuses_a_metered_resource_without_an_opt_in`.
  Line 1532 and line 1553 rest on that being load-bearing, and it now is.

**Why seventeen stay open, grouped by the kind of missing thing.** This list is
the next packages' brief and is the more useful half of this entry.

- **No source exists yet in this build**: 1534 (context quality), 1535 and 1545
  (prompt-cache temperature — Phase 30/31, 0/15), 1537 (provider health — Phase
  33, 0/15; deliberately *not* conflated with `routing::free::FreePool`'s
  per-credential cooldown, which is a disposable-pool signal, not a
  cross-provider reliability ledger), 1538 (marginal cost — Phase 32G, 0/10),
  1539 (expected latency), 1546 (cadence — Phase 33A's ledger, built this same
  round and not yet read by anything).
- **Structurally inapplicable to this caller, which is different from
  unbuilt**: 1533 and 1551 (session affinity, switching cost — a disposable job
  has no session, and `routing/interactive.rs` carries a test that fails the
  build if `crate::session` appears in it), 1543 and 1544 (effective TTFC, tool
  rounds — `RoutedNoModel` calls no model and disposable jobs use no tools).
- **Nothing differentiates candidates on this axis**: 1531 (workload-tier fit —
  no candidate carries a capability rating; `WorkloadTier::Leaf` is the *job's*
  tier for the reserve gate, not a per-candidate score).
- **Hypothesis killed — see below**: 1540, 1541, 1542.
- **Mechanism built, caller unreachable in practice**: 1550.
- **1547 is now CLOSED** (batch 39) — failure-domain diversity. The full
  evidence is in `docs/product/evidence/phase-33c.md`, because the ranking it
  feeds is `InteractiveRouting::on_provider_failure`'s rather than the
  disposable scorer's. Note what changed against the guess recorded here: this
  entry expected `FreePool`'s per-credential allowance to be the signal, and the
  signal that actually decided it was **the provider on the candidate's own
  `Backend`** — a credential exhaustion is a *quota*-domain fact, and 1547 asks
  about the *failure* domain. Phase 33C's line 1371 exists precisely to keep
  those two apart, and conflating them here would have scored a sibling
  credential as if it were a different upstream.

**The pairing prior does not apply here, and the packet was wrong to say it
did.** The orchestrator's own design decision listed 1540/1541 as closable via
`native_pairing_prior_contribution`. The worker checked before building and
found it structurally impossible, not merely unsourced:
`PairingQuery::harness` is a required `IntegrationId`; all ten variants are
third-party coding harnesses a user launches; and `DisposableCandidate` carries
no harness because **a disposable job is Glasshouse's own internal call, never
made inside one of those harnesses**. Constructing a `PairingQuery` there would
mean choosing an `IntegrationId` unrelated to the actual call — exactly the
"do not invent a source" rule the same packet stated for every other input.

**Consequence for the round's premise, recorded because it corrects the
orchestrator.** Batch 35's finding was "eighteen boxes wait on one missing
consumer." Half of that was right — the reserve policy — and half was too
coarse: Phase 9J's eleven need `InteractiveRouting` (the gateway session), a
different caller entirely, and no amount of work on `DisposableRouting` will
reach them.

Missing evidence:
- **1550's caller.** `evaluate_reserve_spend` is now called from `choose`'s
  metered-fallback branch and mutation-proven there, but
  `main.rs::disposable_candidates` **never builds a metered candidate** — it
  iterates `free_models()` only. The gate is reachable by type and dead in the
  shipped binary. Closing it needs a product decision the worker correctly
  refused to take alone: *which* metered model a provider falls back to has no
  source in `ProviderConfig`, and inventing a "disposable fallback model" field
  decides whether a background job may spend a user's paid quota unasked.
- **1530's caveat, stated rather than left implicit.** In the free branch,
  `choose` still returns at the first available candidate in the user's own
  order — the pre-existing tested algorithm — so `score` runs on the winner, not
  eagerly on every candidate. The metered branch does score and compare all of
  them. The worker reasoned that sorting by `.total()` would reproduce the same
  winner in every case the existing tests exercise, and said plainly that it did
  not prove it mechanically. A future reader should not take it as proven.

### Phase 35B — evidence that varies where the prior cannot (lines 1541, 1548)

Contract: Given two routing candidates that a same-vendor pairing prior scores
identically, when Glasshouse has accumulated reliable observations for one of
them, Glasshouse ranks the candidate with better observed success above the one
the prior merely favours — while discounting observations drawn from small
samples or stale windows rather than treating a single lucky turn as
established evidence.

State: **COMPLETE** for map lines 1541 and 1548. **NOT STARTED** for 1542 and
1545, both declined by the integrator against the worker's proposal for 1542 —
reasons below.

**Why this package existed at all.** Phase 9J built the pairing prior, wired it
to `InteractiveRouting::on_provider_failure`, mutation-proved it, and it was
**inert**: `harness::pairing::classify` derives `PairingClass` from `(harness,
model attribution, harness vendor)` and never reads `route`, so every same-model
candidate scores an identical prior. The *evidence* does not share that defect —
`EvidenceKey` is built per candidate carrying `candidate.model()` and `route`,
so two same-model candidates on different providers get different observations.
**The fifth link passes for evidence exactly where it fails for the prior**, and
that is the whole basis of this package.

Production evidence:

- `routing/evidence.rs::ObservedEvidenceSource::observed` now supplies the
  `reliable_observation_count` that `config/pairing.rs::decay_factor` decays
  against, from a real `EvidenceLedger`, with a staleness discount built on
  `AggregateReading::freshness` — line 1541's *real* input and line 1548's
  stale-window half.
- `config/pairing.rs::SUFFICIENT_EVIDENCE_OBSERVATIONS`, a hard sufficiency gate
  in `native_pairing_prior_contribution` — line 1548's small-sample half.
- The scoring path itself is unchanged and pre-existing:
  `InteractiveRouting::on_provider_failure` → `score_candidate` →
  `native_pairing_prior_contribution` → `RoutingExplanation::total` → `best()`.

Regression evidence:

- `routing::interactive::tests::on_provider_failure_prior_decays_as_real_recorded_evidence_accumulates`
  — a **real** `EvidenceLedger`, 5 versus 15 fresh observations, strictly smaller
  prior magnitude at 15.
- `routing::interactive::tests::on_provider_failure_discounts_a_stale_observation_window`
  — the same eight successes, ten seconds old versus two days old.
- `routing::interactive::tests::score_candidate_does_not_let_a_thin_sample_outrank_an_established_one`
- Pre-existing: `tests/pairing_prior.rs::the_prior_contribution_decays_to_zero_as_observations_accumulate`.

**The new tests use a real ledger rather than the pre-existing hand-built
`ObservationSource` doubles**, which matters here more than usual: the doubles
can express counts and rates the shipped ledger can never produce, and one of
this batch's findings is precisely that the shipped ledger's range is narrower
than the doubles suggested.

Failure/isolation evidence:

- **Inertness check** (the one that mattered): `evidence_signal` neutralised to
  an unconditional `return 0.0`. The acceptance test failed — both candidates
  tie at their equal priors and `best()` falls back to caller order, returning
  the wrong candidate. **The term is load-bearing, not decorative.**
- Mutations, all killed: `bypass-fallback`, `remove-guard`, `invert-condition`,
  `alter-boundary`, `accept-stale-state`.

**A pre-existing mechanism found inert, and not fixed here.**
`CONFIDENT_AT_OBSERVATIONS = 5` (Phase 9J) scales confidence continuously, but
`routing/evidence.rs::MIN_SAMPLE_FOR_SUMMARY = 5` means `EvidenceLedger::summarize`
**never returns a count below 5**. So every real observation is already at
maximum confidence the instant it exists: **5 real samples and 5000 real samples
score identically.** That curve is reachable only through a hand-built
`ObservationSource`. This is why 1548 needed a hard gate rather than a second
curve layered on a saturated one, and it is recorded rather than repaired
because repairing it means choosing between lowering `MIN_SAMPLE_FOR_SUMMARY`
and raising `CONFIDENT_AT_OBSERVATIONS`, which is a policy decision this package
was not scoped to make.

**Three provisional constants, named as provisional.**
`SUFFICIENT_EVIDENCE_OBSERVATIONS = 5` (matched to the two existing constants
answering the same question), `EVIDENCE_STALE_AFTER_SECONDS = 86400` (chosen
clearly shorter than `FAILOVER_EVIDENCE_WINDOW_SECONDS`'s 7 days so *stale* and
*outside the window* stay distinct concepts), `STALE_OBSERVATION_DISCOUNT = 0.5`
(a fraction, not a curve — 1548 asks staleness to count for less, not to vanish;
zeroing would silently reproduce the "no evidence" case). None is measured. Same
standing as `RETRIEVAL_WEIGHT_FLOOR`, and recorded so none is mistaken for a
constant with a derivation behind it.

**Line 1542 was proposed COMPLETE by the worker and declined by the
integrator.** The line reads *"prefer observed success **and reliability** over
same-vendor alignment"*. `ObservedEvidence::reliability` is **`None` on 100% of
real production rows** — `ObservedEvidenceSource::observed` leaves it unset
because `RoutingSummary` has no field distinct from `failure_rate` that could
honestly fill it, and the worker correctly declined to duplicate
`task_success_rate` into it (both are summed as independent terms, so copying
would double-count the one real signal). The mechanism therefore closes on
`task_success_rate` alone.

**That is the identical standard by which line 1545 was refused in the same
round**, and consistency is the point: an input that is absent or constant
across every real observation cannot support a box that names it. 1545's
`ContextState` is `Unknown` on every real row because
`NewObservation::with_context_state` has zero non-test callers; 1542's
`reliability` is `None` on every real row for the same class of reason. Closing
1542 would retire a requirement that a second, independent reliability signal
ever be observed. It needs one — not more scoring code.

**Line 1541 is closed with one narrowing recorded rather than hidden.** The line
names the *"exact harness-profile-model-backend combination"*. The evidence key
carries harness, model, provider and protocol, but **not the launch profile** —
`ObservedEvidenceSource::observed`'s own doc states *"`key`'s launch profile is
not part of the query, because nothing this ledger stores carries one."* The
line's substantive requirement — decay as reliable observations accumulate,
scoped to a combination rather than global — is implemented, production-wired
and mutation-proven against a real ledger, so the box is ticked; recording the
narrowing here keeps the remaining dimension discoverable. Adding it means a
ledger column and a migration, which no phase currently asks for.

Platform/external evidence: pure computation, no `#[cfg]` added.

Missing evidence:

- **1542** needs a reliability signal independent of `task_success_rate`.
- **1545** needs a real producer for `ContextState`.
- **1541's launch-profile dimension** needs a ledger column, and with it a
  migration.
- `CONFIDENT_AT_OBSERVATIONS`'s curve needs `MIN_SAMPLE_FOR_SUMMARY` and it to
  stop being the same number before it can discriminate.

### Phase 35B / Phase 32F — the reserve gate becomes reachable (lines 1293, 1550)

Contract: Given a disposable support job and no free resource able to serve it,
when Glasshouse routes that job, it considers metered candidates, lets the
protected-reserve policy decide whether the spend is permitted, and reports that
decision as a named contribution in the routing explanation — while always
preferring free capacity when any can serve, and never inventing a model name or
a price.

State: **COMPLETE** for map lines 1293 and 1550.

**Four checkpoints misdiagnosed this as a policy gap. It was candidate
generation.** `main.rs::disposable_candidates` iterated
`provider_config.free_models()` and hardcoded `Cost::Free`, so
`routing/disposable.rs:558`'s `filter(|c| !c.cost().is_free())` was **always
empty in the shipped binary**, `evaluate_reserve_spend` never ran, and the
reserve contribution — written and correct since batch 36 — could never appear.
The policy was ready the whole time.

**No migration, and no new mechanism.** `MeteredUse`, `evaluate_reserve_spend`
and the reserve `Contribution` are untouched. The change is a
`ProviderConfig::metered_models` field, symmetric to the existing
`free_models()`, and candidate generation that uses it.

Production evidence:

- `config/mod.rs::ProviderConfig::metered_models` — the user names the specific
  paid model IDs disposable jobs may fall back to. **Glasshouse never invents a
  model name**, which is why an unconfigured project generates no metered
  candidate at all.
- `main.rs::disposable_candidates` — now builds real `Cost::Metered` candidates,
  reached from `disposable_extraction_model` ← `report_hook` (`main.rs:1177`),
  the post-turn extraction trigger.
- `routing/disposable.rs::DisposableRouting::choose` (unchanged) now reaches
  `evaluate_reserve_spend` and pushes the named `protected-reserve policy`
  contribution into the explanation — line 1293.
- `DisposableRouting::score` (unchanged) consumes the reserve result as a
  **named, zero-magnitude, reason-carrying** contribution — line 1550.

Regression evidence:

- `main.rs::disposable_extraction_model_prefers_a_free_model_over_a_configured_metered_one`
  — free capacity wins whenever any can serve. **Load-bearing.**
- `main.rs::disposable_extraction_model_falls_back_to_a_configured_metered_model_when_permitted`
- `main.rs::disposable_extraction_model_lets_the_protected_reserve_policy_deny_a_metered_candidate`

**Why the zero magnitude is correct and not inertness.** An allow/deny **gate**
is not a scoring weight, and `DisposableRouting::score`'s own doc comment — which
predates this batch and was *verified rather than assumed* — says the gate must
not be double-counted as a magnitude. The gate's effect is proven by the deny
test: with 10% remaining, band `Reserve`, and 7200s to reset (past
`RESET_DISTANT_SECONDS`), it **refuses a real candidate** built from real
telemetry through `disposable_candidates`, not from a hand-built
`CandidateCapacity`. `routing/disposable.rs`'s own pre-existing
`the_protected_reserve_policy_gates_the_metered_fallback` could not prove that,
because it constructs the capacity by hand rather than deriving it through the
production reading path (§35).

Failure/isolation evidence:

- Mutations killed at candidate generation, including one the worker added
  itself (`remove-fallback-generation`) when it found the packet's named
  mutations targeted `routing/disposable.rs`, a file this change did not need to
  touch. **Mutating what the change actually did, rather than what the packet
  guessed it would do, is the right correction.**
- **§69 applied without being asked.** The worker ran the full `--lib` (1384) and
  full `--bin` (35) suites rather than the named targets, on the reasoning that
  candidate generation is something anything routing-, config- or shell-adjacent
  could assert about — and confirmed the new field does not collide with the
  concurrent `shell/**` work it was forbidden from opening.
- Binary run: `glasshouse resources` against a real on-disk `config.toml`
  carrying `metered_models = [...]`, proving the TOML round-trip for real rather
  than through serde unit tests.

**The control surface, and why there is no second switch.** `metered_models`
**is** the setting: an empty list is the coherent off state (no metered
candidates, free-only), a populated one is the on state. A separate boolean was
considered and rejected — it would be a second source of truth able to contradict
the list (models configured, flag says off), which is the *"second place that has
to know"* defect this project keeps paying for. Glasshouse cannot spend on a
provider the user never configured, so an empty default is not a hedge; it is the
only honest default when inventing a model name is forbidden.

**One deliberate non-enforcement, recorded so it is met rather than
rediscovered.** The user's decision bounds the spend *"in proportion to the actual
task"*. That ratio is **not** enforced here and must not be faked: cost estimation
is **Phase 32G, 0/10**, and map line 1305 requires unknown pricing be treated as
unknown rather than assigned a fake zero. What is implemented is the decision's
own recorded validity condition — free-first, last-resort, bounded, inspectable —
which holds *while cost estimation does not exist* and should be revisited
against Phase 32G rather than re-derived.

Missing evidence:

- The proportion bound above, pending Phase 32G.
- A model named in both `free_models` and `metered_models` resolves to `Free`
  and is deduplicated rather than rejected. Well-defined by `cost_of`'s existing
  contract, untested, and flagged by the worker as a candidate for a
  `glasshouse doctor` warning rather than a hard error.


---

## Orchestrator ruling — map line 1541, 2026-08-29 (batch 47)

**Inherited unresolved through three orchestrators. Ruling: the tick stands,
with the residual gap bounded below. Closed — do not re-open it without new
evidence.**

Verified directly by the orchestrator rather than accepted from a report:
`EvidenceKey` (`harness/pairing.rs:502`) carries `harness`, `launch_profile`,
`model` and `route`, but `ObservedEvidenceSource::observed`
(`routing/evidence.rs:1152`) builds its `ObservationQuery` from provider,
model, route and harness — **`launch_profile` is read into the key and then
not used in the query.** The entry above already recorded this; it is
confirmed, not newly discovered.

**Why the tick is still right.** The line's substantive requirement is that the
prior decays against real accumulated observations *scoped to a combination*
rather than globally. That is implemented, production-wired and mutation-proven
against a real ledger. The missing dimension is not a defect in the decay; it
is a dimension the ledger cannot express — nothing it stores carries a launch
profile — so the query cannot filter on what does not exist.

**The residual, stated precisely so nobody has to re-derive it.** Two launch
profiles that differ in harness, model, provider or protocol are already
distinct keys and decay separately. The gap is confined to two profiles that
share *all four* of those and differ only in something the route does not
capture; those share a prior today. Whether that residual is worth a ledger
column and a migration is a **Phase 51-shaped question about the event-log
schema**, not a Phase 35B question, and no phase currently asks for it.

**This is deliberately not a re-tick or an un-tick.** Un-ticking would retire a
mechanism that works for the case the ledger can actually distinguish; leaving
the question open a fourth time is what this ruling exists to stop.


---

## From `GH-LAUNCH-CLASSIFIER` (2026-08-31)

The launch-path classifier package (router request schema, classification on the acting path) touched this phase's lines 1531 (open — same missing producer as 1516). The full entry — production sites, regression names, the 23 killed mutations, the one honestly-survived one, and the missing producer for 1516/1517/1531 — is in `phase-34d.md`, *Phase 34D — router request schema* and *lines outside Phase 34D*, because the mechanism lives there.

### Lines 1516 and 1531 — tier and hard-capability terms in the score; 1517 open

Package `GH-TIER-CEILING`, 2026-08-31, Opus at high. Nine mutations, nine killed. The worker **refused OBJECTIVE 3** — attaching adapter-declared `ResourceFacts` to destinations — and the orchestrator verified the refusal: `capability_fit` (`routing/session.rs:786`) already reads `adapter_for(destination.harness())` and `prefer()` falls through to those declarations whenever the facts are `Unverified`, so the wiring would have changed no score and survived its own mutation; `Destination::with_resource_facts` keeps no production caller, deliberately.


### Exclude candidates below the classified minimum workload tier. (line 1516)

Contract: Given a classified task with a required workload tier and a destination whose model the user capped below it, when Glasshouse ranks destinations on the shipped binary, it refuses that destination with a readable workload-tier reason naming both tiers -- while preserving that a destination whose ceiling nobody established is never refused on it.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts.

Production evidence:
- `src/main.rs` — `destination_tier_ceiling`
- `src/main.rs` — `routing_destinations (both Destination::with_tier_ceiling call sites)`
- `src/routing/session.rs` — `hard_constraint (the WorkloadTier arm)`

Regression evidence:
- `tier_ceiling::a_configured_ceiling_excludes_a_destination_below_the_required_tier_on_the_shipped_binary`
- `tier_ceiling::without_a_ceiling_nothing_is_excluded_and_the_fit_term_says_not_established`
- `tier_ceiling::a_warm_sessions_ceiling_comes_from_the_model_it_is_running`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| main.rs: `destination_backend(effective, &profile, None);\n        let ceiling = destination_tier_ceiling(effective, &backend);` -> `... let ceiling = None;` | `bypass-fallback` | **killed** | `tier_ceiling::a_configured_ceiling_excludes_a_destination_below_the_required_tier_on_the_shipped_binary` |
| main.rs, the OTHER call site: `destination_backend(effective, &profile, record.model.clone());` + the comment + `let ceiling = destination_tier_ceiling(effective, &backend);` -> `... let ceiling = None;` | `bypass-fallback` | **killed** | `tier_ceiling::a_warm_sessions_ceiling_comes_from_the_model_it_is_running` |
| routing/session.rs hard_constraint: `&& offered < required` -> `&& offered > required` | `invert-condition` | **killed** | `tier_ceiling::a_hard_capability_outranks_raw_model_cheapness_at_a_lower_tier` |

> bypass-fallback observed: panicked at tier_ceiling.rs:250:28: nothing was rejected at all: [full route report]; three further tests failed with it, while --test session_router and --test routing_capability stayed green

> bypass-fallback observed: panicked at tier_ceiling.rs:395:5: nothing was rejected at all -- routing_destinations has two construction sites and §35's rule is about a call site, so both are mutated separately

> invert-condition observed: assertion `left == right` failed: with nothing required beyond a leaf tier, the free destination is the one to take -- the inverted gate refuses the frontier-ceiling destination for a leaf task

Recorded scope limits — stated by the worker, not discovered later:
- The required tier comes from RouterAnswer::requirements(); with no routing model configured that is classify_heuristically's answer, so the classifier's own accuracy is not what this proves.


---


### Include workload-tier fit in candidate scoring. (line 1531)

Contract: Given two eligible destinations alike in every other term, when one's established ceiling equals the required tier and the other's is above it, Glasshouse scores the exact fit strictly higher and chooses it -- while preserving that a destination with no established ceiling scores zero rather than a penalty and says so in words.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts.

Production evidence:
- `src/routing/session.rs` — `workload_tier_fit`
- `src/routing/session.rs` — `score (pushed only when a tier is stated)`
- `src/main.rs` — `destination_tier_ceiling`

Regression evidence:
- `tier_ceiling::tier_fit_orders_two_otherwise_equal_destinations`
- `tier_ceiling::without_a_ceiling_nothing_is_excluded_and_the_fit_term_says_not_established`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| routing/session.rs: `const TIER_FIT_EXACT: f64 = 0.4;` -> `= 0.2;` | `alter-boundary` | **killed** | `tier_ceiling::tier_fit_orders_two_otherwise_equal_destinations` |

> alter-boundary observed: assertion `left == right` failed: a ceiling equal to the required tier is the fit the router should prefer -- and, because the two then tie, the winner falls back to caller order and the chosen() assertion fails too

Recorded scope limits — stated by the worker, not discovered later:
- The winning profile is offered second so caller order cannot explain it; the three-way ordering exact > headroom > not-established is proven pairwise, not as a total order over all five tiers.

### Lines 1533 and 1551 — affinity and switching cost in the INTERACTIVE score, proved

Package `GH-PROVE-IT-MISC`, 2026-08-31, Sonnet at medium (Green): mechanisms the recon found already in production, proved by tests only. Four mutations, four killed in the worker's tree. **1174 is NOT closed by this package** — its test (`precompact_memory.rs`) failed one run in three on the merged tree even single-threaded (model called once, no memory stored within a 10 s bounded wait); see `.agent-runtime/defect-hook-extraction-may-lose-its-write.md`. The existing 35B entry had proved the disposable router's absence of these terms; these tests are scoped to `SessionRouting::choose`.


### Include existing session affinity in candidate scoring. (line 1533)

Contract: Given two otherwise-equal destinations differing only in existing-session warmth, when the interactive SessionRouter scores them, the warmer one wins and its explanation carries a non-zero, differing session-affinity contribution

State: COMPLETE — ruled 2026-08-31 by the orchestrator: a line the code already satisfied, proved by a test and a killed mutation (GH-MAP-SIDE-EFFECT-AUDIT's finding; no production change).

Production evidence:
- `src/routing/session.rs` — `session_affinity, called from score() at :3083, score() called from choose() at :2961`

Regression evidence:
- `interactive_score_terms::a_warm_candidates_explanation_carries_session_affinity_and_it_moves_the_ranking`
- `interactive_score_terms::the_interactive_routers_explanation_names_both_terms`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/routing/session.rs: delete explanation.push(session_affinity(...)) at score()'s line 3083 | `skip-state-update` | **killed** | `interactive_score_terms::a_warm_candidates_explanation_carries_session_affinity_and_it_moves_the_ranking` |

> skip-state-update observed: assertion `left == right` failed: the warmer session did not win: session affinity is not deciding anything

Recorded scope limits — stated by the worker, not discovered later:
- docs/product/evidence/phase-35b.md's existing entry proves the disposable-job scorer's absence of this term; this closes only the interactive router's presence of it


---


### Include session-switching and bootstrap cost in candidate scoring. (line 1551)

Contract: Given two otherwise-equal destinations differing only in whether they require a fresh-session bootstrap, when the interactive SessionRouter scores them, the cheaper-to-reach one wins and its explanation carries a non-zero switching-and-bootstrap-cost contribution that is worse for the bootstrapping candidate

State: COMPLETE — ruled 2026-08-31 by the orchestrator: a line the code already satisfied, proved by a test and a killed mutation (GH-MAP-SIDE-EFFECT-AUDIT's finding; no production change).

Production evidence:
- `src/routing/session.rs` — `switching_and_bootstrap_cost, called from score() at :3093, score() called from choose() at :2961`

Regression evidence:
- `interactive_score_terms::a_candidate_needing_a_bootstrap_carries_switching_and_bootstrap_cost_and_it_moves_the_ranking`
- `interactive_score_terms::the_interactive_routers_explanation_names_both_terms`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/routing/session.rs: delete explanation.push(switching_and_bootstrap_cost(...)) at score()'s line 3093 | `skip-state-update` | **killed** | `interactive_score_terms::a_candidate_needing_a_bootstrap_carries_switching_and_bootstrap_cost_and_it_moves_the_ranking` |

> skip-state-update observed: panicked on .expect("every candidate must be scored for switching and bootstrap cost") -- the contribution no longer exists in the explanation

Recorded scope limits — stated by the worker, not discovered later:
- same correction and same gap as 1533: the existing evidence entry covers the disposable scorer only


# Lines 1537, 1538 — COMPLETE 2026-09-01; 1534 investigated and OPEN with its link named

Package `GH-SCORE-TERMS-35B` (Sonnet, high, Amber; batch 73). Targeted gate
green on the merged tree; both mutations KILLED in-worktree.

- **1537 provider health**: a term over `RouterInputs.health`'s observed
  state for the candidate's provider — degraded scores against, healthy
  contributes nothing, the `why` line names the observation basis.
- **1538 expected marginal cost**: fires ONLY where the existing free-pool
  cost preference structurally cannot (`movement.is_none()`) — the
  partition that makes double-counting impossible rather than avoided by
  convention. **Orchestrator ruling on the worker's own flagged judgment
  call: the partition is the right shape.** The cross-branch interaction
  (a tier-movement decision plus the cost term in one scenario) deserves a
  test when a package with tier-movement fixtures makes it cheap; noted,
  not owed here.
- **1534 stays OPEN — the constraint working as designed.** Every context
  field reaching the scorer (`SessionContextFacts`: observed_compactions,
  last_task, touched_files, task_named_paths) is already fully consumed by
  Phase 36's affinity facets; a "context quality" term from the same fields
  would be the identical signal scored twice under a new name. The genuine
  missing link: a context-size producer (map line 1158 — behind the
  body-parsing wall per the register) or an independent staleness signal.
  No successor package until one of those exists; do not re-litigate this
  by counting open lines.

Packet error recorded: `cost_of` moved to `config/mod.rs:2870`.

---

# Line 1546 — held 2026-09-02, **CLOSED the same night** by `GH-CADENCE-CROSSING` (see the end of this section)

## The hold, kept because the reasoning is the point

**The ledger's blocker was stale and the work is good. The box still does not
tick, and the reason is production reach.**

The stale blocker: this file grouped 1546 under *"cadence — Phase 33A's ledger,
built this same round and not yet read by anything."* Line 1319 wired the
cadence signal and line 1368 made the gateway read it, so that has not been
true for a day.

## What landed, and it is sound

- `CooldownCause { Declared, Invented }` (`routing/free.rs`), recorded by
  `ResourceHealth::fail` on the branch it already had — `Some(retry_after)` is
  a provider's own cadence, the `None` path past `FAILURES_BEFORE_COOLDOWN` is
  Glasshouse's own caution. `Served` clears it with `cooling_down_until`.
- `ResourceHealth::declared_wait_remaining(now)` — `Some` **only** while a
  declared wait is in force. `is_available`'s meaning and signature untouched,
  as the packet required.
- `routing::session::cadence_availability` — a `Contribution` sibling to
  `provider_health`, pushed into the same `RoutingExplanation`
  (`session.rs:4629-4630`). Two terms, two names, two evidence sentences.

Both required mutations **KILLED**, each by the test that names the property:

| mutation | result | killed by |
|---|---|---|
| `CooldownCause::Invented` -> `::Declared` on the invented branch | **killed** | `routing_policy::cadence_availability_scoring::invented_backoff_alone_reports_cadence_available_while_provider_health_reflects_it` |
| `CADENCE_DECLARED_WAIT_PENALTY` -> `0.0` | **killed** | `routing_policy::cadence_availability_scoring::a_declared_wait_scores_strictly_worse_on_cadence_than_an_untouched_resource` |

> observed: *"an invented cooldown is Glasshouse's own caution, not a provider cadence, and must not score as one"*

## Why it is held: the term is structurally inert in the shipped binary

The worker reported the gap honestly and argued it did not block the line —
that `cadence_availability` works "for every resource whose health this process
observed directly (the normal, intended case)." **The integrator checked which
case production actually is, and it is the other one.**

`GatewayHealthReading` (`provider/telemetry.rs:1529`) persists
`credential_label`, `model`, `consecutive_failures`, `cooling_down_until_unix`
and `credential_rejected` — **no cause**. So `FreePool::adopt_observed` sets
`cooldown_cause = None` on every adoption, which is correct rather than
guessing.

And every production consumer of the new term reads an *adopted* pool:

- `SessionRouter::choose` is called from `main.rs:4073` and `main.rs:4842`;
  both take `health.pool()` from `observed_provider_health`
  (`main.rs:2611`) -> `observed_health_of` (`main.rs:2691`) ->
  `pool.adopt_observed(..)` (`main.rs:2747`), which is the **only**
  `adopt_observed` call site in the tree.
- The gateway process, which *does* hold directly-observed health
  (`SessionRouting.free`), does not call `SessionRouter::choose` at all.

So `declared_wait_remaining` returns `None` for every destination the shipped
router ever scores, and `cadence_availability` contributes a flat `0.0`
forever. **The mechanism is real, tested and mutation-proven, and its
production reach is zero** — the same shape as the `pricing-channel` hold, and
the shape behind all ten of this project's historical un-tickings.

**Successor, and it is cheap: carry the cause across the process boundary.**
`GatewayHealthReading` is a plain serde struct persisted as JSON by
`GatewayHealthCache` — **not a database migration**, so this is not the Red-tier
schema decision the packet's STOP CONDITIONS reserved. Add an optional
`cooldown_cause` field (absent in old files, defaulting to `None`), write it
where `cooling_down_until_unix` is written, and restore it in `adopt_observed`.
The proof this line then needs is one test showing a **declared** wait recorded
by the gateway process still scores worse after crossing the cache, and an
invented one does not. **When that lands, 1546 closes.**

Recorded limits carried into the hold:

- `CADENCE_DECLARED_WAIT_PENALTY` is `-1.5`, matching `HEALTH_UNAVAILABLE_PENALTY`.
  The packet left the weight to the worker within one bound; the relative
  severity of "paced" versus "unhealthy" has had no product ruling.
- The declared-case evidence string renders whole seconds, so a sub-second
  remainder is dropped from the *string* (never from the score, which is a flat
  penalty). Cosmetic.


---

## Line 1546 — CLOSED (`GH-CADENCE-CROSSING`, 2026-09-02)

The hold above named its successor: carry `CooldownCause` across the process
boundary so the scoring term actually fires. That is what landed.

`GatewayHealthReading` (`provider/telemetry.rs`) gained an **optional**,
serde-defaulted `cooldown_cause`, written by
`SessionRouting::health_readings_for` (`gateway/session.rs:332`) off the same
`ResourceHealth` it already reads `cooling_down_until()` from, and restored by
`FreePool::adopt_observed` (`routing/free.rs:500`) — which now takes the cause
instead of hardcoding `None`. **An absent cause still adopts as unknown**, so an
old cache file behaves exactly as before. No migration: the store is a JSON
cache, not a table.

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| `health_readings_for` writes the cause as `None` regardless | `cause-dropped-at-the-boundary` | **killed** | `gateway::session::tests::health_readings_for_carries_the_cooldown_cause_the_pool_already_recorded` |
| `adopt_observed` treats an absent cause as `Declared` | `unknown-cause-adopted-as-declared` | **killed** | the old-cache-file acceptance test (`routing_policy.rs:1610`) |

> observed: *"a provider-declared wait must cross as a recorded Declared cause, never dropped to None"*

### Two corrections to the orchestrator's packet, both the worker's

1. **The struct-field ripple was seven files, not the one the packet named.**
   Every full-field-list `GatewayHealthReading { .. }` literal, all
   `#[cfg(test)]`: `provider/resources.rs`, `shell/mod.rs`, and five test
   files. The worker enumerated them, applied the same one-line fix the packet
   pre-authorised for `shell/mod.rs`, and verified completeness with a grep
   before and after plus a clean `--all-targets` build.

2. **The packet's own acceptance test could not have killed the load-bearing
   mutation, and the worker proved it rather than guessing.** The packet put
   the crossing test in `tests/routing_policy.rs` and required it to kill a
   mutation on `health_readings_for`, which is `pub(super)` — unreachable from
   an external integration-test binary that links the crate and sees only `pub`
   items. Written as specified, the test necessarily **hand-mirrors** the
   field-mapping instead of calling it, and the mutation **SURVIVED**
   (`34 passed; 0 failed` — nothing observed the mutated line at all). The
   worker diagnosed the visibility cause, kept that test (it still proves the
   adopt-and-score half after a real JSON round trip), and added a fourth test
   inside `gateway/session.rs`'s own `#[cfg(test)] mod tests` that drives the
   real `observe_exchange -> health_readings_for` path. **That** one kills it.

**The rule, recorded because a false SURVIVED is worse than no mutation — it
looks exactly like a real coverage gap:** *a mutation on a non-`pub` item needs
its killing test in the same crate. Check the mutated item's visibility when
writing the MUTATION section, and list that source file under EXPECTED FILES.*
