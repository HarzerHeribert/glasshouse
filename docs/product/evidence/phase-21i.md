# Capability evidence — phase 21I

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 21I — production-aware implementation checks, 11 of 11 (lines 968–978)

Package `GH-IMPLEMENTATION-POLICY`, 2026-08-31, Opus at high. The policy is **Glasshouse-authored text delivered like a briefing**: thirty `Rule { id, line, text }` values in three parts, rendered by one `segments` function so the text a person reads (`glasshouse policy`, `Request::ImplementationPolicy`, the MCP tool `glasshouse_implementation_policy`) and the text a session receives (`deliver_policy`, once per session, `MessageOrigin::Machine`) cannot drift. **All thirty close on the carried-instruction reading**: the rule reaches the agent that can act on it; Glasshouse performs none of these checks and this package invents no analyser — refusing that reading for one line would have implied the other twenty-nine were mechanised. Delivery is six bounded lines, not one: `MAX_CANON` is ~1022 bytes on macOS and `phase-27.md` measured that a longer line is discarded *and wedges the session's input*. The switch is `implementation_policy = true|false`, flat like `memory_extraction`, on the project then user layer. Thirty mutations, thirty killed — after the worker discovered that `scripts/mutate.sh --script` had been manufacturing false KILLEDs for deletion mutations (fixed in `efd6e65`), re-ran with a compiling replacement, found one real SURVIVED, and closed it with a test.


### Require implementation planning to consider whether a solution that works on development data remains acceptable at realistic production scale. (line 968)

Contract: Given an agent Glasshouse briefs -- at spawn with a task, or on its first message -- when the briefing is assembled, Glasshouse delivers rule `p1` of the production-aware part (Phase 21I) as part of a labelled, Glasshouse-authored block, so that the instruction reaches the agent that can act on it, while preserving the marker separation from extracted memory and staying inside the terminal's canonical line limit.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts.

Production evidence:
- `src/policy/mod.rs` — `rules (the Rule this line owns)`
- `src/policy/mod.rs` — `render`
- `src/policy/mod.rs` — `deliveries`
- `src/api/unix.rs` — `deliver_policy`

Regression evidence:
- `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers`
- `implementation_policy::every_rule_names_a_real_map_line_and_the_whole_fits_the_ceiling`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| crates/glasshouse/src/policy/mod.rs :: 'works on development data' -> 'DELETED' | `delete-the-carried-text` | **killed** | `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers` |

> delete-the-carried-text observed: map line 968 (rule `p1`) must reach the agent exactly once, as the phrase "works on development data" -- printed with the whole delivered policy, showing `(p1) ... DELETED ...` on the terminal

Recorded scope limits — stated by the worker, not discovered later:
- Closed on the CARRIED-INSTRUCTION reading: the rule reaches the agent. Glasshouse does not itself perform, evaluate or enforce it, and this package adds no analyser that could.
- Proves the text arrives on the harness's terminal. Does not prove any agent read it, obeyed it, or changed a decision because of it -- that is Phase 51's question and it has no producer today.


---


### Prefer indexed lookup paths for high-cardinality database access when a stable indexed identifier is available. (line 969)

Contract: Given an agent Glasshouse briefs -- at spawn with a task, or on its first message -- when the briefing is assembled, Glasshouse delivers rule `p2` of the production-aware part (Phase 21I) as part of a labelled, Glasshouse-authored block, so that the instruction reaches the agent that can act on it, while preserving the marker separation from extracted memory and staying inside the terminal's canonical line limit.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts.

Production evidence:
- `src/policy/mod.rs` — `rules (the Rule this line owns)`
- `src/policy/mod.rs` — `render`
- `src/policy/mod.rs` — `deliveries`
- `src/api/unix.rs` — `deliver_policy`

Regression evidence:
- `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers`
- `implementation_policy::every_rule_names_a_real_map_line_and_the_whole_fits_the_ceiling`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| crates/glasshouse/src/policy/mod.rs :: 'indexed lookup path for high-cardinality' -> 'DELETED' | `delete-the-carried-text` | **killed** | `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers` |

