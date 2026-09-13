# Capability evidence — phase 65

Phase 65 — Pane: smarter-and-cheaper execution (map lines 2580–2594), recorded 2026-09-13 from the user's ruling (`design-decisions.md`, *Pane wins by spending less expensive attention, not by forcing an interface*). The register is `docs/product/pane/smarter-cheaper-roadmap.md`; this file is its evidence ledger. Entries are bounded by the *Decompression* ruling: the contract, the tests by name, the decisive mutation where a decision was added, the limits, and the report by path.

**Non-negotiables every entry below is checked against.** One canonical execution kernel — a direct provider tool and an authored cell lower into the same isolate; no second executor, helper runtime, ledger or evidence class. Exact evidence stays retrievable behind every bounded or derived view. No direct-tool quota; interface choice is measured, never forced. Ordinary host security is unchanged; outer-container behaviour is separately named and fails closed elsewhere.

**Gate.** `cargo test -p pane --no-fail-fast`, `cargo clippy -p pane --all-targets -- -D warnings`, `cargo fmt -p pane --check`, `git diff --check`; the pane cells of the GitHub sweep are the platform verdict.

**Gate result, 2026-09-13 (integrated tree, `scratchpad/pane-gate-2.log`).** `cargo test -p pane --no-fail-fast`: 92 targets, 1386 passed, 0 failed, 1 ignored (the pre-existing ignore); `cargo clippy -p pane --all-targets -- -D warnings` clean; `cargo fmt -p pane -- --check` clean; `git diff --check` clean. The GitHub sweep has not run (nothing pushed, by instruction).

**Mutation table (one decisive mutation per Amber decision; `scripts/mutate.sh --script`, every file restored byte-identical).**

| Decision | Mutation | Killing target | Result |
|---|---|---|---|
| A mutation Pane performed is a version Pane knows (2585) | `runtime/bindings.rs`: register the reversed SHA-256 instead of the real one | `--test mutation_composition` | KILLED — `a_written_file_can_be_edited_without_a_context`, `an_edit_in_a_later_cell_binds_to_panes_own_previous_edit`, `two_sequential_edits_of_one_file_in_one_cell_both_apply` fail with "The source version changed" |
| Debuggers are admitted only in container mode (2583) | `sandbox/profile.rs`: `escaping_command(command_line, false)` | `--test container_profile` | KILLED |
| Container reads span the container (2583) | `sandbox/profile.rs`: the container read branch never taken | `--test container_profile` | KILLED |
| Missing verification holds only where a check is declared (2590) | `completion.rs`: drop the `verification_expected` guard | `--test final_state_contract --test evidence_gate` | KILLED |
| A multi-call direct frame isolates its calls (2584) | `abi/intent.rs`: `let isolated = false` | `--test direct_frame_outcomes` | KILLED |

The first mutation's literal-zero form was a COMPILE-ERROR (the orphaned `sha256` binding under `-D warnings`), so it was re-run keeping the symbol in use; the table records the run that proved something. Worker-reported mutations for the observation delta and the exact-edit hunks are in their line entries.

---

## The build — one integration, nine packages, 2026-09-13

Nine packages ran in parallel worktrees on disjoint file surfaces (TypeScript contract; the execution kernel; the container profile and manifest; the interface-aware prompt; telemetry; the observation delta; the capsule, completion and progress modules; the scouting preflight; the ruler ablation) and were integrated by the primary in one tree, with the session loop, the wire layer and the isolate glue written by the primary. Records: the workflow journal `wf_087d7824-383` (session scratch), the briefs under the session scratchpad `briefs/`. Gate results are at the end of this file.

### Line 2580 — interface-origin telemetry

