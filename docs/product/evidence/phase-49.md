# Phase 49 — Configuration

Sixteen boxes, `docs/product/capability-map.md` lines 1786–1801. Audit, close,
and fill the gaps against the existing `crates/glasshouse/src/config/` module
(`RoutingConfig`, `ProfileTable`, pairing/response sub-modules) rather than
new architecture.

## Pass 1 — audit

| Line | Status before this package | Symbol / file |
|---|---|---|
| 1786 | already satisfied | `UserConfig::load`/`save`, `config/mod.rs:1791,1805` |
| 1787 | already satisfied | `load_project_config`, `config/mod.rs:1959` |
| 1788 | already satisfied | `project_config_path` via `Project::scope().resolve`, `config/mod.rs:1946` |
| 1789 | already satisfied, one gap | structural test covered `UserConfig`; no file-level test for `ProjectConfig`'s own write path |
| 1790 | already satisfied | `session::select::resolve_executable`, `session/select.rs:554` |
| 1791 | partly | routing half already real (`RoutingModelChoice::Deterministic`); memory-extraction half had a real automatic trigger (`main.rs` `run_extraction_after_turn`) but no way to turn it off |
| 1792 | absent, no consumer | nothing named "quota" anywhere in the crate |
| 1793 | absent, no consumer | nothing named "budget" anywhere in the crate |
| 1794 | already satisfied | `RoutingConfig::premium_reserve_percent`, real caller `shell/mod.rs:1159` (Phase 2D settings) |
| 1795 | absent, no consumer | no "fallback chain" concept anywhere; routing-model selection is Phase 34C |
| 1796 | absent, no consumer | no "workload tier ceiling" concept anywhere |
| 1797 | absent, no consumer | `PairingClass::is_vendor_native` (`harness/pairing.rs:196`) has zero callers outside its own definition — the "soft prior" its own doc comment describes does not exist yet |
| 1798 | already satisfied | `response::ResponseConfig`, real end-to-end tests in `tests/response_profiles.rs` |
| 1799 | absent, buildable | no toggle existed; built this round |
| 1800 | already satisfied, undertested | generic `IntegrationTable`/`enabled` mechanism already applies to `IntegrationId::Cmux` with no special case; added a regression test naming it |
| 1801 | not a code box | argued below |

## Pass 2/3 — what this package did

### 1789 — closed further

`serialized_form_has_no_secret_capable_field` already proved `UserConfig`
cannot serialize a credential value, structurally and by a word-scan of its
serialized text. It never exercised `ProjectConfig`'s own write path —
`write_project_config_with_consent` — which is the file a project would
actually check into a repository, so the guarantee 1789 asks for was resting
one level removed from the tracked file itself.

Added `project_config_file_never_contains_a_planted_secret_value_across_every_table`
(`config/mod.rs`): builds a `ProjectConfig` populated across every component
table this module exposes (a provider with a base URL, headers, a
`credential_store` and `credential_env`; a profile; an integration
executable override; a pairing correction; a response preset; a pinned
routing model), plants a real-looking secret value in the environment
variable the provider's `credential_env` names, writes the file through
`write_project_config_with_consent`, and reads the raw bytes back off disk.
Asserts the planted value never appears, and — since a fixture this wide
legitimately writes `credential_env`/`credential_store` as *keys* — narrows a
supplementary word-scan (`token`/`secret`/`password`) to lines that are not
those keys' own declarations, rather than reusing the narrow-fixture
broad-scan the existing `UserConfig` test runs.

"Wide", per this packet's instruction, is comprehensiveness across every
table rather than a TUI viewport: TOML text is never truncated the way a
100-column render is, so §17's specific failure mode (an assertion passing
because the value was clipped off-screen) does not apply here. The existing
`no_credential_value_leaks_through_the_free_resource_editors` test in
`tests/settings_persistence.rs` already covers the literal TUI-render case at
both 100×30 and 400×60, which is where §17's risk actually lives in this
codebase.

Mutation-proof: not separately re-run — this test is a direct descendant of
`an_os_credential_reference_round_trips_through_configuration_without_its_value`,
which already plants a real value and would fail exactly the same way if a
serializer resolved a reference; the two share the same failure mode.

### 1791 / 1799 — the disable trio

Three independently-switchable automatic behaviours: routing, memory
extraction, response-profile injection.

