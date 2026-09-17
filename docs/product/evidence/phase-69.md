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

## Live probes over the Max subscription, 2026-09-17 01:05–01:15 (release build of 91a7eee7, `claude-sonnet-4-6`, `[decisions] mode = on`, `jev-latest`)

Five scripted `pane session --output-format json` runs against the scratch Python project used for the Phase 66 calibration. (1) `--mode explore`, a modify request: intent `modify` 1.00; the model, told the mode, made no write attempt (2 cells, 0 failed) and the tree stayed unchanged — the enforcement was not exercised here, the binary tests exercise it. (2) `--mode explore`, a read-only question: `read_only` 1.00, complexity `needs_exploration` 0.80; answer state, completion noul 0.17 (no finding at the 0.10 floor). (3) `--mode plan`: `plan written: .pane/scratch/plan.md (72 lines)`; the plan correctly reported the requested change already present. (4) execute, a repeat of an earlier task: answer state, the model reported it done — not a defect, a repeated probe. (5) execute, a new function plus a test, the model asked to plan first: `modify` 0.99; **drift asked 6, held 1** (step "Run the tests", answer 0.05, the cell ran on re-issue), hygiene `has_tests` 0.98 / `out_of_scope` 0.08 / `debug_leftovers` 0.06 / `deletes_tests` 0.05 / `changes_signature` 0.57 (undecided; a new function), completion noul 0.74 in the diff state, 8 cells (3 failed: two `cd src` denials under the profile, one command), 57 s wall. Decision latencies 580–690 ms. Whether the one drift hold was a true or false hold is the 2646 measurement's question; it cost one cell.

## Line 2637 — `explore` as a request mode

