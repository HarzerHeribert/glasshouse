# Capability evidence — phase 21J

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 21J — implementation review checklist, 9 of 9 (lines 982–990)

Package `GH-IMPLEMENTATION-POLICY`, 2026-08-31, Opus at high. The policy is **Glasshouse-authored text delivered like a briefing**: thirty `Rule { id, line, text }` values in three parts, rendered by one `segments` function so the text a person reads (`glasshouse policy`, `Request::ImplementationPolicy`, the MCP tool `glasshouse_implementation_policy`) and the text a session receives (`deliver_policy`, once per session, `MessageOrigin::Machine`) cannot drift. **All thirty close on the carried-instruction reading**: the rule reaches the agent that can act on it; Glasshouse performs none of these checks and this package invents no analyser — refusing that reading for one line would have implied the other twenty-nine were mechanised. Delivery is six bounded lines, not one: `MAX_CANON` is ~1022 bytes on macOS and `phase-27.md` measured that a longer line is discarded *and wedges the session's input*. The switch is `implementation_policy = true|false`, flat like `memory_extraction`, on the project then user layer. Thirty mutations, thirty killed — after the worker discovered that `scripts/mutate.sh --script` had been manufacturing false KILLEDs for deletion mutations (fixed in `efd6e65`), re-ran with a compiling replacement, found one real SURVIVED, and closed it with a test.


### Before marking a substantial implementation complete, check whether any remembered rule forced avoidable complexity. (line 982)

Contract: Given an agent Glasshouse briefs -- at spawn with a task, or on its first message -- when the briefing is assembled, Glasshouse delivers rule `r1` of the pre-completion review checklist (Phase 21J) as part of a labelled, Glasshouse-authored block, so that the instruction reaches the agent that can act on it, while preserving the marker separation from extracted memory and staying inside the terminal's canonical line limit.

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
| crates/glasshouse/src/policy/mod.rs :: 'remembered rule forced avoidable complexity' -> 'DELETED' | `delete-the-carried-text` | **killed** | `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers` |

> delete-the-carried-text observed: map line 982 (rule `r1`) must reach the agent exactly once, as the phrase "remembered rule forced avoidable complexity" -- printed with the whole delivered policy, showing `(r1) ... DELETED ...` on the terminal

Recorded scope limits — stated by the worker, not discovered later:
- Closed on the CARRIED-INSTRUCTION reading: the rule reaches the agent. Glasshouse does not itself perform, evaluate or enforce it, and this package adds no analyser that could.
- Proves the text arrives on the harness's terminal. Does not prove any agent read it, obeyed it, or changed a decision because of it -- that is Phase 51's question and it has no producer today.


---


### Before marking a substantial implementation complete, check whether the design still matches current project requirements rather than historical ones. (line 983)

Contract: Given an agent Glasshouse briefs -- at spawn with a task, or on its first message -- when the briefing is assembled, Glasshouse delivers rule `r2` of the pre-completion review checklist (Phase 21J) as part of a labelled, Glasshouse-authored block, so that the instruction reaches the agent that can act on it, while preserving the marker separation from extracted memory and staying inside the terminal's canonical line limit.

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
| crates/glasshouse/src/policy/mod.rs :: ' historical ones' -> 'DELETED' | `delete-the-carried-text` | **killed** | `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers` |

> delete-the-carried-text observed: map line 983 (rule `r2`) must reach the agent exactly once, as the phrase "rather than historical ones" -- printed with the whole delivered policy, showing `(r2) ... DELETED ...` on the terminal

Recorded scope limits — stated by the worker, not discovered later:
- Closed on the CARRIED-INSTRUCTION reading: the rule reaches the agent. Glasshouse does not itself perform, evaluate or enforce it, and this package adds no analyser that could.
- Proves the text arrives on the harness's terminal. Does not prove any agent read it, obeyed it, or changed a decision because of it -- that is Phase 51's question and it has no producer today.


---


### Before marking a substantial implementation complete, check correctness under realistic concurrency assumptions. (line 984)

Contract: Given an agent Glasshouse briefs -- at spawn with a task, or on its first message -- when the briefing is assembled, Glasshouse delivers rule `r3` of the pre-completion review checklist (Phase 21J) as part of a labelled, Glasshouse-authored block, so that the instruction reaches the agent that can act on it, while preserving the marker separation from extracted memory and staying inside the terminal's canonical line limit.

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
| crates/glasshouse/src/policy/mod.rs :: 'realistic concurrency assumptions' -> 'DELETED' | `delete-the-carried-text` | **killed** | `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers` |