Contract: given a task run with `--output-format json|stream-json`, the result's `telemetry` names the provider-selected interface (`execute_cell` calls vs direct calls by name), frames and operations by origin, failures by kind and origin, lifting, observation and reduction figures, and parent usage by request cause — while preserving every v1 field. Production: `crates/pane/src/abi/telemetry.rs` (`FailureKind`, `RequestCause`, `classify_call`, `classify_cell_error`, `request_cause`, `is_single_intent_cell`, `shell_family`), `crates/pane/src/session/output.rs` (`interface`, `cell_frame`, `parent_request_started_with`, `completion`, `no_progress_notice`, `capsule`), `session.rs` (`write_cell`, `timed_send_task_turn`, `TaskState::next_cause`). Tests: `tests/telemetry_taxonomy.rs` (23), `tests/session_output.rs::{provider_selected_interface_is_counted_and_every_v1_field_stays, direct_tool_and_authored_frames_are_counted_by_origin, a_request_after_a_thrown_cell_is_charged_to_repair}`. Amber: the classifiers are decisions; the decisive check is the repair-cause test, which fails if `request_cause` ignores `previous_failed`. Limits: `is_single_intent_cell` is a documented heuristic with false negatives only; `little_helper` origin rows are emitted at zero.

State: **COMPLETE** — implementation, tests and the gate below; ticked 2026-09-13.

### Line 2581 — the TypeScript contract

Contract: every construct in `cell::ERASABLE_CONSTRUCTS` compiles, keeps every line's width and runs; every construct in `NOT_ERASABLE_CONSTRUCTS` is refused before execution with its name and an alternative; `as const` in an object literal runs. Production: `crates/pane/src/runtime/cell.rs` (`FreeNames` no longer visits a `TSType`; the two tables; `alternative_for`). Tests: `tests/typescript_contract.rs::{as_const_in_an_object_literal_compiles_and_runs, every_erasable_construct_compiles_keeps_its_columns_and_runs, every_refused_construct_is_named_before_execution_with_an_alternative, a_type_only_name_is_still_undefined_at_value_level}`, `runtime::cell::tests` (23). Mutation (worker): `visit_ts_type` restored to the default walk → KILLED by `a_type_position_contributes_no_free_name` and the two integration tests, with the pilot's exact `ReferenceError: \`const\` is not defined` message. Limits: decorators, index signatures and `export` are in neither table.

State: **COMPLETE** — implementation, tests and the gate below; ticked 2026-09-13.

### Line 2582 — the capability/environment manifest

Contract: the system block carries an `## Environment` block built from the compiled profile (readable and writable roots, reserved paths, deny patterns, command policy, never-admitted names, present and absent executables, container mode, unavailable capabilities), byte-equal to what `session_facts_with`/`system_manifest` build from the same profile. Production: `crates/pane/src/manifest.rs` (`Manifest::collect`, `render`), `session.rs::{system_manifest, session_facts_with, build_system_prompt, MANIFEST_PROBE}`, `prompt/mod.rs` (`SessionFacts.manifest`). Tests: `tests/session.rs::the_system_block_is_render_systems_own_bytes`, `tests/container_profile.rs` (manifest cases), `manifest::tests`. Green (wiring an existing fact). Limits: the probe list is fixed; executables are resolved on `PATH` only.

State: **COMPLETE** — implementation, tests and the gate below; ticked 2026-09-13.

### Line 2583 — the container profile

Contract: under the explicit Linux-only `--yolo --dangerously-bypass-os-sandbox`, reads outside the root are granted unless a never-grantable rule or a `deny` pattern refuses, and debuggers are admitted; sandbox launchers stay refused, writes stay root plus `--add-dir`, credential stores stay refused; an ordinary session is byte-identical. Production: `crates/pane/src/sandbox/profile.rs` (`container_mode`, `NEVER_GRANTABLE_LAUNCHERS`/`NEVER_GRANTABLE_DEBUGGERS`, `check`, `admits_command`). Tests: `tests/container_profile.rs` (12), `tests/sandbox_profile.rs` (36), `tests/additional_roots.rs` (7), `tests/sandbox_apply.rs` (27) unchanged. Red by the CLAUDE.md table (sandbox policy): the primary read the diff of `check` and `admits_command`; the decisive tests are the `~/.ssh`-in-container-mode refusal and the write-outside-root-in-container-mode refusal. Limits: platform appliers are untouched — in container mode no OS sandbox is applied by design; no process is spawned by the tests.

State: **COMPLETE** — implementation, tests and the gate below; ticked 2026-09-13.

### Line 2584 — typed outcomes per call

