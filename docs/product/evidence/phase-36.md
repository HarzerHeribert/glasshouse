# Capability evidence — phase 36

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 36 — session affinity, 8 of 8 (lines 1581–1588)

Package `GH-AFFINITY`, 2026-08-31, Fable 5 at xhigh. `session_affinity` already existed as one contribution with one reason; these eight lines are its facets. It stays **one number** — `AffinityBreakdown::total` — so the ranking, the overview and every existing assertion on the term keep reading what they read, and the evidence is the breakdown's `Display`: seven named facets (same task, touched files, native context still useful, prompt cache likely hot, noisy or unrelated, quota pressure, and the fresh-session baseline), each with its own sentence. A launch that stated no task leaves the task facets *unknown* rather than inventing a task to compare against. Constants are named with their reasons (`PROMPT_CACHE_TTL_SECONDS = 5 * 60`, `NOISY_COMPACTION_COUNT = 3`, …). Sixteen mutations, sixteen killed. The report's `packet_errors` corrected four of the packet's own anchors (§81): `CacheLocality` lives in `routing/mod.rs`; `memory_files` has a writer and no reader yet; the sticky cache holds a classification, not task text; the adapter's `--session-id` is the harness-native UUID — each handled in the code rather than papered over.

### Compute a session-affinity score for candidate existing sessions. (line 1581)

Contract: Given an existing session offered as a destination, when the session router scores it, Glasshouse computes its affinity as an AffinityBreakdown of seven named facets whose sum is the `session affinity` contribution, while preserving the single-contribution shape every existing assertion on that term reads and the fresh destination's unchanged zero.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts and the diff of the decision.

Production evidence:
- `src/routing/session.rs` — `AffinityBreakdown`
- `src/routing/session.rs` — `AffinityBreakdown::total`
- `src/routing/session.rs` — `session_affinity`
- `src/routing/session.rs` — `affinity_breakdown`
- `src/routing/session.rs` — `score`

Regression evidence:
- `session_affinity::the_affinity_contribution_is_the_breakdown_and_its_explanation_names_every_facet`
- `session_affinity::a_fresh_destination_has_no_breakdown_and_is_not_penalised`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| Contribution evidence `breakdown.to_string()` -> `breakdown.warmth.evidence().to_owned()` | `collapse-the-explanation` | **killed** | `session_affinity::the_affinity_contribution_is_the_breakdown_and_its_explanation_names_every_facet` |

> collapse-the-explanation observed: assertion `left == right` failed (term.evidence() == breakdown.to_string()); the binary test also failed: `native context (line 1584)` absent from the route report

Recorded scope limits — stated by the worker, not discovered later:
- The breakdown is one Contribution's magnitude and evidence; RoutingExplanation itself is unchanged, so a consumer wanting a facet programmatically calls affinity_breakdown rather than reading the contribution.


---


### Increase affinity when the session is already working on the same task or feature. (line 1582)

Contract: Given the sticky classification cache names this session and the task in hand is classed the same way (hard capabilities, workload tier, repo-context and code-modification flags), when the router scores the session, Glasshouse adds SAME_TASK_AFFINITY (0.5) and says so, while preserving an unknown, weightless facet whenever no task was stated or the cache names another session.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts and the diff of the decision.

Production evidence:
- `src/routing/session.rs` — `affinity_breakdown (same task arm)`
- `src/routing/session.rs` — `same_work`
- `src/routing/session.rs` — `SessionContextFacts::with_last_task`
- `src/routing/request.rs` — `StickyClassification::classification`
- `src/main.rs` — `routing_destinations (sticky load + with_last_task)`

Regression evidence:
- `session_affinity::the_session_the_last_matching_classification_was_made_for_wins_on_same_task`
- `session_affinity::same_task_is_unknown_without_a_stated_task_or_a_recorded_one`
- `session_affinity::the_launch_paths_router_reads_the_sticky_caches_last_task_for_the_session`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| const SAME_TASK_AFFINITY: f64 = 0.5; -> 0.0 | `zero-the-weight` | **killed** | `session_affinity::the_session_the_last_matching_classification_was_made_for_wins_on_same_task` |
| same_work body -> `true || previous.hard_capabilities() == ...` | `same-work-always-true` | **killed** | `session_affinity::an_unrelated_task_or_unrelated_files_cost_a_session_against_an_unknown_one` |
| main.rs `.and_then(|sticky| sticky.classification())` -> `.and_then(|_| None)` | `sever-the-caller` | **killed** | `session_affinity::the_launch_paths_router_reads_the_sticky_caches_last_task_for_the_session` |

