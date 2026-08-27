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


---

### Phase 9A, second pass — the generated-configuration mechanism (lines 362, 363, 366)

> **A note on line numbers.** The section above was written against the map's
> earlier numbering and still uses it: its 359, 360 and 363 are this section's
> **362**, **363** and **366**; its 350 is **353**; its 365 is **368**; its 369
> is **372**. Nothing above is restated here.

Contract: when a harness's provider configuration exists only as a document,
the launch overlay carries that document — generated by the harness's own
adapter, written into the directory Glasshouse owns for one session, pointed
at through a mechanism the adapter declares, carrying no credential value, and
removed when the session ends.

State: **the mechanism is COMPLETE and proven; its production caller is two
lines in `main.rs`, which was frozen for the package that built it.** See
`.agent-runtime/patch-launch-overlay-main.diff`. Until that patch lands, lines
362, 363 and 366 have a mechanism with no production caller, which is exactly
what practice §5 says does not get a box.

**Why OpenCode, and how "requires file-based provider configuration" was
established.** `opencode --help` on OpenCode 1.18.22 lists every option the
binary takes, and none of them names a base URL, an API key or a provider
definition. `--model <provider>/<model>` *selects* among providers that
already exist; `opencode providers` authenticates one interactively. A
provider that is not already configured can only be brought into existence by
a configuration document. That is the first harness in this project's
catalogue whose direct-provider mechanism is document-shaped rather than
variable-shaped — Claude Code takes four environment variables, and Codex
composes everything out of `-c` overrides and writes nothing at all.

**The finding that nearly went the other way, and what it decided.** OpenCode
accepts the *same document* inlined into an environment variable
(`OPENCODE_CONFIG_CONTENT`), which would have needed no file. Both were probed
against a project holding its own `opencode.json`, with `opencode debug
config` reporting the resolved value:

- `OPENCODE_CONFIG=<file>` merges at **global** scope, *below* the project's
  own configuration — the project's value won;
- `OPENCODE_CONFIG_CONTENT=<json>` merges at **local** scope, *above* it —
  Glasshouse's value won.

Glasshouse adds a provider; it does not get to overrule what the user wrote in
their own repository while doing so. The file is the mechanism that adds
without overruling, so the file is the mechanism. Recorded because the honest
reading of line 362 was very nearly "OpenCode does not require a file", and
the reason it requires *this* one is a merge order rather than a preference.

Production evidence:
- `harness/mod.rs` — `GeneratedConfigSite` (the only thing that decides where
  a generated document may live), `GeneratedConfig`, `ConfigPathPlacement`,
  `unsafe_config_file_name`, and `HarnessAdapter::direct_provider_requires_model`.
  `DirectProviderPlan` gained one field; an adapter still never sees a
  credential and now also never names a path.
- `harness/opencode.rs` — `direct_provider_launch`, composing the provider
  entry as `serde_json`, plus `protocols` promoted from `Unverified` to
  `openai-chat` on a recorded request line, and a
  `BackendSelection::GeneratedConfiguration` declaration.
- `profile/mod.rs` — `PendingConfig` and `LaunchOverlay::{configs, install}`;
  `accept_generated_config`; `require_model_if_the_harness_selects_through_it`;
  `SUBSTITUTION_SEQUENCES`; and three new `Refusal` variants
  (`DirectProviderNeedsModel`, `UnsafeGeneratedConfigName`,
  `UnsafeGeneratedConfigValue`).
- `profile/generated.rs` — **the only thing under `profile/` that opens a
  file**, and it takes every path from its caller. `EphemeralConfigs` removes
  what it wrote on `Drop` and registers a `shutdown::on_forced_exit` cleanup
  for the path that runs no destructors.
- `main.rs::launch_session` — **not yet wired**; the two-line patch is in
  `.agent-runtime/patch-launch-overlay-main.diff`, and every observation below
  was made with it applied.

**Resolution still opens nothing.** `harness::resolving_a_launch_profile_touches_no_files`
is unchanged and still passing: `resolve` composes the document and decides
nothing about where it goes, because at resolution time no session record
exists and therefore no session directory does either. The adapter declares a
`ConfigPathPlacement` instead of a path, and `LaunchOverlay::install` fills it
in afterwards. A new companion scan,
`harness::the_only_writer_in_profile_takes_its_paths_from_its_caller`, forbids
`profile/generated.rs` the ambient environment, this crate's own path
resolver, and the two dot-directories a harness keeps configuration in — so
the one writer cannot arrive at somebody else's configuration even by
accident, and a second writer under `profile/` cannot appear without failing a
test.

**The credential never enters the document.** OpenCode substitutes
`{env:NAME}` anywhere in a configuration document before parsing it, so the
document names the provider's own credential variable and the value travels
the way every other harness's already does — in the child's environment,
placed by `resolve`. That the substitution is real was read off the wire, not
assumed. The same mechanism is a hazard in the other direction, so a
configured base URL or header value containing `{env:` or `{file:` is refused
(`Refusal::UnsafeGeneratedConfigValue`).

Regression evidence:
- `crates/glasshouse/tests/launch_overlay.rs` — six tests. Five drive the
  library through the exact sequence the `main.rs` patch performs; the sixth,
  `the_binary_refuses_an_opencode_profile_that_names_no_model_and_records_no_session`,
  spawns the shipped binary and is the §35 entry point.
- `profile/mod.rs` — `a_generated_configuration_names_the_credential_variable_and_never_its_value`,
  `the_generated_configuration_diagnostic_shows_names_only`,
  `installing_a_name_that_could_leave_the_site_refuses_and_writes_nothing`,
  and four new scenarios in `no_environment_value_is_ever_rendered`, whose
  exhaustive `match` now covers twenty `Refusal` variants.

