# Phase 58 — Context economy: cache-stable translation, entitlement-aware reduction, and a measured token budget

> Evidence for map lines 2014–2040. The phase was recorded on 2026-09-02 from the user's instruction after the Headroom comparison; `docs/product/design-decisions.md` (*Headroom, compared*) holds the reasoning, the refusals and the order of work. Read that entry before re-deriving any of it.

## Lines 2014–2018 CLOSED, 2019 OPEN — 2026-09-02 (`GH-TRANSLATE-CACHE-STABILITY`, Amber, Sonnet high): a default Claude Code launch is served on every translated pair, and the encoders are deterministic by construction

Before this package the Anthropic decoder refused `cache_control` by field name at four seams, so a default Claude Code launch on any translated pairing needed `DISABLE_PROMPT_CACHING=1` (`phase-56.md`, T1's recorded limit). Now the decoder carries the marker into one flag on the canonical form, `Request::cache_requested` — the worker's own representation, chosen over the packet's per-block bool because no target this gateway translates to has a per-block cache primitive: OpenAI's `prompt_cache_key` and Gemini's cached-content resource are request- or session-scoped, so a marker's *position* is information nothing downstream can use and only the *fact* is carried. OpenAI Chat and Responses emit `prompt_cache_key` from `Request::user` (Claude Code's own per-session `metadata.user_id`, already sent to the provider as `user`, so nothing new crosses the wire) and emit nothing when the harness set no user id; Gemini strips, and the strip is recorded in the pair table's `FieldRows.cache` (`CacheDisposition::Stripped` with a reason) and logged per exchange at `serve` under the gateway's opt-in debug logging — the same convention every `Exchange::record` line follows. Tools are ordered by name once, in `serve` through `Request::normalized`, before any codec encodes; JSON-Schema key order was already sorted (this crate never enables `serde_json`'s `preserve_order`) and is now pinned by a tripwire. 4/4 mutations KILLED with output quoted; `gateway_translate_cache` 6/6, `gateway_translate` 9/9 (the old refusal test moved onto `thinking`), `--lib gateway::translate` 73/73, the five sibling translate suites 29/29, targeted blast green, rustdoc clean.

**Packet error, the orchestrator's:** the packet named `provider/mod.rs:854` as the pair table's production printer; it is a test, and `field_rows()` has no production caller — the field rows have never been printed by the shipped binary. The recorded reason therefore reaches a user through the debug log only. Recorded as a Green candidate in the register (`GH-PAIR-TABLE-PRINT`).

### Carry a harness's prompt-cache markers across a translated pairing where the target protocol has an equivalent, and strip them with a recorded reason where it does not, instead of refusing the request. (line 2014)

Contract: Given a Claude Code request carrying `cache_control` on the system, a content block or a tool definition, when it is translated, Glasshouse carries the marker onto the canonical form and either emits the target's own hint (OpenAI: `prompt_cache_key`) or strips it with the reason recorded in the pair table's field rows and the exchange's debug record (Gemini), while preserving that every other refused field still refuses by name with nothing opened upstream.

State: **COMPLETE** — ruled 2026-09-02.

Production evidence:
- `crates/glasshouse/src/gateway/translate/canonical.rs` — `Request::cache_requested`, `Request::prompt_cache_key`
- `crates/glasshouse/src/gateway/translate/anthropic.rs` — `decode_request`, `decode_block`, `text_block`, `decode_tool` (the four seams carry; `REFUSED_FIELDS` no longer names `cache_control`)
- `crates/glasshouse/src/gateway/translate/mod.rs` — `CacheDisposition`, `FieldRows::cache`, `Codec::cache_disposition`, `serve` (the strip record)
- `openai_chat.rs`, `openai_responses.rs` (`Carried`), `gemini.rs` (`Stripped`, with its reason)

Regression evidence:
- `gateway_translate_cache::cache_control_is_carried_as_prompt_cache_key_and_the_read_ratio_still_reaches_the_ledger`
- `gateway_translate_cache::the_same_request_at_a_gemini_only_fixture_is_served_with_the_marker_stripped`
- `gateway::translate::anthropic::tests::cache_control_is_carried_not_refused_at_every_seam`; `gateway::translate::tests::field_rows_exist_for_every_codec_and_for_nothing_else`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| `gemini.rs` `refuse_unencodable`: an early `Err(Unsupported)` when `cache_requested` | `refuse-instead-of-strip` | **killed** | `the_same_request_at_a_gemini_only_fixture_is_served_with_the_marker_stripped` |

> refuse-instead-of-strip observed: panicked at tests/gateway_translate_cache.rs:679:5 (the `HTTP/1.1 200` assertion; the mutated Gemini path answers 400)

Recorded limits: the Anthropic *encoder* (Anthropic as target) does not re-emit `cache_control` from the flag — no supported source ever sets it there; the pair table's field rows have no production printer (above).

### Keep a default Claude Code launch usable on every supported translated pairing without the user disabling prompt caching. (line 2015)

Contract: Given a default Claude Code launch on a translated pairing, when its first request carries `cache_control`, Glasshouse serves it, while preserving that the launch link (`profile::apply_gateway`) is unchanged.

State: **COMPLETE** — ruled 2026-09-02. Proven through `glasshouse launch` itself on the chat pairing (`a_claude_code_launch_on_a_chat_only_entitlement_serves_cache_control_without_the_switch`); the responses and Gemini pairings share the one decoder and the two fixture tests above, so no per-target refusal can remain. Recorded limit: the launch-driven test covers the chat pairing only.

### Serialize translated requests deterministically — stable tool order and stable JSON Schema key order — so an unchanged prompt prefix stays byte-identical across turns. (line 2016)

Contract: Given a translated request, when it crosses `serve`, Glasshouse orders the tool definitions by name once before any codec encodes it, and JSON object keys serialize sorted, while preserving that no encoder is changed in what it emits for a given canonical form.

State: **COMPLETE** — ruled 2026-09-02.

Production evidence: `canonical.rs` — `Request::normalized`; `mod.rs` — `serve` (the call after decode).

Regression evidence: `gateway_translate_cache::the_same_tools_in_two_orders_encode_to_the_same_bytes`; `canonical::tests::normalized_sorts_tools_by_name_regardless_of_the_harnesss_order`; `canonical::tests::json_object_keys_serialize_sorted_because_this_crate_never_enables_preserve_order` (the tripwire for a future feature unification).

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| `Request::normalized`: the sort removed | `unstable-tool-order` | **killed** | `the_same_tools_in_two_orders_encode_to_the_same_bytes` |

> unstable-tool-order observed: assertion `left == right` failed: the harness listed the tools in opposite orders; the wire bytes must not differ

### Never alter the bytes of a message already sent upstream in an earlier turn of the same session on a translated pairing, as the relay already guarantees for native ones. (line 2017)

Contract: Given a second turn that repeats an earlier turn's messages verbatim plus new ones, when it is encoded, Glasshouse produces the same bytes for the repeated prefix (system, tools, earlier messages) as before, while preserving that the relay path is untouched.

State: **COMPLETE** — ruled 2026-09-02. The property is each encoder being a pure function of the canonical form; proven end to end on the chat pairing and by inspection on the other two (no `SystemTime`, `Uuid`, `rand`, counter or timestamp in any encoder). Recorded limit, the worker's: one end-to-end target; the orchestrator accepts on the purity argument and the mutation below, and names a second target as cheap insurance for a later Green.

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| `openai_chat.rs` `encode_request`: the system message gains a `SystemTime::now()` suffix | `rewrite-a-sent-message` | **killed** | `a_second_turn_repeats_the_first_turns_encoded_prefix_byte_for_byte` |

> rewrite-a-sent-message observed: assertion `left == right` failed: the system segment and turn one's messages must stay byte-identical in turn two

### Supply a stable per-session prompt-cache key on targets that accept one when the harness did not set its own. (line 2018)

Contract: Given a translated request whose harness set `metadata.user_id`, when the target accepts a cache-routing hint, Glasshouse emits `prompt_cache_key` set to that value, while preserving that a request with no user id gets no key and that no key is ever derived from a credential or the gateway token.

State: **COMPLETE** — ruled 2026-09-02.

Production evidence: `canonical.rs` — `Request::prompt_cache_key`; `openai_chat.rs`, `openai_responses.rs` — `encode_request`.

Regression evidence: `gateway_translate_cache::cache_control_is_carried_as_prompt_cache_key_and_the_read_ratio_still_reaches_the_ledger`; `gateway_translate_cache::a_request_with_no_user_id_gets_no_prompt_cache_key`.

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| `prompt_cache_key` emitted from a constant when `user` is `None` | `key-from-nothing` | **killed** | `a_request_with_no_user_id_gets_no_prompt_cache_key` |

> key-from-nothing observed: assertion `left == right` failed: no user id in the request, so no key is invented for it

Recorded limit: the Responses decoder still refuses a client-set `prompt_cache_key` when Responses is the *source* protocol (unchanged, out of this box).

### Measure prompt-cache read and creation tokens per exchange where the provider reports them, and show the per-session cache ratio beside the routing evidence. (line 2019)

State: **PARTIALLY VERIFIED** — ruled 2026-09-02. The measuring half is production since Phase 56: `Usage.cached` decoded from `cached_tokens` (OpenAI) and `cachedContentTokenCount` (Gemini) → `Exchange.tokens` → `with_tokens` → `routing_observations.cached_input_tokens`, proven end to end by the first test above (fixture `cached_tokens` → the harness's `cache_read_input_tokens` → the ledger row). No translated target reports cache *creation* tokens distinctly — a wire-protocol limit, not a column to add. The readout half is `GH-SAVINGS-READOUT`'s, and the *per-session* clause has no producer: `routing_observations` carries no session column (`database.rs:1299`), so the ratio the readout can show is per route and per credential — the packet says so, and the line closes only when a session identity reaches the rows, which is a schema decision (Cluster G).

---

## Lines 2023 and 2024 CLOSED — 2026-09-02 (`GH-FIREWALL-ENTITLEMENT-POLICY`, Amber, Sonnet high): the reduction policy follows what pays

`install_context_firewall_hook` classifies the launch's serving entitlement once — a provider the registry marks `Local` (through the entitlement's backing, or the launch's own direct-provider backend when no entitlement describes it) → `Local`; `EntitlementKind::{Claude, ChatGpt, Gemini}` → `Subscription`; `ApiKey` → `Metered`; an unresolved entitlement → none, never guessed — and resolves mode, passthrough threshold and minimum semantic tokens through three new `EffectiveConfig` accessors with precedence **profile > entitlement > kind > flat > constant**, stated once with its reason (the profile is the more specific choice; the entitlement is what pays). `[context_firewall.subscription|metered|local]` sub-tables and `[profiles.<p>.context_firewall]` / `[entitlements.<e>.context_firewall]` overrides share one `ContextFirewallOverride` shape (mode, passthrough, aggressive passthrough, min semantic tokens); the reducer name and model stay flat because they are a resource, not a policy. `--min-semantic-tokens` is baked into the registered command line for the first time — `context_firewall_min_semantic_tokens()` had no production caller before this package, which the packet's Phase −1 stated. The firewall core and the hook subprocess remain entitlement-blind: numbers and a mode word are all that reach them. 3/3 mutations KILLED; `firewall_bridge` 17/17, `--lib config` 156/156, `--lib config::firewall` 13/13, `--lib harness::claude_code` 18/18, targeted blast green.

### Key the context firewall's reduction policy on the serving entitlement's kind, with per-kind thresholds that default to today's values. (line 2023)

Contract: Given a Claude Code launch whose serving entitlement is resolved, absent, or backed by a local provider, when Glasshouse registers the session's context-firewall hook, it classifies the entitlement into a reduction-policy kind and resolves the thresholds from that kind's sub-table, falling through to the flat table and the constant, while preserving that an unresolved entitlement with no local backend gets today's values exactly and a byte-identical command line.

State: **COMPLETE** — ruled 2026-09-02, with one recorded limit: the `Local` arm has no shipped-binary test because no local-inference fixture exists in the suite; it reuses the unedited, tested `ResourceKind::locality()` and is read in the diff. A Green follow-up test is owed the day a local fixture exists.

Production evidence:
- `crates/glasshouse/src/config/firewall.rs` — `ReductionPolicyKind`, `ContextFirewallConfig::kind_override`, the three sub-tables
- `crates/glasshouse/src/config/mod.rs` — `EffectiveConfig::context_firewall_policy_mode`, `context_firewall_policy_passthrough_tokens`, `context_firewall_policy_min_semantic_tokens`
- `crates/glasshouse/src/main.rs` — `install_context_firewall_hook` (the classification), its call site in `launch_session`
- `crates/glasshouse/src/harness/claude_code.rs` — `context_firewall_command_line` (`--min-semantic-tokens`)

Regression evidence (`tests/firewall_bridge.rs`, through the shipped binary): `line_2023_a_metered_entitlement_carries_the_metered_kinds_threshold`, `line_2023_a_subscription_entitlement_never_reads_the_metered_kinds_threshold`, `line_2023_no_entitlement_never_guesses_a_kind_and_stays_byte_identical`, `line_2023_a_subscription_kinds_min_semantic_tokens_reaches_the_registered_line`.

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| the classification always answers `None` | `ignore-the-kind` | **killed** | `line_2023_a_metered_entitlement_carries_the_metered_kinds_threshold` |

> ignore-the-kind observed: panicked at tests/firewall_bridge.rs:705:5 (`--passthrough-tokens 900` absent; the flat 4000 was baked)

### Allow a launch profile or an entitlement to declare its reduction policy explicitly, overriding the kind's default. (line 2024)

Contract: Given a profile or an entitlement with its own `[context_firewall]` override, when the policy is resolved, Glasshouse lets it outrank the kind's default with the profile above the entitlement, while preserving that a launch with neither falls through unchanged.

State: **COMPLETE** — ruled 2026-09-02.

Production evidence: `config/firewall.rs` — `ContextFirewallOverride`; `config/mod.rs` — `ProfileConfig::context_firewall`, `EntitlementConfig::context_firewall`, `ResolvedEntitlement::context_firewall`, the profile and entitlement branches of the three accessors.

Regression evidence: `line_2024_a_profile_override_outranks_the_kinds_threshold`, `line_2024_an_entitlement_override_outranks_the_kind_and_loses_to_the_profile`.

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| the profile-override branch can never fire | `override-is-ignored` | **killed** | `line_2024_a_profile_override_outranks_the_kinds_threshold` |
| the kind's sub-table consulted before the entitlement's override | `wrong-precedence` | **killed** | `line_2024_an_entitlement_override_outranks_the_kind_and_loses_to_the_profile` |

> override-is-ignored observed: panicked at tests/firewall_bridge.rs:757:5 (`--passthrough-tokens 700` absent; the kind's 900 won)

> wrong-precedence observed: panicked at tests/firewall_bridge.rs:781:5 (`--passthrough-tokens 800` absent; the kind's 900 won)

Recorded limit: `--min-semantic-tokens` is asserted on the registered command line; the hook subcommand's parsing of it is pre-existing plumbing.

---

## Lines 2034 and 2035 CLOSED, 2019 and 627 OPEN — 2026-09-02 (`GH-SAVINGS-READOUT`, Amber, Sonnet medium): a savings claim is a query, and the ladder's ratios are a checked-in table

`glasshouse routing-cost` prints a `SAVINGS` section after its per-purpose groups with three facets, each over rows production already writes and each carrying its denominator: *context firewall* — estimated tokens kept local (`RawStore::savings_in_window`, the raw store's per-entry estimates, never the ledger's token columns, per the 1987 ruling) across `R` reductions of `R+B` results above threshold, `R` read from the raw store because the ledger's reduction purpose holds two rows per semantic reduction (the bookkeeping row and the reducer-call row) and would double-count, `B` from the existing bypass-purpose group; *translation* — prompt-cache reads over translated input tokens per `(route, quota_context)` from `EvidenceLedger::translation_cache_savings`, whose SQL keeps only harness-turn rows with `input_tokens IS NOT NULL`, so a relayed row is out of the denominator by construction; *response profile* — the words *not counted: no exchange row carries a response profile*, with the code comment naming what a producer would need. A seeded corpus (`Xorshift64`, one seed, five samples over the ladder's rule families) lives under `tests/fixtures/firewall/` with `ratios.txt`, and `firewall_ladder_proof` recomputes the table from `firewall::reduce::reduce` and asserts it byte-identical. 4/4 mutations KILLED; `savings_readout` 3/3, `firewall_ladder_proof` 3/3 (+1 ignored generator), `--lib routing::evidence firewall::` 135/135, `routing_cost` 8/8, `firewall_observability` 16/16, `gateway_translate_evidence` 2/2, targeted blast green.

The reader test plants harness-turn rows through `EvidenceLedger::record` in-process, disclosed in the file header: the producer of a translated row is proven in `gateway_translate_evidence`, and this file tests the reader and the renderer.

### Report token savings by purpose — firewall reduction, response profile, translation — from the evidence ledger's own rows with denominators, so a savings claim is a query over recorded exchanges. (line 2034)

Contract: Given the ledger and the raw store as production writes them, when the user runs `glasshouse routing-cost`, Glasshouse prints, per purpose, what was saved and over what denominator, while preserving that a quantity nobody recorded prints as words and never as a digit, and that a relayed exchange is never counted as translated.

State: **COMPLETE** — ruled 2026-09-02. The response-profile facet is honest words, not a figure: no row carries a profile and no session column joins one, which is the same schema fact that keeps 2019 open. The line asks for a query over recorded rows with denominators, and that is what each facet is.

Production evidence: `firewall/store.rs` — `WindowSavings`, `RawStore::savings_in_window`; `routing/evidence.rs` — `TranslationSavings`, `EvidenceLedger::translation_cache_savings`; `main.rs` — `routing_cost_report`, `render_savings_section`.

Regression evidence: `savings_readout::firewall_facet_counts_a_real_reduction_with_a_positive_estimate` (a real hook run through the shipped binary); `savings_readout::translation_facet_sums_cached_and_input_tokens_and_excludes_relayed_rows`; `savings_readout::every_facet_reports_not_counted_words_and_no_digit_when_nothing_is_recorded`; `firewall::store::tests::an_entry_with_no_forwarded_estimate_counts_toward_results_never_toward_kept_local`, `…::an_entry_outside_the_window_is_excluded_entirely`.

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| an unestimated entry folded into `kept_local` | `claim-the-unestimated` | **killed** | `an_entry_with_no_forwarded_estimate_counts_toward_results_never_toward_kept_local` |
| `AND input_tokens IS NOT NULL` dropped from the translation query | `relay-rows-counted` | **killed** | `translation_facet_sums_cached_and_input_tokens_and_excludes_relayed_rows` |
| the response-profile line prints `0 exchange rows …` | `digit-for-nothing` | **killed** | `every_facet_reports_not_counted_words_and_no_digit_when_nothing_is_recorded` |

> claim-the-unestimated observed: assertion `left == right` failed comparing kept_local (mutant produced 10059, expected 60)

> relay-rows-counted observed: routing-cost's exit status failed once a pure-relay route's NULL SUM(input_tokens) could not read into a non-Option i64

> digit-for-nothing observed: assertion failed on contains("not counted: no exchange row carries a response profile")

Recorded limits: the translation facet is per route and credential, not per session (2019); a raw-store I/O error renders as *not counted*, indistinguishable from an unused firewall by design; `WindowSavings::sessions` is computed and not rendered.

### Provide a seeded, offline proof fixture for the firewall's deterministic ladder so its reduction ratios are reproducible without any provider. (line 2035)

Contract: Given the checked-in seeded corpus, when it is run through `firewall::reduce::reduce`, Glasshouse's fixture reproduces the same ratio table on any machine with no provider involved, while preserving that the fixture measures the ladder and never tunes it.

State: **COMPLETE** — ruled 2026-09-02. The table: `duplicate_hits 0.3340`, `repeated_log_progress 0.0414`, `blank_line_runs 0.9301`, `generated_noise_blob 0.2000`, `all_uncertain 1.0000` (original → forwarded estimates in `ratios.txt`).

Regression evidence: `firewall_ladder_proof::ratio_table_recomputed_from_reduce_is_byte_identical`, `…::checked_in_corpus_matches_the_seeded_generator`, `…::the_all_uncertain_sample_survives_whole`.

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| `reduce.rs`: `if prev_line_was_blank {` → `if false {` | `ladder-drift` | **killed** | `ratio_table_recomputed_from_reduce_is_byte_identical` |

> ladder-drift observed: assertion `left == right` failed: the recomputed blank_line_runs row no longer matched the checked-in ratios.txt

**2019 stays open** (the entry above): the readout now shows the cache ratio per route and credential beside the routing evidence; *per session* needs a session identity on `routing_observations`, a schema decision recorded in the register. **627 stays open**: no row carries a response profile or any of the line's other four facets; the readout says so in words.

---

## Line 2039 OPEN — 2026-09-02 (`GH-RECON-EFFORT-CLAMP`, Sonnet medium, read-only): the clamp has nothing to clamp

The recon read every codec in full. `canonical::Request` carries no effort or reasoning field; Claude Code's `thinking` object is refused at the shared decode seam (`anthropic.rs`, `REFUSED_FIELDS`, before any target is selected — so the reason text's *"no OpenAI Chat equivalent"* is stale: the refusal fires for all three targets); no encoder emits `reasoning_effort`, `reasoning.effort` or `thinkingConfig`, and each decoder refuses the harness-side equivalent. A default launch's use of `thinking` cannot be verified from this repository (`MAX_THINKING_TOKENS` is Claude Code's own variable, not ours). Of the four fixture requests carrying a `tool_result`, none is a pure tool-resume turn. The ledger's `outcome` on a harness-turn row is a transport-level 2xx proxy and cannot judge quality; the harness's `TurnEnded` verdict is the honest signal and is not joined to those rows today.

**Ruling.** *Clamp-only, never raising* is not yet well-defined against current code: there is no harness-stated level to clamp from and no encoder mapping to clamp within. Neither `GH-EFFORT-CLAMP-SHADOW` nor `GH-EFFORT-CLAMP` passes Phase −1. The successor is a **design note before a packet** (the local reducer's shape): an effort-shaped field on the canonical form carried the way `cache_requested` is, a decode-side carry of `thinking` instead of a refusal (its own scope: it changes today's all-or-nothing answer for every Claude Code request that sets `thinking`), a researched per-target mapping — and only then the shadow measurement joined to `TurnEnded`. Register: *Phase 58, after its first four packages*. The stale reason text is a Green residue.

---

## Line 2040 CLOSED — 2026-09-02 (`GH-MEMORY-EXPORT`, Amber-light, Sonnet medium): remembered constraints and failed approaches, on request, into the harness's local file

`glasshouse memory export-local [--harness claude-code] [--limit N] [--no-exclude]` — a sibling verb to Phase 50's `memory export --tracked` (the worker's first report blocked on the name collision, correctly; the orchestrator ruled the sibling verb). It lists current binding memories (`MemoryStore::binding`) and current failed attempts (the one new reader, `MemoryStore::current_of_kind`, modelled on `binding`), renders each with the injection path's own `render_entry` (widened to crate scope, a visibility change only), and splices the block between `<!-- glasshouse:memory:begin -->` / `<!-- glasshouse:memory:end -->` in `CLAUDE.local.md` at the project root — replacing only its own block, appending when absent, removing it when nothing is left, never creating the file for nothing. The file is excluded through `.git/info/exclude` by default (never the user's `.gitignore`), with the same bounded line match Phase 50 uses because the fixtures' bare `.git` directories cannot answer `git check-ignore`. Codex and Gemini CLI are refused by name: their instruction files are tracked, not local. 3/3 mutations KILLED; `memory_export` 7/7, `--lib memory` 140/140, `tracked_knowledge` 5/5 (Phase 50 untouched), targeted blast green, rustdoc clean.

