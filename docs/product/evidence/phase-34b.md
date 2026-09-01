# Phase 34B — Routing-model role, 7 of 15 closed

Capability map lines 1413–1427. Read-only proof packet `GH-PROOF-ROUTER`,
worktree `.worktrees/proof-router`; full report in
`.agent-runtime/report-proof-router.md`. Integrated 2026-08-29.

## What kind of package this was

**No `src/**` change.** This package's job was to find out whether Phase 34B's
boxes are already true of shipped code, not to make them true — the routing-model
*config surface* was built in earlier phases and nobody had held it to these
lines. Its entire deliverable is `crates/glasshouse/tests/routing_model_config.rs`,
five tests.

Eleven candidate lines went in; **seven survived and four were refused**. The
refusals are recorded here at equal length, because a package that closes seven
of eleven and says which four it could not is worth more than one that closes
eleven.

**Every mutation below was run against production code, not against the tests** —
`shell/mod.rs`, `config/mod.rs`, `shell/view.rs`. A tests-only diff is exactly the
shape §14 warns about (a closure resting on existing tests), so the non-vacuity
burden is higher, not lower, and it was met by mutating the production paths the
tests enter through.

---

### Phase 34B — Define a dedicated routing_model role separate from interactive coding sessions and memory-extraction models (line 1413)

Contract: Given a user choosing a routing model, when they save it, Glasshouse
persists that choice as its own configuration surface, while leaving every other
model role — interactive sessions, memory extraction, providers, profiles,
pairing — untouched.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/shell/mod.rs` — `save_user_settings_with_routing`, the
  exact function the Settings overlay's `W` save calls.
- `crates/glasshouse/src/shell/mod.rs` — `apply_routing_edit` writes only the
  `routing` field of `UserConfig`.

Regression evidence:
- `routing_model_config.rs::routing_model_choice_is_a_distinct_config_surface_persisted_independently`
  — clones the pre-save `UserConfig`, mutates **only** its `routing` field by hand
  to build `expected`, saves through the real path, reloads, and asserts
  full-struct equality (`UserConfig` derives `PartialEq, Eq`). Entanglement with
  any other role would show up here as an inequality, without the test having to
  enumerate the roles.

Mutation: `apply_routing_edit`'s `model` branch forced to write a fixed
`Pinned{"MUTATED","MUTATED"}` regardless of the requested edit — **killed**
(this test and 1414–1417's together).

---

### Phase 34B — Allow the routing model to be a remote paid model (line 1414)<br>Phase 34B — Allow the routing model to be a free-tier remote model (line 1415)<br>Phase 34B — Allow the routing model to be a local model (line 1416)<br>Phase 34B — Allow GPT-5.6 Luna or another inexpensive fast model to be configured for the routing-model role when available to the user (line 1417)

Contract: Given any provider and model string, when a user pins it as the routing
model, Glasshouse stores and resolves it unchanged, while never restricting the
choice to a known list or a particular class of model.

State: COMPLETE (all four)

These four are one mechanism seen from four directions. Glasshouse does not model
"paid", "free-tier" and "local" as distinct routing-model kinds — a pin is a
`(provider, model)` pair and the classes are a property of the provider the pair
names. The right proof is therefore that *any* string round-trips, demonstrated
over one case per class.

Production evidence:
- `crates/glasshouse/src/config/mod.rs` — `EffectiveConfig::routing_model()`, the
  same function `shell::build_settings` calls to build the on-screen Routing row.
- `crates/glasshouse/src/shell/mod.rs` — `save_user_settings_with_routing`.

Regression evidence:
- `routing_model_config.rs::pinned_routing_model_accepts_any_provider_and_model_string_including_gpt_5_6_luna`
  — table-driven over four cases: a remote-paid-shaped name (1414), a
  free-tier-shaped name (1415), a local-shaped name (1416), and `gpt-5.6-luna`
  itself (1417). Saved through the production path, read back through
  `routing_model()`.

Mutation: as 1413's — **killed**, failing on the first case
(`"openrouter-remote-paid"`/`"claude-frontier-tier"` returned as
`"MUTATED"`/`"MUTATED"`).

---

### Phase 34B — Never hard-code GPT-5.6 Luna or any other specific model as a mandatory routing dependency (line 1418)

Contract: Given a completely fresh configuration, when Glasshouse resolves the
routing model, it resolves to deterministic heuristics, while never naming a
vendor's model as a default and never treating the absence of one as an error.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/config/mod.rs` — `routing_model()`'s final fallback is
  `Layered::new(RoutingModelChoice::Deterministic, Layer::Default)`.
