# Capability evidence — phase 39

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 39 — lines 1607, 1609, 1611; 1608 refused

Package `GH-SUPPORT-WORK-ECONOMY`, 2026-08-31, Opus at high. Six mutations, six killed. **1608 refused** and stays open: Cluster Q — `JobKind` is Classification | MemoryExtraction | Reranking | Evaluation, no repository-summarization job exists, so there is nothing to prefer a cheap resource *for*. The worker left a tripwire (`no_repository_summarization_job_exists_to_route_cheaply_yet`) that stops compiling if a variant is added, and did not tick the box.


### Prefer local or free resources for trivial classification and extraction work when suitable. (line 1607)

Contract: Given trivial classification and extraction work, when Glasshouse picks the resource to run it, it prefers a free resource for extraction and a local one for classification among candidates that are otherwise equally adequate — while preserving that extraction's selector ranks on the user's own free-resource order and never on locality, so the line's disjunction is satisfied by the half each job actually has.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts.

Production evidence:
- `src/routing/disposable.rs` — `DisposableRouting::choose (the free-before-metered order)`
- `src/routing/disposable.rs` — `DisposableRouting::classification_preferences (the locality term)`
- `src/routing/disposable.rs` — `DisposableRouting::choose_for_automatic_classification (the preference pre-order)`
- `src/main.rs` — `disposable_candidates (with_locality from ResourceKind::locality)`

Regression evidence:
- `support_work_economy::free_capacity_is_preferred_for_extraction_on_the_shipped_binary`
- `support_work_economy::a_local_free_candidate_is_preferred_for_trivial_classification_and_extraction`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/routing/disposable.rs: the free set's `.filter(|candidate| candidate.value().cost().is_free())` -> `.filter(|candidate| !candidate.value().cost().is_free())` | `invert-predicate` | **killed** | `support_work_economy::free_capacity_is_preferred_for_extraction_on_the_shipped_binary` |
| src/routing/disposable.rs: locality's Local arm weight `CLASSIFICATION_PREFERENCE_WEIGHT,` -> `0.0,` | `zero-the-term` | **killed** | `support_work_economy::a_local_free_candidate_is_preferred_for_trivial_classification_and_extraction` |

> invert-predicate observed: panicked at support_work_economy.rs:484: the free model must be the one chosen — the stored rationale named the metered model instead

> zero-the-term observed: assertion `left == right` failed: a local candidate must be preferred over an equally adequate remote one; left `a-remote-model`, right `a-local-model`

Recorded scope limits — stated by the worker, not discovered later:
- Extraction has NO locality preference. `choose`'s free loop walks the user's configured order and consults no score, so the locality term reaches an extraction rationale as text only. Free-first is what satisfies the disjunction for extraction.
- The classification half is proven at `choose_for_automatic_classification`, not through the binary: a local candidate needs a registry-known local provider slug (`ollama` / `llama-cpp`) that a fixture cannot fabricate.


---


### Prefer premium warm sessions for difficult tasks that benefit strongly from existing context. (line 1609)

Contract: Given a person's own routing decision with a warm session on a premium resource in the reserve band and a cheaper cold alternative beside it, when the task is classified heavy, Glasshouse keeps the warm premium session and names the tier that justified it; when the same setup is given trivial work, it moves to the cheaper alternative — while preserving that 'benefits strongly from existing context' is not measured per task, so the preference is conditioned on difficulty and warmth, which are the two signals this build actually has.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts.

Production evidence:
- `src/main.rs` — `classify_for_routing (the --task classification, reached from both `route` and `launch`)`
- `src/routing/session.rs` — `SessionRouter::choose (PressureInputs.tier = requirements.minimum_tier)`
- `src/routing/pressure.rs` — `capacity_band_pressure (the reserve arm, via evaluate_reserve_spend)`
- `src/routing/session.rs` — `session_affinity`

