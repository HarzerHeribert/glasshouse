# Glasshouse implementation handoff

Last updated: 2026-08-25 (Europe/Berlin)

## Current capability / phase

**Phase 8 is five of ten; Phase 6 is twelve of thirteen. 131 -> 134 checked boxes.** `main` clean at
`63bd260`. CI `32849951837` green on Linux, macOS, Windows and lint, with the
Windows job confirmed to have executed both new end-to-end tests by name.

- **Phase 8 line 2 — capture the native Codex session identifier: closed.**
  This is the line that unblocks Codex resume, and it is the first time a
  Codex session gets a `Resumable` disposition.
- **Phase 6** — harness adapters: eleven of twelve (communication style open).
- **Phase 7** — Claude Code: eight of ten. Both open lines are honest gaps:
  permission detection needs a permission prompt to fire (this machine runs
  Claude Code in auto mode), and Claude Code 2.1.245 exposes no compaction
  hook at all.

A correction to an earlier revision of this file: its header said the checked
count went "from 107 to 120" while its own body already described the
131-box state. The authoritative count comes from the map, which is what the
number above is read from.

### The previous session's plan for this line was wrong, and the install said so

The last checkpoint said: match rollout headers on `payload.cwd`, take
`payload.id`. Read against all 555 real rollout files in `~/.codex/sessions`,
that is not sufficient:

- **Subagent threads write their own rollouts with their parent's `cwd`.**
  There are 171 of them against 70 real interactive sessions, so matching on
  `cwd` alone would have captured a subagent's identifier most of the time,
  and `glasshouse resume` would have reopened a subagent thread wearing the
  user's session's name.
- **`payload.session_id` — the better-named field — is missing in 28 of 555.**
  `payload.id` is present in all 555 and always equals the filename UUID.

The rule that is exact on this install: `originator == "codex-tui"` **and** no
`parent_thread_id`. It selects precisely the 70 real interactive sessions, with
zero counterexamples. `source == "cli"` corroborates it in all 70 and is
deliberately *not* a condition — every extra condition is another way to break
on a Codex update. Full reasoning in `GLASSHOUSE_DESIGN_DECISIONS.md`.

## Verified completed work

### This session — every adapter declares its approval modes

Phase 6's new line, closed. `ApprovalModes` carries `automatic_review`,
`bypass` and `sandbox` as `Declared<&'static str>`, all seven adapters fill it
in from their own binaries, and `glasshouse doctor` prints it.

The distinction is the point: **three harnesses classify, four only bypass.**
Claude Code's auto mode, Codex's `--approve-for-me` and Cursor's
`--auto-review` are automatic review; OpenCode's `--auto`, Hermes's `--yolo`
and Antigravity's `--dangerously-skip-permissions` are not, and are recorded as
bypasses only.

**A mutation caught a weak test, which is what mutations are for.** The first
version asserted only that an `automatic_review` evidence string avoided the
words "yolo", "dangerously" and "bypass". A mutation recording OpenCode's
`--auto` as automatic review — evidence reading "…(dangerous!)" — walked
straight through it, because "dangerous!" is not "dangerously". The fuzzy check
was replaced with an exact harness-by-harness table, and the same mutation now
fails. That weak test was specified by the orchestrator's own packet, not
invented by the worker.

**And running the binary caught an overstatement before it shipped.** The first
rendering printed "no automatic review" for anything `Unverified` — but
`Declared` cannot say "verified absent" for a mode name, so `Unverified` means
nobody established one. Pi makes the difference concrete: installed, not on
`PATH` here, `--help` unreadable. It now reads "automatic review unverified",
matching the convention the neighbouring `capabilities:` line already used.


### This session — resuming a Codex session, which cost no production code

Phase 8 line 3 closed without a line of new production code, and that is the
Phase 6 adapter contract paying for itself. `resume_session` selects the
harness the *record* names and asks its adapter; `Codex::resume` already
returned `["resume", <id>]`. The only thing missing was an identifier, and
line 2 supplied it.

**Codex resumes with a subcommand, Claude Code with a flag.** That difference
is exactly what the contract exists to absorb, and it is now asserted rather
than assumed: the test fails if a Codex invocation is ever handed
`--resume`, with an assertion message that names the failure as one harness's
vocabulary leaking into another's.

The test is deliberately **not** `#[cfg]`-gated. Windows CI found a real defect
on this same rollout-fixture path for line 2, so there is a concrete reason to
keep proving it on all three platforms rather than only where it was written.

Three mutations, three kills: Codex given Claude Code's flag, Codex returning
no resume arguments, and Codex resuming a different conversation.

**Verified against the real Codex 0.149.1, at no model cost** — a known
identifier replays the conversation, an unknown one answers `ERROR: No saved
session found with ID <id>`. Two traps recorded with it: a pseudo-terminal with
no window size makes Codex draw nothing and look hung, and its update prompt
defaults to an option that runs `curl … | sh`.


### This session — the Codex session identifier, and the rule `cwd` alone cannot express

`session::native_id::discover` finds the rollout a Glasshouse-started Codex
session wrote, and `capture` records it. Four conditions, all required:
`originator == "codex-tui"`, no `parent_thread_id`, `payload.cwd` canonically
equal to the project root, and `payload.timestamp` inside the window between
Glasshouse starting the session and observing it end.

**Two or more survivors means nothing is recorded.** Not "take the newest" —
the failure mode of guessing is resuming a stranger's conversation, and
`session::select` and the resume identifier resolver already refuse ambiguity
for the same reason.

**Only the first line of a rollout is ever read**, capped at 1 MiB. Everything
after it is the user's own conversation, and
`nothing_is_read_past_the_first_line` is what keeps that a boundary rather than
a habit.

**Discovery runs once, at session end, from both producers** — `launch_session`
and the shell's `poll_exits` loop. That is when the identifier is needed (a
stopped session is `Resumable` only if it has one) and when the window is
two-sided and therefore tightest. Codex writes no rollout until a turn has
happened, verified again this session under an isolated `CODEX_HOME`, so there
is nothing to find earlier.

`session::store::set_native_session_id` finally has a production caller; it had
been unused since Phase 2.

**Eight mutations, all eight killed** — including deleting each of the two call
sites in turn, which is what makes the wiring proved rather than asserted. The
first attempt at the mutation harness was itself defective in two ways worth
recording:

- it restored each mutation with `git checkout -- crates/glasshouse/src/`,
  which — because workers are told never to commit — reverted the worker's
  entire contribution to five tracked files rather than the one mutated line.
  Recovery meant asking the still-live worker session to rewrite them. **Never
  use a path-wide git restore in a worktree whose value is uncommitted.**
- its verdict logic grepped for `0 failed` across all four test binaries, which
  always matches the filtered-out lib line, so it reported "survived" for every
  mutation including ones that never compiled. **A mutation harness must read
  the named test's own result line**, and must distinguish `error: test failed`
  (the kill) from `could not compile` (no result at all).

**An adapter may no longer depend on the session model.** The first
implementation had `harness/codex.rs` importing `crate::session::native_id`;
no adapter on `main` imported `crate::session` at all. The two record types
moved to `harness/mod.rs` where the rest of the adapter vocabulary lives, the
RFC3339 parser became private to `codex.rs`, and
`no_adapter_depends_on_the_session_model` now scans all seven adapters, with a
paired test proving the scan fires on a fabricated `use` and stays quiet on a
doc comment.

