# Capability evidence — phase 39

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 39 — lines 1607, 1609, 1611; 1608 refused

Package `GH-SUPPORT-WORK-ECONOMY`, 2026-08-31, Opus at high. Six mutations, six killed. **1608 refused** and stays open: Cluster Q — `JobKind` is Classification | MemoryExtraction | Reranking | Evaluation, no repository-summarization job exists, so there is nothing to prefer a cheap resource *for*. The worker left a tripwire (`no_repository_summarization_job_exists_to_route_cheaply_yet`) that stops compiling if a variant is added, and did not tick the box.


### Prefer local or free resources for trivial classification and extraction work when suitable. (line 1607)

Contract: Given trivial classification and extraction work, when Glasshouse picks the resource to run it, it prefers a free resource for extraction and a local one for classification among candidates that are otherwise equally adequate — while preserving that extraction's selector ranks on the user's own free-resource order and never on locality, so the line's disjunction is satisfied by the half each job actually has.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts.

Production evidence:
- `src/routing/disposable.rs` — `DisposableRouting::choose (the free-before-metered order)`
- `src/routing/disposable.rs` — `DisposableRouting::classification_preferences (the locality term)`
- `src/routing/disposable.rs` — `DisposableRouting::choose_for_automatic_classification (the preference pre-order)`
- `src/main.rs` — `disposable_candidates (with_locality from ResourceKind::locality)`

Regression evidence:
- `support_work_economy::free_capacity_is_preferred_for_extraction_on_the_shipped_binary`
- `support_work_economy::a_local_free_candidate_is_preferred_for_trivial_classification_and_extraction`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/routing/disposable.rs: the free set's `.filter(|candidate| candidate.value().cost().is_free())` -> `.filter(|candidate| !candidate.value().cost().is_free())` | `invert-predicate` | **killed** | `support_work_economy::free_capacity_is_preferred_for_extraction_on_the_shipped_binary` |
| src/routing/disposable.rs: locality's Local arm weight `CLASSIFICATION_PREFERENCE_WEIGHT,` -> `0.0,` | `zero-the-term` | **killed** | `support_work_economy::a_local_free_candidate_is_preferred_for_trivial_classification_and_extraction` |

> invert-predicate observed: panicked at support_work_economy.rs:484: the free model must be the one chosen — the stored rationale named the metered model instead

> zero-the-term observed: assertion `left == right` failed: a local candidate must be preferred over an equally adequate remote one; left `a-remote-model`, right `a-local-model`

Recorded scope limits — stated by the worker, not discovered later:
- Extraction has NO locality preference. `choose`'s free loop walks the user's configured order and consults no score, so the locality term reaches an extraction rationale as text only. Free-first is what satisfies the disjunction for extraction.
- The classification half is proven at `choose_for_automatic_classification`, not through the binary: a local candidate needs a registry-known local provider slug (`ollama` / `llama-cpp`) that a fixture cannot fabricate.


---


### Prefer premium warm sessions for difficult tasks that benefit strongly from existing context. (line 1609)

Contract: Given a person's own routing decision with a warm session on a premium resource in the reserve band and a cheaper cold alternative beside it, when the task is classified heavy, Glasshouse keeps the warm premium session and names the tier that justified it; when the same setup is given trivial work, it moves to the cheaper alternative — while preserving that 'benefits strongly from existing context' is not measured per task, so the preference is conditioned on difficulty and warmth, which are the two signals this build actually has.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts.

Production evidence:
- `src/main.rs` — `classify_for_routing (the --task classification, reached from both `route` and `launch`)`
- `src/routing/session.rs` — `SessionRouter::choose (PressureInputs.tier = requirements.minimum_tier)`
- `src/routing/pressure.rs` — `capacity_band_pressure (the reserve arm, via evaluate_reserve_spend)`
- `src/routing/session.rs` — `session_affinity`

Regression evidence:
- `support_work_economy::a_heavy_task_keeps_its_warm_premium_session_on_the_shipped_binary`
- `support_work_economy::a_heavy_task_keeps_its_warm_premium_session`
- `support_work_economy::the_route_command_carries_a_tasks_difficulty_into_the_ranking`
- `support_work_economy::the_two_task_descriptions_this_file_relies_on_really_do_classify_differently`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/main.rs: `let text = site.task.map(str::trim).filter(|text| !text.is_empty())?;` -> `... .filter(|_| false)?;` — the task's difficulty never reaches the ranking | `cut-the-input` | **killed** | `support_work_economy::a_heavy_task_keeps_its_warm_premium_session_on_the_shipped_binary` |

