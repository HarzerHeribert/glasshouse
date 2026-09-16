# Capability evidence — phase 66

Phase 66 — Pane: a decision model beside the task model (map lines 2613–2616),
recorded 2026-09-16 from the user's steering (`design-decisions.md`, *Jev is a
classifier for Pane first; routing stays static*). Entries are bounded by the
*Decompression* ruling: the contract, the tests by name, the decisive mutation where
a decision was added, the limits, and the report by path.

**Non-negotiables every entry is checked against.** The decision model writes no
text and decides no capability: it never sets a tool's `Purity`, never adds or
removes a sandbox grant, never answers an exact-call approval, never proves a
command-lifting equivalence. A failed, slow or absent decision leaves the session
exactly as it is without one. The gateway relays the protocol byte for byte and
parses nothing of it. The key lives in the gateway's store or its environment,
never in a packet, a report or a worker.

**Gate.** `scripts/blast-radius.sh --targeted <changed files>` per package; the
GitHub sweep's cells are the platform verdict.

**Provider facts — VERIFIED 2026-09-16 (evening) with the user's key, three live calls from a
script that never printed the key.** `POST https://api.typesafe.ai/v1/systemone`, `Authorization:
Bearer`, `model: "jev-latest"` answers as `"model": "jev-1.13.0"`. Request `{state, model,
questions{key: {type, instructions, criteria}}}` where **choice `criteria` is a map option →
rubric** (a list is a 422 `dict_type`), **score `criteria` is an ordered list of level
descriptions** (a map is a 422 `list_type`), noul `criteria` is optional `{true, false}`.
Answers: noul `{noul: 0.04}`; choice `{choice, confidence: 1.0, probabilities{…}}`; score
`{score: 2.72, confidence: 0.71, legend{"0": …, "4": …} (zero-indexed), probabilities{…}}`.
`usage{input_tokens, output_tokens}` (406/65 for a 2-question small state; 20,489/41 for a 42 KB
diff). Latency measured from macOS: 540–730 ms for small states, 1,029 ms for 42 KB — not the
70–150 ms the marketing page implies; a per-cell question is therefore asynchronous or nothing.
Headers: `x-typesafe-request-id`; no rate-limit headers; the docs say 429/529 want exponential
backoff. Intent probe: "read the config file and tell me what the timeout is" → `read_only` 1.0,
"needs a write" 0.04. Pane's `decide.rs` already sends the choice map and the bare noul, so
its wire shape is correct as landed. The gateway's `typesafe` template stays `Declared`
`Unverified` in code until a probe through the gateway (not direct) is recorded.

---

## Line 2613 — the gateway carries the protocol

**State: COMPLETE** (2026-09-16, `GH-GATEWAY-SYSTEMONE-CARRIER`, Sonnet medium, Amber; report `.agent-runtime/report-gateway-systemone-carrier.md`; integrated by the primary on the targeted gate).

**Contract.** Given a catalogue with a `typesafe` account, when a client bound to any other account posts `/v1/systemone`, the gateway relays it to `https://api.typesafe.ai` with the typesafe credential in `authorization` and the client's token gone, records the exchange under protocol `typesafe-systemone` and purpose `decision`, and reads its two token counts — while every other protocol routes byte for byte as before and a gateway with no typesafe account still refuses the target by name.

**Production.** `routing/wire.rs :: WireProtocol::TypesafeSystemOne` (relay-only; `translate/mod.rs` refuses every pair with it as `NOT_A_CHAT_PROTOCOL`); `pool.rs :: GATEWAY_INGRESS_PROTOCOLS`, `ingress_targets` (`/systemone`); `provider/mod.rs :: templates` (`typesafe`, `TYPESAFE_API_KEY`, every fact `Unverified`); `gateway/usage.rs :: TYPESAFE_SYSTEMONE` (`Format::cached` became `Option`; no cached figure exists); `routing/evidence/vocabulary.rs :: DECISION_PURPOSE`; and the decision, `gateway/upstream.rs :: Upstream::serving_for_target` called from `ingress.rs`: the model-chosen backend serves every target it claims; a target it does not claim goes to the backend that does when exactly one does; two claimants or none keep today's answer. Host side: the exhaustive matches in `crates/glasshouse/src/{config/capability.rs, config/profile.rs, profile/mod.rs, session/mod.rs}` and the templates pin fixture, spliced by insertion.