**One worker judgement was better than the packet.** The packet asked for a
bidirectional consistency test between `session_id_source` and
`SessionIds::Discoverable`. Cursor, Hermes, Pi and OpenCode all correctly
declare `Discoverable` about their own harnesses without Glasshouse having
built a reader for each, so the converse is not a defect and the test is
one-directional by design.

**A worker's report can be written before it stops working.** The report file
appeared while the worker was still running its own mutation checks, and two
successive `git status` snapshots each showed a different call site missing.
Gate review on the pane going idle, not on the report appearing.


### Previous session — Codex, and a question it asks that Claude Code does not

Codex's startup handshake is `ESC[>5u`, `ESC[6n`, `ESC[?u`, `ESC[c`,
`ESC[0 q`. The `ESC[?u` is the kitty keyboard-protocol probe — a fourth
question, and Glasshouse **deliberately stays silent on it**.

Answering would be the obvious move and the wrong one. The reply means
"supported"; the harness would enable the protocol and expect key events
encoded that way, and `tui::event` sends ordinary bytes. The session would come
up looking perfect and then mis-read every keystroke.

Silence is not a hang here, because of the idiom Codex uses: it sends `ESC[?u`
and `ESC[c` together, and a device-attributes reply arriving with no keyboard
reply before it *is* the negative answer. So the device-attributes reply added
this session is exactly what lets Codex conclude "no kitty protocol" without
waiting. Two tests pin it, and the constant is named
`DELIBERATELY_UNANSWERED` so the next person to find an unanswered query has to
read why before answering it.

The real-harness viewport probe is now shared between Claude Code and Codex,
so both are held to the same check: the harness's own version string must be
absent before a session exists and present afterwards.

**Codex writes no session file until a turn happens** — starting it and killing
it left the rollout count unchanged. So its identifier can only be discovered
after the first turn, by matching a rollout header's `payload.cwd` against the
project and taking its `payload.id`. That is the next piece of Phase 8.

### Previous session — the hooks, observed firing for real

A Glasshouse session was opened against the real `claude` in a pseudo-terminal,
one prompt was submitted, and the session record moved from `starting` to
**`idle`**.

That value settles it rather than suggesting it. The only production code that
*writes* `Idle` is the `Stop`/`StopFailure` arm of
`session::lifecycle::lifecycle_for`; nothing else in Glasshouse can produce it.
So the record could only have reached that state by Claude Code running the
hook Glasshouse generated and installed, which invoked `glasshouse hook`, which
translated the event and wrote it down. Generate, install, fire, report,
translate, record — the whole chain, end to end, against the real harness.

**And one line closed by having nothing rather than something.** "Keep
terminal-text parsing only as a fallback" is satisfied because Glasshouse has
no such fallback at all: state comes from the operating system or from the
harness, never from reading the screen.
`nothing_derives_session_state_from_terminal_output` keeps it that way — the
runtime is the one component that sees terminal output, and it may not move a
session's state. Giving it a method that infers one fails the test.

### Previous session — answering the terminal's questions

A real Claude Code startup was captured in a pseudo-terminal and every escape
sequence it writes before drawing was examined. Three are *questions*:
`ESC[6n` (cursor position), `ESC[c` (primary device attributes) and `ESC[>0q`
(XTVERSION). Everything else — bracketed paste, focus reporting, synchronised
output, keyboard-protocol pushes — is an instruction.

**Glasshouse answered one of the three.** Phase 5's design note had already
written the rule down — "an embedded session must always answer, or the harness
hangs" — and only the cursor-position half was ever built.

The consequence was worse than a hang. Claude Code counts the failures and,
after two, disables its fullscreen renderer *globally*, writing that decision
into the user's own configuration where it outlives Glasshouse entirely. This
user's `settings.json` says `"tui": "fullscreen"`, so Glasshouse had overridden
an explicit preference of theirs, on their machine, permanently.

`TerminalQueryScanner` now recognises all three across chunk boundaries and
answers each: the emulated screen's cursor position; `ESC[?1;2c` for device
attributes, which is what the viewport actually is rather than a richer
terminal whose sequences it could not draw; and Glasshouse's own name for
XTVERSION, so an application that knows the name can decide for itself and one
that does not falls back to conservative defaults.

**Verified against the real binary, in an isolated Claude configuration so the
user's own was not touched.** Before, two sessions were enough to trigger the
auto-disable. After, three consecutive sessions left it absent, the failure
notice was gone, and with `"tui": "fullscreen"` set the fullscreen interface
rendered in the viewport with no notice at all. The isolated configuration was
deleted afterwards.

### Previous session — Claude Code lifecycle hooks

Glasshouse installs per-session hooks so a session's state comes from the
harness saying what happened, not from reading its terminal and guessing.

- The adapter builds the settings document, because its shape is the harness's
  own business. Glasshouse writes it into a directory it owns inside the
  project's state and passes `--settings` — which loads *additional* settings,
  so the user's own hooks keep running and their `~/.claude` is never touched.
- The hooks invoke Glasshouse itself (`glasshouse hook --session … --event …`)
  rather than a shell one-liner, because a one-liner would need different
  quoting on every platform and a harness's configuration is not the place to
  hide shell portability.
- `session/lifecycle.rs` is the only place that knows both vocabularies. An
  unfamiliar event changes nothing, and a late hook cannot revive a finished
  session — hook processes outlive their harness.

**A hook must always exit 0, and that is not a preference.** Claude Code treats
a non-zero exit as a veto: a `UserPromptSubmit` hook that exits non-zero blocks
the prompt outright, with the user's own words echoed back and nothing sent.
That was observed directly — and it is also what made the whole hook mechanism
verifiable *without spending a turn*, since a deliberately failing hook proves
firing while cancelling the API call.

**Two facts read from the real binary, not assumed:**

- `SessionStart` **does not fire** in Claude Code 2.1.245. A document declaring
  one was installed and its hook never ran, while `UserPromptSubmit` from the
  same document did. It is deliberately not among the reported events, and a
  test pins that.
- The hook schema was read out of a real settings document rather than
  recalled: entries hold `{type, command, timeout}`, and only tool events carry
  a `matcher`.

**A defect that only appeared by running it.** The first version of the hook
command carried no paths, so it discovered its own project from wherever the
harness happened to run it. It exited 0, looked healthy, and silently updated
nothing. Every path is pinned now, and dropping them fails two tests.

### Previous session — `glasshouse resume`

- `glasshouse resume <session>` reopens a recorded session in the harness that
  created it — not whichever harness is configured now, because resuming a
  Codex conversation in Claude Code would be nonsense.
- **The identifier resolver accepts any leading part of an identifier**, and
  that is a requirement rather than a nicety: `glasshouse sessions` prints only
  the first twelve characters, so the short form is the *only* identifier a
  user can copy off the screen. Running the shipped binary is what made that
  obvious. Ambiguity is refused and names every candidate.
- Matching uses `substr`, not `LIKE`. Under `LIKE` a bare `%` typed by the user
  would match every session in the project, and "resume whichever came first"
  is precisely the wrong answer.
- The order in `resume_session` is the safety property: the store decides
  whether the session may be resumed *at all* — right project, not still
  running, something to resume to — before a harness is selected and long
  before a process exists.

