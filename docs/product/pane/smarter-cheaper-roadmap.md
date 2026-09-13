# Pane smarter-and-cheaper execution roadmap

Status: product direction and measured gap register, 2026-09-13; the register's
rows were implemented the same day — see *Implementation status* at the end,
which is the truthful state of each row, and `docs/product/evidence/phase-65.md`
for the evidence per map line (map Phase 65, lines 2580–2594).

This record turns the Terminal-Bench pilot, the Tool ABI design, and the useful
parts of GVS5H's ledger-based orchestration into one product plan. It does not
close capability-map boxes. Every row still needs implementation and evidence
through the normal ledger.

## Product thesis

Pane is not valuable because it renames ordinary coding-agent tools or because
it makes the model learn JavaScript. It is valuable when a model can use the
tool vocabulary it already knows while Pane supplies a better execution
mechanism underneath:

```text
provider-familiar intent
    -> canonical capability
    -> dependable deterministic execution
    -> exact artifact and provenance
    -> deduplication, bounding or cheap semantic reduction
    -> only decision-relevant feedback reaches the expensive parent
```

The practical analogy is an ordinary power tool versus a better professional
one: the operator makes the familiar motion, but the better tool removes more
friction and fatigue over a full day. Pane wins when it preserves competitor
correctness while lowering weighted cost, expensive-parent attention and
avoidable recovery work.

Raw token count alone is not the objective. Cheap-model tokens can be an
excellent trade when they remove expensive parent turns or prevent a failed
task.

## What the 91 `execute_cell` calls mean

The accepted 2026-09-12 Terminal-Bench campaign invoked Pane without an
explicit `--interface` option. Pane therefore used its default `hybrid`
interface, which exposed provider-familiar direct tools and `execute_cell`.

The raw provider conversation contains 91 assistant `tool_use` blocks, all
named `execute_cell`, and no direct `Read`, `Edit`, `Bash`, `Grep` or related
provider tool call. This observation is made before lowering, so it is not the
result of direct tools being represented as Cell IR internally. The model
really selected `execute_cell` for every acting turn.

That fact is not itself a defect. The selected tasks required dependent
compilation, inspection, branching and verification, for which composed cells
are an intended interface. A model preferring cells can be evidence that the
composition surface is useful.

It becomes a concern only when one of these is true:

- the model selected cells because Pane's prompt biased it away from a more
  familiar or cheaper direct call;
- a simple single-intent operation paid cell-language or result-envelope tax;
- the chosen cell exposed Pane-specific syntax, permission or mutation
  failures that a direct familiar call would have avoided;
- composition did not reduce parent turns or context enough to repay that tax;
- the direct translation path remains unexercised and therefore unproven in
  real work.

The pilot cannot decide among those explanations. Direct calls and authored
cells converge on the same kernel by design, but final machine telemetry counts
only cells and primitive tools. The stream event also omitted the frame origin.
The next telemetry revision must separately report:

- provider-native direct tool calls;
- model-authored cells;
- lowered direct-tool frames;
- primitive operations per frame and per parent request;
- single-intent authored cells that could have been direct calls;
- failures and repair turns by invocation origin;
- repeated observation bytes and parent tokens by invocation origin.

Do not force a quota of direct calls. Measure **interface regret** instead: for
the same task and model, did the interface selected in hybrid mode cost more,
fail more, or expose more context than the best supported alternative?

## Terminal-Bench failure audit

The pilot's 13 failed cells were not 13 ordinary bad shell commands. Their
causes were:

| Cause | Cells | Interpretation |
|---|---:|---|
| `gdb` denied before execution | 3 | Policy/capability mismatch |
| `/build` search denied before execution | 3 | Benchmark omitted a task-declared read root |
| TypeScript `as const` rejected | 2 | Advertised TypeScript/runtime mismatch |
| Edit rejected after Pane's own earlier mutation | 2 | Mutation composition/version-chaining gap |
| Write beneath reserved `.pane` denied | 1 | Correct boundary, insufficient steering/scratch affordance |
| Deliberately crashing target surfaced as killed Bash | 1 | Expected diagnostic action counted as a tool failure |
| Headless Vim command exceeded 60 seconds | 1 | One genuine unintended execution failure |