> delete-the-carried-text observed: map line 984 (rule `r3`) must reach the agent exactly once, as the phrase "realistic concurrency assumptions" -- printed with the whole delivered policy, showing `(r3) ... DELETED ...` on the terminal

Recorded scope limits — stated by the worker, not discovered later:
- Closed on the CARRIED-INSTRUCTION reading: the rule reaches the agent. Glasshouse does not itself perform, evaluate or enforce it, and this package adds no analyser that could.
- Proves the text arrives on the harness's terminal. Does not prove any agent read it, obeyed it, or changed a decision because of it -- that is Phase 51's question and it has no producer today.


---


### Before marking a substantial implementation complete, check security boundaries affected by the change. (line 985)

Contract: Given an agent Glasshouse briefs -- at spawn with a task, or on its first message -- when the briefing is assembled, Glasshouse delivers rule `r4` of the pre-completion review checklist (Phase 21J) as part of a labelled, Glasshouse-authored block, so that the instruction reaches the agent that can act on it, while preserving the marker separation from extracted memory and staying inside the terminal's canonical line limit.

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
| crates/glasshouse/src/policy/mod.rs :: 'security boundaries this change affects' -> 'DELETED' | `delete-the-carried-text` | **killed** | `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers` |

> delete-the-carried-text observed: map line 985 (rule `r4`) must reach the agent exactly once, as the phrase "security boundaries this change affects" -- printed with the whole delivered policy, showing `(r4) ... DELETED ...` on the terminal

Recorded scope limits — stated by the worker, not discovered later:
- Closed on the CARRIED-INSTRUCTION reading: the rule reaches the agent. Glasshouse does not itself perform, evaluate or enforce it, and this package adds no analyser that could.
- Proves the text arrives on the harness's terminal. Does not prove any agent read it, obeyed it, or changed a decision because of it -- that is Phase 51's question and it has no producer today.


---


### Before marking a substantial implementation complete, check obvious database and algorithmic scaling characteristics. (line 986)

Contract: Given an agent Glasshouse briefs -- at spawn with a task, or on its first message -- when the briefing is assembled, Glasshouse delivers rule `r5` of the pre-completion review checklist (Phase 21J) as part of a labelled, Glasshouse-authored block, so that the instruction reaches the agent that can act on it, while preserving the marker separation from extracted memory and staying inside the terminal's canonical line limit.

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
| crates/glasshouse/src/policy/mod.rs :: 'algorithmic scaling characteristics' -> 'DELETED' | `delete-the-carried-text` | **killed** | `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers` |

> delete-the-carried-text observed: map line 986 (rule `r5`) must reach the agent exactly once, as the phrase "algorithmic scaling characteristics" -- printed with the whole delivered policy, showing `(r5) ... DELETED ...` on the terminal

Recorded scope limits — stated by the worker, not discovered later:
- Closed on the CARRIED-INSTRUCTION reading: the rule reaches the agent. Glasshouse does not itself perform, evaluate or enforce it, and this package adds no analyser that could.
- Proves the text arrives on the harness's terminal. Does not prove any agent read it, obeyed it, or changed a decision because of it -- that is Phase 51's question and it has no producer today.


---


### Before marking a substantial implementation complete, check whether hot-path database queries use appropriate indexes. (line 987)

Contract: Given an agent Glasshouse briefs -- at spawn with a task, or on its first message -- when the briefing is assembled, Glasshouse delivers rule `r6` of the pre-completion review checklist (Phase 21J) as part of a labelled, Glasshouse-authored block, so that the instruction reaches the agent that can act on it, while preserving the marker separation from extracted memory and staying inside the terminal's canonical line limit.

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
| crates/glasshouse/src/policy/mod.rs :: 'hot-path database queries use appropriate indexes' -> 'DELETED' | `delete-the-carried-text` | **killed** | `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers` |

> delete-the-carried-text observed: map line 987 (rule `r6`) must reach the agent exactly once, as the phrase "hot-path database queries use appropriate indexes" -- printed with the whole delivered policy, showing `(r6) ... DELETED ...` on the terminal

