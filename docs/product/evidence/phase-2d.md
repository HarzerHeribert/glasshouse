# Capability evidence — phase 2d

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 2D — the Providers and Launch Profiles settings sections (four lines)

Contract: Given a user in the settings view, when they manage providers or
launch profiles, Glasshouse lets them add, edit, disable, duplicate and remove
those entries — while preserving: **no secret value is ever displayed**,
user-level defaults stay visually distinct from project-level overrides, a
project-level write still needs explicit confirmation, and disabling is
reversible without retyping anything.

State: **COMPLETE** for the four settings lines. Phase 9D line 426 (provider
connectivity testing) stays **open** — see below.

#### Line 426 stopped exactly where the packet told it to

Glasshouse has no HTTP client on the branch this batch was cut from, and the
packet forbade adding one because the concurrent gateway batch was introducing
`ureq`. So the "test" affordance is a **reachability precondition check** —
provider resolves, protocol declared, base URL non-empty, credential variable
present — named honestly in the UI as such, and the line is left unchecked.
`ureq` is now on `main`, so a follow-up can make it a real request.

#### The orchestrator's mutation found a weak test, not a weak mutation

Acceptance test 7 asked that no credential value ever render. The test plants a
real environment variable, drives nine settings screens and asserts `!contains`.
It looked thorough. **It passed a mutation that renders the credential's value
instead of `set`/`not set`.**

The reason is the finding: at the test's 100-column render the providers row is
**truncated**, so a leaked 46-character value was clipped off-screen. The test
was passing for a reason that had nothing to do with the code. Re-rendered at
400 columns with the same mutation, it fails.

Every snapshot in that test is now captured at **both** a realistic and a wide
size, and the mutation is caught. Verified in both directions: hardened test +
mutation → FAILED; hardened test + clean code → ok.

**The general lesson, which is new to this project's practice:** a test that
asserts the *absence* of a string in rendered output is only as strong as the
viewport it renders into. Truncation makes absence trivially true.

#### CI evidence