Thus six failures were policy/profile mismatches, five were Cell/Tool ABI
friction, one was the crash the debugging task explicitly asked the agent to
reproduce, and one was a genuine hung command. A cell can also complete useful
earlier operations before a later tool throws; labelling the whole frame
`failed` is truthful about control flow but not equivalent to the per-command
failure display of Claude Code or Codex.

Telemetry must retain the aggregate while splitting it into syntax, denial,
mutation conflict, expected diagnostic non-zero/signal, timeout, transient
infrastructure and genuine command failure. It must also attribute the next
parent request and its usage when that request is actually a repair.

## Current strengths to preserve

These mechanisms are aligned with the product thesis and should be hardened,
not replaced:

- one canonical capability kernel for direct calls and cells;
- provider-dialect façades that lower rather than implement separate tools;
- exact artifact handles and provenance;
- bounded exact versus derived evidence classes;
- deterministic-first semantic command lifting;
- cheap, bounded and separately metered Little Helpers;
- durable event and rollout ledgers;
- sandbox admission, credential stripping and cancellation;
- context compaction and resumable sessions.

## Build, debug and redesign register

`Debug` means the intended feature exists but behaved incorrectly. `Redesign`
means its present semantics work against the product thesis. `Build` means the
capability is materially missing. `Prove` means implementation exists but no
representative measurement supports the claim yet.

| Priority | Feature | Work | Current evidence or problem | Required outcome |
|---|---|---|---|---|
| P0 | Interface-origin telemetry | Build | The final result cannot distinguish authored cells from lowered direct calls | Machine output records provider selection, frame origin, primitive operations and failures by origin |
| P0 | Hybrid interface choice | Prove | Hybrid exposed both paths, but the model selected 91 authored cells and zero direct calls | Matched tools/cells/hybrid runs establish whether model choice reduces turns, failures and weighted cost; no arbitrary direct-call quota |
| P0 | Familiar Tool ABI correctness | Debug | Real work did not exercise direct provider façades | OpenAI- and Anthropic-shaped calls pass representative read/search/edit/shell tasks through the shared kernel |
| P0 | TypeScript Cell contract | Debug | `as const` failed twice although Pane specifies model-authored TypeScript | Every advertised erasable construct runs; unsupported constructs receive an accurate diagnostic before execution |
| P0 | Capability/environment manifest | Build | The model repeatedly selected task-advertised `/build` and `gdb` capabilities that Pane denied | Before acting, the parent sees effective readable/writable roots, available commands, reserved paths and unavailable capabilities |
| P0 | Benchmark container profile | Debug | Outer-container mode still conflicted with task-required reads and debugging | The adapter grants declared in-container capabilities while normal host security remains unchanged |
| P0 | Tool outcome semantics | Redesign | One late denial throws the whole composed cell after useful earlier work | Each call has a typed outcome; a frame aborts only when safe continuation is impossible, while partial effects remain explicit |
| P0 | Transactional mutation composition | Redesign | Two edits were stale because Pane itself changed the file first | Multi-hunk and sequential same-file edits compile into one checked atomic mutation; only external races are stale |
| P0 | Observation delta | Redesign | 678 handle rows were emitted over 91 cell results, mean 7.45 and maximum 26 | Return new/changed handles, dependencies and unresolved failures rather than the full repeated inventory |
| P0 | Handle lifecycle | Redesign | Resolved intermediate handles accumulate in the active result surface | Retire them from active context while preserving exact ledger access and explicit pinning |
| P0 | Semantic read/search lifting | Debug and prove | The recognizer exists, but the pilot did not isolate its context or round-trip effect | Equivalent familiar shell/direct/cell reads converge, deduplicate and return the smallest exact sufficient observation |
| P0 | Adaptive result reduction | Debug and build | Only one pushed reducer ran in 12 trials, on a 403-line result | Large noisy observations are reduced when the expected parent-attention saving exceeds cheap-helper cost; raw evidence stays addressable |
| P0 | Structured task capsule | Build | The active conversation retains repeated handles and resolved diagnostics | Maintain a bounded goal, current state, verified facts, evidence references, unresolved risks and one next action |
| P0 | Evidence-gated completion | Build | Three incorrect final filesystem states were reported as complete | Fresh contradictory evidence overrides `done`; completion requires the task's applicable acceptance contract |
| P0 | Final-state contract checker | Build | Two stray binaries and one misplaced gcov tree caused all genuine failures | Check required paths, forbidden artifacts, directory contents, tests, installation state and working-tree state before completion |
| P1 | Fresh independent checker | Build | A checker inheriting the working narrative can inherit its mistaken assumptions | A cheap fresh context sees the original request, current diff/state and exact evidence, but not the parent's rationale |
| P1 | Preflight Helper | Redesign | Some preflights tried to complete an executable task and merely reported that they could not run commands | Preflight scouts constraints, files, tests, capabilities and risks; it never impersonates the acting parent |
| P1 | No-progress guard | Build | Denied or ineffective actions can recur without changing strategy | Detect repeated intent, denial, unchanged diff and unchanged failing evidence, then choose another method or escalate clearly |
| P1 | Verified checkpoint | Build | Later work can disturb a previously correct intermediate state | Preserve the newest verified state and prevent an unverified finalizer from silently replacing it |
| P1 | Cut-off salvage | Build | A timeout or output limit can discard useful partial reasoning | A cheap bounded summarizer writes established facts and unresolved work into the task capsule without claiming completion |
| P1 | Adaptive orchestration | Redesign | Every benchmark task paid for preflight whether or not it helped | Direct execution remains the fast path; helpers and workers activate from complexity, uncertainty, failure or verification signals |
| P0 | Recovery-cost attribution | Build | Failure counts do not reveal the Sol usage actually caused by recovery | Attribute parent requests, tokens and wall time to implementation, exploration, verification and repair causes |
| P0 | Interface ablation runner | Build | One hybrid campaign cannot prove the Tool ABI's advantage | Run the same model, task, limits and environment in tools-only, cells-only and hybrid modes |
| P0 | Request-derived acceptance list | Build | The full-suite run (2026-09-13) passed eight completions through the gate and the checker that the official verifier then failed: the gate reads a diff, never what the request asked for | Before the first turn, turn the request into verifiable items (files, commands, outputs, judged sentences); show them; decide each against the tree or a command at completion; hold an unmet item once |
| P0 | Stall detection instead of a cell cap | Redesign | A 40-cell cap ended a task with 1.1 M tokens of real progress; other harnesses run to the task timeout | A run of cells that changes nothing gets a notice and a count, never a stop; the cap becomes a high backstop |

