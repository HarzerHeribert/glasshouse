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

⟨open⟩

## Line 2615 — request intent, effect hold

⟨open⟩

## Line 2616 — the diff matches the request, before completion

⟨open⟩