Recorded scope limits — stated by the worker, not discovered later:
- Closed on the CARRIED-INSTRUCTION reading: the rule reaches the agent. Glasshouse does not itself perform, evaluate or enforce it, and this package adds no analyser that could.
- Proves the text arrives on the harness's terminal. Does not prove any agent read it, obeyed it, or changed a decision because of it -- that is Phase 51's question and it has no producer today.


---


### Before marking a substantial implementation complete, check whether a simpler implementation would satisfy the same requirements with less code or fewer moving parts. (line 988)

Contract: Given an agent Glasshouse briefs -- at spawn with a task, or on its first message -- when the briefing is assembled, Glasshouse delivers rule `r7` of the pre-completion review checklist (Phase 21J) as part of a labelled, Glasshouse-authored block, so that the instruction reaches the agent that can act on it, while preserving the marker separation from extracted memory and staying inside the terminal's canonical line limit.

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
| crates/glasshouse/src/policy/mod.rs :: ' code or fewer moving parts' -> 'DELETED' | `delete-the-carried-text` | **killed** | `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers` |

> delete-the-carried-text observed: map line 988 (rule `r7`) must reach the agent exactly once, as the phrase "less code or fewer moving parts" -- printed with the whole delivered policy, showing `(r7) ... DELETED ...` on the terminal

Recorded scope limits — stated by the worker, not discovered later:
- Closed on the CARRIED-INSTRUCTION reading: the rule reaches the agent. Glasshouse does not itself perform, evaluate or enforce it, and this package adds no analyser that could.
- Proves the text arrives on the harness's terminal. Does not prove any agent read it, obeyed it, or changed a decision because of it -- that is Phase 51's question and it has no producer today.


---


### Before marking a substantial implementation complete, check whether a clever optimization introduces complexity disproportionate to its demonstrated benefit. (line 989)

Contract: Given an agent Glasshouse briefs -- at spawn with a task, or on its first message -- when the briefing is assembled, Glasshouse delivers rule `r8` of the pre-completion review checklist (Phase 21J) as part of a labelled, Glasshouse-authored block, so that the instruction reaches the agent that can act on it, while preserving the marker separation from extracted memory and staying inside the terminal's canonical line limit.

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
| crates/glasshouse/src/policy/mod.rs :: 'disproportionate to its ' -> 'DELETED' | `delete-the-carried-text` | **killed** | `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers` |

> delete-the-carried-text observed: map line 989 (rule `r8`) must reach the agent exactly once, as the phrase "disproportionate to its demonstrated benefit" -- printed with the whole delivered policy, showing `(r8) ... DELETED ...` on the terminal

Recorded scope limits — stated by the worker, not discovered later:
- Closed on the CARRIED-INSTRUCTION reading: the rule reaches the agent. Glasshouse does not itself perform, evaluate or enforce it, and this package adds no analyser that could.
- Proves the text arrives on the harness's terminal. Does not prove any agent read it, obeyed it, or changed a decision because of it -- that is Phase 51's question and it has no producer today.


---


### Record material architecture or performance decisions discovered during this review as current memories with rationale and scope. (line 990)

Contract: Given an agent Glasshouse briefs -- at spawn with a task, or on its first message -- when the briefing is assembled, Glasshouse delivers rule `r9` of the pre-completion review checklist (Phase 21J) as part of a labelled, Glasshouse-authored block, so that the instruction reaches the agent that can act on it, while preserving the marker separation from extracted memory and staying inside the terminal's canonical line limit.

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
| crates/glasshouse/src/policy/mod.rs :: 'glasshouse memory extract --session <id> --from-events' -> 'DELETED' | `delete-the-carried-text` | **killed** | `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers` |

> delete-the-carried-text observed: map line 990 (rule `r9`) must reach the agent exactly once, as the phrase "glasshouse memory extract --session <id> --from-events" -- printed with the whole delivered policy, showing `(r9) ... DELETED ...` on the terminal

Recorded scope limits — stated by the worker, not discovered later:
- Closed on the CARRIED-INSTRUCTION reading: the rule reaches the agent. Glasshouse does not itself perform, evaluate or enforce it, and this package adds no analyser that could.
- Proves the text arrives on the harness's terminal. Does not prove any agent read it, obeyed it, or changed a decision because of it -- that is Phase 51's question and it has no producer today.

