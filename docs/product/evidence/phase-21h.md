# Capability evidence — phase 21H

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 21H — simplicity-first implementation policy, 10 of 10 (lines 955–964)

Package `GH-IMPLEMENTATION-POLICY`, 2026-08-31, Opus at high. The policy is **Glasshouse-authored text delivered like a briefing**: thirty `Rule { id, line, text }` values in three parts, rendered by one `segments` function so the text a person reads (`glasshouse policy`, `Request::ImplementationPolicy`, the MCP tool `glasshouse_implementation_policy`) and the text a session receives (`deliver_policy`, once per session, `MessageOrigin::Machine`) cannot drift. **All thirty close on the carried-instruction reading**: the rule reaches the agent that can act on it; Glasshouse performs none of these checks and this package invents no analyser — refusing that reading for one line would have implied the other twenty-nine were mechanised. Delivery is six bounded lines, not one: `MAX_CANON` is ~1022 bytes on macOS and `phase-27.md` measured that a longer line is discarded *and wedges the session's input*. The switch is `implementation_policy = true|false`, flat like `memory_extraction`, on the project then user layer. Thirty mutations, thirty killed — after the worker discovered that `scripts/mutate.sh --script` had been manufacturing false KILLEDs for deletion mutations (fixed in `efd6e65`), re-ran with a compiling replacement, found one real SURVIVED, and closed it with a test.

### Add a project-level implementation policy that prefers the simplest correct, secure, maintainable, and scalable design satisfying current requirements. (line 955)

Contract: Given an agent Glasshouse briefs -- at spawn with a task, or on its first message -- when the briefing is assembled, Glasshouse delivers rule `s1` of the simplicity-first part (Phase 21H) as part of a labelled, Glasshouse-authored block, so that the instruction reaches the agent that can act on it, while preserving the marker separation from extracted memory and staying inside the terminal's canonical line limit.

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
| crates/glasshouse/src/policy/mod.rs :: 'simplest correct, secure, maintainable and scalable design' -> 'DELETED' | `delete-the-carried-text` | **killed** | `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers` |

> delete-the-carried-text observed: map line 955 (rule `s1`) must reach the agent exactly once, as the phrase "simplest correct, secure, maintainable and scalable design" -- printed with the whole delivered policy, showing `(s1) ... DELETED ...` on the terminal

Recorded scope limits — stated by the worker, not discovered later:
- Closed on the CARRIED-INSTRUCTION reading: the rule reaches the agent. Glasshouse does not itself perform, evaluate or enforce it, and this package adds no analyser that could.
- Proves the text arrives on the harness's terminal. Does not prove any agent read it, obeyed it, or changed a decision because of it -- that is Phase 51's question and it has no producer today.


---


### Require agents to revisit a stale ordinary decision before introducing significant complexity solely to preserve it. (line 956)

Contract: Given an agent Glasshouse briefs -- at spawn with a task, or on its first message -- when the briefing is assembled, Glasshouse delivers rule `s2` of the simplicity-first part (Phase 21H) as part of a labelled, Glasshouse-authored block, so that the instruction reaches the agent that can act on it, while preserving the marker separation from extracted memory and staying inside the terminal's canonical line limit.

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
| crates/glasshouse/src/policy/mod.rs :: 'revisit a stale ordinary decision' -> 'DELETED' | `delete-the-carried-text` | **killed** | `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers` |

> delete-the-carried-text observed: map line 956 (rule `s2`) must reach the agent exactly once, as the phrase "revisit a stale ordinary decision" -- printed with the whole delivered policy, showing `(s2) ... DELETED ...` on the terminal

Recorded scope limits — stated by the worker, not discovered later:
- Closed on the CARRIED-INSTRUCTION reading: the rule reaches the agent. Glasshouse does not itself perform, evaluate or enforce it, and this package adds no analyser that could.
- Proves the text arrives on the harness's terminal. Does not prove any agent read it, obeyed it, or changed a decision because of it -- that is Phase 51's question and it has no producer today.


---


### Discourage compatibility shims when removing or superseding an obsolete internal rule is cleaner and safe. (line 957)

Contract: Given an agent Glasshouse briefs -- at spawn with a task, or on its first message -- when the briefing is assembled, Glasshouse delivers rule `s3` of the simplicity-first part (Phase 21H) as part of a labelled, Glasshouse-authored block, so that the instruction reaches the agent that can act on it, while preserving the marker separation from extracted memory and staying inside the terminal's canonical line limit.

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
| crates/glasshouse/src/policy/mod.rs :: 'do not add a compatibility shim' -> 'DELETED' | `delete-the-carried-text` | **killed** | `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers` |