Contract: a lowered frame of two or more direct calls isolates each call (a denial or throw stops no sibling; every call is in the trajectory; each `tool_result` carries its own message; a failed call's binding is not a live handle), and a thrown authored cell's result names the effectful calls that completed. Production: `abi/intent.rs::{guarded_statement, lower}`, `runtime/isolate.rs::free_undefined_direct_bindings`, `session.rs::{partial_effects, act_on}` (the `Threw` arm reads `call.error`). Tests: `tests/direct_frame_outcomes.rs` (5), `session::tests::partial_effects_name_only_completed_effectful_calls_of_a_thrown_cell`. Amber: the lowering shape is the decision; the killing test is the two-call frame whose first call is denied and whose second succeeds.

State: **COMPLETE** — implementation, tests and the gate below; ticked 2026-09-13.

### Line 2585 — transactional mutation composition

Contract: a successful `edit`/`write` registers its resulting version as visible so the next edit binds to it; `edit({olds, replacements})` applies N hunks atomically or leaves the file byte-identical; an external change is still stale. Production: `runtime/state.rs::note_mutation`, `runtime/bindings.rs` (after a successful edit/write), `tools/exact_edit.rs::apply_hunks`, `tools/invoke.rs` (the `edit` arm), `tools/registry.rs` (`ArgKind::Texts`), `abi/dialect.rs`, `abi/types.rs`. Tests: `tests/mutation_composition.rs` (9), `tests/exact_edit.rs` (+3), `tests/context_tools.rs` unchanged. Amber: the decisive test is the external-overwrite-between-edits refusal (`stale_hash`), which fails if the writer stops comparing disk.

State: **COMPLETE** — implementation, tests and the gate below; ticked 2026-09-13.

### Line 2586 — the observation delta

Contract: the turn's `## Handles` renders entries changed this cell or pinned by `keep` in full and every other live handle as one line; `handles()` and the rollout keep the inventory; the first rendering of a task is a full inventory and a later unchanged turn is not. Production: `runtime/handles.rs::{render_table_delta, HandleTable::begin_cell, pin}`, `runtime/isolate.rs` (`begin_cell`, the delta call in `finish`), `runtime/state.rs::note_pin`. Tests: `tests/observation_delta.rs` (8), `tests/handles.rs` (12, golden unchanged). Mutation (worker): `refresh` always counting as a change → KILLED by `a_refresh_is_a_change_only_when_the_rendering_differs`.

State: **COMPLETE** — implementation, tests and the gate below; ticked 2026-09-13.

### Line 2587 — observation dedup and lifting counts

Contract: a pure call whose result hash equals an earlier call's with the same checked arguments carries `repeat_of` and a header suffix; the call still runs; lifting is counted by family. Production: `runtime/state.rs` (the observation ledger), `runtime/bindings.rs`, `session/output.rs` (`lifting`, `observation.repeated_observations`). Tests: `tests/observation_dedup.rs` (7), `tests/lifting.rs` (unchanged, 10). Green.

State: **COMPLETE** — implementation, tests and the gate below; ticked 2026-09-13.

### Line 2588 — adaptive reduction

Contract: the pushed reducer fires above `[helpers] reduce_above_tokens` only when a helper is configured and the expected parent saving is positive; `ReductionStats` count attempts, results, cache hits and bytes; exact output stays complete. Production: `runtime/bindings.rs::reduce_oversized`, `runtime/state.rs` (`ReductionStats`), `config.rs`. Tests: `tests/observation_dedup.rs` (reduction section: above-threshold reaches the reducer once, a repeat is served from the cache, below-threshold makes no request), `tests/helpers.rs` (26). Limits: the saving is a rule, not a measurement; the measurement is the campaign's.

State: **COMPLETE** — implementation, tests and the gate below; ticked 2026-09-13.

### Line 2589 — the task capsule

Contract: a bounded capsule (goal, state, facts with cell references, checkpoint digest, risks, next action, latest verification) is derived from the trajectory, rendered as `## Task` in the live feedback when it changes, carried on every `CellView` and in `telemetry.capsule`. Production: `runtime/capsule.rs`, `session.rs::TaskState::observe`, `tui.rs::CellView.capsule`. Tests: `tests/task_capsule.rs` (4), `tests/evidence_gate.rs::the_task_capsule_reaches_the_feedback_and_the_result`. Green-to-Amber: the render bound (1,200 chars) is pinned by `render_stays_bounded`.

State: **COMPLETE** — implementation, tests and the gate below; ticked 2026-09-13.

### Line 2590 — evidence-gated completion and the final-state contract

Contract: a terminal return is held once when `completion::check` (unexpected compiled artifact beside a deliverable, coverage data outside the source tree, stale verification, no verification where the project declares checks, plus `[contract]` rules) or the fresh checker finds something; the same findings a second time finish with `completion.verified = false`. Production: `completion.rs`, `session.rs::TaskState::gate`, `act_on`'s `Returned` arm, `verification.rs` (`[contract]`). Tests: `tests/final_state_contract.rs` (6, the three pilot fixtures), `tests/evidence_gate.rs::{a_stray_binary_beside_the_deliverable_holds_the_return_once_and_is_recorded_unverified, coverage_data_outside_the_source_tree_is_a_finding_before_completion, a_declared_check_that_never_ran_holds_the_return_and_an_undeclared_one_does_not}` through the built binary. Red (completion semantics): ruled by the primary — `NoVerification` fires only where verification is declared (`Contract::verification_expected`), because holding every mutating task in a project with nothing configured taxed two existing session tests and proved no evidence; `StaleVerification` stays evidence-based.

State: **COMPLETE** — implementation, tests and the gate below; ticked 2026-09-13.

### Line 2591 — the fresh independent checker

Contract: with `[helpers] completion_check = true`, CHECKER runs once per task on the original request, the task diff, the capsule's facts and the findings — never the parent's narrative — and a `does not hold` verdict joins the findings. Production: `completion::fresh_checker_evidence`, `session.rs::TaskState::gate`, `helpers::check_completion`. Tests: `completion::tests` (the evidence carries no control text), `tests/checker_preparation.rs` (unchanged, the checker itself). Limits: no live run; the verdict parse is the first line.

State: **COMPLETE** — implementation, tests and the gate below; ticked 2026-09-13.

### Line 2592 — the scouting preflight and adaptive orchestration

Contract: preflight hands the Scout a scouting brief and never the task; the block renders Constraints/Files/Tests/Capabilities/Risks; with `preflight_scope = "auto"` (default) it runs only on uncertainty signals. Production: `preflight.rs`, `helpers.rs` (SCOUT), `session.rs::preflight_block`, `config.rs`. Tests: `tests/preflight_scout.rs` (12), `tests/helpers.rs` (26), `tests/config.rs` (16), `tests/session.rs` (the preflight block tests, re-pinned to `## Scouting record`). Amber: the decision matrix is pinned per signal.

State: **COMPLETE** — implementation, tests and the gate below; ticked 2026-09-13.

### Line 2593 — no-progress, the verified checkpoint, salvage

Contract: the same calls, the same failure and an unchanged tree twice in a row produce one notice at the head of the next feedback, counted; the checkpoint status moves Verified → Disturbed on a later mutation; a task cut off by the cell limit, a poisoned runtime or a provider failure salvages its capsule into the result. Production: `progress.rs`, `session.rs::{TaskState::observe, TaskState::salvage}`. Tests: `tests/no_progress.rs` (4), `tests/evidence_gate.rs::an_identical_failing_cell_repeated_is_noticed_once_and_counted`. Green.

State: **COMPLETE** — implementation, tests and the gate below; ticked 2026-09-13.

### Line 2594 — the interface ablation runner

Contract: `pane ruler run --pane-interface hybrid,cells,tools` runs matched arms with `--interface` and `--output-format json`, parses each arm's telemetry, and reports interface regret per task and dimension with an explicitly assumed credit ratio; the Terminal-Bench adapter exposes `interface`. Production: `ruler/interface.rs`, `ruler/{attempt,cli,model,report,score}.rs`, `pane-benchmarks/pane_tb/{agent,metrics,runner}.py`. Tests: `tests/interface_regret.rs` (5), `tests/ruler_run.rs` (30), `tests/ruler.rs` (9), `pane-benchmarks/tests` (10). State: the runner is built; **no campaign has run** — the line stays open until the matched trials exist.

State: **PARTIAL** — the runner, the arms, the regret report and the adapter option are implemented and tested; the measured campaign has not run (no model-backed benchmark in the implementing session, by instruction). **Box deliberately unchecked** until the campaign in the roadmap's *The next model-backed ablation* fills the register's measured columns.