**Routing.** Already real: `RoutingModelChoice::Deterministic`, set via
`RoutingConfig::set_model`, is exactly "no model classifies automatically;
heuristics do" — and it already has real callers (the onboarding wizard,
`shell`'s settings screen). No change needed.

**Memory extraction — built this round.** `main.rs`'s `report_hook_with`
already ran `run_extraction_after_turn` after every `TurnEnded { Completed }`
event, unconditionally — a real, live, automatic trigger (Phase 21), even
though the model behind it (`NoExtractionModel`) always answers
`Unavailable` until Phase 39 supplies a real one. There was no way to turn
this off.

Added:
- `UserConfig::memory_extraction: Option<bool>` and `ProjectConfig`'s mirror
  (`config/mod.rs`), with accessors. `None` means never decided and resolves
  to enabled — the same reasoning `RoutingConfig::model` already documents.
- `EffectiveConfig::memory_extraction_enabled() -> Layered<bool>`
  (`config/mod.rs:2225`), layered project-over-user-over-default like every
  other lookup on the type.
- `main.rs:1028`, `memory_extraction_enabled(runtime)`, gating the existing
  trigger call at `main.rs:1238`. Loads configuration the same way
  `disposable_extraction_model` already does on this exact path, and defaults
  to enabled on a read failure for the same reason that function does: a
  broken config file must not silently and permanently disable a working
  capability.

Regression: `memory_extraction_enabled_layers_project_over_user_over_default`
(`config/mod.rs:4636`) covers default/user/project layering. Mutation-proof:
flipped the `Layer::Default` fallback from `true` to `false` in production —
the test failed with a clear diff; restored, `ok`.