> delete-the-carried-text observed: map line 957 (rule `s3`) must reach the agent exactly once, as the phrase "do not add a compatibility shim" -- printed with the whole delivered policy, showing `(s3) ... DELETED ...` on the terminal

Recorded scope limits — stated by the worker, not discovered later:
- Closed on the CARRIED-INSTRUCTION reading: the rule reaches the agent. Glasshouse does not itself perform, evaluate or enforce it, and this package adds no analyser that could.
- Proves the text arrives on the harness's terminal. Does not prove any agent read it, obeyed it, or changed a decision because of it -- that is Phase 51's question and it has no producer today.


---


### Discourage duplicate code paths created only to satisfy contradictory historical memories. (line 958)

Contract: Given an agent Glasshouse briefs -- at spawn with a task, or on its first message -- when the briefing is assembled, Glasshouse delivers rule `s4` of the simplicity-first part (Phase 21H) as part of a labelled, Glasshouse-authored block, so that the instruction reaches the agent that can act on it, while preserving the marker separation from extracted memory and staying inside the terminal's canonical line limit.

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
| crates/glasshouse/src/policy/mod.rs :: 'do not duplicate a code path' -> 'DELETED' | `delete-the-carried-text` | **killed** | `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers` |

> delete-the-carried-text observed: map line 958 (rule `s4`) must reach the agent exactly once, as the phrase "do not duplicate a code path" -- printed with the whole delivered policy, showing `(s4) ... DELETED ...` on the terminal

Recorded scope limits — stated by the worker, not discovered later:
- Closed on the CARRIED-INSTRUCTION reading: the rule reaches the agent. Glasshouse does not itself perform, evaluate or enforce it, and this package adds no analyser that could.
- Proves the text arrives on the harness's terminal. Does not prove any agent read it, obeyed it, or changed a decision because of it -- that is Phase 51's question and it has no producer today.


---


### Discourage speculative abstraction that is not justified by current requirements or observed extension pressure. (line 959)

Contract: Given an agent Glasshouse briefs -- at spawn with a task, or on its first message -- when the briefing is assembled, Glasshouse delivers rule `s5` of the simplicity-first part (Phase 21H) as part of a labelled, Glasshouse-authored block, so that the instruction reaches the agent that can act on it, while preserving the marker separation from extracted memory and staying inside the terminal's canonical line limit.

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
| crates/glasshouse/src/policy/mod.rs :: 'do not abstract speculatively' -> 'DELETED' | `delete-the-carried-text` | **killed** | `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers` |

> delete-the-carried-text observed: map line 959 (rule `s5`) must reach the agent exactly once, as the phrase "do not abstract speculatively" -- printed with the whole delivered policy, showing `(s5) ... DELETED ...` on the terminal

Recorded scope limits — stated by the worker, not discovered later:
- Closed on the CARRIED-INSTRUCTION reading: the rule reaches the agent. Glasshouse does not itself perform, evaluate or enforce it, and this package adds no analyser that could.
- Proves the text arrives on the harness's terminal. Does not prove any agent read it, obeyed it, or changed a decision because of it -- that is Phase 51's question and it has no producer today.


---


### Prefer existing language, framework, database, and platform primitives over custom mechanisms when they satisfy the requirement cleanly. (line 960)

Contract: Given an agent Glasshouse briefs -- at spawn with a task, or on its first message -- when the briefing is assembled, Glasshouse delivers rule `s6` of the simplicity-first part (Phase 21H) as part of a labelled, Glasshouse-authored block, so that the instruction reaches the agent that can act on it, while preserving the marker separation from extracted memory and staying inside the terminal's canonical line limit.

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
| crates/glasshouse/src/policy/mod.rs :: 'primitives you already have' -> 'DELETED' | `delete-the-carried-text` | **killed** | `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers` |

> delete-the-carried-text observed: map line 960 (rule `s6`) must reach the agent exactly once, as the phrase "primitives you already have" -- printed with the whole delivered policy, showing `(s6) ... DELETED ...` on the terminal

Recorded scope limits — stated by the worker, not discovered later:
- Closed on the CARRIED-INSTRUCTION reading: the rule reaches the agent. Glasshouse does not itself perform, evaluate or enforce it, and this package adds no analyser that could.
- Proves the text arrives on the harness's terminal. Does not prove any agent read it, obeyed it, or changed a decision because of it -- that is Phase 51's question and it has no producer today.


---


### Prefer explicit straightforward implementations over clever indirection when both satisfy the same requirements. (line 961)

Contract: Given an agent Glasshouse briefs -- at spawn with a task, or on its first message -- when the briefing is assembled, Glasshouse delivers rule `s7` of the simplicity-first part (Phase 21H) as part of a labelled, Glasshouse-authored block, so that the instruction reaches the agent that can act on it, while preserving the marker separation from extracted memory and staying inside the terminal's canonical line limit.

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
| crates/glasshouse/src/policy/mod.rs :: 'clever indirection' -> 'DELETED' | `delete-the-carried-text` | **killed** | `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers` |

