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


# Lines 1475–1479, 1483–1485 — COMPLETE 2026-09-01; 1482 OPEN by ruling

Package `GH-TIER-AXIS-34F` (Sonnet, high, Amber; batch 70b). One mechanism:
`ModelCapabilityRecord` (`config/capability.rs`, new) under
`providers.<p>.model_capabilities.<model>`, resolved by `resolve_ceiling` /
`ProviderConfig::resolved_ceiling` and consumed in production through the
already-wired `EffectiveConfig::model_ceiling` →
`main.rs::destination_tier_ceiling` → `Destination::with_tier_ceiling` chain
— zero `main.rs` lines changed, proved on the shipped binary
(`a_configured_capability_record_excludes_a_destination_below_the_required_tier_on_the_shipped_binary`).

- **1475** stored as configurable data; unknown fields refused
  (`deny_unknown_fields`), round-trip tested.
- **1476** initial ceiling, resolving when no override exists.
- **1477** structured-output suitability recorded ("record" is the line's
  verb; 34C is its consumer phase).
- **1478** task-kind suitability: `SupportOnly` caps the effective ceiling at
  Leaf, gate-reachable; a record with no stated ceiling stays unknown.
- **1479** override precedence: `model_ceilings` beats the seeded ceiling
  beats nothing. Mutation `invert-1479-precedence` KILLED by
  `line_1479_the_override_beats_the_seeded_ceiling`.
- **1483** pairing class + evidence strength stored alongside ("store" is the
  verb; nothing consumes them yet, and the line does not ask it).
- **1484** benchmark provenance can rank and can never refuse:
  `CeilingResolution::hard_ceiling` returns `None` for `Prior`, the identical
  ceiling with `User` provenance does bind, and `explain()` says "not proof".
- **1485** two provider entries with the same nominal model resolve
  independently — the provider entry IS the backend axis of the key.

**1482 stays OPEN, and the ruling matters.** The record stores the
harness/launch-profile/protocol narrowing and `applies_to` enforces it in
isolation — but the live resolution path reads records by `(provider, model)`
alone. The worker's first submission consumed scoped records on that
context-blind path anyway, which would have let a harness-scoped record cap
every harness; the orchestrator caught it in review and the worker fixed it
the same hour: `is_context_general` makes any record stating a narrowing axis
INERT to context-blind resolution
(`line_1482_a_harness_scoped_record_is_inert_to_context_blind_resolution`,
with an unscoped control). Conservative-inert is safe; it is not the line's
promise. 1482 closes when a caller with harness/profile/protocol context in
hand — `main.rs`, per the broker package's accepted seam: attach the tier to
`Destination` beside `tier_ceiling` — actually applies scoped records
per-context. That successor also unblocks line 1970's tier steps.

Gates on the merged tree (batch 70b integration): blast radius across the
full traced set; the two reds were `shell::settings_persistence_tests`
(documented Gatekeeper flake family, file untouched, green in isolation) and
`entitlement_pool::a_launch_no_entitlement_serves_carries_no_accounts_variable`
— a real cross-patch consequence of batch 70a's line-372 closure, not of this
package: an unpinned launch under default-on automatic routing may now
legitimately land on an account destination. The test's premise (a launch no
entitlement serves) now requires automatic routing off, which is what it
states since 70b; 21/21 twice on the merged tree.

# Lines 1481, 1482 — COMPLETE 2026-09-01; PHASE 34F CLOSED (11/11)

Package `GH-TIER-WIRING` (Sonnet, high, Amber; batch 70e). Integration gate:
86 targets, zero failures, rustdoc clean.

**1482** — the context-carrying caller the batch-70b ruling reserved:
`main.rs::routing_destinations` builds a `CapabilityQuery` (harness, launch
profile, wire protocol) at every `destination_tier_ceiling` call and resolves
through `resolved_ceiling_for`/`model_ceiling_for`, which filter with
`applies_to(query)` instead of the conservative `is_context_general` inert
default. Proven on the shipped binary with an unscoped control: a record
scoped to claude-code caps claude-code and is invisible to codex on the same
provider and model. Mutation `applies-to-ignores-context` KILLED.
`Destination` carries `capability_tier` (the same resolved value, its own
builder — the recorded one-axis reading of the tier-attachment ruling), and
`same_capability_tier` compares two attached values: 1970's same-tier
fallback steps now fire for two models the user assigned one tier
(`a_shared_user_assigned_capability_tier_reaches_the_fallbacks_tier_step`)
and still never on unknown. Limits: profile/protocol axes proven at the
`applies_to` unit level, harness axis on the shipped binary; no Windows leg
owed.

**1481** — `capability_suggestions_section` in `route_report`, gated by
`TierOutcomeVerdict::Measured` itself (the same `MIN_SAMPLE_FOR_SUMMARY`
floor as `outcomes_by_tier` — no second threshold family), comparing
measured per-tier failures against `calibrated_model_ceilings`, rendering
the model, its configured tier and provenance, the counts, and the exact
config key — and writing NOTHING (config byte-compare asserted in the test;
no `&mut` config anywhere in the path). Mutation `suggest-below-evidence-gate`
KILLED. Limits: only capability-record-backed models are compared, and only
the majority-failing direction renders; the promote-a-prior-on-success
suggestion is a separate product judgment, deliberately not invented.