> delete-the-carried-text observed: map line 969 (rule `p2`) must reach the agent exactly once, as the phrase "indexed lookup path for high-cardinality" -- printed with the whole delivered policy, showing `(p2) ... DELETED ...` on the terminal

Recorded scope limits — stated by the worker, not discovered later:
- Closed on the CARRIED-INSTRUCTION reading: the rule reaches the agent. Glasshouse does not itself perform, evaluate or enforce it, and this package adds no analyser that could.
- Proves the text arrives on the harness's terminal. Does not prove any agent read it, obeyed it, or changed a decision because of it -- that is Phase 51's question and it has no producer today.


---


### Flag unindexed scans on large or expected-to-grow tables when they occur on latency-sensitive request paths. (line 970)

Contract: Given an agent Glasshouse briefs -- at spawn with a task, or on its first message -- when the briefing is assembled, Glasshouse delivers rule `p3` of the production-aware part (Phase 21I) as part of a labelled, Glasshouse-authored block, so that the instruction reaches the agent that can act on it, while preserving the marker separation from extracted memory and staying inside the terminal's canonical line limit.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts.

Production evidence:
- `src/policy/mod.rs` — `rules (the Rule this line owns)`
- `src/policy/mod.rs` — `render`
- `src/policy/mod.rs` — `deliveries`
- `src/api/unix.rs` — `deliver_policy`

Regression evidence:
- `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers`
- `implementation_policy::every_rule_names_a_real_map_line_and_the_whole_fits_the_ceiling`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| crates/glasshouse/src/policy/mod.rs :: 'scans a large or expected-to-grow ' -> 'DELETED' | `delete-the-carried-text` | **killed** | `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers` |

> delete-the-carried-text observed: map line 970 (rule `p3`) must reach the agent exactly once, as the phrase "scans a large or expected-to-grow table without an index" -- printed with the whole delivered policy, showing `(p3) ... DELETED ...` on the terminal

Recorded scope limits — stated by the worker, not discovered later:
- Closed on the CARRIED-INSTRUCTION reading: the rule reaches the agent. Glasshouse does not itself perform, evaluate or enforce it, and this package adds no analyser that could.
- Proves the text arrives on the harness's terminal. Does not prove any agent read it, obeyed it, or changed a decision because of it -- that is Phase 51's question and it has no producer today.


---


### Consider query complexity, index availability, cardinality, and expected access frequency before accepting a database lookup strategy. (line 971)

Contract: Given an agent Glasshouse briefs -- at spawn with a task, or on its first message -- when the briefing is assembled, Glasshouse delivers rule `p4` of the production-aware part (Phase 21I) as part of a labelled, Glasshouse-authored block, so that the instruction reaches the agent that can act on it, while preserving the marker separation from extracted memory and staying inside the terminal's canonical line limit.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts.

Production evidence:
- `src/policy/mod.rs` — `rules (the Rule this line owns)`
- `src/policy/mod.rs` — `render`
- `src/policy/mod.rs` — `deliveries`
- `src/api/unix.rs` — `deliver_policy`

Regression evidence:
- `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers`
- `implementation_policy::every_rule_names_a_real_map_line_and_the_whole_fits_the_ceiling`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| crates/glasshouse/src/policy/mod.rs :: 'index availability, cardinality' -> 'DELETED' | `delete-the-carried-text` | **killed** | `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers` |

> delete-the-carried-text observed: map line 971 (rule `p4`) must reach the agent exactly once, as the phrase "index availability, cardinality" -- printed with the whole delivered policy, showing `(p4) ... DELETED ...` on the terminal