Mutation evidence — **eleven run, ten killed, one survivor diagnosed and
replaced.** Each named test `ok` before, `FAILED` mutated, `ok` restored, with
the mutated file touched so no verdict came from a cached binary (§16):

| id | mutation | test | result |
|---|---|---|---|
| M1 | `GeneratedConfigSite::file` joins onto the site's *parent* | `a_generated_configuration_lives_in_the_directory_glasshouse_owns_and_dies_with_the_session` | FAILED |
| M2 | the guard owns no paths, so the document survives the session | same | FAILED |
| M3a | OpenCode's own protocol check removed | `an_unsupported_harness_and_protocol_combination_is_refused_rather_than_written` | **SURVIVED** |
| M3b | OpenCode declares a protocol nobody read off a request line | same | FAILED |
| M4 | the credential value replaces its variable name in the document | `a_generated_configuration_names_the_credential_variable_and_never_its_value` | FAILED |
| M5 | the diagnostic renders the document instead of naming it | `the_generated_configuration_diagnostic_shows_names_only` | FAILED |
| M6 | a configured value may smuggle a substitution into the document | `a_configured_value_cannot_smuggle_a_substitution_into_a_generated_document` | FAILED |
| M7 | `install` joins a file name without checking it | `installing_a_name_that_could_leave_the_site_refuses_and_writes_nothing` | FAILED |
| M8 | the document is written `0o644` instead of `0o600` | `a_generated_configuration_lives_…_and_dies_with_the_session` | FAILED |
| M9 | the child is never pointed at the document that was written | same | FAILED |
| M10 | the launch path never asks whether the harness needs a model — **the call, not the callee** | `the_binary_refuses_an_opencode_profile_that_names_no_model_and_records_no_session` | FAILED |

M3a is §41's shape and is recorded rather than quietly replaced. Deleting
OpenCode's own `if request.protocol != OpenAiChat` changed nothing, because
`profile::choose_protocol` refuses the combination *before* the adapter is
asked: the test and the mutation both assumed the adapter was what refused.
The adapter's check is a second guard against a core that stops filtering, and
it is unreachable through today's public paths. M3b attacks what can actually
vary — the adapter's *declaration* — and dies immediately, and under it the
adapter's internal check is what keeps the launch from being composed.

M5 is the same lesson one turn later: the first version of
`the_generated_configuration_diagnostic_shows_names_only` asserted only what
the note *contains*, so a mutation that appended the whole document passed it.
The test now asserts what the note may not contain, and kills it.

Platform/external evidence — **OpenCode 1.18.22, the real one**:
- Its wire protocol was read off a **recorded request line**, not inferred: a
  listener on `127.0.0.1:8731` received
  `POST /v1/chat/completions HTTP/1.1` with
  `User-Agent: opencode/1.18.22 ai-sdk/provider-utils/4.0.23 runtime/bun/1.3.14`.
  The `/v1` came from the provider's own declared base URL and
  `/chat/completions` was appended by the harness, so the base URL passes
  through verbatim — the same rule Codex's adapter records.
- The credential arrived as `Authorization: Bearer <value>` from a document
  containing only `{env:NAME}`, and a configured header
  `X-Glasshouse-Probe: header-value-here` arrived as `x-glasshouse-probe`.
- **End to end, against the shipped binary in a real pty** (`script -q`), with
  the `main.rs` patch applied: `glasshouse launch opencode --profile probe`
  against a `[profiles.probe]` naming an `openai-compatible` provider. While
  the session ran:
  - the document was at
    `…/projects/proj-d000…/sessions/c62cbde90192…/opencode-provider.json`,
    mode **`-rw-------`**;
  - it contained `"apiKey": "{env:GLASSHOUSE_PROBE_KEY}"` and **not** the
    credential — a `grep -rl` for the planted value across the whole state
    directory found nothing;
  - the child's environment carried
    `OPENCODE_CONFIG=<exactly that path>` and the credential variable, read
    from the running process;
  - the child's argv was `opencode --model probe-router/probe-model-x`.

  After the session ended the document was **gone** and the session directory
  was empty, while `glasshouse sessions` still listed the session with
  `PROFILE = probe`. Repeated with three interrupts delivered back to back, so
  the second reached `shutdown::force_exit`: the document was gone there too.

Local gate: `scripts/ci-local.sh`, run alone, 12 of 13 jobs PASS. The one FAIL
is `lint / script tests` → `test_worker_signal`, which is **an artefact of
running the gate from a worker worktree** rather than anything in this diff:
the Stop hook writes its done marker into the main checkout while the test
reads `<the checkout the script lives in>/.agent-runtime/done`. The same test
run in `/Users/eneas/projects/glasshouse` passes. Nothing in this package
touches `scripts/`.

Missing evidence, stated plainly:
- **No production caller.** `LaunchOverlay::install` is called by no shipped
  code path until the two-line `main.rs` patch lands.
- **The forced-exit cleanup has no mutation test of its own.** The
  registration is made by construction and `shutdown.rs`'s own
  `forced_exit_cleanup_runs_while_registered_and_not_after` proves the
  registry runs what is registered, but no test in this package fails if the
  registered closure is replaced by a no-op — running the registry needs a
  `shutdown.rs` seam this package did not own.
- **`SIGKILL` leaves the document behind**, and nothing can change that.
- **The `--model <provider>/<model>` pair is passed as two argv entries**,
  which is the form that was run against the installed binary. A model
  identifier beginning with `-` would be ambiguous to the harness's own
  argument parser; nothing in `config` restricts one today.
