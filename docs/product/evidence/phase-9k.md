# Capability evidence — phase 9k

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 9K — Profile model, 11 of 11 (lines 587–597)

Contract: Given a project, a role and a session, Glasshouse resolves a
*communication policy* — five independent axes, resolved one axis at a time
down the six-layer chain line 596 names — and can say for each axis which layer
supplied it. No axis, no preset and no configuration can reduce what a session
reports, and a project's answer never reaches another project.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/profile/response.rs` — the domain model. Five enums,
  one per axis; `ResponseProfile` with private fields and one accessor per
  axis; `Preset`, `Role`, `PrecedenceLayer`, `PrecedenceStack`, `resolve`.
  Pure: it imports no configuration and no adapter, for the reason
  `crate::profile`'s own module header gives.
- `crates/glasshouse/src/config/response.rs: EffectiveConfig::response_stack`
  — the one place the six layers are filled in, and the **only** builder of a
  `PrecedenceStack` in the crate. `glasshouse response` and `glasshouse
  launch` both go through it, so what a user is shown and what a session gets
  cannot disagree.
- `crates/glasshouse/src/config/response.rs: report` — **the production
  caller**, and the single line `main.rs`'s `Command::Response` arm runs.
- `crates/glasshouse/src/cli.rs: Command::Response` and
  `crates/glasshouse/src/main.rs` — `glasshouse response [--role ROLE]
  [--session NAME] [--verbosity|--audience|--narration|--evidence|--format
  VALUE]`, the surface a person runs.
- `crates/glasshouse/src/main.rs: launch_session` — the **second** production
  caller, on the real launch path: it resolves the same profile and hands the
  result to `install_session_document`. A resolved profile is never something
  only a report has seen.
- `crates/glasshouse/src/config/response.rs: ResponseConfig` — the
  `[response]` and `[response.roles.<role>]` tables, in both the user and the
  project layer.

Line by line:
- **587** — `ResponseProfile` has five fields and none of them is effort,
  permission mode, diligence or tool access. Those live elsewhere
  (`ApprovalSelection` is the permission one) and there is no path from a
  response profile to any of them.
  `directives_never_mention_effort_permissions_or_tools` renders all 324
  combinations and fails if the word `permission`, `tool`, `effort` or `skip`
  ever appears in the text a harness is handed.
- **588–592** — `Verbosity`, `Audience`, `Narration`, `EvidenceDetail`,
  `AnswerFormat`: five types with no conversion between them, five private
  fields, five accessors, and five directive functions each of which takes
  only its own axis's type — so `narration_directive` *cannot* consult
  verbosity, because it was never given one. Proven by M1 and M1b.
- **593** — `Preset` holds five axis values, a name and a description, and has
  no field that could hold a harness's own vocabulary. Translating a preset
  into what one harness calls it is `harness::response`'s job.
- **594** — `concise-technical` is `Concise` verbosity and `Silent` narration,
  which is "leads with outcomes, suppresses routine narration". The third
  clause is not a property of that row: `REQUIRED_REPORTS` is a constant,
  `required_reports()` returns it without reading `self`, and `directives()`
  appends `floor_directive()` unconditionally.
  `every_profile_reports_changed_files_verification_risks_and_blockers`
  enumerates all 324 combinations rather than sampling.
- **595** — `Role::default_preset` gives five separate answers; a
  `[response.roles.<role>]` table overrides any of them, project over user.
  `each_role_resolves_to_its_own_default_through_the_binary` reads five
  reports out of the binary and requires at least four distinct profiles.
- **596** — six layers, in that order, resolved per axis.
  `every_one_of_the_six_precedence_layers_can_win_in_the_binary` silences them
  from the top one at a time and checks the next one down takes over, all
  through `config::response::report`. Six of the mutations below delete one
  layer each.
- **597** — a project's `[response]` table is read from that project's own
  `.glasshouse/config.toml` by `load_project_config`, and `EffectiveConfig`
  holds exactly one of them.
  `a_projects_response_profile_does_not_reach_another_project` builds two
  project roots against one user configuration file and resolves in both.

### Phase 9K — Harness-native application, 9 of 9 (lines 601–609)