**Packet errors, the orchestrator's:** the packet named `MemoryCommand::Export` and `memory/export.rs` as new; both were Phase 50's. Grep `cli.rs` and `src/` for a verb and a module before naming them.

### Offer an opt-in export of remembered constraints and failed approaches into a marker-delimited block of the harness's native local instruction file, gitignored by default, replacing only its own block on re-export. (line 2040)

Contract: Given a project with durable memory, when the user runs `memory export-local`, Glasshouse writes the current constraints, invariants and failed approaches as a marker-delimited block into `CLAUDE.local.md`, replacing only its own block and excluding the file from git by default, while preserving that nothing runs automatically, that bytes outside the block are never changed, that a harness without a local instruction file is refused by name, and that only this project's memory can appear.

State: **COMPLETE** — ruled 2026-09-02.

Production evidence: `memory/export_local.rs` — `export`, `LocalHarness::parse`, `splice`, `ensure_excluded`; `memory/store.rs` — `MemoryStore::current_of_kind`; `main.rs` — `memory_export_local`; `cli.rs` — `MemoryCommand::ExportLocal`.

Regression evidence (`tests/memory_export.rs`, through the shipped binary): `exports_only_binding_memories_and_failed_attempts_never_decisions`, `reexport_replaces_only_the_block_leaving_surrounding_bytes_identical`, `superseding_the_only_constraint_removes_it_from_the_block`, `exporting_with_nothing_left_removes_the_block_and_keeps_user_text`, `the_exclude_file_gains_the_pattern_once_and_git_status_sees_nothing` (a real `git init`), `no_exclude_leaves_the_exclude_file_untouched`, `an_unsupported_harness_is_refused_by_name_and_nothing_is_written`.

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| `splice`'s replace branch returns the block alone | `clobber-the-file` | **killed** | `reexport_replaces_only_the_block_leaving_surrounding_bytes_identical` |
| `current_of_kind(FailedAttempt)` → every active memory | `export-everything` | **killed** | `exports_only_binding_memories_and_failed_attempts_never_decisions` |
| `binding`'s `status = ?2` → `status != ?2` | `ignore-supersession` | **killed** | `superseding_the_only_constraint_removes_it_from_the_block` |

> clobber-the-file observed: panicked at tests/memory_export.rs:234:5 (text above the block did not survive)

> export-everything observed: panicked at tests/memory_export.rs:191:5 (content.contains("kind=decision"))

> ignore-supersession observed: assertion failed: first_content.contains("kind=constraint")

Recorded limits, the worker's: the exclude match is exact-line, not a glob engine (Phase 50's own limit); the export's temporary file name carries the pid but no random part (no production caller runs two exports at once); macOS only; the header's timestamp is a Unix second.

---