Recorded scope limits — stated by the worker, not discovered later:
- Closed on the CARRIED-INSTRUCTION reading: the rule reaches the agent. Glasshouse does not itself perform, evaluate or enforce it, and this package adds no analyser that could.
- Proves the text arrives on the harness's terminal. Does not prove any agent read it, obeyed it, or changed a decision because of it -- that is Phase 51's question and it has no producer today.


---


### Consider concurrency and race behavior before accepting code that is correct only under single-user development conditions. (line 972)

Contract: Given an agent Glasshouse briefs -- at spawn with a task, or on its first message -- when the briefing is assembled, Glasshouse delivers rule `p5` of the production-aware part (Phase 21I) as part of a labelled, Glasshouse-authored block, so that the instruction reaches the agent that can act on it, while preserving the marker separation from extracted memory and staying inside the terminal's canonical line limit.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts.

Production evidence:
- `src/policy/mod.rs` — `rules (the Rule this line owns)`
- `src/policy/mod.rs` — `render`
- `src/policy/mod.rs` — `deliveries`
- `src/api/unix.rs` — `deliver_policy`

Regression evidence:
- `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers`
- `implementation_policy::every_rule_names_a_real_map_line_and_the_whole_fits_the_ceiling`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| crates/glasshouse/src/policy/mod.rs :: 'concurrency and race behaviour' -> 'DELETED' | `delete-the-carried-text` | **killed** | `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers` |

> delete-the-carried-text observed: map line 972 (rule `p5`) must reach the agent exactly once, as the phrase "concurrency and race behaviour" -- printed with the whole delivered policy, showing `(p5) ... DELETED ...` on the terminal

Recorded scope limits — stated by the worker, not discovered later:
- Closed on the CARRIED-INSTRUCTION reading: the rule reaches the agent. Glasshouse does not itself perform, evaluate or enforce it, and this package adds no analyser that could.
- Proves the text arrives on the harness's terminal. Does not prove any agent read it, obeyed it, or changed a decision because of it -- that is Phase 51's question and it has no producer today.


---


### Consider memory and response-size growth before accepting algorithms whose resource use scales linearly with large datasets. (line 973)

Contract: Given an agent Glasshouse briefs -- at spawn with a task, or on its first message -- when the briefing is assembled, Glasshouse delivers rule `p6` of the production-aware part (Phase 21I) as part of a labelled, Glasshouse-authored block, so that the instruction reaches the agent that can act on it, while preserving the marker separation from extracted memory and staying inside the terminal's canonical line limit.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts.

Production evidence:
- `src/policy/mod.rs` — `rules (the Rule this line owns)`
- `src/policy/mod.rs` — `render`
- `src/policy/mod.rs` — `deliveries`
- `src/api/unix.rs` — `deliver_policy`

Regression evidence:
- `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers`
- `implementation_policy::every_rule_names_a_real_map_line_and_the_whole_fits_the_ceiling`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| crates/glasshouse/src/policy/mod.rs :: 'memory and response-size growth' -> 'DELETED' | `delete-the-carried-text` | **killed** | `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers` |

> delete-the-carried-text observed: map line 973 (rule `p6`) must reach the agent exactly once, as the phrase "memory and response-size growth" -- printed with the whole delivered policy, showing `(p6) ... DELETED ...` on the terminal

Recorded scope limits — stated by the worker, not discovered later:
- Closed on the CARRIED-INSTRUCTION reading: the rule reaches the agent. Glasshouse does not itself perform, evaluate or enforce it, and this package adds no analyser that could.
- Proves the text arrives on the harness's terminal. Does not prove any agent read it, obeyed it, or changed a decision because of it -- that is Phase 51's question and it has no producer today.


---


### Consider network round trips before accepting repeated remote calls in hot request paths. (line 974)

Contract: Given an agent Glasshouse briefs -- at spawn with a task, or on its first message -- when the briefing is assembled, Glasshouse delivers rule `p7` of the production-aware part (Phase 21I) as part of a labelled, Glasshouse-authored block, so that the instruction reaches the agent that can act on it, while preserving the marker separation from extracted memory and staying inside the terminal's canonical line limit.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts.