**What is not covered:** the one-line gate in `main.rs` itself
(`&& memory_extraction_enabled(runtime)`) has no binary-level regression test
within this package's file scope — the natural home for one is
`tests/events_lifecycle.rs`, which is this round's `FORBIDDEN FILES`
(another worker's). The config-layer function it calls is fully tested and
mutation-proofed; the one-line call-site wiring is not. Flagging this
honestly rather than claiming full closure: whoever owns that file should add
a binary-level case (plant `memory_extraction = false`, drive a
`TurnEnded { Completed }` hook event, assert no extraction attempt is logged).

**Response-profile injection — built this round.** Added
`ResponseConfig::enabled: Option<bool>` (`config/response.rs:171`) and
`EffectiveConfig::response_injection_enabled()` (`config/response.rs:323`),
consumed inside `EffectiveConfig::response_stack` to suppress the
file-configured `Role`-table, `Project` and `UserDefault` layers when
disabled. Deliberately does **not** suppress `TaskOverride`, `Session`, or
the role's *built-in* default preset when a role was explicitly requested —
those are a request made on one invocation, not automatic injection from a
file. `response_profile`/`response_stack` is the single function both
`glasshouse response` and the real launch path (`main.rs:393`, `main.rs:1837`)
call, so this reaches the shipped binary with no separate wiring needed in
`main.rs`.

Regression tests, all in `config/response.rs`'s own new `#[cfg(test)]`
module: `enabled_key_round_trips_and_absence_parses_to_never_decided`,
`injection_enabled_layers_project_over_user_over_default`,
`disabling_injection_suppresses_configured_layers_but_not_an_explicit_request`
(covers the `Role`, `Project` and `UserDefault` layers separately, plus the
explicit-session-preset carve-out), and the trio test below.

**Independence, all three.** `the_three_automatic_behaviours_disable_independently`
(`config/response.rs`) exercises five corners of the eight-combination space
(all-on, each one off alone, all-off) and asserts each behaviour's resolved
state depends only on its own field.

Mutation-proof, response half: removed the `injection_enabled` guard from the
`Project` layer arm — `disabling_injection_suppresses_configured_layers_but_not_an_explicit_request`
failed (it plants a project-level entry specifically to catch this); restored,
`ok`. Removed the guard from the `UserDefault` layer (`if injection_enabled`
→ `if true`) — the same test failed on its very first assertion; restored,
`ok`.

### 1794 — confirmed already closed, no new code

`RoutingConfig::premium_reserve_percent` / `PremiumReservePercent` already
exist with full validation (0–100), already reach a real caller
(`shell/mod.rs:1159` reads it for the Phase 2D settings screen; the same
file's edit path writes it back), and already have a mutation-proofed
round-trip/layering test
(`routing_policy_values_round_trip_layer_independently_and_reject_absurd_inputs`,
`config/mod.rs`). No change needed; cited here because the packet's own
recon flagged this line as "likely absent" and it was not.

### 1800 — confirmed already closed, added missing regression

`IntegrationTable`/`IntegrationConfig::enabled` is the same generic
tri-state mechanism used for every integration, `IntegrationId::Cmux`
included, with no cmux-specific carve-out anywhere in `config/`. The config
module has no concept of "detected" at all — that lives entirely in
`integrations::` and `onboarding::` — so an explicit decision recorded here
is structurally immune to whatever detection finds.
`onboarding::state::build_rows` (`onboarding/state.rs:1399`) reads exactly
this field (`existing.integrations().is_enabled(id)`) to seed the wizard's
cmux row regardless of live detection, which is the real production caller
(read-only verification; `onboarding/` is outside this package's file
scope).

Nothing exercised this for `IntegrationId::Cmux` specifically before now —
only harness IDs were named in the generic tri-state tests. Added
`cmux_can_be_explicitly_disabled_and_the_decision_is_ordinary_configuration`
(`config/mod.rs`), which disables cmux, asserts the decision is `Some(false)`
independent of any detection concept, and round-trips it through a real
save/load.

### 1786 / 1787 / 1788 / 1790 / 1798 — confirmed already closed, no new code

- 1786: `UserConfig::load`/`save` (`config/mod.rs:1791,1805`); a missing file
  loads as `UserConfig::default()` — `missing_file_loads_as_default`.
- 1787: `load_project_config` (`config/mod.rs:1959`) returns `Ok(None)` for a
  project that never created one — `project_config_is_never_created_automatically`.
- 1788: `project_config_path` resolves `.glasshouse/config.toml` through
  `Project::scope().resolve`, never a raw join —
  `project_config_path_is_resolved_through_the_project_scope`.
- 1790: no configuration at all still resolves to PATH discovery by
  candidate name — `EffectiveConfig::executable` returns `None` when neither
  layer has an override, and `session::select::resolve_executable`
  (`session/select.rs:554`, read-only verification) falls through to
  `id.executable_candidates()` in that case. This is exactly what makes a
  bare `claude`/`codex` on `PATH` sufficient with zero configuration.
- 1798: `response::ResponseConfig` (`config/response.rs`) already supports
  named presets and per-role overrides for all five `Role` values
  (orchestrator, worker, reviewer, explainer, interactive), with real
  end-to-end binary tests in `tests/response_profiles.rs`:
  `every_one_of_the_six_precedence_layers_can_win_in_the_binary`,
  `each_role_resolves_to_its_own_default_through_the_binary`, and others in
  that file (outside this package's scope; read-only verification, not
  edited).

## Left open, with the consumer named

- **1792** (provider-specific quota overrides) — nothing named "quota"
  exists anywhere in the crate. There is no per-provider health/rate-limit
  telemetry consumer to override in the absence of automatic telemetry;
  that machinery is Phase 32/34 work. Adding a config field with no reader
  would repeat exactly the mistake Phase 9J line 576 was left open for.
- **1793** (monthly/rolling monetary budget) — same reasoning; no cost-ledger
  consumer exists yet to spend against a configured ceiling.
- **1795** (routing-model fallback chain) — `RoutingModelChoice` records one
  choice (deterministic / automatic / pinned), with a single built-in
  fallback to heuristics when a pin's provider disappears
  (`RoutingModelChoice::resolve`). An ordered *chain* of several fallback
  candidates is a distinct, larger shape this module does not have a reader
  for; the reader is Phase 34C's routing-model selection.
- **1796** (workload-tier ceilings per model) — no "workload tier" concept
  exists in `crate::routing` to cap. Same phase dependency as 1795.
- **1797** (native-pairing preference strength) — `PairingClass::is_vendor_native`
  (`harness/pairing.rs:196`) is named and documented as exactly the hook a
  future routing prior would attach to, and has zero callers today. A
  strength knob configuring a prior that does not exist would be a field
  parsed and never read. Left open until the prior itself is built.

Ask, per §33's standard, whether the honest answer to "can a user configure
this today, and does it change something in the shipped binary" is yes. For
these five it is not yet — the missing half is a consumer that belongs to a
phase that has not shipped, not a missing config field this package could
add by itself.

## 1801 — argued, not coded

*"Keep configuration schema small until real usage demonstrates a need for
additional options."* This is a restraint claim, not a capability with a
production caller — there is no mechanism to build for it.

Argued against what this package actually added: three new fields
(`UserConfig`/`ProjectConfig::memory_extraction`, `ResponseConfig::enabled`),
each a single `Option<bool>`, each mirroring an existing decision a user can
already make somewhere in the product (a routing choice already toggles
deterministic-vs-automatic; a response profile is already something a user
configures) rather than inventing new policy surface. Five boxes (1792,
1793, 1795, 1796, 1797) were left open specifically *because* their consumer
does not exist yet, rather than adding a field now and hoping a later phase
reads it the way this line was written. That is the schema staying small on
purpose: closeable boxes closed with the smallest field that has a real
reader, and boxes with no reader left alone rather than pre-built.

## Gate

- `cargo doc -p glasshouse --no-deps` — one broken intra-doc link found and
  fixed (`config/response.rs`), clean after.
- `cargo build -p glasshouse --bins` — clean.
- `cargo clippy -p glasshouse --all-targets -- -D warnings` — clean.
- `cargo fmt` on the three touched files (`config/mod.rs`,
  `config/response.rs`, `main.rs`) — one reflow applied, clean after.
- `cargo test -p glasshouse --lib` — 1109 passed, 0 failed (run alone, not
  beside another `cargo` invocation — §40).
- `cargo test -p glasshouse --test settings_persistence --test launch_overlay`
  — 17 passed, 0 failed (the two test files this package owns).
- Did **not** run the full local gate (`scripts/ci-local.sh`) — out of this
  package's scope to invoke; the commands above are what this package ran
  and all of them are clean.

## Orchestrator verification: 1791 stays OPEN

The worker flagged that the memory-extraction half of this line rests on a single
`&&` at `main.rs:1238` with **no binary-level regression test**, because the
natural home is `tests/events_lifecycle.rs` — another worker's file that round.
It said so plainly rather than ticking through it.

**Checked, and it was right.** The orchestrator deleted
`&& memory_extraction_enabled(runtime)` from production, rebuilt, and ran the
whole suite:

    1138 lib + 24 bin + every integration binary — 0 failed

**Nothing noticed.** That is §35's rule exactly: *a caller you can delete without
a test noticing is, to the test suite, not a caller.* The config layer beneath it
is unit-tested and mutation-proofed, but the wiring that makes the switch reach
the shipped binary is proven by reading only, and reading is not evidence.

**So line 1791 is not closed.** The routing half is genuinely satisfied
(`RoutingModelChoice::Deterministic`, with real onboarding and settings callers);
the memory-extraction half has a production caller and no proof it is load-bearing.

The test that would close it is already designed, in the worker's own words:
plant `memory_extraction = false`, drive a `TurnEnded { Completed }` hook event
through the binary, and assert no extraction is attempted. Ten lines, in
`tests/events_lifecycle.rs`, free the moment Phase 10A's worker releases it.

Restored byte-identical afterwards; the tree the batch was integrated from is the
tree that passed 13/13.

### Phase 49 line 1797 — native-pairing preference strength, configurable

Contract: Given a user who wants Glasshouse to weight vendor-native
harness/model pairings more or less heavily, when they set
`native_pairing_preference` in a `[pairing]` table, Glasshouse records that
choice, layers it project-over-user, and shows it in effect — while preserving
the rule that no routing logic branches on a vendor's name.

State: **COMPLETE** for the configuration capability. **Phase 9J's eleven
scoring lines stay open** and this entry does not claim otherwise — see below.

Production evidence:
- `config/pairing.rs: PairingConfig::native_pairing_preference` — a field on
  the struct `config/mod.rs` already embeds by value in both the user and the
  project layer, so no `config/mod.rs` change was needed.
- `config/pairing.rs: EffectiveConfig::native_pairing_preference` — resolves
  project-over-user, falls back to `Strong`, and **names any layer whose
  spelling it had to ignore**.
- `config/pairing.rs: report` — the one line `main.rs`'s `pairing` arm calls,
  and the same function `tests/pairing.rs` enters through. The preference line
  is printed from there, so a mutation to the resolution is a mutation to the
  path the shipped binary runs (§35).

Regression evidence:
- `tests/pairing_prior.rs::the_report_the_binary_prints_names_the_configured_preference`
  — the report the binary prints names the configured value. Mutation M7
  (hard-code `"MUTATED"` instead of interpolating) killed it.
- `tests/pairing_prior.rs::an_unusable_preference_spelling_is_reported_rather_than_silently_defaulted`
  — added by the integrator, see the finding below. Mutation (make the source
  description drop the ignored list) killed it: *"the ignored spelling was
  swallowed instead of reported"*.
- Four `preference_for` layering tests: default, user layer, project layer,
  project-over-user.

Failure/isolation evidence:
- A spelling this build cannot parse is ignored **and reported**, with the
  layer that set it. Verified in the shipped binary, not only in tests.
- No vendor literal reaches production logic. Independently re-grepped by the
  integrator: the one match in `routing/mod.rs` is a pre-existing
  `"anthropic-messages"` wire-protocol slug in a `mod tests` fixture, and the
  two in `config/pairing.rs` are `IntegrationId::ClaudeCode` and a model id in
  tests. Every new function reads `PairingClass::is_vendor_native()`.

Platform/external evidence:
- Verified against the built binary on macOS across all four valid spellings,
  the unset case, and an invalid spelling. Batch 36 gate and `--windows-vm`
  to follow with the rest of the batch.

**Why this closes while Phase 9J's eleven do not.** The line asks that the user
be *able to configure* a preference strength. They can, it layers, and the
binary shows it. That is the same bar three sibling lines in this phase were
already ticked at — including *"Allow the user to configure protected reserve
percentages for premium subscriptions"*, which is checked today while Phase 32F,
the policy that would read it, is 0 of 8. The trailing clause *"without
hard-coding vendor-specific routing rules"* is a constraint on **how** it is
implemented, and it is satisfied and re-verified. Phase 9J's lines ask for a
*prior contributing to a routing decision*, which is a different claim with no
caller: nothing in this build ranks candidates at all.

**The worker left this box open and referred the judgement up** (practice §33).
It was right to; the orchestrator ticked it on the sibling-line precedent above,
and this paragraph records which way that went and why.

**Integrator's finding — the fix that came with the tick.** As delivered, an
unusable spelling fell back to `Strong` and reported *"from the default —
nothing configured"*, which is false when a value is sitting in the file. The
worker cited the right rule — every other field here degrades *visibly*, and a
bad `behaviour` prints back as `behaviour=nonsense` — and then did not
implement it. A person debugging their own configuration would have been told
to add the setting they had already added. Fixed in
`EffectiveConfig::native_pairing_preference` and
`describe_preference_source`, with the regression test above.

---

## Phase 49 line 1791 — CLOSED 2026-08-29 (batch 47)

**This supersedes "Orchestrator verification: 1791 stays OPEN" above.** That
section is kept, not deleted: it records the mutation that proved the gap, and
the gap is what this entry closes.

Contract: Given a project whose configuration disables automatic memory
extraction, when a harness reports a completed turn through `glasshouse hook`,
Glasshouse records the lifecycle event as usual but attempts no memory
extraction — while the same hook with extraction enabled does attempt it.

State: COMPLETE

**What was actually missing, and it was not the switch.** The routing half was
already satisfied (`RoutingModelChoice::Deterministic`, with real onboarding and
settings callers). The memory-extraction half had a production caller and no
proof it was load-bearing: deleting `&& memory_extraction_enabled(runtime)` from
`main.rs` left 1138 lib + 24 bin tests + every integration binary green. §35 —
a caller you can delete without a test noticing is, to the test suite, not a
caller.

Production evidence:
- `crates/glasshouse/src/main.rs:1200` — `memory_extraction_enabled(runtime)`,
  reading `EffectiveConfig::memory_extraction_enabled()`
  (`config/mod.rs:2634`), which layers project over user over default.
- `crates/glasshouse/src/main.rs:1490-1498` — the gate itself, on
  `LifecycleEvent::TurnEnded { outcome: TurnOutcome::Completed }`, guarding
  `run_extraction_after_turn`.
- **No production code was written to close this line.** The mechanism shipped
  already; only its proof was missing.

Regression evidence — `crates/glasshouse/tests/session_hook.rs`, which spawns
`env!("CARGO_BIN_EXE_glasshouse")` and runs `glasshouse hook` as a real
separate process:
- `memory_extraction_left_enabled_is_attempted_after_a_completed_turn` — the
  premise (§17): with no `memory_extraction` key written, the hook produces one
  of `run_extraction_after_turn`'s two `tracing::info!` lines.
- `memory_extraction_disabled_in_user_config_is_not_attempted_while_the_hook_still_records_the_turn`
  — the line: with `memory_extraction = false` planted through
  `UserConfig::set_memory_extraction` (not hand-written TOML), **neither** line
  appears, while the `TurnEnded { Completed }` event is still recorded — so the
  switch turned off extraction specifically, not the hook.

**The assertion is on the extraction log lines, deliberately not on an empty
memory database.** An empty database is exactly what a run that extracted and
stored nothing would leave, and that is the vacuous pass this line already
survived once.

Mutation, re-run by the orchestrator rather than accepted from the report:

| mutation | vocabulary | result |
|---|---|---|
| `&& memory_extraction_enabled(runtime)` → deleted (`main.rs:1495`) | `remove-guard` | **killed** — `memory_extraction_disabled_..._records_the_turn` FAILED at `session_hook.rs:508` |

The `--test session_hook` target is the one holding the killing test; checked,
because a SURVIVED from a command that never ran the killing test is
indistinguishable from a real one (`phase-40.md` records that exact error).

**A packet error worth keeping.** The packet placed this test in
`tests/events_lifecycle.rs`, following this ledger's own earlier note. That was
wrong: `events_lifecycle.rs` drives the library type `SessionRuntime`, and the
gate is in `main.rs` inside the binary, which no library seam reaches. Corrected
to `session_hook.rs` before dispatch.

Platform/external evidence: `session_hook.rs` is not platform-gated and runs on
Windows. No `#[cfg]` was added.

Missing evidence: CI run on all three platforms.


---

## From `GH-ROUTING-ECONOMICS` (2026-08-31)

The routing-model selector package closed this phase's lines 1795 (routing.model_fallback); the full entry — production sites, regression names, the 22 killed mutations and the four refusals with their producers — is in `phase-34c.md` under *Package GH-ROUTING-ECONOMICS*, because the mechanism (`DisposableRouting::choose_for_automatic_classification`) lives in that phase.

### Line 1796 — per-model workload-tier ceilings

Package `GH-TIER-CEILING`, 2026-08-31, Opus at high. Nine mutations, nine killed. The worker **refused OBJECTIVE 3** — attaching adapter-declared `ResourceFacts` to destinations — and the orchestrator verified the refusal: `capability_fit` (`routing/session.rs:786`) already reads `adapter_for(destination.harness())` and `prefer()` falls through to those declarations whenever the facts are `Unverified`, so the wiring would have changed no score and survived its own mutation; `Destination::with_resource_facts` keeps no production caller, deliberately.

### Allow the user to configure workload-tier ceilings for individual models. (line 1796)

Contract: Given a user who has written providers.<p>.model_ceilings = { "<model>" = "<tier>" }, when Glasshouse loads that configuration, it resolves the named model's ceiling as a WorkloadTier through EffectiveConfig::model_ceiling, layered project over user -- while preserving that an unknown spelling is a load error rather than an absent ceiling, and that a model, provider or layer nobody stated a ceiling for resolves to None rather than to a low one.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts.

Production evidence:
- `src/config/mod.rs` — `ProviderConfig::model_ceilings`
- `src/config/mod.rs` — `ProviderConfig::ceiling_of`
- `src/config/mod.rs` — `ConfiguredWorkloadTier (Serialize/Deserialize)`
- `src/config/mod.rs` — `EffectiveConfig::model_ceiling`

Regression evidence:
- `config::tests::every_workload_tier_spelling_round_trips`
- `config::tests::an_unknown_model_ceiling_spelling_is_refused_at_load_rather_than_read_as_absent`
- `config::tests::model_ceiling_is_layered_and_absent_where_nobody_stated_one`
- `config::tests::serialized_form_has_no_secret_capable_field`
- `tier_ceiling::a_configured_ceiling_excludes_a_destination_below_the_required_tier_on_the_shipped_binary`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| config/mod.rs: `return Layered::new(config.ceiling_of(model), Layer::User);` -> `return Layered::new(None, Layer::User);` | `skip-state-update` | **killed** | `tier_ceiling::tier_fit_orders_two_otherwise_equal_destinations` |

> skip-state-update observed: assertion `left == right` failed: a ceiling equal to the required tier is the fit the router should prefer; left: 0.0 right: 0.4 -- and three further tests failed with it

Recorded scope limits — stated by the worker, not discovered later:
- Only a `[providers.*]` key can carry a ceiling; a native subscription and the gateway never can, so a harness's own sign-in cannot be capped.
- The project layer replaces the user layer's whole map for that provider rather than merging into it -- the same replace-not-merge rule credential_env follows, asserted, not accidental.