**State: COMPLETE** (enforcement 2026-09-17 by `GH-PANE-EXPLORE-MODE`, Opus high, Red, report `.agent-runtime/report-pane-explore-mode.md`, integrated three-way over the approval-hint commit with the worker's `tui_live` patch and the `/mode` help text applied at integration; the scratchpad by `GH-PANE-SCRATCH-AND-PLAN-FILE`; the configured globs and commands by `GH-PANE-MODES-WIRED` — the three parts below).

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

**Configured globs and commands** (2026-09-17, `GH-PANE-MODES-WIRED`): `[modes.explore] writable = [...]` and `commands = [...]` in `pane.toml` (`config.rs :: {ModesConfig, ModeExploreConfig}`, registered as `modes.explore.writable`/`commands`, `[modes]` a recognised table) build the session's one `ModeOverlay` at startup (`session.rs :: Session::overlay`, replacing the three `ModeOverlay::default()` sites). Tests: `request_modes::configured_explore_writable_and_commands_reach_the_overlay` (a `docs/**` write admitted in explore, `src/x.rs` refused; a configured `cargo metadata*` runs), `settings_store::mode_proposal_and_explore_overlay_keys_are_known_and_validated`. Mutation: the overlay built from defaults KILLED by `request_modes::configured_explore_writable_and_commands_reach_the_overlay` (`:635`). Limit: the configured command patterns are enforced but not named in the prompt's mode line.

**Scratchpad built** (2026-09-17, `GH-PANE-SCRATCH-AND-PLAN-FILE`, Opus high, Red; report `.agent-runtime/report-pane-scratch-plan.md`). `.pane/scratch/**` is carved out of the `.pane/**` never rule with the profile's existing exception shape (`profile.rs :: never_rules`, `SCRATCH_DIR`); dangling symlinks are now followed before judging (`canonical_prefix`, `DANGLING_LINK_LIMIT`), so a link under the scratchpad to a not-yet-existing host file is refused; every applier renders the carve-out (macOS `(allow file-write* (subpath …/.pane/scratch))` after the deny, Linux `bwrap_argv` bind, Windows a nested read-write DACL). Tests: `sandbox_profile::{the_scratchpad_is_carved_out_of_dot_pane_and_explore_writes_it, the_rest_of_dot_pane_stays_never_writable_in_every_mode, no_spelling_under_the_scratchpad_reaches_outside_it, the_scratchpad_carve_out_holds_in_verbatim_and_plain_spellings}`, `sandbox_apply::{every_applier_carves_the_scratchpad_out_of_dot_pane, a_confined_child_writes_the_scratchpad_and_not_the_host_configuration}`, `request_modes::explore_writes_the_scratchpad_and_is_refused_outside_it`. Mutations: carve-out removed (`except_spelling: None`) KILLED by `request_modes::explore_writes_the_scratchpad_and_is_refused_outside_it`; dangling link not followed KILLED by `sandbox_profile::no_spelling_under_the_scratchpad_reaches_outside_it`; seatbelt allow removed KILLED by `sandbox_apply::a_confined_child_writes_the_scratchpad_and_not_the_host_configuration`. Full `cargo test -p pane --no-fail-fast` in the worktree: 94 targets, 1493 passed, 0 failed.

**Verified by probing** (2026-09-17, `GH-PANE-EXPLORE-VERIFY`, Sonnet high, tests only; report `.agent-runtime/report-pane-explore-verify.md`): `tests/explore_escape_probes.rs`, six families, 82 probes through the real binary in `--mode explore` — redirects and clobbers (19), quoting and splitting (13), substitution and evaluation (29), paths and tools (12), network (6), direct and native doors (3) — asserting on the tree, the permission bits and a local listener; **0 slipped**. Not probed: a NUL byte in a line, a FIFO write, a direct `/tool bash` frame (the fixture's grammar does not reach it), configured patterns (no producer). The scratch probe was inverted at integration once the carve-out landed.

**Defect found and fixed** (found by the scratch-plan worker; fixed 2026-09-17 by `GH-PANE-HARD-LINKS`, Opus high, Red; report `.agent-runtime/report-pane-hard-links.md`). A hard link planted under the root — `.pane/scratch/hard` → `.pane/config.toml` — was judged by its path, so a write through it rewrote the target. Now `Profile::check` refuses any `write`/`edit` whose resolved target is a regular file with more than one name: `hard-linked file (<n> names): a write here reaches every name; copy it to a new file instead` — after the never rules and `deny`, before any grant, for `Access::Write` only; reads, directories, new and single-name files are unchanged; a metadata error fails closed. Measured on the real seatbelt: a confined `ln .pane/config.toml .pane/scratch/hard` is already refused (`link(2)` is checked against the source under `deny file-write*`), so the in-process rule closes the unconfined-plant case; `.git/config` is an ordinary project file and links, and the rule refuses the write through either name. Tests: `sandbox_profile::{a_write_through_a_hard_link_to_a_never_writable_file_is_refused_in_every_mode, a_hard_link_between_two_ordinary_project_files_is_refused_and_a_single_name_is_not}`, `sandbox_apply::a_confined_child_cannot_hard_link_a_never_writable_file_into_the_writable_tree`, `explore_escape_probes::family_hard_link_in_the_scratchpad`. Mutations: the check removed (`nlink() > 1` → `> u64::MAX`) KILLED by `sandbox_profile` (`:111`) and by the binary probe (`:789`); the check on reads KILLED by the read-through-link assertion (`:2577`). Limits: Windows refuses nothing (`number_of_links` is unstable on 1.98.0); Linux linking at the OS layer not measured; a bash redirect through a link planted from outside is judged by seatbelt path rules only; rollback of a hard-linked file now fails closed, untested.

**Limits.** In-process admission, not OS confinement: a read-only child still runs under the session's OS grant, and git config-driven execution (`core.fsmonitor`, textconv, pagers) is not seen by the word scan. `command_segments` does not track quotes (`grep "a>b"` is refused — the refusing direction). Acceptance commands and checkers run under the session profile. The Windows refusal-by-name is verified only by the `pane (windows-latest)` cell.

## Line 2638 — `plan` reads

**State: COMPLETE** (2026-09-17; plan reads by `GH-PANE-EXPLORE-MODE`, the plan file by `GH-PANE-SCRATCH-AND-PLAN-FILE`). Plan mode no longer short-circuits: its cells run under the plan narrowing (reads and read-only commands run, every write refused with `mode plan:`), so "executes no change" holds by the profile rather than by not running anything. Production: `session.rs :: run_task` (short-circuit removed), `modes.rs :: Narrowing::compile` (`Plan` → no writable globs). Tests: `request_modes::plan_reads_and_refuses_a_write`, `sandbox_profile::plan_reads_and_refuses_every_write_even_under_a_documentation_glob`. Mutation: `Plan => Vec::new()` → `return (None, diagnostics)` KILLED by `request_modes::plan_reads_and_refuses_a_write` (`:199`, PLAN.md exists). **The plan file** is `.pane/scratch/plan.md` (`modes.rs :: PLAN_FILE`): in `plan`, `Narrowing::compile` yields exactly that path and `write_refusal` compares by equality (a sibling, `plan.md/x`, `PLAN.md` or a link in its place are refused `mode plan:`); the prompt line says so. When a plan request wrote it (`written_plan`, mtime after the request started), the session carries `## Plan (.pane/scratch/plan.md)` — cut at a line boundary within 16 KiB — into the next non-plan request's system block once (`session.rs :: run_task`/`run_task_inner`, `prompt::plan_section`) and prints `plan written: .pane/scratch/plan.md (<n> lines)`. Tests: `sandbox_profile::plan_writes_only_its_plan_file`, `request_modes::{plan_writes_its_plan_file_and_is_refused_every_other_write, a_written_plan_reaches_the_next_request_once, a_plan_request_that_writes_nothing_carries_nothing}`, `prompt::tests::a_long_plan_is_cut_at_a_line_boundary_within_the_bound`.

| Decision | Mutation | Killing test | Result |
|---|---|---|---|
| Plan writes one file, not the subtree | `modes.rs`: equality → `covers(<scratch>)` | `request_modes::plan_writes_its_plan_file_and_is_refused_every_other_write` | KILLED (`:322`, notes.md exists) |
| The plan is carried | `session.rs`: `push_str(&section)` → `let _ = section` | `request_modes::a_written_plan_reaches_the_next_request_once` | KILLED (`:361`) |
| Carried once | `session.plan.take()` → `borrow().clone()` | same test | KILLED (`:367`) |

**Limits.** The carry-over keys on mtime with 1 s tolerance. The TUI line is asserted on the binary's stdout, not the live rendering. Linux Landlock cannot carve `.pane` at the OS layer at all (pre-existing, documented); Windows ACL rendering is verified only by the `pane (windows-latest)` cell.

## Line 2639 — the mode proposed from the intent

**State: COMPLETE** (2026-09-17, `GH-PANE-MODES-WIRED`, Sonnet high, Amber; report `.agent-runtime/report-pane-modes-wired.md`).

**Contract.** Given `[decisions] model` set and `mode = on`, when a request's intent answers `read_only` at or above `mode_above` (default 0.85) while the session mode is `execute` and unpinned, Pane runs that one request under the `explore` narrowing and prints `decision: explore for this request (read_only 0.97); /mode execute to pin`; between 0.5 and `mode_above` it prints one offer line and runs as today (a blocking question would stall scripted sessions); a pinned mode — `/mode <m>`, Shift-Tab, `--mode`, `--plan` — is never overridden and `/mode auto` unpins; `shadow` counts `would_apply` only; no model, `off`, a non-`read_only` intent or a failed decision leave the request in the session's mode. `session.mode` itself never changes. Bare `/mode` now prints the mode and its pin instead of cycling (Shift-Tab still cycles).

**Production.** `session/mode_proposal.rs :: propose`, `session.rs :: run_task_inner` (`proposal.narrow_mode` at the `narrowed_to` call), `session/controls.rs` (`/mode` pin, unpin, auto), `config.rs :: mode_above`, `settings/registry.rs`, `session/task.rs` (telemetry `decisions.mode_proposal: {proposed, applied, would_apply, pinned}` — not `decisions.mode`, which is the config string).

**Tests.** `tests/request_modes.rs::{a_confident_read_only_intent_proposes_explore_for_one_request, a_read_only_intent_below_mode_above_offers_explore_and_runs_as_today, mode_execute_pins_and_a_confident_intent_never_proposes, mode_auto_unpins_and_a_confident_intent_proposes_again, shadow_mode_runs_as_today_and_counts_would_apply}`.

| Decision | Mutation | Killing test | Result |
|---|---|---|---|
| The proposal needs confidence | `mode_proposal.rs`: `confidence < mode_above` → `< 0.0` | `request_modes::a_read_only_intent_below_mode_above_offers_explore_and_runs_as_today` | KILLED (`:533`) |
| A pin is never overridden | `\|\| proposal.pinned` → `\|\| false` | `request_modes::mode_execute_pins_and_a_confident_intent_never_proposes` | KILLED (`:553`, src/new.rs refused instead of written) |

**Gates.** `cargo test -p pane --test request_modes`: 16 passed; `--test settings_store` 30; `--test config` 17; `--test session` 105; targeted blast radius exit 0 in the worktree and on the merged tree.

**Limits.** The offer band (0.5 to `mode_above`) is a printed line, not a counted proposal. The threshold is a default (2646). **Interaction with the Phase 66 hold:** in `on` mode a confident `read_only` request now enters `explore`, where an effectful cell is refused by the profile before the hold could fire, so the hold (2613) applies only below `mode_above` or under a pin — the sweep on cf60020f caught this in `tests/decisions.rs`'s two hold tests (red on every platform), fixed forward by pinning `mode_above = 1.0` in that file's `DECISIONS_ON`.

## Line 2640 — remote commands gated in `explore`

**State: AWAITING USER DECISION (premise)** — found 2026-09-17 01:00 while writing the packet. Pane's shell has no network in any mode: `Profile::grants_network` is always false by design (sandbox-grants §4.1, "a network grant would have to be invented"), the seatbelt profile carries `(deny network*)`, Linux drops the network namespace / seccomp-denies sockets, Windows measures its network isolation. So no `ssh` or `scp` cell can run today, and the line's producer — a remote command that reaches a host — does not exist. Building the gate as written would mean inventing a network door, which §4.1 forbids the allow-list from doing on its own. **Proposal for the morning:** a host-run `remote` tool outside the cell sandbox — `[remote] hosts = ["nutanix.example"]` and an identity, the host's own account as the only reach — through which `ssh`/`scp` lines pass the static parse (mutating verbs, redirections, remote scripts refused), the decision model's "only reads" question above a threshold in `explore`, and a rollout line per command. The verifier's network family (6 probes, 0 slipped) shows the shell door is closed either way. Nothing is built until the user rules.

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

**State: COMPLETE** (2026-09-17, `GH-PANE-CELL-DRIFT`, Sonnet high, Amber; report `.agent-runtime/report-pane-cell-drift.md`; integrated by the primary, clean apply).

**Contract.** Given `[decisions] model` set and `mode = on`, when an effectful cell or direct frame is about to run and the intent hold has returned `Run`, if the runtime's plan has an `Active` step (the first, if several) Pane asks one `noul` — "the cell does what the plan's current step says and nothing else" — over `{request, step, cell}` (the cell bounded to 8 KiB at a line) and, at or below `drift_no_below` (default 0.10), returns the cell unrun once per task with a block naming the step and the answer; the re-issued cell is asked again but always runs (the once rule lives in `drift_for`, as in `hold_for`); an in-between answer, no plan, no active step, no effectful name, `mode = off`, a failed or slow decision all run the cell as today; `shadow` counts `would_drift` and never holds.

**Production.** `session/system.rs :: apply_decision_hold` → `apply_drift_hold`, `decide.rs :: {drift_for, Drift, bound_cell}`, `session/task.rs` (`drift_asked`, `drift_holds`, `would_drift`, `drift_failed`; telemetry `decisions.drift`), `config.rs :: drift_no_below` (0.0..=0.5), `settings/registry.rs`.

**Tests.** `tests/decisions.rs::{a_confident_drift_no_holds_the_cell_once_then_lets_it_run, an_in_between_drift_answer_runs_the_cell, a_plan_with_no_active_step_asks_nothing, shadow_counts_would_drift_and_runs_the_cell, a_slow_drift_decision_runs_the_cell_and_counts_drift_failed}`, `decide::tests::{drift_for_applies_the_threshold_and_the_once_rule, drift_block_names_the_step_and_the_confidence}`, `config::tests::the_drift_threshold_defaults_and_is_refused_out_of_range`.

| Decision | Mutation | Killing test | Result |
|---|---|---|---|
| A no must be confident | `decide.rs`: `noul <= drift_no_below` → `<= drift_no_below.max(1.0)` | `decisions::an_in_between_drift_answer_runs_the_cell` | KILLED (`:573`) |
| Held once, then it runs | `decide.rs`: `already_held` → `already_held && false` | `decisions::a_confident_drift_no_holds_the_cell_once_then_lets_it_run` | KILLED (`:544`) |

**Gates.** `cargo test -p pane --test decisions`: 35 passed; `--test settings_store` 29; `--test config` 17; `--lib decide` 19; targeted blast radius exit 0 in the worktree and on the merged tree.

**Limits.** Loopback fake only; the threshold is a default (2646). The step text is not byte-bounded (plan items are short). A first design that skipped the question after a hold let mutation (b) survive — the worker moved the once rule into `drift_for` and re-ran; recorded here as the reason the re-issued cell is asked at all.

## Line 2644 — the Scout's files ranked by relevance

**State: PARTIAL** (2026-09-17, `GH-PANE-HELPERS-JUDGED`, Sonnet high, Amber; report `.agent-runtime/report-pane-helpers-judged.md`). The ranking is built, wired at the Scout's production call site and mutation-proven; open on the three things named below.

**Contract (as built).** Given `[decisions] model` set and `mode = on`, when the preflight Scout is about to run, Pane asks one `noul` per candidate file (`{path, head}`, the head bounded to 40 lines, the whole state to 64 KiB by dropping heads then candidates from the end) in one request, hands the Scout its brief with a `## Candidate files, ranked by relevance` section highest first, and drops candidates at or below `scout_relevance_below` (0.10); the Scouting record says `ranked N, skipped M, top: a.rs 0.94, b.rs 0.81`; `shadow` asks and counts only; no model, `off`, an error or a timeout leave today's brief byte-identical.

**Production.** `helpers.rs :: {rank_scout_candidates, discover_scout_candidates, preflight_judged}`, `session/system.rs :: preflight_block` (the call site), `preflight.rs :: render` (the record's ranking note).

**Tests.** `tests/helpers_judged.rs::{the_scout_reads_the_higher_ranked_file_first_and_skips_the_one_below_the_floor, a_timeout_leaves_todays_order, one_request_per_ranking_however_many_files, shadow_asks_and_records_but_shows_nothing}`, `tests/preflight_scout.rs` (the record's shape).

| Decision | Mutation | Killing test | Result |
|---|---|---|---|
| Descending by relevance | `helpers.rs`: `b.1.total_cmp(&a.1)` → `a.1.total_cmp(&b.1)` | `helpers_judged::the_scout_reads_the_higher_ranked_file_first_and_skips_the_one_below_the_floor` | KILLED (b.rs 0.94 read after a.rs 0.20) |

**Gates.** `cargo test -p pane --test helpers_judged`: 6 passed; `--test preflight_scout`: 13; `--test helpers`: 26; targeted blast radius exit 0 in the worktree.

**Open, and why.** (1) `scout_relevance_below` is a constant (`helpers::DEFAULT_SCOUT_RELEVANCE_BELOW`), not a `[decisions]` key — `config.rs` was another package's this round. (2) The candidate pool is a new bounded walk (512 nodes / 40 files, directory order), not `helper_context.rs :: prepare_scout`'s term-matched selection — that module does no model work by its own invariant, so the ranking could not be hooked into it; the successor ranks `prepare_scout`'s pool instead. (3) `decisions.helpers` telemetry is unwired (`session/output.rs` was another package's). All three go to `GH-PANE-HELPERS-WIRED`.

## Line 2645 — a helper's result checked

**State: PARTIAL** — same package. **Contract (as built).** When the preflight Scout returns, Pane asks one `noul` over `{asked, result}` ("the result answers what was asked") and, at or below `helper_no_below` (0.10) with `mode = on`, appends one line to the helper's record — `decision: this result may not answer what was asked (0.06)` — never withholding, truncating or rerunning it; `shadow` asks and records; `judge: None` is byte-identical to today. Production: `helpers.rs :: {judge_outcome, run_judged, HelperJudge}`. Tests: `helpers_judged::{a_confident_no_carries_the_line_and_still_delivers, a_confident_yes_carries_nothing, judge_none_never_asks}`. Mutation: `noul <= judge.floor` → `<= 1.0` KILLED by `helpers_judged::a_confident_yes_carries_nothing`. **Open:** only the preflight Scout's call site is judged — the checker (`session/task.rs`) and cell-called helpers (`runtime/bindings.rs`) are not; `helper_no_below` is a constant. Successor: `GH-PANE-HELPERS-WIRED`.

## Line 2646 — off / shadow / on measured

**State: OPEN** — the orchestrator's, last: `pane ruler run` over one request set × three modes.