> zero-the-weight observed: assertion `left == right` failed: the caller's order must not be what decides this pair

> same-work-always-true observed: a session last seen doing something classed differently must lose to one nothing is known about

> sever-the-caller observed: panicked at session_affinity.rs:932 — the report read `no classified task is recorded` for a session the planted record names

Recorded scope limits — stated by the worker, not discovered later:
- Same task means same classification: the sticky cache stores a classification and never task text, so two different tasks classed identically read as the same task.
- The binary test plants the sticky record (StickyClassification::to_json, where ClassificationStickyCache::new reads); the binary writes it only after a routing model answered, and the fixture has none.


---


### Increase affinity when the session has recently touched relevant files. (line 1583)

Contract: Given the task text names paths and the session's own latest checkpoint lists files (handoff files and working-tree changed files), when the router scores the session, Glasshouse adds TOUCHED_FILES_AFFINITY (0.6) scaled by the fraction of named paths the checkpoint lists, while preserving an unknown, weightless facet when no task was stated, the task names no path, or the session has no checkpoint.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts and the diff of the decision.

Production evidence:
- `src/routing/session.rs` — `affinity_breakdown (touched files arm)`
- `src/routing/session.rs` — `paths_named_in`
- `src/routing/session.rs` — `path_names`
- `src/main.rs` — `session_touched_files`
- `src/main.rs` — `routing_destinations (with_touched_files, with_task_named_paths)`

Regression evidence:
- `session_affinity::the_session_whose_checkpoint_lists_the_file_the_task_names_wins`
- `session_affinity::touched_files_is_unknown_when_either_operand_is_missing`
- `session_affinity::paths_named_in_reads_paths_and_not_prose`
- `session_affinity::the_launch_paths_router_reads_the_files_the_sessions_own_checkpoint_lists`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| const TOUCHED_FILES_AFFINITY: f64 = 0.6; -> 0.0 | `zero-the-weight` | **killed** | `session_affinity::the_session_whose_checkpoint_lists_the_file_the_task_names_wins` |
| main.rs `.with_touched_files(session_touched_files(checkpoints.as_ref(), &record.id))` -> `.with_touched_files(None)` | `sever-the-caller` | **killed** | `session_affinity::the_launch_paths_router_reads_the_files_the_sessions_own_checkpoint_lists` |
| main.rs `.with_task_named_paths(task_named_paths.clone())` -> `(None)` | `sever-the-task-side` | **killed** | `session_affinity::the_launch_paths_router_reads_the_compaction_count_and_the_tasks_named_paths` |

