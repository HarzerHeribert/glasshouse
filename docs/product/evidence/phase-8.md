# Capability evidence — phase 8

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 8 — Record observed Codex compaction events or compaction-related state when available

Contract: Given a Codex session that compacts its context, when Glasshouse is
asked about that session afterwards, it can say that a compaction happened —
without replacing the harness's own compaction mechanism.

State: **BLOCKED on Phase 30, and deliberately not forced.**

What is established:
- Codex exposes `PreCompact` and `PostCompact` in the hook catalogue read from
  its own review screen, so unlike Claude Code — which exposes no compaction
  hook at all, and whose equivalent line is blocked for that reason — the
  events are genuinely *available* here.
- Codex has a `/compact` command ("summarize conversation to prevent hitting
  the context limit"), so a compaction can be triggered deliberately rather
  than waited for. Observing these events is therefore cheap, which is not the
  usual situation with compaction.
- Requesting them is two lines in `REPORTED_EVENTS`.

Why the box is not checked anyway:
- `lifecycle_for` maps a harness event to a `SessionLifecycle`, and compaction
  **is not a lifecycle state** — a session that compacts was running before and
  is running after. Mapping it to one would be a lie of convenience.
- So "record" has to mean durable state of some other kind, and that state is
  **Phase 30's line: "Track the number of observed compactions for a session
  when known."** Phase 30 is unimplemented and far later in map order.
- Requesting the events and writing only a log line would satisfy "observed"
  while quietly failing "record". A log is not state, and checking the box on
  it would be exactly the confusion between code presence and product behaviour
  this ledger exists to prevent.

The honest shape: this line is the observation half of a capability whose
storage half belongs to Phase 30, the same way Phase 1 line 90 sat complete and
unreachable until an adapter existed to feed it. When Phase 30 adds the counter,
this becomes a two-line change plus one `/compact` against the real binary.

Missing evidence:
- Phase 30's per-session compaction count. Everything else is ready.

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

---

# Line 327 — closed 2026-08-30 by a runtime probe against a live Codex

Package `GH-CODEX-COMPACTION-PROBE`. **This line was never blocked on code.**
Its producer, caller and propagation have shipped for some time; its reader
landed the same afternoon as a side effect of `GH-SESSION-CONTEXT-DOOR`
(`glasshouse sessions show` prints a `compactions` line). What it had never had
was an **observation**.

State: **COMPLETE**

## The ruling, recorded before the probe so it could not be reverse-fitted

> *"Record observed Codex compaction events **or compaction-related state**
> when available"* is written disjunctively, and the durable per-session count
> satisfies the **state** disjunct.

Reading it to require one timestamped row per occurrence reads the "or" out of
a line written for exactly this case: Codex only ever says *about to compact*,
`PostCompact` is excluded on purpose (`session/lifecycle.rs`), and a
per-occurrence row would carry a time and no verified event. `NULL` / `0` / `n`
remain three distinguishable states, as `database.rs:1682-1698` argues at
length.

## What was observed

**Five real compactions, five counted.** A live `codex-cli 0.150.1` decided to
compact its own context and spawned the hook itself; the hook was never invoked
by hand. Each `/compact` moved the count by **exactly one** — so `PostCompact`
is being correctly ignored — and `glasshouse sessions show` printed the new
value each time. §60 is satisfied: this is five trials, not one.

Attribution is airtight. On the fresh session Codex reported
*"7 hooks need review"*, and the seven with a review count of 1 were exactly
`REPORTED_EVENTS`, element for element. `PreCompact` and `PostCompact` showed
`1` — **only Glasshouse's hook could have produced the increments.** The probe
ran in a throwaway project with its own data and config roots.

## The catalogue drifted, and now there is a tripwire

The observed catalogue was read from **0.149.1**; the installed binary is
**0.150.1**. `PreCompact` and `PostCompact` are present and unchanged in
spelling and position — but Codex gained a twelfth event, `Interrupt`, which
`HOOK_EVENTS` did not list. It has been added to the catalogue and
**deliberately not** to `REPORTED_EVENTS`: an aborted turn says nothing about a
lifecycle that `Stop` does not already say.

**Codex publishes no machine-readable hook catalogue**, and the probe verified
that a `hooks.json` naming an event Codex does not recognise is accepted **in
complete silence** — no diagnostic, exit 0. So the catalogue's *contents*
cannot be asserted against anything offline, and asserting `HOOK_EVENTS`
against itself is the vacuous shape this project has been bitten by.

What *can* be checked is **provenance**:
`harness::codex::CATALOGUE_OBSERVED_VERSION` records the version the catalogue
was read from, and
`session_hook::the_codex_hook_catalogue_was_read_from_the_installed_codex`
compares it to `codex --version`. Mutating the constant back to `0.149.1`
**KILLED** it, so it would have caught this exact drift. It skips when Codex is
absent, so it is inert on CI.

**Its cost, stated rather than hidden:** it fails on every Codex release,
including ones that change nothing. That is the honest trade for an
observed-not-documented catalogue — the claim *"read from version X"* genuinely
expires when X changes. Bumping the constant alone is the one edit that makes
the check worthless, and its doc comment says so.

## Two defects found, neither fixed here

Reported rather than patched, because their files were forbidden to the probe.

**(a) A resumed session's lifecycle stays `stopped`, and every later hook is
silently discarded.** `may_apply` is `current.is_live() && current != next`, and
`is_live()` is `false` for `Stopped` — so once a session stops, **no transition
can ever apply again**, and nothing moves it back on resume. Observed: Codex
was demonstrably running and writing `turn_started`/`turn_ended` rows while the
record read `stopped`. Timestamps rule out a race — `process_exited` and
`session_resumed` were **29 seconds apart**. `GH-RESUME-LIFECYCLE-FIX` is
dispatched against it.

**(b)** The compaction count is written outside that gate, so a row can read
`stopped` with the count incremented after the stop. The probe judged this a
symptom of (a), and it is.

**Neither affects this line's claim:** in the trials where the lifecycle was
correct throughout, the count was correct.

## Limits

- **macOS only, Codex 0.150.1 only.** The hook catalogue is observed per
  platform and this observation is one platform's.
- Says nothing about Claude Code, which emits no compaction event at all — the
  reason map line 310 is refused rather than open.