Regression evidence:
- `support_work_economy::a_heavy_task_keeps_its_warm_premium_session_on_the_shipped_binary`
- `support_work_economy::a_heavy_task_keeps_its_warm_premium_session`
- `support_work_economy::the_route_command_carries_a_tasks_difficulty_into_the_ranking`
- `support_work_economy::the_two_task_descriptions_this_file_relies_on_really_do_classify_differently`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/main.rs: `let text = site.task.map(str::trim).filter(|text| !text.is_empty())?;` -> `... .filter(|_| false)?;` — the task's difficulty never reaches the ranking | `cut-the-input` | **killed** | `support_work_economy::a_heavy_task_keeps_its_warm_premium_session_on_the_shipped_binary` |

> cut-the-input observed: panicked at support_work_economy.rs:1036: a difficult task must keep the warm premium session rather than start somewhere cheaper and cold — the ranking chose a `fresh:` destination

Recorded scope limits — stated by the worker, not discovered later:
- NO NEW TERM WAS NEEDED and none was added; `pressure.rs` and `session.rs` are untouched. The packet's contingency did not fire.
- RULING OWED: 'that benefit strongly from existing context' is not measured per task. `session_affinity`'s own doc records that Phase 36's same-task, touched-file and semantic-quality signals (1581-1588) have no producer, so warmth is the whole affinity signal and the preference applies to difficult tasks generally. Precedent for closing on the nearest real signal with the limit named is 1438 ('quality requirements are the reliability floor'). If the orchestrator reads the qualifier as load-bearing, this is `open` and its successor is Phase 36's affinity producers.
- Warmth in the fixture is a `resumable` session at +0.750, not a live one at the full weight. The ordering holds at either magnitude for the heavy case; the leaf flip is carried by RESERVE_DENIED_PENALTY -2.0 plus LOW_TIER_SPEND_PENALTY -3.0 together.
- The two binary route tests are `#[cfg_attr(windows, ignore)]` — the fake-harness launch they need is the unix shim.


---


### Avoid spending premium model capacity on internal Glasshouse bookkeeping when a cheap resource can perform it reliably. (line 1611)

Contract: Given Glasshouse's own bookkeeping, when a free adequate resource is available, Glasshouse does not reach a metered one at all; and for classification, where reliability is measured, a free candidate below the reliability floor is not treated as one that can perform the job — while preserving that extraction has no reliability record and its verdict therefore rests on the metered gate alone.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts.

Production evidence:
- `src/routing/disposable.rs` — `DisposableRouting::choose (the metered loop runs only after every free candidate is exhausted)`
- `src/routing/disposable.rs` — `classification_verdict (CLASSIFICATION_RELIABILITY_FLOOR)`

Regression evidence:
- `support_work_economy::premium_capacity_is_not_spent_on_bookkeeping_when_a_cheap_reliable_resource_exists`
- `support_work_economy::free_capacity_is_preferred_for_extraction_on_the_shipped_binary`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/routing/disposable.rs: `if fraction < CLASSIFICATION_RELIABILITY_FLOOR {` -> `if false {` | `disable-the-gate` | **killed** | `support_work_economy::premium_capacity_is_not_spent_on_bookkeeping_when_a_cheap_reliable_resource_exists` |

> disable-the-gate observed: assertion `left == right` failed: a cheap resource below the reliability floor is not one that can perform the job reliably; the 1-of-10 free candidate was chosen over the metered one

Recorded scope limits — stated by the worker, not discovered later:
- SPLIT VERDICT. The `reliably` clause is measured for CLASSIFICATION only. Extraction has no reliability record — `routing_observations` rows carry no purpose for an extraction call — so extraction's verdict rests on the metered gate alone. If the ledger wants the clause proven for both jobs, record this as 1611-partial until the extraction purpose stamp (`evaluation-producers`) has a reader.
- The reliability floor never filters a pinned model, and rows written before GH-ROUTING-ECONOMICS carry no outcome and count toward neither side.