> cut-the-input observed: panicked at support_work_economy.rs:1036: a difficult task must keep the warm premium session rather than start somewhere cheaper and cold — the ranking chose a `fresh:` destination

Recorded scope limits — stated by the worker, not discovered later:
- NO NEW TERM WAS NEEDED and none was added; `pressure.rs` and `session.rs` are untouched. The packet's contingency did not fire.
- RULING OWED: 'that benefit strongly from existing context' is not measured per task. `session_affinity`'s own doc records that Phase 36's same-task, touched-file and semantic-quality signals (1581-1588) have no producer, so warmth is the whole affinity signal and the preference applies to difficult tasks generally. Precedent for closing on the nearest real signal with the limit named is 1438 ('quality requirements are the reliability floor'). If the orchestrator reads the qualifier as load-bearing, this is `open` and its successor is Phase 36's affinity producers.
- Warmth in the fixture is a `resumable` session at +0.750, not a live one at the full weight. The ordering holds at either magnitude for the heavy case; the leaf flip is carried by RESERVE_DENIED_PENALTY -2.0 plus LOW_TIER_SPEND_PENALTY -3.0 together.
- The two binary route tests are `#[cfg_attr(windows, ignore)]` — the fake-harness launch they need is the unix shim.


---


### Avoid spending premium model capacity on internal Glasshouse bookkeeping when a cheap resource can perform it reliably. (line 1611)

Contract: Given Glasshouse's own bookkeeping, when a free adequate resource is available, Glasshouse does not reach a metered one at all; and for classification, where reliability is measured, a free candidate below the reliability floor is not treated as one that can perform the job — while preserving that extraction has no reliability record and its verdict therefore rests on the metered gate alone.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts.

Production evidence:
- `src/routing/disposable.rs` — `DisposableRouting::choose (the metered loop runs only after every free candidate is exhausted)`
- `src/routing/disposable.rs` — `classification_verdict (CLASSIFICATION_RELIABILITY_FLOOR)`

Regression evidence:
- `support_work_economy::premium_capacity_is_not_spent_on_bookkeeping_when_a_cheap_reliable_resource_exists`
- `support_work_economy::free_capacity_is_preferred_for_extraction_on_the_shipped_binary`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/routing/disposable.rs: `if fraction < CLASSIFICATION_RELIABILITY_FLOOR {` -> `if false {` | `disable-the-gate` | **killed** | `support_work_economy::premium_capacity_is_not_spent_on_bookkeeping_when_a_cheap_reliable_resource_exists` |

> disable-the-gate observed: assertion `left == right` failed: a cheap resource below the reliability floor is not one that can perform the job reliably; the 1-of-10 free candidate was chosen over the metered one

Recorded scope limits — stated by the worker, not discovered later:
- SPLIT VERDICT. The `reliably` clause is measured for CLASSIFICATION only. Extraction has no reliability record — `routing_observations` rows carry no purpose for an extraction call — so extraction's verdict rests on the metered gate alone. If the ledger wants the clause proven for both jobs, record this as 1611-partial until the extraction purpose stamp (`evaluation-producers`) has a reader.
- The reliability floor never filters a pinned model, and rows written before GH-ROUTING-ECONOMICS carry no outcome and count toward neither side.

### Lines 1621–1624, 1626–1628 — the disposable-job interface, proved; 1625 refused

Package `GH-PROVE-IT-39`, 2026-08-31, Sonnet at medium (Green): the recon found these seven lines already satisfied by `JobKind` + `ExtractionModel` + `ConfiguredModel`; this package added `tests/disposable_interface.rs` — one test and one killed mutation per line, plus a structural scan for 1626–1628 in the shape of Phase 9E's `SecretRef` test. Zero production change. **1625 refused, and stays open:** the line names reranking, and no reranking job exists (Cluster Q); a tripwire fails the moment a `JobKind` variant appears. Eight mutations, eight killed.

### Define disposable jobs as bounded internal LLM calls rather than native interactive sessions or coding harnesses. (line 1621)

Contract: Given a disposable job, when Glasshouse names its kind, it draws from exactly {Classification, MemoryExtraction, Reranking, Evaluation} and DisposableRouting::choose accepts no other kind — while preserving that the closure is enforced by the type system, not a runtime check.