Production evidence:
- `src/policy/mod.rs` — `rules (the Rule this line owns)`
- `src/policy/mod.rs` — `render`
- `src/policy/mod.rs` — `deliveries`
- `src/api/unix.rs` — `deliver_policy`

Regression evidence:
- `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers`
- `implementation_policy::every_rule_names_a_real_map_line_and_the_whole_fits_the_ceiling`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| crates/glasshouse/src/policy/mod.rs :: 'network round trips' -> 'DELETED' | `delete-the-carried-text` | **killed** | `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers` |

> delete-the-carried-text observed: map line 974 (rule `p7`) must reach the agent exactly once, as the phrase "network round trips" -- printed with the whole delivered policy, showing `(p7) ... DELETED ...` on the terminal

Recorded scope limits — stated by the worker, not discovered later:
- Closed on the CARRIED-INSTRUCTION reading: the rule reaches the agent. Glasshouse does not itself perform, evaluate or enforce it, and this package adds no analyser that could.
- Proves the text arrives on the harness's terminal. Does not prove any agent read it, obeyed it, or changed a decision because of it -- that is Phase 51's question and it has no producer today.


---


### Consider authentication and authorization lookup cost at realistic user counts. (line 975)

Contract: Given an agent Glasshouse briefs -- at spawn with a task, or on its first message -- when the briefing is assembled, Glasshouse delivers rule `p8` of the production-aware part (Phase 21I) as part of a labelled, Glasshouse-authored block, so that the instruction reaches the agent that can act on it, while preserving the marker separation from extracted memory and staying inside the terminal's canonical line limit.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts.

Production evidence:
- `src/policy/mod.rs` — `rules (the Rule this line owns)`
- `src/policy/mod.rs` — `render`
- `src/policy/mod.rs` — `deliveries`
- `src/api/unix.rs` — `deliver_policy`

Regression evidence:
- `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers`
- `implementation_policy::every_rule_names_a_real_map_line_and_the_whole_fits_the_ceiling`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| crates/glasshouse/src/policy/mod.rs :: 'authentication and authorization lookup cost' -> 'DELETED' | `delete-the-carried-text` | **killed** | `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers` |

> delete-the-carried-text observed: map line 975 (rule `p8`) must reach the agent exactly once, as the phrase "authentication and authorization lookup cost" -- printed with the whole delivered policy, showing `(p8) ... DELETED ...` on the terminal

Recorded scope limits — stated by the worker, not discovered later:
- Closed on the CARRIED-INSTRUCTION reading: the rule reaches the agent. Glasshouse does not itself perform, evaluate or enforce it, and this package adds no analyser that could.
- Proves the text arrives on the harness's terminal. Does not prove any agent read it, obeyed it, or changed a decision because of it -- that is Phase 51's question and it has no producer today.


---


### Prefer stable indexed IDs over high-cost ad hoc lookups when the product already has an appropriate identifier. (line 976)

Contract: Given an agent Glasshouse briefs -- at spawn with a task, or on its first message -- when the briefing is assembled, Glasshouse delivers rule `p9` of the production-aware part (Phase 21I) as part of a labelled, Glasshouse-authored block, so that the instruction reaches the agent that can act on it, while preserving the marker separation from extracted memory and staying inside the terminal's canonical line limit.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts.

Production evidence:
- `src/policy/mod.rs` — `rules (the Rule this line owns)`
- `src/policy/mod.rs` — `render`
- `src/policy/mod.rs` — `deliveries`
- `src/api/unix.rs` — `deliver_policy`

Regression evidence:
- `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers`
- `implementation_policy::every_rule_names_a_real_map_line_and_the_whole_fits_the_ceiling`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| crates/glasshouse/src/policy/mod.rs :: 'high-cost ad hoc lookup' -> 'DELETED' | `delete-the-carried-text` | **killed** | `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers` |

