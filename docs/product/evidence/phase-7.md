# Capability evidence — phase 7

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 7 — Keep terminal-text parsing only as a fallback for state that cannot be obtained structurally

Contract: Given a session whose state changes, when Glasshouse records it, the
change came from something structural — the operating system reporting the
process, or the harness reporting through a hook — and never from reading the
terminal and inferring.

State: COMPLETE. Glasshouse has no text-parsing fallback at all, which is a
stronger position than the line asks for.

Production evidence:
- `session/lifecycle.rs` translates harness events only, and cannot see
  terminal output.
- `session/runtime.rs: poll_exits` asks the process, never the output — a
  harness can be silent for minutes while thinking.
- The only writers of session state are the launch path and the shell (session
  started, stopped, failed) and the hook path.

Regression evidence:
- `nothing_derives_session_state_from_terminal_output` — the translator names
  no scrollback, screen or emulator type, and the runtime, which is the one
  place that *does* see terminal output, may not move a session's state.
  **Mutation-checked** by giving the runtime a method that infers a state and
  writes it: the test fails.

Missing evidence:
- None. If a future capability genuinely needs a text heuristic, this test is
  where the decision has to be argued rather than slipped in.

### Phase 7 — Record Claude compaction events when they can be observed reliably

Contract: Given a Claude Code session that compacts its context, when that
happens, Glasshouse records it.

State: NOT STARTED — **blocked by the harness, not by Glasshouse.**

Missing evidence:
- Claude Code 2.1.245 exposes **no compaction hook**. The event names a real
  installation was found accepting are `PreToolUse`, `PostToolUse`,
  `PostToolUseFailure`, `PermissionRequest`, `UserPromptSubmit`, `Stop`,
  `StopFailure`, `SubagentStart`, `SubagentStop` and `TeammateIdle`. None
  concerns compaction.
- Codex, by contrast, *does*: its recorded hook state carries `pre_compact` and
  `post_compact`, so Phase 8's equivalent line is reachable and this one is not.
- The capability says "when they can be observed reliably". They cannot be, in
  this version, and the only honest alternatives are to wait for a hook or to
  scrape the terminal for a compaction banner — which
  `nothing_derives_session_state_from_terminal_output` exists to prevent.
- Revisit when a Claude Code release exposes one.

### Phase 7 — Claude Code lifecycle hooks (one line closed, three pending one probe)

Contract: Given a running Claude Code session, when the harness reaches a
point it reports structurally — a prompt submitted, permission asked, a turn
ended — Glasshouse records the matching session state, while never editing the
user's own Claude Code configuration and never costing the user a turn if the
reporting fails.

State: **COMPLETE** for hook integration, translation and turn-completion
detection — all three observed end to end against the real harness. Permission
detection is PARTIALLY VERIFIED and its box stays unchecked; see the missing
evidence.

Production evidence:
- `harness/mod.rs: HookCommand`, `HookInstallation`,
  `HarnessAdapter::hook_installation` — the contract. The adapter builds the
  document because its *shape* is the harness's own business.
- `harness/claude_code.rs: hook_installation` — generates a settings document
  declaring `UserPromptSubmit`, `PermissionRequest`, `Stop` and `StopFailure`,
  installed with `--settings`, which loads *additional* settings so the user's
  own hooks keep running.
- `session/lifecycle.rs: lifecycle_for` / `may_apply` — the translation, and
  the only place that knows both vocabularies.
- `main.rs: report_hook` and the hidden `glasshouse hook` command — what the
  harness actually runs.
- `session/select.rs: HarnessSelection::install_hooks` — writes the document
  into a directory Glasshouse owns inside the project's own state. It never
  touches `~/.claude`.

Regression evidence:
- `an_installed_hook_moves_the_session_state` (PTY smoke, Unix) — launches
  through the shipped binary, reads the `PermissionRequest` command *out of the
  document Glasshouse generated*, runs it through a shell exactly as Claude
  Code would, and asserts the session moved to `WaitingForUser`. This is a test
  of the quoting as much as of the reporting. **Mutation-checked**: dropping
  the pinned paths from the command fails it.
- `a_hook_that_cannot_report_still_exits_zero` — an unknown session, a
  malformed identifier and an unrecognised event all exit 0.
  **Mutation-checked**: exiting non-zero on failure fails it.
- `the_generated_settings_document_is_valid_json_in_the_verified_shape` —
  parsed with `serde_json` and checked field by field against the shape read
  from a real Claude Code settings document.
