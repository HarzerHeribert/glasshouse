# Glasshouse capability evidence ledger

This ledger supports—but never replaces—the authoritative
[`GLASSHOUSE_IMPLEMENTATION_CAPABILITY_MAP.md`](GLASSHOUSE_IMPLEMENTATION_CAPABILITY_MAP.md).
It maps requirements to observable product contracts, production paths, and
non-vacuous regression evidence.

Populate entries incrementally as a capability becomes active or as previously
checked work is reconciled. Do not spend a whole implementation cycle filling
hundreds of future entries speculatively.

## Entry template

```markdown
### <phase and stable short name> — <exact capability text>

Contract: Given <context>, when <trigger>, Glasshouse <observable behavior>,
while preserving <invariant or failure behavior>.

State: NOT STARTED | SCAFFOLDED | PARTIALLY VERIFIED | LOCALLY VERIFIED |
CI VERIFIED | COMPLETE

Production evidence:
- `<file>: <symbol/path>` — why this is a real reachable production path

Regression evidence:
- `<test name>` — behavior proved and platforms actually executed

Failure/isolation evidence:
- `<test or probe>` — negative, fail-closed, cleanup, or boundary behavior

Platform/external evidence:
- `<CI run or runtime probe>` — commit and platforms covered

Missing evidence:
- exact remaining proof or implementation
```

## Evidence rules

- Quote the capability exactly enough to find it in the map.
- Keep the contract to one product sentence.
- Cite symbols and test names, not merely directories.
- State which platform actually executed a test.
- A test-only type or fake caller is not production evidence.
- A checked box requires **COMPLETE**.
- If later evidence contradicts an entry, downgrade it immediately and reopen
  the map checkbox if necessary.

## Active entries

### Phase 6 — Make each adapter declare which native approval/permission modes it supports

Contract: Given a supported harness, when Glasshouse is asked what approval
modes it offers, the adapter answers with what was read from that harness's own
binary — naming its automatic-review mode when it has one, and saying plainly
when nobody established one — while never presenting a blanket bypass as though
it were review.

State: COMPLETE.

Production evidence:
- `harness/mod.rs: ApprovalModes` — `automatic_review`, `bypass` and `sandbox`,
  each a `Declared<&'static str>` like every other harness fact.
- All seven adapters declare it; `HarnessDescription` carries it.
- `integrations/mod.rs: write_adapter_report` — `glasshouse doctor` prints it,
  which is what keeps the declaration from being data nothing reads.

The distinction the type exists for: **a blanket bypass is not automatic
review.** Claude Code's auto mode, Codex's `--approve-for-me` ("automatic review
using the workspace-write sandbox") and Cursor's `--auto-review` ("a server
classifier auto-runs safe tool calls") classify. OpenCode's `--auto`
("auto-approve permissions that are not explicitly denied (dangerous!)"),
Hermes's `--yolo` and Antigravity's `--dangerously-skip-permissions` do not, and
are recorded as bypasses only.

Regression evidence:
- `each_adapter_declares_the_approval_mode_its_binary_documents` — pins the
  whole table, harness by harness, mode string by mode string.
- `every_verified_declaration_cites_its_evidence` covers the new field.
- `the_doctor_report_describes_every_adapter` — the rendered row.

Non-vacuity: **the first version of this test was weak and a mutation proved
it.** It asserted only that an `automatic_review` evidence string avoided the
words "yolo", "dangerously" and "bypass"; a mutation recording OpenCode's
`--auto` as automatic review, with evidence reading "…(dangerous!)", walked
straight through — "dangerous!" is not "dangerously". The fuzzy test was
replaced by the exact table above, and the same mutation now fails, as does
removing Cursor's real `--auto-review`. The lesson is recorded in the test's own
comment: pin *which mode each harness has*, never how a declaration is worded.

Failure/isolation evidence:
- `Unverified` renders as **"automatic review unverified"**, never "no automatic
  review". `Declared` cannot express "verified absent" for a mode name, so
  absence of a declaration means nobody established one — a different claim from
  the harness not having one. Pi makes it concrete: installed, but not on `PATH`
  here, so its `--help` could not be read and everything about it is
  `Unverified`.

Platform/external evidence:
- Every declaration read on 2026-08-25 from the installed binaries: Claude Code
  2.1.245, Codex 0.149.1, Cursor CLI 2026.08.11, OpenCode 1.18.22, Hermes Agent
  0.15.1, Antigravity CLI 1.1.20. Pi 0.73.1 could not be read.
- `glasshouse doctor` run from the built binary and its output read, which is
  how the "no automatic review" overstatement was caught before it shipped.

Missing evidence:
- Pi's approval modes. Needs `~/.hermes/node/bin` on `PATH`, or a configured
  explicit executable path.
- Selecting a mode is Phase 9A, and unimplemented. This line is the declaration
  half only.

### Phase 8 — Detect Codex waiting-for-user and permission states structurally when possible

Contract: Given a Codex session Glasshouse started, when Codex stops to ask the
user to allow something, Glasshouse records the session as waiting for the user
— and records it moving on once they answer — without reading the terminal or
inferring anything from what is on screen.

State: COMPLETE (macOS; the mechanism is platform-independent and its unit
coverage runs everywhere).

Production evidence:
- `harness/codex.rs: REPORTED_EVENTS` includes `PermissionRequest`.
- `session/lifecycle.rs: lifecycle_for` — `"PermissionRequest" =>
  WaitingForUser`. Shared with Claude Code, which spells it identically.
- The hook chain proven in the entry below carries it.

Regression evidence:
- `codex_events_translate_to_the_states_they_mean` covers
  `PermissionRequest -> WaitingForUser`.
- `nothing_derives_session_state_from_terminal_output` — the state cannot have
  come from reading the screen, because no production code may do that.

Platform/external evidence — **the whole cycle was watched against the real
binary**:
- `glasshouse launch codex -- --sandbox read-only --ask-for-approval on-request`
  (which also demonstrates the `--` pass-through reaching the harness: Codex
  reported "Read Only").
- Asked to create a file, Codex raised its own approval prompt — "Apply
  proposed file edits / Yes, proceed" — and the session record moved to
  **`lifecycle = 'waiting_for_user'`**.
- On approving, the file was created and the record moved to
  **`lifecycle = 'idle'`**.
- So the observed cycle is `running -> waiting_for_user -> idle`, every
  transition written by a hook Codex fired, none of it inferred from the
  screen.

Missing evidence:
- Linux and Windows have not executed a real permission cycle; the translation
  itself is unit-covered on all three.

### Phase 8 — Codex lifecycle hooks (three lines: integrate, translate, detect turn completion)

Contract: Given a Codex session Glasshouse starts in a project where the user
has consented to project-local hooks, when Codex reports a lifecycle event,
Glasshouse records the session state that event implies — while never reading
the conversation content the payload carries, never writing outside the
project, and never letting a hook failure affect the session.

State: COMPLETE on macOS and Linux; Windows green after a defect it caught.

**Windows CI found a third real defect on this repository**, this time in the
payload scan's own helper. It located the end of `report_hook` with
`find("\n}\n")`, and Windows checks this file out with CRLF endings, so the
closing brace reads `\r\n}\r\n` and the pattern never matched. It panicked
rather than scanning an empty span — the right direction to fail in, and exactly
what the hardening was for. The fix matches only the newline *before* the brace,
which works on both; a brace at column zero can only be the function's own.

**The whole chain was watched running against the real binary**, the same proof
Phase 7 used. `glasshouse launch codex` was run in a real terminal against
Codex 0.149.1 with `project_hooks = true`. Glasshouse generated the document and
wrote it to `<project>/.codex/hooks.json` (five events, `timeout: 3`, every path
pinned); Codex asked to trust the directory, then asked to review the hooks;
after trusting, one real turn was taken — and the session record moved to
**`lifecycle = 'idle'`**.

That value settles it rather than suggesting it. The only production code that
*writes* `Idle` is the `Stop`/`StopFailure` arm of
`session::lifecycle::lifecycle_for`; nothing else in Glasshouse can produce it.
So the record could only have reached that state by Glasshouse generating the
document, writing it under consent, Codex reading and trusting it, Codex firing
`Stop`, that hook invoking `glasshouse hook`, and the translation recording it.
Generate, install, trust, fire, report, translate, record — end to end.

Quitting the session cleanly then moved it to `stopped` and **captured
`native_session_id = 01a03983-b696-7832-ac49-296a4deccda1`**, which was verified
to be the exact rollout Codex wrote for that project
(`originator: codex-tui`, no `parent_thread_id`, matching `cwd`). Disposition
read `resumable`. **That also closes the open gap in the Phase 8 line 2 entry
above** — a live Codex turn has now had its identifier captured end to end.

Production evidence:
- `harness/mod.rs: HookDestination` — `GlasshouseOwned` or
  `ProjectLocal { relative_path }`. Making the destination part of the
  declaration is what keeps the consent rule enforceable in one place; no
  adapter can opt out of it.
- `harness/codex.rs: Codex::hook_installation` — builds the `hooks.json`
  document, declares `ProjectLocal { ".codex/hooks.json" }` and **empty
  arguments**, because Codex finds the file itself.
- `session/select.rs: install_hooks` — writes a `ProjectLocal` installation
  only with consent; without it, no file, no directory, `Ok(None)`, and the
  session starts normally.
- `config/mod.rs` — `project_hooks` consent, an `Option<bool>` so the tri-state
  is per field. A plain `bool` with `#[serde(default)]` caused a real defect
  before and is not repeated.
- `session/lifecycle.rs` — `SessionStart` added. `UserPromptSubmit`,
  `PermissionRequest` and `Stop` needed no change at all, because **Codex
  spells them exactly as Claude Code does**. The module's doc claiming Codex
  used snake_case was wrong and is corrected.
- `main.rs: report_hook` — drains stdin into `std::io::sink()` and never parses
  it.

Regression evidence:
- `codex_hooks_are_written_into_the_project_only_with_consent` — without
  consent, no file and no `.codex` directory.
- `codex_hooks_are_written_where_codex_reads_them` — with consent, the document
  lands at `<root>/.codex/hooks.json` and names the five reported events.
- `a_codex_hook_declares_a_timeout_codex_will_not_clamp` — every timeout <= 3.
- `codex_declares_a_project_local_destination`,
  `claude_code_and_codex_are_the_harnesses_with_a_verified_hook_installation`.
- `codex_events_translate_to_the_states_they_mean`.
- `the_hook_command_never_reads_its_payload` plus
  `the_payload_scan_would_catch_a_violation`.
- `tri_state_project_hooks_consent_distinguishes_never_asked_from_a_decision`.

Non-vacuity: **four mutations run by the orchestrator, four killed** — removing
the consent gate; raising Codex's declared timeout to one Codex would clamp;
mapping `SessionEnd` (which must stay unmapped); and making the hook handler
read and log its payload. The source scan was additionally hardened: it now
asserts the slice it scans actually contains `std::io::sink()`, because a scan
over the wrong span passes for the wrong reason — this project has been caught
by exactly that before, with a `skip_while` that found a harness list where an
adapter block was meant.

Failure/isolation evidence:
- **The payload carries the conversation** — `prompt` is the user's own words,
  `last_assistant_message` the model's reply. Glasshouse takes the event name
  and session identifier from its own argv and reads neither. Proven by mutation,
  not asserted.
- `SessionEnd` is deliberately **not** mapped: the operating system reporting
  the process is the authority for a session ending, and a hook only races it.
- A late hook cannot revive a finished session (`may_apply`); an unfamiliar
  event changes nothing.
- The one place Glasshouse ever writes inside a user's repository logs the exact
  path it created.

Platform/external evidence:
- Codex 0.149.1, probed in a real terminal: a project-local
  `.codex/hooks.json` is read (Codex named the file in its own diagnostic),
  hook trust is a prompt distinct from workspace trust, and `SessionStart`,
  `UserPromptSubmit` and `Stop` all fired with one real turn taken.
- **Codex clamps hook timeouts**, announcing `clamping SessionEnd hook timeout
  to 3s`. The declared timeout is 3 so a real installation warns about nothing.

Missing evidence:
- CI on Linux and Windows.
- Lines 8 and 9 (waiting-for-user/permission states, compaction) are open.
  `PermissionRequest` is installed and translated but has not been watched
  firing; `PreCompact`/`PostCompact` exist in Codex's catalogue and are not yet
  requested.

### Phase 8 — Support resuming a known Codex session through Codex's native resume mechanism

Contract: Given a recorded Codex session whose native identifier Glasshouse
captured, when the user resumes it, Glasshouse reopens that same conversation
through Codex's own `resume` subcommand — while never handing Codex a flag
belonging to a different harness, and never assigning a fresh identifier to a
session that already has one.

State: COMPLETE.

Production evidence:
- `main.rs: resume_session` — generic by construction. It selects the harness
  the *record* names, not whichever is configured now, and asks that harness's
  adapter for the invocation. Nothing in it knows Codex exists.
- `session/select.rs: HarnessSelection::resume_args` — the adapter's start
  arguments, then its resume invocation, then the user's own.
- `harness/codex.rs: Codex::resume` — `["resume", <id>]`.
- `session/store.rs: open_for_resume` — decides whether a session may be
  resumed at all (right project, not still running, something to resume to)
  before a harness is selected and long before a process exists.
- Nothing was written for this line. What made it reachable was Phase 8 line 2:
  until an identifier was captured, `glasshouse resume` could only ever answer
  "not resumable".

**The shape difference is the whole point.** Codex resumes with a *subcommand*,
Claude Code with a *flag*. That is precisely the harness-specific knowledge the
Phase 6 adapter contract exists to absorb, and it is now asserted rather than
assumed.

Regression evidence:
- `a_recorded_codex_session_is_resumed_through_its_own_subcommand` (PTY smoke,
  **executed on macOS, Linux and Windows** — deliberately not `#[cfg]`-gated,
  because Windows CI found a real defect on this exact fixture path for line 2.
  Confirmed by name in CI `32854403090`'s Windows and Linux job output, not
  inferred from a green tick; the Windows PTY count went 35 -> 36).
  Drives the shipped binary end to end: `glasshouse launch codex` against a
  fake harness that writes a real rollout header under an isolated `CODEX_HOME`
  and echoes its own argv; asserts the launch argv is **bare**; takes the
  *short* twelve-character identifier out of `glasshouse sessions`, which is
  the only form that listing shows and therefore the only one a user could
  type; resumes with it; then asserts the harness was handed `resume` and the
  captured identifier, and **not** `--resume` and **not** `--session-id`.

Non-vacuity: **three mutations run by the orchestrator, all three killed** —
giving Codex Claude Code's `--resume` flag; returning no resume arguments at
all; and resuming a different conversation. The first is the one that matters:
it is the adapter contract leaking one harness's vocabulary into another, and
the test names that failure in its own assertion message.

Failure/isolation evidence:
- `resuming_a_session_with_no_conversation_is_refused` reaches `open_for_resume`
  through a Codex session precisely because Codex has no identifier to resume
  to until one is captured — that test was written for this shape.
- `resuming_an_unknown_session_is_refused`,
  `resuming_a_session_belonging_to_another_project_is_refused`,
  `a_live_session_is_not_resumable`.

Platform/external evidence:
- Against the real Codex 0.149.1 in a pseudo-terminal, neither costing a model
  turn: `codex resume <a real recorded id for this project>` printed
  `Resuming session…` and replayed the conversation; `codex resume
  9f1c0b2e-0000-4000-8000-0123456789ab` answered `ERROR: No saved session found
  with ID <id>. Run `codex resume` without an ID to choose from existing
  sessions.`
- Two traps found while probing, recorded so nobody rediscovers them: a
  pseudo-terminal with **no window size** makes Codex emit its handshake and
  draw nothing, which reads exactly like a hang; and Codex may open with an
  **update prompt whose default option runs `curl … | sh`**, so it must never
  be dismissed with Enter.

Missing evidence:
- Glasshouse does not yet surface the harness's own refusal. If a recorded
  identifier no longer exists, Codex prints its `ERROR: No saved session found`
  and exits, and Glasshouse reports only that the harness exited non-zero. The
  session record is optimistic — it says `resumable` on the strength of a
  captured identifier, not on proof the conversation still exists. Honest, but
  the message the user sees should be the harness's.

### Phase 8 — Capture the native Codex thread or session identifier when it can be obtained reliably

Contract: Given a Codex session Glasshouse started in the project root, when
that session ends, Glasshouse records the native identifier of the interactive
Codex session that ran in that directory inside that window — while recording
nothing at all when the right record cannot be told apart from a subagent
thread, another client's session, another project's session, or a second
candidate.

State: COMPLETE. Linux, macOS and Windows all executed these tests in CI
`32849951837`, and the two end-to-end tests were confirmed by name in the
Windows job's output rather than inferred from a green tick.

Production evidence:
- `session/native_id.rs: discover` — walks the harness's own records root,
  applies four filters, and refuses ambiguity.
- `session/native_id.rs: capture` — the production wiring, called at session
  end from **both** producers: `main.rs: launch_session` (before
  `note_lifecycle`) and `shell/mod.rs`'s `poll_exits` loop (before
  `set_lifecycle`). Best effort by construction: it only ever logs, because
  the harness has already run and a bookkeeping failure must not become the
  user's error.