> delete-the-carried-text observed: map line 976 (rule `p9`) must reach the agent exactly once, as the phrase "high-cost ad hoc lookup" -- printed with the whole delivered policy, showing `(p9) ... DELETED ...` on the terminal

Recorded scope limits — stated by the worker, not discovered later:
- Closed on the CARRIED-INSTRUCTION reading: the rule reaches the agent. Glasshouse does not itself perform, evaluate or enforce it, and this package adds no analyser that could.
- Proves the text arrives on the harness's terminal. Does not prove any agent read it, obeyed it, or changed a decision because of it -- that is Phase 51's question and it has no producer today.


---


### Do not optimize prematurely where scale is demonstrably irrelevant, but record the assumption if the implementation depends on that fact. (line 977)

Contract: Given an agent Glasshouse briefs -- at spawn with a task, or on its first message -- when the briefing is assembled, Glasshouse delivers rule `p10` of the production-aware part (Phase 21I) as part of a labelled, Glasshouse-authored block, so that the instruction reaches the agent that can act on it, while preserving the marker separation from extracted memory and staying inside the terminal's canonical line limit.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts.

Production evidence:
- `src/policy/mod.rs` — `rules (the Rule this line owns)`
- `src/policy/mod.rs` — `render`
- `src/policy/mod.rs` — `deliveries`
- `src/api/unix.rs` — `deliver_policy`

Regression evidence:
- `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers`
- `implementation_policy::every_rule_names_a_real_map_line_and_the_whole_fits_the_ceiling`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| crates/glasshouse/src/policy/mod.rs :: 'scale is demonstrably irrelevant' -> 'DELETED' | `delete-the-carried-text` | **killed** | `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers` |

> delete-the-carried-text observed: map line 977 (rule `p10`) must reach the agent exactly once, as the phrase "scale is demonstrably irrelevant" -- printed with the whole delivered policy, showing `(p10) ... DELETED ...` on the terminal

Recorded scope limits — stated by the worker, not discovered later:
- Closed on the CARRIED-INSTRUCTION reading: the rule reaches the agent. Glasshouse does not itself perform, evaluate or enforce it, and this package adds no analyser that could.
- Proves the text arrives on the harness's terminal. Does not prove any agent read it, obeyed it, or changed a decision because of it -- that is Phase 51's question and it has no producer today.


---


### Allow production incidents to promote previously hypothetical scale concerns into validated constraints. (line 978)

Contract: Given an agent Glasshouse briefs -- at spawn with a task, or on its first message -- when the briefing is assembled, Glasshouse delivers rule `p11` of the production-aware part (Phase 21I) as part of a labelled, Glasshouse-authored block, so that the instruction reaches the agent that can act on it, while preserving the marker separation from extracted memory and staying inside the terminal's canonical line limit.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts.

Production evidence:
- `src/policy/mod.rs` — `rules (the Rule this line owns)`
- `src/policy/mod.rs` — `render`
- `src/policy/mod.rs` — `deliveries`
- `src/api/unix.rs` — `deliver_policy`

Regression evidence:
- `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers`
- `implementation_policy::every_rule_names_a_real_map_line_and_the_whole_fits_the_ceiling`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| crates/glasshouse/src/policy/mod.rs :: 'a production incident promotes' -> 'DELETED' | `delete-the-carried-text` | **killed** | `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers` |

> delete-the-carried-text observed: map line 978 (rule `p11`) must reach the agent exactly once, as the phrase "a production incident promotes" -- printed with the whole delivered policy, showing `(p11) ... DELETED ...` on the terminal

Recorded scope limits — stated by the worker, not discovered later:
- Closed on the CARRIED-INSTRUCTION reading: the rule reaches the agent. Glasshouse does not itself perform, evaluate or enforce it, and this package adds no analyser that could.
- Proves the text arrives on the harness's terminal. Does not prove any agent read it, obeyed it, or changed a decision because of it -- that is Phase 51's question and it has no producer today.