- `a_hook_command_pins_every_path_it_needs`,
  `a_hook_command_survives_a_space_in_a_path`,
  `a_generated_document_escapes_backslashes`.
- `claude_codes_events_translate_to_the_states_they_mean`,
  `a_failed_turn_leaves_the_session_alive`,
  `an_unfamiliar_event_changes_nothing`,
  `a_finished_session_is_never_revived_by_a_late_hook` (**mutation-checked**),
  `a_live_session_follows_its_harness`.

Failure/isolation evidence:
- A hook **must** exit 0. Claude Code treats a non-zero exit as a veto: a
  `UserPromptSubmit` hook that exits non-zero blocks the prompt outright, with
  the user's own words echoed back and nothing sent. Observed directly against
  the real binary, which is why `report_hook` swallows every failure into the
  log.
- A late hook cannot revive a finished session: hook processes outlive their
  harness, and `may_apply` requires the current state to be live.
- An unrecognised event changes nothing rather than guessing a state.

Platform/external evidence:
- Claude Code 2.1.245 **fires hooks from a `--settings` document**: a
  hand-written document with a `UserPromptSubmit` hook ran its command and,
  by exiting non-zero, blocked the prompt — so the mechanism was proven
  without spending a turn.
- Claude Code 2.1.245 **does not fire `SessionStart`**: a document declaring
  one was installed and never ran, while `UserPromptSubmit` from the same
  document did. That is why `SessionStart` is not among the reported events,
  pinned by `session_start_is_not_among_the_reported_events`.
- Claude Code 2.1.245 **accepts the document Glasshouse generates**: started
  with one, it parsed it and reached its workspace-trust prompt.

- **Claude Code fires the generated document's hooks — run and observed.** A
  Glasshouse session was opened against the real `claude` in a pseudo-terminal,
  one prompt was submitted, and the session record moved from `starting` to
  **`idle`**.
- That value is conclusive rather than suggestive: the only production code
  that *writes* `Idle` is the `Stop`/`StopFailure` arm of
  `session::lifecycle::lifecycle_for`. Nothing else in Glasshouse can produce
  it, so the record could only have got there by Claude Code running the hook
  Glasshouse installed, which invoked `glasshouse hook`, which translated the
  event. The full chain — generate, install, fire, report, translate, record —
  is proven.

Missing evidence:
- **Permission detection specifically.** `PermissionRequest` is installed,
  translated and proven to move the record when its command runs, but Claude
  Code firing *that particular* event has not been watched: the verifying turn
  needed no permission, and this machine's Claude Code runs in auto mode, where
  a prompt that would ask is approved without asking. Its map box stays
  unchecked.
- Hook firing is verified on macOS only. The generated document and the
  reporting command are platform-independent and covered everywhere by the
  tests above, but Claude Code's own hook execution on Windows is unverified.

### Phase 7 — Support resuming a known Claude Code session through Claude Code's native resume mechanism

Contract: Given a recorded session that has a conversation to return to, when
the user resumes it, Glasshouse reopens that same conversation in the harness
that created it, while refusing anything it cannot honestly reopen.

State: COMPLETE.

Production evidence:
- `cli.rs: Command::Resume` and `main.rs: resume_session` — the command.
- `session/store.rs: SessionStore::resolve_id` — accepts any leading part of an
  identifier. Not a convenience: `glasshouse sessions` prints only the first
  twelve characters, so the short form is the *only* identifier a user can copy
  from the screen, and a command demanding all thirty-two would be unusable
  with what Glasshouse itself shows. Ambiguity is refused and names every
  candidate.
- `session/select.rs: HarnessSelection::resume_args` — the adapter's start
  arguments, then its resume invocation, then the user's own.
- `harness/claude_code.rs: resume` — `--resume <id>`.
- The harness is whichever one the *record* names, not whichever is configured
  now: resuming a Codex conversation in Claude Code would be nonsense.

Regression evidence:
- `a_recorded_session_is_resumed_under_the_identifier_it_was_given` (PTY smoke,
  Unix) — launches through the shipped binary, reads the assigned identifier
  from the harness's own arguments, takes the *short* identifier out of
  `glasshouse sessions`, resumes with it, and asserts the harness was handed
  `--resume` with that same identifier and **not** a fresh `--session-id`.
  **Mutation-checked**: resuming a different conversation fails it.