- Read-only inspection for the "mandatory" clause: every `gpt-5.6-luna` string
  literal in the crate sits inside a `#[cfg(test)]` module (`config/mod.rs`,
  `onboarding/state.rs`, `onboarding/view.rs`). None outside test code.

Regression evidence:
- `routing_model_config.rs::fresh_configuration_resolves_to_deterministic_heuristics_with_no_vendor_hardcoded`
  — a `UserConfig` with nothing ever saved resolves to `Deterministic` at
  `Layer::Default`; not a model name, not an error.

Mutation: the final fallback changed to a hard-coded `Pinned{"MUTATED","MUTATED"}`
— **killed**, `left: Pinned { provider: "MUTATED", model: "MUTATED" }` /
`right: Deterministic`.

---

### Phase 34B — Prefer a routing model whose marginal decision cost is materially lower than the premium capacity it protects (line 1419)<br>…sufficient requests per minute (line 1420)<br>…low enough latency (line 1421)<br>…reliably returns the required structured classification schema (line 1422)<br>Phase 34B — Allow multiple routing-model candidates to form a fallback chain (line 1423)

State: NOT STARTED. Not claimed and not investigated by this package. All five
describe *preference among routing-model candidates*, and there is no candidate
set: a routing model is a single optional pin with a deterministic fallback.
These are blocked behind the same architectural wall recorded in
`.agent-runtime/WAVE-2-PLAN.md` — nothing in the shipped binary makes a routing
decision over a real candidate set.

---

### Phase 34B — Allow deterministic heuristics to remain the final fallback when every routing model is unavailable (line 1424)

Contract: Given a routing model pinned to a provider that is no longer
configured, when Glasshouse resolves the routing choice, it degrades to
deterministic heuristics, while never silently continuing to claim a pin it
cannot honour.

State: COMPLETE

**This closes through a different production caller than its packet cited**, and
the correction is the interesting part. The packet expected a key-driven path;
`WizardState::new` seeds `pending_routing` straight from the loaded `UserConfig`,
so a config pinning a vanished provider puts the wizard in the degraded state
**the moment the onboarding screen opens** — no key-driving needed.

Production evidence:
- `crates/glasshouse/src/config/mod.rs` — `RoutingModelChoice::resolve()`'s
  `Pinned` arm checks the named provider is still configured.
- `crates/glasshouse/src/onboarding/state.rs` — `WizardState::routing_selection()`,
  called by `routing_step()` on every redraw of the real onboarding screen
  (`onboarding::run` → `view::render`), calls `resolve()` directly.

Regression evidence:
- `routing_model_config.rs::a_pinned_routing_models_vanished_provider_degrades_to_heuristics_in_the_onboarding_wizard`
  — asserts on the `Debug` text of the returned `RoutingSelectionView`, because
  `onboarding::state` is a private module whose type cannot be named from an
  external integration test. This is the same reasoning `main.rs`'s own tests use
  for source-scanning rather than importing a private type.

Mutation: the provider-configured check removed from the `Pinned` arm, so a pin
always resolves as found — **killed**: *"a pin naming a provider that is no
longer configured must degrade to heuristics rather than silently keep claiming
the pin: Pinned { provider: "vanished-provider", model: "some-pinned-model" }"*.

---

### Phase 34B — Keep routing-model prompts short and exclude unnecessary repository history (line 1425)<br>Phase 34B — Do not send secrets, unrelated project memory, or full conversation histories to the routing model (line 1426)

State: NOT STARTED. Both describe the content of a prompt sent to a routing
model, and no routing model is ever called — see 1427.

---

### Phase 34B — Allow a user to route classifications through a privacy-preserving local model even when remote models are available (line 1427)

State: **NOT STARTED — refused, and the refusal is the finding.**

Nothing in the crate calls a routing model to classify anything, so there is no
data in transit for a local pin to protect. A test here would prove a fixture,
not the product.