**A mutation that passed, and what it exposed.** Bypassing `open_for_resume`
entirely left `resuming_an_unknown_session_is_refused` green, because the
identifier resolver turns an unknown identifier away before the guard is ever
reached. That test proved nothing about the guard.
`resuming_a_session_with_no_conversation_is_refused` was written to reach it —
a Codex session, which has no identifier to resume to — and the same mutation
now fails. This is the fourth time a passing mutation has been information
about the tests rather than the code.

**One unreproduced failure, recorded rather than dismissed.** The resume smoke
test failed once, on the run that first compiled it, while clippy, rustdoc and
an MSRV check were building concurrently. It has not failed since in 23 further
runs (15 targeted, 8 full-suite). That matches the macOS `openpty` allocation
race this project already diagnosed and retried around, rather than anything in
the resume path, but it is written down because an unexplained failure that is
merely rare is not the same as one that is understood.

### Previous session — assigned native session identifiers (Phase 7)

**The whole chain is verified against the real binary**, with the user's
approval for the one step that needed a turn:

- `claude --session-id not-a-uuid` → "Error: Invalid session ID. Must be a
  valid UUID." The format requirement is enforced, not merely documented.
- `claude --session-id <minted> -p "..."` → Claude Code wrote its transcript to
  `~/.claude/projects/<slugged-cwd>/<minted>.jsonl`. The assigned identifier
  *is* the conversation's identity.
- `claude --resume <minted>` → reopened that conversation with its earlier turn
  replayed. `claude --resume <unknown-uuid>` → "No conversation found with
  session ID: …". Both observed in a real pseudo-terminal, neither costing a
  model turn.

- `HarnessAdapter::assign_session_id` is how a harness says it will take an
  identifier rather than invent one. Assigning beats discovering: the
  identifier exists before the process does, so a harness that dies during
  startup still leaves a named session, and nothing has to be parsed or
  watched for afterwards.
- `SessionStore::new_native_session_id` mints a valid RFC 4122 version-4 UUID
  from SQLite's randomness — the same source the store already uses. It is
  deliberately **not** derived from the Glasshouse session identifier: the two
  identifier spaces are independent by design, and a session's own name has to
  stay meaningful after the harness's history is gone.
- Both production start paths mint it, record it on the `NewSession`, and pass
  it to the harness. `a_claude_code_session_is_launched_and_recorded_under_one_identifier`
  runs the shipped binary and compares the identifier the harness *received*
  with the one Glasshouse *recorded*. **Mutation-checked in both directions** —
  either half alone is useless, and either half alone now fails.
- Claude Code's own binary enforces the format: `--session-id not-a-uuid`
  answers "Error: Invalid session ID. Must be a valid UUID." A minted
  identifier is accepted and the harness runs normally.

**A test's expectation changed for a good reason.** A cleanly stopped session
used to read `closed`, because nothing ever gave a session a native identifier.
It now reads `resumable`, which is the point of the work. The test asserts the
new truth and records why the old one was right at the time.

**Two smoke tests had to stop using a plain shell as Claude Code.** Glasshouse
now hands that harness `--session-id <uuid>`, and `/bin/sh` answers by printing
its usage. One test was re-registered under Codex — which names its own
sessions and so is started bare — because it is about resize reaching the
child, not about arguments. Worth knowing: **anything configured as
`claude-code` now receives that flag**, so a user's wrapper script has to pass
its arguments through.

### Previous session — the harness adapter interface

- `harness::HarnessAdapter` is the contract: `id`, `executable_candidates`,
  `start`, `resume`, `describe`, `message`, `interrupt`. The map's six verbs
  all land on it — observing is `describe().hooks` plus
  `describe().session_ids`.
- `IntegrationId::executable_candidates` **delegates to the adapter** for every
  harness. One place a harness's executable name lives, which is the phase's
  fixed requirement made structural rather than aspirational. The catalogue
  keeps names only for cmux, Ollama and llama.cpp, which are not harnesses and
  have no session to start.