## GVS5H concepts worth adopting

[GVS5H](https://github.com/slee-persis/GVS5H) studies a deliberately simple
ledger-based manager/worker scaffold. Its reported gains are research evidence,
not a Pane result: it evaluates LiveCodeBench algorithm problems, adds
substantial test-time compute, and reports cases where orchestration is flat or
harmful. The transferable concepts are nevertheless well aligned with Pane:

| GVS5H mechanism | Pane-native form |
|---|---|
| Shared `task.md`, `plan.md`, `notes.md`, current solution | Typed task capsule backed by the existing ledger and artifact handles |
| Fresh model context for each worker | Narrow Helper/subagent context containing the capsule and one task, not the transcript |
| Manager curates exactly one next task | Cheap curator updates one bounded next-action record after meaningful state changes |
| Public test verdict overrides a worker's `done` | Fresh Pane evidence overrides unsupported completion claims |
| Repeated-task no-progress guard | Compare canonical intent, diff hash and evidence outcome before retrying |
| Cut-off attempt summarizer | Salvage only established facts and unfinished work into the capsule |
| Finalizer skipped when a usable solution is already accepted | Never let a redundant final pass overwrite a verified state |
| Proposed fresh-perspective worker | Independent task-plus-diff checker without inherited notes or rationale |

Do not copy the Markdown-file protocol or make every task a manager loop. Pane
already has typed state, exact evidence, deterministic tools, Helpers and
subagents. Adopt the control principles inside those mechanisms. The intended
flow is:

```text
familiar action
    -> canonical execution
    -> exact evidence
    -> compact task-state update
    -> cheap independent verification when warranted
    -> expensive parent receives only the next real decision
```

## Economic definition of success

Use observed subscription credit debits when the provider exposes them. When
it does not, report a declared weighted model-equivalent estimate separately
from raw tokens:

```text
weighted spend = Sol spend + rL * Luna spend + rT * Terra spend
quality-adjusted spend = weighted spend / verified successful trials
```

`rL` and `rT` are the observed or explicitly assumed credit ratios relative to
Sol. Cache-read, ordinary input and output classes remain separate because they
need not have the same price. Never present an assumed ratio as a provider bill.

The product scorecard is:

| Dimension | Success condition |
|---|---|
| Correctness | Predeclared non-inferiority comparison on matched tasks with enough trials; no aggregate-to-subset substitution |
| Expensive-parent attention | Lower Sol requests, new input and recovery usage per verified pass |
| Weighted economics | Lower declared weighted spend per verified pass, even when cheap-helper raw tokens rise |
| Context hygiene | Fewer repeated observation bytes and no resolved diagnostic replay |
| Tool dependability | Zero predictable capability denials and zero runtime-caused mutation conflicts after preflight |
| Completion integrity | No false completion on the three known final-state failure fixtures |
| Wall time | Competitive task latency after verification, reported independently of summed worker time |

An initial engineering target is a 25--30% reduction in Sol usage per verified
pass without reducing correctness. It is a target for the next matched
experiment, not a present product claim.

## Work and measurement order

1. Repair TypeScript, capability-profile, direct-facade and mutation semantics.
2. Add invocation-origin and recovery-cost telemetry before optimizing.
3. Replace repeated handle inventories with delta observations and lifecycle.
4. Add the task capsule, evidence-gated completion and final-state checker.
5. Add fresh cheap checking, no-progress handling and adaptive orchestration.
6. Replay the known polyglot and gcov failures as targeted canaries.
7. Run matched tools-only, cells-only and hybrid trials.
8. Start a larger competitive benchmark only after the canaries and interface
   measurement are sound.

This order preserves the core Tool ABI thesis: the parent may choose familiar
direct tools or composed cells, but Pane must make either choice smarter than
the raw provider interaction and must prove the saving in expensive attention.

## Implementation status — 2026-09-13

Each row of the register above, with what now exists, the tests that pin it,
and what is still genuinely open. **No model-backed campaign has run on this
code**; every "measured" figure in the register above is still the pilot's, and
every row marked *Complete* here is complete as an implementation with
deterministic evidence, not as a measured product claim. The next campaign is
specified under *The next model-backed ablation*.

| Row | State | What exists now | Evidence | Still open |
|---|---|---|---|---|
| Interface-origin telemetry | Complete | `telemetry.interface` (provider-selected `execute_cell` vs direct calls by name), `frames.by_origin`, `operations_per_frame_mean`, `operations_per_parent_request_mean`, `single_intent_cells`, `failures.by_kind`/`by_origin`, `lifting`, `observation`, `reductions`, `recovery.by_cause`, `completion`, `progress`, `capsule` in `--output-format json`/`stream-json` | `tests/telemetry_taxonomy.rs` (23), `tests/session_output.rs::{provider_selected_interface_is_counted_and_every_v1_field_stays, direct_tool_and_authored_frames_are_counted_by_origin, a_request_after_a_thrown_cell_is_charged_to_repair}` | The single-intent detector is a documented heuristic (false negatives only) |
| Hybrid interface choice | Measured, and hybrid lost | Matched arms on 2026-09-13 (below): cells-only passed 12/12 with 114 parent requests, hybrid 11/12 with 145, tools-only 10/12 with 188. In hybrid the model chose `execute_cell` for 125 of 136 frames and `shell` for the rest; direct calls bought nothing it could not do in a cell and the tools-only arm hit the frame limit twice on the debugging task at about one operation per frame. The predeclared test (hybrid non-inferior on passes and lower parent requests per pass) fails against cells | `results/tb2-pane-ablation-20260913/` in pane-benchmarks; `TERMINAL-BENCH-RESULTS.md` *Interface ablation* | Decided 2026-09-13: `cells` is the default (`abi::Interface::default`, `tests/prompt_interface.rs::the_default_interface_is_cells_only`); `hybrid` and `tools` stay by flag and no quota is forced either way |
| Familiar Tool ABI correctness | Complete | Multi-call direct frames isolate each call; `Edit`/`apply_patch` gain `old_strings`/`new_strings`; lowered `Bash` keeps `reduced`/`reduction_error` | `tests/direct_frame_outcomes.rs` (5), `tests/abi.rs`, `abi::tests::a_bounded_process_result_keeps_its_derived_view_beside_the_exact_bytes` | Real-work exercise of the façades is the campaign's |
| TypeScript Cell contract | Complete | `FreeNames` never reads a type position; `cell::ERASABLE_CONSTRUCTS` (24) all compile, keep every column and run; `NOT_ERASABLE_CONSTRUCTS` (6) are refused by name with an alternative before execution | `tests/typescript_contract.rs` (4, incl. `as_const_in_an_object_literal_compiles_and_runs`), `runtime::cell::tests` | — |
| Capability/environment manifest | Complete | `manifest::Manifest::collect` from the compiled profile plus a 24-executable `PATH` probe, rendered as `## Environment` in the system block (readable/writable roots, reserved paths, deny patterns, command policy, never-admitted names, absent executables, container mode, unavailable capabilities) | `tests/container_profile.rs`, `manifest::tests`, `tests/session.rs::the_system_block_is_render_systems_own_bytes` | — |
| Benchmark container profile | Complete | Under `--yolo --dangerously-bypass-os-sandbox` (Linux only, CLI only): reads span the container except credential stores and deny patterns; debuggers are admitted; sandbox launchers stay refused; writes unchanged; ordinary sessions byte-identical | `tests/container_profile.rs` (12) | The adapter still grants `/usr/local/bin` by name for one task |
| Tool outcome semantics | Complete | Direct frames: a denial or throw in one call stops no sibling and its binding is freed; each `tool_result` carries its own recorded message. Authored cells: JavaScript semantics unchanged, plus an `## Effects` section naming the effectful calls that completed before a throw | `tests/direct_frame_outcomes.rs`, `session::tests::partial_effects_name_only_completed_effectful_calls_of_a_thrown_cell` | — |
| Transactional mutation composition | Complete | A successful `edit`/`write` registers its resulting version as visible; `edit({olds, replacements})` applies N hunks atomically or not at all; only an external change is stale | `tests/mutation_composition.rs` (9), `tests/exact_edit.rs` | — |
| Observation delta | Complete | `handles::render_table_delta`: entries changed this cell (or pinned by `keep`) in full, the rest one line each; `handles()` and the rollout keep the inventory; `ObservationStats` per turn | `tests/observation_delta.rs` (8), `tests/handles.rs` | — |
| Handle lifecycle | Complete | `keep` pins; redeclaration unpins; unchanged handles render as `(unchanged since cell N)`; nothing is evicted | same | Explicit `unpin` host function not exposed |
| Semantic read/search lifting | Complete (dedup), proven deterministically | Repeated pure observations carry `repeat_of` and a header suffix; lifting counted by family | `tests/observation_dedup.rs` (7), `tests/lifting.rs` | Context/round-trip effect is the campaign's |
| Adaptive result reduction | Complete | Threshold `[helpers] reduce_above_tokens` (default 2,048), attempted only when the expected parent saving is positive; `ReductionStats` in telemetry; exact output always complete | `tests/observation_dedup.rs` (reduction section) | Measured saving |
| Structured task capsule | Complete | `runtime::capsule::Capsule` maintained from the trajectory; `## Task` block in live feedback when it changes; on every `CellView` and in the result telemetry | `tests/task_capsule.rs` (4); `tests/evidence_gate.rs::the_task_capsule_reaches_the_feedback_and_the_result` | — |
| Evidence-gated completion | Complete | A terminal return **or a prose completion** is held once when findings exist; the same findings a second time finish with `completion.verified = false`; `completion.deferred` counts the holds. The prose half landed 2026-09-13 after the first hybrid Terminal-Bench trial showed gpt-5.6-sol ending every task with a structured return and then prose, which a gate on returns alone never saw | `session::TaskState::gate`; `tests/final_state_contract.rs`; `tests/evidence_gate.rs` (7 binary-level canaries through the built `pane` against a loopback provider: stray binary held on a return and on a prose completion, misplaced coverage, declared-but-unrun check, a clean prose completion, planted repeat, capsule) | Live measurement |
| Final-state contract checker | Complete | `completion::check`: unexpected compiled artifact beside a deliverable, coverage tree outside the source tree, stale verification, no verification only where the project declares checks or a `[contract]` (the unconditional form held two ordinary fixtures for no evidence — integration ruling), plus `[contract]` in `checks.toml` | `tests/final_state_contract.rs` (6, the three pilot fixtures) | — |
| Fresh independent checker | Complete | `[helpers] completion_check = true` runs CHECKER once on the original request, the task diff, the capsule's facts and the findings only | `completion::fresh_checker_evidence` tests | Live measurement |
| Preflight Helper | Complete | The Scout receives a scouting brief and answers Constraints/Files/Tests/Capabilities/Risks; never the task | `tests/preflight_scout.rs` (12), `tests/helpers.rs` | — |
| No-progress guard | Complete | `progress::Guard`: same calls, same failure, unchanged tree twice in a row → one notice; a succeeding frame resets the guard, so a repeated successful read is never a notice | `tests/no_progress.rs` (4); `tests/evidence_gate.rs::an_identical_failing_cell_repeated_is_noticed_once_and_counted` | — |
| Verified checkpoint | Complete | `progress::Checkpoints`; the capsule shows `UnverifiedSince`; the gate flags stale verification | same | — |
| Cut-off salvage | Complete | `Capsule::salvage` on the cell limit, a poisoned runtime or a provider failure; emitted as `telemetry.capsule` | `tests/task_capsule.rs` | Optional cheap summariser not added |
| Adaptive orchestration | Complete | `[helpers] preflight_scope = "auto"` (default) scouts only on uncertainty signals | `tests/preflight_scout.rs` | — |
| Recovery-cost attribution | Complete | `recovery.by_cause.{implementation,exploration,verification,repair}` with requests, token classes and wall time | `tests/session_output.rs::a_request_after_a_thrown_cell_is_charged_to_repair` | — |
| Interface ablation runner | Complete, campaign run | `pane ruler run --pane-interface hybrid,cells,tools --credit-ratio luna=…`, `pane-result.json` per arm, regret table; the Terminal-Bench adapter's `interface` option and `scripts/compare_interfaces.py` over the arms' summaries | `tests/interface_regret.rs`, `tests/ruler_run.rs`, `pane-benchmarks/tests`; the 2026-09-13 campaign | — |
| Request-derived acceptance list | Complete (2026-09-14) | `acceptance.rs`: the `accept` helper (toolless, one-shot, roster entry, `[helpers] acceptance_list`, on by default with helpers) turns the request into at most eight items in five line forms; the list is shown to the model as `## Acceptance list`; at completion every file/run/output item is decided against the tree or a command through the one kernel, an unmet item is a final-state finding (held once, then `verified = false`), and judge items go to the fresh checker with the mechanical results | `acceptance::tests` (4), `tests/evidence_gate.rs::an_unmet_acceptance_item_holds_the_completion_with_what_was_observed`, `tests/config.rs` | The live measurement: the eight false completions of 2026-09-13 |
| Stall detection instead of a cell cap | Complete (2026-09-14) | `progress::Stall`: six cells in a row with no tree change, no new fact and no verification produce one notice at the head of the next feedback and `progress.stall_notices`; never a stop. `[limits] cells` default 120 as a backstop; the adapter's default follows | `progress::stall_tests`, `tests/evidence_gate.rs::six_cells_without_progress_get_one_stall_notice_and_the_task_continues` | Whether a notice changes the model's course is the campaign's to measure |

### The 2026-09-13 ablation, and the next one

Run 2026-09-13 exactly as specified above on the Linux artifact of commit
`5290644` (Terminal-Bench 2.0, four tasks, three attempts, concurrency two,
official timeouts, oracle 4/4 first; gpt-5.6-sol medium, gpt-5.6-luna helpers,
Scout preflight on `auto`, `completion_check = true`). The arms ran one after
another, 1 h 47 min of campaign wall time, 8.7 M known tokens on the
subscription route (twice the plan: the tools-only arm alone took 4.4 M).

| Arm | Verified passes | Parent requests | Known tokens (Sol / Luna) | Provider-selected calls | Frames failed |
|---|---|---|---|---|---|
| hybrid | 11/12 | 145 | 2,033,915 / 296,883 | 125 `execute_cell`, 11 `shell` | 13/136 |
| cells | **12/12** | **114** | **1,584,163** / 364,971 | 107 `execute_cell` | 6/107 |
| tools | 10/12 | 188 | 4,145,330 / 288,565 | 209 `shell`, 3 `apply_patch` | 5/178 |

Per task, mean parent requests per trial (passes): `custom-memory-heap-crash`
hybrid 18.3 (3/3), cells 16.0 (3/3), tools 36.0 (1/3, two trials at the
40-frame limit at about one operation per frame); `large-scale-text-editing`
10.0 / 4.0 / 7.0 (all 3/3); `polyglot-c-py` 5.7 / 6.7 / 8.0 (all 3/3);
`sqlite-with-gcov` 14.3 (2/3) / 11.3 (3/3) / 11.7 (3/3). Interface regret of
hybrid against the best arm: 1.15× the requests per pass on the debugging task,
2.5× on file processing, one pass behind on system administration, none on
software engineering. **The predeclared test fails: hybrid is not
non-inferior on passes and does not use fewer parent requests per pass than
cells-only.** The one hybrid miss was the model concluding the container had
no build toolchain and saying so — an honest report, not a false completion.

What else the telemetry says, the same for every arm: the delta handle table
suppressed 28–56 % of observation bytes per trial; no observation repeated
byte for byte; no single-intent cell was authored. Failures in 36 trials: 12
`bash` children killed by a signal (the debugging task's crashing target),
seven thrown cells of the syntax kind (four JavaScript syntax errors of the
model's own, two unbound names, one attempt to bind a local named `checks`
over the host function), one `edit` refused for an ambiguous match, three
`rg` calls at exit 127 because the official images have no ripgrep. None of
the pilot's five causes recurred: no denial, no `/build` or `gdb` refusal,
no TypeScript erasure refusal, no stale-version edit. The reducer ran 32
times on build and test output; the Scout ran on every trial (each request
carried at least one uncertainty signal); the no-progress guard never fired.
The completion gate saw only the eight trials that ended in a string
`return`, all verified with no finding; the other 28 ended in prose after a
structured return, which the artifact's gate did not cover — fixed the same
day (*Evidence-gated completion*, above) and not yet measured. One hybrid
trial was cut off by a provider `408` after its fix had landed and passed on
the salvaged state. Two small follow-ups the trials name: `rg` should fall
back to `grep` where ripgrep is absent, and a cell that binds a host name
should get the alternative in one line rather than a refusal.

The 2026-09-13 full-suite pass (`campaign.full-cells.json`, 89 tasks, one
attempt, cells-only, artifact of `37ac55b` then `58c23ee`) was paused at 37
finished trials — 18 verified; 16 of 28 on oracle-passing tasks; three
provider safety refusals on security tasks, which are not Pane's and leave
the scored denominator — once it had shown two harness defects (the
instruction-index budget, the adapter's positional task argument) and the
two product gaps above. Its raw artifacts stay under
`campaigns/tb2-pane-full-cells` unsealed. **The next campaign is a fresh
full-suite pass on the artifact that carries the acceptance list, the stall
notice and the 120-cell backstop**, with the same manifest shape, the
oracle first, and `scripts/full_suite_report.py` over the oracle-passing
tasks with provider refusals listed and excluded. Budget about 15 M known
tokens on the subscription route; the run alone is four to five hours at
concurrency two.