Contract: Given a resolved response profile, Glasshouse prefers the harness's
own communication-style mechanism where one has been read from that harness and
can express the profile without weakening its coding instructions; otherwise it
adds an instruction beside the harness's own system prompt; otherwise it
applies nothing and says why. It never replaces a native system prompt and
never rewrites a prompt at the gateway.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/harness/response.rs: apply` — line 601 as a function:
  nothing-was-asked-for, then native, then additive, then a refusal that names
  the gap. `AppliedMechanism` has exactly three variants and **no variant
  meaning "replaced"**, so nothing downstream can record a replacement and
  nothing upstream can perform one.
- `crates/glasshouse/src/harness/mod.rs: HarnessAdapter::native_response_style`
  and `HarnessAdapter::additive_response_injection` — line 602's seam. Both
  default to `None`, which is the fail-closed answer `approval_args` and
  `direct_provider_launch` already give.
- `crates/glasshouse/src/harness/claude_code.rs: BUILT_IN_OUTPUT_STYLES` and
  `closest_output_style` — the only place in the crate that knows the words
  "output style", which is line 603.
- `crates/glasshouse/src/session/select.rs: install_session_document` — what
  `main.rs`'s launch path calls. It writes **one** document per file name and
  emits **one** flag pointing at it.
- `crates/glasshouse/src/config/mod.rs: ProviderConfig::prompt_transform` —
  line 609's backend metadata, surfaced by `glasshouse response` with the
  warning attached. Nothing in Glasshouse ever writes it.

Line by line:
- **601** — `apply` asks for a native mechanism first. The *"without weakening
  coding instructions"* half is answered inside the adapter, which is the only
  thing that knows its own styles: `closest_output_style` filters on
  `keeps_coding_instructions && communication_only`, and two of Claude Code's
  four built-in styles fail the second test.
- **602** — a trait method on `HarnessAdapter`, returning the harness's own
  vocabulary with the evidence it was read from.
- **603** — `OutputStyle` is a private type in `claude_code.rs`, unreachable
  from anywhere else in the crate. There is no Glasshouse-level enum of style
  names and no "personality" concept.
- **604** — `AppliedMechanism::{Native, Additive, NotApplied}`, printed by
  `glasshouse response` per harness and logged at launch.
  `the_applied_record_names_the_mechanism_that_actually_reached_the_child`
  checks the record against what was delivered rather than against itself.
- **605** — every Glasshouse-launched session resolves its own role and its
  own profile; the resolution has no "inherit" state, and `--response-role
  worker` is on `launch` and `run`. `a_worker_gets_its_profile_explicitly_
  rather_than_inheriting_one` requires the worker's answer to differ from the
  interactive one, so "explicit" is not a synonym for "whatever the parent
  had". **See "Missing evidence" — Glasshouse spawns no subagents of its own,
  so this is the reachable half of the line.**
- **606** — the settings key is merged into the document rather than replacing
  it, `--settings` loads *additional* settings, and all four of Claude Code's
  built-in styles declare `keepCodingInstructions: true`. The lifecycle hooks
  written into the same document survive the merge, and
  `the_launch_carries_exactly_one_settings_flag_and_keeps_the_lifecycle_hooks`
  fails on a build where they do not.
- **607** — `claude --help` documents `--system-prompt` (replaces) beside
  `--append-system-prompt` (appends). Only the second is declared by the
  adapter, and `the_launch_appends_to_the_system_prompt_and_never_replaces_it`
  fails on any composed argv containing the first.
- **608** — `harness/response.rs` imports nothing from `crate::gateway` and
  has no path into it; the report says so in as many words with no
  transformation configured.
- **609** — `prompt_transform` on a provider, layered project-over-user, is
  printed with the sentence *"This is something your backend does, not
  something Glasshouse does. It may interact with the harness's own
  instructions and with the profile above."*

Regression evidence (macOS, `cargo test -p glasshouse`; local gate below):
- `crates/glasshouse/tests/response_profiles.rs` — nineteen tests, **every one
  entering through `config::response::report` or
  `HarnessSelection::install_session_document`** against real configuration
  files on a real filesystem, so a test cannot pass against a build whose
  configuration layering or document composition has been deleted (§35).
- `crates/glasshouse/src/profile/response.rs` unit tests — fourteen, including
  two that enumerate all 324 axis combinations rather than sampling.
- `crates/glasshouse/src/harness/response.rs` unit tests — seven, over every
  adapter and every preset.
- `crates/glasshouse/src/harness/claude_code.rs` unit tests — five, on the
  declared style table and the two styles Glasshouse may never select.

Mutation evidence — nineteen run, nineteen killed, each named test `ok` before,
`FAILED` mutated, `ok` restored:

| id | mutation | test | result |
|---|---|---|---|
| M1 | terse verbosity silently sets silent narration | `the_five_dimensions_are_independent` | FAILED |
| M1b | concise verbosity silently sets minimal evidence | `the_five_axes_are_reported_separately_and_one_never_moves_another` | FAILED |
| M2 | a terse or concise profile drops the floor sentence | `every_profile_reports_changed_files_verification_risks_and_blockers` | FAILED |
| M3 | `required_reports` shrinks when evidence is minimal | `no_axis_can_reduce_the_required_reports` | FAILED |
| M4 | the task-override layer is never set | `every_one_of_the_six_precedence_layers_can_win_in_the_binary` | FAILED |
| M5 | the session layer is never set | same | FAILED |
| M6 | the role layer is never set | same | FAILED |
| M7 | the project layer is never set | same | FAILED |
| M8 | the user-default layer is never set | same | FAILED |
| M9 | an axis that reaches the bottom is attributed to the user default | same | FAILED |
| M10 | the record says `native` and delivers no settings key | `the_applied_record_names_the_mechanism_that_actually_reached_the_child` | FAILED |
| M11 | the native mechanism is never asked for | `the_report_names_the_mechanism_each_harness_would_apply` | FAILED |
| M12 | the launch drops the profile's own arguments | `no_configured_profile_can_stop_the_binary_reporting_the_four_that_matter` | FAILED |
| M13 | the style keys are not merged into the hook document | `the_launch_carries_exactly_one_settings_flag_and_keeps_the_lifecycle_hooks` | FAILED |
| M14 | the project layer reads the user's table | `a_projects_response_profile_does_not_reach_another_project` | FAILED |
| M15 | a concise profile maps to the `Learning` style | `the_report_names_the_mechanism_each_harness_would_apply` | FAILED |
| M16 | M15 plus the `communication_only` filter removed | `no_profile_ever_selects_a_style_that_changes_what_the_agent_does` | FAILED |
| M17 | every role returns the same default preset | `each_role_resolves_to_its_own_default_through_the_binary` | FAILED |
| M18 | `report` ignores the request it was given | `every_one_of_the_six_precedence_layers_can_win_in_the_binary` | FAILED |

M12, M13 and M18 are the §35 group: they mutate the *call*, on the production
path, rather than the callee. M16 is the only two-edit mutation, because the
property it attacks needs both a mapping that reaches a forbidden style and the
guard that stops it — either alone is invisible, which is §41's warning about a
mutation weak in the same way as its test.

**One mutation survived and was a finding, not a gap.** M1b, in its first form,
survived because `ResolvedProfile` stored the five resolved *values* a second
time beside the sources, and the report printed the stored copy. A build in
which `ResponseProfile::new` forced one axis from another therefore printed the
honest value and shipped the mutated one. `ResolvedProfile` now stores the
profile once and derives the axis list from it; there is no second copy to
disagree.

Failure/isolation evidence:
- `a_value_this_build_does_not_know_is_reported_rather_than_guessed_at` — a
  `verbosity = "chatty"` is named in the report and the axis falls through to
  the layer below, never to a silent default. A typo that quietly became
  `standard` would be a communication policy the user never chose.
- `a_role_table_this_build_does_not_know_is_named_rather_than_ignored` and
  `a_session_preset_the_build_does_not_know_is_refused_by_name`.
- `an_unconfigured_project_composes_a_launch_that_says_nothing_about_
  communication` — with nothing configured, the launch carries no
  `--append-system-prompt` and the settings document carries no `outputStyle`.
  An unconfigured Glasshouse leaves the harness exactly as it found it.
- `an_adapter_that_declares_nothing_says_so_rather_than_inventing_a_mechanism`
  — six of seven harnesses refuse, by name.

Platform/external evidence (Claude Code 2.1.247, installed on the development
machine, read on 2026-08-27; no model was called for any of it):
- `claude --help` documents `--append-system-prompt <prompt>` — "Append a
  system prompt to the default system prompt" — beside `--system-prompt
  <prompt>` — "System prompt to use for the session". The declared mechanism is
  the first.
- The settings schema inside the installed bundle declares
  `outputStyle: …optional().describe("Controls the output style for assistant
  responses")`.
- The built-in style table inside the same bundle declares four styles, each
  with `source: "built-in"` and `keepCodingInstructions: true`: `Concise`
  ("responds tersely, leading with results and skipping preamble and
  narration"), `Explanatory` ("explains its implementation choices and codebase
  patterns"), `Learning` ("pauses and asks you to write small pieces of code
  for hands-on practice") and `Proactive` ("executes immediately, minimizes
  interruptions, and prefers action over planning").
- **The key is reachable through `--settings`, verified against the host it is
  cited for.** `claude --settings '{"outputStyle": 42}' doctor` answers
  `Invalid settings — … › outputStyle: Expected string, but received number`.
  A control: the same command with a valid string produces no complaint.
- **`--settings` is not repeatable; the last one wins.** `claude --settings
  '{"outputStyle": 42}' --settings '{"theme": 99}' doctor` reports nothing,
  while the same two flags in the opposite order report the `outputStyle`
  error. This is why the response profile's settings key is merged into the
  hook document rather than given a `--settings` of its own: a second flag
  would have silently switched off every lifecycle hook in the session.
- End-to-end against the shipped binary, with `claude-code`'s executable
  configured to a script that recorded its argv: `glasshouse run --headless
  --response-profile concise-technical` handed the child exactly
  `--settings <one path> --append-system-prompt <the floor sentence>
  --session-id <uuid> --permission-mode auto`, and the document at that path
  contained the four lifecycle hooks **and** `"outputStyle": "Concise"`. No
  `--system-prompt` appeared.

Missing evidence:
- **Six of seven harnesses declare no communication-style mechanism and no
  additive mechanism, so `apply` refuses for them.** That is the honest state
  of the declarations, not a gap in this module: `agy --help`, `codex --help`,
  `cursor-agent --help`, `opencode --help` and `hermes --help` were each read
  in full for Phase 9J and none documented one, and Pi is not installed. Any of
  the six becomes usable by adding one `AdditiveInjection` to its adapter.
- **Line 605's subagent half is unreachable.** Glasshouse spawns no subagents:
  `Capabilities::subagents` is a declaration *about* a harness, and nothing in
  the binary starts one. What is closed is that every session Glasshouse itself
  opens resolves its profile explicitly, including a worker one. If Glasshouse
  ever spawns a subagent, that spawn must resolve a profile the same way.
- The shell's quick-open (`n`) applies no response profile. It resolves no
  launch profile either, so a session opened that way gets the harness
  untouched in every respect; closing it is a change to `crate::shell`, which
  this package did not own.

---

### Phase 9K — Additive fallback and cache behavior, assessed (lines 613–623)

State: NOT ATTEMPTED AS A GROUP — assessed, with four lines argued closed by
work group 2 required and the orchestrator to decide.

Group 2's line 604 requires recording that an *additive instruction* was
applied, which is only meaningful if the additive path can run. It can:
`--append-system-prompt` is declared on the Claude Code adapter and is reached
by every profile no built-in output style expresses. Building that path
necessarily built part of this group.

- **613** ("a small stable additive response contract") — arguably closed.
  `ResponseProfile::directives` is that contract: six sentences, one per axis
  plus the floor, composed in one function so a terse profile cannot grow a
  shorter variant. Against: the map may mean a contract shared across harnesses
  and versioned, which this is not called.
- **614** ("state that concision must not reduce analysis, verification,
  diagnostics, error reporting, or checkpoint completeness") — arguably closed,
  and this is the strongest of the four: `floor_directive()` says exactly that,
  in those words, in every instruction Glasshouse writes, and
  `no_instruction_glasshouse_writes_omits_the_floor` fails otherwise.
- **615** ("inject through the safest adapter-supported session-start,
  append-system, … mechanism") — arguably closed for the one harness that
  declares a mechanism; open in the sense that six declare none.
- **616** ("avoid repeatedly injecting an unchanged contract on every turn") —
  **true by construction and therefore weak.** The instruction is composed once
  at session start and there is no per-turn injection path to avoid. §5's own
  standard suggests a line satisfied by the absence of a mechanism should not
  be ticked.
- **617** ("prefer selecting a profile when a session is created so the
  system-prefix and prompt-cache behaviour stay stable") — the same shape as
  616, with one difference: this one names a *preference* that was actually
  exercised. `launch_session` resolves and applies at creation, and the
  alternative (re-deriving per turn) was available and not taken.
- **618** ("let adapters declare whether a live profile change is supported,
  delayed until a new session, or likely to invalidate prompt caching") —
  **half exists and predates this package.**
  `HarnessDescription::communication_style` already carries
  `StyleChange::{InPlace, NewSession}`, and Claude Code declares `NewSession`
  with evidence. What is missing is the third term: nothing declares
  prompt-cache invalidation, and nothing reads `StyleChange` today.
- **619** ("warn before a profile change that requires clearing or recreating a
  valuable warm session") — **blocked.** There is no in-session profile-change
  surface to warn from: a profile is selected at launch, and a running session
  cannot be asked to change it.
- **620** ("a lightweight in-session communication instruction for a one-turn
  override") — **blocked on the same thing.** The `TaskOverride` precedence
  layer exists and is consulted, and `glasshouse response --verbosity …`
  enters through it, but there is no way to deliver an override into a
  *running* session. `HarnessAdapter::message` types text at a session and is
  what such a feature would use.
- **621** ("preserve raw native terminal output even when Glasshouse offers
  optional folding") — **not this package, and not blocked by it.** Folding is
  a TUI capability; `crate::tui` and `crate::shell` own it.
- **622** ("do not run a second language model to rewrite every final answer")
  — **true by construction.** Nothing here calls a model; there is no rewriting
  path to disable.
- **623** ("keep arbitrary custom prompt additions separate from named response
  profiles and require explicit user configuration") — **open, and cheap.**
  `ResponseConfig` has no free-text field and `Preset` cannot hold one, so the
  separation exists; what does not exist is the arbitrary-addition surface the
  line says to keep separate *from*.

---

### Phase 9K — Evaluation and safeguards, 0 of 6 (lines 627–632)

State: NOT STARTED — assessed, not attempted.

**Four of the six have no production caller and none can exist in this
repository today.**

- **627, 628, 629, 630** — all four begin "Measure". Nothing in Glasshouse
  measures output tokens, time to actionable information, user steering,
  overrides or cognitive load, and nothing records a per-pairing observation.
  `grep -rn 'fn score\|Score' crates/glasshouse/src` finds no match, as Phase
  9J's own assessment recorded on 2026-08-27; the same is true of any
  measurement seam. These wait on **Phase 47 (observability)** for the
  measurement channel and on **Phase 51 (evaluation hooks)** for anything to
  hang an evaluation on. 630 additionally waits on **Phase 33A**, because
  "separately for each harness-model pairing" is that ledger's key.
  A profile-effect metric with no metric pipeline is a struct nothing reads —
  §5, and the packet's own named failure mode.
- **631** ("allow the user to disable Glasshouse response-profile injection and
  use the untouched harness default") — **not blocked, and not built.** The
  untouched harness default is already what an unconfigured Glasshouse
  produces, and `PrecedenceLayer::HarnessDefault` already means exactly "the
  harness untouched". What is missing is a way to *select* it above a layer
  that has said something — a user with `[response] preset = "brief"` cannot
  turn it off for one project. That is a `disabled = true` key on
  `ResponseConfig` plus one arm in `response_stack`, perhaps thirty lines. It
  was left because it is group 4's, not because it is hard.
- **632** ("keep the active response profile and application mechanism
  inspectable from session details") — **CLOSED, by Phase 10's migration 8.**
  See the update at the end of this entry. The account below is why it was
  blocked when this was written, and it named exactly what unblocked it.

  Original state: **blocked on a schema migration.**
  `glasshouse response` shows the profile a session *would* get and the launch
  log records what one *did* get, but `sessions` has no column for either, so
  `glasshouse sessions` cannot show it after the fact. Adding
  `sessions.response_profile` and `sessions.response_mechanism` is an
  append-only migration 8 in `crates/glasshouse/src/database.rs`, in the shape
  migration 3 already used for `launch_profile` and `backend_resource`. That
  file was outside this package's ownership.

Missing evidence:
- Phase 47: a metrics channel any of 627–630 could write to.
- Phase 51: evaluation hooks.
- Phase 33A: per-pairing observation storage, for 630.
- `crates/glasshouse/src/database.rs` migration 8, for 632.


## Update — 632 closed by Phase 10

This entry said 632 was blocked on *"an append-only migration 8 in
`crates/glasshouse/src/database.rs`, in the shape migration 3 already used for
`launch_profile` and `backend_resource`"*, and that the file was outside that
package's ownership. Phase 10 wrote exactly that migration, with seven columns
rather than two, and `sessions.response_profile` and
`sessions.response_mechanism` are two of them.

`glasshouse sessions show` now prints both, read back from the session record
rather than re-resolved. Verified by the orchestrator against the shipped
binary: `a_launched_session_records_seven_facts_and_the_binary_shows_them_apart`
launches headless through `CARGO_BIN_EXE_glasshouse`, runs `sessions show`, and
requires the `response profile` line to name **all five axes** and the
`response mechanism` line to name the mechanism. Phase 10's M16 — the launch
path records no response profile — kills it.

**Nothing about the response-profile package changed.** The capability it built
was already complete; what was missing was somewhere to keep the answer, and
that belonged to the session model. This is a box closed by the phase after it,
which is why it was worth writing down as blocked rather than as done.

## Update — 613, 614, 615 decided; 627–632 re-audited; 631 found already closed

Package `phase-9k` (this round) was asked to decide the three lines the
previous pass left open, and to re-check the rest before building anything.
No production code in `profile/**` or `harness/**` changed — the audit found
nothing absent that this package's file ownership could close, and §5's rule
against manufacturing a caller applied to every remaining gap.

**613 — decided CLOSED.** `ResponseProfile::directives`/`instruction`
(`crates/glasshouse/src/profile/response.rs`) *is* "a small stable additive
response contract": six sentences, composed in one function, reached by
`harness::response::apply`'s additive branch and covered by
`every_profile_reports_changed_files_verification_risks_and_blockers` (324
combinations) and `harness::response::tests::no_instruction_glasshouse_writes_omits_the_floor`.
The counter-reading in the previous entry — a contract shared across harnesses
and versioned — is not what line 613 says; it says "small" and "stable", and a
one-function composition with no per-harness variant is exactly that.

**614 — decided CLOSED**, confirming the previous entry's own strongest
argument: `floor_directive()` states the prohibition in the line's own words
and no axis or preset can omit it (`no_axis_can_reduce_the_required_reports`,
`no_instruction_glasshouse_writes_omits_the_floor`).

**615 — decided CLOSED, for what the binary can reach.** `apply` (line
601) already *is* the safest-first selection line 615 asks for: native, then
additive (Claude Code's is `--append-system-prompt`, an append-system
mechanism), then a named refusal. Six of seven harnesses have no mechanism to
select among — that is an honest absence in those adapters' declarations, not
a defect in the selection logic this line describes, and it is the same
"six declare none" fact 601 through 609 already record. Tested by
`a_native_mechanism_is_preferred_over_an_additive_one`,
`a_profile_no_native_style_represents_falls_through_to_the_additive_mechanism`,
`an_adapter_that_declares_nothing_says_so_rather_than_inventing_a_mechanism`.

**616, 617, 618, 619, 620, 621, 622, 623 — unchanged from the previous entry**,
re-verified rather than re-argued:
- 616 stays open (vacuously true, no per-turn injection path exists to avoid
  repeating — §5's standard against ticking an absence).
- 617 stays closed, on `main.rs::launch_session`, which this package does not
  own or touch.
- 618 stays open. `StyleChange` still has two of the line's three terms
  (`InPlace`/`NewSession`) and grepping the whole crate
  (`grep -rn 'communication_style\|StyleChange' crates/glasshouse/src`) still
  finds no reader outside `harness/mod.rs` itself and test scaffolding
  (`session/select.rs`'s and `profile/mod.rs`'s uses are all `Declared::Unverified`
  fixtures). Adding a third variant here would be inert without a reader, and
  a reader belongs in `config/response.rs` or `session/**`, neither of which
  this package owns — §5 and the packet's own instruction against
  manufacturing a caller both apply.
- 619, 620 stay open/blocked: no surface exists to change a *running*
  session's profile (session/**, not owned here).
- 621 stays not-this-package's (`crate::tui`/`crate::shell`).
- 622 stays closed, true by construction (no model-rewrite path exists at
  all, which is what the line asks for — unlike 616, this line's requirement
  *is* the absence).
- 623 stays open. Re-checked for a free-text surface that might have been
  added since: `grep -rn 'custom_prompt\|extra_prompt\|prompt_addition\|freeform\|custom_instruction' crates/glasshouse/src/config/response.rs crates/glasshouse/src/profile/response.rs`
  finds nothing. `ResponseConfig` and `Preset` still have no field to hold an
  arbitrary addition, so nothing exists yet for this line to keep separate
  from. Any surface would live in `config/response.rs`, not owned here.

**627–630 — still NOT STARTED, unchanged.** `grep -rn 'fn score\|Score'
crates/glasshouse/src` still finds nothing; still waiting on Phase 47
(measurement channel), Phase 51 (evaluation hooks), and — for 630 — Phase 33A
(per-pairing storage).

**631 — found ALREADY CLOSED, by a package this one does not own.**
The previous entry recorded this as "not blocked, and not built... a
`disabled = true` key on `ResponseConfig` plus one arm in `response_stack`,
perhaps thirty lines." That work now exists:
`crates/glasshouse/src/config/response.rs`'s `ResponseConfig::enabled` /
`set_enabled`, `EffectiveConfig::response_injection_enabled` (project, then
user, then default-enabled) and `EffectiveConfig::response_stack`'s
`injection_enabled` gate on the `Role`, `Project` and `UserDefault` layers —
an explicit task/session request still applies, matching the line's intent
that disabling automatic injection is not the same as refusing an explicit
one. Regression tests already exist and pass:
`config::response::tests::disabling_injection_suppresses_configured_layers_but_not_an_explicit_request`,
`injection_enabled_layers_project_over_user_over_default`,
`the_three_automatic_behaviours_disable_independently`,
`enabled_key_round_trips_and_absence_parses_to_never_decided` (`cargo test -p
glasshouse --lib config::response:: `, 4 passed, 2026-08-27). This package did
not write any of it — `config/**` is outside its file ownership this round —
and does not tick the box; it is recorded here so the orchestrator does not
re-open work that is already done, and does not credit this package for it.

**632 — re-verified, still CLOSED as the earlier update recorded.**
`database.rs` still carries the `response_profile`/`response_mechanism`
columns from Phase 10's migration 8.

**Verification run this round** (macOS, no other `cargo` running — checked
with `ps aux` first, per §40): `cargo doc -p glasshouse --no-deps` clean;
`cargo test -p glasshouse --test response_profiles` (19 passed); `cargo test -p
glasshouse --lib profile::` (80 passed) and `--lib harness::` (95 passed);
`cargo test -p glasshouse --lib config::response::` (4 passed, read-only
confirmation of the 631 finding — that file is not this package's to change).
The full workspace gate (`scripts/ci-local.sh`) was **not** run by this
package: it is out of scope for a package that changed no code, and running
it beside four other live workers would violate §40 regardless.

### Orchestrator decision on 622 — conditional, NOT ticked

*"Do not run a second language model to rewrite every final answer by default."*
The audit argued this closed **by construction**: nothing in the crate calls a
model, so there is no rewriting path to disable. The reading is sound and the
property is real today.

**It is not ticked, and the reason is about how the box decays rather than about
today's code.** A prohibition satisfied by absence has nothing holding it: the
day a post-processing path is added, this box stays `☑` and nothing notices. That
is the opposite of every other closed box here, each of which has a test that
fails if its behaviour is removed.

This project already has the idiom — a source-scanning guard read by `str::lines`
(§14, so a CRLF checkout cannot blind it), of the kind that proves a code path
never opens the user's conversation databases. **One such guard closes 622**, and
it belongs with whoever next holds `profile/**`.

Ticking it without one would be recording a promise that no longer has a keeper.

---

# Correction to line 618's blocker — 2026-08-30

`GH-CLOSED-PHASES-RECON` re-derived this file's claim against current source
(§81) rather than accepting it, and it is **stale**.

This entry records *"no reader outside `harness/mod.rs`"* for line 618. That was
true when written. **`glasshouse doctor` now reads `StyleChange` in production**
— `integrations/mod.rs:1168-1178`, wired by `fc16943`.

**Two of line 618's three terms are therefore closed.** The
cache-invalidation term remains.

## But it is NOT "one enum variant", and the orchestrator's first record of this
## was wrong — corrected the same day, before anything was dispatched

The recon called the remainder *"one enum variant plus one adapter declaration —
smaller than anything else in either phase"*, and this entry repeated it. **A
Phase −1 check against source contradicts it on two counts.**

**1. It is not a variant.** `StyleChange` (`harness/mod.rs:693`) is two
*mutually exclusive* states — `InPlace | NewSession` — answering "does changing
the style need a new session". *"Likely to invalidate prompt caching"* is
**orthogonal**: a change can be in-place **and** invalidate the cache. Adding it
as a third variant would make the enum say two different things on one axis, and
`integrations/mod.rs:1168-1178` matches it exhaustively to render one phrase. It
needs a **separate field**, and that is a type change across every adapter.

**2. The declaration is the expensive half, not the wiring.** Every value here
is a `Declared::verified(value, evidence)` carrying a *measured* justification —
`claude_code.rs:120-135` names the harness version, the date, and the exact
observation (*"Claude Code 2.1.246 `claude --help` documents `--settings
<file-or-json>` as a launch option"*). To declare cache invalidation honestly, a
package must **observe** it per harness. Nothing in this repository can tell you
whether changing Claude Code's output style invalidates its prompt cache.

**So 618's third term is blocked on an observation nobody has made**, and
inventing a value would be map line 1294's error exactly: *"a fabricated value
here does not degrade the policy, it inverts it."* **Do not package it as
wiring.** What it needs first is a measurement, on a real harness, of whether a
style change costs the prompt cache — the same kind of work that produced the
two values already here.

**The rest of 9K is not cheap**, which was the recon's headline and is recorded
in the refusal register: 616 and 622 are Cluster Q (vacuous restraints over
capabilities that do not exist), 619/620 and half of 618 share one missing
in-session surface, 623 is a standalone missing surface, and **627–630 all wait
on a single measurement channel that does not exist** — attack the channel, not
the four lines (§83).

**Do not close 616 or 622 with a source-scanning guard test.** The recon noted
they are "closeable with a test rather than a feature"; that is exactly how map
lines 1455 and 1456 were closed and then un-ticked the same week. A guard over
an absent capability is a useful **tripwire** and does not tick a box.

# Line 618 — COMPLETE 2026-09-01, on a real measurement

Packages `GH-STYLE-CACHE-MEASUREMENT` (recon) + `GH-STYLE-CACHE-DECLARATION`
(batch 75). The measurement this entry's own correction said nobody had
made: on Claude Code 2.1.252, an output-style change — and an appended
one-turn system prompt, identically — invalidates the prompt cache
**partially, for exactly one turn, non-sticky** (~13–14k re-cached over an
~18.5k surviving base; 4 experimental runs vs 2 controls, raw numbers in
`.agent-runtime/swarm-2026-09-01/style-cache.md`). Landed as
`CommunicationStyle::cache_invalidation: Declared<CacheInvalidation>` — the
separate field this entry ruled for, one variant (`Partial { one_turn }`)
because no other shape was observed; `Declared::verified` with the
measurement's evidence string on Claude Code, honestly `Unverified` on
Hermes, and no literal to change on the five whole-struct-Unverified
adapters. `doctor` renders measured and unknown distinctly. Mutation
(declaration flipped to does-not-invalidate) KILLED.

9K's remainder after this batch: 616/622 (Cluster Q, standing), 623 (a
standalone surface), 627–630 (one missing measurement channel — attack the
channel).
