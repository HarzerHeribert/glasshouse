# Capability evidence — phase 55

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules). Phase 55 — the V1 completion definition — is a list of *"Consider V1 usable when …"* criteria over phases the map already records as closed. Each entry below is a tripwire test named after its criterion, driving the shipped binary or the exact production seam the underlying phase's own entry names, with one KILLED mutation on that seam. The underlying phase's entry remains the proof of the mechanism; these entries prove the criterion still holds on the tree as shipped.

### Lines 1930–1937 — routing, quota, reserve, guardrail and explanation criteria

Package `GH-PROVE-IT-V1-ROUTING`, 2026-08-31, Sonnet at medium (Green — tests and mutations only; no production change). One new file, `tests/v1_criteria_routing.rs`, eight tests, 8/8 mutations KILLED. Two mutations SURVIVED on the first pass because the first-draft tests did not put the mutated value under real competition (1930: warm and fresh must be in ONE candidate list; 1931: the unit must be pinned beside the number, not matched as the pool label's own word); both tests were strengthened and re-run KILLED — recorded by the worker as history rather than rewritten.

### Consider V1 usable when a simple router can choose between an existing relevant session and a fresh session using inspectable rules. (line 1930)

Contract: Given a relevant warm session offered alongside a fresh alternative, SessionRouter::choose continues the warm one and names session affinity; given no warm session, it starts fresh, while preserving the affinity term as inert rather than absent.

State: COMPLETE — ruled 2026-08-31 by the orchestrator. A Phase 55 line is a completion CRITERION over a phase already ruled COMPLETE in its own entry; this test is the criterion's tripwire. Artifacts: the report's `test result:` lines (8/8, plus the four neighbouring suites green), one KILLED mutation per line with its killing test named, and the model-level proofs declared as limits with the production caller that reaches each.

Production evidence:
- `src/routing/session.rs` — `SessionRouter::choose`
- `src/routing/session.rs` — `session_affinity`

Regression evidence:
- `v1_criteria_routing::line_1930_the_router_chooses_a_relevant_warm_session_or_starts_fresh_and_names_the_rule`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| Contribution::new("session affinity", breakdown.total(), breakdown.to_string()) -> Contribution::new("session affinity", -breakdown.total(), breakdown.to_string()) | `invert-warm-fresh-preference` | **killed** | `v1_criteria_routing::line_1930_the_router_chooses_a_relevant_warm_session_or_starts_fresh_and_names_the_rule` |

> invert-warm-fresh-preference observed: assertion left == right failed: one relevant warm session, offered against a fresh alternative, must be the one chosen: destination fresh-alternative on claude-code via anthropic (fresh)

Recorded scope limits — stated by the worker, not discovered later:
- proven for SessionStart with no current destination; the task-boundary/mid-turn re-decision gate (line 1592) is tests/session_router.rs's own territory

---

### Consider V1 usable when at least one authoritative or observed provider quota can be displayed in native units. (line 1931)

Contract: Given a fixture provider's quota headers planted where the gateway writes them, glasshouse resources displays the reading in the provider's own units next to the number, never a bare percentage.

State: COMPLETE — ruled 2026-08-31 by the orchestrator. A Phase 55 line is a completion CRITERION over a phase already ruled COMPLETE in its own entry; this test is the criterion's tripwire. Artifacts: the report's `test result:` lines (8/8, plus the four neighbouring suites green), one KILLED mutation per line with its killing test named, and the model-level proofs declared as limits with the production caller that reaches each.

Production evidence:
- `src/provider/resources.rs` — `render_pool`
- `src/provider/resources.rs` — `render_amount`
- `src/provider/telemetry.rs` — `GatewayQuotaCache`

Regression evidence:
- `v1_criteria_routing::line_1931_a_fixture_providers_quota_headers_render_in_native_units_not_a_bare_percentage`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| format!("{} {}", amount.value(), amount.unit()) -> format!("{}", amount.value()) | `drop-the-unit` | **killed** | `v1_criteria_routing::line_1931_a_fixture_providers_quota_headers_render_in_native_units_not_a_bare_percentage` |

> drop-the-unit observed: assertion failed at the unit-adjacency check after the test was strengthened to pin "297 requests"/"300 requests" rather than the bare word "requests"

Recorded scope limits — stated by the worker, not discovered later:
- proven via a planted GatewayQuotaCache reading (the gateway's own write door), not a live network capture; the live-capture half is line 1937's own territory

---

### Consider V1 usable when opaque subscription capacity can be represented as unknown or estimated without fabricating exact token counts. (line 1932)

Contract: Given a native subscription resource, ResourceKind::capacity() represents every unpublished pool as ProviderOpaque, Capacity::is_readable() answers false for it, and CapacityState::normalized() computes no percentage with nothing measured.

State: COMPLETE — ruled 2026-08-31 by the orchestrator. A Phase 55 line is a completion CRITERION over a phase already ruled COMPLETE in its own entry; this test is the criterion's tripwire. Artifacts: the report's `test result:` lines (8/8, plus the four neighbouring suites green), one KILLED mutation per line with its killing test named, and the model-level proofs declared as limits with the production caller that reaches each.

Production evidence:
- `src/provider/quota.rs` — `CapacityState::opaque_subscription`
- `src/provider/quota.rs` — `Capacity::is_readable`
- `src/provider/registry.rs` — `ResourceKind::capacity`

Regression evidence:
- `v1_criteria_routing::line_1932_an_opaque_subscriptions_capacity_is_unknown_and_never_fabricated`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| Capacity::Unmeasured | Capacity::DelegatedUpstream | Capacity::Measured(_) -> ... | Capacity::ProviderOpaque | `render-unknown-as-readable` | **killed** | `v1_criteria_routing::line_1932_an_opaque_subscriptions_capacity_is_unknown_and_never_fabricated` |

> render-unknown-as-readable observed: panic at the is_readable() assertion

Recorded scope limits — stated by the worker, not discovered later:
- proven at the model/registry level, the function apply_direct_provider/apply_gateway call on every native-subscription launch; does not additionally drive glasshouse resources's CLI rendering for a subscription resource

---

### Consider V1 usable when a configurable cheap/free/local routing model can assign workload tiers with deterministic fallback. (line 1933)

Contract: Given a configured cheap routing model reachable through a fixture, glasshouse classify assigns the tier the model answered with; given the same model unreachable, it falls back to the deterministic heuristic and prints the degrade.

State: COMPLETE — ruled 2026-08-31 by the orchestrator. A Phase 55 line is a completion CRITERION over a phase already ruled COMPLETE in its own entry; this test is the criterion's tripwire. Artifacts: the report's `test result:` lines (8/8, plus the four neighbouring suites green), one KILLED mutation per line with its killing test named, and the model-level proofs declared as limits with the production caller that reaches each.

Production evidence:
- `src/main.rs` — `the glasshouse classify handler's ClassificationAttempt::Failed arm`

Regression evidence:
- `v1_criteria_routing::line_1933_a_configured_routing_model_assigns_a_tier_and_a_failing_one_falls_back_and_says_so`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| eprintln!("glasshouse: {why}; deterministic heuristics answered instead"); -> let _ = &why; | `drop-the-fallback-message` | **killed** | `v1_criteria_routing::line_1933_a_configured_routing_model_assigns_a_tier_and_a_failing_one_falls_back_and_says_so` |

> drop-the-fallback-message observed: panic: the fallback message assertion on stderr failed

Recorded scope limits — stated by the worker, not discovered later:
- proven via glasshouse classify (routing::classify::classify's Some(model) caller); the glasshouse launch/route classification call site is a separate, structurally identical seam already covered by tests/classification_call.rs and tests/route_command.rs

---

### Consider V1 usable when protected premium reserve can influence a routing decision. (line 1934)

Contract: Given a resource in the Reserve band with a cheaper adequate alternative and a task below the heavy tier, evaluate_reserve_spend denies the spend and names the reserve and its map line in the reason; above the Reserve band, it is never denied by this policy.

State: COMPLETE — ruled 2026-08-31 by the orchestrator. A Phase 55 line is a completion CRITERION over a phase already ruled COMPLETE in its own entry; this test is the criterion's tripwire. Artifacts: the report's `test result:` lines (8/8, plus the four neighbouring suites green), one KILLED mutation per line with its killing test named, and the model-level proofs declared as limits with the production caller that reaches each.

Production evidence:
- `src/provider/quota.rs` — `evaluate_reserve_spend`

Regression evidence:
- `v1_criteria_routing::line_1934_a_reserve_protecting_a_premium_resource_routes_low_tier_work_away_and_names_it`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| if inputs.cheaper_adequate_resource_exists { -> if false { | `ignore-the-reserve` | **killed** | `v1_criteria_routing::line_1934_a_reserve_protecting_a_premium_resource_routes_low_tier_work_away_and_names_it` |

> ignore-the-reserve observed: panic: the denial assertion failed, the policy allowed the spend

Recorded scope limits — stated by the worker, not discovered later:
- proven at evaluate_reserve_spend directly, the function routing/disposable.rs::choose calls with real inputs; does not additionally drive a full DisposableRouting::choose call with a real candidate list

---

### Consider V1 usable when a substantial high-risk task can record a small set of critical assumptions with evidence state and create a checkpoint before broad implementation. (line 1935)

Contract: Given a substantial change (a stated migration), the preflight door takes a checkpoint before any implementation, and the assumption recorded afterward carries an explicit evidence-source state among its six required fields, never a reasoning field.

State: COMPLETE — ruled 2026-08-31 by the orchestrator. A Phase 55 line is a completion CRITERION over a phase already ruled COMPLETE in its own entry; this test is the criterion's tripwire. Artifacts: the report's `test result:` lines (8/8, plus the four neighbouring suites green), one KILLED mutation per line with its killing test named, and the model-level proofs declared as limits with the production caller that reaches each.

Production evidence:
- `src/api/unix.rs` — `the Preflight/RecordAssumption handlers`
- `src/guardrails/mod.rs`

Regression evidence:
- `v1_criteria_routing::line_1935_a_substantial_task_records_assumptions_with_evidence_state_and_checkpoints_first`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| answer.risk == RiskClass::Substantial -> answer.risk == RiskClass::Trivial | `skip-the-checkpoint` | **killed** | `v1_criteria_routing::line_1935_a_substantial_task_records_assumptions_with_evidence_state_and_checkpoints_first` |

> skip-the-checkpoint observed: panic: the checkpoint-id extraction failed, no checkpoint was taken

Recorded scope limits — stated by the worker, not discovered later:
- proven over glasshouse mcp serve's stdio protocol; the Unix-only api serve + fake-harness path is tests/assumption_guardrails.rs's own territory and was not re-driven here

---

### Consider V1 usable when routing explanations show workload tier, session affinity, resource capacity, and the primary reason for selection. (line 1936)

Contract: One glasshouse route decision's report, for a classified task, shows workload tier fit, session affinity and known quota pressure as named contributions, and the largest-magnitude contribution in the why section carries real explanatory text.

State: COMPLETE — ruled 2026-08-31 by the orchestrator. A Phase 55 line is a completion CRITERION over a phase already ruled COMPLETE in its own entry; this test is the criterion's tripwire. Artifacts: the report's `test result:` lines (8/8, plus the four neighbouring suites green), one KILLED mutation per line with its killing test named, and the model-level proofs declared as limits with the production caller that reaches each.

Production evidence:
- `src/routing/session.rs` — `workload_tier_fit`
- `src/routing/session.rs` — `session_affinity`
- `src/routing/session.rs` — `quota_pressure`
- `src/routing/session.rs` — `Routed::render_overview`

Regression evidence:
- `v1_criteria_routing::line_1936_the_route_report_shows_workload_tier_affinity_capacity_and_a_primary_reason`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| explanation.push(workload_tier_fit(destination, movement.preferred_tier())); -> let _ = workload_tier_fit(destination, movement.preferred_tier()); | `drop-workload-tier-from-report` | **killed** | `v1_criteria_routing::line_1936_the_route_report_shows_workload_tier_affinity_capacity_and_a_primary_reason` |

> drop-workload-tier-from-report observed: panic: the workload tier fit assertion on the report text failed

Recorded scope limits — stated by the worker, not discovered later:
- "the primary reason" is operationalised as the largest-magnitude why-section contribution carrying non-trivial explanatory text; the report has no field literally named "primary reason"

---

### Consider V1 usable when at least one gateway-backed route records classified success and failure outcomes plus TTFT or TTFC and can cite that evidence in a routing explanation. (line 1937)

Contract: A real gateway, started through the production entry point against a fixture upstream, forwards one request that succeeds and one that fails; routing_observations carries both, classified and timed with first-byte timing on each; a later glasshouse route, pointed at the same data directory, cites the health that exchange left behind.

State: COMPLETE — ruled 2026-08-31 by the orchestrator. A Phase 55 line is a completion CRITERION over a phase already ruled COMPLETE in its own entry; this test is the criterion's tripwire. Artifacts: the report's `test result:` lines (8/8, plus the four neighbouring suites green), one KILLED mutation per line with its killing test named, and the model-level proofs declared as limits with the production caller that reaches each.

Production evidence:
- `src/gateway/mod.rs` — `the accept loop's telemetry writes`
- `src/gateway/session.rs` — `record_routing_observation`
- `src/main.rs` — `observed_health_of`
- `src/routing/session.rs` — `provider_health`

Regression evidence:
- `v1_criteria_routing::line_1937_a_gateway_backed_route_records_a_success_and_a_failure_and_route_cites_it`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| .with_first_byte_at(exchange.first_byte_at) -> .with_first_byte_at(None) | `drop-first-byte-timing` | **killed** | `v1_criteria_routing::line_1937_a_gateway_backed_route_records_a_success_and_a_failure_and_route_cites_it` |

> drop-first-byte-timing observed: panic: the first-byte-timestamp assertion on the recorded row failed

Recorded scope limits — stated by the worker, not discovered later:
- proven for one provider, one model, one gateway process, two exchanges; does not prove the routing_observations pairwise-correlation citation path (route_correlations_section), which needs two different routes and MIN_CORRELATION_SAMPLE overlapping observations — the citation proven here is the provider-health contribution, fed by the same gateway exchange through GatewayHealthCache
- macOS only, this worktree

---

### Lines 1917–1922 and 1939 — isolation, native sessions, session switching, session records, and the isolation suites

Package `GH-PROVE-IT-V1-SESSIONS`, 2026-08-31, Sonnet at medium (Green — tests and mutations only; no production change). One new file, `tests/v1_criteria_sessions.rs`, seven tests under a real outer PTY where the criterion is interactive. All seven lines close — 1921 over the worker's `open`, which the packet caused (see its entry). Two mutations SURVIVED and were retargeted rather than hidden: `HarnessAdapter::id` — the packet's suggested seam — has no production caller (`HarnessSelection::id` is set in `session::select::select` and never calls it), so the seam that feeds the recorded harness column is `IntegrationId::slug`, where the mutation KILLED. That trait method is presently decorative; a hygiene note, not a criterion gap.

### Consider V1 usable when Glasshouse can start in a Git project and isolate all state to that project. (line 1917)

Contract: Given a fresh temp Git project and a fake $HOME no flag names, when `glasshouse launch claude-code --headless` runs in it, Glasshouse writes state only under that project's own runtime directory, while a second untouched project and the fake $HOME receive nothing.

State: COMPLETE — ruled 2026-08-31 by the orchestrator. A Phase 55 line is a completion CRITERION over a phase already ruled COMPLETE in its own entry; this test is the criterion's tripwire through the shipped binary (a real PTY for the interactive lines). Artifacts: `test result: ok. 7 passed`, the four isolation suites quoted in full (15/15), one KILLED mutation per line on the seam production actually reaches.

Production evidence:
- `project/mod.rs` — `Project::discover`
- `paths.rs` — `RuntimePaths::project_state_dir`

Regression evidence:
- `v1_criteria_sessions::v1_1917_state_is_isolated_to_the_starting_projects_own_runtime_directory`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| let root = std::fs::canonicalize(&raw_root) -> let root = std::fs::canonicalize(raw_root.parent().unwrap_or(&raw_root)) | `project-root-resolver-returns-parent` | **killed** | `v1_criteria_sessions::v1_1917_state_is_isolated_to_the_starting_projects_own_runtime_directory` |

> project-root-resolver-returns-parent observed: glasshouse-facts: mutate m1917-parent-root -> KILLED

Recorded scope limits — stated by the worker, not discovered later:
- does not re-prove the database-level project_id triggers (phase-1.md), only that the shipped binary's file layout is scoped correctly

---

### Consider V1 usable when Claude Code can run as a fully interactive embedded native session. (line 1918)

Contract: Given a project with Claude Code enabled, when the user opens a session, Glasshouse spawns the configured executable in a real PTY, forwards keystrokes to it and back, and records a session whose harness is claude-code.

State: COMPLETE — ruled 2026-08-31 by the orchestrator. A Phase 55 line is a completion CRITERION over a phase already ruled COMPLETE in its own entry; this test is the criterion's tripwire through the shipped binary (a real PTY for the interactive lines). Artifacts: `test result: ok. 7 passed`, the four isolation suites quoted in full (15/15), one KILLED mutation per line on the seam production actually reaches.

Production evidence:
- `harness/claude_code.rs` — `ClaudeCode`
- `integrations/mod.rs` — `IntegrationId::slug`
- `main.rs` — `launch_session`

Regression evidence:
- `v1_criteria_sessions::v1_1918_claude_code_runs_as_a_fully_interactive_embedded_session`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| IntegrationId::ClaudeCode => "claude-code", -> IntegrationId::ClaudeCode => "codex", | `harness-slug-swapped` | **killed** | `v1_criteria_sessions::v1_1918_claude_code_runs_as_a_fully_interactive_embedded_session` |
| IntegrationId::ClaudeCode -> IntegrationId::Codex (in ClaudeCode::id, harness/claude_code.rs) | `adapter-trait-id-has-no-caller` | **SURVIVED — investigate** | `none` |

> harness-slug-swapped observed: glasshouse-facts: mutate m1918-wrong-slug -> KILLED

> adapter-trait-id-has-no-caller observed: test result: ok. 1 passed; 0 failed -- HarnessAdapter::id has no production caller outside session::select, which sets HarnessSelection::id independently

**A SURVIVING MUTATION IS THE MOST VALUABLE OUTCOME HERE** —
it names a case where passing tests do not prove the claimed
behaviour. Do not tick this box; write down what it means.

Recorded scope limits — stated by the worker, not discovered later:
- does not probe a real installed claude executable, only a fake one

---

### Consider V1 usable when Codex can run as a fully interactive embedded native session. (line 1919)

Contract: Given a project with Codex enabled, when the user opens a session, Glasshouse spawns the configured executable in a real PTY, forwards keystrokes to it and back, and records a session whose harness is codex.

State: COMPLETE — ruled 2026-08-31 by the orchestrator. A Phase 55 line is a completion CRITERION over a phase already ruled COMPLETE in its own entry; this test is the criterion's tripwire through the shipped binary (a real PTY for the interactive lines). Artifacts: `test result: ok. 7 passed`, the four isolation suites quoted in full (15/15), one KILLED mutation per line on the seam production actually reaches.

Production evidence:
- `harness/codex.rs` — `Codex`
- `integrations/mod.rs` — `IntegrationId::slug`

Regression evidence:
- `v1_criteria_sessions::v1_1919_codex_runs_as_a_fully_interactive_embedded_session`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| IntegrationId::Codex => "codex", -> IntegrationId::Codex => "claude-code", | `harness-slug-swapped` | **killed** | `v1_criteria_sessions::v1_1919_codex_runs_as_a_fully_interactive_embedded_session` |
| IntegrationId::Codex -> IntegrationId::ClaudeCode (in Codex::id, harness/codex.rs) | `adapter-trait-id-has-no-caller` | **SURVIVED — investigate** | `none` |

> harness-slug-swapped observed: glasshouse-facts: mutate m1919-wrong-slug -> KILLED

> adapter-trait-id-has-no-caller observed: test result: ok. 1 passed; 0 failed -- same finding as 1918's second mutation row

**A SURVIVING MUTATION IS THE MOST VALUABLE OUTCOME HERE** —
it names a case where passing tests do not prove the claimed
behaviour. Do not tick this box; write down what it means.

Recorded scope limits — stated by the worker, not discovered later:
- does not probe a real installed codex executable, only a fake one

---

### Consider V1 usable when the user can switch between multiple live native sessions without restarting them. (line 1920)

Contract: Given two live embedded sessions, when the user focuses the non-presented one from the overview and later switches back, both children's processes are the same ones the whole time and both still answer a forwarded keystroke.

State: COMPLETE — ruled 2026-08-31 by the orchestrator. A Phase 55 line is a completion CRITERION over a phase already ruled COMPLETE in its own entry; this test is the criterion's tripwire through the shipped binary (a real PTY for the interactive lines). Artifacts: `test result: ok. 7 passed`, the four isolation suites quoted in full (15/15), one KILLED mutation per line on the seam production actually reaches.

Production evidence:
- `shell/state.rs` — `ShellState::focus_overview_target`
- `shell/mod.rs` — `sync_focus`

Regression evidence:
- `v1_criteria_sessions::v1_1920_switching_between_two_live_sessions_and_back_does_not_respawn_either`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| self.selected = index; -> self.selected = self.selected; (in ShellState::focus_overview_target) | `focus-never-moves` | **killed** | `v1_criteria_sessions::v1_1920_switching_between_two_live_sessions_and_back_does_not_respawn_either` |

> focus-never-moves observed: glasshouse-facts: mutate m1920-focus-never-moves -> KILLED

Recorded scope limits — stated by the worker, not discovered later:
- the packet's own suggested mutation ("switching respawns the session") has no one-line production seam; see the test's doc comment and the report body

---

### Consider V1 usable when one session can be designated as orchestrator and spawn at least one visible worker session. (line 1921)

Contract: Given a session designated orchestrator, when it spawns one worker session, the worker is visible in the project's session list with its owning orchestrator.

State: COMPLETE — ruled 2026-08-31 by the orchestrator, **overruling the worker's `open`, which followed the packet rather than the map.** The map's words are *"designated as orchestrator and spawn at least one visible worker session"*; the packet added *"with its owning orchestrator"*, a fact the binary does not persist and the map does not ask for. What the worker proved is exactly the map's criterion: a session tagged `orchestrator` spawns a worker through the control socket, both list with their roles, and the worker-default-role mutation is KILLED. The worker's finding stands as a note — no `spawned_by` attribution exists anywhere — but it is not a Phase 55 clause. Packet error: the orchestrator's.

Production evidence:
- `api/unix.rs` — `parse_role, spawn_session, session_summary`

Regression evidence:
- `v1_criteria_sessions::v1_1921_a_designated_orchestrator_spawns_a_worker_visible_in_the_listing_by_role`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| None => Ok(SessionRole::Worker), -> None => Ok(SessionRole::Normal), (in parse_role) | `worker-default-role-lost` | **killed** | `v1_criteria_sessions::v1_1921_a_designated_orchestrator_spawns_a_worker_visible_in_the_listing_by_role` |

> worker-default-role-lost observed: assertion `left == right` failed: a session spawned with no stated role is a worker by default

Recorded scope limits — stated by the worker, not discovered later:
- no field anywhere in SessionRecord/NewSession/session_summary records which orchestrator session's spawn_session call produced a worker -- "with its owning orchestrator" is not provable without a production change, which this packet may not make

---

### Consider V1 usable when every interactive native, direct-provider, or gateway-backed session records a real owning harness and launch profile. (line 1922)

Contract: Given a native, a direct-provider, or a glasshouse-gateway-backed launch profile, when a session starts under it, the recorded session row's harness, launch profile and backend resource are all non-empty.

State: COMPLETE — ruled 2026-08-31 by the orchestrator. A Phase 55 line is a completion CRITERION over a phase already ruled COMPLETE in its own entry; this test is the criterion's tripwire through the shipped binary (a real PTY for the interactive lines). Artifacts: `test result: ok. 7 passed`, the four isolation suites quoted in full (15/15), one KILLED mutation per line on the seam production actually reaches.

Production evidence:
- `main.rs` — `launch_session`

Regression evidence:
- `v1_criteria_sessions::v1_1922_native_direct_and_gateway_launches_all_record_harness_launch_profile_and_backend`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| .with_backend_resource(Some(launch_profile.backend.slug())) -> .with_backend_resource(None) | `backend-resource-dropped` | **killed** | `v1_criteria_sessions::v1_1922_native_direct_and_gateway_launches_all_record_harness_launch_profile_and_backend` |

> backend-resource-dropped observed: assertion `left != right` failed: session 097aa610f87d has no recorded backend resource:

Recorded scope limits — stated by the worker, not discovered later:
- does not re-prove line 368's resume-path re-resolution (phase-9a.md "door two"), a separate already-closed line

---

### Consider V1 complete only after project-isolation and cross-contamination tests pass. (line 1939)

Contract: Given two real projects sharing one --data-dir, when either searches its own memory (through the CLI or the MCP door), it never sees a memory belonging to, or planted from, the other project.

State: COMPLETE — ruled 2026-08-31 by the orchestrator. A Phase 55 line is a completion CRITERION over a phase already ruled COMPLETE in its own entry; this test is the criterion's tripwire through the shipped binary (a real PTY for the interactive lines). Artifacts: `test result: ok. 7 passed`, the four isolation suites quoted in full (15/15), one KILLED mutation per line on the seam production actually reaches.

Production evidence:
- `memory/search.rs` — `MemoryStore::search_matching`
- `database.rs` — `memories_reject_foreign_project_insert trigger`

Regression evidence:
- `project_isolation::canonicalized_paths_cannot_escape_the_project_root_through_parent_directory_traversal`
- `project_isolation::symlink_targets_outside_the_project_root_are_rejected_by_project_config_io`
- `project_isolation::memory_extraction_only_ever_writes_into_its_own_projects_database`
- `project_isolation::deleting_one_projects_state_leaves_a_sibling_projects_state_intact`
- `project_isolation::a_session_from_project_a_cannot_be_resumed_from_project_b`
- `project_isolation::one_project_database_cannot_be_queried_through_another_projects_glasshouse_instance`
- `project_isolation::every_revalidation_primitive_refuses_a_memory_planted_from_another_project_and_writes_nothing`
- `memory_project_scope::the_review_queue_and_the_status_count_never_reach_a_memory_planted_from_another_project`
- `mcp_project_scope::the_mcp_layer_opens_no_store_of_its_own`
- `mcp_project_scope::no_tool_argument_can_name_a_project_a_path_or_a_socket`
- `mcp_project_scope::a_tool_call_naming_another_projects_session_is_refused_without_leaking_its_path`
- `mcp_project_scope::memory_and_checkpoints_are_answered_only_for_the_project_the_server_was_started_in`
- `cmux_project_scope::a_forged_insert_carrying_a_valid_looking_presentation_ref_is_refused_by_the_database_trigger`
- `cmux_project_scope::a_session_recorded_with_a_pane_belongs_to_its_project_like_any_other_session`
- `cmux_project_scope::a_cmux_reference_is_not_a_second_identity_across_projects`
- `v1_criteria_sessions::v1_1939_memory_search_over_the_cli_refuses_a_memory_planted_from_another_project`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| AND memories.project_id = ?2 -> AND (memories.project_id = ?2 OR 1=1) | `memory-search-drops-project-filter` | **killed** | `v1_criteria_sessions::v1_1939_memory_search_over_the_cli_refuses_a_memory_planted_from_another_project` |

> memory-search-drops-project-filter observed: glasshouse-facts: mutate m1939-no-project-filter -> KILLED

Recorded scope limits — stated by the worker, not discovered later:
- the new test covers the CLI door only; the MCP door's equivalent case was already covered by mcp_project_scope.rs and is cited above rather than re-added

---

---


### Lines 1925–1929 and 1938 — orchestration wake-up, worker control, optional cmux, and the memory criteria

Package `GH-PROVE-IT-V1-ORCH-MEMORY`, 2026-08-31, Sonnet at medium (Green — tests and mutations only; no production change). One new file, `tests/v1_criteria_orch_memory.rs`, six tests, 6/6 mutations KILLED, no packet errors. Notable honest limits: 1926's mutation targets the client delivery path because the door has no per-session ownership gate to mutate (`MessageOrigin` does not distinguish a person's input — by design); 1928 proves FTS5 phrase-token semantics across indexed columns against a substring scan, and prefix-`*` semantics do not exist for any caller (`sanitize_query` quotes every token).

### Consider V1 usable when a worker completion event can reliably wake or notify the orchestrator. (line 1925)

Contract: Given an orchestrator that watched a worker, when a real lifecycle hook reports the worker's turn ended, Glasshouse types one worker-completion line into the orchestrator's own terminal, while never touching the worker itself.

State: COMPLETE — ruled 2026-08-31 by the orchestrator. A Phase 55 line is a completion CRITERION over a phase already ruled COMPLETE in its own entry; this test is the criterion's tripwire. Artifacts: `test result: ok. 6 passed`, the five neighbouring suites green, one KILLED mutation per line, clean porcelain after every mutation.

Production evidence:
- `src/api/unix.rs` — `pump_watches`
- `src/api/unix.rs` — `Completion::line`

Regression evidence:
- `v1_criteria_orch_memory::worker_control::a_workers_completion_event_reliably_wakes_the_orchestrator`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| match api.send_text(&watch.notify, &completion.line(), MessageOrigin::Machine) { -> match Ok::<(), ApiError>(()) { | `drop-the-completion-notification` | **killed** | `v1_criteria_orch_memory::worker_control::a_workers_completion_event_reliably_wakes_the_orchestrator` |

> drop-the-completion-notification observed: timed out waiting for the orchestrator to be woken (v1_criteria_orch_memory.rs:285)

Recorded scope limits — stated by the worker, not discovered later:
- does not re-prove the dedup/ordering guarantees (lines 733/739) — tests/worker_wakeup.rs already does and was re-run green

---

### Consider V1 usable when the user can enter and directly control any orchestrated worker. (line 1926)

Contract: Given a live orchestrated worker, a person from their own terminal can send text, interrupt it with a real SIGINT, and read back its terminal output, and the session survives the interrupt.

State: COMPLETE — ruled 2026-08-31 by the orchestrator. A Phase 55 line is a completion CRITERION over a phase already ruled COMPLETE in its own entry; this test is the criterion's tripwire. Artifacts: `test result: ok. 6 passed`, the five neighbouring suites green, one KILLED mutation per line, clean porcelain after every mutation.

Production evidence:
- `src/api/client.rs` — `send_message`
- `src/api/client.rs` — `interrupt`
- `src/api/client.rs` — `read_output`
- `src/session/api.rs` — `SessionApi::recent_output`

Regression evidence:
- `v1_criteria_orch_memory::worker_control::a_user_can_enter_and_directly_control_an_orchestrated_worker`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| "op": "send_message", -> "op": "noop", | `refuse-a-persons-delivery` | **killed** | `v1_criteria_orch_memory::worker_control::a_user_can_enter_and_directly_control_an_orchestrated_worker` |

> refuse-a-persons-delivery observed: timed out waiting for the worker to read the line a person sent (v1_criteria_orch_memory.rs:409)

Recorded scope limits — stated by the worker, not discovered later:
- the mutation targets the client's delivery path rather than a per-session ownership gate, because the door has no such gate to mutate (phase-15.md/phase-16.md: MessageOrigin does not distinguish a person's input from an orchestrator's on this door)

---

### Consider V1 usable when cmux integration can expose or spawn a session externally without being required for normal operation. (line 1938)

Contract: Given cmux absent, every command runs unchanged; given cmux present, a launch asking for a pane opens one and records it as presentation metadata on the SAME session row, never a second identity.

State: COMPLETE — ruled 2026-08-31 by the orchestrator. A Phase 55 line is a completion CRITERION over a phase already ruled COMPLETE in its own entry; this test is the criterion's tripwire. Artifacts: `test result: ok. 6 passed`, the five neighbouring suites green, one KILLED mutation per line, clean porcelain after every mutation.

Production evidence:
- `src/main.rs` — `launch_session (Absent arm eprintln)`
- `src/integrations/cmux.rs` — `CmuxCli`
- `src/session/store.rs` — `SessionRecord::presentation_ref`

Regression evidence:
- `v1_criteria_orch_memory::cmux_optional::cmux_exposes_a_session_externally_and_is_never_required_for_normal_operation`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| eprintln!("glasshouse: cmux is not available ({reason}); the session runs {here}"); -> anyhow::bail!("cmux is not available ({reason})"); | `refuse-instead-of-degrade` | **killed** | `v1_criteria_orch_memory::cmux_optional::cmux_exposes_a_session_externally_and_is_never_required_for_normal_operation` |

> refuse-instead-of-degrade observed: the launch that asked for a pane with no cmux present no longer succeeded (v1_criteria_orch_memory.rs:651)

Recorded scope limits — stated by the worker, not discovered later:
- --fresh was needed on two launches to avoid the router continuing an existing stopped session, which is Phase 42's documented behaviour, not this line's concern

---

### Consider V1 usable when project-specific durable memory can store the six initial memory kinds. (line 1927)

Contract: Given a memory recorded under any of Phase 20's six kinds, the kind returned at write time and the kind read back afresh by id are both the one asked for.

State: COMPLETE — ruled 2026-08-31 by the orchestrator. A Phase 55 line is a completion CRITERION over a phase already ruled COMPLETE in its own entry; this test is the criterion's tripwire. Artifacts: `test result: ok. 6 passed`, the five neighbouring suites green, one KILLED mutation per line, clean porcelain after every mutation.

Production evidence:
- `src/memory/store.rs` — `MemoryStore::record`
- `src/memory/store.rs` — `MemoryStore::get`

Regression evidence:
- `v1_criteria_orch_memory::project_memory_stores_each_of_the_six_kinds_and_reads_each_back_with_its_kind_intact`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| kind: new.kind, -> kind: if matches!(new.kind, MemoryKind::Constraint) { MemoryKind::Decision } else { new.kind }, | `collapse-two-kinds-into-one` | **killed** | `v1_criteria_orch_memory::project_memory_stores_each_of_the_six_kinds_and_reads_each_back_with_its_kind_intact` |

> collapse-two-kinds-into-one observed: the kind handed back at write time must be the one asked for (v1_criteria_orch_memory.rs:803)

---

### Consider V1 usable when project memory can be searched with FTS5. (line 1928)

Contract: A two-word query whose terms sit in different indexed columns of one memory, and never appear together as a literal substring, is found by FTS5 MATCH's cross-column AND semantics.

State: COMPLETE — ruled 2026-08-31 by the orchestrator. A Phase 55 line is a completion CRITERION over a phase already ruled COMPLETE in its own entry; this test is the criterion's tripwire. Artifacts: `test result: ok. 6 passed`, the five neighbouring suites green, one KILLED mutation per line, clean porcelain after every mutation.

Production evidence:
- `src/memory/search.rs` — `MemoryStore::search_matching`
- `src/memory/search.rs` — `sanitize_query`

Regression evidence:
- `v1_criteria_orch_memory::project_memory_is_searched_with_fts5_across_indexed_columns_not_a_substring_scan`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| memories_fts MATCH ?1 -> memories.subject LIKE ?1 | `route-through-a-non-fts-path` | **killed** | `v1_criteria_orch_memory::project_memory_is_searched_with_fts5_across_indexed_columns_not_a_substring_scan` |

> route-through-a-non-fts-path observed: an FTS5 MATCH joins a term in the subject with a term in the body; a substring scan of either column alone could not: [] (v1_criteria_orch_memory.rs:871)

Recorded scope limits — stated by the worker, not discovered later:
- does not test prefix-* semantics: sanitize_query quotes every token as an exact phrase and exposes no prefix operator to any caller

---

### Consider V1 usable when a small portable checkpoint can hand work from one harness to another. (line 1929)

Contract: A checkpoint written under one harness starts a fresh session under a DIFFERENT harness the launch asks for, that session's own process receives the objective and state, and the source session's record is left untouched.

State: COMPLETE — ruled 2026-08-31 by the orchestrator. A Phase 55 line is a completion CRITERION over a phase already ruled COMPLETE in its own entry; this test is the criterion's tripwire. Artifacts: `test result: ok. 6 passed`, the five neighbouring suites green, one KILLED mutation per line, clean porcelain after every mutation.

Production evidence:
- `src/main.rs` — `launch_session (resolve_bootstrap_prompt, checkpoint_command)`
- `src/session/select.rs` — `select_with`

Regression evidence:
- `v1_criteria_orch_memory::checkpoint_handoff::a_checkpoint_written_under_one_harness_hands_work_to_a_session_of_a_different_harness`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| NewSession::embedded(selection.id().slug()) -> NewSession::embedded("antigravity") | `bind-checkpoint-to-the-writing-harness` | **killed** | `v1_criteria_orch_memory::checkpoint_handoff::a_checkpoint_written_under_one_harness_hands_work_to_a_session_of_a_different_harness` |

> bind-checkpoint-to-the-writing-harness observed: assertion `left == right` failed (v1_criteria_orch_memory.rs:1005) — the fresh session was recorded under a fixed harness rather than the one the launch asked for

Recorded scope limits — stated by the worker, not discovered later:
- does not re-prove bootstrap_prompt()'s field ordering or plain-text/no-harness-naming shape — checkpoint_portability.rs and handoff_lines.rs already do and are unchanged

---

---

## Line 1923 — HELD 2026-09-02 (`GH-PAIRING-PRIOR` built the decay; nothing feeds it yet)

(line 1923)

Contract: Given a vendor-native destination that has accumulated at least PAIRING_PRIOR_EVIDENCE_THRESHOLD local observations, when Glasshouse scores it, the prior reads 0.0 with text saying observed evidence replaced it; given a RoutingOverride naming the non-native destination, the override wins regardless of the prior; while preserving that no candidate is ever rejected on this axis.

State: **PARTIALLY VERIFIED — HELD, not ticked.** Ruled 2026-09-02. The *user choice* half is proven (`apply_override` wins over the prior, with a test). The *without overriding stronger observed evidence* half has its mechanism — `pairing_prior` decays to `0.0` at `PAIRING_PRIOR_EVIDENCE_THRESHOLD`, mutation KILLED — but its input, `Destination::pairing_prior_evidence`, has **no production caller**: every destination `routing_destinations` builds carries `0`, so on the shipped path the prior never yields to evidence. The worker found this itself: the packet's two suggested evidence sources were not growing counters (`consecutive_failures` resets on success), and it added the field in the file's own wire-now/populate-later shape rather than borrow the wrong signal. Successor, named: `GH-PAIRING-EVIDENCE` — count the destination's own rows in the window `routing_destinations` already reads (`consumption`, since `bc81b11`) and call `with_pairing_prior_evidence`; ticks then, with a mutation on the count.

Production evidence:
- `crates/glasshouse/src/routing/session.rs` — `pairing_prior`
- `crates/glasshouse/src/routing/session.rs` — `Destination::pairing_prior_evidence`
- `crates/glasshouse/src/routing/session.rs` — `SessionRouter::apply_override`

Regression evidence:
- `routing::session::pairing_prior_tests::accumulated_local_evidence_decays_the_prior_to_zero`
- `routing::session::pairing_prior_tests::a_user_override_naming_the_non_native_destination_wins`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| if observed >= PAIRING_PRIOR_EVIDENCE_THRESHOLD { -> if false { | `remove-evidence-decay` | **killed** | `routing::session::pairing_prior_tests::accumulated_local_evidence_decays_the_prior_to_zero` |

> remove-evidence-decay observed: assertion `left == right` failed: accumulated local evidence must decay the prior to zero; left: 0.2 right: 0.0

Recorded scope limits — stated by the worker, not discovered later:
- Destination::pairing_prior_evidence has no production caller yet (see packet_errors and the report's own section on this gap)

---
