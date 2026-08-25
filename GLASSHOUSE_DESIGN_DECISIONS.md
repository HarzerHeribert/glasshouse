# Glasshouse design decisions

Decisions that shaped the implementation and would be expensive to rediscover.
Each records the conflict, the choice, and the reasoning — not just the outcome,
because the reasoning is what a future change has to argue against.

The capability map says *what* Glasshouse must do; this says *why* it does it
the way it does.

---

## Session modes — how the shell owns a keyboard it must also give away

### The conflict

Phase 3 gave the shell single-key bindings: `q` quits, `o` opens the overview,
Tab moves between sessions. Phase 4 requires "forward user keystrokes from the
active Glasshouse session to the active PTY". Both cannot be true at once: a
harness needs `q`, `o` and Tab far more than Glasshouse does, and a shell that
swallows them is a shell that breaks every harness it hosts.

This was flagged when Phase 3 landed ("bindings are plain single keys because no
native session owns the keyboard yet") and is now due.

### The decision: two modes, one escape hatch

**Control mode** (the default, and where the shell starts). Glasshouse owns the
keyboard. Today's bindings all work unchanged. Nothing is forwarded.

**Session mode.** Every keystroke goes to the focused PTY untouched — including
`q`, Tab, Ctrl-C, and escape sequences — except one reserved chord that returns
to control mode.

- Enter **session mode** from control mode with `Enter` or `i`.
- Leave it with **`Ctrl-]`** (0x1D).

Why `Ctrl-]`: it is what `telnet` has used for decades, it is not produced by
any ordinary key, and no common harness binds it. It must be exactly one chord —
a prefix that needs a second key (tmux's `Ctrl-b`) doubles the latency of the
escape and is worse when the thing you are escaping is a runaway session.

Ctrl-C is deliberately NOT the escape. In session mode Ctrl-C belongs to the
harness, which is the entire reason `RawModeGuard` exists rather than
`TerminalGuard`.

### What each mode does to the rest of the interface

- The status bar shows which mode is active and the escape chord. A user who
  cannot see how to get out is the failure this design exists to prevent, so
  the chord is on screen in session mode at all times.
- Overlays are control-mode only. Opening one from session mode is impossible
  by construction; leaving an overlay returns to the mode you were in.
- Focus (which session) and mode (who owns the keyboard) are independent.
  Changing focus in control mode never enters session mode.

### Invariants a test must hold to

1. In session mode, `q` reaches the harness and does not quit Glasshouse.
2. In session mode, Ctrl-C reaches the harness as a byte, and does not quit.
3. `Ctrl-]` returns to control mode from session mode, and is never forwarded.
4. Entering and leaving session mode never touches any process: same pids, all
   still running, and the session's scrollback keeps growing throughout.
5. With nothing focused, session mode cannot be entered — there is nowhere to
   send the keys.
6. A session exiting while in session mode drops back to control mode rather
   than leaving keystrokes going nowhere.

### Where it goes

`ShellState` gains a `mode: Mode` and `handle_key` branches on it first, before
any binding is consulted. That keeps the whole decision in one place, which is
what the Phase 3 module documentation promised would be the only thing that had
to change.

### Postscript: the chord has two spellings

`Ctrl-]` is the byte `0x1D`, and the two platforms' parsers name it
differently. Crossterm's Unix parser decodes the control range `0x1C..=0x1F`
arithmetically as `Char((c - 0x1C + b'4'))`, so a real terminal's `Ctrl-]`
arrives as `Ctrl` + `'5'`; Windows reads virtual key codes and gives
`Ctrl` + `']'`.

The first implementation matched only `']'` — which is what the synthetic
`KeyEvent` in its unit tests looked like — so on Unix the escape never fired and
a user entering session mode had no way back. Exactly the failure this design
exists to prevent, shipped past a full unit-test suite, and caught only by
driving the real binary through a real pseudo-terminal.

`is_session_escape` accepts both spellings. Do not "tidy" it to one.

---

## Settings — what a settings view may contain before the settings exist

### The conflict

Phase 2D lists twenty settings capabilities: Harnesses, Providers, Launch
Profiles, Routing, Memory, Integrations. Four of those six sections configure
features that do not exist — providers are Phase 9C, launch profiles 9A,
routing 9I-9K, memory Phase 20. A settings view containing all six sections
would be four-sixths empty shells.

### The decision: build only the sections whose feature exists

The settings view ships with **Harnesses** and **Integrations**, and nothing
else. The remaining four sections arrive with the features they configure, and
their map boxes stay unchecked until then.

An empty "Providers" section is worse than an absent one. It tells the user a
capability exists, invites them to look for it, and leaves the next
implementer a shape to fill rather than a design to make — which is exactly how
a settings screen accumulates dead controls.

### The decision: writes default to the user layer, and the project layer needs consent

Glasshouse's project-level file lives at `<root>/.glasshouse/config.toml`, which
is **inside the user's repository** and will be seen by their version control,
their diffs, and their colleagues. Writing there is not a preference change; it
is a modification to their project.

So:

- Every edit in settings applies to the **user** layer by default.
- Writing the project layer is a separate, explicit action that first shows the
  exact path to be created and requires a distinct confirmation.
- Cancelling leaves no file. Not an empty file, not a directory — nothing.

`config::write_project_config_with_consent` is the only writer, and its name is
a contract with the caller: the consent is the *caller's* job to obtain, and
this design says where.

### The decision: provenance is shown, not inferred

`config::Layer` already distinguishes `Project`, `User`, and `Default`. The
settings view shows which layer supplied each value, because "why is this
harness pointing somewhere unexpected" is the question a settings screen exists
to answer, and a value with no provenance cannot answer it.

### Invariants a test must hold to

1. Opening settings does not disturb the session: same process, still running,
   and leaving settings returns to the mode the user was in.
2. Cancelling a project-level write creates **no** file and no directory.
3. Confirming a project-level write calls
   `write_project_config_with_consent` and creates exactly that one file.
4. A user-level edit never writes into the project root.
5. Every displayed value carries its layer.
6. No secret value is rendered — structurally guaranteed, since no
   configuration type has a field able to hold one.

---

## Terminal emulation — who answers the terminal's questions

### The decision: `vt100`

Phase 5 requires rendering a harness's ANSI output "faithfully enough for
native Claude Code and Codex TUIs to remain usable". That needs a terminal
emulator. Four were considered:

- **`vt100`** — parses terminal data into a screen grid. **Chosen.**
- `alacritty_terminal` — Alacritty's production state machine. More
  battle-tested, materially heavier.
- `termwiz` — WezTerm's; brings input handling and widgets that duplicate
  Crossterm and Ratatui.
- `vte` — parser only, no grid; the screen model would be hand-written, which
  is where subtle rendering bugs live.

`vt100` costs three new crates (`vt100`, `vte`, `arrayvec` — its other
dependencies were already present), taking the tree from 138 to 141, and holds
the 1.85 MSRV. That fits Phase 0's fixed requirement to keep the dependency set
limited to what terminal UI, PTYs, serialization, SQLite and process control
need.

The escape hatch is deliberate: both `vt100` and `alacritty_terminal` produce a
cell grid, and the viewport renderer is the only consumer. If fidelity proves
insufficient against a real harness TUI, swapping is a bounded change to one
module rather than an architectural reversal.

### The consequence: embedded sessions invert the DSR rule

`session::attach` is a **pass-through**. Its module documentation is explicit
that Glasshouse must *never* answer the ConPTY startup handshake
(`ESC[6n` → `ESC[<row>;<col>R`), because the user's real terminal is on the
other end and will answer it. Two replies would reach the harness as input.

An **embedded** session is the exact opposite. Glasshouse is the terminal: the
output goes into a buffer it owns and is redrawn into a viewport, and no real
terminal ever sees the query. If nothing answers, a harness that waits for the
reply hangs — silently, looking like a session that started and did nothing.

So the rule inverts, and both halves must hold:

- **Pass-through session (`attach`): never answer.** Unchanged.
- **Embedded session (`SessionRuntime`): always answer, and never forward the
  query onward.**

That the runtime's existing tests needed a hand-written `DsrTracker` helper to
answer queries on the test's behalf is the evidence that nothing in production
answers them today. Phase 5 is where that stops being acceptable.

### Invariants a test must hold to

1. A harness that emits `ESC[6n` receives exactly one well-formed reply.
2. The reply reports the cursor position of the **viewport**, not the outer
   terminal — the harness is being told about the screen it actually has.
3. `session::attach` still answers nothing.
4. Resizing the viewport resizes the emulator's grid and the child's
   pseudo-terminal to the same dimensions.
5. Colours, cursor position and line wrapping survive a round trip through the
   emulator into Ratatui cells.


---

## Harness adapters — what an adapter is allowed to claim

### The conflict

Phase 6 asks each adapter to declare a dozen facts about its harness: how it
resumes, whether it has hooks, whether its session identifiers can be
discovered, which protocols it speaks, what it can do. Every one of those is
easy to answer from memory and expensive to answer correctly, and a wrong
answer is not inert — it launches the wrong program, resumes the wrong
conversation, or promises the user a capability that is not there.

The previous session's handoff said it plainly: *derive them from the real
binaries; recalling a flag is not evidence.* This decision makes that
mechanical rather than a habit someone has to remember.

### The decision: every declaration is `Declared<T>`, and there is no third state

An adapter's every fact is either:

- `Verified { value, evidence }` — read from the installed harness itself, with
  the source named concretely enough to re-check: a `--help` line, one of its
  own configuration files, a session record it wrote; or
- `Unverified` — nobody could establish it here.

There is deliberately no "probably", no "likely", and no bare `bool`.
`every_verified_declaration_cites_its_evidence` fails on a `Verified` whose
evidence string is too short to be a real citation, so the honesty rule is
enforced by the suite rather than by review.

`Declared<bool>::is_known_present` reads `Unverified` as *false*, because a
caller asking "may I rely on this" must be told no when nobody has checked.
Distinguishing "verified absent" from "not checked" is a `match`, and the two
really are different: one tells a router to avoid a harness, the other tells it
to go and look.

The cost is visible in `glasshouse doctor`, where OpenCode's code-editing
capability reads *unverified* — plainly true of the product, and not
established by anything its installation exposes. That is the right trade. The
map itself asks for capabilities "when known", and an honest gap invites
someone to close it, while a confident guess never gets revisited.

### What this immediately caught

Glasshouse searched `PATH` for `antigravity` and would never have found a real
install. The published Antigravity CLI links its binary onto `PATH` as **`agy`**.
The old single-name list was carefully reasoned, documented at length, pinned by
a test — and simply wrong, because no reference install had ever been
inspected. One `brew install` settled in a minute what the comment had been
hedging for weeks, and the Phase 2B detection box that had been blocked on
exactly this is now closed.

### The decision: the adapter owns the executable name, and nothing else keeps a copy

`IntegrationId::executable_candidates` delegates to the adapter for every
harness. The catalogue keeps names only for cmux, Ollama and llama.cpp — the
integrations that are not harnesses, have no session to start, and so have no
adapter.

Two copies of a harness's executable name would be two places for it to be
wrong, and they would drift. This is also the phase's fixed requirement
("commands ... remain isolated inside adapters") made structural: the name is
the first and most consequential command there is.

### The decision: adapters describe, they never act

An adapter returns descriptions — names to look for, arguments, the bytes that
deliver a message or an interrupt. It never spawns a process, never touches a
`SessionRuntime`, and never parses terminal output.

`the_generic_pty_runtime_depends_on_no_adapter` and
`the_session_model_depends_on_no_adapter` scan the production source of
`pty/`, `session/runtime.rs` and `session/store.rs` for `HarnessAdapter`,
`crate::harness` and `IntegrationId`, and fail if one appears. Comments are
stripped first: `session/store` *documents* that it holds an identifier's
string form, and a scan that punished the comment explaining the boundary
would be teaching the wrong lesson.

No adapter parses anything yet, so today the guard protects a property nothing
is pushing against. Installing it before Phase 7 rather than after is the whole
point — the pressure arrives with the first lifecycle parser.

### Invariants a test must hold to

1. Every `IntegrationKind::Harness` has an adapter; nothing else does.
2. No two adapters claim the same executable name.
3. A resume identifier is its own `argv` entry, and always the last one.
4. A `Verified` declaration cites evidence; an `Unverified` one carries none.
5. The generic PTY runtime and the session model name no adapter.
6. A session's arguments are the adapter's, then the user's.

---

## Which harnesses Glasshouse ships adapters for

### The decision: seven, each verified against a real install

Claude Code, Codex, Antigravity, OpenCode, Cursor CLI, Pi, and Hermes Agent.
The last three were added at the user's request, for subscription pooling —
Hermes in particular manages pooled provider credentials natively — and all
three were installed and interrogated before a single declaration was written.

Adding a harness later is cheap (one adapter module, one catalogue entry);
retrofitting the *interface* around a harness that does not fit is not. That is
why the roster was settled while Phase 6 was being built rather than after.

### Two that were asked for and deliberately not shipped

**DeepSeek Harness** (`@deepseek-ai/dsh` 0.1.1-rc.2) was installed and does
run. It is a *profile launcher*: `dsh --profile <name>` boots a stack of
plugin bundles, and the profiles it ships are `web` (a browser UI at
`127.0.0.1:3080`) and `headless` (a one-shot runner). It ships no interactive
terminal profile — the only one is a community package outside DeepSeek's own
namespace — which was confirmed against both the installed package and the
project's own repository.

Glasshouse embeds *terminal* harnesses in a pseudo-terminal; a browser UI is
not something a viewport can hold. And DSH's own "profile" concept is the same
idea as Phase 9A's launch profiles, so an adapter written now would have to be
rewritten then. It waits for Phase 9A, or for an official terminal profile.

**ZCode** (Z.ai) is a desktop application. Its documentation offers `.dmg`,
`.exe` and `.AppImage` installers and no CLI at all, so there is no executable
for an adapter to start.

Neither is a rejection of the product. Both are the same rule the rest of this
phase runs on: an adapter may only describe something that was actually there
to look at.

---

## Discovering a Codex session's identifier — and why `cwd` is not enough

### The conflict

Claude Code lets Glasshouse *assign* an identifier: `--session-id <uuid>` is
handed over before the process exists, so the identifier is known even if the
harness dies during startup. Codex has no equivalent on its interactive path —
`codex --help` 0.149.0 offers nothing of the kind — so its identifier has to be
discovered afterwards, from the rollout file Codex writes.

The obvious plan, and the one the previous session's checkpoint proposed, was:
read the rollout headers under `$CODEX_HOME/sessions/<yyyy>/<mm>/<dd>/`, match
`payload.cwd` against the project root, take `payload.id`.

**That plan captures the wrong session most of the time.**

### What the real install said

Across the 555 rollout files on the development machine, `payload.originator`
takes four values and one of them is not a user's session at all:

| originator | count | what it is |
|---|---|---|
| `codex-tui` | 241 | the interactive TUI — but see below |
| `Codex Desktop` | 229 | the desktop / VS Code client |
| `codex_exec` | 81 | headless `codex exec` |
| `codex_work_desktop` | 4 | another desktop client |

And of the 241 `codex-tui` rollouts, **171 are subagent threads**, each carrying
a `parent_thread_id`. A subagent's `cwd` is its parent's `cwd`. So in the
directory Glasshouse cares about, records written by subagents outnumber records
written by real sessions **171 to 70**. Matching on `cwd` alone would usually
have recorded a subagent's identifier — and `glasshouse resume` would then
reopen a subagent thread wearing the user's session's name.

### The decision: four conditions, and refuse ambiguity

A record is the session Glasshouse started only if all four hold:

1. `payload.originator == "codex-tui"` — an interactive terminal session.
2. `payload.parent_thread_id` is absent — not a subagent thread.
3. `payload.cwd` canonically equals the project root.
4. `payload.timestamp` falls between when Glasshouse started the session and
   when it observed it end.

On the reference install, conditions 1 and 2 together select **exactly** the 70
real interactive CLI sessions, with zero counterexamples in 555 files. All 70
also carry `source == "cli"`, which is deliberately *not* a fifth condition:
it is corroborating evidence, and every extra condition is another way to break
on a Codex update.

`forked_from_id` is deliberately not disqualifying either. All 128 of its
occurrences are subagents, already excluded by condition 2, and a session made
with `codex fork` is a real resumable session.

**Two or more survivors means Glasshouse records nothing.** Not "take the
newest" — the failure mode of guessing here is resuming a stranger's
conversation, and `session::select` and the resume identifier resolver already
refuse ambiguity for the same reason. A session with no identifier reads as
`closed` rather than `resumable`, which is the honest answer.

### The decision: `payload.id`, never `payload.session_id`

Both fields normally hold the same UUID, and the obvious-looking one is wrong:
`session_id` is present in only **527 of 555** records, while `id` is present in
all 555 and always equals the UUID in the file name. A reader reaching for the
better-named field would silently skip one record in twenty.

### The decision: discovery runs once, when the session ends

Not on a timer, not in a watcher thread. That is when the identifier is needed —
a stopped session is only `Resumable` if it has one — and it is also the moment
the time window is two-sided and therefore tightest. Codex writes no rollout
until a turn has happened (verified by starting it under an isolated
`CODEX_HOME` and killing it: the `sessions/` directory was never created), so
there is nothing to find earlier anyway.

### The decision: only the first line is ever read

The first line is the `session_meta` header. Everything after it is the user's
conversation — their prompts, their file contents, their tool output. Glasshouse
reads one line, capped, and stops. This is a secret boundary, so it is a test
rather than a habit.

### The decision: the adapter parses, core walks

`adapter.read_session_record(&first_line)` is a pure function from text to a
description; `session::native_id` owns the directory walk, the time bound and
the ambiguity rule and knows nothing about Codex. That keeps Phase 6's rule
intact — adapters describe, they never act — while leaving every Codex-shaped
field name inside `codex.rs`.

### The alternative that was rejected

`~/.codex/sqlite/codex-dev.db` carries a `local_thread_catalog` table with
`thread_id` and `cwd`, and `codex migrate-rollouts` shows Codex is moving
session history into it. It was rejected: it is an undocumented internal store
of a dev build whose schema has already been rebuilt by migration, whereas the
rollout header's shape held steady across 555 files spanning three months, and
its UUID is the one `codex resume --help` documents accepting.

### Invariants a test must hold to

1. Given a subagent, a desktop and an interactive record with the same `cwd`
   in the same window, only the interactive one's identifier is captured.
2. A window containing only a subagent record captures nothing.
3. A record whose `cwd` is another project captures nothing.
4. A record predating the window captures nothing.
5. Two interactive candidates are refused, not ranked.
6. A record with `session_id` but no `id` is skipped.
7. A malformed second line does not prevent a valid header being read.

---

## Harness configuration is project-local, always

### The decision

**Every piece of harness configuration Glasshouse creates is scoped to the
project it was created for.** Glasshouse never writes hooks, settings, or
launch configuration into a user-level location that follows the user to every
other repository on their machine.

This is a user directive (2026-08-25) and it settles Phase 8's open hooks
question, which had three candidate answers. The reasoning is short and
decisive:

> What if someone does not want to use Glasshouse somewhere else, and has its
> hooks still configured?

A user-level hook is a promise Glasshouse cannot keep. It fires in repositories
Glasshouse has never heard of, it outlives any decision to stop using
Glasshouse, and the person who has to notice and remove it is the user. A
project-local one is visible in the project it belongs to and dies with it.

### What this already means, and what it changes

**Claude Code already satisfies this** and did not have to change. Glasshouse
writes its hook document into a directory it owns under the project's own state
and passes `--settings <file>`, which loads *additional* settings for that one
process. `~/.claude` is never touched, so the user's own hooks keep running and
nothing survives the session.

**Codex is the case that forced the decision**, because it has no `--settings`
equivalent — no per-invocation hooks-file override exists, and a
`--strict-config` probe cannot even discriminate one, since `hooks` is a
free-form table. Its hooks must live at `<project>/.codex/hooks.json`.

That is **inside the user's repository**, so Phase 2D's rule applies without
exception: the write is a separate, explicit action that shows the exact path
first and requires its own confirmation, exactly like
`config::write_project_config_with_consent`. Cancelling leaves no file and no
directory.

### The alternative that a real uninstall argued against

The tempting answer was a Glasshouse-owned `CODEX_HOME`. It relocates Codex's
entire state root — verified — so hooks could be installed and their trust hash
pre-seeded without writing anything into the user's repository at all.

It is the wrong answer, and this machine happened to contain the proof.
**Orca — a previous multi-agent tool the user had removed — had done exactly
that.** Uninstalling it turned up `~/Library/Application Support/orca/
codex-runtime-home`, a private Codex home holding **1.2 GB and 235 session
transcripts** that the user did not know were there, stranded outside the
`~/.codex` they actually use.

That is the cost made concrete. Relocating `CODEX_HOME` also takes away the
user's auth, MCP servers, skills and model configuration, so the session is not
"the user's own installed harness" at all — which is this product's first
invariant. Copying their real home in would duplicate credentials, which the
secret boundary forbids.

### Trust is user-level, and that is fine — because it grants, it does not install

Codex trust-gates hooks by content hash, keyed by absolute path:

    [hooks.state."<abs-path>/.codex/hooks.json:<event_snake_case>:0:0"]
    trusted_hash = "sha256:<64 hex>"

Those entries live in the user's `~/.codex/config.toml`, and writing one there
is **allowed** (user decision, 2026-08-25). The rule this file states is not
"never touch a user-level file"; it is *"hooks must not end up running in every
session on a global level — only in the folder where they are actually
configured."* A trust entry cannot cause that, because of what it is:

- It **installs nothing.** It records that one file, at one absolute path, with
  one exact content hash, is trusted for one event. Hooks run only where a
  `.codex/hooks.json` actually exists, which under this design is only inside
  the project Glasshouse was asked to configure.
- It is **content-bound.** Change the file and the hash no longer matches, so
  trust has to be granted again. Glasshouse regenerating its hooks document
  cannot silently inherit an old grant.
- It is **enumerable and revocable.** Every entry names the project path it
  belongs to, so Glasshouse can find and remove its own grants.

The thing that *would* make hooks global is a different file entirely:
**`$CODEX_HOME/hooks.json`**, the user-level hooks document. That is a real
mechanism, it does apply everywhere, and it is exactly what a previous tool on
this machine was using — seven events, every session, every repository.
**Glasshouse must never write it.** That is the line, and it is not the same
line as `config.toml`.

`--dangerously-bypass-hook-trust` stays rejected regardless. It bypasses trust
for *every* enabled hook in the invocation, including project hooks the user
has never vetted, which trades a narrow scoped grant for a blanket one.

Whether Codex prompts for trust interactively is still worth knowing — a
visible viewport could simply let the user answer it, and then Glasshouse
writes nothing at all. Probe it; if it does prompt, prefer it, because the
best version of a grant is the one the user makes themselves in the harness's
own interface.

### Invariants a test must hold to

1. No Glasshouse code path writes to `~/.claude`, `~/.codex`, or any other
   user-level harness configuration.
2. A project-level write shows its exact path and requires its own
   confirmation; cancelling leaves neither file nor directory.
3. Glasshouse never writes `$CODEX_HOME/hooks.json`. That file is the global
   mechanism; a test should fail on any code path that could produce it.
4. Every trust grant Glasshouse writes is keyed by a path **inside the current
   project root**. A grant naming any other path is a defect.
5. Removing a project removes every piece of harness configuration Glasshouse
   made for it, including its trust grants in the user-level `config.toml` —
   nothing is left behind elsewhere.

---

## Approvals — Glasshouse selects a harness's own policy, and never becomes one

### The conflict

Per-command approval is a poor control and the user put the reason plainly
(2026-08-25):

> approving bash with (Y) is just stupid because no normal human reads a regex
> lookup with routing into /dev/null and a sed with 16 parameters and
> understands what's happening really.

That is correct, and it is not a preference. A prompt a person cannot evaluate
does not produce a decision; it produces a reflex, and a reflex trained to press
Y on every command is worse than no prompt at all, because it manufactures the
appearance of review. The observed behaviour follows: the first thing done in a
new session is to turn approvals off entirely.

The stated wish was that Claude Code's auto-mode classifier "should be the case
for all harnesses".

### Why Glasshouse must not build that classifier

The capability map forbids it, in three places and one of them is a **fixed
architectural requirement**:

- Phase 5: *"Native commands, permission flows, model controls, compaction,
  resume behavior, and tool interfaces remain owned by the harness."*
- Phase 5, already checked: *"Allow native permission prompts to remain
  interactive."*
- Phase 9G: Glasshouse *"must not acquire an autonomous coding loop, repository
  tool surface, permission system, or compaction system."*

Glasshouse also has no seam to put a classifier in. It owns a pseudo-terminal,
not the harness's tool dispatch. The only interception point it has is the
hooks it installs — `PreToolUse` / `pre_tool_use`, where a non-zero exit is a
veto — and using that as a decision engine would be precisely "acquiring a
permission system", with Glasshouse answering for commands it did not generate
and cannot see the context of.

### The decision: declare the modes, select one, default to automatic review

The wish turns out not to need the deviation, because **the harnesses already
ship the mechanism**. Read from the installed binaries on 2026-08-25:

| Harness | Automatic review | Blanket bypass | Sandbox |
|---|---|---|---|
| Claude Code 2.1.245 | auto mode (`auto-mode` subcommand inspects/resets the classifier) | `--dangerously-skip-permissions`, `--permission-mode bypassPermissions` | — |
| Codex 0.149.1 | `--approve-for-me` — "route approval requests through automatic review using the workspace-write sandbox" | `--dangerously-bypass-approvals-and-sandbox`, `-a never` | `-s read-only\|workspace-write\|danger-full-access` |
| Cursor CLI | `--auto-review` — "Smart Auto: a server classifier auto-runs safe tool calls and prompts for the rest" | `--yolo` (alias for `--force`) | `--sandbox <mode>` |
| OpenCode 1.18.22 | none | `--auto` — "auto-approve permissions that are not explicitly denied (dangerous!)" | — |
| Hermes 0.15.1 | none | `--yolo` | — |
| Antigravity 1.1.20 | none | `--dangerously-skip-permissions` | `--sandbox` |
| Pi 0.73.1 | unverified — not on `PATH` on this machine | unverified | unverified |

So:

1. **Each adapter declares its approval modes**, as `Declared<T>` like every
   other harness fact, including whether a native automatic-review mode exists.
   Three of seven have one; four do not, and the honest declaration says so
   rather than implying parity.
2. **A launch profile selects one**, through the child-process argument overlay
   Phase 9A already defines. No new mechanism.
3. **The default is the harness's automatic-review mode where one exists.**
   Blanket bypass remains available as a profile the user picks deliberately,
   never as what happens by default.

The permission flow stays owned by the harness, so the fixed requirement holds
and line 267 stays true: when a mode does prompt, the prompt is still the
harness's own and still interactive in the viewport.

### Why the default is automatic review and not bypass

They are not the same thing and the difference is the whole point. Claude
Code's auto mode is a *classifier* — it blocked an attempt to spawn a
`--dangerously-skip-permissions` process during the very session this decision
was written in. `--yolo` classifies nothing. Codex's `--approve-for-me` is
additionally sandbox-bounded to workspace-write, so it is strictly narrower
than the bypass flag it replaces.

Defaulting to bypass would also make Glasshouse the thing that silently
widened a user's blast radius, which is not a default any tool should choose on
someone's behalf even when that user would have chosen it themselves.

### What this costs, and the workaround the user chose

"All harnesses" is a promise Glasshouse cannot keep uniformly. For OpenCode,
Hermes and Antigravity the closest available mode is a blanket bypass, which is
not automatic review and must never be described as though it were.

The user settled what happens then (2026-08-25): **bypass is an acceptable
workaround, provided they are told the risk the first time and then never
again.**

> "I'm fine if they bypass as a workaround, that's totally okay by me. User
> should be notified about risk first time they approve this, then all good."

So the rule is *informed*, not *forbidden*:

- The default is still the harness's automatic-review mode, and never a bypass.
- On a harness that declares no automatic review, Glasshouse may use its bypass
  — but the first time, it shows what that means in that harness's own terms
  (OpenCode's `--auto` is "auto-approve permissions that are not explicitly
  denied"; Hermes's `--yolo` "bypasses all dangerous command approval prompts";
  Antigravity's `--dangerously-skip-permissions` "auto-approves all tool
  permission requests without prompting") and takes an explicit acknowledgement.
- That acknowledgement is **recorded per harness**, so it is asked once and not
  again. Nagging a user who has already decided is how a warning becomes
  noise, and noise is what made per-command approval useless in the first place.
- A silent downgrade remains forbidden. The failure this guards against is a
  user believing a session is being classified when it is not.

The asymmetry is deliberate: the *warning* is once, because it is a decision
about a harness's nature; the *per-command prompt* was every time, which is
exactly why nobody read it.

### Invariants a test must hold to

1. Every adapter's declared approval modes are `Verified` with a citation, or
   `Unverified`. No bare defaults.
2. A launch profile requesting automatic review on a harness that declares none
   is refused, not downgraded to bypass.
3. The default profile never selects a blanket-bypass flag.
4. Glasshouse contains no code that decides whether a harness's tool call is
   permitted. The hook adapters report lifecycle; they do not veto.

---

## Codex lifecycle hooks — a second destination, and a payload not to read

### The conflict

Phase 7 gave Claude Code hooks a clean shape: the adapter builds a settings
document, Glasshouse writes it into a directory **it owns**, and `--settings`
points the harness at it. Nothing of the user's is touched and nothing survives
the session.

Codex has no `--settings`. It reads hooks from exactly one place —
`<project>/.codex/hooks.json` — which is **inside the user's repository**. The
mechanism that made Phase 7 clean does not exist here.

### The decision: `HookInstallation` gains a destination

`HookInstallation` currently means "write this file somewhere Glasshouse owns
and pass these arguments". That is one of two real cases, so the type says which:

    pub enum HookDestination {
        /// A directory Glasshouse owns; the harness is pointed at it by the
        /// installation's own arguments. Nothing of the user's is touched.
        GlasshouseOwned,
        /// Inside the user's project, at this relative path, because the
        /// harness reads hooks from nowhere else. Requires explicit consent.
        ProjectLocal { relative_path: &'static str },
    }

Claude Code keeps `GlasshouseOwned`; Codex declares
`ProjectLocal { relative_path: ".codex/hooks.json" }` and empty arguments,
because Codex finds the file by itself.

Making the destination part of the declaration rather than a special case in
core is what keeps the consent rule enforceable in one place: **core refuses to
write a `ProjectLocal` installation unless consent has been given**, and no
adapter can quietly opt out of that.

### The decision: consent is configuration, not a per-session prompt

Phase 2D requires that writing inside the user's repository shows the exact path
and takes a distinct confirmation. A modal prompt in front of every session
start would be the wrong shape — a session is started to be used, not to be
interrogated.

So consent is an explicit setting the user turns on once, and the first write
logs the exact path it created. Absent that setting, Glasshouse installs no
Codex hooks and the session runs without them, which is a working session with
less telemetry rather than a broken one.

### The decision: report five events, and mind the clamp

`SessionStart`, `UserPromptSubmit`, `PermissionRequest`, `Stop`, `SessionEnd`.
Deliberately not `PreToolUse`/`PostToolUse`, which fire many times per turn and
say nothing about a *session's* state — the same reasoning Phase 7 applied.

**`SessionStart` fires for Codex**, which it does not for Claude Code 2.1.245.
The two harnesses genuinely differ, and `session/lifecycle.rs` is the one place
allowed to know that.

Codex **clamps hook timeouts** — it announced `clamping SessionEnd hook timeout
to 3s` about a declared 10s. Declare 3s and the clamp never fires, so a real
installation produces no warning the user has to wonder about.

### The decision: the payload is read for two fields and no more

Every Codex hook payload carries `session_id`, `transcript_path`, `cwd`,
`hook_event_name`, `model` and `permission_mode`. `UserPromptSubmit` adds
`prompt`; `Stop` adds `last_assistant_message`.

Those last two are **the conversation itself** — the user's words and the
model's reply. Glasshouse needs the event name and the session identifier and
has no business with the rest. The handler must drain stdin and discard it, and
a test must prove no payload field reaches a log, a diagnostic, a `Debug`, or
the database — the same way `nothing_is_read_past_the_first_line` guards
rollouts.

Draining matters mechanically too, not only ethically: a hook that never reads
its stdin can leave the harness writing into a closed pipe.

### The consequence: two identifier sources, and the weaker one stays

`session_id` is in every payload, so a hook hands Glasshouse the native
identifier directly — no originator filtering, no subagent exclusion, no time
window, no ambiguity, and `transcript_path` even names the exact rollout. It is
strictly better information than Phase 8 line 2's rollout discovery produces.

Line 2 stays regardless. Hooks require installation *and* the user's consent
*and* the user trusting them in Codex's own review prompt; discovery requires
none of those and still works for a session that predates the hooks. So:
**prefer a reported `session_id`, fall back to discovery.** A capability that
degrades to a working fallback is worth more than one that is merely elegant.

### Invariants a test must hold to

1. A `ProjectLocal` installation is not written without consent, and cancelling
   leaves neither file nor directory.
2. No Glasshouse code path writes `$CODEX_HOME/hooks.json`.
3. No hook payload field other than the event name and session identifier
   reaches a log, diagnostic, `Debug`, or the database.
4. An unfamiliar event changes nothing, and a late hook cannot revive a
   finished session.
5. A hook always exits 0.
6. A reported `session_id` wins over a discovered one; with no hook, discovery
   still captures the identifier.