- `resuming_a_session_with_no_conversation_is_refused` — see the Phase 1 entry
  above; the harness is never started.
- `resuming_an_unknown_session_is_refused`.
- `the_short_form_the_listing_prints_is_enough_to_resolve`,
  `an_ambiguous_prefix_is_refused_and_names_its_candidates`,
  `a_wildcard_cannot_be_smuggled_into_the_lookup` — the last because
  identifiers are matched with `substr` and not `LIKE`, under which a bare `%`
  would match every session in the project.

Platform/external evidence:
- Against the real binary: `claude --resume <assigned-uuid>` reopened the
  conversation with its earlier turn replayed, and `claude --resume
  00000000-0000-4000-8000-000000000000` answered "No conversation found with
  session ID: …" and exited. Neither cost a model turn.

Missing evidence:
- A session that started and died before its harness created a conversation
  still reads as resumable, and resuming it will be refused by the *harness*
  rather than by Glasshouse. That is a clear message rather than lost state,
  and it is recorded as a loose end.

### Phase 7 — Add a Claude Code adapter that starts the real claude executable inside the current project root

Contract: Given a project and an enabled Claude Code, when the user opens a
session, Glasshouse starts the user's own installed `claude` with its working
directory set to the project root, while never substituting a different
program or a different directory.

State: COMPLETE.

Production evidence:
- `harness/claude_code.rs: ClaudeCode` — names `claude`, and `start()` is bare
  because `claude` run with no arguments "starts an interactive session by
  default".
- `session/select.rs: select` → `HarnessSelection::adapter` — resolves that
  name, or an explicitly configured path, refusing ambiguity.
- `main.rs: launch_session` and `shell/mod.rs: start_session` — both build the
  command through `HarnessSelection::start_args` and `launch::HarnessLaunch`,
  which derives the working directory from the active project and offers no
  way to override it.

Regression evidence:
- `a_claude_code_session_is_launched_and_recorded_under_one_identifier` (PTY
  smoke, Unix) — runs the shipped binary with `launch claude-code` and reads
  the argument list back from the harness itself.
- `the_working_directory_of_a_launched_harness_is_the_project_root` and the
  existing Phase 1 marker-harness smokes — the child's own `pwd` is the
  project root, on all three platforms in CI.
- `the_executable_names_match_the_installed_binaries` — the name is `claude`.

Platform/external evidence:
- `glasshouse doctor` on this machine resolves the real Claude Code 2.1.245 at
  its installed path and reads its version.

Missing evidence:
- None. Note that "the real claude executable" means whatever the user's
  configuration or `PATH` resolves; Glasshouse deliberately never bundles or
  substitutes one.

### Phase 7 — Capture the native Claude Code session identifier when it can be obtained reliably

Contract: Given a new Claude Code session, when Glasshouse starts it,
Glasshouse knows that session's native identifier and records it against the
session, while never recording an identifier the harness did not receive.

State: COMPLETE.

Production evidence:
- `harness/mod.rs: HarnessAdapter::assign_session_id` — the contract. Assigning
  beats discovering: an identifier chosen before the process exists survives a
  harness that dies during startup, needs no filesystem watching and no
  parsing, and cannot be confused with a session started at the same moment.
- `harness/claude_code.rs: assign_session_id` — `--session-id <uuid>`.
- `session/store.rs: SessionStore::new_native_session_id` and
  `uuid_v4_from_hex` — mints an RFC 4122 version-4 UUID from SQLite's
  randomness, the same source the store already uses for its own identifiers.
  Deliberately *not* derived from the Glasshouse session identifier: the two
  identifier spaces are independent by design.
- `session/select.rs: HarnessSelection::start_args` /
  `assigns_native_session_id`, and both production start paths, which mint the
  identifier, record it on the `NewSession`, and pass it to the harness.

Regression evidence:
- `a_claude_code_session_is_launched_and_recorded_under_one_identifier` (PTY
  smoke, Unix) — the shipped binary launches a harness that reports its own
  argument list, and the identifier found there is compared with the one read
  back from the store. **Mutation-checked twice, in both directions**: handing
  the identifier over without recording it fails, and recording it without
  handing it over fails. Either alone would be useless — an unrecorded
  identifier cannot be resumed, and an unhanded one names a conversation that
  does not exist.
- `assignment_agrees_with_the_declaration` — an adapter cannot hand out
  `--session-id` arguments while declaring its identifiers are only
  discoverable, or the reverse.
