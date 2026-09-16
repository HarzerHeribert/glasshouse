# Phase 69 — Pane: request modes and the decision model's helpers

Map lines 2637–2646, recorded 2026-09-16 (night) from the user's rulings (`design-decisions.md`, *Request modes, and what the decision model may help with*).

## Non-negotiables

- The decision model proposes; the sandbox profile and the person decide. A mode narrows, never widens; nothing the profile denies is ever allowed by a mode or by an answer.
- Every decision is fail-open: no model, an error or a timeout leaves the session exactly as it is today.
- Certainty about a remote environment comes from the far side's account and Pane's command gate, never from the model.
- No threshold becomes a default before the off/shadow/on measurement (2646) is recorded here.

## Gate

Targeted: the changed files' own targets plus the worker's quoted tests, on the merged tree; Red packages (2637, 2638, 2640) owe the full `cargo test -p pane` and a mutation per decision. The twelve-cell GitHub sweep trails on push.

## Shadow calibration inherited from Phase 66 (2026-09-16)

Five real tasks over Claude Max (`claude-sonnet-4-6`): intents `read_only` 1.00/1.00 and `modify` 1.00/0.98 in 575–791 ms; completion noul 0.94 (done), 0.65 (ambiguous), 0.10–0.22 (answer-only, empty diff — the defect 2641/2642's packet fixes by asking over the answer). `would_hold` 0 in all five.

## Line 2637 — `explore` as a request mode

**State: PARTIAL** (2026-09-17, `GH-PANE-EXPLORE-MODE`, Opus high, Red; report `.agent-runtime/report-pane-explore-mode.md`; integrated by the primary with a three-way apply over the approval-hint commit, the worker's `tui_live` patch and the `/mode` help text applied at integration). The enforcement is built; the line stays open on two producers named below.

**Contract (as built).** Given a session in `explore`, when a cell, a direct `/tool` frame or a native tool frame invokes `write`/`edit` outside the writable globs, or `bash` with anything but a read-only command (a fixed list plus configured patterns; redirects, process substitution, `VAR=` prefixes, quoted or pathed names, `sudo`/`sh -c` refused), Pane refuses it with a rule starting `mode explore:` and executes nothing, while reads and read-only commands run exactly as in `execute`; the profile decides first (never-grantable, deny, allow) and the mode only narrows what the profile admitted; a narrowed profile admits no MCP tool; `/mode execute` lifts the narrowing from the next request; the OS layer is rendered from the unnarrowed profile (the appliers probe `check`, the mode lives in `check_request`).

**Production.** `sandbox/modes.rs :: {RequestMode, ModeOverlay, Narrowing::{write_refusal, command_refusal}}`, `sandbox/profile.rs :: Profile::{narrowed_to, check_request, admits_command, admits_mcp_tool}`, `tools/invoke.rs :: check_arguments`, `session.rs :: run_task` (per-request narrowed profile clone, `--mode`), `session/controls.rs` (`/mode execute|explore|plan`), `prompt/mod.rs :: request_mode_line`.

**Tests.** `tests/sandbox_profile.rs::{explore_refuses_a_write_outside_its_globs_and_names_the_mode, explore_admits_a_write_under_a_configured_documentation_glob, explore_runs_read_only_commands_and_refuses_every_writer_by_mode, reads_and_profile_refusals_are_unchanged_in_every_mode, a_configured_command_pattern_is_read_only_in_explore, a_writable_glob_outside_the_root_makes_nothing_writable}`, `tests/sandbox_apply.rs::a_request_mode_leaves_the_os_sandbox_exactly_as_the_profile_renders_it`, `tests/request_modes.rs::{explore_refuses_a_write_the_model_attempts_despite_the_prompt, explore_bash_runs_read_only_commands_and_refuses_writers, a_direct_tool_frame_is_refused_by_the_same_rule, slash_mode_execute_restores_writes_from_the_next_request}`, `tests/tui_live.rs::shift_tab_cycles_through_explore_into_a_plan_mode_that_reads`.

| Decision | Mutation | Killing test | Result |
|---|---|---|---|
| A write outside the globs is refused | `modes.rs` glob match `→ \|\| true` | `sandbox_profile::explore_refuses_a_write_outside_its_globs_and_names_the_mode` | KILLED (`sandbox_profile.rs:111` expected a refusal, got a grant) |
| A configured doc glob is writable | same match `→ && false` | `sandbox_profile::explore_admits_a_write_under_a_configured_documentation_glob` | KILLED (`:2176`) |
| Only listed commands run | `READ_ONLY_COMMANDS.contains → \|\| true` | `sandbox_profile::explore_runs_read_only_commands_and_refuses_every_writer_by_mode` | KILLED (`:111`; `a_configured_command_pattern_is_read_only_in_explore` too) |
| `/mode execute` lifts the narrowing | `controls.rs` keep the old mode on Execute | `request_modes::slash_mode_execute_restores_writes_from_the_next_request` | KILLED (`request_modes.rs:226`) |
| The task runs under the narrowed profile | `session.rs narrowed_to(session.mode) → narrowed_to(Execute)` | `request_modes::explore_refuses_a_write_the_model_attempts_despite_the_prompt` | KILLED (`:163`) |
| Direct frames pass the same check | `invoke.rs check_request → check` | `request_modes::a_direct_tool_frame_is_refused_by_the_same_rule` | KILLED (`:268`) |

**Gates.** `cargo test -p pane --no-fail-fast` in the worktree: 93 targets, 1465 passed, 1 failed (`tui_live`, the superseded plan contract, patched at integration); targeted blast radius exit 0 in the worktree and on the merged tree (`request_modes` 6, `sandbox_apply` 28, `sandbox_profile` 46, `session` 105).

**Open, and why.** (1) `[modes.explore] writable = […]` / `commands = […]` have no producer: `settings/registry.rs` refuses unknown keys and was another package's file this round — only `ModeOverlay::default()` reaches production. (2) The default writable glob `.pane/scratch/**` is refused by the profile's `.pane/**` never rule, so `explore` can write nothing today. Both go to `GH-PANE-SCRATCH-AND-PLAN-FILE` (the never-rule carve-out, Red) and `GH-PANE-MODES-WIRED` (the config keys, with 2639).

**Limits.** In-process admission, not OS confinement: a read-only child still runs under the session's OS grant, and git config-driven execution (`core.fsmonitor`, textconv, pagers) is not seen by the word scan. `command_segments` does not track quotes (`grep "a>b"` is refused — the refusing direction). Acceptance commands and checkers run under the session profile. The Windows refusal-by-name is verified only by the `pane (windows-latest)` cell.

## Line 2638 — `plan` reads

**State: PARTIAL** — same package. Plan mode no longer short-circuits: its cells run under the plan narrowing (reads and read-only commands run, every write refused with `mode plan:`), so "executes no change" holds by the profile rather than by not running anything. Production: `session.rs :: run_task` (short-circuit removed), `modes.rs :: Narrowing::compile` (`Plan` → no writable globs). Tests: `request_modes::plan_reads_and_refuses_a_write`, `sandbox_profile::plan_reads_and_refuses_every_write_even_under_a_documentation_glob`. Mutation: `Plan => Vec::new()` → `return (None, diagnostics)` KILLED by `request_modes::plan_reads_and_refuses_a_write` (`:199`, PLAN.md exists). **Open on "and write the plan file":** no plan file exists in current source (plan's output was the assistant reply in the rollout); the location is ruled in `GH-PANE-SCRATCH-AND-PLAN-FILE` (`.pane/scratch/plan.md`, the one write plan may make).

## Line 2639 — the mode proposed from the intent

**State: OPEN** — packet after 2637 lands.

## Line 2640 — remote commands gated in `explore`

**State: OPEN** — packet after 2637 lands (Red).

## Line 2641 — diff-hygiene questions at the completion gate

**State: COMPLETE** (2026-09-17, `GH-PANE-COMPLETION-JUDGE`, Sonnet high, Amber; report `.agent-runtime/report-pane-completion-judge.md`; integrated by the primary with a three-way apply over the approval-hint commit — `decision-model.md` §6 collided, both kept).

**Contract.** Given `[decisions] model` set and a non-empty diff, when the model claims completion, Pane asks five `noul`s in the same request as the satisfaction question — `has_tests`, `out_of_scope`, `debug_leftovers`, `deletes_tests`, `changes_signature` — and a decisive answer (`has_tests` at or below `hygiene_no_below` 0.10; the other four at or above `hygiene_yes_above` 0.90) becomes one `HygieneIssue` finding held once, in between the checker as today; an in-between answer or `mode = shadow` adds nothing and the raw answers are recorded. An empty diff or a `read_only` intent asks over `{request, answer}` instead (the Phase 66 shadow-calibration defect), and hygiene is not asked in that state.

**Production.** `decide.rs :: {CompletionState, hygiene_questions, completion_satisfied}`, `session/task.rs :: TaskState::gate` (the hygiene block; `use_answer_state`), `completion.rs :: FindingKind::HygieneIssue`, `config.rs` (`hygiene_no_below`, `hygiene_yes_above`), `settings/registry.rs`.

**Tests.** `tests/decisions.rs::{a_confident_out_of_scope_yes_is_one_finding_held_once_with_the_reason, an_undecided_has_tests_answer_adds_no_finding, shadow_records_the_five_hygiene_nouls_and_adds_no_finding, an_answer_only_task_with_an_empty_diff_is_asked_over_the_answer_state, a_confident_no_over_the_answer_state_is_one_finding_held_once}`, `decide::tests::{hygiene_questions_cover_the_five_keys, the_answer_state_question_asks_about_the_answer_not_a_diff}`, `config::tests::the_hygiene_and_judge_thresholds_default_and_are_refused_out_of_range`.

| Decision | Mutation | Killing test | Result |
|---|---|---|---|
| A hygiene answer must be decisive | `task.rs`: `out_of_scope >= hygiene_yes_above` → `>= 0.0` | `decisions::an_undecided_has_tests_answer_adds_no_finding` | KILLED ("an undecided hygiene answer never holds") |
| An empty diff asks over the answer | `task.rs`: `use_answer_state = diff_is_empty \|\| is_read_only_intent` → `false` (bindings kept) | `decisions::an_answer_only_task_with_an_empty_diff_is_asked_over_the_answer_state` | KILLED (the fake's request carried `diff`, not `answer`) |

**Gates.** `cargo test -p pane --test decisions`: 30 passed; `--test settings_store` 29; `--test evidence_gate` 9; `--test final_state_contract` 6; `--test config` 17; `--lib` 396; targeted blast radius exit 0 in the worktree and on the merged tree.

**Limits.** Thresholds are the completion pair's defaults, not a calibration (2646). A `read_only` task that nevertheless produces a diff is asked over the answer, so that diff's hygiene is never asked. No live endpoint was called by the tests.

## Line 2642 — judge items decided when confident

**State: COMPLETE** — same package. **Contract.** Given acceptance items still `judge` after the mechanical pass, when the model claims completion, Pane asks one `noul` per item (`judge_<n>`, the item text and its evidence) in the same request; at or above `judge_yes_above` 0.90 the item is `Met` with the decision in its evidence, at or below `judge_no_below` 0.10 it is one `JudgeNotSatisfied` finding naming the item, held once; in between it stays `judge` and the fresh checker runs as today. The checker is spared only when the satisfaction answer is a confident yes, no other finding exists and every judge item is decided. `shadow` classifies (`judged: {yes, no, undecided}`) and changes nothing.

**Production.** `session/task.rs :: TaskState::gate` (the judge block; `output::acceptance` emitted after it), `decide.rs :: {judge_question, judge_key}`, `completion.rs :: FindingKind::JudgeNotSatisfied`, `config.rs` (`judge_yes_above`, `judge_no_below`). **Tests.** `tests/decisions.rs::{a_judge_item_answered_yes_is_satisfied_without_the_checker, a_judge_item_answered_no_is_a_finding_held_once_then_verifies_unverified_with_the_item_named, a_judge_item_answered_undecided_runs_the_checker_as_today, shadow_records_judged_and_changes_nothing}`, `decide::tests::a_judge_question_embeds_the_item_and_its_evidence`.

| Decision | Mutation | Killing test | Result |
|---|---|---|---|
| A yes must be confident | `task.rs`: `noul >= judge_yes_above` → `>= 0.0` | `decisions::a_judge_item_answered_undecided_runs_the_checker_as_today` | KILLED (the checker never ran; the 0.5 answer was treated as yes) |

**Limits.** A judge item's evidence sent to the model is empty today (`acceptance::evaluate` has none for `judge` items). The checker, when it runs, still sees every judge item's text including the decided ones (redundant, not contradictory). `judged` is a raw classification in every mode, not a `would_*` count.

## Line 2643 — a drift question before an effectful cell

**State: OPEN** — packet after 2641/2642 land (shares `decide.rs`).

## Line 2644 — the Scout's files ranked by relevance

**State: OPEN** — `GH-PANE-HELPERS-JUDGED` (packet written 00:15).

## Line 2645 — a helper's result checked

**State: OPEN** — same package.

## Line 2646 — off / shadow / on measured

**State: OPEN** — the orchestrator's, last: `pane ruler run` over one request set × three modes.