**Tests.** `gateway::conformance::a_systemone_request_reaches_the_typesafe_account_while_messages_stay_with_the_bound_one` (two real backends, two credentials, a socket); `gateway::upstream::target_rule_tests::{a_target_claimed_by_exactly_one_backend_is_served_even_when_the_session_is_bound_elsewhere, two_claimants_keep_todays_behaviour, no_claimant_falls_back_to_the_model_chosen_backend}`; `gateway::translate::tests::every_pair_touching_typesafe_systemone_is_refused_as_not_a_chat_protocol`; `gateway::usage::tests::a_typesafe_systemone_response_yields_the_two_counts_and_no_cached_figure`; `gateway::tests::a_decision_purpose_header_stamps_the_row_and_never_reaches_the_upstream`; `provider::tests::typesafe_is_a_template_serving_only_system_one_with_every_fact_unverified`; glasshouse `provider::tests::every_wire_protocol_pair_has_exactly_one_row_in_the_gateway_table`, `harness::pairing::tests::wire_protocol_from_slug_round_trips_every_known_slug_and_refuses_an_unknown_one`.

| Decision | Mutation | Killing test | Result |
|---|---|---|---|
| A target claimed by exactly one backend is served from it (2613) | `upstream.rs`: `if chosen.route_for(target).is_some() {` → `if true {` | `target_rule_tests::a_target_claimed_by_exactly_one_backend_is_served_even_when_the_session_is_bound_elsewhere` | KILLED — `assertion left == right failed` at upstream.rs:999, expected `typesafe`, got `claude-max` |

**Gates.** `cargo test -p inference-gateway`: 526 + 4 + 10 passed, 0 failed; `cargo test -p glasshouse --lib`: 1865 passed, 0 failed, 1 ignored; the targeted gate (19 files) green on the worker's tree and again on the merged tree at integration.

**Limits.** Every `Declared` on the `typesafe` template is `Unverified`: no probe with a real key was made, and the ingress target `/systemone` is this package's choice by the crate's convention, not an observed request line. **Debt, one line, not a package:** the ledger's `provider` column names the session's assigned backend, not the one `serving_for_target` placed the request with — a pre-existing shape of per-model routing, now reachable through this target.

## Line 2614 — Pane asks typed questions

**State: COMPLETE** (2026-09-16, `GH-PANE-DECISIONS-HOLD`, Sonnet high, Amber; report `.agent-runtime/report-pane-decisions-hold.md`; integrated by the primary on the targeted gate).

**Contract.** Given `[decisions] model` in `.pane/config.toml`, when a task starts, Pane asks the decision model the intent question over the gateway's `/v1/systemone` with `x-glasshouse-model` and `x-glasshouse-purpose: decision`, on its own thread, bounded to two seconds — and on any failure, timeout or absent model the task proceeds exactly as it does without one, the failure recorded in one notice and in telemetry.

**Production.** `crates/pane/src/decide.rs :: decide` (the body `{state, model, questions}`, the parsed `answers` with `choice`/`probabilities`/`confidence` or `noul`, `DECISION_TIMEOUT = 2 s`, `DecideError` carrying no body past 200 bytes), `intent_of` (the `intent` choice over `read_only | modify | run | other`); `crates/pane/src/session/system.rs :: task_decision` (the thread, the notice `decision: intent read_only (0.94, 180 ms)` or `decision: no answer (…)`); `config.rs :: DecisionsConfig`, `parse_decisions`; `settings/registry.rs` (`decisions.model|mode|hold_above`); `session/task.rs :: TaskState::decisions_telemetry`; `session/output.rs :: decisions`; the `/cell` note through `tui.rs :: Notebook.decision`.

