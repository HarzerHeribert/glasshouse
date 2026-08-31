# Capability evidence — phase 56

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
