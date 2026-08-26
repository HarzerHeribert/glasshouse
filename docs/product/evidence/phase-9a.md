# Capability evidence — phase 9a

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 9A — harness launch profiles (seventeen of twenty-six)

Contract: Given a harness and a selected launch profile, when the user starts a
session, Glasshouse resolves that profile through the harness's own adapter
into an overlay that applies to the child process only — refusing any
combination the adapter does not declare rather than inventing one — and
records which profile the session ran under, while never modifying the user's
global harness installation or configuration.

State: **COMPLETE for seventeen lines; nine deliberately left open** (listed at
the end, each with the phase that unblocks it).

Production evidence:
- `profile/mod.rs` — `LaunchProfile`, `BackendResource`, `ProfileClass`,
  `ApprovalSelection`, `LaunchOverlay`, `MechanismNote`, `Refusal`, `resolve`.
- `main.rs: launch_session` — the production caller. Resolution happens
  **before** `ProjectSessions::open`, so a refusal starts no process and writes
  no row. Argument order is adapter args, then the overlay, then the user's own
  `--` arguments last.
- `config/mod.rs` — `ProfileTable`/`ProfileConfig` in both layers,
  `EffectiveConfig::profile_names`/`launch_profile`, and
  `bypass_acknowledged`, which reads the **user layer only**.
- `database.rs` — migration 3 adds `sessions.launch_profile` and
  `sessions.backend_resource`, both nullable. Migrations 1-2 untouched.
- `cli.rs` — `glasshouse launch --profile <name>`.

The design and its reasoning are in `docs/product/design-decisions.md`.

Regression evidence (the twelve named acceptance tests, plus):
- `a_native_profile_exists_for_every_harness_and_adds_nothing`
- `an_explicit_automatic_review_request_is_refused_on_a_harness_without_one`
- `a_defaulted_profile_on_such_a_harness_adds_no_approval_argument`
- `a_bypass_is_refused_until_it_is_acknowledged_for_that_harness`
- `a_provider_backed_profile_is_refused_with_the_phase_that_supplies_it`
- `an_overlay_reaches_only_the_child_process`
- `the_user_s_own_arguments_stay_last`
- `a_refused_profile_starts_no_process_and_records_no_session` and
  `an_unacknowledged_bypass_also_starts_no_process_and_records_no_session` —
  both call the real `launch_session` and assert the store is empty afterwards.
- `upgrading_a_version_2_database_preserves_every_existing_session`
- `no_launch_profile_definition_is_stored_in_the_project_database`
- `no_environment_value_is_ever_rendered`
- `a_project_layer_cannot_acknowledge_a_bypass`
- `resolving_a_launch_profile_touches_no_files` — added by the orchestrator for
  line 362, which had no guard. A source scan of `profile/mod.rs`'s production
  code for `std::fs`/`fs::`/`File::`/`OpenOptions`/`std::env`: a module that
  never opens a file cannot modify the user's global harness configuration,
  which is stronger and cheaper to keep true than enumerating forbidden paths.
- `a_configured_gateway_profile_never_displaces_the_native_one` — added by the
  orchestrator for line 371. Fails if anyone ever "unifies" the lookup by
  moving Native into the table.

Non-vacuity: **nine mutations, nine kills.** Silently using the bypass for an
explicit automatic-review request; a defaulted profile falling back to the
bypass; the acknowledgement gate removed; the provider-backend refusal removed
(killing both its own test and the no-session-recorded one); the project layer
allowed to acknowledge a bypass; migration 3 using a `'native'` sentinel
instead of NULL; a filesystem write added to `profile/mod.rs`; and Native
dropped from `profile_names`.

Platform/external evidence — **the real harness, not a fake one**:
- `glasshouse launch codex` was run from the built binary in a real terminal
  against Codex 0.149.1. It started, and Codex displayed **its own workspace
  trust prompt** in the viewport — a native prompt staying interactive, which
  is the product invariant, and proof the injected `--approve-for-me` did not
  break startup. The prompt was declined; nothing was trusted.
- `glasshouse sessions` then showed the session with `PROFILE = native`.
- `glasshouse launch codex --profile bogus` printed
  "`bogus` is not a known launch profile; valid names are: native" and the
  session count stayed at one — refusing really does cost nothing.
- The mechanism diagnostic, read from a real log:
  `profile=native backend=native mechanisms=approval mode: automatic review:
  Route approval requests through automatic review using the workspace-write
  sandbox (--approve-for-me)`.
- Flag-conflict probes: `codex --approve-for-me --sandbox read-only
  --ask-for-approval on-request` and `claude --permission-mode auto
  --permission-mode plan` both parse, so Glasshouse's injected default does not
  break a user's own pass-through arguments, which come last.

**A behaviour change this closes, stated plainly:** every Codex session
Glasshouse starts now carries `--approve-for-me`, and every Claude Code session
`--permission-mode auto`, because the default profile selects the harness's own
automatic-review mode. That is the decision recorded under "Approvals"; it is a
real change in what the user's harness does, and the end-to-end PTY test now
asserts the exact argv rather than the previous "no arguments at all".

CI evidence:
- **CI `32881520282` green on Linux, macOS, Windows and lint** for `6e730f6`,
  with the new tests confirmed to have *executed* on the Windows runner by
  name — including
  `upgrading_a_version_2_database_preserves_every_existing_session`, which is
  the one that would matter most if the migration behaved differently there,
  plus `resolving_a_launch_profile_touches_no_files`,
  `a_configured_gateway_profile_never_displaces_the_native_one`,
  `a_bypass_is_refused_until_it_is_acknowledged_for_that_harness`,
  `a_provider_backed_profile_is_refused_with_the_phase_that_supplies_it` and
  `an_explicit_automatic_review_request_is_refused_on_a_harness_without_one`.

Missing evidence — the nine open lines and what each waits for:
- **350** (a profile is harness + backend + model + protocol + overlay +
  *response profile*): `LaunchProfile` has five of the six. Response profiles
  are **Phase 9K**.
- **353** (additional profiles such as Claude / OpenRouter) and **373**'s
  richer cases: need provider configuration, **Phases 9C/9D**.
- **355** (inject environment variables): the mechanism is proven, but **no
  shipped profile can populate `env`** — only `Native` resolves, and it
  contributes none. The test constructs an overlay directly and says so. Same
  rule as `SessionRuntime` and Phase 1 line 90: a mechanism with no production
  caller does not get its box. Unblocks with **Phase 9F**.
- **359** (isolated generated configuration file) and therefore **360** (the
  overlay representing *all* the mechanisms together) and **363** (prefer
  Glasshouse-owned generated configuration): nothing generates configuration
  yet. **Phase 9F**.
- **365** (record backend, model, protocol, pairing class, response profile):
  pairing class is **9J**, response profile **9K**. `model` and
  `wire_protocol` columns were deliberately *not* added — no profile can carry
  either until 9F, and a column nothing writes is the speculative
  infrastructure Phase 21H forbids.
- **369** (the router selects among enabled profiles): **Phases 34-37**.

Known gaps that are not blockers:
- There is no wizard for authoring a profile; they are hand-edited in
  `config.toml` today. Phase 2D's "Launch Profiles" section is now unblocked.
- `resume_session` does not apply an overlay, so a resumed session does not
  carry its profile's arguments. Which profile a resume should use — the one
  recorded, or whatever is configured now — is a real design question and is
  recorded as a loose end rather than guessed at.