- `harness/codex.rs: Codex::session_id_source` / `::read_session_record` — the
  Codex-shaped half: `CODEX_HOME`/`.codex`/`sessions`/`rollout-`/`jsonl`, and
  a pure parse of one header line.
- `session::store::set_native_session_id` finally has a production caller. It
  has existed unused since Phase 2.

Regression evidence (all executed on macOS; the unit tests are
platform-independent and run everywhere):
- `the_interactive_session_in_the_window_is_the_one_captured` — a subagent, a
  desktop record and the real interactive one, same cwd, same window; only the
  interactive one is taken.
- `a_subagent_thread_is_never_captured` — the case that would otherwise be
  captured most often.
- `another_project_s_session_is_never_captured`,
  `a_session_that_started_before_this_one_is_never_captured`,
  `two_candidates_are_refused_rather_than_guessed`,
  `a_record_without_a_session_id_field_is_skipped`,
  `an_unreadable_records_root_is_not_an_error`.
- `nothing_is_read_past_the_first_line` — the secret boundary.
- `a_codex_session_s_identifier_is_captured_by_the_launch_path` and
  `a_codex_session_started_from_the_shell_has_its_identifier_captured_on_exit`
  — the shipped binary, a fake `codex` that writes a real rollout-shaped
  header under an isolated `CODEX_HOME`, one per production call site.

Non-vacuity: **eight mutations were run by the orchestrator and all eight
failed their target test.** Dropping the interactive filter, the cwd filter or
the time window; resolving ambiguity by taking the first candidate; reading
`payload.session_id` instead of `payload.id`; reading the whole file instead of
the first line; and deleting each of the two call sites in turn. The two
call-site mutations are what make the wiring proved rather than asserted.

Failure/isolation evidence:
- Ambiguity records nothing: `Discovered::Ambiguous` has no writer.
- `no_adapter_depends_on_the_session_model` — a new architecture scan: no
  adapter may name `crate::session` in production code, with
  `the_adapter_dependency_scan_would_catch_a_violation` proving it fires on a
  fabricated `use` and stays quiet on a doc comment.
- `a_discoverable_adapter_declares_discoverable_session_ids` — one-directional
  by design: Cursor, Hermes, Pi and OpenCode correctly declare
  `SessionIds::Discoverable` about their own harnesses without Glasshouse
  having built a reader for each, so the converse is not a defect.

Platform/external evidence:
- Codex 0.149.0 on macOS. The rule was derived from **all 555 real rollout
  files** in `~/.codex/sessions`: every first line is `session_meta`;
  `payload.id` is present in 555 and always equals the filename UUID, while
  `payload.session_id` is present in only 527; `originator == "codex-tui"`
  with no `parent_thread_id` selects exactly the 70 real interactive sessions,
  zero counterexamples. **Subagent rollouts share their parent's `cwd` and
  outnumber real sessions 171 to 70**, which is why matching on `cwd` alone —
  the previous session's plan — would have captured the wrong identifier most
  of the time.
- `CODEX_HOME` relocates Codex's entire state root (verified: an isolated home
  received `version.json`, `installation_id`, its sqlite files, `skills`,
  `tmp`), which is what makes the two end-to-end tests hermetic.
- Codex writes no rollout at all until a turn has happened — verified by
  starting bare `codex` in a pseudo-terminal under an isolated `CODEX_HOME`
  and killing it: `sessions/` was never created. That is why discovery runs at
  session end and why `NotFound` is an ordinary outcome, not a fault.

What CI caught, and it was a production defect rather than a test one:
- Linux and macOS passed; **Windows failed both end-to-end tests** on the first
  push (`32849379504`). `read_first_line` refused any first line not ending in
  `\n`, and the Windows fixture wrote its rollout with `Set-Content
  -NoNewline`. A harness writes its header before it has anything to append, so
  a record whose only line is a complete one is ordinary — Glasshouse was
  discarding it and reporting the session as having no identifier.
- Invisible to the eight unit tests because the `write_rollout` helper appends
  `\n` to every fixture. `a_header_with_no_trailing_newline_is_still_read`
  writes the bytes directly and fails against the old rule.
- A second change made at the same time — widening the header trim to
  `str::trim` for CRLF — was **removed after a mutation showed it dead**:
  `serde_json` already treats a trailing carriage return as whitespace.
  `a_header_terminated_by_crlf_is_read` stays, pinning the property rather than
  the mechanism, and its comment records that the mutation is what settled it.

Missing evidence:
- ~~No live Codex turn has had its identifier captured end to end.~~ **Closed
  2026-08-25.** A real `glasshouse launch codex` session took a turn, was quit,
  and the record captured `native_session_id =
  01a03983-b696-7832-ac49-296a4deccda1` — verified against the rollout Codex
  actually wrote for that project. Disposition read `resumable`. See the Codex
  hooks entry below.
- Two Glasshouse Codex sessions started in the same project inside the same
  window will each see the other's rollout and both refuse. Fail-closed and
  honest, but a real edge if anyone runs parallel Codex sessions in one
  project; the fix is a narrower discriminator, never a ranking rule.
- No real Codex session's identifier has been captured end to end, because
  that costs a model turn. The header format is proven against 555 real files
  and the wiring against the shipped binary; what is unproven is only the
  join between them on a live turn.

### Phase 8 — the Codex adapter's first three lines

Contract: Given a project and an enabled Codex, when the user opens a session,
Glasshouse starts the user's own installed `codex` in the project root and its
interface appears in the viewport as Codex drew it — while Glasshouse's own
code stays unable to reach inside Codex.

State: COMPLETE for starting Codex, preserving its interface, and staying
uncoupled from its internals. The rest of Phase 8 is open.

