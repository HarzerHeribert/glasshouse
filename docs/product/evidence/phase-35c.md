# Capability evidence — phase 35C

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 35C — line 1558, the cheapest healthy adequate candidate

Package `GH-TIER-CEILING`, 2026-08-31, Opus at high. Nine mutations, nine killed. The worker **refused OBJECTIVE 3** — attaching adapter-declared `ResourceFacts` to destinations — and the orchestrator verified the refusal: `capability_fit` (`routing/session.rs:786`) already reads `adapter_for(destination.harness())` and `prefer()` falls through to those declarations whenever the facts are `Unverified`, so the wiring would have changed no score and survived its own mutation; `Destination::with_resource_facts` keeps no production caller, deliberately. 1558 needed a term: the only cost-sensitive terms in the router are inert unless a capacity reading is cached AND the band is Tight or worse, so two adequate healthy candidates differing only in price tied — `cost_preference` is that term.


### Prefer the cheapest healthy candidate that satisfies the required workload tier and hard capabilities. (line 1558)

Contract: Given several healthy candidates that all satisfy the required workload tier and the required hard capabilities, when Glasshouse ranks them it prefers the one that costs the user nothing -- while preserving that adequacy, capability and health each outrank price by construction, and that a launch stating no task is scored and rendered exactly as it was before this preference existed.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts.

Production evidence:
- `src/routing/session.rs` — `cost_preference`
- `src/routing/session.rs` — `METERED_COST_PREFERENCE`
- `src/routing/session.rs` — `score (pushed under the same `if let Some(required)` as workload_tier_fit)`

Regression evidence:
- `tier_ceiling::the_cheapest_healthy_adequate_candidate_wins`
- `tier_ceiling::a_task_less_route_reads_exactly_as_it_did_before_ceilings_existed`
- `route_command::empty_or_whitespace_only_task_text_behaves_as_absent`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| routing/session.rs: `const METERED_COST_PREFERENCE: f64 = -0.1;` -> `= 0.0;` | `remove-guard` | **killed** | `tier_ceiling::the_cheapest_healthy_adequate_candidate_wins` |

> remove-guard observed: panicked at tier_ceiling.rs:496:5 -- with the term zeroed the two candidates tie and the winner is the metered profile, which is offered first

Recorded scope limits — stated by the worker, not discovered later:
- -0.1 is a chosen number with a stated constraint (strictly smaller than every other differentiating constant in the module, the smallest of which is 0.2), not a measured one. Nothing fails if a future term is added below 0.2.
- `healthy` is priced by provider_health, not re-decided here; the test's candidates have no observed health at all, so this proves the cost ordering among candidates health could not separate rather than proving an unhealthy cheap candidate loses.

### Lines 1559–1566 — tier escalation and downgrade, the movement decision