> zero-the-weight observed: first run SURVIVED (the pair was separated by 1586's penalty on the other session); pair rebuilt with the other session's files unknown; re-run panicked at session_affinity.rs:256 on the mirrored half

> sever-the-caller observed: panicked at session_affinity.rs:899 — `lists 1 of the 1 path` absent after `glasshouse checkpoint save --file src/routing/session.rs::choose`

> sever-the-task-side observed: panicked at session_affinity.rs:863 — the facet read `no task was stated` on a `route --task` that named a path

Recorded scope limits — stated by the worker, not discovered later:
- memory_files (migration 17) is NOT read: this build writes it and reads it nowhere (database.rs:231). The facet accepts any path list; a reader joining memories.source_session_id is the follow-up.
- paths_named_in is a spelling heuristic; `Node.js` in prose reads as a path (documented), which is why the 1586 penalty needs a `/`-path.


---


### Increase affinity when the native context is still semantically useful. (line 1584)

Contract: Given a session whose compaction count was recorded and which is inside the warm-session relevance window, when the router scores it, Glasshouse adds NATIVE_CONTEXT_INTACT (0.3) at zero compactions and half at one or two, while preserving a weightless, unknown facet for a row nobody counted (None) and a zero for a session past the window.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts and the diff of the decision.

Production evidence:
- `src/routing/session.rs` — `affinity_breakdown (native context arm)`
- `src/routing/session.rs` — `SessionContextFacts::with_observed_compactions`
- `src/main.rs` — `routing_destinations (record.observed_compactions)`

Regression evidence:
- `session_affinity::a_counted_clean_history_outscores_an_uncounted_one_and_says_why`
- `session_affinity::a_never_compacted_session_beats_a_repeatedly_compacted_one`
- `session_affinity::the_launch_paths_router_reads_the_compaction_count_and_the_tasks_named_paths`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| const NATIVE_CONTEXT_INTACT: f64 = 0.3; -> 0.0 | `zero-the-weight` | **killed** | `session_affinity::a_counted_clean_history_outscores_an_uncounted_one_and_says_why` |
| main.rs `.with_session_context(context)` -> `.with_session_context(SessionContextFacts::UNREAD)` | `sever-the-caller` | **killed** | `session_affinity::the_launch_paths_router_reads_the_compaction_count_and_the_tasks_named_paths` |

> zero-the-weight observed: assertion failed: counted.native_context.magnitude() > 0.0

> sever-the-caller observed: panicked at session_affinity.rs:788 — the report read `nobody counted` for a session this build created with Some(0)

Recorded scope limits — stated by the worker, not discovered later:
- Semantically useful is observed as: not compacted and not stale. No harness exposes context size (SessionContext's own doc), so nothing finer is claimed.


---


### Increase affinity when the prompt cache is likely hot. (line 1585)

Contract: Given an existing session active within PROMPT_CACHE_TTL_SECONDS (300s, source cited) and, at a task boundary, on the backend the work is already on, when the router scores it, Glasshouse adds PROMPT_CACHE_HOT (0.4) and says it is likely rather than observed, while preserving zero past the lifetime or on a backend change and unknown on a clock that moved backwards.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts and the diff of the decision.

Production evidence:
- `src/routing/session.rs` — `affinity_breakdown (prompt cache arm)`
- `src/routing/session.rs` — `PROMPT_CACHE_TTL_SECONDS`
- `src/routing/mod.rs` — `CacheLocality::between`

Regression evidence:
- `session_affinity::the_prompt_cache_facet_steps_at_the_published_lifetime_and_off_the_backend`
- `session_affinity::the_prompt_cache_step_is_what_separates_two_sessions_either_side_of_it`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| const PROMPT_CACHE_HOT: f64 = 0.4; -> 0.0 | `zero-the-weight` | **killed** | `session_affinity::the_prompt_cache_facet_steps_at_the_published_lifetime_and_off_the_backend` |
| const PROMPT_CACHE_TTL_SECONDS: i64 = 5 * 60; -> 5 * 60 * 60 | `stretch-the-lifetime` | **killed** | `session_affinity::the_prompt_cache_facet_steps_at_the_published_lifetime_and_off_the_backend` |

> zero-the-weight observed: panicked at session_affinity.rs:459 (inside.magnitude() > 0.0)

> stretch-the-lifetime observed: panicked at session_affinity.rs:464 (past.magnitude() == 0.0 — 301s now read as hot)

Recorded scope limits — stated by the worker, not discovered later:
- Reasoned from a published default lifetime, not observed: no provider reports a cache hit.


---


### Decrease affinity when the session context has become noisy or unrelated. (line 1586)

Contract: Given a session compacted three or more times, or whose last classified task differs from this one, or whose checkpoint lists files while the task names `/`-paths none of which it lists, when the router scores it, Glasshouse subtracts the named penalties (-0.2/compaction floored at -0.6; -0.3; -0.3) and lists each reason, while preserving an unknown facet when none of the three signals is readable.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts and the diff of the decision.

Production evidence:
- `src/routing/session.rs` — `affinity_breakdown (noise arm)`

Regression evidence:
- `session_affinity::a_never_compacted_session_beats_a_repeatedly_compacted_one`
- `session_affinity::an_unrelated_task_or_unrelated_files_cost_a_session_against_an_unknown_one`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| const COMPACTION_NOISE_PENALTY: f64 = -0.2; -> 0.0 | `zero-the-compaction-penalty` | **killed** | `session_affinity::a_never_compacted_session_beats_a_repeatedly_compacted_one` |
| const UNRELATED_TASK_PENALTY: f64 = -0.3; -> 0.0 | `zero-the-unrelated-task-penalty` | **killed** | `session_affinity::an_unrelated_task_or_unrelated_files_cost_a_session_against_an_unknown_one` |
| const UNRELATED_FILES_PENALTY: f64 = -0.3; -> 0.0 | `zero-the-unrelated-files-penalty` | **killed** | `session_affinity::an_unrelated_task_or_unrelated_files_cost_a_session_against_an_unknown_one` |

> zero-the-compaction-penalty observed: panicked at session_affinity.rs:349 — the compacted session trails only by the intact one's credit: line 1586's penalty is not being applied

> zero-the-unrelated-task-penalty observed: a session last seen doing something classed differently must lose to one nothing is known about

> zero-the-unrelated-files-penalty observed: a session whose checkpoint lists files the task does not name must lose to one nothing is known about

Recorded scope limits — stated by the worker, not discovered later:
- Unrelated-files requires a `/`-path in the task text, so a bare file name in prose can earn 1583's credit but never this penalty.


---


### Decrease affinity when the session’s quota resource is under significant pressure. (line 1587)

Contract: Given the capacity band the caller derived from the same reading quota_pressure prices is Reserve or below, when the router scores the session, Glasshouse subtracts QUOTA_PRESSURE_AFFINITY_PENALTY (0.4) and says the reading itself is priced once elsewhere, while preserving zero at Tight or better and unknown when nothing was read.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts and the diff of the decision.

Production evidence:
- `src/routing/session.rs` — `affinity_breakdown (quota pressure arm)`
- `src/routing/session.rs` — `Destination::capacity_facts`
- `src/main.rs` — `destination_capacity`

Regression evidence:
- `session_affinity::the_reserve_band_costs_a_session_affinity_and_the_healthy_band_does_not`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| const QUOTA_PRESSURE_AFFINITY_PENALTY: f64 = -0.4; -> 0.0 | `zero-the-weight` | **killed** | `session_affinity::the_reserve_band_costs_a_session_affinity_and_the_healthy_band_does_not` |

> zero-the-weight observed: panicked at session_affinity.rs:547 (reserve.magnitude() < 0.0)

Recorded scope limits — stated by the worker, not discovered later:
- A deliberate second term on one reading (decision 4): 1598 prices the reading linearly outside the affinity score and 1587 puts a threshold decrease inside it. If the ruling prefers no double-pricing, it is one constant to zero and the line then has no term.
- Proven at the library; through the binary only via the same capacity_facts producer 1598 already proved (route_command::known_quota_pressure_decides_...).


---


### Keep the affinity calculation inspectable so the user can understand why a session was selected. (line 1588)

Contract: Given any affinity score, when `glasshouse route` or a launch renders the explanation, Glasshouse prints the AffinityBreakdown's Display — a summary line and one line per facet with signed magnitude, name, map line, `unknown` where the signal did not arrive, and its evidence sentence — while preserving the one-line-per-contribution rendering of every other term.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts and the diff of the decision.

Production evidence:
- `src/routing/session.rs` — `impl Display for AffinityBreakdown`
- `src/routing/session.rs` — `AffinityFacet::is_known`
- `src/main.rs` — `render_route_recommendation (unchanged; renders the term)`

Regression evidence:
- `session_affinity::the_affinity_contribution_is_the_breakdown_and_its_explanation_names_every_facet`
- `session_affinity::the_launch_paths_router_reads_the_compaction_count_and_the_tasks_named_paths`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| Contribution evidence `breakdown.to_string()` -> `breakdown.warmth.evidence().to_owned()` | `collapse-the-explanation` | **killed** | `session_affinity::the_affinity_contribution_is_the_breakdown_and_its_explanation_names_every_facet` |

> collapse-the-explanation observed: assertion `left == right` failed; the binary test also failed on `native context (line 1584)` absent

Recorded scope limits — stated by the worker, not discovered later:
- Inspectable means rendered in the explanation a person reads; no new CLI surface was added.

