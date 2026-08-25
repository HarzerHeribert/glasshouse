# Glasshouse implementation handoff

Last updated: 2026-08-25 (Europe/Berlin)

## Current capability / phase

**Phase 6 is eleven of twelve and Phase 7 has begun.** The checked count went
from 107 to 120.

- **Phase 2B** — Antigravity detection: **closed**, against a real CLI.
- **Phase 6** — harness adapters: eleven of twelve. The communication-style
  line is deliberately unchecked; see the loose ends.
- **Phase 1 line 90** — the cross-project resume guard: **closed after three
  sessions blocked.** `glasshouse resume` is its production caller.
- **Phase 8** — Codex adapter: three of ten lines closed. Codex starts, its
  interface renders in the viewport, and Glasshouse depends on none of its
  internals.
- **Phase 7** — Claude Code adapter: **eight of ten lines closed.** The two
  that are not are honest gaps, not unfinished work: permission detection needs
  a permission prompt to fire (this machine runs Claude Code in auto mode), and
  Claude Code 2.1.245 exposes no compaction hook at all.
- **A defect that was damaging the user's installation is fixed.** Glasshouse
  answered only one of the three questions a harness asks its terminal at
  startup, so Claude Code's fullscreen renderer failed and, after two strikes,
  the harness turned it off *globally* — overriding the user's own
  `"tui": "fullscreen"` setting. Glasshouse
  **assigns** Claude Code its native session identifier with `--session-id`,
  mints it as a valid version-4 UUID, and records it before the process
  exists — and the whole chain is verified against the real binary.

**Sessions reach `Resumable` in production for the first time.** A cleanly
stopped Claude Code session now has something to resume to, which closes a
loose end that had been open since Phase 2.

Glasshouse now reaches every supported harness through one contract.
`glasshouse doctor` prints what each adapter declares — vendor, resume
command, hooks mechanism, session-identifier discovery, capabilities,
protocols, model override — and every one of those facts carries the evidence
it was read from.

### Seven harnesses, each verified against a real install

Claude Code 2.1.245, Codex 0.149.0, Antigravity CLI 1.1.20, OpenCode 1.18.22,
Cursor CLI 2026.08.11, Pi 0.73.1, Hermes Agent 0.15.1.

Cursor, Pi and Hermes were added this session at the user's request, for
subscription pooling — Hermes manages pooled provider credentials natively.
All three were installed and interrogated before any declaration was written.

**Two that were asked for and deliberately not shipped**, both recorded in
`GLASSHOUSE_DESIGN_DECISIONS.md`:

- **DeepSeek Harness** (`@deepseek-ai/dsh` 0.1.1-rc.2) is installed and runs,
  but it is a *profile launcher* whose shipped profiles are a browser UI and a
  one-shot headless runner. It has no official interactive terminal profile —
  confirmed against both the installed package and DeepSeek's own repository.
  A browser UI is not something a viewport can hold, and DSH's own "profile"
  concept is Phase 9A's launch profiles under another name, so an adapter
  written now would be rewritten then.
- **ZCode** (Z.ai) is a desktop application with no CLI, so there is no
  executable for an adapter to start.

### The decision this phase rests on

Every fact an adapter states is a `Declared<T>`: `Verified { value, evidence }`
with the source named concretely enough to re-check, or `Unverified`. There is
no third state meaning "probably", and
`every_verified_declaration_cites_its_evidence` fails a `Verified` whose
evidence is too thin to be a citation.

**It paid for itself immediately.** Glasshouse searched `PATH` for
`antigravity` and would never have found a real install: the published
Antigravity CLI links its binary onto `PATH` as **`agy`**. The old single-name
list was carefully reasoned, documented at length, and pinned by a test — and
simply wrong, because no reference install had ever been inspected.

## Verified completed work

### This session — Codex, and a question it asks that Claude Code does not

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

### This session — the hooks, observed firing for real

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

### This session — answering the terminal's questions

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

### This session — Claude Code lifecycle hooks

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

### This session — `glasshouse resume`

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

### This session — assigned native session identifiers (Phase 7)

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

### This session — the harness adapter interface

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

**Phase 7 — the Claude Code adapter — is next in map order and unblocked.**
Phase 6 gives it the contract; Phase 7 is where the Claude Code adapter starts
doing rather than declaring:

- capture the native session identifier. Claude Code is the easy case and the
  adapter already declares why: `--session-id <uuid>` lets Glasshouse *assign*
  one, so the identifier is known before the process exists rather than
  discovered afterwards. That is what finally lets a record reach the
  `Resumable` disposition.