Package `GH-ESCALATION`, 2026-08-31, Fable 5 at xhigh. A tier-movement decision computed beside the existing contributions (`decide_tier_movement`): escalation triggers in order — an attributable `RequestIncompatibility | EmptyCompletion` on retry (1564), every candidate at the classified tier struggling (1559), a heuristic-only verdict naming heavy work (1560) — capped at `MAX_ESCALATION_STEPS = 1` and announced (1565); downgrade of routine support work when the band is `DOWNGRADE_PRESSURE_BAND` (Tight) or worse (1562), refused when the task's own `DurationClass` exceeds `DOWNGRADE_RETRY_TOLERANCE` (1563 — the packet's *median milliseconds against a saving nothing prices* was rightly refused); a warm higher-tier session holds a downgrade on the affinity term itself (1561); every fired decision recorded under `TIER_ESCALATION_PURPOSE` / `TIER_DOWNGRADE_PURPOSE` (1566). Four packet corrections in `packet_errors`, all accepted: Phase 35C has eight open lines, not nine; `JobKind` cannot be reached from `session.rs` (`routing/mod.rs`'s own test forbids it) so 1562 keys on the classification; `on_provider_failure` only sees `from_status` failures, so 1564's classes arrive through `retry_after`.

**Thin spot, stated:** the worker's report carried the template placeholders for `status:` and `gates:` and quoted no `test result:` line; asked to fill them, it re-signalled done unchanged. Integration supplied what it did not: `cargo check`, clippy `-D warnings`, seven targeted targets (`tier_escalation` 13 passed among them), the symbol-loss check, and the batch gate. One cross-package seam was found there — affinity had widened `session_affinity` to three arguments; the brake now passes `current = None` (no move exists yet to read cache locality from), which makes it slightly less likely to hold.

### Escalate to a higher tier when lower-tier candidates are unhealthy, exhausted, or repeatedly fail the task. (line 1559)

Contract: Given a classified tier and a candidate set in which every destination established at that tier is refused, cooling down, exhausted or repeatedly failing, when Glasshouse ranks them, it escalates its preference one tier and scores a healthy candidate established at the tier above as the exact fit, while preserving that the struggling candidates stay eligible, that a set with no healthy higher-tier candidate holds and says so, and that a launch stating no task renders exactly as before.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's mutation artifacts (twelve, each with its killing test and output) and the diff of the decision; the report shipped WITHOUT its gate lines, so the gate artifact is the orchestrator's batch gate on the merged tree (`.agent-runtime/blast-batch58.log`), recorded here rather than taken on assertion.

Production evidence:
- `src/routing/session.rs` — `decide_tier_movement`
- `src/routing/session.rs` — `struggling`
- `src/routing/session.rs` — `TierMovement::preferred_tier`
- `src/routing/session.rs` — `workload_tier_fit (the below-moved arm)`
- `src/routing/session.rs` — `SessionRouter::choose (the two-half gate)`

Regression evidence:
- `tier_escalation::every_struggling_candidate_at_the_classified_tier_escalates_the_preference_one_step`
- `tier_escalation::a_healthy_candidate_at_the_classified_tier_holds_the_preference`
- `tier_escalation::an_escalation_with_no_healthy_target_is_held_and_says_so`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| session.rs decide_tier_movement: `.all(|destination| struggling(destination, inputs))` -> `.all(|destination| !struggling(destination, inputs))` | `invert-condition` | **killed** | `tier_escalation::every_struggling_candidate_at_the_classified_tier_escalates_the_preference_one_step` |

> invert-condition observed: panicked at tier_escalation.rs:285: assertion `left == right` failed: the only standard-tier candidate is refused by its provider -- the movement was Held, not Escalated; routine_work_under_tight_premium_capacity_is_downgraded_to_a_free_resource failed with it (:600), because the inverted rule fires on a healthy tier and holds on a missing target instead of downgrading

Recorded scope limits — stated by the worker, not discovered later:
- REPEATED_FAILURES (2) equals free.rs FAILURES_BEFORE_COOLDOWN (2): a second consecutive failure also starts a cooldown, so the repeated-failure arm is redundant with !is_available in this build; the refused and exhausted arms are what the tests prove.
- The winner changes only when two candidates sit above the classified tier (exact vs headroom); with one, the health penalty already picks it and the movement adds the named decision and the fit reading.
- Struggling is read from the same FreePool and band the health and pressure terms read; a ledger failure count (provider_health_failures) is not consulted here.


---


### Escalate to a higher tier when the routing classifier reports low confidence and task failure would be expensive. (line 1560)

Contract: Given a task deterministic heuristics rated heavy or above at medium confidence with no model confirming it, when Glasshouse ranks destinations, it escalates its preference one tier to a healthy candidate established there, while preserving that a model's own heavy verdict is not escalated, that line 1459's low-confidence escalation is never applied twice, and that without a healthy candidate at the tier above the preference holds and says so.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's mutation artifacts (twelve, each with its killing test and output) and the diff of the decision; the report shipped WITHOUT its gate lines, so the gate artifact is the orchestrator's batch gate on the merged tree (`.agent-runtime/blast-batch58.log`), recorded here rather than taken on assertion.

Production evidence:
- `src/routing/session.rs` — `decide_tier_movement (HeuristicHeavy arm)`
- `src/routing/session.rs` — `EXPENSIVE_FAILURE_TIER`

Regression evidence:
- `tier_escalation::a_heuristic_heavy_verdict_escalates_and_a_model_confirmed_one_does_not`
- `tier_escalation::the_shipped_binary_announces_and_records_an_escalation`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| session.rs decide_tier_movement: `&& answer.stated_tier() >= EXPENSIVE_FAILURE_TIER` -> `&& answer.stated_tier() > EXPENSIVE_FAILURE_TIER` | `alter-boundary` | **killed** | `tier_escalation::a_heuristic_heavy_verdict_escalates_and_a_model_confirmed_one_does_not` |

> alter-boundary observed: panicked at tier_escalation.rs:412: assertion `left == right` failed: destination a-heavy ... (fresh) -- with `>` a heavy verdict no longer qualifies and nothing moved; the_shipped_binary_announces_and_records_an_escalation failed with it (:911)

Recorded scope limits — stated by the worker, not discovered later:
- The line's literal conjunction (low confidence and expensive failure) is consumed upstream: RouterAnswer::required_tier() is conservative_workload_tier(), which escalates every Low-confidence task; this rule adds the medium-confidence heuristic-heavy case and skips a conservative answer.
- 'Failure would be expensive' is the stated tier at or above Heavy; nothing prices a failure cost.


---


### Preserve a warm higher-tier session when its existing context makes it cheaper or safer than starting a nominally cheaper cold session. (line 1561)

Contract: Given routine work under premium pressure that would otherwise be downgraded and an existing session established at or above the classified tier whose session-affinity contribution is at least the tier delta an exact fit is worth over headroom, when Glasshouse ranks, it holds the tier and continues the warm session, while preserving that the same session resumable and idle long enough for its affinity to decay below that delta no longer holds it and the downgrade proceeds.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's mutation artifacts (twelve, each with its killing test and output) and the diff of the decision; the report shipped WITHOUT its gate lines, so the gate artifact is the orchestrator's batch gate on the merged tree (`.agent-runtime/blast-batch58.log`), recorded here rather than taken on assertion.

Production evidence:
- `src/routing/session.rs` — `decide_tier_movement (WarmContext arm)`
- `src/routing/session.rs` — `WARM_CONTEXT_TIER_DELTA`
- `src/routing/session.rs` — `session_affinity (reused, not re-derived)`

Regression evidence:
- `tier_escalation::a_warm_higher_tier_session_holds_the_downgrade_until_its_warmth_decays`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| session.rs decide_tier_movement: `.find(|(_, affinity)| *affinity >= WARM_CONTEXT_TIER_DELTA)` -> `*affinity < WARM_CONTEXT_TIER_DELTA` | `invert-condition` | **killed** | `tier_escalation::a_warm_higher_tier_session_holds_the_downgrade_until_its_warmth_decays` |

> invert-condition observed: panicked at tier_escalation.rs:715: the live session no longer held the downgrade (movement was Downgraded, not Held WarmContext)

Recorded scope limits — stated by the worker, not discovered later:
- Warmth is the whole of the affinity this build computes (session_affinity's own doc); 'cheaper or safer' is warmth against the tier delta, not a priced comparison.
- Reaches only the downgrade path: an escalation moves a preference toward a stronger tier and abandons nothing by construction.


---


### Downgrade routine support work to free, local, or low-cost resources when premium capacity is tight. (line 1562)

Contract: Given routine support work (a question or investigation classified above leaf) and a candidate set where every metered destination with a capacity reading is in the tight band or worse, when Glasshouse ranks, it downgrades the tier one step so a free, available, adequate destination established at that tier becomes eligible and the exact fit while low-tier spend prices the premium destination out, while preserving that a healthy premium destination, non-routine work, or no free destination leaves the classified gate exactly as it was.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's mutation artifacts (twelve, each with its killing test and output) and the diff of the decision; the report shipped WITHOUT its gate lines, so the gate artifact is the orchestrator's batch gate on the merged tree (`.agent-runtime/blast-batch58.log`), recorded here rather than taken on assertion.

Production evidence:
- `src/routing/session.rs` — `decide_tier_movement (downgrade arm)`
- `src/routing/session.rs` — `ROUTINE_SUPPORT_CEILING`
- `src/routing/session.rs` — `DOWNGRADE_PRESSURE_BAND`
- `src/routing/session.rs` — `one_tier_below`
- `src/routing/session.rs` — `TierMovement::gate_tier`
- `src/routing/session.rs` — `TierMovement::pressure_tier`
- `src/routing/session.rs` — `hard_constraint (minimum_tier argument)`

Regression evidence:
- `tier_escalation::routine_work_under_tight_premium_capacity_is_downgraded_to_a_free_resource`
- `tier_escalation::a_healthy_premium_resource_or_non_routine_work_is_not_downgraded`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| session.rs: `const DOWNGRADE_PRESSURE_BAND: CapacityBand = CapacityBand::Tight;` -> `CapacityBand::Reserve;` | `alter-boundary` | **killed** | `tier_escalation::a_multi_turn_task_is_not_downgraded_because_its_retry_costs_more_than_it_saves` |
| session.rs TierMovement::gate_tier: `Self::Downgraded { to, .. } => *to,` -> `Self::Downgraded { .. } => self.classified(),` | `bypass-fallback` | **killed** | `tier_escalation::routine_work_under_tight_premium_capacity_is_downgraded_to_a_free_resource` |
| session.rs TierMovement::pressure_tier: `self.classified().min(self.preferred_tier())` -> `.max(` | `invert-condition` | **killed** | `tier_escalation::routine_work_under_tight_premium_capacity_is_downgraded_to_a_free_resource` |

> alter-boundary observed: panicked at tier_escalation.rs:677: assertion `left == right` failed: destination a-premium ... -- with Reserve as the threshold the tight band no longer counts as pressure, so the movement was Held NoTrigger rather than Held RetryCost; a_warm_higher_tier_session_holds_the_downgrade_until_its_warmth_decays failed with it (:715)

> bypass-fallback observed: panicked at tier_escalation.rs:610: z-free was rejected by the workload-tier constraint -- the downgrade no longer reached the gate; a_warm_higher_tier_session_holds_the_downgrade_until_its_warmth_decays failed with it (:750, chosen w-warm instead of z-free)

> invert-condition observed: panicked at tier_escalation.rs:612: assertion `left == right` failed -- a-premium's `low-tier spend` was 0.0, not -3.0: the pressure terms read the classified tier and stayed inert; a_warm_higher_tier_session_holds_the_downgrade_until_its_warmth_decays failed with it (:750)

Recorded scope limits — stated by the worker, not discovered later:
- Reachable on the shipped binary only through a model classification: classify_heuristically rates every question/investigation Leaf and every Standard task a code modification, and Leaf has no tier below it a model serves. Inert with heuristics only.
- 'Free, local or low-cost' is decided by Cost::Free (the user's free_models) alone; no locality or price producer exists.
- Phase 39's JobKind never reaches SessionRouter (session.rs may not name disposable); RouterAnswer::task_class() is the fact that names the same work here.


---


### Avoid downgrading work when the expected cost of failure and retry exceeds the premium-resource savings. (line 1563)

Contract: Given routine work under premium pressure that qualifies for a downgrade, when its expected duration is longer than a single turn, Glasshouse holds the classified tier and says a failed retry would cost more than the downgrade saves, while preserving that single-turn work is still downgraded.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's mutation artifacts (twelve, each with its killing test and output) and the diff of the decision; the report shipped WITHOUT its gate lines, so the gate artifact is the orchestrator's batch gate on the merged tree (`.agent-runtime/blast-batch58.log`), recorded here rather than taken on assertion.

Production evidence:
- `src/routing/session.rs` — `decide_tier_movement (RetryCost arm)`
- `src/routing/session.rs` — `DOWNGRADE_RETRY_TOLERANCE`

Regression evidence:
- `tier_escalation::a_multi_turn_task_is_not_downgraded_because_its_retry_costs_more_than_it_saves`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| session.rs decide_tier_movement: `if duration > DOWNGRADE_RETRY_TOLERANCE {` -> `if duration < DOWNGRADE_RETRY_TOLERANCE {` | `invert-condition` | **killed** | `tier_escalation::a_multi_turn_task_is_not_downgraded_because_its_retry_costs_more_than_it_saves` |

> invert-condition observed: panicked at tier_escalation.rs:677: assertion `left == right` failed: destination z-free ... (fresh) -- the long-running task was downgraded and the free resource won

Recorded scope limits — stated by the worker, not discovered later:
- Neither the cost of failure nor the saving is priced: the rule is the task's own DurationClass against a stated tolerance. The packet's median_duration_ms comparison has no producer for the saving side.
- With heuristics, routine work is never multi-turn, so on the binary this brake needs a model classification stating a duration.


---


### Allow retry policy to promote a task by one tier after a clearly attributable model-capability failure. (line 1564)

Contract: Given a task-boundary routing decision with a current destination whose most recent evidence-ledger row is a request-incompatibility or empty-completion failure, when Glasshouse ranks, it promotes its preference one tier and names the failure, while preserving that a throttle, upstream, credential or quota failure promotes nothing and is named as priced elsewhere, and that a session start reads nothing.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's mutation artifacts (twelve, each with its killing test and output) and the diff of the decision; the report shipped WITHOUT its gate lines, so the gate artifact is the orchestrator's batch gate on the merged tree (`.agent-runtime/blast-batch58.log`), recorded here rather than taken on assertion.

Production evidence:
- `src/routing/evidence.rs` — `EvidenceLedger::latest_failure_class_for_model`
- `src/main.rs` — `latest_failure_class`
- `src/main.rs` — `route_recommendation (with_retry_after on the task-boundary path)`
- `src/routing/session.rs` — `SessionRouter::with_retry_after`
- `src/routing/session.rs` — `decide_tier_movement (AttributableFailure arm)`

Regression evidence:
- `tier_escalation::an_attributable_failure_promotes_one_tier_and_a_health_failure_does_not`
- `tier_escalation::a_task_boundary_route_promotes_after_the_ledgers_last_attributable_failure`
- `routing::evidence::tests::the_latest_failure_class_is_the_most_recent_rows_and_nothing_older`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| session.rs decide_tier_movement: `Some(class @ (FailureClass::RequestIncompatibility | FailureClass::EmptyCompletion))` -> `... | FailureClass::Throttle))` | `wrong-class` | **killed** | `tier_escalation::an_attributable_failure_promotes_one_tier_and_a_health_failure_does_not` |
| evidence.rs latest_failure_class_for_model: `ORDER BY observed_at DESC, seq DESC` -> `ORDER BY observed_at ASC, seq ASC` | `alter-order` | **killed** | `routing::evidence::tests::the_latest_failure_class_is_the_most_recent_rows_and_nothing_older` |

> wrong-class observed: panicked at tier_escalation.rs:473: assertion `left == right` failed: empty_completion -- no longer promoted; two_triggers_move_one_tier_and_the_second_is_named_as_capped failed with it (:549)

> alter-order observed: panicked at evidence.rs:2378: assertion `left == right` failed: the most recent row, not the first or the most frequent -- the oldest row (throttle) was returned

Recorded scope limits — stated by the worker, not discovered later:
- Attribution is the latest ledger row for this provider and model in the project, not this session's last exchange: rows carry no session id.
- Reaches only `glasshouse route --moment task-boundary`; a launch is a session start with no current destination and `resume` states no task.
- The packet's named source, interactive.rs on_provider_failure, never sees these classes (ProviderFailure::from_status maps 429/5xx only); the ledger is the only producer.


---


### Cap automatic escalation so a malformed task cannot consume every premium resource without user visibility. (line 1565)

Contract: Given a decision on which more than one escalation trigger fires, when Glasshouse ranks, it moves its preference exactly one tier, names the applied trigger and every capped one, renders the movement as a heading of the route report, and announces it on stderr before the destination on a launch, while preserving that a held movement announces nothing.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's mutation artifacts (twelve, each with its killing test and output) and the diff of the decision; the report shipped WITHOUT its gate lines, so the gate artifact is the orchestrator's batch gate on the merged tree (`.agent-runtime/blast-batch58.log`), recorded here rather than taken on assertion.

Production evidence:
- `src/routing/session.rs` — `MAX_ESCALATION_STEPS`
- `src/routing/session.rs` — `decide_tier_movement (steps and capped)`
- `src/routing/session.rs` — `TierMovement::describe`
- `src/routing/session.rs` — `Routed::render (the tier heading)`
- `src/main.rs` — `launch_session (the announcement)`

Regression evidence:
- `tier_escalation::two_triggers_move_one_tier_and_the_second_is_named_as_capped`
- `tier_escalation::the_shipped_binary_announces_and_records_an_escalation`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| session.rs: `const MAX_ESCALATION_STEPS: usize = 1;` -> `= 2;` | `alter-boundary` | **killed** | `tier_escalation::two_triggers_move_one_tier_and_the_second_is_named_as_capped` |
| main.rs launch_session: `routed.movement().filter(|movement| movement.fired())` -> `!movement.fired()` | `invert-condition` | **killed** | `tier_escalation::the_shipped_binary_announces_and_records_an_escalation` |

> alter-boundary observed: panicked at tier_escalation.rs:549: assertion `left == right` failed: destination a-frontier ... (fresh) -- two triggers moved two tiers and the frontier candidate won

> invert-condition observed: panicked at tier_escalation.rs:955: the launch's stderr no longer carried `glasshouse: tier escalated from `heavy` to `frontier``

Recorded scope limits — stated by the worker, not discovered later:
- The cap is per decision; nothing caps across successive task boundaries. The 1566 rows are what a later reader would count that from.


---


### Record escalation and downgrade decisions for later evaluation. (line 1566)

Contract: Given a launch whose routing decision escalated or downgraded the tier, when Glasshouse acts on it, it records one evidence-ledger row under tier-escalation or tier-downgrade with the harness and the time, counted in RoutingOverhead's own tier-movement bucket, while preserving that a held movement and a route report record nothing and that the row cannot blend into a model's latency summary.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's mutation artifacts (twelve, each with its killing test and output) and the diff of the decision; the report shipped WITHOUT its gate lines, so the gate artifact is the orchestrator's batch gate on the merged tree (`.agent-runtime/blast-batch58.log`), recorded here rather than taken on assertion.

Production evidence:
- `src/routing/evidence.rs` — `TIER_ESCALATION_PURPOSE`
- `src/routing/evidence.rs` — `TIER_DOWNGRADE_PURPOSE`
- `src/routing/evidence.rs` — `RoutingOverhead::from_consumption (tier_movement bucket)`
- `src/main.rs` — `record_tier_movement`

Regression evidence:
- `tier_escalation::the_shipped_binary_announces_and_records_an_escalation`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| main.rs launch_session: `record_tier_movement(runtime, selection.id(), movement);` -> `let _ = (runtime, movement);` | `skip-state-update` | **killed** | `tier_escalation::the_shipped_binary_announces_and_records_an_escalation` |

> skip-state-update observed: panicked at tier_escalation.rs:959: assertion `left == right` failed: line 1566: one row per movement acted on -- the ledger held 0 tier-escalation rows, not 1

Recorded scope limits — stated by the worker, not discovered later:
- The row records direction, harness and time; the tiers and the destination have no column, and adding one is a migration.
- Only the escalation row is proven on the shipped binary (1562's reachability limit); the downgrade purpose shares the one match arm.
- The RoutingOverhead bucket is rendered by the overhead output but has no test of its own line.

