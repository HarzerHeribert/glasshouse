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