State: COMPLETE — ruled 2026-08-31 by the orchestrator: a line the code already satisfied, proved by a test and a killed mutation (GH-MAP-SIDE-EFFECT-AUDIT's finding; no production change).

Production evidence:
- `src/routing/disposable.rs` — `JobKind`
- `src/routing/disposable.rs` — `JobKind::as_str`
- `src/routing/disposable.rs` — `DisposableRouting::choose`

Regression evidence:
- `disposable_interface::job_kind_is_a_closed_vocabulary_of_bounded_internal_calls`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/routing/disposable.rs: Self::Reranking => "reranking", -> Self::Reranking => "unknown", | `rename-the-name` | **killed** | `disposable_interface::job_kind_is_a_closed_vocabulary_of_bounded_internal_calls` |

> rename-the-name observed: assertion `left == right` failed: the closed vocabulary a disposable job may name itself with has changed

Recorded scope limits — stated by the worker, not discovered later:
- Proves the vocabulary is closed and named correctly today; does not prove no future variant will ever be session-shaped — only that today's four are not.


---


### Add a simple provider interface for non-interactive disposable LLM jobs. (line 1622)

Contract: Given something that answers an extraction prompt, when it implements only ExtractionModel::describe and ::complete, Extractor::run calls both exactly once per run and succeeds — while preserving that complete_observed's default forwarding is what makes the two-method implementation sufficient.

State: COMPLETE — ruled 2026-08-31 by the orchestrator: a line the code already satisfied, proved by a test and a killed mutation (GH-MAP-SIDE-EFFECT-AUDIT's finding; no production change).

Production evidence:
- `src/memory/extract/mod.rs` — `ExtractionModel`
- `src/memory/extract/mod.rs` — `Extractor::run`

Regression evidence:
- `disposable_interface::a_minimal_extraction_model_impl_drives_extractor_run_through_both_its_methods`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/memory/extract/mod.rs: ExtractionOutcome::empty(trigger, self.model.describe(), chunk) -> ExtractionOutcome::empty(trigger, String::new(), chunk) | `drop-the-name` | **killed** | `disposable_interface::a_minimal_extraction_model_impl_drives_extractor_run_through_both_its_methods` |

> drop-the-name observed: assertion `left == right` failed: Extractor::run must call ExtractionModel::describe() to name the resource

Recorded scope limits — stated by the worker, not discovered later:
- Proves the minimal two-method shape drives a real run through a bootstrapped project; does not exercise the ten other ExtractionModel implementations already in the crate.


---


### Allow OpenAI-compatible gateways to be configured through the disposable-job interface. (line 1623)

Contract: Given a provider naming the OpenAI chat-completions protocol, when ConfiguredModel::new builds a client for it, the endpoint is exactly {base_url}/chat/completions — while preserving that the field itself stays private and this is proved through Debug, the only external surface that exposes it.

State: COMPLETE — ruled 2026-08-31 by the orchestrator: a line the code already satisfied, proved by a test and a killed mutation (GH-MAP-SIDE-EFFECT-AUDIT's finding; no production change).

Production evidence:
- `src/memory/extract/model.rs` — `ConfiguredModel::new`

Regression evidence:
- `disposable_interface::an_openai_compatible_gateway_reaches_the_chat_completions_path`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/memory/extract/model.rs: endpoint: format!("{base_url}/chat/completions"), -> endpoint: format!("{base_url}/v1/completions"), | `change-the-path` | **killed** | `disposable_interface::an_openai_compatible_gateway_reaches_the_chat_completions_path` |

> change-the-path observed: the endpoint must be the base URL with /chat/completions appended: ... assertion failed

Recorded scope limits — stated by the worker, not discovered later:
- Proves the endpoint string shape only, not the request body's wire schema (private fn, not reachable from an integration test) — the body's shape is covered instead by 1626-1628's structural scan of the same file.


---


### Allow local Ollama or llama.cpp endpoints to be configured through the disposable-job interface. (line 1624)

Contract: Given Ollama's real default local endpoint http://127.0.0.1:11434/v1, when ConfiguredModel::new builds a client for it, it reaches the same generic {base_url}/chat/completions path and the same generic '... via openai-chat' description as any hosted provider — while preserving that no code branches on that port or on the words ollama/llama.cpp.

State: COMPLETE — ruled 2026-08-31 by the orchestrator: a line the code already satisfied, proved by a test and a killed mutation (GH-MAP-SIDE-EFFECT-AUDIT's finding; no production change).

Production evidence:
- `src/memory/extract/model.rs` — `ConfiguredModel::new`
- `src/memory/extract/model.rs` — `ConfiguredModel::describe`

Regression evidence:
- `disposable_interface::a_local_ollama_endpoint_round_trips_with_no_port_special_case`
- `disposable_interface::a_non_openai_chat_protocol_is_refused_even_on_a_loopback_host`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/memory/extract/model.rs: endpoint: format!("{base_url}/chat/completions"), -> endpoint: if base_url.contains("11434") { format!("{base_url}/api/chat") } else { format!("{base_url}/chat/completions") }, | `special-case-the-port` | **killed** | `disposable_interface::a_local_ollama_endpoint_round_trips_with_no_port_special_case` |

> special-case-the-port observed: Ollama's default endpoint must reach the generic chat-completions path, unmodified: ... assertion failed

Recorded scope limits — stated by the worker, not discovered later:
- Proves no special-casing exists in ConfiguredModel::new today; a special case introduced anywhere else in the call chain (e.g. at a caller in main.rs) is not covered by this test.


---


### Keep disposable jobs distinct from first-class interactive harness sessions. (line 1626)

Contract: Given the disposable-model call path, when its production source is scanned, it carries no Pty, SessionId, tool_calls, or function_call surface — while preserving that the scan excludes the file's own #[cfg(test)] module, so an unrelated test-fixture field cannot false-positive it.

State: COMPLETE — ruled 2026-08-31 by the orchestrator: a line the code already satisfied, proved by a test and a killed mutation (GH-MAP-SIDE-EFFECT-AUDIT's finding; no production change).

Production evidence:
- `src/memory/extract/model.rs (whole production section, pre-#[cfg(test)])`

Regression evidence:
- `disposable_interface::disposable_model_calls_carry_no_pty_session_or_tool_surface`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/memory/extract/model.rs doc comment: /// `credential` is resolved by the caller, because resolving a -> /// `credential` is resolved by the caller, because resolving a SessionId or a | `introduce-the-surface` | **killed** | `disposable_interface::disposable_model_calls_carry_no_pty_session_or_tool_surface` |

> introduce-the-surface observed: assertion failed: !production_source.contains(forbidden) (forbidden = SessionId)

Recorded scope limits — stated by the worker, not discovered later:
- Text scan, not a type-level proof (SecretRef's stricter form does not apply here — Debug/Serialize cannot express 'has no PTY'). A forbidden term hidden via string concatenation or an alias import would not be caught.


---


### Do not give disposable jobs an autonomous coding-agent loop, unrestricted repository tools, or native-session identity. (line 1627)

Contract: Given the disposable-model call path, it carries no tool_calls or function_call surface and names no SessionId — while preserving the same scan-scope exclusion as 1626, since this is the same test.

State: COMPLETE — ruled 2026-08-31 by the orchestrator: a line the code already satisfied, proved by a test and a killed mutation (GH-MAP-SIDE-EFFECT-AUDIT's finding; no production change).

Production evidence:
- `src/memory/extract/model.rs (whole production section, pre-#[cfg(test)])`

Regression evidence:
- `disposable_interface::disposable_model_calls_carry_no_pty_session_or_tool_surface`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/memory/extract/model.rs doc comment: /// `credential` is resolved by the caller, because resolving a -> /// `credential` is resolved by the caller, because resolving a SessionId or a | `introduce-the-surface` | **killed** | `disposable_interface::disposable_model_calls_carry_no_pty_session_or_tool_surface` |

> introduce-the-surface observed: assertion failed: !production_source.contains(forbidden) (forbidden = SessionId)

Recorded scope limits — stated by the worker, not discovered later:
- Same as 1626's limit: a text scan, and shares that test/mutation rather than adding a second one — the three lines (1626-1628) are one mechanism, `ConfiguredModel::call`'s single HTTP POST.


---


### Do not pretend a disposable API call is a user-enterable worker session. (line 1628)

Contract: Given the disposable-model call, the request body is non-streaming ("stream": false) — one bounded call, never a user-enterable ongoing session.

State: COMPLETE — ruled 2026-08-31 by the orchestrator: a line the code already satisfied, proved by a test and a killed mutation (GH-MAP-SIDE-EFFECT-AUDIT's finding; no production change).

Production evidence:
- `src/memory/extract/model.rs` — `ConfiguredModel::body`

Regression evidence:
- `disposable_interface::disposable_model_calls_carry_no_pty_session_or_tool_surface`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/memory/extract/model.rs: "stream": false, -> "stream": true, | `flip-the-flag` | **killed** | `disposable_interface::disposable_model_calls_carry_no_pty_session_or_tool_surface` |

> flip-the-flag observed: assertion failed: production_source.contains("\"stream\": false")

Recorded scope limits — stated by the worker, not discovered later:
- Text scan of the literal, not a live HTTP call — proves the request Glasshouse builds asks for a non-streaming reply; does not exercise a real provider's response handling for that field.



---

### Line 1629 — which resource performed important support work, for debugging

Package `GH-HARNESS-EFFICIENCY`, 2026-08-31, Sonnet at high (Amber; same package as Phase 56's 1951).

### Record which resource performed important memory extraction or classification for debugging. (line 1629)

Contract: Given routing_observations rows in this project, when Glasshouse renders glasshouse route, it lists the most recent 10 rows whose purpose is classification or memory-extraction — when, purpose, provider, model, route, outcome, wall-clock — while never including a row with no purpose or a purpose outside that pair

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's artifacts (4/4 mutations KILLED with killing tests named; real `test result:` lines; three blast runs with every red attributed — a doc-link defect fixed, the rest pre-existing load flakes — and `integrate.sh`'s merged-tree blast, see the commit). Tokens are carried where a row has them and `token_rows_present == 0` never prints a zero (`print-zero-for-null-tokens` KILLED); token data begins arriving with translated pairs (T1).

Production evidence:
- `crates/glasshouse/src/routing/evidence.rs` — `EvidenceLedger::recent_support_work`
- `crates/glasshouse/src/main.rs` — `support_work_section`
- `crates/glasshouse/src/main.rs` — `route_report (wiring)`

Regression evidence:
- `support_work_debug::recent_support_work_reads_only_the_two_support_purposes_newest_first`
- `support_work_debug::the_route_command_prints_the_support_work_section`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| project_id = ?1 AND purpose IN (?2, ?3) -> project_id = ?1 AND (purpose IN (?2, ?3) OR harness IS NOT NULL) (routing/evidence.rs, recent_support_work) | `drop-purpose-filter` | **killed** | `support_work_debug::recent_support_work_reads_only_the_two_support_purposes_newest_first` |

> drop-purpose-filter observed: assertion left == right failed: the interactive coding-agent row (provider: "anyrouter", purpose: None, harness: Some("claude-code")) and the routing-latency row leaked into the result; recent.len() became 7 where 5 was asserted

Recorded scope limits — stated by the worker, not discovered later:
- no Windows leg run
- EvidenceLedger::recent (the exact-identity reader) is unchanged; this is a new sibling, not a rewrite of it

---

### Use disposable jobs for classification, memory extraction, reranking, and other bounded support tasks. (line 1625) — CLOSED 2026-09-02

**How it closed.** The 2026-08-31 refusal above rested on reranking's Cluster Q
absence, and `tests/disposable_interface.rs` carried a tripwire asserting no
production reranking caller existed, *"so that the day one is added it fails
loudly, by name"*. `GH-MEMORY-RERANKER` landed the reranking seat in wave 101
(`phase-24.md`), and the waves 101–102 trailing sweep fired the tripwire
exactly as designed. The orchestrator ruled rather than re-armed it.

**Contract.** Given Glasshouse's own support work, when it needs a bounded
model call, Glasshouse routes it as a disposable job through
`DisposableRouting` under one of its named kinds — classification, memory
extraction, reranking, context reduction — never as a native interactive
session, while preserving that each seat's failure is a stated bypass and
that the vocabulary stays closed (1621).

**Production evidence.** Classification —
`main.rs::choose_for_automatic_classification` (Phase 34C); memory
extraction — `main.rs::disposable_extraction_model`, `JobKind::MemoryExtraction`
(Phase 9I, `GH-ROUTED-EXTRACTION-CLIENT`); reranking —
`memory/rerank.rs::resolve_rerank_model`, `JobKind::Reranking` (Phase 24; in
the library because the machine door cannot call the binary crate); another
bounded support task — `main.rs::disposable_reducer`,
`JobKind::ContextReduction` (Phase 58's reducer seat).

**Regression evidence.** The census,
`disposable_interface::disposable_jobs_serve_classification_extraction_reranking_and_reduction_in_production`
(the inverted tripwire — four source assertions naming each caller), and the
behaviour of each seat through the shipped binary in its own package:
`classification_call.rs` (10), `routed_extraction.rs` (4),
`memory_reranker.rs` (8), `firewall_reducer.rs` (9) — each with its package's
KILLED mutations (`phase-34c.md`, `phase-9i.md`, `phase-24.md`,
`phase-58.md`). No mutation was run on the census itself: a source scan is a
census, not a behaviour proof (§35), and the behaviour proofs are the four
files above.

**Recorded limit.** *Other bounded support tasks* is proven by one — the
reducer; `JobKind::Evaluation` has no production caller yet and the census
does not claim it.

State: **COMPLETE**. **Phase 39 stands at 9 of 9.**