**CI `32911326442` green on Linux, macOS, Windows and lint** at `2bdd89f`,
with the three decisive tests confirmed by name in the **Windows** job's own
log: `no_credential_value_is_ever_rendered_across_every_settings_screen` (the
one the orchestrator's mutation hardened),
`an_unknown_harness_error_is_visible_not_clipped_by_a_wrapped_label`, and
`a_stale_reachability_result_does_not_shadow_a_later_profile_input`.

Worth noting for the render tests specifically: they assert over a fixed
viewport, so running them on a second platform is not redundant — it is the
only thing that shows the layout is not host-dependent.

#### Three defects found by running the binary, which is why the packet demands it

- **A stale test-result banner shadowed the profile wizard**, leaving it
  silently un-drivable. Fixed twice over so the two fixes do not depend on each
  other.
- **`cmux`, Ollama and llama.cpp were accepted as launch-profile harnesses**,
  because validation used `IntegrationId::ALL` rather than filtering to
  `IntegrationKind::Harness`.
- **A long refusal message rendered off-screen.** The input panel's height was a
  fixed `2`, and the harness hint wraps on a realistic width.
  `Paragraph::line_count` turned out to be gated behind an unstable upstream
  feature the packet forbade enabling, so the batch wrote a small tested
  word-wrap counter instead of guessing.

### Phase 2D — the Routing settings section and its five policy controls (six of seven)

Contract: Given a user who wants to constrain how Glasshouse will route work,
when they open Routing settings, they can set the router model choice, a
maximum acceptable router latency, a maximum marginal cost per decision, a
free-resource preference and a premium-capacity reserve; each value is
validated, shows which layer supplied it, and persists to the layer they chose
without disturbing its siblings.

State: COMPLETE for the six routing lines. **The seventh, "Add a Memory
settings section", stays open** — see below.

Production evidence:
- `config/mod.rs` — `RouterLatencyMs` (`10..=60000`, default `2000`),
  `RouterCostMicroUsd` (`0..=1000000`, default `1000`), `PremiumReservePercent`
  (`0..=100`, default `20`), `RoutingModelChoice`. Each resolves project ->
  user -> default independently, so setting one field never promotes its
  siblings into another layer. Zero cost is a deliberate free-only ceiling and
  zero reserve disables reserve protection; values past the maxima are refused
  as likely unit errors rather than silently clamped.
- `shell/state.rs`, `shell/view.rs` — the Routing tab; `m`, `l`, `c`, `f`, `p`
  edit the five controls, and every displayed value carries its layer, per the
  standing provenance invariant.
- `shell/mod.rs: save_user_settings_with_routing` — routing edits go through
  the existing writer, and the project layer still requires the separate `W`
  confirmation. No new path to the repository-local file was created.

Regression evidence:
- `routing_policy_values_round_trip_layer_independently_and_reject_absurd_inputs`
- `routing_settings_validate_and_stage_every_policy_control`
- `routing_and_memory_sections_render_their_complete_honest_states`
- `routing_edits_persist_to_the_chosen_layer_without_clobbering_siblings`
- Four mutation proofs from the worker. The one decisive for *these* boxes was
  re-run by the orchestrator on integrated `main`, because the whole argument
  for closing them is that the controls do something durable:

      (make the staged max_cost field never reach the saved routing table)
      test routing_edits_persist_to_the_chosen_layer_without_clobbering_siblings ... FAILED
      error: test failed, to rerun pass `-p glasshouse --test settings_persistence`
      (restored)
      test routing_edits_persist_to_the_chosen_layer_without_clobbering_siblings ... ok

Design-decision conflict, resolved and recorded:
- A standing decision said the settings view ships only sections whose feature
  exists, and that the other four sections' **map boxes stay unchecked until
  then**. Nothing routes yet, so that rule read literally would keep these six
  boxes shut indefinitely for reasons belonging to Phase 34.
- `docs/product/design-decisions.md` now records the refinement: the test is
  whether using a control does something real and durable, not whether its
  consumer exists. Routing passes it; Memory does not. The original rule had
  never been tested against a section with real controls and no consumer,
  because Providers and Launch Profiles both shipped alongside their features.

Missing evidence — and why the seventh box is still open:
- The Memory section renders "Project memory is not available in this build.
  There are no memory settings to save." That is truthful and worth keeping;
  it tells the user the capability is absent rather than implying it is
  present. But **a section with no settings in it is not a settings section**,
  and the box asks for one. It closes when memory has something to configure.
- `lead-memory` is building Phase 20 now but owns `src/memory/**` only, not
  `src/shell/**`. Whoever owns `shell/` next inherits this box, and the honest
  placeholder is the handoff.
- The five policy values are stored and read back; **nothing consumes them
  yet**, and the UI says so — free preference is stated to apply only after
  capability, health, rate-limit and latency checks pass. The router that
  honours them is Phases 34-38.

### Phase 2D — the settings view (nine of twenty lines)

Contract: Given a project session, when the user opens settings, they can see
and change which harnesses are enabled and where their executables are, with
each value's originating configuration layer shown, while leaving settings
returns them to exactly the session and mode they came from and nothing is
written into the repository without a distinct confirmation.

State: COMPLETE for the nine lines checked; the other eleven configure features
that do not exist.

Production evidence:
- `crates/glasshouse/src/shell/state.rs`: `Overlay::Settings`,
  `SettingsSection`, `HarnessRow`, `IntegrationRow`, `SettingsEdit`. Opened
  with `s` from control mode, closed with `Esc`, mode untouched by either.
  `state.rs` remains I/O-free; it returns `Action::SaveUserSettings` /
  `Action::SaveProjectSettings` and `shell/mod.rs` acts.
- `crates/glasshouse/src/shell/mod.rs`: `build_settings`,
  `apply_settings_edits`, `save_user_settings`, `save_project_settings` — the
  last going only through `config::write_project_config_with_consent`.
- `crates/glasshouse/src/shell/view.rs`: `render_settings`, showing each value's
  `config::Layer` as `(project)` / `(user)` / `(default)`.

Regression evidence:
- `shell::state::settings_tests` (12) — keymap and in-memory model, including
  that opening and closing settings leaves mode and session untouched.
- `shell::view::settings_tests` (8) — every displayed value carries a layer
  tag; the confirmation names the exact path; no decorative block elements.
- `tests/settings_persistence.rs` (4) — real filesystem, real `Runtime`.

Failure/isolation evidence — mutations, each observed to fail its target:
- `W` saving immediately with no confirmation.
- The confirmed save not calling the writer.
- A user-level save also writing into the project root.

**The cancel test was vacuous as first written, and this is the interesting
part.** It asserted that an untouched workspace contained no `.glasshouse`
directory without ever invoking the cancel path — the comment even said so.
Mutating `W` to save with no confirmation left it green. Rewritten to drive the
keys and act on the result, it still passed, because it kept only the answer to
the *cancel* key and discarded the answer to `W` — which is exactly where a
missing confirmation saves. Only when it acts on **every** action, as the run
loop does, does the mutation fail it. Two rounds of a test that looked right
and proved nothing.

Secrets: no configuration type has a field able to hold one
(`config::tests::serialized_form_has_no_secret_capable_field`), so the settings
view cannot render a secret value. Structural, not a display rule.

Platform/external evidence:
- CI `32825574631` on `473d2b0` — green on Linux, macOS, Windows and lint.

Missing evidence:
- Eleven lines unchecked: Providers, Launch Profiles, Routing (six lines) and
  Memory sections, and provider/launch-profile management. Each configures a
  feature that does not exist (Phases 9C, 9A, 9I-9K, 20). Building the sections
  now would ship dead controls — see `docs/product/design-decisions.md`.
- The settings view has no end-to-end test through the shipped binary. The same
  differential-repaint limit that blocked the multi-session test applies.