**Tests.** `tests/decisions.rs::{the_decision_request_carries_purpose_model_and_the_intent_question, a_failed_or_slow_decision_leaves_the_task_as_it_is, no_model_means_no_request_and_no_thread}`; `decide::tests` (6: the body shape byte for byte, the documented `choice` response, a missing `confidence` is a parse error, `EFFECTFUL_NAMES` equals the registry's effectful tools plus `checks`, `agent`, `mcp`, `names_effect` finds `write` at its line and nothing for `read`); `tests/config.rs` (17, the `[decisions]` table, refused unknown key and range).

**Limits.** No live TypeSafe endpoint was called: the wire is the documented shape against a loopback fake, and every fact about the provider stays `Unverified` in the gateway's template. Wiring, no decision — no mutation owed.

## Line 2615 — request intent, effect hold

**State: COMPLETE** (same package and report).

**Contract.** Given a `read_only` intent at or above `hold_above` (default 0.85), when a cell's free names or a lowered direct frame name an effectful capability (`bash`, `write`, `edit`, `checks`, `agent`, `mcp`), Pane in `mode = on` does not run it the first time and answers the model with one `## Held (decision)` block; the same call re-issued runs and counts an override; `mode = shadow` runs everything and counts the would-be hold — while no mode changes a grant, a `Profile`, a tool's `Purity` or an approval `Decision`.

**Production.** `crates/pane/src/decide.rs :: hold_for` (the pure rule: mode, intent, threshold, effect, once), `names_effect` / `direct_frame_names_effect` over `CompiledCell::free_names` and `Lowered.calls`, `held_block`; `crates/pane/src/session/system.rs :: apply_decision_hold` (compiles the cell for its free names before V8 runs anything, returns the `Step` whose `answer` is the block, charges no cell of the budget); `session.rs :: act_on` (the one call); `TaskState.{intent, effect_holds, effect_overrides, would_hold}`.

**Tests.** `tests/decisions.rs::{a_read_only_request_holds_the_first_effectful_cell_once_then_lets_it_run, a_confidence_below_hold_above_never_holds, shadow_records_the_would_be_hold_and_writes_the_file, a_modify_intent_or_a_pure_cell_is_never_held, a_direct_tool_frame_is_held_by_the_same_rule}` — binary-level through the built `pane` against a path-dispatching loopback fake.

| Decision | Mutation | Killing test | Result |
|---|---|---|---|
| The threshold (2615) | `decide.rs`: `intent.confidence < hold_above` → `intent.confidence < hold_above \|\| true` | `tests/decisions.rs::a_read_only_request_holds_the_first_effectful_cell_once_then_lets_it_run` | KILLED — `assertion left == right failed: held, then the re-issue that runs; left: 1, right: 2` |
| The once rule (2615) | `decide.rs`: `DecisionMode::On if already_held => Hold::Overridden` → `… if already_held && false` | the same test | KILLED — `left: 3, right: 2` (the re-issue is held again; the third request outruns the two-response fake) |

**Gates.** `cargo test -p pane --test decisions`: 8 passed, 0 failed; `--lib decide::`: 6 passed; `--test config --test evidence_gate --test session_output`: 17 + 9 + 9 passed; `--test settings_store --test tui --test tui_look --test cell_inspection --test standing_handlers`: 29 + 33 + 42 + 19 + 19 passed; clippy clean; the targeted gate (13 files) green on the worker's tree and again on the merged tree at integration; size ratchet ok (the hold site lives in `session/system.rs` because `session.rs` sits at its baseline).

**Limits.** The threshold 0.85 is a default, not a calibration — `shadow` telemetry on real tasks decides it. `effect_overrides` counts every later qualifying cell, not only the one after the hold (the worker's reading, tested). The `/cell` note line has no test of its own (Debt, one line). Not exercised: a live decision model. **Curiosity, recorded once:** the first integration gate saw `session_output::provider_failure_produces_machine_error_and_nonzero_exit` fail with `remove_dir_all` NotFound at its own cleanup; alone and as a whole target it passed four times in a row on the same tree, and the second targeted gate was green — a cleanup race in the test, not a package.

## Line 2616 — the diff matches the request, before completion

**State: COMPLETE** (2026-09-16, `GH-PANE-DIFF-MATCHES-REQUEST`, Sonnet high, Amber; report `.agent-runtime/report-pane-diff-matches-request.md` with its *Addendum*; integrated by the primary on the targeted gate).

**Contract.** Given a decision model, when the model claims completion, Pane asks one `noul` — does the task's diff satisfy the request? — over `{request, diff (bounded to 64 KiB at a hunk boundary), findings}`, re-asking whenever the diff has changed since the last answer; a confident no (`noul <= completion_no_below`, default 0.10) is one more finding held once like the others and never a refusal; a confident yes (`noul >= completion_yes_above`, default 0.90) with no other finding spares the fresh checker for that claim; `shadow` records only; a failed or slow answer leaves the gate exactly as it is.

**Production.** `crates/pane/src/session/task.rs :: TaskState::gate` (the question between the acceptance findings and the checker block; `completion_answer: Option<(String, CompletionAnswer)>` keyed by the diff text; `RequestNotSatisfied` pushed on a confident no; `checker_ran = true` with `checker_skipped` on a confident yes); `crates/pane/src/decide.rs :: completion_satisfied`, `satisfied_question`, `bound_diff`, `DIFF_STATE_BYTES`; `completion.rs :: FindingKind::RequestNotSatisfied`; `config.rs :: DecisionsConfig.{completion_no_below, completion_yes_above}`; `settings/registry.rs` (two `Kind::Float` specs); `session/output.rs` (the `decisions.completion` block).

**Tests.** `tests/decisions.rs::{a_confident_no_holds_the_completion_once_then_records_it_unverified, a_confident_yes_spares_the_fresh_checker_when_nothing_else_is_found, an_undecided_answer_runs_the_checker_as_today, a_yes_never_removes_a_mechanical_finding, shadow_records_the_completion_answer_and_changes_nothing, a_failed_or_slow_completion_decision_leaves_the_gate_as_it_is, a_large_diff_is_cut_at_a_hunk_boundary_and_still_asked, a_changed_diff_is_asked_again_and_a_fixed_task_verifies}` (binary-level, loopback fake dispatching `/v1/systemone` by question key); `decide::tests::{the_satisfied_body_serializes_to_the_documented_shape, bound_diff_cuts_at_a_hunk_boundary_and_sets_the_flag, a_completion_answer_missing_a_noul_is_a_parse_error}`; `config::completion_threshold_tests` (2); `tests/settings_store.rs` (the two refused ranges).

| Decision | Mutation | Killing test | Result |
|---|---|---|---|
| A confident no is a finding (2616) | `task.rs`: `answer.noul <= completion_no_below` → `answer.noul <= -1.0` | `tests/decisions.rs::a_confident_no_holds_the_completion_once_then_records_it_unverified` | KILLED — `held once, then the same claim again; left: 1, right: 2` |
| A changed diff is asked again (2616, the integration's addendum) | `task.rs`: `asked_about != &diff_text` → `… && false` | `tests/decisions.rs::a_changed_diff_is_asked_again_and_a_fixed_task_verifies` | KILLED — `asked: 1` and the stale `noul: 0.05` reused, `verified: false` |

**Gates.** `cargo test -p pane --test decisions`: 16 passed, 0 failed; `--test evidence_gate --test final_state_contract --test config`: 9 + 6 + 17; `--test settings_store --test session_output`: 29 + 9; `--lib`: 375 passed; fmt and clippy clean; the targeted gate (9 files) green on the worker's tree twice and again on the merged tree at integration.

**Limits.** The two thresholds are defaults, not a calibration; `shadow` telemetry on real tasks decides them. A confident yes proves nothing the acceptance list or the final-state contract did not already decide mechanically. `Question::Noul` carries `instructions` only — the documented optional `criteria {true, false}` is folded into the sentence (Debt, one line, if a live probe shows the criteria matter). The hunk-boundary cut assumes one hunk header per changed file, which is `changes.rs`'s shape today. Not exercised: a live decision model. **Ruling at integration:** the worker's first version cached the answer for the whole task, so a task fixed after a confident no would have finished `verified = false` against a stale answer; the addendum keyed the cache by the diff, and the second mutation pins it.