**One correction to the worker's reasoning, made by the integrator.** The report
argued this from `classify_heuristically` and `TaskClassification::new` having
"zero callers outside their own module". That grep pattern was too narrow and the
claim is wrong as stated: `main.rs:144` calls
`glasshouse::routing::classify::report(&text)` for `glasshouse classify <text>`,
a real production caller shipped in Phase 35. The same miss was caught
independently by `GH-CLASSIFY-CALLER` in the same batch.

The refusal survives the correction, in the sharper form:
**the classifier's only production caller is a manual CLI diagnostic, not a
routing decision.** `glasshouse classify` prints a report when a user asks it to;
it does not route anything, and no `JobKind` reaching `DisposableRouting::choose`
carries a classification. That is the same wall five of six recons named — and
1427 asks for user control over a transit that does not happen.

---

## Related lines assessed by this package but owned elsewhere

- **1443** — settled separately in commit `845d2c9`: "resource diagnostics" means
  the CLI, and the line stays open. This package demonstrated and mutation-proved
  the TUI reading (`the_settings_screen_shows_the_currently_selected_routing_model`,
  killed by constant-ing `render_routing`'s `Pinned` arm) and confirmed the CLI
  reading is absent, then correctly declined to pick between them.
- **1457 / 1459** — refused for the caller reason above, with the same integrator
  correction applied. 1457 additionally has a real field-shape problem (a
  `likely_multi_turn: bool` where the line wants an expected duration class), but
  fixing the field would still leave a mechanism nothing calls.

## Platform evidence

`routing_model_config.rs` is configuration and rendering: no OS-specific claim in
any of these seven contracts. Run on macOS, Linux and Windows in
`scripts/ci-local.sh --windows-vm`.


---

## From `GH-ROUTING-ECONOMICS` (2026-08-31)

The routing-model selector package closed this phase's lines 1420, 1421, 1422, 1423, 1427 (1419 refused: no per-model price)

**1419 re-checked 2026-09-02 and still REFUSED — but both recorded reasons are
now stale, which is exactly how a line gets packaged by mistake.** Whoever picks
this up will find the old blockers false and should not conclude the line is
open:

- *"there is no candidate set"* (this file, above): **false.**
  `DisposableRouting::choose_for_automatic_classification`
  (`routing/disposable.rs:1409`) takes `candidates: &[DisposableCandidate]` and
  is called in production from `main.rs:7440`.
- *"no per-model price"*: **false as stated.**
  `PriceTable::price_for(provider, model)` exists
  (`provider/pricing.rs:117`), is loaded from the user's own `pricing.toml`,
  and is already read by `routing/session.rs` — Phase 32G's line 1307 proved
  it round-trips through the shipped binary.

**What is actually missing, verified rather than assumed:**

1. **The price does not reach the selector.** `DisposableCandidate` carries
   `cost: Cost` (`routing/disposable.rs:216`), and `Cost` is a two-variant
   enum — `Free` or `Metered` (`routing/mod.rs:139`). That is a *category*,
   not a marginal cost. `PriceTable` appears nowhere in `routing/disposable.rs`.
2. **"the premium capacity it protects" has no established producer at the
   call site.** The line asks for a *comparison* between two quantities, and
   only the first is even plumbed. Nothing at `main.rs:7440` has been shown to
   know which premium destination this classification is protecting or what it
   is worth.
3. **"materially lower" is an unmade threshold decision**, and belongs with the
   other capacity-band thresholds rather than invented in a packet.

**Successor, when someone takes it:** plumb `PriceTable` to `main.rs:7440` and
onto `DisposableCandidate` as a real per-token price (not a `Cost` variant)
first — that is a self-contained package with a provable Phase -1. Only then is
the comparison this line names writable, and the threshold is a ruling before
it is code.; the full entry — production sites, regression names, the 22 killed mutations and the four refusals with their producers — is in `phase-34c.md` under *Package GH-ROUTING-ECONOMICS*, because the mechanism (`DisposableRouting::choose_for_automatic_classification`) lives in that phase.


---

## From `GH-LAUNCH-CLASSIFIER` (2026-08-31)

The launch-path classifier package (router request schema, classification on the acting path) touched this phase's lines 1425, 1426 (closed — prompt ceiling and the structural no-secrets/no-history request). The full entry — production sites, regression names, the 23 killed mutations, the one honestly-survived one, and the missing producer for 1516/1517/1531 — is in `phase-34d.md`, *Phase 34D — router request schema* and *lines outside Phase 34D*, because the mechanism lives there.
