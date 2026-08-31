# Capability evidence — phase 34F

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Line 1480 — task outcomes by workload tier, from rows the router already writes

Package `GH-TIER-OUTCOMES`, 2026-08-31, Sonnet at high (Amber). One reader, `outcomes_by_tier`, joining `RoutingTierObserved` (1834) with `RoutingOutcomeObserved` (1835) per session inside the evidence window; summarised only at `MIN_SAMPLE_FOR_SUMMARY`, undecided sessions never counted as failures; printed by `glasshouse route`, acted on by nothing (1481 is a later package). The register's Cluster O row for 34F predates both producers and is stale for this line only. Three mutations, three killed; gates quoted (fmt, clippy, lib 1696, bin 49, `tier_outcomes` 2).

### Record successful and failed task outcomes by workload tier when enough evidence exists. (line 1480)

Contract: Given a window of routed sessions, when Glasshouse counts a workload tier's reported turns, it reports the successful and failed counts once at least MIN_SAMPLE_FOR_SUMMARY reported turns exist for that tier and insufficient evidence with the count otherwise, while preserving that a session with a tier decision and no turn end yet is undecided and never counted as failed, and that an escalated tier is never folded into its non-escalated sibling

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts and the diff of the reader.

Production evidence:
- `src/evaluation/mod.rs` — `EvaluationObservations::outcomes_by_tier`
- `src/evaluation/mod.rs` — `TierOutcome, TierOutcome::from_counts`
- `src/evaluation/mod.rs` — `TierOutcomeVerdict`
- `src/main.rs` — `tier_outcome_section`
- `src/main.rs` — `route_report (wiring)`

Regression evidence:
- `tier_outcomes::outcomes_by_tier_gates_by_sample_and_never_counts_undecided_as_failed`
- `tier_outcomes::the_route_command_prints_the_tier_outcomes_section`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| route_outcomes_by(EvaluationKind::RoutingTierObserved, from, to) -> route_outcomes_by(EvaluationKind::RoutingCostClassObserved, from, to) | `join-wrong-key` | **killed** | `tier_outcomes::outcomes_by_tier_gates_by_sample_and_never_counts_undecided_as_failed` |
| if sample_size < MIN_SAMPLE_FOR_SUMMARY as i64 -> if false | `drop-minimum-sample-gate` | **killed** | `tier_outcomes::outcomes_by_tier_gates_by_sample_and_never_counts_undecided_as_failed` |
| failed: counts.failed, -> failed: counts.failed + counts.sessions_without_outcome, | `count-undecided-as-failed` | **killed** | `tier_outcomes::outcomes_by_tier_gates_by_sample_and_never_counts_undecided_as_failed` |

> join-wrong-key observed: no `heavy`/`leaf`/`standard-escalated`/`unclassified` bucket found among the cost-class vocabulary the mutated query now reads

> drop-minimum-sample-gate observed: assertion `left == right` failed: three reported turns is below the gate, and the count is carried rather than hidden: TierOutcome { bucket: "leaf", undecided: 0, verdict: Measured { successful: 2, failed: 1, sample_size: 3 } }

> count-undecided-as-failed observed: assertion `left == right` failed: five completed turns clear the gate with zero failures: TierOutcome { bucket: "heavy", undecided: 2, verdict: Measured { successful: 5, failed: 2, sample_size: 5 } }

Recorded scope limits — stated by the worker, not discovered later:
- the fourth candidate mutation the packet named ("collapse escalated and non-escalated into one bucket") has no corresponding line in this diff to mutate — the bucket string passes through unchanged from RoutingTier::as_str, an already-tested closed vocabulary from GH-EVALUATION-PRODUCERS
- no Windows leg run
- line 1481 (calibration suggestions) is explicitly not built here

