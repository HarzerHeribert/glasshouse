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

**Provider facts.** Every `Declared` fact about `typesafe` stays `Unverified` until
a probe with a real key is recorded here with its date and its exact response shape.
Read 2026-09-16 from docs.typesafe.ai, unverified against a live endpoint: `POST
https://api.typesafe.ai/v1/systemone`, `Authorization: Bearer`, model `jev-latest`,
body `{state, model, questions{key: {type, instructions, criteria}}}`, answers
`{noul}` / `{choice, probabilities, confidence}` / `{score, legend, confidence}`,
`usage{input_tokens, output_tokens}`.

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

⟨open⟩