Production evidence:
- `harness/codex.rs: Codex` — names `codex`, starts bare ("If no subcommand is
  specified, options will be forwarded to the interactive CLI").
- The same selection and launch path as every other harness: `session::select`
  resolves it, `HarnessLaunch` derives the working directory from the project.

Regression evidence:
- `the_real_codex_interface_appears_in_the_viewport` (PTY smoke, Unix, opt-in
  via `GLASSHOUSE_PROBE_REAL_HARNESS=1`) — drives the shipped shell against the
  real Codex and asserts Codex's own version string reaches the viewport,
  having first asserted it is absent before any session exists so a match
  cannot come from Glasshouse's chrome. The probe is shared with Claude Code,
  so both harnesses are held to the same check.
- `glasshouse_depends_on_no_harness_internal_crate` and
  `the_dependency_guard_would_catch_a_coupling` — the manifest names no
  harness's internals, and the guard is checked against a fabricated manifest
  that does. Fabricated deliberately: adding a nonexistent dependency to the
  real manifest fails in cargo's resolver and proves nothing about the test.
- The existing launch smokes already start a fake harness under the `codex`
  slug and assert its working directory is the project root.

Platform/external evidence:
- Codex 0.149.0, driven through the viewport on macOS: its interface renders.
- **Codex asks a question Claude Code does not.** Its startup handshake is
  `ESC[>5u`, `ESC[6n`, `ESC[?u`, `ESC[c`, `ESC[0 q`. The `ESC[?u` is the kitty
  keyboard-protocol probe, and Glasshouse deliberately stays silent on it —
  see `DELIBERATELY_UNANSWERED` in `session/runtime.rs`. Replying would claim a
  protocol `tui::event` does not encode for, and the harness would then
  mis-read every keystroke. Silence is the correct answer *and* is not a
  timeout: Codex sends `ESC[?u` and `ESC[c` together, and the device-attributes
  reply arriving with no keyboard reply before it is the negative answer.
  Pinned by `the_keyboard_protocol_query_is_deliberately_unanswered` and
  `device_attributes_still_answer_after_an_unanswered_question`.

Missing evidence:
- Session-identifier capture, resume, hooks and compaction are open. Codex
  writes no rollout file until a turn has happened (verified: starting it and
  killing it left the session count unchanged), so identifier discovery has to
  wait for the first turn and match on the rollout header's `payload.cwd` and
  `payload.id`.

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

### Phase 5/7 — the terminal handshake, and the defect it was hiding

Contract: Given a live harness session, when the harness asks its terminal a
question at startup, Glasshouse answers it, so the harness's own interface
works exactly as it would in a real terminal — and does not quietly degrade
itself, or the user's installation, when it does not.

State: COMPLETE. Phase 7's "Preserve the complete native Claude Code TUI
inside the Glasshouse PTY" box is now checked on the strength of this.

The defect, and how it was found:
- Driving the shipped shell against the real Claude Code 2.1.245, the viewport
  carried the harness's own notice: *"Claude Code's fullscreen renderer has
  repeatedly failed to start on this machine, so it has been turned off
  here."* The user's `~/.claude.json` had gained
  `fullscreenAutoDisabled = {"version": "2.1.245", …, "strikes": 2}`.
- So a Glasshouse session had made a harness permanently change the user's own
  installation, globally — breaking the product invariant that Glasshouse
  operates real harnesses without altering them. Worse, that user's
  `settings.json` reads `"tui": "fullscreen"`: Glasshouse had overridden an
  explicit preference.

The cause:
- A real Claude Code startup was captured in a pseudo-terminal. Of everything
  it writes before drawing, three sequences are *questions*: `ESC[6n`
  (cursor position), `ESC[c` (primary device attributes) and `ESC[>0q`
  (XTVERSION). The rest — bracketed paste, focus reporting, synchronised
  output, keyboard-protocol pushes — are instructions.
- Glasshouse answered exactly one of the three. Phase 5's own design note had
  already stated the rule ("an embedded session must always answer, or the
  harness hangs"); only the cursor-position half was ever built.

The fix:
- `session/runtime.rs: TerminalQuery` / `TerminalQueryScanner` recognise all
  three across chunk boundaries, and `answer_terminal_queries` replies to each:
  the emulated screen's cursor position, `ESC[?1;2c` for device attributes
  (what the viewport actually is, rather than a richer terminal whose sequences
  it could not draw), and its own name for XTVERSION rather than impersonating
  a terminal it is not.

Regression evidence:
- `every_startup_question_a_harness_asks_is_answered` (PTY smoke, Unix) — a
  harness asks all three through a real pseudo-terminal and every reply is
  found in its scrollback. **Mutation-checked three ways**: making either new
  query unrecognisable, or emptying the cursor reply, fails it.
- `a_query_is_found_however_a_read_splits_it`,
  `one_byte_at_a_time_still_finds_every_query`,
  `several_queries_in_one_chunk_are_all_found`,
  `a_near_miss_does_not_count_and_does_not_poison_the_next_match`,
  `a_reply_flowing_back_is_not_mistaken_for_a_question`.

Platform/external evidence (macOS, real Claude Code 2.1.245):
- **Before:** two sessions were enough to trigger `fullscreenAutoDisabled`.
- **After:** three consecutive sessions against an isolated Claude
  configuration left it **absent**, and the failure notice was gone —
  replaced first by Claude Code's *offer* of the fullscreen renderer, and then,
  with `"tui": "fullscreen"` set, by the fullscreen interface itself rendering
  in the viewport with no notice at all.
- The isolated configuration was used precisely so the verification did not
  touch the user's own; it was deleted afterwards.

Missing evidence:
- The user's real `~/.claude.json` still carries the `fullscreenAutoDisabled`
  record this defect caused. Glasshouse will not edit a harness's own
  configuration, so clearing it is the user's to do — `/tui fullscreen` in any
  Claude Code session resets it, and it also resets on the next update.
- Verified on macOS. The queries and replies are platform-independent and the
  tests run everywhere, but no real harness has been driven through the
  viewport on Windows.

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

### Phase 6 — the harness adapter interface (eleven of twelve)

Contract: Given a harness Glasshouse supports, when anything in core needs to
know how that harness is started, resumed, messaged, interrupted, observed or
described, Glasshouse asks that harness's adapter and gets an answer derived
from the installed binary, while core itself stays unable to name any
particular harness.

State: COMPLETE for eleven of the phase's twelve lines. The
communication-style line is PARTIALLY VERIFIED and its box stays unchecked —
see its own entry below.

Production evidence:
- `harness/mod.rs: HarnessAdapter` — the contract, with `id`,
  `executable_candidates`, `start`, `resume`, `describe`, `message` and
  `interrupt`. The six verbs the map names map onto it: starting is
  `start`, resuming `resume`, messaging `message`, interrupting `interrupt`,
  observing is `describe().hooks` plus `describe().session_ids`, and
  describing is `describe` itself.
- `harness/mod.rs: adapter_for` — the registry. Total over
  `IntegrationKind::Harness`, `None` for everything else.
- `integrations/mod.rs: IntegrationId::executable_candidates` — **delegates to
  the adapter for every harness.** This is the production path that makes the
  phase's fixed requirement structural rather than aspirational: there is one
  place a harness's executable name lives, and it is the adapter.
- `session/select.rs: HarnessSelection::adapter` and
  `HarnessSelection::start_args` — the seam both start paths go through.
- `main.rs: launch_session` and `shell/mod.rs: start_session` — the two real
  session producers, both using `start_args`.
- `integrations/mod.rs: doctor_report` / `write_adapter_report` — `glasshouse
  doctor` prints every adapter's declarations. This is what keeps `describe`
  from being a data structure nothing reads; it is generic over the trait and
  cannot tell one harness from another.

Regression evidence (macOS, and Linux/Windows in CI — all are pure logic):
- `every_harness_has_an_adapter_and_nothing_else_does` — a harness added to
  the catalogue without an adapter fails here rather than at a user's launch.
- `the_catalogue_takes_harness_executable_names_from_the_adapter` — proves the
  delegation above; **mutation-checked** by restoring a hard-coded name to the
  catalogue, which fails it.
- `resume_shapes_match_the_installed_binaries` — pins all seven resume
  invocations, which genuinely differ (`--resume`, a `resume` subcommand,
  `--conversation`, `--session`). **Mutation-checked** by giving Codex Claude
  Code's flag.
- `the_executable_names_match_the_installed_binaries` — pins `agy` for
  Antigravity. **Mutation-checked** by removing it.
- `resume_passes_the_identifier_as_one_whole_argument` — an identifier is its
  own `argv` entry and always last, so a value starting with a dash cannot be
  re-read as a flag.
- `every_verified_declaration_cites_its_evidence` — a `Verified` declaration
  with no usable evidence string fails. This is the honesty rule made
  mechanical.
- `no_two_adapters_claim_the_same_executable_name` — two harnesses claiming
  one name would silently resolve as each other.
- `a_sessions_arguments_are_the_adapters_first_then_the_users` — the ordering
  rule, exercised against a test adapter that actually declares start
  arguments, since none of the seven real ones needs any today.
- `the_doctor_report_shows_each_adapters_declarations` — asserts the specific
  rows of one adapter's block, not a `contains` over the whole report.
  **Mutation-checked** by making the report loop print nothing.
- `the_doctor_report_describes_every_harness_adapter` — every adapter gets a
  block whether or not it is installed on the machine running the tests.

Failure/isolation evidence:
- `the_generic_pty_runtime_depends_on_no_adapter` and
  `the_session_model_depends_on_no_adapter` — scan the production source of
  `pty/mod.rs`, `pty/process.rs`, `session/runtime.rs` and `session/store.rs`
  for `HarnessAdapter`, `crate::harness` and `IntegrationId`. Comments are
  stripped first, because `session/store` *documents* that it stores an
  identifier's string form, which is the boundary working rather than
  breaking. **Mutation-checked** by adding a method returning an
  `IntegrationId` to `SessionRuntime`, which fails it.
- `the_dependency_scan_would_catch_a_violation` — the scanner's own
  non-vacuity check: it fires on code, and not on a doc comment or a test.
- `an_unverified_capability_is_not_treated_as_present` — `Declared::Unverified`
  reads as "cannot rely on it", never as present.

Platform/external evidence:
- Declarations derived on 2026-08-25 from binaries installed on this machine:
  Claude Code 2.1.245, Codex 0.149.0, Antigravity CLI 1.1.20, OpenCode
  1.18.22, Cursor CLI 2026.08.11-e8db854, Pi 0.73.1, Hermes Agent 0.15.1.
  Each adapter module names what was read.
- `glasshouse doctor` run from the built binary; its output is what surfaced
  two rendering defects (nested backticks, a source phrase that did not fit
  its sentence) that no unit test would have shown.

Missing evidence:
- `resume`, `message` and `interrupt` are declared and unit-proven but have no
  production caller yet: resuming is Phase 7/8's, messaging and interrupting
  are Phase 13/14's. Line 3 asks an adapter to *expose* the resume command,
  which it does; executing one is a later phase's line and is not claimed
  here.
- No adapter parses harness output yet, so line 12's guard currently protects
  a property nothing is pushing against. That is the point of installing it
  before Phase 7 rather than after.

### Phase 6 — Make each adapter declare which native communication-style mechanisms it supports and whether changing them requires a new or cleared native session

Contract: Given a harness with a native way to control how it talks to the
user, when Glasshouse needs to apply a response profile, the adapter names
that mechanism and says whether changing it costs the running session, while
never presenting an unverified guess as a mechanism.

State: PARTIALLY VERIFIED — **box deliberately unchecked.**

Production evidence:
- `harness/mod.rs: CommunicationStyle`, `StyleChange`, and
  `HarnessDescription::communication_style` — the declaration exists and every
  adapter fills it in.
- `harness/claude_code.rs` — declares output styles, supplied through the
  settings document `--settings` reads at startup, as `StyleChange::NewSession`.

Missing evidence:
- Six of seven adapters declare `Unverified`, because their installed binaries
  document no communication-style mechanism at all. Codex is the pointed case:
  the capability map names "Codex personalities" as an example, and Codex
  0.149.0's `--help` exposes none.
- `StyleChange::InPlace` has no instance. Claude Code's declaration is
  `NewSession` because the mechanism *Glasshouse can drive* — a settings
  document read once at startup — is fixed for the life of the process. A
  native in-session command may well exist; relying on one that has not been
  observed would be exactly the guess this phase's design forbids, and the
  conservative direction is also the safe one, since Phase 9K requires warning
  before a profile change that costs a warm session.
- Closing this needs one verified in-place mechanism, or a second harness with
  a verified native mechanism of any kind.

### Phase 2B — Detect Antigravity when a supported Antigravity CLI executable is present

Contract: Given a machine with the Antigravity CLI installed, when Glasshouse
runs discovery, it finds it and reports its version, while never resolving an
unrelated program that merely has a similar name.

State: COMPLETE.

Production evidence:
- `harness/antigravity.rs: Antigravity::executable_candidates` — `["agy",
  "antigravity"]`, reached through `IntegrationId::executable_candidates` by
  the existing `Discovery` pass.

Regression evidence:
- `the_executable_names_match_the_installed_binaries` — pins both names.
- `no_integration_is_searched_for_under_a_guessed_abbreviation` — replaces a
  test that asserted the *wrong* single name; keeps the hazard it guarded (no
  `ag`, which is the-silver-searcher on many machines).

Platform/external evidence:
- A real Antigravity CLI 1.1.20 was installed on this machine on 2026-08-25,
  and `glasshouse doctor` reported it as `[available]` at its Caskroom path
  with version `1.1.20` read by the normal `--version` probe.

Missing evidence:
- None for this line. Note that the *published package* links the binary onto
  `PATH` as `agy`; an install by another route may expose `antigravity`
  instead, which is why both names are searched.


### Phase 5 — native terminal embedding (complete, 8 of 8)

Contract: Given a live harness session, when Glasshouse draws it, the harness's
own interface appears as it drew it — colours, cursor, wrapping and control
sequences intact — and the harness's own commands, prompts and controls keep
working, while Glasshouse's chrome stays out of the way.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/session/runtime.rs`: each `LiveSession` owns a
  `vt100::Parser` fed by the reader thread; `answer_terminal_queries` replies to
  `ESC[6n`; `resize` moves the emulator grid and the child's pseudo-terminal
  together.
- `crates/glasshouse/src/shell/mod.rs`: `build_viewport_grid`, `cell_style` and
  `convert_color` — the single place vt100's colour model meets Ratatui's.
  The tick rebuilds the grid, answers terminal queries, and sizes the child from
  the viewport's inner rect rather than the outer terminal.
- `crates/glasshouse/src/shell/view.rs`: `GridView` draws the grid cell by cell.
  The border is dropped once a live grid exists, so the harness gets the whole
  area — the chrome is four rows and the harness's name is already in the title.

Regression evidence:
- `an_embedded_session_answers_the_cursor_position_query_itself` — a real
  harness asks, and receives exactly `ESC[1;1R`.
- `colours_bold_inverse_and_cursor_position_survive_the_conversion`,
  `line_wrapping_is_preserved_in_the_grid`, `a_hidden_cursor_is_not_shown`,
  `a_fresh_screen_converts_to_a_full_grid_of_blank_cells`.
- `the_viewport_border_is_dropped_once_a_live_grid_is_shown`,
  `the_viewport_does_not_panic_with_a_real_grid_at_absurd_sizes`,
  `a_cursor_outside_the_render_area_does_not_panic`.
- The cursor-query scanner is tested at every one of the five possible read
  splits, one byte at a time, on a near miss, and on `ESC ESC [ 6 n`.

Failure/isolation evidence — mutations, each observed to fail its target:
- Nothing answers the cursor query: the harness hangs for the full timeout.
- The reply uses vt100's zero-based cursor: emits `ESC[0;0R`, not a position.
- The scanner forgets a byte that begins a fresh match.
- Every colour converts to default; every modifier is dropped.

**Two findings worth recording.**

*The responder had no production caller.* It shipped in `a1fa6c0` called only
from its own test — exactly the standard applied to Phase 1 line 90 and to the
runtime boxes, missed by the orchestrator who wrote it and caught by the worker
implementing the rendering. An embedded harness sending `ESC[6n` at startup
would have hung in the real shell while every test passed. It now runs on the
tick.

*The viewport's clipping clamp is not observable.* Removing
`area.height.min(grid.rows())` changes no rendered frame: `Buffer::cell_mut`
refuses anything outside the buffer, and the chrome below the viewport is drawn
after it. A containment test written to catch this passed for the wrong reason —
render order, not clipping — and was deleted rather than kept. The clamp stays,
with a comment saying plainly that it is cheap insurance rather than the thing
keeping the frame intact, because the render order it currently relies on is not
a property the widget can see.

Platform/external evidence:
- CI `32830685235` on `79a0600` — green on Linux, macOS, Windows and lint, with
  the Windows job confirmed to have executed 321 lib and 33 PTY tests including
  the colour, wrapping and border tests by name.

Missing evidence:
- Fidelity is asserted against synthetic escape sequences, not against Claude
  Code's or Codex's real TUI. The stated bar is "usable", which only a real
  harness can settle. `vt100` was chosen partly because swapping to
  `alacritty_terminal` is a bounded change if it proves insufficient.

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
  now would ship dead controls — see `GLASSHOUSE_DESIGN_DECISIONS.md`.
- The settings view has no end-to-end test through the shipped binary. The same
  differential-repaint limit that blocked the multi-session test applies.

### Phase 5 — the input half of native terminal embedding

Lines: "Preserve native harness input behavior instead of replacing it with a
Glasshouse chat composer"; "Allow native slash commands to pass directly to the
underlying harness"; "Add an escape key sequence that temporarily captures input
for Glasshouse-level navigation without permanently stealing input from the
harness".

Contract: Given a session on screen, when the user types, every keystroke
reaches the harness as the bytes its own interface expects — including the keys
Glasshouse binds for itself — while one reserved chord borrows input for
Glasshouse and hands it straight back.

State: COMPLETE

These three are satisfied by the session-mode design (see
`GLASSHOUSE_DESIGN_DECISIONS.md`) rather than by new work, and are checked here
as reconciliation. The rest of Phase 5 is the *rendering* half and needs a
terminal emulator.

Production evidence:
- `crates/glasshouse/src/shell/state.rs: handle_key`, `encode` — in session
  mode the mode is consulted before any binding, and every key is encoded to
  the bytes a terminal would send. Glasshouse has no composer, no input buffer,
  and no interpretation of `/`.
- `crates/glasshouse/src/shell/state.rs: is_session_escape` — one chord,
  `Ctrl-]`, both platform spellings.

Regression evidence:
- `a_slash_command_passes_straight_through_to_the_harness` — every character of
  `/compact` forwarded verbatim.
- `keys_glasshouse_binds_elsewhere_belong_to_the_harness_in_session_mode` —
  `q`, `n`, `o`, `i`, Tab, Esc, Enter, Backspace and Up all reach the harness.
- `the_escape_captures_input_only_until_it_is_handed_back` — control mode's
  bindings work again, then input returns to the harness, with no session
  touched. "Temporarily" and "without permanently stealing", asserted.
- `the_shell_enters_and_leaves_session_mode_in_a_real_terminal` — the same
  round trip through the shipped binary on all three platforms.

Failure/isolation evidence:
- Mutation: consulting bindings before the mode makes `q` quit instead of
  reaching the harness.
- Mutation: accepting only one spelling of the escape chord fails the
  real-terminal test — the defect that shipped past a full unit-test suite.

Platform/external evidence:
- CI `32821964808` on `f77b9c8` — Linux, macOS, Windows and lint.

Missing evidence:
- The rendering half of Phase 5 is untouched. The viewport prints raw bytes, so
  escape sequences are shown rather than obeyed. Until an emulator exists,
  "native permission prompts remain interactive" and the colour/cursor/wrapping
  line stay unchecked — a prompt the user cannot read is not interactive, even
  if the keystrokes would reach it.

### Phase 3 — return from overlays to the active native session, and propagate resize to it

Lines: "Allow the user to return from Glasshouse overlays to the active native
session without terminating it" and "Preserve terminal resize events and
propagate the new dimensions to the active embedded terminal".

Contract: Given an overlay open over a live harness session, when the user
leaves it, they are returned to that same session still running, and a resize
of Glasshouse's window reaches that session's own terminal.

State: COMPLETE

Both lines were blocked until this session, for the same reason: there was no
live native session to return to or to resize. `session::runtime` supplies one
and `shell::run` drives it.

Production evidence:
- `crates/glasshouse/src/shell/state.rs: close_overlay`, `enter_session_mode` —
  leaving an overlay restores the previous mode; entering session mode closes
  any open overlay. Neither touches a process.
- `crates/glasshouse/src/shell/mod.rs: run` — `Event::Resize` calls
  `screen.on_resize` and `SessionRuntime::resize` for the focused session.

Regression evidence:
- `leaving_an_overlay_returns_to_the_active_session_without_ending_it`
- `entering_session_mode_closes_any_open_overlay`
- `entering_and_leaving_session_mode_never_touches_a_real_process` — spawns a
  real child and checks its pid and liveness across the switch.
- `resizing_the_shell_reaches_the_harness_terminal` (Unix, tests/pty_smoke.rs)
  — the harness is asked `stty size` before and after Glasshouse's own terminal
  is resized, through the shipped binary.

Failure/isolation evidence:
- Mutation: making Escape always quit fails the overlay-first test.
- The resize test initially failed for two different reasons, both instructive:
  first because it asked before the SIGWINCH had travelled (a test timing
  fault, not a defect), and then because the escape chord never matched — see
  the Phase 4 entry.

Platform/external evidence:
- CI `32821964808` on `f77b9c8` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 287 lib and 33 PTY tests,
  including the shell's mode machinery in a real terminal. The resize test is Unix-only: `stty` is the
  portable way for a shell harness to report its terminal size and Windows has
  no equivalent a `.cmd` harness can run. The underlying `PtyProcess::resize`
  is covered on all three platforms by `resize_reaches_the_operating_system`
  and `a_resize_is_visible_to_the_child_process`.

Missing evidence:
- none.

### Phase 4 — the multi-session PTY runtime (covers seven map lines)

Lines: stream PTY output into an in-memory buffer; send text programmatically
without focus; send interrupt signals; bounded scrollback per live session;
keep inactive sessions running; switching changes only presentation focus;
headless presentation mode.

Contract: Given several harnesses started in one Glasshouse, when any of them
is acted on, it runs, buffers its own output within a fixed bound, and accepts
text or an interrupt whether or not it is the one on screen, while changing
which session is on screen never starts, stops, or signals any process.

State: COMPLETE for six of the seven lines; see Missing evidence.

Production evidence:
- `crates/glasshouse/src/shell/mod.rs: run` — **the production consumer.** The
  shell owns a `SessionRuntime`, starts sessions with `n`, forwards keystrokes
  in session mode, forwards resize events to the focused session, polls exits
  on every tick, and renders the focused session's scrollback in the viewport.
- `crates/glasshouse/src/session/runtime.rs: SessionRuntime`, `LiveSession`,
  `Scrollback` — each session gets its own reader thread and its own bounded
  buffer; focus is a field, and `focus()` touches nothing else.
- **No production caller yet.** `glasshouse launch` still uses
  `session::attach`, which is correct for handing one harness the whole
  terminal, and the shell reads records rather than live sessions. Until the
  shell drives this runtime, these seven boxes stay unchecked — the same
  standard applied to Phase 1 line 90 and to three Phase 3 lines.

Regression evidence (all in tests/pty_smoke.rs, real processes, real pseudo-
terminals, written by a Sonnet worker in an isolated worktree and re-verified
here):
- `two_sessions_run_concurrently_with_independent_scrollback`
- `an_unfocused_session_still_receives_sent_text`
- `focus_changes_nothing_but_focus` — pids recorded before and after five
  focus changes.
- `a_headless_session_runs_but_cannot_be_focused`
- `exit_is_detected_with_no_output_at_all`
- `scrollback_stays_bounded_under_real_output`
- `closing_one_session_leaves_the_others_running`
- `keystrokes_reach_the_focused_session`
- Plus unit tests for `Scrollback`: eviction order, an oversized chunk keeping
  its tail, a severed multi-byte character dropped rather than mangled, escape
  sequences preserved, zero capacity.

Failure/isolation evidence — mutations run by the orchestrator, each observed
to fail the test it targets: letting a headless session be focused; requiring
focus before `send_text`; making the scrollback unbounded; making `close` kill
every session; removing focus recovery after a close; giving all sessions one
shared scrollback; making `poll_exits` report nothing.

Two mutations did **not** fail, and both were acted on:

- Mutating `close` to kill every session initially left
  `closing_one_session_leaves_the_others_running` green, because `is_running()`
  reads the status cached by the last `poll_exits` — a freshly killed survivor
  still reported itself running. The test now polls the operating system over a
  window instead, and the mutation fails.
- Mutating `poll_exits` to wait for end-of-file before asking the process left
  `exit_is_detected_with_no_output_at_all` green. A harness that prints nothing
  and exits has its output end at the same instant, so that test cannot tell
  "asks the process" from "waits for output, then asks". An attempt to build
  the discriminating case — a harness leaving a background child holding the
  pseudo-terminal open past its own exit — was written and then removed,
  because a direct probe showed macOS reports end-of-file on the master as soon
  as the foreground child exits regardless of the background holder. The
  capability's real risk, mistaking a silent-but-running harness for a finished
  one, is covered by `exit_is_detected_from_the_process_not_from_quiet_output`.

Platform/external evidence:
- CI `32819167010` on `bb4c383` — green on Linux, macOS, Windows and lint, with
  the Windows job confirmed to have executed 267 lib and 31 PTY tests including
  every multi-session test by name. Several concurrent ConPTY sessions with
  independent scrollbacks work, which was the platform risk worth checking.

End-to-end evidence through the shipped binary (tests/pty_smoke.rs):
- `a_keystroke_typed_into_the_shell_reaches_a_real_harness_and_comes_back` —
  `glasshouse` with no arguments, `n` starts a real harness, session mode hands
  it the keyboard, the typed bytes arrive and its reply is drained into the
  scrollback and drawn. The payload begins with `q` on purpose: in session mode
  that belongs to the harness, so a broken mode split would quit instead.
  Mutations caught: swallowing the bytes before `write_to_focused`; never
  refreshing the viewport from the scrollback.
- `resizing_the_shell_reaches_the_harness_terminal` (Unix) — asks the harness
  `stty size`, resizes Glasshouse's own terminal, asks again. Proves the chain
  from Crossterm's resize event to the child's pseudo-terminal is joined up,
  which nothing previously did.

**A real defect this found, that unit tests could not.** The session-mode
escape chord was implemented as `Ctrl` + `']'`, matching the synthetic
`KeyEvent` its unit tests constructed. Crossterm's Unix parser decodes the
control range `0x1C..=0x1F` arithmetically, so a real terminal's `Ctrl-]`
arrives as `Ctrl` + `'5'` and never matched — leaving the user in session mode
**with no way back**, which is precisely the failure the single-chord escape
exists to prevent. `is_session_escape` now accepts both spellings, with the
Windows path (virtual key codes, `']'`) and the Unix path (`'5'`) documented
and separately tested. Reverting to the single spelling fails the resize test.

Missing evidence:
- Three of the twelve Phase 4 lines stay unchecked, all for the same reason —
  no production caller: sending text to an **unfocused** session and sending an
  **interrupt** are both orchestrator operations (Phase 14), and nothing yet
  creates a **headless** session, because the shell always starts sessions
  embedded. The runtime supports all three and each is tested against real
  processes; what is missing is a caller, not a mechanism.
- The shell's own multi-session switching has no end-to-end test. One was
  written and removed: a full-screen Ratatui application repaints
  differentially, so a captured pseudo-terminal stream cannot be sliced back
  into frames without a real terminal emulator, and an assertion about "the
  viewport" silently reads every viewport ever drawn. Phase 5 needs an emulator
  anyway; that is when this becomes testable. Meanwhile the behaviour is proven
  at the runtime layer against real processes
  (`focus_changes_nothing_but_focus` records pids across five switches), and
  the shell's only route to switching is through that layer.

### Phase 4 — Implement a generic PTY-backed child-process abstraction for interactive harnesses

Contract: Given any interactive harness, when Glasshouse runs it, it does so
through one pseudo-terminal abstraction that hides the platform difference
between Unix PTYs and Windows ConPTY, while exposing spawn, input, output,
resize, signal, and exit uniformly.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/pty/mod.rs: PtyProcess`, `TerminalCommand`,
  `PtyOutput`, `ExitStatus` — the only route by which Glasshouse starts a
  child, reached in production from `session::attach` (via `glasshouse launch`)
  and from `integrations`' version probes.

Regression evidence:
- `streams_output_and_reports_a_successful_exit`, `reports_a_failing_exit_code`,
  `forwards_input_to_an_interactive_child`,
  `terminating_stops_a_long_running_process`,
  `dropping_a_running_process_kills_it`,
  `terminate_reaches_the_session_leader_under_job_control` — all in
  tests/pty_smoke.rs, all against real processes in real pseudo-terminals.
- `the_launch_command_opens_the_configured_harness_inside_the_project_root` —
  the shipped binary end to end.

Failure/isolation evidence:
- `signalling_an_exited_process_is_reported_rather_than_misdirected` and its
  unpolled variant — a signal is never sent to a reused process identifier.
- `dropping_reaps_a_child_that_already_exited` — no zombies.
- `pty::open_pty` retries allocation five times, side-effect free, after the
  macOS `openpty(3)` race was diagnosed.

Platform/external evidence:
- CI on every batch this session — Linux, macOS and Windows, with the Windows
  job confirmed to have executed the PTY suite.

Missing evidence:
- none. Verified by reading the code and the named tests rather than taken from
  a worker's inventory, which had marked two neighbouring lines satisfied on
  the strength of production paths their named tests did not actually cover.

### Phase 4 — Detect process exit independently from textual terminal output

Contract: Given a harness that produces no output at all, when it ends,
Glasshouse notices from the process itself, while a harness that is merely
silent is never mistaken for one that has finished.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/pty/mod.rs: PtyProcess::try_wait` / `wait`, which read
  the child's status and cache it so it is reaped exactly once.
- `crates/glasshouse/src/session/attach.rs: supervise` polls `try_wait`, never
  output quiet, to decide a session is over.

Regression evidence:
- `exit_is_detected_from_the_process_not_from_quiet_output` — a child that
  prints nothing and lingers: `try_wait` reports it still running, then reports
  its exit. Silence and completion are proven distinguishable.
- `the_launch_command_opens_the_configured_harness_inside_the_project_root`
  asserts a distinctive exit code (7, so neither generic success nor generic
  failure) survives to the shipped binary's own exit code.

Failure/isolation evidence:
- `signalling_an_unpolled_but_exited_process_is_reported_rather_than_misdirected`
  — exit detection and signalling agree about a process that ended without
  anyone having polled it.

Platform/external evidence:
- CI on every batch this session — Linux, macOS and Windows.

Missing evidence:
- none.

### Phase 3 — Build the main interactive interface with Ratatui and Crossterm

Contract: Given a terminal on both standard input and standard output, when
`glasshouse` is run with no arguments, it opens a full-screen interface that
answers the keyboard and restores the terminal on the way out, while a piped or
redirected run falls back to the plain summary rather than drawing into a file.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/shell/mod.rs: run` — the event loop, entered from
  `main.rs`'s no-argument arm behind an `IsTerminal` check on both streams.
- `crates/glasshouse/src/tui/mod.rs: Screen` — terminal ownership, restored by
  `TerminalGuard` on a normal return, an error, a panic, or a signal.

Regression evidence:
- `the_shell_opens_in_a_real_terminal_and_answers_the_keyboard`
  (tests/pty_smoke.rs) — the shipped binary in a real pseudo-terminal: the
  interface draws, `o` opens the overview, Escape leaves it, `q` exits cleanly,
  and the alternate screen is left behind.
- The whole `shell::state` and `shell::view` suite, driven without a terminal.

Failure/isolation evidence:
- Mutation: making the no-argument arm fall through to the summary instead of
  the shell fails the pty_smoke test.
- The test asserts the alternate screen was left, so an exit that stranded the
  user on a dead frame would fail.

Platform/external evidence:
- CI `32816717226` on `5da067a` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 257 lib and 23 PTY tests,
  including the real-terminal shell test, rather than merely reporting green.

Missing evidence:
- none.

### Phase 3 — Create a persistent top bar that shows the project name, project root, and active session

Contract: Given any shell screen, when a frame is drawn, the project name, the
active canonical project root, and the session currently presented are all on
it, while a terminal too narrow for the root keeps the tail that identifies the
project rather than the head.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/shell/view.rs: render_title`, `render_root`.

Regression evidence:
- `the_project_root_is_displayed_on_every_frame`
- `the_project_root_stays_visible_while_an_overlay_is_open`
- `a_narrow_terminal_keeps_the_end_of_the_project_root` and
  `a_wide_terminal_shows_the_whole_project_root` — asserted against the root's
  own row, not the whole frame.
- `the_shell_opens_in_a_real_terminal_and_answers_the_keyboard` — the root is
  checked in the real terminal's output, anchored to the `root ` field.

Failure/isolation evidence:
- Mutation: blanking the root fails both the unit test and the pty_smoke test.
  The first version of the pty_smoke assertion survived this mutation, because
  the project's name and its root's last component are the same string and a
  bare `contains` matched the title bar; it now anchors on the field.
- Mutation: truncating the root from the end instead of the start fails
  `a_narrow_terminal_keeps_the_end_of_the_project_root`. That test also
  initially survived, for the same reason, and now reads a single row.

Platform/external evidence:
- CI `32816717226` on `5da067a` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 257 lib and 23 PTY tests,
  including the real-terminal shell test, rather than merely reporting green.

Missing evidence:
- none.

### Phase 3 — Create a persistent session bar that lists currently known sessions

Contract: Given the project's recorded sessions, when a frame is drawn, every
one of them appears in the bar with the active one distinguished, while a
project with no sessions says so instead of showing an empty strip.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/shell/view.rs: render_session_bar`, over the records
  `shell::run` reads from `session::store`.

Regression evidence:
- `the_session_bar_lists_every_known_session`
- `an_empty_project_says_so_instead_of_showing_an_empty_bar`
- `the_shell_opens_in_a_real_terminal_and_answers_the_keyboard` starts two real
  sessions first, so the bar is drawn from records a real launch wrote.

Failure/isolation evidence:
- Mutation: dropping the per-session span fails the listing test.

Platform/external evidence:
- CI `32816717226` on `5da067a` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 257 lib and 23 PTY tests,
  including the real-terminal shell test, rather than merely reporting green.

Missing evidence:
- none.

### Phase 3 — Create a central viewport reserved for the active session terminal

Contract: Given a shell screen, when a frame is drawn, the central region is
reserved for the active session's terminal and describes what will occupy it,
while never drawing a convincing empty terminal for a session that is not
attached.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/shell/view.rs: render_viewport` — a bordered region
  sized by the layout solver, holding the active session's identity and an
  explicit note that the space is reserved.

Regression evidence:
- `an_empty_project_says_so_instead_of_showing_an_empty_bar` — with no session
  the viewport says so rather than looking like an idle terminal.
- `renders_without_panicking_at_absurd_sizes` — 1x1 through 200x60, with and
  without an overlay.

Failure/isolation evidence:
- Nothing here computes a size by subtraction, which is the usual way a
  "must not panic on a tiny terminal" claim fails; the 1x1 cases prove it.

Platform/external evidence:
- CI `32816717226` on `5da067a` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 257 lib and 23 PTY tests,
  including the real-terminal shell test, rather than merely reporting green.

Missing evidence:
- The viewport is reserved, not filled. Embedding a live harness terminal is
  Phase 5, and this deliberately does not fake it.

### Phase 3 — Create a compact bottom status bar for Glasshouse-level key bindings and status messages

Contract: Given any shell screen, when a frame is drawn, one row carries
Glasshouse's own key bindings for that screen, and a key that could not do
anything leaves a note beside them, while the bindings survive a terminal too
narrow for both.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/shell/view.rs: render_footer`
- `crates/glasshouse/src/shell/state.rs: ShellState::set_status`, set when
  navigation has nowhere to go and cleared by the next keystroke.

Regression evidence:
- `the_status_bar_always_shows_the_key_bindings` — including the overlay's
  different bindings.
- `the_status_bar_shows_a_note_next_to_the_bindings`
- `a_note_is_dropped_rather_than_crowding_out_the_bindings`
- `a_status_note_is_cleared_by_the_next_keystroke`

Failure/isolation evidence:
- Mutation: dropping the hint span fails the bindings test.
- Mutation: removing the status message fails the navigation test.
- Mutation: writing the note before the bindings fails the narrow-terminal
  test, which is the entire mechanism — the row clips on the right, so order
  decides what is lost.

Platform/external evidence:
- CI `32816717226` on `5da067a` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 257 lib and 23 PTY tests,
  including the real-terminal shell test, rather than merely reporting green.

Missing evidence:
- none.

### Phase 3 — Allow the user to move to the previous / next session with a keyboard shortcut

Contract: Given a project with several sessions, when the user presses Tab or
Shift-Tab (or Right/Left), the shell presents the next or previous session,
wrapping at either end, while changing nothing about any session itself.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/shell/state.rs: ShellState::next_session`,
  `previous_session`, reached from `handle_key`.

Regression evidence:
- `tab_moves_to_the_next_session_and_wraps`
- `shift_tab_moves_to_the_previous_session_and_wraps`
- `arrow_keys_navigate_the_same_way_as_tab`
- `navigating_changes_only_which_session_is_presented` — the session list is
  compared before and after, so navigation cannot be quietly mutating records.
- `navigating_with_fewer_than_two_sessions_explains_itself`
- `a_refresh_keeps_the_same_session_presented_even_when_the_order_changes` —
  the selection follows the session's identifier, not its index, so a
  background refresh cannot move the user to a different session.

Failure/isolation evidence:
- `an_empty_project_has_no_active_session_and_does_not_panic`
- Mutation: removing the no-op status message fails the explanatory test.

Platform/external evidence:
- CI `32816717226` on `5da067a` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 257 lib and 23 PTY tests,
  including the real-terminal shell test, rather than merely reporting green.

Missing evidence:
- Bindings are plain single keys because no native session owns the keyboard
  yet. When one does (Phase 5) they must move behind a prefix or a mode, or
  they will steal keystrokes the harness needs. `handle_key` is deliberately
  the only place that has to change.

### Phase 3 — Allow the user to open a session overview from the keyboard

Contract: Given any shell screen, when the user presses `o`, an overview opens
showing every session with the detail the bar has no room for, while the shell
stays visible around it and the active session is unchanged.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/shell/state.rs: ShellState::open_overview`
- `crates/glasshouse/src/shell/view.rs: render_overview` — drawn over the
  shell, not in place of it.

Regression evidence:
- `o_opens_the_session_overview_from_the_keyboard`
- `the_overview_shows_detail_the_session_bar_has_no_room_for`
- `leaving_an_overlay_returns_to_the_active_session_without_ending_it`
- `escape_leaves_an_overlay_first_and_only_then_leaves_glasshouse`
- `the_shell_opens_in_a_real_terminal_and_answers_the_keyboard` opens it with a
  real keystroke in a real terminal.

Failure/isolation evidence:
- Mutation: making Escape always quit fails the overlay-first test, which is
  what stops Escape closing Glasshouse from inside the overview.

Platform/external evidence:
- CI `32816717226` on `5da067a` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 257 lib and 23 PTY tests,
  including the real-terminal shell test, rather than merely reporting green.

Missing evidence:
- none.

### Phase 3 — Keep the visual design text-first and avoid decorative graph visualizations that do not expose actionable state

Contract: Given any shell screen, when a frame is drawn, it contains only text
and the box-drawing characters that frame it, and never a gauge, sparkline, or
bar chart.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/shell/view.rs` uses only `Paragraph`, `Block`, and
  `Clear`. No Ratatui chart, gauge, sparkline, or canvas widget is imported.

Regression evidence:
- `nothing_draws_with_block_elements_so_the_design_stays_text_first` — renders
  the shell, the overview, and a screen carrying a status note, and fails on
  any character in U+2580..U+259F. Ratatui's decorative widgets are all drawn
  from that block-element range, so a frame containing none of it cannot be
  rendering one. Border characters live in a different range and stay allowed.

Failure/isolation evidence:
- Mutation: adding a `load ▇▇▅▂▁` line to the viewport fails the test, so it
  is a real check rather than a restatement of intent.

Platform/external evidence:
- CI `32816717226` on `5da067a` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 257 lib and 23 PTY tests,
  including the real-terminal shell test, rather than merely reporting green.

Missing evidence:
- Mechanical rather than aesthetic: the test proves no block-element widget is
  drawn, not that the layout is well judged.

### Phase 1 — Display the active canonical project root prominently in the TUI

Contract: Given the interactive shell, when any frame is drawn, the active
canonical project root is on its own labelled row, on every screen including
behind an overlay, while a narrow terminal drops the head of the path and keeps
the tail that identifies the project.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/shell/view.rs: render_root` — a dedicated row, not a
  corner. The value comes from `Project::display_root`, the same canonical root
  every access-control decision uses.

Regression evidence:
- `the_project_root_is_displayed_on_every_frame`
- `the_project_root_stays_visible_while_an_overlay_is_open` — "prominently"
  cannot mean "until you open something".
- `a_narrow_terminal_keeps_the_end_of_the_project_root`
- `a_wide_terminal_shows_the_whole_project_root`
- `the_shell_opens_in_a_real_terminal_and_answers_the_keyboard` — proved in a
  real terminal, anchored to the `root ` field rather than to any word that
  also appears in the title bar.

Failure/isolation evidence:
- Mutation: blanking the root fails the unit test and the real-terminal test.
- Mutation: truncating from the wrong end fails the narrow-terminal test.
- Both assertions were vacuous in their first form and were tightened after the
  mutations exposed them.

Platform/external evidence:
- CI `32816717226` on `5da067a` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 257 lib and 23 PTY tests,
  including the real-terminal shell test, rather than merely reporting green.

Missing evidence:
- none.

### Phase 2 — Persist Glasshouse session metadata independently from the native harness session files

Contract: Given a harness session started by Glasshouse, when the session is
recorded, Glasshouse stores its own session metadata in the project database
and can read it back in a later process, while never parsing, depending on, or
being invalidated by whatever session files the harness keeps for itself.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/session/store.rs: SessionStore::create` — the only
  writer of the `sessions` table; reached from `main.rs: launch_session`, which
  records a session before the harness process exists.
- `crates/glasshouse/src/main.rs: session_report` — `glasshouse sessions` reads
  the records back in a separate process.
- `crates/glasshouse/src/database.rs: MIGRATIONS[1]` — the `sessions` table.
  `native_session_id` is nullable, so a record is complete before any harness
  has produced an identifier and stays valid after the harness's own history is
  deleted.

Regression evidence:
- `launching_a_harness_records_a_session_that_a_later_command_reads_back`
  (tests/pty_smoke.rs) — the shipped binary, a real pseudo-terminal, two real
  harness runs, then a second process reading the records. Executed on macOS
  locally and on Linux, macOS and Windows in CI.
- `a_session_is_recorded_and_survives_a_reopen_with_no_harness_involved` —
  the record is complete with no harness identifier and survives a reopen.

Failure/isolation evidence:
- Mutation: making `create` skip its `INSERT` fails the pty_smoke test.
- Mutation: dropping the post-exit `note_lifecycle` call fails it.
- `a_session_write_is_refused_when_the_project_binding_is_missing` — writes are
  refused rather than orphaned when the database has no project bound.

Platform/external evidence:
- CI `32815286487` on `3d606e3` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 228 lib and 22 PTY tests
  (a green tick is not proof the suite ran: when the lib target fails,
  cargo never reaches the integration tests).

Missing evidence:
- none.

### Phase 2 — Persist a mapping between Glasshouse session IDs and native harness session IDs when native IDs are available

Contract: Given a harness that reveals its own session identifier, when
Glasshouse records it, the identifier is stored against exactly one Glasshouse
session and can be read back, while a Glasshouse session identifier never
changes and no native session can be claimed by two Glasshouse sessions.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/session/store.rs: SessionStore::set_native_session_id`
  — attaches the identifier after creation, which is when harnesses reveal it.
- `crates/glasshouse/src/database.rs: MIGRATIONS[1]` — the partial unique index
  `sessions_native_id` over `(harness, native_session_id)` is what makes the
  column a mapping rather than an annotation.

Regression evidence:
- `a_native_session_identifier_can_be_attached_later_and_read_back`
- `one_native_session_cannot_map_to_two_glasshouse_sessions`
- `two_harnesses_may_use_the_same_native_identifier`
- `many_sessions_may_have_no_native_identifier_at_once`

Failure/isolation evidence:
- Mutation: dropping the unique index lets one native session be claimed twice.
- Mutation: narrowing the index to `(native_session_id)` alone makes two
  harnesses collide.
- Mutation: replacing `NULL` with an empty-string sentinel makes every
  unidentified session collide — the reason the column stays nullable.

Platform/external evidence:
- CI `32815286487` on `3d606e3` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 228 lib and 22 PTY tests
  (a green tick is not proof the suite ran: when the lib target fails,
  cargo never reaches the integration tests).

Missing evidence:
- No harness adapter captures a native identifier yet (Phase 7/8), so in
  production the column is currently always `NULL`. The mapping mechanism is
  complete and proven; what feeds it is a later phase.

### Phase 2 — Persist the harness type, creation time, last activity time, role, lifecycle state, and project identifier for every session

Contract: Given any recorded session, when it is read back, every one of those
six fields is present and accurate, while creation time never changes and last
activity time advances on every state change and every recorded interaction.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/session/store.rs: SessionRecord`, `SessionStore::create`,
  `SessionStore::set_lifecycle`, `SessionStore::touch`.
- `crates/glasshouse/src/main.rs: launch_session` — moves a real session through
  `Starting` -> `Running` -> `Stopped`/`Failed`.

Regression evidence:
- `every_required_field_is_persisted` — asserted by value against an injected
  clock, not by round-trip.
- `every_role_and_lifecycle_value_round_trips`
- `activity_time_advances_while_creation_time_stays_put`
- `sessions_are_listed_most_recently_active_first`

Failure/isolation evidence:
- Mutation: stopping `set_lifecycle` from touching `last_activity_at` fails the
  activity test.
- Mutation: recording every ended session as `Stopped` fails the pty_smoke
  test, because a failed harness stops being distinguishable.
- `the_schema_rejects_enum_values_it_does_not_define` — `CHECK` constraints
  reject a role, lifecycle, or presentation the schema does not define.
- `an_unrecognized_stored_enum_value_is_reported_rather_than_guessed` — a value
  written by a future build surfaces as a typed error naming the column, not a
  panic or a silent default.
- `touching_an_unknown_session_reports_it_missing_rather_than_inventing_one`

Platform/external evidence:
- CI `32815286487` on `3d606e3` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 228 lib and 22 PTY tests
  (a green tick is not proof the suite ran: when the lib target fails,
  cargo never reaches the integration tests).

Missing evidence:
- none.

### Phase 2 — Persist the process presentation mode for every session

Contract: Given a session presented embedded, headless, or externally, when it
is recorded and read back, its presentation mode is preserved exactly, while an
undefined presentation value cannot be stored at all.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/session/store.rs: SessionPresentation`, stored by
  `SessionStore::create` and shown by `main.rs: session_report`.
- Vocabulary is the map's own (Phase 10/11: "embedded, headless, or externally
  presented"), not invented here.

Regression evidence:
- `every_presentation_mode_is_persisted` — all three modes.
- `launching_a_harness_records_a_session_that_a_later_command_reads_back`
  asserts the presentation column reaches the listing.

Failure/isolation evidence:
- `the_schema_rejects_enum_values_it_does_not_define` covers `presentation`.
- `stored_values_honour_format_width_so_listings_align` — the `Display` impls
  use `Formatter::pad`, so the listing's columns cannot silently go ragged.

Platform/external evidence:
- CI `32815286487` on `3d606e3` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 228 lib and 22 PTY tests
  (a green tick is not proof the suite ran: when the lib target fails,
  cargo never reaches the integration tests).

Missing evidence:
- Only `Embedded` occurs in production today, because `glasshouse launch` is the
  only session producer. `Headless` and `External` arrive with Phase 4's
  headless mode and Phase 17's cmux panes.

### Phase 2 — Persist enough metadata to distinguish active, resumable, closed, and failed sessions

Contract: Given the stored metadata alone, when Glasshouse classifies a
session, it separates active, resumable, closed, and failed without consulting
any harness, while never reporting a session resumable when nothing was
recorded to resume it to.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/session/store.rs: SessionRecord::disposition` — derived
  from lifecycle plus the presence of a native identifier, deliberately not a
  second stored column that could disagree with the first.
- `crates/glasshouse/src/main.rs: session_report` — the STATE column of
  `glasshouse sessions`.

Regression evidence:
- `the_four_dispositions_are_distinguishable_from_stored_metadata` — all seven
  lifecycle states, with and without a native identifier.
- `launching_a_harness_records_a_session_that_a_later_command_reads_back` — a
  clean exit reads as `closed` and a failing one as `failed`, end to end.

Failure/isolation evidence:
- Mutation: treating a stopped session with no native identifier as resumable
  fails the disposition test.
- `a_stopped_session_with_no_native_identifier_is_not_resumable` and
  `a_live_session_is_not_resumable` — the refusals `open_for_resume` enforces.

Platform/external evidence:
- CI `32815286487` on `3d606e3` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 228 lib and 22 PTY tests
  (a green tick is not proof the suite ran: when the lib target fails,
  cargo never reaches the integration tests).

Missing evidence:
- none.

### Phase 2 — Never store provider credentials directly in the project memory database

Contract: Given the project database at any schema version this build produces,
when its full schema is enumerated, there is no column and no key/value slot in
which a provider credential could be stored, while any future schema change
that adds one fails the build's tests until it is deliberately reviewed.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/database.rs: MIGRATIONS` — the complete schema is three
  tables: `project_metadata`, `schema_migrations`, `sessions`. None has a
  credential column.
- `crates/glasshouse/src/session/store.rs: NewSession` — the only way to create
  a session, and it has no field a secret could be passed through.

Regression evidence:
- `the_project_database_schema_has_nowhere_to_put_a_credential` — asserts the
  exact `(table, column)` list. Deliberately an allowlist rather than a name
  pattern: `project_metadata.key` would false-positive on any name match, and a
  credential column could just as easily be called `value`. Any new column
  fails this test until someone updates the list, which is the moment to ask
  whether it can hold a secret.
- `project_metadata_holds_only_the_project_identifier` — the one key/value table
  is pinned to its single known key, closing the route by which a secret could
  be stored without a schema change.

Failure/isolation evidence:
- The test fails by construction on any schema addition; it is an exact
  equality, so it cannot pass vacuously.

Platform/external evidence:
- CI `32815286487` on `3d606e3` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 228 lib and 22 PTY tests
  (a green tick is not proof the suite ran: when the lib target fails,
  cargo never reaches the integration tests).

Missing evidence:
- Provider credentials do not exist yet (Phase 9E). This entry proves the
  project database is not where they can land; it does not yet prove where they
  do land.

### Phase 1 — Reject any attempt to resume a Glasshouse-managed session whose project identifier differs from the current project identifier

Contract: Given a session record whose project identifier differs from the
active project's, when anything attempts to resume it, Glasshouse refuses and
names both projects, while leaving the record untouched and while the database
itself refuses to store such a record in the first place.

State: COMPLETE.

Production evidence:
- `session/store.rs: SessionStore::open_for_resume` — compares the stored
  project identifier against the active one before returning anything
  actionable.
- `main.rs: resume_session` — **the production caller this entry waited three
  sessions for.** `glasshouse resume` resolves an identifier and then goes
  through `open_for_resume` *before* a harness is selected and long before any
  process exists, so a refusal costs nothing and cannot half-start a session.
- `database.rs: MIGRATIONS[1]` — `BEFORE INSERT` and `BEFORE UPDATE OF
  project_id` triggers abort any row whose `project_id` is not the identifier
  bound in `project_metadata`. Structural, so no present or future query has to
  remember to filter by project.

Regression evidence:
- `resuming_a_session_belonging_to_another_project_is_refused` — the error names
  both projects and the planted record is left byte-for-byte intact.
- `the_database_refuses_to_store_a_session_from_another_project`
- `a_stored_session_cannot_be_reassigned_to_another_project`
- `a_stopped_session_of_this_project_can_be_resumed` — the permitted case, so
  the refusals above are not merely "resume never works".
- `two_projects_have_independent_session_lists`
- `resuming_a_session_with_no_conversation_is_refused` (PTY smoke, Unix) — the
  shipped binary refuses a session that has nothing to resume to, and the
  harness is never started. This is the test that reaches `open_for_resume` on
  the production path; `resuming_an_unknown_session_is_refused` does not,
  because the identifier resolver turns it away first.
- `a_recorded_session_is_resumed_under_the_identifier_it_was_given` (PTY smoke,
  Unix) — the permitted case end to end through the shipped binary.

Failure/isolation evidence:
- Mutation: removing the project comparison in `open_for_resume` fails the
  cross-project test.
- Mutation: dropping the `BEFORE INSERT` trigger fails the structural test.
- Mutation: weakening the trigger's `IS NOT` to `<>` fails
  `a_session_write_is_refused_when_the_project_binding_is_missing`, which is
  what proves the guard fails closed rather than silently passing a NULL
  comparison.
- Mutation: making `resume_session` read the record directly instead of through
  `open_for_resume` fails `resuming_a_session_with_no_conversation_is_refused`.
  **This mutation initially passed**, which is how it was discovered that the
  unknown-identifier test proved nothing about the guard — the resolver refuses
  first. The test above was written specifically to reach it.

Platform/external evidence:
- CI `32815286487` on `3d606e3` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 228 lib and 22 PTY tests
  (a green tick is not proof the suite ran: when the lib target fails,
  cargo never reaches the integration tests).

Missing evidence:
- None. Note that the cross-project case cannot be reached end to end through
  the binary, because the migration triggers refuse to store such a row in the
  first place — reaching it at all requires the test to plant one by tampering,
  which `resuming_a_session_belonging_to_another_project_is_refused` does. That
  is the guard being defence in depth, not a gap: the structural refusal is the
  first line and the comparison is the second.

### Phase 1 — Ensure every spawned harness process starts with its working directory set to the current project root

Contract: Whenever Glasshouse invokes an installed harness—including discovery
probes and interactive sessions—the child starts in the active canonical
project root and never inherits an unrelated caller directory.

State: COMPLETE

Production evidence:

- `main::launch_session` is the production consumer: `glasshouse launch
  [harness]` resolves a harness through `session::select::select` and starts it
  through `launch::HarnessLaunch`, which is the only route that reaches PTY
  spawn for a harness. This closes the gap recorded in the previous revision of
  this entry, where `HarnessLaunch` had no production caller at all.
- `session::attach::attach` runs the resulting session against the real
  terminal: raw mode, input/output pumps, resize forwarding, exit propagation,
  and restoration on every exit path.
- `launch::HarnessLaunch::spawn` reaches PTY spawn only through
  `TerminalCommand::for_harness`, which derives the directory from
  `Project::display_root` and is `pub(crate)`.
- `integrations::Discovery::run(&Project)` threads the active project into
  `version::probe_version`, which sets `Command::current_dir` from
  `Project::display_root`.

Regression evidence:

- `the_launch_command_opens_the_configured_harness_inside_the_project_root`
  (pty_smoke, macOS) runs the *shipped binary* in a real pseudo-terminal and
  matches the harness's own report of its working directory against the project
  root by filesystem identity. Glasshouse itself is deliberately run from a
  different directory, so an inherited cwd cannot pass.
- `a_fake_installed_harness_launches_inside_the_discovered_project_root`
  proves the same for the `HarnessLaunch` mechanism directly.
- `version_probe_child_starts_in_the_active_project_root` uses a resolved fake
  probe that prints a version only in the correct child directory.
- `project_configured_executable_wins_over_user_level` and the rest of
  `session::select::tests` pin executable precedence and every refusal path.
- Windows-only tests pin verbatim drive and UNC prefix conversion.

Failure/isolation evidence:

- The end-to-end test installs a *decoy* executable at the user level and the
  real one at the project level; a precedence failure runs the decoy and fails
  the test loudly rather than silently passing.
- `a_failing_configured_executable_never_falls_back_to_path` proves a broken
  configured path is an error, not a silent substitution of another binary.
- `attaching_without_a_terminal_is_refused` fails closed rather than hanging on
  a pty query nothing can answer.
- `PtyProcess::spawn` refuses a working directory that does not exist instead
  of starting the child somewhere else.
- Unsafe Windows-script arguments are rejected before `HarnessLaunch` spawns.

Non-vacuity (mutations actually run, each observed to fail the test):

- Removing the project layer from `EffectiveConfig::executable` → the decoy
  runs; the end-to-end test fails on precedence.
- Making `TerminalCommand::for_harness` use the process cwd instead of the
  project → no harness reports the project root; the test fails.
- Making `exit_code_for` always return success → the test fails on exit-code
  propagation.
- Setting `PTY_ALLOCATION_ATTEMPTS` to 1 → the pty retry test fails.

Platform/external evidence:

- **CI run `32788123095` on commit `f3effe6` is green on `ubuntu-latest`,
  `macos-latest`, and `windows-latest`, plus lint.** The Windows job was
  confirmed to have *actually executed* the tests cited here — 186 lib tests
  and 20 PTY smoke tests — rather than only reporting a green tick;
  `the_launch_command_opens_the_configured_harness_inside_the_project_root`
  and `a_fake_installed_harness_launches_inside_the_discovered_project_root`
  both appear as `ok` in the Windows log.
- Local macOS: `cargo fmt --check`, `cargo clippy -D warnings`, unit and PTY
  smoke tests, MSRV 1.85.0 `cargo check --locked`, `git diff --check`, and
  live CLI probes of `glasshouse launch` (help, no-terminal refusal, unknown
  harness, non-harness integration).
- An independent spawn-site inventory, re-run after the merge, found three
  production spawn sites, all project-bound, and zero production callers of
  the generic `TerminalCommand::new`.

What CI caught that local evidence had not:

- `cmd.exe` cannot open the verbatim `\\?\` path that resolving an executable
  produces on Windows, so **no `.cmd`-shimmed harness could start there at
  all** — and npm installs most of them that way. Fixed in `4aa31ad`.
- A prior revision of this entry cited a Windows job that had never run
  `tests/pty_smoke.rs`: when the lib target fails, cargo never reaches the
  integration tests, so the `.cmd` and verbatim-path claims were unproven
  while the entry implied otherwise. Confirming *execution*, not just the
  job's conclusion, is part of this evidence and not a formality.

Remaining caveats (recorded, not blocking):

- Native Windows UNC project roots are still refused rather than supported;
  `cmd.exe` cannot reliably hold a UNC working directory. This is a
  documented limitation of the contract, not a gap in it.


### Phase 2B — Detect cmux when a usable cmux executable or supported cmux control environment is present

Contract: Given a machine where the `cmux` executable is not on `PATH` but
Glasshouse is running inside a cmux surface, when discovery runs, cmux is
reported as detected and configured with the evidence that proves it, while
never being reported as launchable and never recording any environment
variable's value.

State: COMPLETE

Production evidence:

- `integrations::presence_without_executable` reads the cmux control
  environment (`CMUX_SOCKET_PATH`, corroborated by `CMUX_SURFACE_ID` and
  `CMUX_WORKSPACE_ID`).
- `integrations::detect_one_with` consults it in the `ResolveOutcome::NotFound`
  arm only, yielding `IntegrationStatus::Configured` with `executable: None`.
  The `Usable` and `Unusable` arms are untouched.

Regression evidence:

- `cmux_socket_path_set_yields_evidence_naming_it`,
  `cmux_corroborating_variables_are_also_named`,
  `empty_cmux_socket_path_counts_as_unset`, `no_cmux_variables_yields_no_evidence`
  pin the decision, over injected lookups so no test mutates the environment.
- `absent_executable_but_presence_evidence_is_configured_not_launchable` proves
  the wiring, including that `is_usable()` stays **false** — a detected
  integration with no executable must never be mistaken for a launchable one.
- `absent_executable_with_no_presence_evidence_stays_not_found` pins the
  unchanged behaviour for everything else.

Failure/isolation evidence:

- `evidence_notes_never_contain_a_value_only_names` fills every consulted
  variable with sentinels and asserts no note contains one.
  `CMUX_SOCKET_CAPABILITY` is a capability token and is never read at all.
- Non-vacuity (mutation actually run): making the `NotFound` arm ignore the
  presence evidence makes the wiring test fail.

Platform/external evidence:

- Live probe on macOS inside a real cmux surface, with `cmux` removed from
  `PATH`: `glasshouse doctor` reports `cmux [configured]` with the notes
  "candidates tried: cmux", "CMUX_SOCKET_PATH is set", "CMUX_SURFACE_ID is set".
- CI green on `ubuntu-latest`, `macos-latest`, `windows-latest`, and lint. The
  behaviour is environment-variable based with no `cfg` gating, so all three
  platforms execute the same code.

### Phase 2B — Detect Ollama when a usable ollama executable or configured local endpoint is present

Contract: Given a machine with no `ollama` executable on `PATH` but a
configured local endpoint, when discovery runs, Ollama is reported as detected
and configured, while never being reported as launchable and never recording
the endpoint's value — which can carry credentials.

State: COMPLETE

Production evidence:

- `integrations::presence_without_executable` treats a set, non-empty
  `OLLAMA_HOST` as a configured endpoint, wired through the same
  `detect_one_with` seam as cmux above.
- Deliberately no network request: discovery stays non-destructive and adds no
  HTTP dependency. The capability asks whether an endpoint is *configured*, not
  whether a server is answering.

Regression evidence:

- `ollama_host_set_unset_and_empty` pins set, unset, and empty-as-unset.
- The same wiring and non-launchability tests listed above cover Ollama, which
  is the integration they are written against.

Failure/isolation evidence:

- `evidence_notes_never_contain_a_value_only_names` covers `OLLAMA_HOST`.
- Live probe: `glasshouse doctor` with
  `OLLAMA_HOST=http://user:SUPERSECRET@127.0.0.1:11434` reports
  `Ollama [configured]` with the note "OLLAMA_HOST is set", and the whole
  report contains **zero** occurrences of the secret.

Platform/external evidence:

- Same CI run as the cmux entry above; no `cfg` gating, so all three platforms
  execute this code.

Missing evidence:

- None for the stated contract. Whether a configured endpoint is actually
  *reachable* is deliberately out of scope and would need a network probe.

### Phase 2A — Make unsupported platform/harness combinations fail with a clear diagnostic rather than a partial broken session

Contract: Given a platform and harness combination Glasshouse knows cannot
work, when a session or probe would otherwise be started, Glasshouse refuses
before any process exists and says what is wrong and what to do about it,
rather than starting something that appears alive while operating on the wrong
directory or the wrong process namespace.

State: COMPLETE

Production evidence — the combinations Glasshouse knows, and where each is
refused:

- **UNC project root + `.cmd`/`.bat` harness** — `launch::unsupported_combination`,
  called by `HarnessLaunch::build_command` before the command is returned, so
  `spawn` never reaches PTY creation. `cmd.exe` cannot hold a UNC working
  directory and does not fail when asked to: it substitutes the Windows
  directory and runs, so the session would have looked alive while operating
  outside the project entirely.
- **WSL + a Windows-interop executable** — `platform::exec::resolve_with`
  filters `/mnt/c`-style hits and returns `ResolveError::WindowsInteropOnly`,
  whose message explains that the child would run in the Windows process
  namespace where the project's Linux path is meaningless.
- **No usable executable** — `session::select::SelectionError::NotInstalled`
  names the candidate names that were tried.
- **A requested integration that is not a harness** —
  `SelectionError::NotAHarness` names the category and lists the harnesses
  that can be launched.
- **A harness turned off in configuration** — `SelectionError::Disabled`.
- **No terminal to attach to** — `session::attach::attach` refuses rather than
  hanging on a pty query nothing can answer.

Regression evidence:

- `a_script_harness_in_a_unc_project_is_refused_with_a_diagnostic` asserts the
  message names the directory, the reason, and the remedy — not merely that it
  failed.
- `every_other_combination_is_allowed` keeps the refusal narrow: a `Direct`
  harness in a UNC directory, and a script harness in an ordinary local or
  verbatim-drive directory, must all still launch.
- `unc_detection_covers_both_spellings_but_not_a_verbatim_drive` pins that
  `\\?\C:\...` is a local path despite also starting with two backslashes.
- `cmd_and_bat_are_windows_scripts_only_on_windows`,
  `a_nonsense_slug_is_unknown_and_names_the_valid_ones`, `cmux_is_not_a_harness`,
  and `attaching_without_a_terminal_is_refused` cover the other refusals.

Failure/isolation evidence:

- The refusal happens in `build_command`, before `PtyProcess::spawn`, so no
  process, pty, or terminal state exists when it fires. Non-vacuity (mutation
  actually run): disabling the condition makes the refusal test fail.

Platform/external evidence:

- CI green on `ubuntu-latest`, `macos-latest`, `windows-latest`, and lint. The
  check is a function of a path's shape and an executable's kind, with no
  `cfg` gating, so all three platforms execute it.

Honest limits:

- No real UNC share was exercised. The *refusal* is platform-independent code
  that CI runs everywhere; what is taken from documented Windows behaviour,
  not from a live run, is the premise — that `cmd.exe` would substitute the
  Windows directory rather than fail. That premise is why the refusal exists,
  and it was already recorded as a known limitation before this change.
- This capability covers the combinations Glasshouse currently knows about. A
  newly discovered one would reopen it.

### Phase 2A — Support native Windows as a first-class Glasshouse runtime where the selected harness is available

Contract: Given native Windows with an installed harness, everything Glasshouse
can currently do — resolve a project, isolate its state, discover harnesses,
probe versions, and open a real harness session inside the project root — works
the same way it does on macOS and Linux, and any combination that cannot work
is refused rather than half-started.

State: COMPLETE

This is a summary capability. It is checked because every capability it
summarises is checked and because the same test suite that backs the macOS and
Linux boxes now runs, and passes, on `windows-latest` — not because Windows was
judged by a weaker standard than its siblings.

Production evidence — the Windows-specific paths, all reachable in production:

- `pty`: ConPTY through `portable-pty`, with `process::JobHandle` giving
  Windows a kill-the-whole-tree equivalent that `TerminateProcess` alone does
  not provide — which matters precisely because a `.cmd` harness makes the real
  process a grandchild.
- `platform::exec`: `.exe`/`.cmd`/`.bat` classification, `cmd.exe /D /C`
  translation, `plain_script_path` conversion of the verbatim form `cmd.exe`
  cannot open, and rejection of `cmd.exe` metacharacters in arguments.
- `platform::paths::strip_verbatim_prefix` and `Project::display_root`: a
  canonical Windows root is verbatim, which is correct as an identity and
  unusable at a process boundary, so it is stripped there and only there.
- `launch::unsupported_combination`: refuses the one combination known not to
  work rather than starting a session that would run outside the project.

Regression evidence actually executed on `windows-latest`:

- CI run `32790669974` on `53e98f0`: **197 lib tests and 21 PTY smoke tests,
  0 failures**, plus lint, alongside green `ubuntu-latest` and `macos-latest`.
- `the_launch_command_opens_the_configured_harness_inside_the_project_root`
  runs the shipped binary in a real ConPTY and confirms a `.cmd` harness
  starts in the project root, with project-over-user executable precedence and
  exit-code propagation.
- `a_fake_installed_harness_launches_inside_the_discovered_project_root`
  exercises the real `cmd.exe /D /C` translation and asserts the canonical root
  was verbatim before `display_root` stripped it.
- `a_direct_executable_launches_through_the_harness_seam` covers the `.exe`
  branch, whose verbatim path had never been confirmed acceptable to
  `CreateProcess`.
- The PTY smoke suite proves output streaming, input, resize, and exit
  detection against a real ConPTY child.

Failure/isolation evidence:

- Windows CI caught two defects local gates could not: `cmd.exe` refusing the
  verbatim script path (so **no `.cmd` harness could start at all**), and a
  test comparing Windows path spellings that differ while denoting the same
  directory. Both are fixed and covered.
- Project-root refusals, the canonical-path guard, and per-project database
  isolation all execute in the Windows lib suite.

Honest limits — these are true on every platform, not Windows-specific:

- The interactive multi-session TUI does not exist yet (Phase 3), so
  "first-class runtime" means parity with macOS and Linux in what Glasshouse
  can do *today*, which is exactly the standard those two boxes were checked
  against.
- UNC project roots are refused for script harnesses rather than supported.