- `claude_code_assigns_the_identifier_its_binary_demands` and
  `a_harness_that_cannot_be_told_its_identifier_assigns_none`.
- `a_minted_native_identifier_is_a_valid_version_4_uuid`,
  `minted_native_identifiers_do_not_repeat`, and
  `the_uuid_formatter_only_overwrites_the_version_and_variant` — the last
  pins that 122 of the 128 bits survive, so the identifier keeps the
  randomness it was given.
- `launching_a_harness_records_a_session_that_a_later_command_reads_back` — a
  cleanly stopped Claude Code session now reads as **resumable**, the first
  time any session reaches that disposition in production. It read `closed`
  before, which was correct then and is wrong now.

Platform/external evidence:
- Claude Code 2.1.245 **rejects** a non-UUID outright: `claude --session-id
  not-a-uuid` answers "Error: Invalid session ID. Must be a valid UUID." The
  requirement is enforced by the binary, not merely documented, which is why
  the minted identifier is a strictly valid version-4 UUID rather than merely
  UUID-shaped.
- Claude Code 2.1.245 **accepts** a Glasshouse-minted identifier: started in a
  pseudo-terminal with one, it came up and ran normally until killed, where an
  invalid one exits immediately.

- **The assigned identifier becomes the conversation's own — run and
  observed.** With the user's approval, `claude --session-id
  7f3a91c2-5d84-4e11-9a3b-6c0d2e8f41ab -p "..."` was run once in a scratch
  directory, and Claude Code wrote its transcript to
  `~/.claude/projects/<slugged-cwd>/7f3a91c2-5d84-4e11-9a3b-6c0d2e8f41ab.jsonl`
  — the assigned identifier exactly.
- **The identifier is resumable, and an unknown one fails cleanly.**
  `claude --resume 7f3a91c2-…` reopened that conversation with its earlier turn
  replayed, while `claude --resume 00000000-0000-4000-8000-000000000000`
  answered "No conversation found with session ID: …" and exited. Both were
  observed in a real pseudo-terminal; neither cost a model turn.

Missing evidence:
- None. The chain is closed end to end: Glasshouse mints an identifier, hands
  it to the harness, records the same one, and the harness both stores the
  conversation under it and reopens it on demand.

---

### Phase 7 — Detect when Claude Code requires user input or permission through structured events when possible (line 308)

State: **COMPLETE** — orchestrator ruling, batch 51.

Contract: Given a Claude Code session that stops for a permission decision,
when Claude Code fires its `PermissionRequest` hook, Glasshouse records that
session as waiting for the user everywhere a reader can see it — and does not
record an ordinary turn end the same way.

**The production path already existed and was already correct.**
`session/lifecycle.rs:66` maps `"PermissionRequest" => WaitingForUser`, and
`PermissionRequest` is in `claude_code.rs`'s `REPORTED_EVENTS`. What did not
exist was any test entering where Claude Code enters: the four existing
assertions call `lifecycle_for()` directly, and `lifecycle_for` is not what
Claude Code runs (§35).

Regression: `tests/adapter_lifecycle.rs` spawns the built binary as
`glasshouse hook --session <id> --event PermissionRequest` with a payload on
stdin, exactly as a harness does. **"Observable" is taken as three readers, not
one row** — the session store every listing reads, the event log an observer
tailing the project reads, and the disposition deciding whether a session is
offered as live. A discriminating test sends `Stop` through the same process and
requires the two to land in different states; without it the first test would
pass against a build that mapped every report to one state.

Mutation: `drop-permissionrequest-from-reported-events` — KILLED, re-run by the
orchestrator, failing
`the_settings_document_glasshouse_installs_subscribes_to_permission_requests`.

**A defect a mutation found in the test itself, worth recording.**
`claude_code.rs` has two event lists — `HOOK_EVENTS` (what Claude Code supports)
and `REPORTED_EVENTS` (what Glasshouse subscribes to). The first version of this
test asserted on the former while claiming to prove the latter: deleting
`PermissionRequest` from `REPORTED_EVENTS` left it green against a build that
would never have received a single permission report. That is §80 case 3 — the
site mutated was not the site the test read — caught by mutation rather than by
review. Rewritten to go through `hook_installation()` and assert on the rendered
document bytes.

`scripts/mutate.sh` also refused the naive form of this mutation outright
(*"find string occurs 2 times, need exactly 1"*), which is the same trap guarded
mechanically.