> delete-the-carried-text observed: map line 961 (rule `s7`) must reach the agent exactly once, as the phrase "clever indirection" -- printed with the whole delivered policy, showing `(s7) ... DELETED ...` on the terminal

Recorded scope limits — stated by the worker, not discovered later:
- Closed on the CARRIED-INSTRUCTION reading: the rule reaches the agent. Glasshouse does not itself perform, evaluate or enforce it, and this package adds no analyser that could.
- Proves the text arrives on the harness's terminal. Does not prove any agent read it, obeyed it, or changed a decision because of it -- that is Phase 51's question and it has no producer today.


---


### Allow smart implementation choices that materially improve correctness, security, scalability, latency, or operational simplicity. (line 962)

Contract: Given an agent Glasshouse briefs -- at spawn with a task, or on its first message -- when the briefing is assembled, Glasshouse delivers rule `s8` of the simplicity-first part (Phase 21H) as part of a labelled, Glasshouse-authored block, so that the instruction reaches the agent that can act on it, while preserving the marker separation from extracted memory and staying inside the terminal's canonical line limit.

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
| crates/glasshouse/src/policy/mod.rs :: 'a smart choice is allowed' -> 'DELETED' | `delete-the-carried-text` | **killed** | `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers` |

> delete-the-carried-text observed: map line 962 (rule `s8`) must reach the agent exactly once, as the phrase "a smart choice is allowed" -- printed with the whole delivered policy, showing `(s8) ... DELETED ...` on the terminal

Recorded scope limits — stated by the worker, not discovered later:
- Closed on the CARRIED-INSTRUCTION reading: the rule reaches the agent. Glasshouse does not itself perform, evaluate or enforce it, and this package adds no analyser that could.
- Proves the text arrives on the harness's terminal. Does not prove any agent read it, obeyed it, or changed a decision because of it -- that is Phase 51's question and it has no producer today.


---


### Require the agent to explain unusual complexity when a simpler implementation appears available. (line 963)

Contract: Given an agent Glasshouse briefs -- at spawn with a task, or on its first message -- when the briefing is assembled, Glasshouse delivers rule `s9` of the simplicity-first part (Phase 21H) as part of a labelled, Glasshouse-authored block, so that the instruction reaches the agent that can act on it, while preserving the marker separation from extracted memory and staying inside the terminal's canonical line limit.

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
| crates/glasshouse/src/policy/mod.rs :: 'explain unusual complexity' -> 'DELETED' | `delete-the-carried-text` | **killed** | `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers` |

> delete-the-carried-text observed: map line 963 (rule `s9`) must reach the agent exactly once, as the phrase "explain unusual complexity" -- printed with the whole delivered policy, showing `(s9) ... DELETED ...` on the terminal

Recorded scope limits — stated by the worker, not discovered later:
- Closed on the CARRIED-INSTRUCTION reading: the rule reaches the agent. Glasshouse does not itself perform, evaluate or enforce it, and this package adds no analyser that could.
- Proves the text arrives on the harness's terminal. Does not prove any agent read it, obeyed it, or changed a decision because of it -- that is Phase 51's question and it has no producer today.


---


### Treat simplicity as a design constraint rather than as permission to ignore real scale or security requirements. (line 964)

Contract: Given an agent Glasshouse briefs -- at spawn with a task, or on its first message -- when the briefing is assembled, Glasshouse delivers rule `s10` of the simplicity-first part (Phase 21H) as part of a labelled, Glasshouse-authored block, so that the instruction reaches the agent that can act on it, while preserving the marker separation from extracted memory and staying inside the terminal's canonical line limit.

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
| crates/glasshouse/src/policy/mod.rs :: 'simplicity is a design constraint' -> 'DELETED' | `delete-the-carried-text` | **killed** | `implementation_policy::the_policy_reaches_a_spawned_session_once_after_the_memory_briefing_and_inside_its_own_markers` |

> delete-the-carried-text observed: map line 964 (rule `s10`) must reach the agent exactly once, as the phrase "simplicity is a design constraint" -- printed with the whole delivered policy, showing `(s10) ... DELETED ...` on the terminal

Recorded scope limits — stated by the worker, not discovered later:
- Closed on the CARRIED-INSTRUCTION reading: the rule reaches the agent. Glasshouse does not itself perform, evaluate or enforce it, and this package adds no analyser that could.
- Proves the text arrives on the harness's terminal. Does not prove any agent read it, obeyed it, or changed a decision because of it -- that is Phase 51's question and it has no producer today.

