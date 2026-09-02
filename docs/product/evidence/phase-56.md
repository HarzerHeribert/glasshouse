# Capability evidence — phase 56

> **Renamed 2026-09-02 (batch 92):** `tests/v1_criteria_setup.rs` is now `tests/v1_criteria_first_run.rs` and `tests/dispatch_reservation.rs` is now `tests/launch_reservation.rs` — Windows' installer detection refused to start any binary whose name contains *setup* or *patch* (error 740), so neither had ever run on the VM leg. Test names quoted below are unchanged; only the file (and so the `--test` target) moved.

**This file is the evidence entry for Phase 56 AND for Phase 56A.** They are
kept together because 56A refined 56 the same day and their entries
interleave — `scripts/check-evidence-coverage.py` reads headings, so this
line is what tells it Phase 56A is covered here rather than missing.

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules). Phase 56 — *choose the harness, not the provider* — was recorded on 2026-08-31 from the user's instruction of record (`design-decisions.md` §Phase 56), and refined the same day by Phase 56A (the entitlement pool). Its packages land in the order that section gives; each entry below names its package.

### Lines 1946, 1947 and 1954 — a subscription is a configured routing resource with rules, and the router refuses by name

Package `GH-SUBSCRIPTION-RULES`, 2026-08-31, Fable 5 at xhigh (Red: a new hard constraint the router acts on, and a new configured resource). One mechanism: `[subscriptions.<name>]` in user and project configuration (project over user, resolved by `EffectiveConfig::subscriptions` / `subscription_for` exactly as `reserve_policies` is) carries `kind`, its backing (a harness's own sign-in, or a configured `[providers.<name>]` entry, or unstated — *listed, never matched, never charged*) and `SubscriptionRules` — allow/deny lists over harnesses, workload tiers and job kinds, where an empty allow-list admits everything and **deny wins** (`admits = allowed && !denied`). The launch path attaches the resolved subscription to every routing destination (`destination_subscription` → `Destination::with_subscription`, `main.rs`), and `hard_constraint` asks the subscription's rule **first**, before any capability fact, so a person reads the constraint they wrote: `HardConstraint::Subscription { subscription, refused }` renders *"subscription `X` does not serve harness `Y` / the `Z` tier"*; the tier half fires only on a pass that knows the tier. The launch announcement names the subscription that will serve the session (`announce_subscription`). Phase 56A renames all of this to *entitlement* in its first package (`entitlement-pool`), which also gives the job-kind rules their consumer — the disposable router — which the worker documented no session router reads because a session has no job kind.

### Treat a subscription — a Claude, ChatGPT/Codex, or Gemini plan, or an API key — as a routing resource with its own rules, separate from any harness that consumes it. (line 1946)

Contract: Given the user's `[subscriptions.<name>]` tables in either configuration layer, when Glasshouse resolves the resource a session would be charged to, it treats each subscription — a Claude, ChatGPT/Codex or Gemini plan, or an API key, backed by a harness's own sign-in or by a configured provider — as a named routing resource separate from the harness that consumes it, while preserving that a user who configured nothing sees every harness's own sign-in as an unrestricted default entry named by the harness and nothing else changes.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's artifacts (12/12 mutations KILLED with killing tests named; `test result:` lines with counts; the worker's blast radius 58/59 targets 2482 passed then the remainder run in the foreground; `integrate.sh`'s blast radius on the merged tree — see the commit) and the diff of the decision read before dispatch of the successor: `EffectiveConfig::subscriptions` layers per entry by name and synthesises an unrestricted default per harness-kind id, and `destination_subscription` attaches the resolved entry at both launch sites.

Production evidence:
- `src/config/mod.rs` — `SubscriptionConfig, SubscriptionTable, SubscriptionKind, ConfiguredHarness, ConfiguredJobKind`
- `src/config/mod.rs` — `UserConfig::subscriptions, ProjectConfig::subscriptions`
- `src/config/mod.rs` — `EffectiveConfig::subscriptions (project-over-user by name, plus the default entry per harness-kind IntegrationId)`
- `src/config/mod.rs` — `EffectiveConfig::subscription_for (Native -> the claiming entry; DirectProvider -> the entry naming the provider; GlasshouseGateway -> None)`
- `src/config/mod.rs` — `ResolvedSubscription::to_routing, ::describe; SubscriptionLookupError`
- `src/routing/mod.rs` — `Subscription (name + rules, what a Destination carries)`
- `src/main.rs` — `destination_subscription (the one lookup, attached in routing_destinations at both sites)`

Regression evidence:
- `config::tests::subscriptions_round_trip_and_resolve_project_over_user_with_a_native_default`
- `config::tests::contradictory_subscription_tables_are_refused_by_name`
- `subscription_rules::the_native_default_and_a_configured_native_subscription_are_announced_by_name`
- `subscription_rules::a_continued_session_announces_its_subscription`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| crates/glasshouse/src/config/mod.rs: if claimed { -> if claimed || !claimed { | `skip-state-update` | **killed** | `config::tests::subscriptions_round_trip_and_resolve_project_over_user_with_a_native_default` |
| crates/glasshouse/src/config/mod.rs: if claimed { -> if claimed || !claimed { | `skip-state-update` | **killed** | `the_native_default_and_a_configured_native_subscription_are_announced_by_name` |
| crates/glasshouse/src/main.rs: .with_subscription(fresh_subscription), -> .with_subscription(None), | `remove-guard` | **killed** | `route_names_the_subscription_that_refused_a_destination` |
| crates/glasshouse/src/main.rs: .with_subscription(subscription), -> .with_subscription(None), | `remove-guard` | **killed** | `a_tier_rule_reaches_the_route_report_through_the_task_classification` |

> skip-state-update observed: panicked at crates/glasshouse/src/config/mod.rs:7277 — `let codex = effective .subscription_for(IntegrationId::Codex, &BackendResource::Native) .unwrap() .unwrap();`

> skip-state-update observed: panicked at crates/glasshouse/tests/subscription_rules.rs:695 — `assert!( said.contains( "subscription `claude-code` (Claude Code's own sign-in) will serve this session." ), "{said}" );`

> remove-guard observed: panicked at crates/glasshouse/tests/subscription_rules.rs:663 — `assert!( rejected.contains("fresh:claude-code:alpha"), "the alpha profile is refused:\n{report}" );`

> remove-guard observed: panicked at crates/glasshouse/tests/subscription_rules.rs:872 — `assert!( rejected.contains("via alpha-probe (existing) — hard subscription constraint — subscription `team-key` does not serve the `heavy` tier"), "{heavy_again}" );`

Recorded scope limits — stated by the worker, not discovered later:
- `kind` is optional and descriptive: no rule reads it, only the announcement; the default entry for a harness's own sign-in has no kind, because Glasshouse does not know which plan a person signed a harness in with
- a gateway-backed profile has no subscription at launch (upstream assigned at session start) — rules cannot apply to it until Phase 56 step 3 binds the upstream where the router can see it
- the session record does not store the subscription name: no existing column can hold it and the packet forbade a migration — successor: a `sessions.subscription` column beside `backend_resource`
- layering is per entry by name (a project's `[subscriptions.X]` replaces the user's `X` whole), not per field as `ReservePoliciesConfig` layers — the packet's analogy did not fit a named table

---

### Allow a subscription rule to state which harnesses, workload tiers, and job kinds the subscription may serve, and which it must never serve. (line 1947)

Contract: Given a subscription entry, when the user states allow_harnesses, deny_harnesses, allow_tiers, deny_tiers, allow_job_kinds or deny_job_kinds, Glasshouse resolves them into one SubscriptionRules value in which deny wins over allow, an empty allow-list admits everything not denied and a stated allow-list admits only its members, while preserving that an unknown spelling on any axis is refused by the loader rather than read as no rule.

State: COMPLETE — ruled 2026-08-31 by the orchestrator. The line is *state* the rule over harnesses, tiers and job kinds: all three are stated, resolved, round-tripped and unit-tested, and `admits` is `allowed && !denied` with an empty allow-list admitting everything (deny wins) — the decision read in the diff. **Recorded limit, not a gap in this line:** the job-kind axis is consumed by no router today (a session has no job kind; the worker documented it at `SubscriptionRefusal`); the disposable router gains that consumer in package `entitlement-pool` (56A step 1), which also renames the whole surface to *entitlement*.

Production evidence:
- `src/routing/mod.rs` — `SubscriptionRules (six lists, one private `admits`, serves_harness/serves_tier/serves_job_kind/refusal)`
- `src/config/mod.rs` — `SubscriptionConfig::rules (the six lists -> SubscriptionRules), ConfiguredHarness/ConfiguredWorkloadTier/ConfiguredJobKind spellings`

Regression evidence:
- `subscription_rules::deny_wins_over_allow_on_every_axis`
- `subscription_rules::an_empty_allow_list_admits_everything_not_denied_and_a_stated_one_only_its_members`
- `config::tests::subscriptions_round_trip_and_resolve_project_over_user_with_a_native_default (unknown spellings refused)`
- `config::tests::every_job_kind_spelling_round_trips`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| crates/glasshouse/src/routing/mod.rs: allowed && !denied -> allowed || !denied | `invert-condition` | **killed** | `deny_wins_over_allow_on_every_axis` |
| crates/glasshouse/src/routing/mod.rs: return Some(SubscriptionRefusal::Tier(tier)); -> return None; | `skip-state-update` | **killed** | `a_tier_rule_fires_only_against_an_established_tier` |

> invert-condition observed: panicked at crates/glasshouse/tests/subscription_rules.rs:175 — `assert!( !rules.serves_harness(IntegrationId::ClaudeCode), "a harness on both lists is denied" );`

> skip-state-update observed: panicked at crates/glasshouse/tests/subscription_rules.rs:343 — `assert_eq!( rejection(&heavy, "no-heavy"), Some(&refused_tier("no-heavy", WorkloadTier::Heavy)) );`

Recorded scope limits — stated by the worker, not discovered later:
- THIN SPOT — the job-kind axis is stated, resolved and unit-tested (serves_job_kind) but consumed by no router: a session has no job kind and DisposableRouting's candidates carry no subscription (routing/disposable.rs was outside this partition). Named successor GH-SUBSCRIPTION-JOB-KINDS: DisposableCandidate::with_subscription + a subscription_for_provider lookup + a NAMED refusal in DisposableRouting::choose's rationale, never a silent pre-filter in main.rs::disposable_candidates. The orchestrator may prefer PARTIAL for this line on that ground
- a rule names a harness by IntegrationId slug and only harness-kind ids parse (ollama, llama-cpp, cmux are refused by the loader)

---

### Never charge a task to a subscription the user's rules did not allow for that harness or tier, and announce which subscription served each session. (line 1954)

Contract: Given a launch, a resume or a `glasshouse route` report, when the subscription a destination would charge has a rule that does not admit the harness or the established tier, Glasshouse never makes that destination a candidate, refuses a launch whose only destination it was by the subscription's name, and announces which subscription will serve every session it starts or continues, while preserving that a resource no subscription describes is never refused and is announced as such.

State: PARTIALLY VERIFIED — ruled 2026-08-31 by the orchestrator, **box NOT ticked.** The native and direct-provider halves are complete: `hard_constraint` asks the subscription first, the `--no-routing` launch gets the harness-half gate, the every-destination-refused case refuses by name and starts nothing, and launch and resume announce the serving subscription (six binary tests). The clause that is missing is the worker's own first limit: a gateway-backed profile has no subscription at launch because the gateway assigns its upstream at session start where the router cannot see it — so a `[subscriptions]` entry backed by that provider whose rules deny this harness would still be charged. *Never charge* is not yet true on that path. Successor: the gateway's upstream assignment consults `subscription_for(provider)` against the session's harness and tier (Phase 56A steps 3–4, the broker), or a bounded Amber package on `gateway/session.rs` once `gateway-translate` releases that file. Also open here: the session record stores no subscription name (needs a `sessions.subscription` column beside `backend_resource` — a migration the packet forbade).

Production evidence:
- `src/routing/mod.rs` — `HardConstraint::Subscription { subscription, refused: SubscriptionRefusal }, ::reason ("subscription `X` does not serve harness `Y` / the `Z` tier")`
- `src/routing/mod.rs` — `Subscription::constraint (the one place a rule becomes a HardConstraint)`
- `src/routing/session.rs` — `Destination::with_subscription/subscription; hard_constraint (asked first; harness half on both passes, tier half against the gate tier only)`
- `src/routing/session.rs` — `SessionRouter::gate (extracted step 2), SessionRouter::refused (the gate exposed for the every-destination-refused case)`
- `src/main.rs` — `launch_session — the `nowhere_to_go` guard after `choose` (refuses by name when the launch's own fresh destination was subscription-refused and `choose` answered None)`
- `src/main.rs` — `launch_session — the harness-half gate + announce_subscription after the launch profile resolves (the only check a --no-routing launch gets)`
- `src/main.rs` — `resume_session — announce_subscription after overlay_resolution (reached by `glasshouse resume` and by a launch the router steered into an existing session)`
- `src/routing/session.rs` — `Routed::render_overview (pre-existing) renders the rejection and its reason for `glasshouse route``

Regression evidence:
- `subscription_rules::a_subscription_that_denies_the_harness_removes_the_destination_and_names_itself`
- `subscription_rules::the_subscription_constraint_outranks_a_warm_session`
- `subscription_rules::a_tier_rule_fires_only_against_an_established_tier`
- `subscription_rules::a_destination_with_no_subscription_is_never_refused_by_one`
- `subscription_rules::refused_reports_the_gate_when_choose_has_nowhere_to_go`
- `subscription_rules::an_override_naming_a_refused_destination_is_refused_by_the_subscription`
- `subscription_rules::a_launch_whose_subscription_denies_the_harness_is_refused_by_name_and_starts_nothing (binary)`
- `subscription_rules::route_names_the_subscription_that_refused_a_destination (binary)`
- `subscription_rules::the_native_default_and_a_configured_native_subscription_are_announced_by_name (binary)`
- `subscription_rules::a_continued_session_announces_its_subscription (binary)`
- `subscription_rules::a_routing_off_launch_still_applies_the_harness_rule (binary)`
- `subscription_rules::a_tier_rule_reaches_the_route_report_through_the_task_classification (binary; fresh and existing destinations)`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| crates/glasshouse/src/routing/session.rs: subscription.constraint(destination.harness(), minimum_tier)?; -> let _ = subscription.constraint(destination.harness(), minimum_tier); | `skip-state-update` | **killed** | `a_subscription_that_denies_the_harness_removes_the_destination_and_names_itself` |
| crates/glasshouse/src/routing/session.rs: subscription.constraint(destination.harness(), minimum_tier)?; -> let _ = subscription.constraint(destination.harness(), minimum_tier); | `skip-state-update` | **killed** | `route_names_the_subscription_that_refused_a_destination` |
| crates/glasshouse/src/routing/session.rs: self.gate(destinations, inputs).rejected -> Vec::new() | `bypass-fallback` | **killed** | `refused_reports_the_gate_when_choose_has_nowhere_to_go` |
| crates/glasshouse/src/main.rs: let nowhere_to_go = routed.is_none(); -> let nowhere_to_go = false; | `remove-guard` | **killed** | `a_launch_stating_heavy_work_is_refused_by_a_tier_rule_before_anything_starts` |
| crates/glasshouse/src/main.rs: && let Some(refused) = subscription.rules().refusal(launch_profile.harness, None) -> && let Some(refused) = subscription.rules().refusal(launch_profile.harness, None).filter(|_| false) | `remove-guard` | **killed** | `a_routing_off_launch_still_applies_the_harness_rule` |
| crates/glasshouse/src/main.rs: let served_by = subscription.name(); -> let served_by = profile.harness.slug(); | `wrong-source` | **killed** | `the_native_default_and_a_configured_native_subscription_are_announced_by_name` |

> skip-state-update observed: panicked at crates/glasshouse/tests/subscription_rules.rs:266 — `assert_eq!(routed.chosen().id(), "own", "{}", routed.render_overview());`

> skip-state-update observed: panicked at crates/glasshouse/tests/subscription_rules.rs:663 — `assert!( rejected.contains("fresh:claude-code:alpha"), "the alpha profile is refused:\n{report}" );`

> bypass-fallback observed: panicked at crates/glasshouse/tests/subscription_rules.rs:407 — `assert_eq!(refused.len(), 1);`

> remove-guard observed: panicked at crates/glasshouse/tests/subscription_rules.rs:798 — `assert!( !refused.status.success(), "heavy work charged to a subscription that denies heavy work must be refused:\n{said}" );`

> remove-guard observed: panicked at crates/glasshouse/tests/subscription_rules.rs:753 — `assert!(!refused.status.success(), "{said}");`

> wrong-source observed: panicked at crates/glasshouse/tests/subscription_rules.rs:704 — `assert!( said.contains( "subscription `max` (Claude plan, Claude Code's own sign-in) will serve this session." ), "{said}" );`

Recorded scope limits — stated by the worker, not discovered later:
- the tier half fires only against an established tier: a plain launch with no --task has none, so allow_tiers/deny_tiers are inert there (the limit Phase 35D recorded for the tier being None on production paths); proven on the binary through `route --task` for a fresh and an existing destination
- resume announces and does not gate: `glasshouse resume <id>` of a session whose subscription's rule now denies the harness proceeds, announced; at a task boundary `choose` holds the current destination when every destination fails (pre-existing) and `route` reports the rejection
- a --no-routing launch gets the harness half only (no classification, no tier)
- the router-side launch guard reads only the Subscription constraint; a protocol or tool-semantics refusal of the sole destination keeps the pre-existing fallback (noted in the code) — narrowed on purpose to this line
- macOS only; the #[cfg(windows)] fake-harness arm is copied from route_command.rs and untested here

---

---

---

### Lines 1948, 1949, 1950, 1956 — translation T1: the canonical form, two codecs, the pair table, and the first pair end to end

Package `GH-GATEWAY-TRANSLATE`, 2026-08-31, Fable 5 at xhigh (Red). Implements design-decisions §Phase 56 *"the user's answer on pairs: all of them"*: one canonical form, one codec per wire protocol, a pair a decoder and an encoder meeting in the middle. The relay rule is narrowed, not repealed — a served target is still forwarded byte for byte (tested), and only the branch that answered `404` may enter a codec. 4/4 mutations KILLED (a first batch's verdicts were void on an exit-127 `--test-cmd` misuse, detected and re-run). The integrator's one seam fix, on the merged tree and quoted by the report: `harness/pairing.rs`'s pinned tripwire `no_pair_of_protocols_is_translated_today` — whose own doc says a failure means this report exists — updated to pin exactly the supported pair, because the file was FORBIDDEN to the worker (the packet's own scoping).

### Serve any supported harness through Glasshouse's bundled API gateway from any subscription or model whose wire protocol the gateway can translate to the harness's native protocol. (line 1948)

Contract: Given a supported harness and an entitlement whose wire protocol the gateway can translate to the harness's native protocol, when the harness sends its native request to the bundled gateway, Glasshouse serves it through the translated pair end to end, while preserving the byte-for-byte relay for every natively served target.

State: PARTIALLY VERIFIED — ruled 2026-08-31 by the orchestrator, agreeing with the worker's `open`: this line quantifies over every supported pair, and exactly one exists. T1's evidence stands in this entry (the canonical form, both codecs, the seam that only ever enters on the target the provider does not serve, and the end-to-end test through the shipped binary for Claude Code on an OpenAI-Chat entitlement — ids preserved, streaming in Anthropic's order, refusal by name with nothing opened upstream, byte-for-byte relay untouched). Successors: T2 (openai-responses codec) and T3 (Gemini codec + adapter).

Production evidence:
- `src/gateway/translate/mod.rs` — `serve`
- `src/gateway/ingress.rs` — `unrouted`
- `src/gateway/upstream.rs` — `UpstreamBackend::route_named`

Regression evidence:
- `gateway_translate::a_claude_code_request_is_translated_to_chat_completions_and_the_answer_back_with_ids_preserved`
- `gateway_translate::a_streamed_request_is_translated_event_by_event_in_anthropics_order_and_terminated`
- `gateway_translate::the_shipped_binary_still_refuses_claude_code_on_a_chat_only_entitlement_at_profile_resolution`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| ingress.rs: before `let Some(uri) = route.uri_for(&head.target) else {`, insert `if let translate::Placement::Translate(pair) = translate::place(&head.target, &["openai-chat"]) { return translate::serve(head, reader, out, upstream, serving, agent, pair); }` | `served-target-enters-codec` | **killed** | `gateway_translate::a_target_the_provider_serves_natively_is_relayed_byte_for_byte_even_though_a_codec_exists` |

> served-target-enters-codec observed: test ... FAILED; panicked at tests/gateway_translate.rs:178: assertion `left == right` failed: exactly one request at the fixture (the mutated gateway refused the relay-only body and the fixture saw nothing)

Recorded scope limits — stated by the worker, not discovered later:
- NOT closable by this package: profile::apply_gateway (profile/mod.rs:1157-1175, forbidden file) refuses GatewayProtocolUnserved before the harness starts; (a)-(c) are proven at gateway::start_if_required_with_degrade_sink — the door main.rs calls — with the upstream built by production profile::gateway_upstream; the witness test pins the refusal and fails the day the profile packet lifts it. Successor: GH-GATEWAY-TRANSLATE-LAUNCH (Amber, profile/mod.rs).

---

### Translate between wire protocols at the gateway for concrete harness/provider pairs as each is required, recording every supported pairing and every refused one by name. (line 1949)

Contract: Given any ordered pair of wire protocols, when translation is asked for or a request needs it, Glasshouse answers from one table that lists every pair exactly once — supported (only behind its end-to-end test) or refused with its reason by name — while preserving that protocol_fit and the ingress refusal bodies are the table's production consumers.

State: COMPLETE — ruled 2026-08-31 by the orchestrator. The pair table lists every ordered pair of `WireProtocol` exactly once, supported or refused with its reason; its two production consumers are `protocol_fit` (through `translation_available`, no longer the all-false stub) and the 4xx refusal body; the refusal path is mutation-killed and the every-pair-once test pins the table. `translate::pairs` (the enumeration for a later CLI view) has no production caller and was not counted as evidence.

Production evidence:
- `src/gateway/translate/mod.rs` — `TABLE / lookup / is_supported`
- `src/provider/mod.rs` — `translation_available`
- `src/gateway/ingress.rs` — `unrouted (404 body naming pair and reason)`

Regression evidence:
- `gateway::translate::tests::every_ordered_pair_appears_exactly_once`
- `gateway::translate::tests::exactly_the_first_pair_is_supported_and_every_other_row_carries_a_reason`
- `provider::tests::every_wire_protocol_pair_has_exactly_one_row_in_the_gateway_table`
- `gateway_translate::a_request_the_pair_cannot_carry_is_refused_by_name_and_nothing_is_opened_upstream`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| translate/mod.rs: `status: PairStatus::Refused(NOT_YET_REVERSE),` -> `status: PairStatus::Supported,` | `refused-pair-marked-supported` | **killed** | `gateway_translate::a_request_the_pair_cannot_carry_is_refused_by_name_and_nothing_is_opened_upstream` |

> refused-pair-marked-supported observed: test ... FAILED; panicked at tests/gateway_translate.rs:1048 (the reverse pair was translated instead of refused with its 1956 reason)

Recorded scope limits — stated by the worker, not discovered later:
- translate::pairs() (the enumeration for a later CLI view) has no production caller and is not evidence for this line; the two production consumers are protocol_fit via translation_available, and the ingress refusal bodies.
- harness/pairing.rs's pinned test no_pair_of_protocols_is_translated_today now fails by design (forbidden file); integrator's one-line fix quoted in the report.

---

### Keep a harness's native tooling — editing, shell, repository, and tool-call behaviour — intact when it is served by a non-native provider, and refuse the pairing by name when it cannot be kept. (line 1950)

Contract: Given a harness served through a translated pair, when tool definitions, tool calls, tool results (erroring included), parallel calls, stop reasons and system prompts cross the gateway, Glasshouse preserves them with ids verbatim in both directions, while refusing per request, by field name and before anything is opened upstream, whatever the pair cannot carry.

State: PARTIALLY VERIFIED — ruled 2026-08-31 by the orchestrator, agreeing with the worker's `open`: this line quantifies over every supported pair, and exactly one exists. T1's evidence stands in this entry (the canonical form, both codecs, the seam that only ever enters on the target the provider does not serve, and the end-to-end test through the shipped binary for Claude Code on an OpenAI-Chat entitlement — ids preserved, streaming in Anthropic's order, refusal by name with nothing opened upstream, byte-for-byte relay untouched). Successors: T2 (openai-responses codec) and T3 (Gemini codec + adapter).

Production evidence:
- `src/gateway/translate/anthropic.rs` — `decode_request / encode_response / REFUSED_FIELDS`
- `src/gateway/translate/openai_chat.rs` — `encode_request / decode_response / TOOL_ERROR_MARKER`
- `src/gateway/translate/mod.rs` — `TranslationRefusal`

Regression evidence:
- `gateway_translate::a_claude_code_request_is_translated_to_chat_completions_and_the_answer_back_with_ids_preserved`
- `gateway_translate::a_request_the_pair_cannot_carry_is_refused_by_name_and_nothing_is_opened_upstream`
- `gateway::translate::openai_chat::tests::an_erroring_tool_result_is_carried_as_a_labelled_tool_message_and_restored`
- `gateway::translate::anthropic::tests::every_refused_request_field_is_refused_by_its_name`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| openai_chat.rs decode_response: `let id = call.require_string("id")?;` -> `let _real_id = call.require_string("id")?; let id = format!("toolu_{index}");` | `swap-tool-use-id` | **killed** | `gateway_translate::a_claude_code_request_is_translated_to_chat_completions_and_the_answer_back_with_ids_preserved` |
| openai_chat.rs: `"tool_calls" | "function_call" => StopReason::ToolUse,` -> `=> StopReason::EndTurn,` | `drop-stop-reason-mapping` | **killed** | `gateway_translate::a_streamed_request_is_translated_event_by_event_in_anthropics_order_and_terminated` |

> swap-tool-use-id observed: test ... FAILED; panicked at tests/gateway_translate.rs:758: assertion `left == right` failed: the tool_use id is the fixture's tool_call id, verbatim

> drop-stop-reason-mapping observed: test ... FAILED; panicked at tests/gateway_translate.rs:901: assertion `left == right` failed (message_delta stop_reason)

Recorded scope limits — stated by the worker, not discovered later:
- Same blocked launch link as 1948: the fidelity is proven through the real gateway door and real sockets, not yet under a real launched harness; closes with 1948.
- is_error crosses OpenAI Chat as a labelled first-line content marker (no wire field exists); cache_control is refused per the packet, so a default Claude Code needs DISABLE_PROMPT_CACHING=1 — both flagged for orchestrator ruling.

---

### Cover each supported harness/provider/protocol pairing with an end-to-end test through the shipped binary against a fixture upstream before offering it. (line 1956)

Contract: Given a harness/provider/protocol pairing, when it is offered as supported, Glasshouse has covered it end to end against a fixture upstream before offering it, while preserving that unoffered pairs stay refused by name.

State: PARTIALLY VERIFIED — ruled 2026-08-31 by the orchestrator, agreeing with the worker's `open`: this line quantifies over every supported pair, and exactly one exists. T1's evidence stands in this entry (the canonical form, both codecs, the seam that only ever enters on the target the provider does not serve, and the end-to-end test through the shipped binary for Claude Code on an OpenAI-Chat entitlement — ids preserved, streaming in Anthropic's order, refusal by name with nothing opened upstream, byte-for-byte relay untouched). Successors: T2 (openai-responses codec) and T3 (Gemini codec + adapter).

Production evidence:
- `src/gateway/translate/mod.rs` — `TABLE (the supported row exists only with its test)`

Regression evidence:
- `gateway_translate.rs (a)-(e): 6 tests, fixture speaks only openai-chat and records requests`
- `gateway_translate::the_shipped_binary_still_refuses_claude_code_on_a_chat_only_entitlement_at_profile_resolution`

Recorded scope limits — stated by the worker, not discovered later:
- The test enters at gateway::start_if_required_with_degrade_sink (the binary's own door, real accept loop, real sockets, production gateway_upstream), NOT at `glasshouse launch` — blocked by the profile link; the witness test converts the day it lifts. 'Through the shipped binary' is therefore not yet literally satisfied, which is why this stays open.

---

---

### Line 1951 — per-harness task efficiency, read from rows production already writes

Package `GH-HARNESS-EFFICIENCY`, 2026-08-31, Sonnet at high (Amber). Two readers in the `outcomes_by_tier` shape: `outcomes_by_tier_and_harness` (outcome by task class per harness, same minimum-sample gate, `undecided` never counted as failed) and `request_stats_by_harness` (request count, wall-clock sum/median, tokens where present with `token_rows_present` beside them), rendered by `harness_efficiency_section` in `glasshouse route`'s report so a harness can be compared for a task class without knowing which vendor bills.
### Record per-harness task efficiency — tokens, wall-clock, request count, and outcome by task class — so that harness choice can rest on evidence rather than on which vendor bills for it. (line 1951)

Contract: Given a window of routing_observations and evaluation_observations rows, when Glasshouse renders glasshouse route, it reports per-harness token totals (over rows that have them, with the count of rows that do not), wall-clock sum/median (over rows carrying both dispatched_at and completed_at), request count, and outcome-by-task-class (gated at MIN_SAMPLE_FOR_SUMMARY, undecided never counted as failed), while preserving that a harness or task class with no rows is carried with its count rather than hidden or rendered as zero

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's artifacts (4/4 mutations KILLED with killing tests named; real `test result:` lines; three blast runs with every red attributed — a doc-link defect fixed, the rest pre-existing load flakes — and `integrate.sh`'s merged-tree blast, see the commit). Tokens are carried where a row has them and `token_rows_present == 0` never prints a zero (`print-zero-for-null-tokens` KILLED); token data begins arriving with translated pairs (T1).

Production evidence:
- `crates/glasshouse/src/evaluation/mod.rs` — `EvaluationObservations::outcomes_by_tier_and_harness`
- `crates/glasshouse/src/evaluation/mod.rs` — `HarnessTierOutcome`
- `crates/glasshouse/src/routing/evidence.rs` — `EvidenceLedger::request_stats_by_harness`
- `crates/glasshouse/src/routing/evidence.rs` — `HarnessRequestStats, HarnessRequestStats::from_rows, WallClockSummary`
- `crates/glasshouse/src/main.rs` — `harness_efficiency_section`
- `crates/glasshouse/src/main.rs` — `route_report (wiring)`

Regression evidence:
- `harness_efficiency::outcomes_by_tier_and_harness_and_request_stats_by_harness_join_by_the_right_key`
- `harness_efficiency::the_route_command_prints_the_harness_efficiency_section`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| EvaluationKind::RoutingTierObserved.as_str(), -> EvaluationKind::RoutingCostClassObserved.as_str(), (evaluation/mod.rs, outcomes_by_tier_and_harness) | `join-wrong-key-harness` | **killed** | `harness_efficiency::outcomes_by_tier_and_harness_and_request_stats_by_harness_join_by_the_right_key` |
| failed: counts.failed, -> failed: counts.failed + counts.sessions_without_outcome, (evaluation/mod.rs, TierOutcome::from_counts) | `count-undecided-as-failed-harness` | **killed** | `harness_efficiency::outcomes_by_tier_and_harness_and_request_stats_by_harness_join_by_the_right_key` |
| Some(stats) if stats.token_rows_present == 0 => format!( -> Some(stats) if false => format!( (main.rs, harness_efficiency_section) | `print-zero-for-null-tokens` | **killed** | `harness_efficiency::the_route_command_prints_the_harness_efficiency_section` |

> join-wrong-key-harness observed: no `heavy`/`leaf` bucket found among the cost-class vocabulary the mutated query now reads; both harness_efficiency tests panicked

> count-undecided-as-failed-harness observed: assertion left == right failed: claude-code/heavy's two undecided sessions were folded into failed: TierOutcome { bucket: "heavy", undecided: 2, verdict: Measured { successful: 5, failed: 2, sample_size: 5 } }

> print-zero-for-null-tokens observed: the codex row's expected exact text 'tokens: not exposed on 3 of 3 exchanges' no longer appeared in stdout

Recorded scope limits — stated by the worker, not discovered later:
- no Windows leg run
- sessions.harness fallback-to-unknown path shares route_outcomes_by_pairing_class's already-tested behavior rather than a dedicated new test
- wall-clock sum/median is not gated by MIN_SAMPLE_FOR_SUMMARY, per the packet's own wording

---

---

### Lines 1962, 1963, 1964, 1973 — the entitlement is the unit of capacity (56A step 1), and 1947's job-kind clause gains its consumer

Package `GH-ENTITLEMENT-POOL`, 2026-08-31, Fable 5 at xhigh (Red). The rename subscription→entitlement, complete and alias-free (the table was under a day old; an old `[subscriptions]` table is silently ignored — unknown keys are ignored by the loader, recorded); `[entitlements.<name>]` with `kind`, descriptive `vendor`, its OWN credential as a reference in exactly two shapes (env var name / OS service+account — a value is refused by name and never echoed by this crate's own message), optional `native_harness`; several entries of one vendor and kind coexist as distinct registry resources (`ResourceKind::Entitlement { name }`) with capacity slots pinned `unknown` until 56A-2; the five layers (harness / protocol adapter / authentication / entitlement / model) each varied alone in tests; and the disposable router now consults the entitlement's job-kind rules by name — 1947's third clause's consumer, landing as `phase-56.md`'s 1947 entry promised. Notable degradation ruling (the worker's, accepted): a contradictory `[entitlements]` table refuses a LAUNCH outright but degrades SUPPORT-WORK candidates to no-entitlement with a warning — failing memory extraction over a config contradiction would punish the wrong action.

### Model an entitlement — a specific subscription or API-credit account such as Claude Max A, Claude Max B, ChatGPT Pro, OpenRouter credits, or an API key — as the unit of capacity, distinct from the vendor, the provider adapter, the wire protocol, and the harness. (line 1962)

Contract: Given the user's `[entitlements.<name>]` tables, when Glasshouse resolves the resource a session or a support job would be charged to, it treats each entry as an entitlement — a specific subscription or API-credit account with an optional kind, an optional billing vendor, its own credential REFERENCE and an optional native_harness, distinct from the vendor, the provider adapter, the wire protocol and the harness — while preserving that a user who configured nothing sees the same defaults, the same launches and the same refusals under the new name.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's artifacts (6/6 mutations KILLED with failure text quoted; every renamed test green with counts; blast radius 83/84 targets, 2735 passed, the 84th being the deleted subscription_rules.rs; the residual-`subscription` grep catalogued line by line) and the decision diffs read at review.

Production evidence:
- `src/config/mod.rs` — `EntitlementConfig (kind, vendor, credential, native_harness, provider, six rule lists), EntitlementKind, EntitlementVendor, EntitlementCredential, EntitlementTable, EntitlementBacking, ResolvedEntitlement, EntitlementLookupError`
- `src/config/mod.rs` — `EffectiveConfig::entitlements/entitlement_for (renamed, same resolution; NativeSignInWithOwnCredential and SharedCredential added)`
- `src/routing/mod.rs` — `Entitlement, EntitlementRules, EntitlementRefusal, HardConstraint::Entitlement { entitlement, refused }`
- `src/routing/session.rs` — `Destination::with_entitlement/entitlement; hard_constraint unchanged in behaviour`
- `src/main.rs` — `destination_entitlement, announce_entitlement ("entitlement `X` (…) will serve this session."), both launch guards' texts naming [entitlements.<name>]`

Regression evidence:
- `entitlements (15 tests — every renamed subscription-rules test green under the new names, binary halves included)`
- `config::tests::entitlements_round_trip_and_resolve_project_over_user_with_a_native_default`
- `config::tests::contradictory_entitlement_tables_are_refused_by_name`
- `entitlement_pool::only_the_two_reference_shapes_deserialise_and_a_value_is_refused_by_name`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| crates/glasshouse/src/config/mod.rs: user-layer lookup `.get(name)` -> `.iter().map(|(_, config)| config).next()` (every name resolves to the first entry's config) | `wrong-source` | **killed** | `resolving_two_entitlements_yields_each_its_own_value_and_never_the_others` |

> wrong-source observed: panicked at crates/glasshouse/tests/entitlement_pool.rs:283:10 — resolution crossed accounts (SharedCredential fired); 9 tests FAILED including both launch-isolation binary tests

Recorded scope limits — stated by the worker, not discovered later:
- an old `[subscriptions.<name>]` table is silently ignored after the rename (unknown keys are deliberately ignored on load); the packet ruled no alias — the table is under a day old
- `vendor` is descriptive: read by describe()/the announcement only, keyed on by nothing — deliberately, per line 1962's 'distinct from the vendor'

---

### Allow several entitlements of the same vendor and plan to coexist in one pool, each with its own authentication, remaining capacity, and reset time. (line 1963)

Contract: Given two `[entitlements]` entries of one vendor and one kind, each with its own credential reference, when Glasshouse resolves and enumerates its resources, both coexist — EffectiveConfig::entitlements returns both, ResourceKind::Entitlement { name } lists each configured account as its own resource, glasshouse status prints both by name, and each carries its own remaining-capacity and reset-time slots — while preserving that the slots read unknown (None), never full or empty, until 56A package 2 populates them, and that nothing anywhere dedupes by vendor.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's artifacts (6/6 mutations KILLED with failure text quoted; every renamed test green with counts; blast radius 83/84 targets, 2735 passed, the 84th being the deleted subscription_rules.rs; the residual-`subscription` grep catalogued line by line) and the decision diffs read at review.

Production evidence:
- `src/provider/registry.rs` — `ResourceKind::Entitlement { name } (+ locality/label arms)`
- `src/provider/quota.rs` — `CapacityState::for_resource's Entitlement arm (opaque; no launch path reaches it)`
- `src/config/mod.rs` — `EffectiveConfig::configured_entitlements, ::entitlement_resources (one resource per entry, keyed by name)`
- `src/config/mod.rs` — `ResolvedEntitlement::{credential, vendor, remaining_capacity, seconds_until_reset}`
- `src/main.rs` — `status_report's Entitlements line (the enumeration's production caller)`

Regression evidence:
- `entitlement_pool::two_entitlements_of_one_vendor_and_kind_coexist_as_distinct_resources`
- `entitlement_pool::status_lists_both_accounts_of_one_vendor_by_name (binary)`
- `entitlement_pool::shared_credentials_and_native_sign_ins_with_credentials_are_refused_by_name`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| crates/glasshouse/src/config/mod.rs: .filter(|entry| entry.layer() != Layer::Default) -> same + .scan(BTreeSet::new(), dedupe by entry.vendor()).flatten() | `skip-state-update` | **killed** | `two_entitlements_of_one_vendor_and_kind_coexist_as_distinct_resources` |

> skip-state-update observed: panicked at crates/glasshouse/tests/entitlement_pool.rs:76:5 — assert_eq!(configured.len(), 2); status_lists_both_accounts_of_one_vendor_by_name FAILED on the binary too (claude-b vanished)

Recorded scope limits — stated by the worker, not discovered later:
- the slots have no producer by design (56A package 2); the test pins None — unknown, never full or empty
- own-credential accounts with no backing are listed and never charged: no launch profile resolves to one until the broker packages place work on them

---

### Keep the layering explicit and separately replaceable: harness, protocol adapter, authentication, entitlement, inference model. (line 1964)

Contract: Given the five layers — harness (IntegrationId), protocol adapter (WireProtocol from the provider template), authentication (the credential reference), entitlement (the entry), inference model (LaunchProfile.model) — when any one is varied, the other four stand: the same entitlement serves two harnesses, the same harness runs under two entitlements, the same entitlement serves two models, and one vendor and protocol stand behind two credentials — while preserving that the layering is stated on the entitlement type itself as documentation the next phase can hold to.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's artifacts (6/6 mutations KILLED with failure text quoted; every renamed test green with counts; blast radius 83/84 targets, 2735 passed, the 84th being the deleted subscription_rules.rs; the residual-`subscription` grep catalogued line by line) and the decision diffs read at review.

Production evidence:
- `src/config/mod.rs` — `EntitlementConfig's five-layer doc (names each layer and which field is which)`
- `src/config/mod.rs` — `EffectiveConfig::entitlement_for (keys on backing, blind to model; harness read only for the Native arm)`

Regression evidence:
- `entitlement_pool::the_same_entitlement_serves_two_harnesses`
- `entitlement_pool::the_same_harness_runs_under_two_entitlements`
- `entitlement_pool::the_same_entitlement_serves_two_models`
- `entitlement_pool::one_vendor_and_protocol_stand_behind_two_credentials`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| crates/glasshouse/src/config/mod.rs: user-layer lookup `.get(name)` -> first entry's config for every name (the same mutation as 1962's, which collapses the entitlement layer) | `wrong-source` | **killed** | `the_same_harness_runs_under_two_entitlements` |

> wrong-source observed: panicked at crates/glasshouse/tests/entitlement_pool.rs:417:10 — the two backings no longer resolved to two accounts

Recorded scope limits — stated by the worker, not discovered later:
- every layer varied alone today — no coupling found, so no line is left open on this ground
- the protocol-adapter layer is varied structurally (two providers of one template), not by a wire-protocol assertion: entitlement entries never name a protocol, which is the separation itself

---

### Keep every entitlement's credential isolated: tokens and keys never mixed across accounts, never logged, never written into a project file. (line 1973)

Contract: Given entitlements with their own credentials, when Glasshouse loads, prints, resolves, launches or writes configuration, no credential value can be expressed in the config schema (only SecretRef's two reference shapes deserialise, everything else refused by name), no Debug/Display of an entitlement type contains a resolved value, each account resolves its own value and never another's, a launch's child environment carries only the serving account's variable (every other entitlement's env-shaped credential is scrubbed before the overlay applies, on the fresh and the resume path), and the project-config writer serialises references only — while preserving that a launch no entitlement serves carries no account's variable at all.

State: PARTIALLY VERIFIED — ruled 2026-08-31 by the orchestrator, **box NOT ticked**, over the worker's `closed`: the worker's own first limit is a reachable mixing path — shell-started sessions (`src/shell/mod.rs:1162`, `:1296`, outside the packet's files) build their own `HarnessLaunch` and do NOT scrub foreign entitlement credential variables, so a session started from the TUI can carry both accounts' variables in its child environment. Everything else about the line is proven (reference-only serde with refusal by name and no echo; redacting Debug; per-account resolution; the scrub at both `main.rs` launch sites, binary-tested against the child's own environment dump; the reference-only project-config writer). Successor dispatched the same hour: `entitlement-env-scrub` — apply `foreign_entitlement_credential_vars` at the two shell sites with a test; tick on its landing.

Production evidence:
- `src/config/mod.rs` — `EntitlementCredential (manual serde: two shapes, refusal by name, no echo; manual Debug: names only)`
- `src/config/mod.rs` — `EntitlementLookupError::{SharedCredential, NativeSignInWithOwnCredential}`
- `src/main.rs` — `foreign_entitlement_credential_vars + env_remove loops at both HarnessLaunch sites (launch_session, resume_session), before overlay.apply`
- `src/config/mod.rs` — `write_project_config_with_consent (pre-existing writer; the credential field serialises as its reference)`

Regression evidence:
- `entitlement_pool::only_the_two_reference_shapes_deserialise_and_a_value_is_refused_by_name`
- `entitlement_pool::the_refusal_message_this_crate_writes_never_contains_the_value`
- `entitlement_pool::debug_of_every_entitlement_type_never_contains_a_resolved_value`
- `entitlement_pool::resolving_two_entitlements_yields_each_its_own_value_and_never_the_others`
- `entitlement_pool::a_launch_under_one_entitlement_never_carries_the_other_accounts_variable (binary; the child's own environment dump)`
- `entitlement_pool::a_launch_no_entitlement_serves_carries_no_accounts_variable (binary)`
- `entitlement_pool::the_project_config_writer_serialises_references_and_never_values`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| crates/glasshouse/src/main.rs: launch_session scrub body `launch = launch.env_remove(var);` -> `let _ = var;` | `remove-guard` | **killed** | `a_launch_under_one_entitlement_never_carries_the_other_accounts_variable` |
| crates/glasshouse/src/config/mod.rs: credential Debug `write!(f, "environment variable `{var}`")` -> `write!(f, …, std::env::var(var).unwrap_or_default())` | `wrong-source` | **killed** | `debug_of_every_entitlement_type_never_contains_a_resolved_value` |
| crates/glasshouse/src/config/mod.rs: user-layer lookup `.get(name)` -> first entry's config for every name | `wrong-source` | **killed** | `resolving_two_entitlements_yields_each_its_own_value_and_never_the_others` |

> remove-guard observed: panicked at crates/glasshouse/tests/entitlement_pool.rs:871:5 — the child's environment dump contained claude-b's variable; a_launch_no_entitlement_serves_carries_no_accounts_variable FAILED too

> wrong-source observed: panicked at crates/glasshouse/tests/entitlement_pool.rs:257:5 — the planted value appeared in the rendering

> wrong-source observed: panicked at crates/glasshouse/tests/entitlement_pool.rs:283:10 — claude-b no longer resolved its own value

Recorded scope limits — stated by the worker, not discovered later:
- shell-started sessions (src/shell/mod.rs:1162, :1296 — outside this packet's files) build their own HarnessLaunch and do not scrub; successor: apply foreign_entitlement_credential_vars at those two sites
- the TOML library's error rendering quotes the offending config line, so a value a user pastes into `credential = ` is echoed once, by the parser's snippet, in the load error — the refusal sentence itself never repeats it (pinned by the_refusal_message test); fixing the snippet means reformatting every config parse error
- only env-shaped references are scrubbed; an OsCredential reference has no variable to leak, and provider credential_env pools not named by any entitlement are untouched (out of this line's scope)
- macOS only; the #[cfg(windows)] fake-harness arm mirrors tests/entitlements.rs's and is untested here

---

### Allow a subscription rule to state which harnesses, workload tiers, and job kinds the subscription may serve, and which it must never serve. (line 1947)

Contract: Given an entitlement whose rules state allow_job_kinds or deny_job_kinds, when Glasshouse's disposable router picks a resource for a JobKind, a candidate whose entitlement does not serve that kind is never a candidate, and the refusal names the entitlement and the job kind exactly as the session router's refusals name an entitlement and a harness or tier — while preserving that a candidate with no entitlement is never refused by one, that no scoring changed, and that when every candidate is refused the error names each rule rather than misreporting the pool as exhausted.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's artifacts (6/6 mutations KILLED with failure text quoted; every renamed test green with counts; blast radius 83/84 targets, 2735 passed, the 84th being the deleted subscription_rules.rs; the residual-`subscription` grep catalogued line by line) and the decision diffs read at review.

Production evidence:
- `src/routing/mod.rs` — `EntitlementRefusal::JobKind, Entitlement::job_constraint`
- `src/routing/disposable.rs` — `DisposableCandidate::with_entitlement/entitlement; the job_constraint check in choose's apply_hard_constraints; refusal notes on the winner's explanation; NoResource::EntitlementDeniesEveryCandidate`
- `src/config/mod.rs` — `EffectiveConfig::entitlement_for_provider (a disposable job has no harness)`
- `src/main.rs` — `disposable_candidates attaches the entitlement per provider (never a silent pre-filter)`

Regression evidence:
- `entitlement_pool::an_entitlement_that_denies_the_job_kind_is_not_a_candidate_and_is_named`
- `entitlement_pool::an_allow_list_omitting_the_job_kind_refuses_it_and_no_entitlement_never_does`
- `entitlement_pool::every_candidate_refused_names_every_entitlement_and_the_job_kind`
- `entitlement_pool::the_shipped_binary_attaches_entitlements_to_support_work_candidates (binary, via glasshouse resources' routing-model block)`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| crates/glasshouse/src/routing/disposable.rs: entitlement.job_constraint(job)?; -> let _ = entitlement.job_constraint(job); | `remove-guard` | **killed** | `an_entitlement_that_denies_the_job_kind_is_not_a_candidate_and_is_named` |
| crates/glasshouse/src/main.rs: .with_entitlement(entitlement.clone()), -> .with_entitlement(None), | `remove-guard` | **killed** | `the_shipped_binary_attaches_entitlements_to_support_work_candidates` |

> remove-guard observed: panicked at crates/glasshouse/tests/entitlement_pool.rs:572:9 — the denied candidate was chosen; the all-refused and allow-list tests FAILED too

> remove-guard observed: panicked at crates/glasshouse/tests/entitlement_pool.rs:925:5 — glasshouse resources stopped naming the refusal from the binary's own candidate list

Recorded scope limits — stated by the worker, not discovered later:
- line 1947 is already ☑ (subscription-rules); this entry records the third clause's consumer landing, per the packet's objective 5 — the orchestrator rules whether the ledger entry is amended or left
- choose_for_automatic_classification's retained arm re-validates health and presence, not entitlement rules; a pick retained before a rule changed could serve one more classification (the fresh path that creates picks applies the rule)
- a contradiction in the [entitlements] tables degrades support-work candidates to no-entitlement with a tracing::warn (a launch is refused outright on the same tables; failing memory extraction over it would punish the wrong actor)

---

---

### T2 addendum to lines 1948, 1950, 1956 — the OpenAI Responses codec and two more pairs

Package `GH-GATEWAY-TRANSLATE-T2`, 2026-08-31, Fable 5 at xhigh (Red). The `openai_responses` codec against T1's canonical form (function-call items <-> tool blocks with ids preserved; instructions <-> system; stop/incomplete reasons; usage), two table rows flipped, the pairing pin's expected set updated in the worker's OWN reviewed diff this time, and T2b (responses<->chat) refused by name until its e2e. 4/4 mutations KILLED with output quoted. Integrator's one seam fix, prescribed verbatim by the report: the T1 e2e block that pinned anthropic->responses as *refused* (a behaviour this package changed) deleted; the refusal-by-name behaviour keeps two live witnesses.

**Three findings recorded, each with a successor:** (1) `protocol_fit`'s translation arm asks the table BACKWARDS — T1's shipped pairing classifies `Incompatible` while the gateway translates it, masked in T2's pairs by symmetry; successor `GH-PROTOCOL-FIT-DIRECTION` (dispatched). (2) A translated request toward an Anthropic-serving provider carries no `anthropic-version` header — tolerated by OpenRouter, required by api.anthropic.com; a per-protocol outbound-header hook is owed before a real Anthropic upstream is used through translation. (3) If real Codex sends `prompt_cache_key`/`include`/`reasoning` unconditionally, pair 2 refuses every real request by name — a live-Codex probe is the successor's first check, as `DISABLE_PROMPT_CACHING=1` was T1's.

### Serve any supported harness through Glasshouse's bundled API gateway from any subscription or model whose wire protocol the gateway can translate to the harness's native protocol. (line 1948)

Contract: Given a supported harness and an entitlement whose wire protocol the gateway can translate to the harness's native protocol, when the harness sends its native request to the bundled gateway, Glasshouse serves it through the translated pair end to end, while preserving the byte-for-byte relay for every natively served target.

State: PARTIALLY VERIFIED — ruled 2026-08-31 by the orchestrator, agreeing with the worker's `open`: the line quantifies over every supported pair; three exist now (T1's anthropic->chat, and T2's anthropic<->responses both ways, each behind its own end-to-end test through the shipped binary — ids preserved, streaming in each protocol's own order, refusal by name with nothing opened upstream, the byte-for-byte relay for served targets re-witnessed). Remaining: T2b (responses<->chat, refused by name), T3 (Gemini codec and adapter).

Production evidence:
- `src/gateway/translate/openai_responses.rs` — `OpenAiResponses (full Codec impl)`
- `src/gateway/translate/mod.rs` — `TABLE (two rows flipped) / outbound_target / serve`
- `src/gateway/ingress.rs` — `unrouted (unchanged, protocol-generic consumer)`

Regression evidence:
- `gateway_translate_responses::a_claude_code_request_is_translated_to_openai_responses_and_back_with_ids_preserved`
- `gateway_translate_responses::a_codex_request_is_translated_to_anthropic_messages_and_back_with_ids_preserved`
- `gateway_translate_responses::a_served_responses_target_is_relayed_byte_for_byte_even_though_the_codec_exists`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| ingress.rs: before `let Some(uri) = route.uri_for(&head.target) else {`, insert `if let translate::Placement::Translate(pair) = translate::place(&head.target, &["anthropic-messages"]) { return translate::serve(head, reader, out, upstream, serving, agent, pair); }` | `served-target-enters-codec` | **killed** | `gateway_translate_responses::a_served_responses_target_is_relayed_byte_for_byte_even_though_the_codec_exists` |
| translate/mod.rs TABLE: openai-responses->openai-chat `PairStatus::Refused(NOT_YET_T2B)` -> `PairStatus::Supported` | `refused-pair-marked-supported` | **killed** | `harness::pairing::tests::exactly_the_supported_pairs_are_translated` |

> served-target-enters-codec observed: panicked at tests/gateway_translate_responses.rs:164: assertion `left == right` failed: exactly one request at the fixture (the mutated gateway refused the relay-only body; the fixture saw nothing)

> refused-pair-marked-supported observed: panicked at src/harness/pairing.rs:1353: assertion `left == right` failed: openai-responses -> openai-chat: the translation table disagrees with this pin

Recorded scope limits — stated by the worker, not discovered later:
- Same launch link as T1: proven through gateway::start_if_required_with_degrade_sink with production profile::gateway_upstream, not under `glasshouse launch` — profile::apply_gateway still refuses (T1's witness test, still green); this line stays open until that packet lands
- The line quantifies over every translatable pair; Gemini (T3) and openai-chat<->openai-responses (T2b) remain refused by name
- Only macOS ran

---

### Keep a harness's native tooling — editing, shell, repository, and tool-call behaviour — intact when it is served by a non-native provider, and refuse the pairing by name when it cannot be kept. (line 1950)

Contract: Given a harness served through a translated pair, when tool definitions, tool calls, tool results (erroring included), parallel calls, stop reasons and system prompts cross the gateway, Glasshouse preserves them with ids verbatim in both directions, while refusing per request, by field name and before anything is opened upstream, whatever the pair cannot carry.

State: PARTIALLY VERIFIED — ruled 2026-08-31 by the orchestrator, agreeing with the worker's `open`: the line quantifies over every supported pair; three exist now (T1's anthropic->chat, and T2's anthropic<->responses both ways, each behind its own end-to-end test through the shipped binary — ids preserved, streaming in each protocol's own order, refusal by name with nothing opened upstream, the byte-for-byte relay for served targets re-witnessed). Remaining: T2b (responses<->chat, refused by name), T3 (Gemini codec and adapter).

Production evidence:
- `src/gateway/translate/openai_responses.rs` — `decode_request / encode_request / decode_response / encode_response / EventDecoder / EventEncoder / REFUSED_FIELDS`
- `src/gateway/translate/mod.rs` — `Codec::refuse_unencodable (new hook) + serve's call to it`

Regression evidence:
- `gateway_translate_responses::a_claude_code_request_is_translated_to_openai_responses_and_back_with_ids_preserved`
- `gateway_translate_responses::a_streamed_claude_code_request_is_translated_event_by_event_in_anthropics_order`
- `gateway_translate_responses::a_streamed_codex_request_is_translated_event_by_event_in_the_responses_order`
- `gateway_translate_responses::a_request_the_responses_pair_cannot_carry_is_refused_by_name_and_nothing_opens_upstream`
- `gateway_translate_responses::a_codex_request_the_pair_cannot_carry_is_refused_by_name_and_nothing_opens_upstream`
- `gateway::translate::openai_responses::tests::every_refused_request_field_is_refused_by_its_name`
- `gateway::translate::openai_responses::tests::a_request_round_trips_through_the_openai_responses_wire (and response/stream round-trips)`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| openai_responses.rs decode_output_item: `blocks.push(Block::ToolUse { id, name, input });` -> `blocks.push(Block::ToolUse { id: format!("call_minted_{}", blocks.len()), name, input });` | `swap-function-call-id` | **killed** | `gateway_translate_responses::a_claude_code_request_is_translated_to_openai_responses_and_back_with_ids_preserved` |
| openai_responses.rs stop_reason_of: `.any(|block| matches!(block, Block::ToolUse { .. }))` -> `.any(|_block| false)` | `drop-stop-reason-mapping` | **killed** | `gateway_translate_responses::a_claude_code_request_is_translated_to_openai_responses_and_back_with_ids_preserved` |

> swap-function-call-id observed: panicked at tests/gateway_translate_responses.rs:821: assertion `left == right` failed: the tool_use id is the fixture's call_id, verbatim

> drop-stop-reason-mapping observed: panicked at tests/gateway_translate_responses.rs:818: assertion `left == right` failed (the client's stop_reason must be tool_use)

Recorded scope limits — stated by the worker, not discovered later:
- is_error crosses as the TOOL_ERROR_MARKER first-line convention (no wire field exists on function_call_output), same decision as T1's chat codec
- StopSequence cannot be said on the Responses wire (no stop sequences exist there); unreachable through the pair since no decodable request can set one — requests carrying stop_sequences are refused by name via refuse_unencodable
- An empty reasoning output item is skipped by name; a non-empty one is refused — a Responses upstream running a reasoning model at non-default include settings will be refused per request

---

### Cover each supported harness/provider/protocol pairing with an end-to-end test through the shipped binary against a fixture upstream before offering it. (line 1956)

Contract: Given a harness/provider/protocol pairing, when it is offered as supported, Glasshouse has covered it end to end against a fixture upstream before offering it, while preserving that unoffered pairs stay refused by name.

State: PARTIALLY VERIFIED — ruled 2026-08-31 by the orchestrator, agreeing with the worker's `open`: the line quantifies over every supported pair; three exist now (T1's anthropic->chat, and T2's anthropic<->responses both ways, each behind its own end-to-end test through the shipped binary — ids preserved, streaming in each protocol's own order, refusal by name with nothing opened upstream, the byte-for-byte relay for served targets re-witnessed). Remaining: T2b (responses<->chat, refused by name), T3 (Gemini codec and adapter).

Production evidence:
- `src/gateway/translate/mod.rs` — `TABLE (each supported row commented with its e2e test)`

Regression evidence:
- `tests/gateway_translate_responses.rs: 7 tests, fixtures speak only the provider's protocol and record requests`
- `harness::pairing::tests::exactly_the_supported_pairs_are_translated (pin, three ordered pairs)`
- `gateway::translate::tests::exactly_the_supported_pairs_are_supported_and_every_other_row_carries_a_reason`
- `provider::tests::every_wire_protocol_pair_has_exactly_one_row_in_the_gateway_table`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| (same mutation as line 1948's second row — one mutation, two lines it defends) | `refused-pair-marked-supported` | **killed** | `harness::pairing::tests::exactly_the_supported_pairs_are_translated` |

> refused-pair-marked-supported observed: panicked at src/harness/pairing.rs:1353 (see above)

Recorded scope limits — stated by the worker, not discovered later:
- 'Through the shipped binary' is satisfied to exactly T1's depth: the binary's own gateway door, real sockets, production upstream builder — not `glasshouse launch` (blocked at profile::apply_gateway, witness still green); same reason T1's ruling left these PARTIALLY VERIFIED

---

#### T2 finding 1 resolved same day: the classifier asks the table in the harness's direction

Package `GH-PROTOCOL-FIT-DIRECTION`, 2026-08-31, Sonnet at high (Amber). One arm in `harness::pairing::protocol_fit`: the translation lookup now asks `translation_available(spoken_protocol, route)` — the harness's own direction — so T1's shipped pairing (a harness speaking anthropic-messages on a chat-only route) classifies `Translated` instead of `Incompatible`, and the asymmetric witness (openai-chat spoken on an anthropic-only route) stays `Incompatible`. Both direction-mutations KILLED with distinguishing panics; the four affected suites and the full blast radius green.

#### 1973's missing clause closed same day: the shell launch sites scrub

Package `GH-ENTITLEMENT-ENV-SCRUB`, 2026-08-31, Sonnet at high (Amber). `foreign_entitlement_credential_vars` moved from the binary onto `EffectiveConfig` (the library, so the shell can reach it); both shell launch sites (`start_session`, `resume_session`) resolve the serving entitlement and scrub the other accounts' variables before their `HarnessLaunch` starts; the `main.rs` call sites now call the moved method, behaviour re-verified by 56A-1's own binary tests. 2/2 mutations KILLED with the leaking variable visible in the observed env dumps; full lib (1735) and a 74-target blast green. Recorded limits: the shell's `resume_session` shares the covered call rather than owning a PTY-driven unit test; an `entitlement_for` error at a shell site degrades to "no serving entitlement" (logged) rather than refusing, per the no-behaviour-change rule — the CLI path still refuses. State for line 1973: **COMPLETE** — ruled 2026-08-31 by the orchestrator; the PARTIALLY VERIFIED hold above is discharged.

#### T2b addendum: the chat<->responses pairs land, and the outbound gains its per-protocol header

Package `GH-GATEWAY-TRANSLATE-T2B`, 2026-08-31, Sonnet at high (Amber — both codecs were settled). Five supported ordered pairs now — anthropic→chat, anthropic→responses, responses→anthropic, chat→responses, responses→chat — with `openai-chat -> anthropic-messages` the one refused row left. Each new pair behind its own e2e with ids preserved and streaming in the client protocol's order; `serve` gained the per-protocol outbound-header hook (T2 finding 2): `anthropic-version` added exactly when the outbound protocol is anthropic-messages, asserted present there and absent elsewhere. 3/3 mutations KILLED. Integrator's seam fixes, all five prescribed verbatim by the report and applied on the merged tree (the table flip re-keyed every fixture that used chat<->responses to witness `Incompatible`): the stale T2b-refusal sub-check deleted, the rungs and hard-constraint witnesses moved to OpenCode-on-anthropic-only (the one refused row), and the provider table pin extended to five pairs. Lines 1948/1950/1956 remain open pending T3 (Gemini).

---

### Line 1965 — per-entitlement telemetry, every reading carrying its scope (56A step 2)

Package `GH-ENTITLEMENT-TELEMETRY`, 2026-08-31, Fable 5 at xhigh (Red). One resolver (`configured_entitlements_with_telemetry`) populates 56A-1's pinned-unknown slots and two new facets, each reading scoped honestly: capacity and reset from the gateway quota cache (provider-wide in every reachable case, and SAID so — a per-account reading needs a per-credential key on the write side, deliberately not widened here); recent throttling narrowed to the account where `quota_context` rows carry the credential label, widened to provider scope by any contextless row, and `ThrottleScope` finally gains its `AccountSpecific` variant — the entitlement is the key its own doc said did not exist; the models facet reads the provider's declared catalogue and answers `HarnessDecided` for a native sign-in, never an invented list. `glasshouse status` renders all four facets with `unknown` spelled out and shared readings marked provider-wide.

### Track, per entitlement, remaining capacity, time until reset, recent throttling, and the models it can serve, from the telemetry the provider actually exposes. (line 1965)

Contract: Given configured entitlements, when Glasshouse renders its status, each entry tracks remaining capacity, time until reset, recent throttling, and the models it can serve, read only from telemetry the provider actually exposes — the gateway's cached per-provider rate-limit headers, the ledger's quota_context-keyed throttle rows, the provider's own fetched model catalogue — each reading carrying its scope (per-account only where keyed by this account's own credential, provider-wide where every entitlement of the provider shares it), while preserving that an entitlement with no telemetry reads unknown — a rendered word, never full, never empty, never a number — and that the credential LABEL is the only account identifier read or displayed.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's artifacts (5/5 mutations KILLED, one only after the worker found and fixed its own masked test and re-ran with the wider command — recorded as history; thirteen regression tests including two through the shipped binary; the scope words asserted in the rendered output) and the decision diffs read at review. The recorded limits are 56A-3+'s ground: per-account CAPACITY needs a per-credential key on the gateway's write side; `glasshouse resources` still renders the entitlement kind opaque.

Production evidence:
- `src/config/mod.rs` — `EffectiveConfig::configured_entitlements_with_telemetry`
- `src/config/mod.rs` — `ResolvedEntitlement::with_telemetry / ::populate_provider_facets / ::credential_label / ::capacity_scope / ::throttling / ::models`
- `src/config/mod.rs` — `TelemetryScope, EntitlementThrottleReading, EntitlementModels, EntitlementTelemetry`
- `src/routing/evidence.rs` — `recent_credential_throttles, CredentialThrottles`
- `src/routing/evidence.rs` — `ThrottleScope::AccountSpecific, count_throttles_against_other_accounts, classify_throttle_scope (account axis)`
- `src/routing/evidence.rs` — `EvidenceLedger::observations_in_window`
- `src/main.rs` — `status_report (the sources read once, the resolver called) + entitlement_facets (the rendered line)`
- `src/main.rs` — `throttle_scope_section (the AccountSpecific arm)`

Regression evidence:
- `entitlement_telemetry::two_entitlements_of_one_provider_share_the_same_provider_wide_capacity_reading`
- `entitlement_telemetry::an_entitlement_with_no_telemetry_stays_unknown_on_every_facet`
- `entitlement_telemetry::the_models_facet_reads_the_declared_catalogue_and_never_invents_one_for_native`
- `entitlement_telemetry::the_throttle_facet_narrows_to_the_account_and_reads_only_this_providers_rows`
- `entitlement_telemetry::a_contextless_throttle_widens_both_accounts_readings_to_provider_scope`
- `entitlement_telemetry::status_shows_all_four_facets_with_their_scope_through_the_shipped_binary (binary)`
- `entitlement_telemetry::status_spells_unknown_for_an_entitlement_nothing_measured (binary)`
- `routing::evidence::throttle_scope_tests::sibling_throttles_beside_another_account_serving_read_as_account_specific`
- `routing::evidence::throttle_scope_tests::a_throttle_shared_by_two_accounts_stays_provider_wide`
- `routing::evidence::throttle_scope_tests::contextless_rows_never_produce_an_account_specific_verdict`
- `routing::evidence::credential_throttle_tests::every_row_naming_its_account_narrows_the_count_to_the_credential`
- `routing::evidence::credential_throttle_tests::a_contextless_throttle_row_widens_the_reading_to_provider_scope`
- `routing::evidence::credential_throttle_tests::only_informative_throttles_count_and_zero_is_provider_wide`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| config/mod.rs: 'self.remaining_capacity = state.remaining_capacity_score();' -> 'self.remaining_capacity = if self.name.ends_with("-a") { state.remaining_capacity_score() } else { None };' | `wrong-scope` | **killed** | `entitlement_telemetry::two_entitlements_of_one_provider_share_the_same_provider_wide_capacity_reading` |
| main.rs: 'None => "capacity: unknown".to_owned(),' -> 'None => "capacity: 100%".to_owned(),' | `fabricate-unknown` | **killed** | `entitlement_telemetry::status_spells_unknown_for_an_entitlement_nothing_measured` |
| evidence.rs: '.filter(|row| row.provider == provider)' -> '.filter(|_row| true)' | `wrong-source` | **killed** | `routing::evidence::credential_throttle_tests::every_row_naming_its_account_narrows_the_count_to_the_credential` |
| config/mod.rs: 'self.models = Some(EntitlementModels::HarnessDecided);' -> 'self.models = Some(EntitlementModels::Declared { models: vec!["claude-3-opus".to_owned(), "alpha-m1".to_owned()], scope: TelemetryScope::ProviderWide });' | `invent-list` | **killed** | `entitlement_telemetry::the_models_facet_reads_the_declared_catalogue_and_never_invents_one_for_native` |
| evidence.rs: 'if cross_served > 0 {' -> 'if false {' | `skip-state-update` | **killed** | `routing::evidence::throttle_scope_tests::sibling_throttles_beside_another_account_serving_read_as_account_specific` |

> wrong-scope observed: assertion `left == right` failed: one provider, one reading — the two accounts share it verbatim (tests/entitlement_telemetry.rs:116); the binary test failed at :514 too

> fabricate-unknown observed: panicked at tests/entitlement_telemetry.rs:568:5 (unknown is a rendered word / no % may appear); status_shows_all_four_facets failed at :546

> wrong-source observed: SURVIVED against --test entitlement_telemetry alone (7 passed — the account narrowing masks the provider filter); test strengthened and re-run with --lib in the command: assertion `left == right` failed: KEY_A's own rows, not KEY_B's and not beta's (evidence.rs:4998)

> invent-list observed: assertion `left == right` failed: a native sign-in's models are the harness's decision — no list, ever (tests/entitlement_telemetry.rs:232); binary test failed at :548

> skip-state-update observed: assertion `left == right` failed: account A's models throttled together while account B kept serving (evidence.rs:4886)

Recorded scope limits — stated by the worker, not discovered later:
- capacity/reset readings are provider-wide in every reachable case: the gateway quota cache is keyed by provider and its writer is settled — PerAccount capacity needs a per-credential key on the write side (56A-3+)
- the facets reach production through `glasshouse status` only; `glasshouse resources` still renders ResourceKind::Entitlement opaque
- models facet trusts the cached catalogue without re-checking the provider's current base URL
- AccountSpecific still gates on MIN_CORRELATION_SAMPLE sibling-informative events; cross-account evidence alone reads Unknown
- macOS-only run; nothing added is platform-conditional

---

### Lines 1953, 1966, 1967, 1968, 1969 — the entitlement pool enters the candidate set and the broker scores it (56A step 3)

Package `GH-ENTITLEMENT-BROKER`, 2026-08-31, Fable 5 at xhigh (Red). The pool becomes the candidate set: for a fresh destination on a harness, one candidate per configured entitlement whose rules allow it (`pool_entitlements_for`, `routing_destinations`), the same harness ranked across every account that may serve it, a model the entitlement cannot serve refused by name (`Entitlement::model_constraint`), and the model/tier gate axis-scoped (1953). Five named, inspectable score contributions — capacity band, reset-boundary burn, throttling (account-scoped where 56A-2 narrowed it), session affinity (reused, not re-derived), model availability — each contributing NOTHING and saying `unknown` when its facet is unknown (1966); chosen by score, never rotation (a round-robin mutation is KILLED). The reset-boundary term burns a remainder about to expire and preserves a distant one, with the user's two examples verbatim as tests (1967). Distribution across the pool via the capacity+in-flight terms, and stickiness expressed as the affinity term's weight — a warm session holds its entitlement against a marginally better sibling and moves only when its rules deny it or its band reads exhausted (1968). A launch whose profile pins no entitlement is served by the chosen candidate's account, announced by name (1969). 6/6 mutations KILLED.

**One defect the investigation swarm caught before this box was ticked, fixed in this commit:** `burn_urgency` returned the maximum `+1.0` for any `seconds <= RESET_BURN_HORIZON` — including a **negative** `seconds`, i.e. a reset already in the past, which is routine over the persisted, deliberately un-staled capacity cache. That made the router prefer the *stalest* account over a fresh healthy one — the inverse of 1967's intent. Guarded `seconds <= 0 => 0.0` (a reset already reached has rolled the remainder over — nothing to burn), with a `burn_urgency_tests::a_reset_already_past_is_not_urgent` unit test. The broker's own reset examples (positive seconds) are unchanged.

**Recorded limits (accepted, queued as follow-up fixes — swarm findings routing-broker #1/#3):** an empty cached `ModelCatalogue` deserialises to `Declared(vec![])`, so `model_constraint` would refuse every candidate on that provider (an empty catalogue file makes a profile unlaunchable) — the model gate is guarded to fire only with ≥2 configured entitlements, bounding but not closing this; and the pool gate is set-wide rather than per-`(harness, profile)`, so an unrelated second entitlement can switch the pool terms on for a single-account resource. Neither inverts a closed contract; both are queued (`swarm-fixes-*`). Also: a full blast during the concurrent 12-agent swarm flaked `v1_criteria_setup::v1_1907` once (a real-binary subprocess+fixture test) — it passes twice isolated; noted as load-sensitive, not a broker regression.

---

### Line 1972 — the pool in one inspectable view, and the account that served each session (56A step 4)

Package `GH-ENTITLEMENT-FALLBACK-VIEW`, 2026-08-31, Opus 5 at high (Red).
**COMPLETE.** Line 1970 was **refused, not attempted** — see below.

Contract: given the user's `[entitlements.<name>]` entries, `glasshouse
entitlements` shows the whole pool in one view — every configured account named,
each with its remaining capacity, its time until reset, the throttling recently
observed against it, and what it served — and the entitlement that will serve a
session is announced when it starts; while preserving that an account no
telemetry describes still appears and reads `unknown` on every facet nothing
measured (never full, never empty, never a number), and that an entitlement is
named and never its credential.

**Why a new column was necessary, asserted rather than argued.**
`SessionRecord.backend_resource` holds `BackendResource::slug`, whose entire
vocabulary is `native`, `direct-provider:<provider>`, `glasshouse-gateway` — a
*kind* of resource. Phase 56A's unit of capacity is the *instance*, and the two
Claude accounts that motivate the phase both slug to `native`.
`two_accounts_of_one_vendor_are_two_values_where_backend_resource_is_one` pins
exactly that: the two records' `backend_resource` are equal while their
`entitlement` differ. Migration 22 adds `sessions.entitlement TEXT`, nullable,
no CHECK, no index — migration 20's shape and its stated rationale (validation
in Rust, not in SQL).

**The writer resolves the same value the announcement uses**, deliberately not a
second lookup, so what a person is told and what the record says cannot
disagree. The worker established, before wiring it, that `entitlement` at the
launch site is the router's own winner re-resolved by name and never a
separately-ranked one; the only other branch (`entitlement_for`) is taken when
the router did not run, and it *refuses* a several-account provider as ambiguous
rather than picking one. **Resume deliberately does not write** — its lookup is
the ambiguous one and yields `None` on a pooled provider, so overwriting there
would replace a well-established fact with a weaker one. `create` is the only
writer, and that is a decision, not an omission.

**`served` is deliberately not one of the unknowns.** The four telemetry facets
read `unknown` when nobody looked; `served` *did* look — at every session row the
project recorded — so an account with no rows has a measured zero and reads
`nothing recorded`. Sessions charged to an entry the configuration no longer
describes are still rendered: recorded history does not vanish when someone edits
a file.

Production: `database.rs` (migration 22, `SUPPORTED_SCHEMA_VERSION` 21→22) ·
`session/store.rs` (`SessionRecord::entitlement`, `NewSession::with_entitlement`,
`ALL_COLUMNS`, the `INSERT`, `read_record`) · `cli.rs` (`Command::Entitlements`) ·
`main.rs` (`entitlements_report`, `served_phrase`,
`entitlement_pool_with_telemetry` — extracted and shared with `status_report`, so
two commands cannot describe one account differently — and the dispatch arm).

Regression: `database::tests::the_entitlement_migration_adds_its_column_and_undoes_cleanly` ·
`entitlement_broker::{the_session_record_names_the_entitlement_that_served_it,
two_accounts_of_one_vendor_are_two_values_where_backend_resource_is_one,
the_view_names_every_entitlement_and_spells_unknown_for_one_nothing_measured,
the_view_reports_what_each_entitlement_served,
the_view_still_reports_sessions_charged_to_an_entry_no_longer_configured}`.
**Three of the five acceptance tests run through the shipped binary** (§35):
nothing that builds a `NewSession` by hand can fail on a build where
`launch_session` stops filling the column, and that call site is the whole of
what this line is about.

**5/5 mutations KILLED**, every restore byte-identical:

| vocabulary | change | killed by | observed |
|---|---|---|---|
| `drop-migration-column` | the migration's column renamed away | `the_session_record_names_the_entitlement_that_served_it` | the **launch itself fails** — the INSERT names the column, so an absent one is a loud error at the moment of recording rather than a silently dropped fact |
| `skip-state-update` | `.with_entitlement(...)` → `.with_entitlement(None)` | same | "the record carries the account that actually served, and it is the one the launch announced" |
| `wrong-source` | `read_record`'s column read → `None` | `two_accounts_of_one_vendor_...` | `read_first.entitlement` was not `Some("claude-a")` |
| `fabricate-unknown` | `"capacity: unknown"` → `"capacity: 100%"` | `the_view_names_every_entitlement_and_spells_unknown...` | an account nothing measured has no capacity — never full, never empty, never a number |
| `bypass-fallback` | `served_phrase(served.get(..))` → `served_phrase(None)` | `the_view_reports_what_each_entitlement_served` | the served count vanished |

**Line 1970 — REFUSED, and the refusal is the right outcome.** Its order
("subscription to subscription to API credits") is an order over *kinds*, and
both ways to express it were barred: routing on `EntitlementKind` would falsify
that field's own documented invariant — *"No rule depends on it — so a wrong
`kind` misdescribes an entitlement and never misroutes one"* — and the field is
`Option` and absent by default; the alternative needed a new `EntitlementConfig`
field held by another worker. **Nothing dead was left behind**: no unused purpose
constant, no unreachable ordering function, because production code with no
production caller is the shape `cluster-b.py` exists to find. The user ruled on
it the same evening; the ruling is recorded in `design-decisions.md` §Phase 56A,
"Step 4's fallback order", and makes the order **tier-preserving** with the
subscription/API distinction taken from `EntitlementBacking` rather than `kind`.

The worker also answered, from the code, the architectural question the packet
posed: the order applies **at selection time** inside `choose` and therefore at
all three call sites, because every pool candidate is `Native` or
`DirectProvider` — backends where Glasshouse is *not* in the inference path, so
it never sees the 429 and there is no use-time refusal to retry on. The record
belongs only at the acting sites, since `glasshouse route` reports and records
nothing.

Recorded limits — stated by the worker, not discovered later:
- resume does not write the column (above); a pooled resume still reads `None`
- the Linux and Windows legs were not run; nothing added is platform-conditional
- `routing/session.rs` and `routing/evidence.rs` are **untouched** — they are
  line 1970's files
- packet errors the worker caught and the packet had wrong: the migration ripple
  is wider than the recon's nine `version, 21` pins (four rollback batches need a
  `DROP COLUMN`, plus a tenth pin in `tests/session_context.rs` phrased so the
  grep missed it), and `SessionRecord` has no `Default`, so the new field broke
  seven test-only struct literals in four files

# Phase 56A lines 1970, 1971, 1974 — COMPLETE 2026-09-01; PHASE 56A CLOSED

Package `GH-BROKER-FALLBACK-56A` (Opus, high, Red; batch 70c), plus the
orchestrator landing the one sub-step the packet's own file boundary
deferred. Six worker mutations KILLED, plus the orchestrator's seventh below.

**1970 — the stated, tier-preserving fallback order.** `FallbackStep::ORDER`
(`routing/session.rs`) is the ruling's sentence and the only statement of it
in the build: another subscription serving the same model, a subscription
serving a same-tier model, then API credits under the same two steps.
Post-ranking reselection over `Routed::considered()` — nothing removed,
rescored, or reordered; the winner moves once and says so. The backing
became routing-significant (`EntitlementBacking::source()` →
`EntitlementSource`, carried by `ResolvedEntitlement::to_routing`) while
`EntitlementKind` stayed routing-insignificant, asserted by a test whose
`kind` lies outright. Tier steps consume Phase 34F through
`same_capability_tier`, which answers `Unknown` for every non-identical pair
today — so unknown NARROWS the order to its same-model steps
(mutation-pinned: `unknown-widens` KILLED). Mutations `invert-order` and
`bypass-fallback` KILLED, the latter from the library and the shipped binary
in one run.

The durable record, deferred by the packet (main.rs belonged to another live
worker) and landed by the orchestrator from the worker's own written patch:
`ENTITLEMENT_FALLBACK_{EXHAUSTED,THROTTLED}_PURPOSE` (`routing/evidence.rs`),
their `RoutingOverhead` bucket and consumption arm, and
`main.rs::record_entitlement_fallback` called from `launch_session` OUTSIDE
the classified guard (a launch that states no task can still fall back) —
`purpose` is the trigger, `quota_context` the account the work left, the
destination the `sessions.entitlement` column of the same launch. Binary
test `a_launch_that_falls_back_records_the_fallback_with_its_reason` reads
the row back through `consumption_by_purpose`/`from_consumption` — the same
aggregation the overhead report renders, chosen because a fallback row
carries no outcome and the outcome-carrying readers rightly never see it.
Orchestrator mutation `fallback-row-loses-purpose`
(`.with_purpose(Some(fallback_purpose))` → `None::<&str>`): **KILLED** by
that test (`left: 0, right: 1` — the row fell into another bucket).

**1971 — per-entitlement rules the broker never exceeds.** Allowed/denied
harnesses, tiers, job kinds — and the new fourth axis, `spend_ceiling_tokens`,
read from the ledger by `recent_credential_spend` with the same
narrow-by-credential, widen-when-unsure rule as the throttle reader. Rules
hold under fallback STRUCTURALLY: the reselection reads `considered()`, the
post-gate list, so no path exists to a candidate a rule removed. Mutations
`remove-guard` and `wrong-source` KILLED. Spend is TOKENS by ruling
(design-decisions §Phase 56A step 5): `cost_micro_usd` has no producer, and
a money ceiling would make "never exceed" vacuous.

**1974 — the broker's end-to-end cover.** Fixture entitlements only; normal
selection, the stated-order exhaustion fallback, a throttled entitlement,
capacity returning at the reset boundary, and a rule held under fallback
pressure — three of them through the shipped binary. `entitlement_broker`:
35 passed, 0 failed on the merged tree.

Recorded limits carried forward: the exhaustion trigger is proved at
`entitlement_fallback` (mutation-pinned via `choose`) but a full ranking
normally pre-empts it (−3.5 penalty) — the throttle arm is the one proved
end-to-end; the tier steps stay unreachable until 34F's axis wires in
(successor: attach the tier to `Destination` beside `tier_ceiling`); the
job-kind axis is enforced where a job kind exists (`DisposableRouting`),
1947's own recorded limit.

Cross-patch consequence found at integration, fixed in this batch: three
tests (`entitlements` ×2, `launch_preflight` ×1) premised "an unpinned
launch is native," which batch 70a's 372 closure made conditional — each now
states its native premise explicitly (automatic routing off, or the native
entitlement as the only candidate where continuation itself needs the router
on). 15/15, 10/10 twice each after the fix.

---

# Phase 56 censused 2026-09-02 — seven open lines, four causes, and the pairing-prior refusal was scoped to the wrong type

`GH-RECON-56` (Sonnet high, read-only; report kept at
`.agent-runtime/report-recon-56.md`), the orchestrator verified its decisive
claims against `2edce1b` before writing this.

| line | cause | verdict |
|---|---|---|
| 1945, 1955 | **1** — already true from the profile architecture, never proven | **packageable (Green, proof-only)** — `GH-DECOUPLE-PROOFS`, dispatched |
| 1948, 1950, 1956 | **2** — unchanged since the T2b entry above: `apply_gateway` still refuses before consulting `translate::lookup`; T3 (Gemini) unbuilt | open, same successors (`GH-GATEWAY-TRANSLATE-LAUNCH`, `GH-GATEWAY-TRANSLATE-T3`) |
| 1954 | **3** — unchanged: `pool_entitlements_for` returns `Vec::new()` for a gateway-backed destination, so *never charge* cannot be enforced or announced on that path | open, same successor (bounded Amber on `gateway/session.rs`) |
| 1952 | **4** — the efficiency producer (1951) exists; `RouterInputs` has no field for it and `score()` no term | **packageable (Amber)** — `GH-HARNESS-PREFERENCE` |

**Cause 1.** `Command::Launch` takes `harness` and `--profile` as independent
arguments (`cli.rs:360-395`); `LaunchProfile` carries `harness`, `backend`
and `model` as three fields; `EffectiveConfig::launch_profile` only checks the
stored harness matches (`config/mod.rs:5099-5105`) and `destination_backend`
resolves backend and model from the profile alone. For 1955,
`launch_profile` short-circuits on the native name *before* any config layer
is read (`:5075-5079`), and other profiles are a name-keyed map lookup, so a
new entry cannot mutate an existing one. Neither claim has a test framed as
its line; the adversarial `[profiles.native]` shadow case is untested.

**Cause 4.** `outcomes_by_tier_and_harness` and `request_stats_by_harness`
(line 1951, COMPLETE) are read by exactly one production caller,
`harness_efficiency_section`, which prints them in `glasshouse route`'s report
and never hands them to the router. `route_recommendation` already iterates
every enabled harness *because* the user has not chosen one
(`main.rs:3971-3972`) — line 1952's precondition, implemented for ranking and
not fed the data. Package: a handle beside `pool: &EntitlementPoolView` (the
same shape), one `harness efficiency` term in `score()` gated at
`MIN_SAMPLE_FOR_SUMMARY`, naming the harness and why.

## The question the orchestrator added, and its answer decides four lines outside this phase

**Can one `routing_destinations` output hold destinations of different
`PairingClass`? Yes.** The fresh-destination loop (`main.rs:1237-1308`)
iterates every enabled launch profile for the one harness the call is scoped
to, and `destination_backend` → `session_pairing(effective, profile)` builds
the `PairingQuery` from **that profile's own `model`**
(`main.rs:11241-11254`: `Some(model) => AssignedModel::named(model), None =>
HarnessDefault`). Two enabled profiles of one harness with different models —
a native profile beside a `DirectProvider { provider: "openrouter" }` profile
with a non-Anthropic model — resolve to different classes and land in the same
`Vec<Destination>`. `routing/session.rs` already computes exactly this per
destination: `classify_destination` (`:4859`) builds the query from
`destination.backend().model()` and is called from `hard_constraint` for its
protocol fit; `score()` never calls `.class()` on it.

**Why the standing refusal of 566/569 does not apply here.** `phase-9j.md`'s
constancy proof, and its tripwire
`routing::interactive::tests::the_native_pairing_prior_is_constant_across_a_real_session_start_candidate_set`
(re-run by the census, green), are about `routing::interactive`'s
`UpstreamBackend`, which *"has no model field"* and takes one model for the
whole set at `SessionRouting::bind`. `routing::session`'s own `Backend`
carries a model resolved per profile — the opposite of the type the proof
covers. `routing/session.rs`'s module header (`:30-35`) cites the 9J entry as
if it covered this router; it does not. **Ruling:** the refusal stands for
`routing::interactive` (do not touch it or its tripwire) and is **lifted for
`routing::session`**: 566, 569, 1540 and 1923 are one Amber package,
`GH-PAIRING-PRIOR` — a soft, inspectable `pairing prior` term in `score()`
from `classify_destination(...).class().is_vendor_native()`, bounded below
warm affinity (569) and decaying as observed evidence accumulates (1923,
1541's own rule). Caveat the census stated: no populated real configuration
with two enabled profiles of one harness was observed; the architecture
permits it and the package's own acceptance test is the first to exercise it
end to end.

---

# Cause 1 — CLOSED 2026-09-02 (`GH-DECOUPLE-PROOFS`, Green, tests only): 1945, 1955

Three tests, two KILLED mutations, zero production lines. The worker edited
`main.rs`'s test module before claiming the declared co-edit, was told
mid-turn, claimed, read (no peer), and declared done — recorded because §77's
claim-before-edit is what the hook now enforces.

### Allow a user to choose the coding harness for a task independently of which provider, subscription, or model serves it. (line 1945)

Contract: Given a launch profile naming any harness and any backend/model combination the user configured, when Glasshouse resolves a launch, the harness selected and the provider/subscription/model that serves it are chosen independently, neither constraining the other beyond the profile's own stored harness match.

State: **COMPLETE** — ruled 2026-09-02. Proof-only. Sampled one harness with `Native` beside `DirectProvider`; the resolver is per-name and uniform, so the sampling is a limit, not a gap. The destination half drives the real `routing_destinations`.

Production evidence:
- `crates/glasshouse/src/config/mod.rs:5070-5107` — `EffectiveConfig::launch_profile`
- `crates/glasshouse/src/main.rs:1521-1634` — `destination_backend`
- `crates/glasshouse/src/main.rs:1033` — `routing_destinations`

Regression evidence:
- `decoupled_profiles::line_1945_two_profiles_of_one_harness_resolve_independent_backend_and_model`
- `tests::routing_destinations_1945_carries_each_profiles_own_backend_and_model (main.rs)`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| config/mod.rs launch_profile: replace `if name == crate::profile::NATIVE_PROFILE_NAME {` (in its own function signature context) with `if true {` | `collapse-profile-lookup` | **killed** | `decoupled_profiles::line_1945_two_profiles_of_one_harness_resolve_independent_backend_and_model` |

> collapse-profile-lookup observed: assertion `left == right` failed (decoupled_profiles.rs:44:5)

Recorded scope limits — stated by the worker, not discovered later:
- sampled one harness and two backend kinds (Native, DirectProvider); does not exercise GlasshouseGateway in the same candidate set

---

### Keep the decoupling opt-in per launch profile, so an existing profile keeps its native pairing until the user changes it. (line 1955)

Contract: Given an existing launch profile with no explicit backend/model configured, when Glasshouse resolves it for a launch, it keeps its native pairing regardless of what other profiles are added or enabled, until the user edits that profile's own entry.

State: **COMPLETE** — ruled 2026-09-02. Proof-only. The adversarial case the census named — a config table literally called `native` — is the test's own assertion, and the mutation that removes the short-circuit is KILLED by it.

Production evidence:
- `crates/glasshouse/src/config/mod.rs:5075-5079` — `EffectiveConfig::launch_profile (native short-circuit)`
- `crates/glasshouse/src/config/mod.rs:5082-5089` — `same fn, name-keyed map lookup`

Regression evidence:
- `decoupled_profiles::line_1955_native_is_unshadowable_and_existing_profiles_survive_new_ones`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| config/mod.rs launch_profile: delete the `if name == NATIVE_PROFILE_NAME { return Ok(Layered::new(LaunchProfile::native(harness), Layer::Default)); }` block, falling straight into the name-keyed lookup | `remove-native-shortcircuit` | **killed** | `decoupled_profiles::line_1955_native_is_unshadowable_and_existing_profiles_survive_new_ones` |

> remove-native-shortcircuit observed: assertion `left == right` failed: a configured [profiles.native] table must never shadow the built-in native profile (decoupled_profiles.rs:125:5)

Recorded scope limits — stated by the worker, not discovered later:
- tested a user-layer [profiles.native] shadow only, not a project-layer one; the short-circuit runs before either layer is consulted so the code path is identical

---

### Worker-reported packet errors and gates (transcribed at closure)

Gates the worker ran (re-run the decisive ones yourself):
- cargo test -p glasshouse --test decoupled_profiles: test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
- cargo test -p glasshouse --bin glasshouse routing_destinations_1945: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 65 filtered out
- cargo test -p glasshouse --lib config::: test result: ok. 96 passed; 0 failed; 0 ignored; 0 measured; 1811 filtered out
- cargo clippy -p glasshouse --all-targets --all-features -- -D warnings: clean
- scripts/blast-radius.sh --targeted (no path arg, both changed files traced): cargo check --tests clean; cargo test --test decoupled_profiles ok (2 passed); --targeted skipped 69 full-trace targets (expected, fast gate); cargo doc --no-deps clean; every traced target passed

---

## Line 1952 — CLOSED 2026-09-02 (`GH-HARNESS-PREFERENCE`, Amber, Sonnet high) — Cause 4 above, closed

Cause 4 said the per-harness efficiency rows (line 1951) had one production
reader, the `glasshouse route` report, and never reached the router. They
reach it now: `main.rs::harness_efficiency_summary` reduces
`outcomes_by_tier_and_harness` over the report's own window into a
`HarnessEfficiencySummary`, and `main.rs::session_router` hands it to
`SessionRouter::with_harness_efficiency` for both `route_recommendation` and
`launch_session`. `score()` pushes one `harness efficiency` term: this
destination's success rate for the classified task class minus the mean rate
of the *other* candidate harnesses that also clear `MIN_SAMPLE_FOR_SUMMARY`,
clamped to `[-1.0, 1.0]` — strictly below a warm session's `1.5`. It is inert
and says why when no task was classified, when this harness is below the
sample gate, or when no other candidate harness clears it; that last case is
what keeps a user-assigned harness untouched, because `launch_session`'s
candidate set is already scoped to one harness when the user named one.

### Prefer, for a stated task the user has not assigned a harness to, the harness with the better observed efficiency for that task class, and say why. (line 1952)

Contract: Given a stated task the user has not assigned a harness to, when Glasshouse ranks candidates across every enabled harness, it prefers the harness with the better observed outcome for that task's class and says why, while contributing nothing (and saying so) below the sample gate, and while a harness the user did assign is never overridden by it.

State: **COMPLETE** — ruled 2026-09-02, after reading the term's decision hunk and the merged-tree re-run of the eight named tests at integration. Both mutations are on the decision the line names (the magnitude, and the sample gate) and both are KILLED by the socket-level `harness_preference` tests, not by a same-module mirror. The third `session_router()` caller (the resume report) is inert by construction and is left as the worker recorded it.

Production evidence:
- `crates/glasshouse/src/routing/session.rs` — `HarnessEfficiencySummary::from_outcomes`
- `crates/glasshouse/src/routing/session.rs` — `harness_efficiency`
- `crates/glasshouse/src/routing/session.rs` — `SessionRouter::with_harness_efficiency`
- `crates/glasshouse/src/routing/session.rs` — `score (pushes the term)`
- `crates/glasshouse/src/main.rs` — `session_router (wires both route_recommendation and launch_session)`
- `crates/glasshouse/src/main.rs` — `harness_efficiency_summary`

Regression evidence:
- `routing::session::harness_efficiency_tests::the_harness_with_the_better_recorded_rate_gets_the_positive_term`
- `routing::session::harness_efficiency_tests::below_the_sample_gate_the_term_is_inert`
- `routing::session::harness_efficiency_tests::an_empty_summary_is_inert`
- `routing::session::harness_efficiency_tests::with_no_task_classified_the_term_is_inert`
- `routing::session::harness_efficiency_tests::a_single_harness_candidate_set_cannot_be_re_ranked_across_harnesses`
- `harness_preference::the_harness_with_the_better_recorded_rate_is_chosen`
- `harness_preference::below_the_sample_gate_the_ranking_is_unaffected`
- `harness_preference::a_candidate_set_scoped_to_one_harness_is_never_re_ranked_across_harnesses`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| let magnitude = (own_rate - other_mean).clamp(-1.0, 1.0); -> let magnitude = 0.0; | `null-harness-efficiency-magnitude` | **killed** | `harness_preference::the_harness_with_the_better_recorded_rate_is_chosen` |
| .filter(|entry| entry.sample_size >= MIN_SAMPLE_FOR_SUMMARY as i64) -> .filter(|entry| entry.sample_size >= 0) | `remove-sample-gate` | **killed** | `harness_preference::below_the_sample_gate_the_ranking_is_unaffected` |

> null-harness-efficiency-magnitude observed: panicked at crates/glasshouse/tests/harness_preference.rs:167:5: Contribution { name: "harness efficiency", magnitude: 0.0, evidence: "`claude-code` succeeded 9 of 10 recorded `standard` tasks (90% success), against 20% for the other harness(es) considered here" }

> remove-sample-gate observed: panicked at crates/glasshouse/tests/harness_preference.rs:238:5: assertion `left == right` failed: fewer than MIN_SAMPLE_FOR_SUMMARY observations must score exactly like no summary at all
  left: [0.33333333333333326, -0.33333333333333326]
 right: [0.0, 0.0]

Recorded scope limits — stated by the worker, not discovered later:
- No Windows leg run; main.rs is platform-conditional elsewhere in the file but this package adds no new cfg(unix/windows) code
- The full (non-targeted) blast-radius sweep (77 further targets) was not run — trailing per CLAUDE.md's WAVE process, not owed by this packet
- The third session_router() caller (main.rs:9642, the resume report path) is inert by construction (TaskRequirements::default()) but has no dedicated end-to-end test in this package

---

---

---

### The launch link for lines 1948, 1950 and 1956 — 2026-09-02 (`GH-GATEWAY-TRANSLATE-LAUNCH`, Amber, Sonnet high); the lines stay open on T3

Package `GH-GATEWAY-TRANSLATE-LAUNCH`, 2026-09-02, Sonnet at high (Amber — one
decision at one seam). `profile::apply_gateway` now consults
`gateway::translate::lookup` before refusing a harness whose own wire protocol
the running gateway does not serve natively: a native match takes exactly the
path it took (same variable, same call shape); otherwise the first `Supported`
row over the harness's protocols × the served slugs launches the harness
speaking its own protocol at the ingress and binds the session to the *served*
protocol's backend (`gateway.routing().bind(..., &served_protocol, ...)`, so
`Upstream::serving().as_routing_backend` finds the route and the assignment
names the serving provider); a `Refused` row is refused by name with the new
`Refusal::GatewayTranslationRefused { pair, reason }` and nothing started; a
harness declaring no protocol still gets `GatewayProtocolUnserved`. The
standing tripwire in `tests/gateway_translate.rs` is deleted as its own doc
prescribed and replaced by two launch-driven tests through the shipped binary:
Claude Code on a chat-only entitlement translated end to end (the fixture
records the chat request, the harness gets its Anthropic answer, one routing
observation names the chat provider under the translated pair), and an
OpenCode-shaped `openai-chat` client on an Anthropic-only entitlement refused
by name with no child and no upstream connection.

Three mutations, three KILLED, killing test and output quoted in the report:
`never-consult-the-table` (`.or(translated)` → `.or(None)`; the translated
launch exits 1 before the harness runs), `refused-row-launches`
(`pair.is_supported()` → `true`; the refused launch is accepted),
`bind-the-harness-protocol` (bind with the harness slug; the observation no
longer names the chat provider). Gates: `gateway_translate` 9/9,
`gateway_degrade` 3 + `launch_preflight` 12, `profile::` lib 83/83,
`gateway::` lib 142/142, `blast-radius.sh --targeted` green on rerun — its
first run's one red (`observe_exchange_scores_a_real_failover_against_the_configured_preference`)
attributed to machine load (ten concurrent rustc from three workers), passed
alone, three times in-module, and on the full rerun at load 1.

**Packet error, the worker's, accepted.** Two `profile` lib tests used Codex on
an Anthropic-Messages-only gateway as their "refused" fixture — exactly T2's
own `openai-responses -> anthropic-messages` Supported row — so lifting the
launch-link refusal necessarily turned that fixture into a success. Both moved
to a harness that is genuinely still refused (OpenCode, Cursor) and a positive
test for the Codex case was added
(`a_gateway_serving_only_anthropic_messages_accepts_a_codex_launch_through_translation`).
No property was weakened; the packet's claim that only the tripwire would move
was wrong.

**Ruling: 1948, 1950 and 1956 stay OPEN**, one of their two named successors
now landed. The launch link the T1 entry recorded as *"not yet under a real
launched harness"* is closed for the T1 pair through `glasshouse launch`; the
other four supported pairs keep their wire-level e2e and no launch-driven test
of their own (the worker's recorded limit), and the pair-count half waits on
`GH-GATEWAY-TRANSLATE-T3` (the Gemini codec, live at the time of writing) and
T3b (the Gemini CLI adapter). The 2026-08-31 reading — *every supported pair* —
is kept rather than re-read to fit what exists.

---

---

## Line 1954 — CLOSED 2026-09-02 (`GH-GATEWAY-ENTITLEMENT-1954`, Amber, Sonnet high): the gateway-backed launch consults the entitlement behind the serving provider

Recon-56's cause 3 named the gap and the successor as a bounded package on `gateway/session.rs`; it turned out to need no gateway-session change at all — the serving provider is known at the launch site once the gateway has started, and that is where the rule is asked. `Gateway::serving_provider` is the one accessor added.

### Never charge a task to a subscription the user's rules did not allow for that harness or tier, and announce which subscription served each session. (line 1954)

Contract: Given a launch profile backed by the Glasshouse gateway whose serving provider is named by an [entitlements.<name>] entry, when the user launches or resumes under it, Glasshouse asks that entitlement's rules about this harness (and the tier when one is known) exactly as the direct-provider path does -- refusing by the entitlement's name and starting nothing when the rule does not admit it, and otherwise announcing which entitlement will serve the session and recording its name on the session record -- while preserving that a gateway whose serving provider no entitlement describes is announced as such and never refused, and that native and direct-provider launches are byte-identical to today.

State: **COMPLETE** — ruled 2026-09-02. The 08-31 entry above held this line on one clause: a gateway-backed profile had no entitlement at launch because the *router* cannot see the gateway's upstream. The launch can: `launch_session` starts the gateway before it resolves the profile, and the started gateway's serving provider is known, so the consult now happens there — `entitlement_for_provider(gateway.serving_provider())`, the same `rules().refusal(harness, tier)` and the same refusal text as the direct path through one extracted spelling, the announcement naming the entitlement or, when none describes the provider, saying so by the provider's name, and the name written to `sessions.entitlement`. The resume path reads the same gateway. Three shipped-binary tests, three KILLED mutations with the failure text quoted, every entitlement, preflight and degrade suite unchanged and green. `pool_entitlements_for` still returns nothing for a gateway destination on purpose: the router stays blind to the upstream, and *never charge* is enforced where the charge happens. Limits are the worker's: the tier half fires only on a pass that knows the tier, as on the direct path; the resume path has no dedicated binary test; macOS only.

Production evidence:
- `src/gateway/mod.rs` — `Gateway::serving_provider`
- `src/main.rs` — `launch_session (gateway consult after start_if_required_with_degrade_sink, before EntitlementScopedSecrets/resolve_with_gateway)`
- `src/main.rs` — `announce_entitlement (gateway_provider parameter, new GlasshouseGateway/Some(provider) text)`
- `src/main.rs` — `entitlement_refusal_message (extracted, shared by both call sites)`
- `src/main.rs` — `resume_session's resume_entitlement block (reads overlay_resolution's gateway)`

Regression evidence:
- `entitlement_pool::a_gateway_backed_launch_whose_entitlement_denies_the_harness_is_refused_and_starts_nothing`
- `entitlement_pool::a_gateway_backed_launch_whose_entitlement_admits_the_harness_proceeds_and_is_recorded`
- `entitlement_pool::a_gateway_whose_provider_no_entitlement_names_is_announced_and_never_refused`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| if is_gateway_backend { -> if false && is_gateway_backend { | `skip-the-gateway-consult` | **killed** | `entitlement_pool::a_gateway_backed_launch_whose_entitlement_denies_the_harness_is_refused_and_starts_nothing` |
| .with_entitlement(entitlement.as_ref().map(|entry| entry.name().to_owned())), -> .with_entitlement(None), | `announce-but-do-not-record` | **killed** | `entitlement_pool::a_gateway_backed_launch_whose_entitlement_admits_the_harness_proceeds_and_is_recorded` |
| entitlement_refusal_message's `entitlement?` early return replaced with an unconditional refusal when entitlement is None | `refuse-the-undescribed-gateway` | **killed** | `entitlement_pool::a_gateway_whose_provider_no_entitlement_names_is_announced_and_never_refused` |

> skip-the-gateway-consult observed: panicked at crates/glasshouse/tests/entitlement_pool.rs:1251:5 -- assert!(!refused.status.success(), ...) -- the denied launch was accepted

> announce-but-do-not-record observed: panicked at crates/glasshouse/tests/entitlement_pool.rs:1299:5 -- assertion `left == right` failed: the record carries the entitlement that actually served (left: None, right: Some("ok"))

> refuse-the-undescribed-gateway observed: panicked at crates/glasshouse/tests/entitlement_pool.rs:1325:5 -- assert!(out.status.success(), ...) -- the undescribed-provider launch was refused instead of proceeding

Recorded scope limits — stated by the worker, not discovered later:
- macOS only; no Linux/Windows leg run
- the three new binary tests never forward a real request through the gateway (fake harness exits before dialling)
- no --task is stated at this point in launch_session, so only the harness half of EntitlementRules::refusal is exercised here, mirroring the pre-existing direct-provider limit
- the resume-path fix (item 4) has no dedicated binary test; none of the required regression suites covers a gateway-backed resume

---

---

### T3 — the Gemini codec, the fourth wire protocol, three more supported pairs — 2026-09-02 (`GH-GATEWAY-TRANSLATE-T3`, Opus 5 high, Red)

`WireProtocol::GeminiGenerateContent` (slug `gemini-generate-content`), a
`gemini` provider template (Google AI Studio, bare host, the codec states
`/v1beta` itself, credential in `x-goog-api-key`), `gateway/translate/gemini.rs`
against `generateContent` / `streamGenerateContent`, and a sixteen-row table:
**eight supported ordered pairs now** — the five of T1/T2/T2b plus
`anthropic-messages -> gemini-generate-content`, `openai-responses ->
gemini-generate-content` and `openai-chat -> gemini-generate-content`, each
behind its own document and stream test through the shipped binary's door in
`tests/gateway_translate_gemini.rs` (10/10). Nothing translates *out* of Gemini:
no installed harness speaks it at the ingress, so the four `gemini-generate-content
-> *` rows are `Refused` naming that reason and T3b, not "no test yet".

Five decisions, each written in the codec's module doc and four of them
mutation-pinned: a Gemini function call has no id, so a tool result is matched
to the tool-use block by **name** within the same request and refused by name
where that cannot be kept; `STOP` with a function call in the candidate is
`ToolUse`, not `EndTurn`; Gemini streams whole candidate chunks, so the
harness-side events are synthesised in the client protocol's own order with
every block stopped before the next starts; the model is addressed in the
path, so `Codec` gained `claim(path)` and `outbound_endpoint(&Request)` with
defaults that reproduce the three existing codecs byte for byte; the outbound
header hook in `translate::serve` (T2b's, not `ingress.rs` as the packet said)
adds `x-goog-api-key` exactly on the Gemini target. A relay defect was caught
by the package's own relay test before it could ship: a base URL carrying
`/v1beta` doubled the segment on a relayed Gemini target (`Route::uri_for`
appends the client's own target byte for byte); the base URL is the bare host
now and the codec supplies the version, the same convention OpenRouter's
Anthropic entry already used.

Five mutations, five KILLED with output quoted: `refused-pair-marked-supported`
(the refused-pair test and the pairing table pin), `drop-the-tool-call`,
`wrong-stream-order`, `lose-the-api-key-header`,
`stop-is-end-turn-even-with-a-function-call`. Gates: `gateway_translate_gemini`
10/10; `gateway_translate` 8/8, `gateway_translate_responses` 7/7,
`gateway_translate_t2b` 3/3 all unchanged; `gateway::translate` lib 71/71,
`harness::pairing` 27/27; the default blast sweep's one red
(`session_supervision::a_session_that_keeps_crashing_is_restarted_a_bounded_number_of_times`,
a 20 s bounded wait) reproduced on a pristine `90e8dc4` worktree in three runs
with two different tests of that family and is the load-flaky supervision
family, not this package's. The compiler named four exhaustive matches, not
twelve; `GATEWAY_INGRESS_PROTOCOLS` and config's `wire_protocol_from_slug` also
needed the variant (disclosed as scope overflow, one line each), and the
templates snapshot gained its entry in place.

**Recorded limits, the worker's:** a session served through a Gemini
entitlement records protocol `unknown`, because migration 8's `CHECK` on
`sessions.protocol` lists three slugs and widening a `CHECK` in SQLite is a
table rebuild — a migration this package was forbidden and Cluster G refuses
casually; no live probe was made against `generativelanguage.googleapis.com`
(`model_list_endpoint` and `usage_telemetry` stay `Declared::Unverified`);
Gemini's tool-schema dialect is a subset of JSON Schema and a keyword Google
rejects comes back as the provider's own `400` in the harness's error shape;
the Gemini CLI adapter (T3b) is not built.

**Ruling on lines 1948, 1950 and 1956 — CLOSED 2026-09-02**, with the launch link (`GH-GATEWAY-TRANSLATE-LAUNCH`, `b87d38e`) and this codec both on the tree, and the reading written down so it is not re-derived. The map's *supported harnesses* are the four with adapters (Claude Code, Codex, OpenCode, Antigravity); the map names Gemini only as a *subscription* (line 1946), never as a harness, so the Gemini CLI adapter (T3b) is `design-decisions.md`'s future work and not a clause of these lines. On the map's own words: (1948) every supported harness whose native protocol the gateway can translate from a subscription's protocol is served — Claude Code, Codex and OpenCode each from every other protocol the table carries, including a Gemini plan — and `glasshouse launch` now takes that path; Antigravity, whose protocol is `Declared::Unverified`, is refused by name, which is (1950)'s own clause; (1950) tool definitions, calls, results, ids, stop reasons and streaming order are kept through every supported row, with what a wire cannot carry refused per request by field name, never degraded; (1956) each of the eight supported rows is behind its own end-to-end test through the shipped binary's door against a fixture upstream, and no row was made `Supported` without one. Recorded limits, carried from the three entries: `openai-chat -> anthropic-messages` (OpenCode on a Claude plan) is the one refused row among the four protocols and stays refused by name until its codec direction is built; the launch-driven test covers the T1 pair, the other seven have their wire-level end-to-end tests; a session served through a Gemini plan records protocol `unknown` until a migration widens `sessions.protocol`'s CHECK; the Gemini CLI adapter (T3b) is not built. The 2026-08-31 reading — *every supported pair, and exactly one exists* — counted pairs the table could carry; eight exist now, the launch link is closed, and the quantifier in the map's sentence is the supported harnesses, not every protocol the world has.