- `HarnessSelection::start_args` is the single seam both session producers go
  through (`glasshouse launch` and the shell's `n`): the adapter's arguments,
  then the user's, so an explicit request always has the last word. No harness
  needs a start argument today, so the ordering rule is proven against a test
  adapter that does.
- `glasshouse doctor` prints every adapter's declarations. That is what keeps
  `describe` from being a data structure nothing reads, and it is generic over
  the trait — it cannot tell one harness from another.
- Two source-scanning tests hold the architecture: the generic PTY runtime and
  the session model may not name `HarnessAdapter`, `crate::harness` or
  `IntegrationId` in production code. Comments are stripped first, because
  `session/store` *documents* that it holds an identifier's string form — the
  boundary working, not breaking.

**Running the shipped binary found what the suite could not, again.** Two
rendering defects surfaced only from reading real `glasshouse doctor` output:
declarations rendering as nested backticks, and a session-id source phrase
that did not fit the sentence it was interpolated into. Both were invisible to
every test that passed.

**Five mutations were run and each failed its target**: giving Codex Claude
Code's resume flag; removing Antigravity's `agy`; restoring a hard-coded
executable name to the catalogue; making the doctor report's adapter loop
print nothing; and adding an `IntegrationId`-returning method to
`SessionRuntime`.

**Growing the catalogue moved the setup wizard toward a limit, so the list now
scrolls.** Ten integrations plus two section headers still fit an 80x24 screen,
but the margin is thin, and Ratatui silently draws fewer rows when a list
outgrows its area — an integration past the bottom edge would be one the user
can neither see nor toggle, with every test still green. The list is rendered
with its selection now, so it follows the cursor. Two tests hold it: all rows
present at 80x24, and every row still reachable at 80x12, where the list
genuinely truncates. Reverting to stateless rendering fails the second and not
the first.

**One scare that was not a defect.** The first version of that test asserted
every integration had a row, and failed on cmux — which the wizard
deliberately never offers unless it is actually detected. The layout was fine;
the test's premise was wrong. Worth remembering that a failing new test is a
claim about the code *or* about the test.

**A test was rewritten because it pinned a wrong fact.**
`antigravity_only_searches_the_literal_name` asserted the guess that a real
install disproved. It is now
`no_integration_is_searched_for_under_a_guessed_abbreviation`, which keeps the
hazard the original actually guarded — `ag` is the-silver-searcher on many
machines — and drops the guess.

### Earlier sessions — live sessions behind the interface

- `session::runtime::SessionRuntime` holds several live harnesses, each with
  its own reader thread draining its pseudo-terminal into its own bounded
  `Scrollback`. Focus is only a statement about which session the keyboard
  reaches; `focus()` touches no process.
- `shell::run` is its production consumer: `n` starts a session, session mode
  forwards keystrokes, resize reaches the focused child, ticks poll exits and
  refresh the viewport.
- Exits come from asking the process, never from output going quiet. A harness
  thinking in silence must not be mistaken for one that has finished.

**The defect end-to-end testing found, that unit testing could not.** The
session-mode escape chord was implemented as `Ctrl` + `']'` — which is what the
synthetic `KeyEvent` in its unit tests looked like. Crossterm's Unix parser
decodes the control range `0x1C..=0x1F` arithmetically, so a real terminal's
`Ctrl-]` arrives as `Ctrl` + `'5'` and never matched. A user entering session
mode had **no way back**: precisely the failure the single-chord escape exists
to prevent. Both spellings are now accepted and separately tested.

**A test written and then deleted, twice, for different reasons** — both worth
remembering:

- Asserting that switching sessions changes only the view requires reading the
  frame currently on screen. A full-screen Ratatui application repaints
  differentially, so a captured pseudo-terminal stream cannot be sliced back
  into frames by content, and the assertion silently read every viewport ever
  drawn. Phase 5 needs a real terminal emulator anyway; that is when this
  becomes testable.
- Asserting exit detection is independent of output needs a process that exits
  while its output stream stays open. A direct probe showed macOS reports
  end-of-file on the pseudo-terminal master as soon as the foreground child
  exits, even with a background child still holding the slave, so the
  discriminating case cannot be built there.

### Earlier sessions — the TUI shell

- `glasshouse` with no arguments opens the shell; piped or redirected runs keep
  the plain summary rather than drawing a full-screen interface into a file.
- Split like the first-run wizard: `shell::state` answers keys without drawing,
  `shell::view` draws without deciding anything. That is what makes the
  interesting behaviour testable without a terminal.
- The session bar renders the records `session::store` keeps, so Phase 3 reads
  what Phase 2 wrote — the two halves of this session meet in production, not
  only in tests.
- The overview draws *over* the shell rather than replacing it, so it reads as
  somewhere you leave rather than somewhere you go. Escape leaves the overlay
  while one is open and leaves Glasshouse only when none is.
- Selection follows a session's identifier, not its index. Sessions sort by
  last activity, so a refresh reorders them, and holding an index would move
  the user to a different session behind their back.
- The status bar carries the key bindings plus a note when a key could not do
  anything — pressing Tab in a one-session project explains itself instead of
  looking like a dead keyboard.

**Mutation testing rejected a piece of this code, which is the point of it.**
The status bar originally measured the remaining width and truncated a note to
fit. Removing that measurement changed nothing on screen, because Ratatui
already clips the row. The measurement is gone; the property that matters —
bindings are needed permanently, a note only once — is now carried by writing
the bindings first and letting the clip fall where it should, and swapping the
order fails the test.

**It also exposed two vacuous assertions, both the same mistake.** The
real-terminal check for the project root survived having the root blanked out,
because the project's name and its root's last component are the same string
and a bare `contains` matched the title bar. The same flaw let the
narrow-terminal test pass while truncating from the wrong end. Both now read a
single specific row or field. The lesson generalises: **asserting against a
whole screen is nearly always weaker than it looks.**

The text-first constraint is enforced mechanically rather than by assertion:
Ratatui's decorative widgets all draw with Unicode block elements, so the test
fails on any character in U+2580..U+259F, and adding a sparkline-looking line to
the viewport fails it.

### Earlier sessions — the session store

- `session::store` is Glasshouse's own record of the sessions in a project,
  deliberately not a view over any harness's session files. `native_session_id`
  is a nullable *reference*, so a record is complete before a harness has
  produced an identifier and stays valid after the harness's history is gone.
- **Project isolation is structural, not a query filter.** Migration 2 adds
  `BEFORE INSERT` and `BEFORE UPDATE OF project_id` triggers that abort any row
  whose `project_id` is not the identifier bound in `project_metadata`. No
  present or future query has to remember to filter. The comparison uses
  `IS NOT` rather than `<>` so that a missing binding aborts instead of
  evaluating to NULL and passing — mutation-proven, not merely argued.
- `SessionRecord::disposition` derives active/resumable/closed/failed from
  lifecycle plus the presence of a native identifier, rather than storing a
  second column that could disagree with the first. A stopped session with no
  native identifier reads as closed, because offering a resume with nothing to
  resume to would produce a blank session wearing an old session's name.
- `glasshouse launch` now records what it starts, moving the session through
  `Starting` -> `Running` -> `Stopped`/`Failed`, and `glasshouse sessions`
  reads it back. Creating the record is fatal if it fails; every later state
  change is best effort, because once a harness is running, Glasshouse's
  bookkeeping is not worth failing the user's session over.
- The schema has nowhere to put a provider credential, and
  `the_project_database_schema_has_nowhere_to_put_a_credential` pins the exact
  `(table, column)` list so any future addition fails until someone reviews it.
  An allowlist, not a name pattern: `project_metadata.key` would false-positive
  on any name match, and a credential column could just as easily be `value`.

Two defects were caught by running the thing rather than reading it:

- Every `Display` impl used `Formatter::write_str`, which **silently ignores
  width and alignment**, so the session listing's columns were ragged. Fixed
  with `Formatter::pad` and pinned by a test.
- `too_new_schema_is_rejected_and_not_recreated` set *every* migration row to
  99, which worked with one migration and violated the primary key with two.
  The fixture now appends a row, which is also what a newer build would
  actually leave behind.

One documented claim turned out to be **wrong and was corrected**: the unique
index's `WHERE native_session_id IS NOT NULL` clause was justified as
preventing collisions between sessions with no identifier yet. It does not —
SQLite already treats NULLs as distinct in a unique index. The mutation that
should have failed passed, which is how it was caught. The clause is kept for
index size and intent, and the comment now says so; the real hazard it guards
against is a future `NOT NULL DEFAULT ''` refactor, which is now its own
mutation check.

- `glasshouse launch [harness] [-- args]` is the first production consumer of
  `HarnessLaunch`. Until now the Phase 1 promise rested on a mechanism no
  shipped code exercised.
- `session::select` resolves exactly one harness and one executable, preferring
  a project-level configured path over a user-level one and an explicit path
  over PATH discovery. It refuses ambiguity rather than guessing, and a
  configured path that will not resolve is an error, never a silent fallback to
  a different binary.
- `session::attach` is a transparent bridge, not a renderer. That is what makes
  ConPTY's startup handshake work with no terminal emulation in Glasshouse: the
  cursor-position query reaches the user's real terminal, which answers it as
  it would for any program. Nothing in Glasshouse may answer it as well, or the
  harness receives the reply twice, as input.
- `shutdown::RawModeGuard` takes raw mode without the alternate screen, which
  is what routes Ctrl-C to the harness instead of to Glasshouse.
- The reported parallel PTY flake was diagnosed and is **not a Glasshouse
  defect**. Under stress (320 binary runs, ~6,400 test executions, 27 failing
  runs) every failure had one cause: `openpty` refusing to allocate at spawn
  time. The test named in the earlier report failed zero times. Probes pinned
  it to a macOS `openpty(3)` race under concurrent allocation — 64 live
  pseudo-terminals against a cap of 511 reproduced it, while the same churn
  from one process at ~8,000/s produced none — and it leaves `errno` at `-6`,
  which is not a valid errno. `pty::open_pty` now retries the allocation only,
  five times, side-effect free by construction.

- Discovery no longer gives up when an executable is absent. Both the cmux and
  Ollama capability lines are an OR, and only the left half had been built, so
  Glasshouse running *inside* cmux reported cmux as not found. Presence
  evidence — cmux's control environment, Ollama's configured endpoint — is now
  consulted in the not-found path only, reporting the integration as configured
  with no executable, so `is_usable()` stays false and nothing tries to launch
  it. Only variable *names* are ever recorded: a live `doctor` run with a
  credential in `OLLAMA_HOST` shows zero occurrences of it.
- A `.cmd` harness in a UNC project is refused before any process exists.
  `cmd.exe` would not have failed there — it substitutes the Windows directory
  and runs — so the session would have looked alive while operating outside the
  project entirely.

### What CI caught the moment it was allowed to run

Pushing for CI turned up **two production defects** that every local gate, two
independent reviews, and a green 24-test PTY suite had all missed:

1. **`cmd.exe` cannot open a verbatim `\\?\` path** (`4aa31ad`). Resolving an
   executable canonicalizes it, canonicalizing on Windows yields the verbatim
   form, and that went straight into `cmd.exe /D /C <script>`, which answered
   "The system cannot find the path specified" and exit 1. npm installs
   `claude`, `codex`, and friends as `.cmd` shims, so **no harness could have
   started on Windows at all.**
2. **A project-level executable override silently disabled the harness**
   (`e937dda`). `IntegrationConfig::enabled` was a plain `bool` with
   `#[serde(default)]`, so a project file overriding only a path parsed as
   `enabled = false` and beat a user-level `true`. The decision is now
   `Option<bool>`, making the tri-state per field rather than per entry.

Two process lessons worth keeping:

- **A green Windows tick is not proof the suite ran.** When the lib target
  fails, cargo never reaches `tests/pty_smoke.rs`, so the `.cmd` and
  verbatim-path claims silently did not execute while an earlier ledger
  revision implied they had. Confirm execution, not just the conclusion.
- **Make a platform-only failure explain itself on the first red.** Two CI
  round trips were spent guessing before the test was changed to print
  program, argv, requested cwd, canonical root, marker presence, exit status,
  and both streams. That one change identified the bug immediately.

### Review findings, and one the reviewer got half right

A read-only Ox reviewer worked the batch as a ten-item checklist and returned
ACCEPT WITH FINDINGS. Both findings were real and both are fixed:

- `SessionRecord::disposition` led with `lifecycle if lifecycle.is_live()`. A
  **guarded arm does not count towards exhaustiveness**, so the match needed a
  wildcard, and a new `SessionLifecycle` variant would have silently become
  `Active` — the opposite of what its "unreachable" comment claimed. Both it
  and `is_live` now enumerate every variant with no `_`, verified by adding a
  variant and watching three compile errors appear.
- `format_age`'s explicit `seconds < 0` branch returned the same string as the
  arm below it.

The reviewer's *reasoning* on the second was wrong: it said `saturating_sub`
clamps to zero, making the branch dead. It does not — `i64::saturating_sub`
saturates at `i64::MIN`, so the value really can be negative and the branch was
reachable, merely redundant. Right conclusion, wrong mechanism. Checking it
rather than accepting it also turned up an edge the report missed: a row
holding `i64::MIN` prints an absurd age, now pinned by a test that asserts the
honest contract (finite, never negative) instead of a prettier one that would
have required a magic clamp.

## Unresolved loose ends

- **`codex` on `PATH` is a cmux wrapper script, not Codex.** In a cmux terminal
  — which is where this project is developed and run — `which codex` resolves
  to `…/cmux-cli-shims/<uuid>/codex`, a bash script that execs
  `cmux-codex-wrapper`. That wrapper injects `--enable hooks`,
  **`--dangerously-bypass-hook-trust`** and `-c hooks.X=…` into every
  entrypoint that starts a session (interactive, `exec`/`e`, `resume`, `fork`)
  so cmux's own session hooks run unprompted; other subcommands, including
  `--help`, pass through untouched.

  Observed directly: a session started with no such flag printed
  "`--dangerously-bypass-hook-trust` is enabled. Enabled hooks may run without
  review for this invocation."

  Consequences worth holding on to. `session::select` resolves that shim, so
  Glasshouse's Codex sessions already inherit cmux's hooks and its trust
  bypass. Glasshouse's own project-local hooks would be a *second* source
  alongside cmux's `-c hooks.X=` injections, and whether they compose is an
  assumption rather than a finding. Declarations read from `codex --help`
  remain sound, because the wrapper passes `--help` through — but they were
  read through a shim, which is worth knowing now rather than discovering later.

  **This is the Antigravity lesson in a new shape**: there the executable's
  *name* was wrong; here the name is right and the *identity* is not. Glasshouse
  should not silently prefer the real binary — the shim is what the user's
  environment provides, and stepping around it would break cmux's integration
  and the "operate the user's real installed harness" invariant. Making the
  resolved path visible, which `glasshouse doctor` already does, is the right
  response.

- **Codex's hook trust rides on its workspace-trust prompt.** Entering an
  untrusted directory, Codex asks whether to trust it and says in its own words
  that "Trusting … allows … hooks". So a Glasshouse session, being a real
  harness in a visible viewport, can simply let the user answer it — no
  user-level write needed. Whether that prompt alone enables a project-local
  `.codex/hooks.json`, or a per-file `[hooks.state…]` hash is also required, is
  **not yet established**.

- **No Codex hook has been observed firing yet.** With the workspace trusted
  and `<project>/.codex/hooks.json` present under both PascalCase and
  snake_case, a start-and-kill session fired nothing — consistent with
  `SessionStart` not firing in Claude Code either. Settling whether the file is
  read at all needs one real turn against a `user_prompt_submit` hook. Full
  evidence and the ordered open questions are in
  `.agent-runtime/notes-codex-hooks.md`.

- **The Codex adapter was citing the wrong artifact for its event names, and
  is fixed.** It declared ten `snake_case` events taken from
  `[hooks.state…]` trust keys — real keys, but the spelling Codex uses to
  *record trust*, not the spelling it reads from a hooks document. Codex
  0.149.1's own **hook review screen** enumerates **eleven PascalCase events
  with descriptions**, and that is now what the adapter declares:
  `PreToolUse`, `PermissionRequest`, `PostToolUse`, `PreCompact`,
  `PostCompact`, `SessionStart`, `SessionEnd`, `UserPromptSubmit`,
  `SubagentStart`, `SubagentStop`, `Stop`. `SessionEnd` had been missing
  entirely. This is the second time a declaration derived from the wrong
  artifact was wrong; the first was Antigravity's executable name.

- **Codex hook trust is its own prompt, separate from workspace trust.** On
  first seeing a project's `.codex/hooks.json` it says "Hooks need review — N
  hooks are new or changed. Hooks can run outside the sandbox after you trust
  them", offering `Review hooks` / `Trust all and continue` / `Continue without
  trusting (hooks won't run)`. So the project-local design works with **no
  user-level write at all**: the session is a real harness in a visible
  viewport and the user answers there. The `[hooks.state…]` hash entries are
  what that answer records.

- **Codex hooks are observed firing, and their payloads are captured.** With a
  project-local `.codex/hooks.json` trusted and one real turn taken,
  `SessionStart`, `UserPromptSubmit` and `Stop` all fired. Codex also printed
  `⚠ clamping SessionEnd hook timeout to 3s in <project>/.codex/hooks.json`,
  which names the file and proves it was read — and warns that **Codex clamps
  hook timeouts**, so a declared timeout may be silently shortened.
  Note `SessionStart` *does* fire for Codex, unlike Claude Code.

  Every payload carries `session_id`, `transcript_path`, `cwd`,
  `hook_event_name`, `model` and `permission_mode`; `UserPromptSubmit` adds
  `turn_id` and `prompt`; `Stop` adds `turn_id`, `stop_hook_active` and
  `last_assistant_message`. Full schema in
  `.agent-runtime/notes-codex-hooks.md`.

- **A hook is a better identifier source than a rollout scan, and Phase 8 line
  2 stays anyway.** `session_id` is in every payload, handed over directly with
  none of the originator/parent/cwd/time-window filtering that discovery needs —
  `transcript_path` even names the exact rollout. But hooks require
  installation *and* the user trusting them, while discovery needs nothing and
  works for a session that predates the hooks. Prefer the hook's `session_id`
  when one has reported; fall back to discovery otherwise.

- **The hook payloads carry conversation content.** `prompt` is the user's own
  words and `last_assistant_message` is the model's reply. A Glasshouse hook
  handler needs `session_id` and `hook_event_name` and must read neither of the
  others into a log, a diagnostic, a `Debug`, or the database. Make it a test,
  the way `nothing_is_read_past_the_first_line` already does for rollouts.

- **Never steer a user toward "Trust all and continue".** Doing so during this
  probe trusted five unrelated `warp@claude-code-warp` plugin hooks that
  happened to be pending review, writing them into the user's `config.toml`.
  Restored byte-identical from a backup. It is a blanket action over whatever
  else is pending; "Review hooks" is the honest path.

- **Windows CI caught a real production defect on the first push, again.**
  `read_first_line` required a trailing newline, so a rollout whose only line
  was its header — which is what a harness writes before it has anything to
  append — was discarded and the session reported no identifier. Linux and
  macOS passed; only Windows exercised a fixture written without the newline.
  Fixed, with `a_header_with_no_trailing_newline_is_still_read` writing the
  bytes directly rather than through the helper that appends one. **Every one
  of the eight original unit tests went through `write_rollout`, which appends
  `\n` — a shared fixture helper is a shared blind spot.**
- **No *live* Codex turn has had its identifier captured end to end.** The
  header format is proven against 555 real rollouts and the wiring against the
  shipped binary with a fake harness; what is unproven is only the join between
  the two on a real turn, which costs model usage. Worth doing once,
  deliberately, when a turn is being spent anyway.
- **A Codex session that takes no turn gets no identifier, forever.** That is
  correct — there is nothing to resume to — but it means a Codex session the
  user opened and closed without prompting reads as `closed`, not `resumable`,
  and the reason is invisible in `glasshouse sessions`.
- **Two Glasshouse Codex sessions started in the same project within the same
  window will both refuse to record an identifier**, because each sees the
  other's rollout as a second candidate. Fail-closed and honest, but a real
  usability edge if anyone runs parallel Codex sessions in one project. The fix
  is a narrower discriminator, not a ranking rule.

- The `fullscreenAutoDisabled` record this defect left in the user's
  `~/.claude.json` is **cleared**. `/tui fullscreen` was run in a real Claude
  Code session at the user's explicit request — they could not run it
  themselves, being on Remote Control, where `/tui` is unavailable — and Claude
  Code confirmed "Using flicker-free rendering". The fix and the repair are
  both verified; nothing edited the configuration file directly.
- **The terminal handshake is verified on macOS only.** The queries and replies
  are platform-independent and their tests run everywhere, but no real harness
  has been driven through the viewport on Windows.

- **Permission detection is the one hook line still open.**
  `PermissionRequest` is installed, translated, and proven to move the record
  when its command runs, but Claude Code firing *that* event has not been
  watched: the verifying turn needed no permission, and this machine runs
  Claude Code in auto mode, where a prompt that would ask is approved without
  asking. Closing it needs an isolated configuration with approvals required
  and a prompt that wants to run something.
- **Compaction is blocked by the harness.** Claude Code 2.1.245 exposes no
  compaction hook — the events a real installation accepts are the ten
  recorded in the adapter, none about compaction. Codex *does* expose
  `pre_compact`/`post_compact`, so Phase 8's equivalent is reachable and this
  one is not. Revisit when a release exposes one.
- **Hook firing is verified on macOS only.** The document and the reporting
  command are platform-independent and tested everywhere; Claude Code's own
  hook execution on Windows is not.
- **A new project directory makes Claude Code ask the user to trust the
  workspace.** An embedded session will show that prompt in the viewport,
  which is correct — native prompts stay interactive — but it means a session's
  first screen may be a question rather than a prompt box.

- **Anything configured as `claude-code` now receives `--session-id`.** Before
  this session Glasshouse passed no arguments at all, so any executable
  worked. A user pointing that integration at a wrapper script now needs the
  wrapper to pass its arguments through. This is correct — the flag belongs to
  the harness the user named — but it is a real change in blast radius.
- **A stopped session reads as resumable on the strength of an assigned
  identifier**, not on proof that a conversation exists. If a harness starts
  and dies before creating one, the harness refuses the identifier — Claude
  Code answers "No conversation found with session ID: …" and exits, which was
  observed directly. That is a clear failure rather than lost state or a blank
  session wearing an old name, and `Failed` sessions are never resumable; but
  it is optimism, and the resume command should surface the harness's own
  refusal rather than dressing it up.

- **Phase 6's communication-style line stays unchecked.** Six of seven
  adapters declare `Unverified` because their installed binaries document no
  such mechanism — Codex 0.149.0 in particular exposes no "personality",
  though the capability map names one as its example. `StyleChange::InPlace`
  therefore has no instance: Claude Code's output style is declared
  `NewSession` because the mechanism Glasshouse can drive, a settings document
  read once at startup, is fixed for the life of the process. Closing this
  needs one verified in-place mechanism, or a second harness with any verified
  native mechanism.
- **`resume`, `message` and `interrupt` have no production caller.** They are
  declared and unit-proven. Resuming belongs to Phase 7/8; messaging and
  interrupting to Phase 13/14. Line 3 asks an adapter to *expose* the resume
  command, which it does — executing one is a later line, and is not claimed.
- **No adapter parses harness output yet**, so the isolation guard for line 12
  currently protects a property nothing is pushing against. Installing it
  before Phase 7 rather than after is the point.
- **DeepSeek Harness waits for Phase 9A.** It is installed and its launcher
  interface is verified, but it ships no interactive terminal profile, and its
  own profile concept is Phase 9A's launch profiles under another name.
- **Pi is installed but not on `PATH`** on this machine (npm's global prefix
  is `~/.hermes/node`, which is not in `PATH`), so `glasshouse doctor` reports
  it as not found with `candidates tried: pi`. That is correct behaviour, and
  a good live example of why a configurable explicit executable path exists.
- The rustdoc baseline recorded in earlier revisions of this file as "15
  pre-existing diagnostics" was **wrong**: measured against `HEAD` in a clean
  worktree it is **23**. This session added none — it briefly added two
  ambiguous doc links (`crate::session::select` is both a function and a
  module) and both were fixed to `mod@` form before commit.

- **The shell's key bindings are plain single keys**, because no native session
  owns the keyboard yet. When one does (Phase 5) they must move behind a prefix
  or a mode, or they will steal keystrokes the harness needs.
  `ShellState::handle_key` is deliberately the only place that has to change.
- The shell reads sessions once at startup and on an explicit redraw event.
  Nothing yet raises that event, so a session started elsewhere while the shell
  is open does not appear until it is reopened. `AppEvent::Redraw` and
  `ShellState::refresh` are the seam, and `refresh` already reconciles by
  identifier rather than index.
- The viewport is reserved and empty. Phase 5 fills it.
- **Open question on Windows: does a bare carriage return satisfy a real
  harness?** `encode` sends `\r` for Enter, which is what a terminal sends. The
  Windows *fake* harness reads with `set /p`, which wants CRLF, so the shell's
  end-to-end round-trip test is Unix-only. Making `encode` emit CRLF would be
  wrong — every Unix harness would get a spurious extra newline per keystroke —
  and the harnesses Glasshouse actually targets read raw input and accept CR.
  But that is reasoning, not evidence; confirm it against a real harness on a
  real Windows install. The forwarding path itself is covered on Windows by
  `keystrokes_reach_the_focused_session` at the runtime layer, and the shell's
  mode machinery by `the_shell_enters_and_leaves_session_mode_in_a_real_terminal`.
- `session::runtime` (`SessionRuntime`) exists and is proven against real
  processes on all three platforms, but **has no production caller yet**, so
  seven Phase 4 boxes stay unchecked. `GLASSHOUSE_DESIGN_DECISIONS.md`
  records the decision that unblocks it: the shell's single-key bindings cannot
  coexist with forwarding every keystroke to a harness, so control mode and
  session mode split, with `Ctrl-]` as a single-chord escape.
- `SessionRuntime::is_running()` reports the status cached by the last
  `poll_exits`, not a fresh answer from the operating system. That is honest —
  it is documented as observation-based — but it caught a test out: a mutation
  killing every session on `close` stayed green because the survivor had not
  been polled since. Any test asserting liveness must poll first.
- Exit detection cannot currently be proven independent of output *on macOS*.
  The discriminating case needs a process that exits while its output stream
  stays open, and a direct probe showed macOS reports end-of-file on the
  pseudo-terminal master as soon as the foreground child exits, even with a
  background child still holding the slave. The capability's real risk — a
  silent-but-running harness mistaken for a finished one — is covered.

- **Nothing calls `open_for_resume` in production.** The cross-project resume
  guard is implemented, structurally enforced, and mutation-proven, but there
  is no `glasshouse resume`, so Phase 1 line 90 is `PARTIALLY VERIFIED` and its
  box stays unchecked. The adapter it was waiting for now exists — every one of
  the seven exposes a verified resume invocation — but a `glasshouse resume`
  built today could still only report "not resumable", because no adapter
  *captures* a native identifier yet. That is Phase 7/8, and the earlier
  instruction stands: do not close line 90 with a command that can only say no.
- No harness adapter captures a native session identifier yet, so in production
  `sessions.native_session_id` is always `NULL` and no session ever reaches the
  `Resumable` disposition. The mechanism is complete; what feeds it is Phase
  7/8.
- Only `Embedded` presentation occurs in production, because `glasshouse
  launch` is the only session producer. `Headless` and `External` arrive with
  Phase 4 and Phase 17.
- `glasshouse sessions` has no filtering, no sorting options, and no way to
  remove a record. Phase 11 owns the real overview; this is the minimum that
  makes the stored metadata observable.

- The forced-exit orphan is **fixed**: an attached session registers a cleanup
  that `shutdown`'s force path runs before `process::exit`. It is best effort
  by construction (`try_lock`, never `lock`) because a cleanup that waits could
  hang the one escape hatch whose purpose is to always work. If the lock is
  held at that instant the harness is still orphaned — no worse than before,
  and the alternative is a Glasshouse that will not die.
- `session::attach` owns the process's terminal for its whole life: its stdin
  pump cannot be cancelled, so the process exits out from under it. The
  multi-session TUI will need a different input path.
- Native Windows UNC project roots remain refused; `cmd.exe` cannot reliably
  hold a UNC working directory.
- Antigravity detection is **resolved**: a real Antigravity CLI 1.1.20 was
  installed and `glasshouse doctor` reports it. The executable name was wrong —
  the published package links its binary onto `PATH` as `agy`, not
  `antigravity` — so nothing would ever have detected it. Both names are
  searched now. cmux control-environment and Ollama configured-endpoint
  detection remain implemented and checked.
- The UNC refusal's *premise* — that `cmd.exe` substitutes the Windows
  directory rather than failing — is documented Windows behaviour, not
  something a live run confirmed. No real UNC share was exercised; the refusal
  itself is platform-independent and runs in CI everywhere.
- `IntegrationId::minimum_version()` returns `None` for every integration, so
  unsupported-version classification exists but is unreachable. Declaring a
  real minimum needs verified release data this environment does not have.
- The main session TUI, session metadata schema, harness adapters, durable
  memory table, and session persistence are not implemented.
- Strict rustdoc still fails on 15 pre-existing lib-doc diagnostics, 9 of them
  public docs linking to private items. The count in an earlier revision of
  this file said 12 and was simply wrong; this session added none, verified by
  measuring the baseline with the branch stashed.
- The cross-harness completion protocol remains design documentation. This
  session used its durable-file half — each worker wrote
  `.agent-runtime/report-<TASK-ID>.md` — with manual visible pane polling and
  no automatic wake, exactly as the protocol prescribes until its safety tests
  exist.

## Where to go next

**Phase 8 line 3 — resume a Codex session — is next, and its real-binary
evidence is already gathered.** Both halves were probed this session against
the installed Codex 0.149.0, in a pseudo-terminal, and **neither cost a model
turn**:

- `codex resume 01a03537-903b-73d2-9886-7799d4c90376` (a real interactive
  session recorded for this project) printed `Resuming session…` and replayed
  the conversation — 164 KB of it. The identifier Glasshouse would have
  recorded is the one Codex reopens.
- `codex resume 9f1c0b2e-0000-4000-8000-0123456789ab` answered
  **`ERROR: No saved session found with ID <id>. Run `codex resume` without an
  ID to choose from existing sessions.`** That is the harness's own refusal,
  and it is what `resume_session` should surface rather than dress up.

Two practical notes for whoever writes the test. The pseudo-terminal **must be
given a window size** (`TIOCSWINSZ`); without one Codex emits its handshake and
draws nothing, which looks exactly like a hang. And Codex may open with an
**update prompt whose default option runs `curl … | sh`** — never dismiss it
with Enter; the explicit "Skip" digit is safe.


`resume_session` in `main.rs` is entirely generic: it selects the harness the
record names and calls `selection.resume_args(...)`, which goes through the
adapter, and `Codex::resume` already returns `["resume", <id>]`. The only thing
that was missing was an identifier, and that now exists. What line 3 still
needs is *evidence*: drive `codex resume <uuid>` against a real recorded
session in a pseudo-terminal and watch the conversation reopen. There is a real
interactive rollout for this project to try it against
(`01a03537-903b-73d2-9886-7799d4c90376`), and reopening a conversation replays
history without spending a model turn — the same shape as the Phase 7 evidence.
Also surface the harness's own refusal for an unknown identifier rather than
dressing it up.

**Phase 8's hooks lines (5-9) — the decision is made: project-local, always.**
The user settled it on 2026-08-25: *"always project local configuration of
agent harnesses — what if someone does not want to use Glasshouse somewhere
else and has its hooks still configured?"* Full reasoning, and the alternative
it rules out, are in `GLASSHOUSE_DESIGN_DECISIONS.md`.

So Codex hooks go to `<project>/.codex/hooks.json`, which is inside the user's
repository and therefore needs Phase 2D's explicit consent — show the exact
path, require its own confirmation, leave nothing behind on cancel.

Claude Code already satisfies the rule and does not change: its hook document
lives in Glasshouse-owned state and is passed per session with `--settings`.

**The trust question is settled too.** Writing Codex's trust record into the
user-level `~/.codex/config.toml` is **allowed**. The rule is not "never touch
a user-level file" — it is that hooks must not end up running in every session
globally, only in the folder where they are configured. A trust entry cannot
cause that: it is keyed by absolute path and bound to a content hash, so it
grants permission for one file at one path for one event and installs nothing.
Hooks run only where a `.codex/hooks.json` exists.

**The file that *would* make them global is `$CODEX_HOME/hooks.json`**, and
Glasshouse must never write it. That is a different file from `config.toml`
and a different line entirely. It is also not hypothetical: a previous tool on
this machine had exactly that file configured with seven events, firing in
every repository, and it was found and removed on 2026-08-25.

`--dangerously-bypass-hook-trust` stays rejected: it bypasses trust for every
enabled hook including unvetted project ones, trading a scoped grant for a
blanket one.

Still worth probing: whether Codex prompts for hook trust interactively. If it
does, prefer that — the session is a real harness in a visible viewport, the
user answers it there, and Glasshouse writes nothing at all.

**A specification change was made this session, deliberately and on the
record.** The capability map gained **three** lines (1331 -> 1334 unchecked):
one in Phase 6, two in Phase 9A. Reason, impact and the full evidence table are
in `GLASSHOUSE_DESIGN_DECISIONS.md` under "Approvals".

The short version. The user's position is that per-command approval is theatre
— *"no normal human reads a regex lookup with routing into /dev/null and a sed
with 16 parameters and understands what's happening"* — and that Claude Code's
auto-mode classifier should be the norm everywhere. Correct, and it does **not**
require Glasshouse to build a classifier, which the map forbids in a fixed
architectural requirement and in already-checked line 267.

It requires Glasshouse to *select* the harness's own mode, because three of the
seven already ship one: Claude Code's auto mode, Codex's `--approve-for-me`
("automatic review using the workspace-write sandbox"), and Cursor's
`--auto-review` ("a server classifier auto-runs safe tool calls"). OpenCode,
Hermes and Antigravity offer only a blanket bypass; Pi is unverified because it
is still not on `PATH`.

So adapters declare their approval modes like any other fact, a launch profile
picks one, **the default is automatic review, never bypass**, and a profile
asking for automatic review from a harness that has none is refused rather than
quietly downgraded. Blanket bypass stays available as a profile the user picks
on purpose.

**Also closed: Codex does prompt for hook trust interactively** — confirmed by
the user. That was the open probe from earlier in this session. It means the
project-local hooks design works with no user-level write at all if the prompt
is acceptable; but the same user finds trust prompts annoying, which is exactly
why the trust-grant route stays available and scoped.

Still blocked, unchanged:

- Phase 1 line 92 and Phase 3's memory view — Phase 20's memory table.
- Three Phase 4 lines — unfocused `send_text`, `interrupt`, headless sessions.
- Eleven Phase 2D lines — Providers, Launch Profiles, Routing, Memory sections.
- Real minimum harness versions.
- Phase 2C onboarding — product decisions that need the user.
- Phase 6's communication-style line — needs one verified in-place mechanism.

## Active worker tasks and results

**One worker, and delegation was the point.** GH-P08-SESSIONID went to a
Sonnet 5 implementer running as `claude --model sonnet` in a visible cmux
surface, in its own worktree (`sonnet/codex-session-id`), against a task packet
carrying the evidence table, the settled design, the expected and forbidden
files, and ten named acceptance tests. It produced roughly 660 lines across
seven files.

What it got right, and what needed a second pass:

- It implemented the design as specified and did not redesign it.
- **It corrected the packet on one point and was right**: the consistency test
  between `session_id_source` and `SessionIds::Discoverable` has to be
  one-directional, because four adapters legitimately declare `Discoverable`
  with no reader built yet. Verified before accepting.
- It **named the coverage gap it could not close** — its first end-to-end test
  covered only the `main.rs` call site — rather than hiding it. That is the
  behaviour the packet asked for and got.
- Review sent back two things: an adapter importing `crate::session` (a new,
  wrong-way dependency), and the missing shell-loop test. Both were fixed.

The orchestrator wrote the design decision, ran every gate independently, ran
all eight mutations, checked the box, and made the commit. The worker committed
nothing and touched no project record.

Practical notes for the next session, beyond the routing rules: a worker's
report file appears *before* it stops working, so gate review on the pane going
idle and diff a frozen tree. `cmux send` **replaces** the composer buffer
rather than appending to it. And launching a `--dangerously-skip-permissions`
session from inside an auto-mode Claude Code session is blocked by the
classifier — the plain launch inherits auto mode anyway, which was enough.

## Commands run and outcome

- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets
  --all-features -- -D warnings`, `cargo test --workspace --all-features`
  (399 lib + 2 bin + 52 PTY smoke + 4 settings = **457**), `git diff --check`
  — all pass, run by the orchestrator on the frozen tree, not taken from the
  worker's report.
- `rustup run 1.85.0 cargo check --locked --workspace --all-targets` — passes.
- `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps` — **23**
  diagnostics, exactly the recorded baseline. None added.
- Both new PTY tests confirmed to *execute* on this platform by listing and
  running them by name, not inferred from an aggregate green.
- Eight mutations, eight kills (see the session record above).
- Real-binary probes, none costing a model turn: all 555 rollout headers
  parsed and tabulated; `codex --help`, `codex resume --help`; bare `codex`
  started and killed under an isolated `CODEX_HOME` (no `sessions/` created);
  `CODEX_HOME` relocation confirmed; `--strict-config` probed and recorded as
  unable to answer the hooks question.

## Next exact step

Hand this checkpoint to Opus:

> Start with `git status`, `git log -5`, this handoff,
> `GLASSHOUSE_DESIGN_DECISIONS.md`, and `.agent-runtime/CONTINUATION.md` —
> whose Part 1 is generic standing rules, including re-arming the context and
> usage-window watches, which do not survive a session. Pushing to run CI is
> standing authorization.
>
> **Phase 8 line 3, resuming a Codex session, is next**, and the code for it
> already exists — what it needs is evidence from the real binary. See "Where
> to go next".
>
> The habits that earned this session's results:
>
> - **Read the whole corpus, not one example.** The rule for identifying a
>   Codex session came from all 555 rollout files; the two facts that mattered
>   — subagents outnumbering real sessions 171 to 70, and `session_id` missing
>   from 28 — are both invisible in any single file, and the previous
>   session's plan would have shipped on the strength of one.
> - **Delegate the implementation, keep the design and the evidence.** That
>   worked: a Sonnet worker in a visible cmux pane produced ~660 lines against
>   a settled packet, and the orchestrator's context stayed well under half.
> - **Never path-wide `git checkout` in a worker's worktree.** Workers do not
>   commit, so their whole contribution is uncommitted, and a restore aimed at
>   one mutated line took out five files.
> - **A mutation harness needs its own verification.** The first one reported
>   "survived" for every mutation because it read the wrong result line.
> - **Verify the worker's gates yourself, on a frozen tree.** The report was
>   written while the worker was still editing.