- then resume through it, which is also what unblocks **Phase 1 line 90** — the
  cross-project resume guard that has been complete and unreachable for two
  sessions.
- then lifecycle hooks. The adapter declares the mechanism (a `hooks` section
  in a settings document, suppliable per session with `--settings`) and nine
  verified event names.

Phase 8 (Codex) follows the same shape with different mechanics: a `resume`
subcommand, identifiers discoverable from rollout files under
`$CODEX_HOME/sessions/...`, and hooks in a project-local `.codex/hooks.json`
that is trust-gated per project.

Still blocked, unchanged:

- Phase 1 line 92 and Phase 3's memory view — Phase 20's memory table.
- Three Phase 4 lines — unfocused `send_text`, `interrupt`, headless sessions.
  All three work and are tested; they need callers, which are Phase 14's.
- Eleven Phase 2D lines — Providers, Launch Profiles, Routing, Memory sections.
- Real minimum harness versions.
- Phase 2C onboarding — product decisions that need the user.

## Active worker tasks and results

**No workers were used this session.** The work was a single coherent design —
seven adapters whose every declaration had to be derived from a real binary —
and the risk in it was precisely the judgment a worker tier is not for. The
orchestrator wrote it, ran every gate, and mutation-checked its own tests.

The tiers and their routing are unchanged; see
`GLASSHOUSE_WORKER_CAPABILITIES.md`. The practical notes from earlier sessions
still hold: give an editing worker its own worktree and a task packet with real
context, start Ox as plain `ox` and type into its visible TUI, and read a
surface before sending anything to it.

## Commands run and outcome

- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets
  --all-features -- -D warnings`, `cargo test --workspace --all-features`
  (385 lib + 2 bin + 50 PTY smoke + 4 settings), `rustup run 1.85.0 cargo
  check --locked --workspace --all-targets`, `git diff --check` — all pass.
- `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps` — 23
  diagnostics, exactly the baseline measured at `HEAD` in a throwaway
  worktree. None added.
- Twenty mutations. Nineteen failed the test they targeted; one **passed**,
  and exposed a test that could not reach the guard it was supposed to cover.
  A test was written to reach it, and the mutation now fails. Two further
  mutation attempts produced *no output at all* — a build failure, not a
  result — and were redone properly rather than counted.
- CI `32835774698` green on Linux, macOS, Windows and lint, with the Windows
  job confirmed to have executed 346 lib, 2 bin, 33 PTY and 4 settings tests
  rather than merely reporting green.
- `glasshouse doctor` run from the built binary, which is how two rendering
  defects were found and how Antigravity detection was confirmed against a
  real install.
- Harnesses installed this session, all without `sudo`:
  `brew install --cask antigravity-cli` (links its binary as `agy`),
  `brew install --cask cursor-cli` (links `cursor-agent`), and
  `npm install -g @deepseek-ai/dsh @mariozechner/pi-coding-agent`.

## Next exact step

Hand this checkpoint to Opus:

> Start with `git status`, `git log -5`, this handoff,
> `GLASSHOUSE_DESIGN_DECISIONS.md`, and `.agent-runtime/CONTINUATION.md` —
> whose Part 1 is generic standing rules, including re-arming the context and
> usage-window watches, which do not survive a session. Pushing to run CI is
> standing authorization.
>
> **Phase 7, the Claude Code adapter, is next.** Phase 6 landed the contract
> and seven adapters; Phase 7 is where one of them starts doing rather than
> declaring. Start with the native session identifier — `--session-id <uuid>`
> means Glasshouse can assign one rather than discover it — because that is
> what unblocks resume, the `Resumable` disposition, and Phase 1 line 90.
>
> The habits that earned this session's defects:
>
> - **Derive every harness fact from the installed binary.** The one fact that
>   had been reasoned rather than read — Antigravity's executable name — was
>   wrong, and had been wrong, documented, and test-pinned for weeks.
> - **Drive the shipped binary.** Both defects this session were in output no
>   test reads.
> - **Assert against a specific row or field, never a whole screen.** The
>   doctor-report test failed first because a `skip_while` found the harness
>   *list* rather than the adapter *block* — the same class of mistake the
>   records warn about, caught by the test being specific enough to notice.
> - **A mutation that does not fail is information about the code.** Five were
>   run; all five failed as intended.
> - **Check that a new capability has a production caller.** `describe()` got
>   one — `glasshouse doctor` — rather than shipping as a data structure
>   nothing reads.
